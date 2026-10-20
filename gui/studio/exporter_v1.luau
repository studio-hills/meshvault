-- Meshvault exporter, schema 1: mesh/material + rig + animation metadata as JSON.
-- Served by Meshvault (see its "Export from Roblox Studio" dialog); run in the Command Bar, in
-- Edit mode. Exports metadata, NOT mesh geometry or texture files. Animation assets must be
-- accessible to the current Studio session.
--
-- It returns a function. Called without options it exports the current selection, or, with
-- nothing selected, the whole game except Workspace. Options:
--   roots   = { instances to export }            (default: the selection, or { game })
--   exclude = { instances to leave out, with everything beneath them }
--             (default: { Workspace } when exporting the whole game, otherwise nothing)
--
-- Every line it writes to the Output window starts with [Meshvault]; problems are warnings.
-- It returns the problems as a list (empty when there were none), e.g.
--   local issues = loadstring(source)()()
--   for _, i in issues do print(i.Kind, i.Message) end

local SCHEMA = 1
local MESHVAULT_VERSION = "__MESHVAULT_VERSION__"
local TAG = "[Meshvault]"
local MAX_MODEL_LINES = 25

-- What went wrong along the way: each is warned as it happens, and the function returns
-- them all, as { Kind = "...", Message = "...", Path = "..."?, AnimationId = "..."? }.
local issues = {}
local function log(text)
    print(TAG .. " " .. text)
end
local function issue(kind, text, details)
    local entry = { Kind = kind, Message = text }
    for k, v in pairs(details or {}) do entry[k] = v end
    table.insert(issues, entry)
    warn(TAG .. " " .. text)
end

-- "1 part", "2 parts"
local function count(n, one, many)
    return n .. " " .. (n == 1 and one or many or one .. "s")
end

local function run(options)
local started = os.clock()
log(("exporter for schema %d, served by Meshvault %s"):format(SCHEMA, MESHVAULT_VERSION))
local selection = game:GetService("Selection"):Get()
local useSelection = options.roots == nil and #selection > 0
local roots = options.roots or { game }
if useSelection then
    -- something selected inside a model (a part, its Animator, a joint) stands for the model
    roots = {}
    local picked = {}
    for _, inst in ipairs(selection) do
        local root = inst:IsA("Model") and inst or inst:FindFirstAncestorOfClass("Model") or inst
        if not picked[root] then
            picked[root] = true
            table.insert(roots, root)
        end
        if root ~= inst then
            log(("%s is inside %s, so the whole model is exported"):format(inst:GetFullName(), root:GetFullName()))
        end
    end
end
local excludedRoots = options.exclude
    or ((options.roots or useSelection) and {} or { game:GetService("Workspace") })
if game:GetService("RunService"):IsRunning() then
    issue("PlayMode", "the game is running (Play mode): joints are exported in their current animated pose, "
        .. "and Meshvault cannot be reached from a running game. Stop the test and run the line in Edit mode")
