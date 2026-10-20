-- Meshvault exporter, schema 2: mesh/material + rig + animation metadata as JSON.
-- Served by Meshvault (see its "Export from Roblox Studio" dialog); run in the Command Bar, in
-- Edit mode. Exports metadata, NOT mesh geometry or texture files. Animation assets must be
-- accessible to the current Studio session.
--
-- It returns a function. Called without options it exports the current selection, or, with
-- nothing selected, the whole game except Workspace. Options:
--   roots   = { instances to export }            (default: the selection, or { game })
--   exclude = { instances to leave out, with everything beneath them }
--             (default: { Workspace } when exporting the whole game, otherwise nothing)
return function(options)
options = options or {}
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
    end
end
local excludedRoots = options.exclude
    or ((options.roots or useSelection) and {} or { game:GetService("Workspace") })
if options.roots or useSelection then
    local names = {}
    for i, root in ipairs(roots) do
        if i > 3 then table.insert(names, "and " .. (#roots - 3) .. " more") break end
        table.insert(names, root:GetFullName())
    end
    print("Meshvault: exporting " .. table.concat(names, ", "))
else
    print("Meshvault: exporting the whole game except Workspace")
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

local function getClip(animationId)
    if clipCache[animationId] then return clipCache[animationId] end
    local item = { Status = "Unavailable", Duration = 0, Keyframes = {}, Curves = {} }
    clipCache[animationId] = item
    local ok, clip = pcall(function()
        return AnimationClipProvider:GetAnimationClipAsync(animationId)
    end)
    if not ok or not clip then
        warn("Cannot load animation " .. animationId .. ": " .. tostring(clip))
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
            warn("Cannot export CurveAnimation " .. animationId .. ": " .. tostring(exportedCurves))
        end
    else
        item.Status = "UnsupportedClipType"
    end
    return item
end
local function exportRig(model)
    local m = {Name=model.Name, Path=fullpath(model), Scale=model:GetScale(), Parts={}, Joints={}, Bones={}, Animations={}}
    local animSeen = {}
    for _, inst in ipairs(model:GetDescendants()) do
        if isExcluded(inst) then continue end
        if inst:IsA("BasePart") then
            local meshRef = getOrAddMesh(inst)
            if not meshRef then
                local sm = inst:FindFirstChildOfClass("SpecialMesh")
                if sm then meshRef = getOrAddMesh(sm) end
            end
            -- A FileMesh has its own scale/offset, independent of its parent Part.
            -- Other SpecialMesh types also affect the visible shape.
            local sm = inst:FindFirstChildOfClass("SpecialMesh")
            local specialMesh = nil
            if sm then
                specialMesh = {
                    MeshType = sm.MeshType.Name,
                    Scale = v3(sm.Scale),
                    Offset = v3(sm.Offset),
                }
            end
            local shape = ""
            if inst:IsA("Part") then
                shape = inst.Shape.Name
            elseif inst:IsA("WedgePart") then
                shape = "Wedge"
            elseif inst:IsA("CornerWedgePart") then
                shape = "CornerWedge"
            elseif inst:IsA("TrussPart") then
                shape = "Truss"
            end
            table.insert(m.Parts, {
                Path=fullpath(inst), Name=inst.Name, MeshRef=meshRef or 0,
                ClassName=inst.ClassName, Shape=shape,
                CFrame=cfdata(inst.CFrame), Size=v3(inst.Size),
                Color={inst.Color.R, inst.Color.G, inst.Color.B},
                BrickColor=inst.BrickColor.Name,
                Material=inst.Material.Name,
                Transparency=inst.Transparency,
                Reflectance=inst.Reflectance,
                SpecialMesh=specialMesh,
            })
        elseif inst:IsA("Motor6D") then
            table.insert(m.Joints, {
                Name=inst.Name, Path=fullpath(inst),
                Part0=inst.Part0 and fullpath(inst.Part0) or "",
                Part1=inst.Part1 and fullpath(inst.Part1) or "",
                C0=cfdata(inst.C0), C1=cfdata(inst.C1),
                Transform=cfdata(inst.Transform),
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
            if meshIdOf(inst) then getOrAddMesh(inst) end
            if inst:IsA("Animation") then
                local model = closestModel(inst)
                if model and not modelSeen[model] then
                    modelSeen[model] = true
                    table.insert(models, exportRig(model))
                end
            end
        end
    end
end
for _, root in ipairs(roots) do
    if root:IsA("Model") and not modelSeen[root] and not isExcluded(root) then
        modelSeen[root] = true
        table.insert(models, exportRig(root))
    end
end
local payload = {SchemaVersion=2, Meshes=meshes, Models=models}
local json = HttpService:JSONEncode(payload)

-- Send the export to Meshvault, which saves it and opens it for import. HttpEnabled is
-- switched on only while sending and put back afterwards.
local MESHVAULT_URL = "__MESHVAULT_URL__"
local SEND_CHUNK = 2 * 1024 * 1024
local function sendToMeshvault(text)
    local wasEnabled = HttpService.HttpEnabled
    local ok, err = pcall(function()
        HttpService.HttpEnabled = true
        local id = HttpService:GenerateGUID(false)
        local count = math.max(1, math.ceil(#text / SEND_CHUNK))
        for i = 1, count do
            local res = HttpService:RequestAsync({
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
            if not res.Success then
                error(("Meshvault replied %d: %s"):format(res.StatusCode, res.Body))
            end
        end
    end)
    pcall(function() HttpService.HttpEnabled = wasEnabled end)
    return ok, err
end

if #meshes == 0 and #models == 0 then
    warn("Meshvault: nothing to export: no meshes or animated models in "
        .. ((options.roots or useSelection) and "what was picked" or "the game outside Workspace")
        .. ". Select a model (or anything inside one) in the Explorer and run the line again.")
    return
end
local sent, sendError = sendToMeshvault(json)
if sent then
    print(("Meshvault: sent %.1f MB. It opens in Meshvault for import."):format(#json / 1048576))
else
    warn("Meshvault: could not send the export (" .. tostring(sendError) .. "); printing it in chunks instead.")
    local CHUNK_SIZE = 12000
    local count = math.max(1, math.ceil(#json / CHUNK_SIZE))
    for i = 1, count do
        print(("=== JSON CHUNK %d/%d ==="):format(i, count))
        print(json:sub((i - 1) * CHUNK_SIZE + 1, math.min(i * CHUNK_SIZE, #json)))
    end
    print("=== END JSON EXPORT ===")
end
end