end
if options.roots or useSelection then
    local names = {}
    for i, root in ipairs(roots) do
        if i > 3 then table.insert(names, "and " .. (#roots - 3) .. " more") break end
        table.insert(names, root:GetFullName())
    end
    log("exporting " .. table.concat(names, ", ") .. (useSelection and " (from the selection)" or ""))
else
    log("nothing selected: exporting the whole game except Workspace. This can take a while")
end
if #excludedRoots > 0 then
    local names = {}
    for _, excluded in ipairs(excludedRoots) do table.insert(names, excluded:GetFullName()) end
    log("leaving out " .. table.concat(names, ", "))
end
local excludedSet = {}
for _, excluded in ipairs(excludedRoots) do
    if excluded then excludedSet[excluded] = true end
end

local function isExcluded(inst)
    local current = inst
    while current do
        if excludedSet[current] then return true end
        current = current.Parent
    end
    return false
end
local HttpService = game:GetService("HttpService")
local AnimationClipProvider = game:GetService("AnimationClipProvider")

local meshes, meshByKey, models, modelSeen = {}, {}, {}, {}
local clipCache = {}
local function nonempty(v) return type(v) == "string" and v ~= "" and v or nil end
local function fullpath(inst)
    local names = {}
    while inst and inst ~= game do
        table.insert(names, 1, inst.Name)
        inst = inst.Parent
    end
    return table.concat(names, "/")
end
local function cfdata(cf)
    return {cf:GetComponents()} -- position XYZ then 3x3 rotation (row major)
end
local function v3(v) return {v.X, v.Y, v.Z} end
local function getMaps(inst)
    local sa = inst:FindFirstChildOfClass("SurfaceAppearance")
    if not sa and inst:IsA("SpecialMesh") and inst.Parent then
        sa = inst.Parent:FindFirstChildOfClass("SurfaceAppearance")
    end
    local maps = {ColorMap="", NormalMap="", RoughnessMap="", MetalnessMap=""}
    if sa then
        for k in pairs(maps) do
            local ok, val = pcall(function() return sa[k] end)
            if ok and nonempty(val) then maps[k] = val end
        end
    end
    local tex
    if inst:IsA("MeshPart") then
        local ok, id = pcall(function() return inst.TextureID end)
        if ok then tex = nonempty(id) end
    elseif inst:IsA("SpecialMesh") then
        tex = nonempty(inst.TextureId)
    end
    tex = nonempty(maps.ColorMap) or tex
    if not tex then
        local owners = {inst}
        if inst:IsA("SpecialMesh") and inst.Parent then table.insert(owners, inst.Parent) end
        for _, owner in ipairs(owners) do
            for _, ch in ipairs(owner:GetChildren()) do
                if ch:IsA("Decal") or ch:IsA("Texture") then
                    tex = nonempty(ch.Texture)
                    if tex then break end
                end
            end
            if tex then break end
        end
    end
    return tex or "", maps
end
local function meshIdOf(inst)
    if inst:IsA("MeshPart") then
        local ok, id = pcall(function() return inst.MeshId end)
        return ok and nonempty(id) or nil
    end
    if inst:IsA("SpecialMesh") and inst.MeshType == Enum.MeshType.FileMesh then
        return nonempty(inst.MeshId)
    end
    return nil
end
local function getOrAddMesh(inst)
    local id = meshIdOf(inst)
    if not id then return nil end
    local tex, maps = getMaps(inst)
    local key = HttpService:JSONEncode({id, tex, maps.ColorMap, maps.NormalMap, maps.RoughnessMap, maps.MetalnessMap})
    if not meshByKey[key] then
        local newId = #meshes + 1
        meshByKey[key] = newId
        table.insert(meshes, {
            Id = newId, Name = inst.Name, MeshId = id, TextureId = tex,
            Folder = fullpath(inst.Parent),
            ColorMap = maps.ColorMap, NormalMap = maps.NormalMap,
            RoughnessMap = maps.RoughnessMap, MetalnessMap = maps.MetalnessMap,
        })
    end
    return meshByKey[key]
end
local function exportPose(pose)
    local data = {
        Name = pose.Name,
        Transform = cfdata(pose.CFrame),
        Weight = pose.Weight,
        EasingStyle = pose.EasingStyle.Name,
        EasingDirection = pose.EasingDirection.Name,
        Children = {},
    }
    for _, ch in ipairs(pose:GetChildren()) do
        if ch:IsA("Pose") then table.insert(data.Children, exportPose(ch)) end
    end
    return data
end
-- CurveAnimation records are nested by folder path (rig hierarchy), then channel.
-- Export raw keys, modes, tangents; the viewer must evaluate curves at playback time.
local function exportScalarKeys(curve)
    local keys = {}
    for _, key in ipairs(curve:GetKeys()) do
        table.insert(keys, {
            Time = key.Time,
            Value = key.Value,
            Interpolation = key.Interpolation.Name,
            LeftTangent = key.LeftTangent or 0,
            RightTangent = key.RightTangent or 0,
        })
    end
    return keys
end
local function exportRotationKeys(curve)
    local keys = {}
    for _, key in ipairs(curve:GetKeys()) do
        table.insert(keys, {
            Time = key.Time,
            Value = cfdata(key.Value),
            Interpolation = key.Interpolation.Name,
            LeftTangent = key.LeftTangent or 0,
            RightTangent = key.RightTangent or 0,
        })
    end
    return keys
end
local function exportCurveChannel(curve)
    if curve:IsA("FloatCurve") then
        return {Type="FloatCurve", Keys=exportScalarKeys(curve)}
    elseif curve:IsA("RotationCurve") then
        return {Type="RotationCurve", Keys=exportRotationKeys(curve)}
    elseif curve:IsA("Vector3Curve") or curve:IsA("EulerRotationCurve") then
        local value = {
            Type=curve.ClassName,
            Channels={},
        }
        if curve:IsA("EulerRotationCurve") then
            value.RotationOrder = curve.RotationOrder.Name
        end
        -- Inspect existing children; calling :X()/:Y()/:Z() creates missing channels.
        for _, axis in ipairs({"X", "Y", "Z"}) do
            local child = curve:FindFirstChild(axis)
            if child and child:IsA("FloatCurve") then
                value.Channels[axis] = exportScalarKeys(child)
            end
        end
        return value
    end
    return nil
end
local function exportCurveClip(clip)
    local channels = {}
    local duration = 0
    local function trackKeys(keys)
        for _, key in ipairs(keys) do
            duration = math.max(duration, key.Time)
        end
    end
    local function walk(parent, path)
        for _, child in ipairs(parent:GetChildren()) do
            if child:IsA("Folder") then
                local folderPath = table.clone(path)
                table.insert(folderPath, child.Name)
                walk(child, folderPath)
            else
                local exported = exportCurveChannel(child)
                if exported then
                    if exported.Keys then
                        trackKeys(exported.Keys)
                    end
                    if exported.Channels then
                        for _, keys in pairs(exported.Channels) do
                            trackKeys(keys)
                        end
                    end
                    table.insert(channels, {
                        Path = table.concat(path, "/"),
                        Name = child.Name,
                        Curve = exported,
                    })
                end
            end
        end
    end
    walk(clip, {})
    table.sort(channels, function(a,b)
        return (a.Path .. "/" .. a.Name) < (b.Path .. "/" .. b.Name)
    end)
    return channels, duration
end

local CLIP_TIMEOUT = 20
local function loadClip(animationId)
    local done, ok, clip = false, false, nil
    task.spawn(function()
        ok, clip = pcall(function()
            return AnimationClipProvider:GetAnimationClipAsync(animationId)
        end)
        done = true
    end)
    local waitStarted, told = os.clock(), false
    while not done do
        local waited = os.clock() - waitStarted
        if waited > CLIP_TIMEOUT then
            local why = ("it did not load within %d seconds"):format(CLIP_TIMEOUT)
            if not animationId:find("^rbxassetid://") and not animationId:find("^%d+$") then
                -- RegisterAnimationClip ids are content hashes, valid in the session that made them
                why ..= ". It is a locally registered clip: register it again (RegisterAnimationClip) "
                    .. "in this Studio session, and if it still will not load, restart Studio"
            end
            return false, why
        end
        if waited > 3 and not told then
            told = true
            log("  still loading the animation " .. animationId .. "...")
        end
        task.wait(0.1)
    end
    return ok, clip
end
local function getClip(animationId)
    if clipCache[animationId] then return clipCache[animationId] end
    local item = { Status = "Unavailable", Duration = 0, Keyframes = {}, Curves = {} }
    clipCache[animationId] = item
    local ok, clip = loadClip(animationId)
    if not ok or not clip then
        issue("AnimationUnavailable", "could not load the animation " .. animationId .. ": " .. tostring(clip), { AnimationId = animationId })
        return item
    end
    if clip:IsA("KeyframeSequence") then
        item.Status = "KeyframeSequence"
        item.Loop = clip.Loop
        item.Priority = clip.Priority.Name
        local frames = clip:GetKeyframes()
        table.sort(frames, function(a,b) return a.Time < b.Time end)
        for _, frame in ipairs(frames) do
            local poses, markers = {}, {}
            for _, p in ipairs(frame:GetPoses()) do
                table.insert(poses, exportPose(p))
            end
            for _, m in ipairs(frame:GetMarkers()) do
                table.insert(markers, {Name=m.Name, Value=m.Value})
            end
            table.insert(item.Keyframes, {Time=frame.Time, Poses=poses, Markers=markers})
            item.Duration = math.max(item.Duration, frame.Time)
        end
    elseif clip:IsA("CurveAnimation") then
        local curveOk, exportedCurves, duration = pcall(exportCurveClip, clip)
        if curveOk then
            item.Status = "CurveAnimation"
            item.Loop = clip.Loop
            item.Priority = clip.Priority.Name
            item.Curves = exportedCurves
            item.Duration = duration
        else
            item.Status = "CurveAnimationExportFailed"
            issue("CurveAnimationFailed", "could not read the CurveAnimation " .. animationId .. ": " .. tostring(exportedCurves), { AnimationId = animationId })
        end
    else
        item.Status = "UnsupportedClipType"
    end
    return item
end
local function exportRig(model)
    local m = {Name=model.Name, Path=fullpath(model), Parts={}, Joints={}, Bones={}, Animations={}}
    local animSeen = {}
    for _, inst in ipairs(model:GetDescendants()) do
        if isExcluded(inst) then continue end
        if inst:IsA("BasePart") then
            local meshRef = getOrAddMesh(inst)
            if not meshRef then
                local sm = inst:FindFirstChildOfClass("SpecialMesh")
                if sm then meshRef = getOrAddMesh(sm) end
            end
            table.insert(m.Parts, {
                Path=fullpath(inst), Name=inst.Name, MeshRef=meshRef or 0,
                CFrame=cfdata(inst.CFrame), Size=v3(inst.Size),
                Transparency=inst.Transparency,
            })
        elseif inst:IsA("Motor6D") then
            table.insert(m.Joints, {
                Name=inst.Name, Path=fullpath(inst),
                Part0=inst.Part0 and fullpath(inst.Part0) or "",
                Part1=inst.Part1 and fullpath(inst.Part1) or "",
                C0=cfdata(inst.C0), C1=cfdata(inst.C1),
            })
        elseif inst:IsA("Bone") then
            table.insert(m.Bones, {
                Name=inst.Name, Path=fullpath(inst),
                ParentPath=inst.Parent and fullpath(inst.Parent) or "",
                CFrame=cfdata(inst.CFrame),
            })
        elseif inst:IsA("Animation") then
            local animId = nonempty(inst.AnimationId)
            if animId and not animSeen[animId] then
                animSeen[animId] = true
                local clip = getClip(animId)
                table.insert(m.Animations, {
                    Name=inst.Name, AnimationId=animId,
                    Status=clip.Status, Duration=clip.Duration,
                    Loop=clip.Loop or false, Priority=clip.Priority or "",
                    Keyframes=clip.Keyframes, Curves=clip.Curves,
                })
            end
        end
    end
    return m
end
local modelLines = 0
local function addModel(model)
    local ok, m = pcall(exportRig, model)
    if not ok then
        issue("ModelSkipped", ("skipped the model %s: %s"):format(model:GetFullName(), tostring(m)),
            { Path = model:GetFullName() })
        return
    end
    table.insert(models, m)
    modelLines += 1
    if modelLines > MAX_MODEL_LINES then
        if modelLines == MAX_MODEL_LINES + 1 then
            log("  (more models are exported; only the first " .. MAX_MODEL_LINES .. " are listed)")
        end
        return
    end
    local clips = {}
    for _, a in ipairs(m.Animations) do
        if a.Status == "KeyframeSequence" or a.Status == "CurveAnimation" then
            table.insert(clips, ("%s %.2fs"):format(a.Name, a.Duration))
        else
            table.insert(clips, ("%s (%s)"):format(a.Name, a.Status))
        end
    end
    log(("  model %s: %s, %s, %s, %s"):format(
        m.Path, count(#m.Parts, "part"), count(#m.Joints, "joint"), count(#m.Bones, "bone"),
        #clips > 0 and ("clips " .. table.concat(clips, ", ")) or "no clips"))
end
local function closestModel(inst)
    local candidate = inst:FindFirstAncestorOfClass("Model")
    return candidate
end
-- Mesh/material deduplication is global. Instances within rigs are NOT deduplicated.
-- GetDescendants still enumerates the tree, but excluded branches are not exported.
for _, root in ipairs(roots) do
    if root and not isExcluded(root) then
        local list = root:GetDescendants()
        table.insert(list, 1, root)
        for _, inst in ipairs(list) do
            if isExcluded(inst) then continue end
            if meshIdOf(inst) then
                -- one odd instance should not stop the whole export
                local ok, err = pcall(getOrAddMesh, inst)
                if not ok then
                    issue("MeshSkipped", ("skipped the mesh of %s: %s"):format(inst:GetFullName(), tostring(err)),
                        { Path = inst:GetFullName() })
                end
            end
            if inst:IsA("Animation") then
                local model = closestModel(inst)
                if model and not modelSeen[model] then
                    modelSeen[model] = true
                    addModel(model)
                end
            end
        end
    end
end
for _, root in ipairs(roots) do
    if root:IsA("Model") and not modelSeen[root] and not isExcluded(root) then
        modelSeen[root] = true
        addModel(root)
    end
end
local payload = {SchemaVersion=1, Meshes=meshes, Models=models}
local clipsOk, clipsFailed = 0, 0
for _, clip in pairs(clipCache) do
    if clip.Status == "KeyframeSequence" or clip.Status == "CurveAnimation" then
        clipsOk += 1
    else
        clipsFailed += 1
    end
end
log(("found %s, %s and %s%s in %.1fs"):format(
    count(#models, "model"), count(#meshes, "mesh", "meshes"), count(clipsOk, "clip"), clipsFailed > 0 and (" (" .. clipsFailed .. " could not be read)") or "",
    os.clock() - started))
if #meshes == 0 and #models == 0 then
    issue("NothingToExport", "nothing to export: no meshes or animated models in "
        .. ((options.roots or useSelection) and "what was picked" or "the game outside Workspace")
        .. ". Select a model (or anything inside one) in the Explorer and run the line again")
    return
end
local encoded, json = pcall(function() return HttpService:JSONEncode(payload) end)
if not encoded then
    error("could not turn the export into JSON: " .. tostring(json), 0)
end

-- Send the export to Meshvault, which saves it and opens it for import. HttpEnabled is
-- switched on only while sending and put back afterwards.
local MESHVAULT_URL = "__MESHVAULT_URL__"
local SEND_CHUNK = 2 * 1024 * 1024

-- Roblox's HTTP errors, with what to do about them
local function explain(err)
    err = tostring(err)
    if err:find("ConnectFail") then
        return err .. ": Meshvault is not running, or its Studio link is off (see its status bar)"
    elseif err:find("game server") then
        return err .. ": run the line in the Command Bar in Edit mode, not during a play test"
    elseif err:find("Timedout") or err:find("Timeout") then
        return err .. ": Meshvault did not answer in time; is it busy or not responding?"
    elseif err:find("not enabled") or err:find("HttpEnabled") then
        return err .. ": HttpService.HttpEnabled could not be switched on from here"
    end
    return err
end

local function sendToMeshvault(text)
    local wasEnabled = HttpService.HttpEnabled
    local reply
    local ok, err = pcall(function()
        local switched, why = pcall(function() HttpService.HttpEnabled = true end)
        if not switched then
            error("could not switch HttpService.HttpEnabled on (" .. tostring(why) .. ")", 0)
        end
        local id = HttpService:GenerateGUID(false)
        local count = math.max(1, math.ceil(#text / SEND_CHUNK))
        local size = #text >= 1048576 and ("%.1f MB"):format(#text / 1048576)
            or ("%d KB"):format(math.ceil(#text / 1024))
        log(("sending %s to Meshvault at %s%s"):format(
            size, MESHVAULT_URL, count > 1 and (" in " .. count .. " pieces") or ""))
        for i = 1, count do
            local sentOk, res = pcall(function()
                return HttpService:RequestAsync({
                    Url = MESHVAULT_URL,
                    Method = "POST",
                    Headers = {
                        ["Content-Type"] = "application/json",
                        ["X-Meshvault-Export"] = id,
                        ["X-Meshvault-Chunk"] = i .. "/" .. count,
                        ["X-Meshvault-Name"] = game.Name,
                    },
                    Body = text:sub((i - 1) * SEND_CHUNK + 1, math.min(i * SEND_CHUNK, #text)),
                })
            end)
            if not sentOk then
                error(("piece %d of %d did not reach Meshvault: %s"):format(i, count, explain(res)), 0)
            end
            if not res.Success then
                error(("Meshvault refused piece %d of %d (HTTP %d): %s"):format(
                    i, count, res.StatusCode, tostring(res.Body)), 0)
            end
            if count > 1 then
                log(("  sent piece %d of %d"):format(i, count))
            end
            reply = res.Body
        end
    end)
    local restored = pcall(function() HttpService.HttpEnabled = wasEnabled end)
    if not restored then
        issue("HttpEnabledNotRestored", "could not switch HttpService.HttpEnabled back to " .. tostring(wasEnabled)
            .. "; check it in Game Settings > Security")
    end
    return ok, ok and reply or err
end

local sent, result = sendToMeshvault(json)
if sent then
    log(("done in %.1fs. Meshvault received the export: %s. Finish the import in Meshvault")
        :format(os.clock() - started, tostring(result)))
else
    issue("SendFailed", "could not send the export to Meshvault: " .. tostring(result))
    log("printing the export here instead: copy the chunks below into a .json file and import it in Meshvault")
    local CHUNK_SIZE = 12000
    local count = math.max(1, math.ceil(#json / CHUNK_SIZE))
    for i = 1, count do
        print(("=== JSON CHUNK %d/%d ==="):format(i, count))
        print(json:sub((i - 1) * CHUNK_SIZE + 1, math.min(i * CHUNK_SIZE, #json)))
    end
    print("=== END JSON EXPORT ===")
end
end

return function(options)
    issues = {}
    local ok, err = xpcall(run, function(e)
        return debug.traceback(tostring(e), 2)
    end, options or {})
    if not ok then
        issue("Error", "the export stopped with an error: " .. tostring(err))
        warn(TAG .. " please report this, with the lines above, to Meshvault's developers "
            .. "(exporter schema " .. SCHEMA .. ", Meshvault " .. MESHVAULT_VERSION .. ")")
    elseif #issues > 0 then
        warn(("%s finished with %d warning(s); see above"):format(TAG, #issues))
    end
    -- empty when all went well
    return issues
end
