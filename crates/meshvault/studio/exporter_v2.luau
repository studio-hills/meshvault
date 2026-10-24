-- Meshvault exporter (schema 2).
-- Run in the Roblox Studio Command Bar in Edit mode.
-- Exports mesh, material, rig, and animation metadata (not geometry or textures).
-- Options: roots (defaults to selection or game), exclude (defaults to Workspace for
-- whole-game exports), send = false (return data without sending).
-- Returns: issues, export, send(export).

local SCHEMA = 2
local MESHVAULT_VERSION = "__MESHVAULT_VERSION__"
local TAG = "[Meshvault]"

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

local HttpService = game:GetService("HttpService")

local MESHVAULT_URL = "__MESHVAULT_URL__"
local SEND_CHUNK = 2 * 1024 * 1024

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

-- The game's name: its published name, else the place's name in Studio, else its ID.
local cachedGameName
local function gameName()
	if cachedGameName then return cachedGameName end
	local name
	if game.PlaceId ~= 0 then
		local ok, info = pcall(function()
			return game:GetService("MarketplaceService"):GetProductInfo(game.PlaceId)
		end)
		if ok and type(info) == "table" and type(info.Name) == "string" and info.Name ~= "" then
			name = info.Name
		end
	end
	if not name and game.Name ~= "" then
		name = game.Name
	end
	cachedGameName = name or ("game_" .. game.PlaceId)
	return cachedGameName
end

-- Send JSON in chunks and restore the original HTTP setting.
local function sendToMeshvault(text)
	local wasEnabled = HttpService.HttpEnabled
	local reply
	local ok, err = pcall(function()
		local switched, why = pcall(function() HttpService.HttpEnabled = true end)
		if not switched then
			error("could not switch HttpService.HttpEnabled on (" .. tostring(why) .. ")", 0)
		end
		local id = HttpService:GenerateGUID(false)
		local pieces = math.max(1, math.ceil(#text / SEND_CHUNK))
		for i = 1, pieces do
			local sentOk, res = pcall(function()
				return HttpService:RequestAsync({
					Url = MESHVAULT_URL,
					Method = "POST",
					Headers = {
						["Content-Type"] = "application/json",
						["X-Meshvault-Export"] = id,
						["X-Meshvault-Chunk"] = i .. "/" .. pieces,
						["X-Meshvault-Name"] = gameName(),
					},
					Body = text:sub((i - 1) * SEND_CHUNK + 1, math.min(i * SEND_CHUNK, #text)),
				})
			end)
			if not sentOk then
				error(("piece %d of %d did not reach Meshvault: %s"):format(i, pieces, explain(res)), 0)
			end
			if not res.Success then
				error(("Meshvault refused piece %d of %d (HTTP %d): %s"):format(
					i, pieces, res.StatusCode, tostring(res.Body)), 0)
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

-- Send an export table or JSON string; return success and the response.
local function send(export)
	local text = export
	if type(export) ~= "string" then
		local encoded, json = pcall(function() return HttpService:JSONEncode(export) end)
		if not encoded then
			warn(TAG .. " could not turn the export into JSON: " .. tostring(json))
			return false, "could not turn the export into JSON: " .. tostring(json)
		end
		text = json
	end
	local ok, result = sendToMeshvault(text)
	if ok then
		log("Export sent to Meshvault.")
	else
		warn(TAG .. " could not send the export to Meshvault: " .. tostring(result))
	end
	return ok, result
end

-- Collect selected content, build the export, and optionally send it.
local function run(options)
	local started = os.clock()
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
	if game:GetService("RunService"):IsRunning() then
		issue("PlayMode", "the game is running (Play mode): joints are exported in their current animated pose, "
			.. "and Meshvault cannot be reached from a running game. Stop the test and run the line in Edit mode")
	end
	local excludedSet = {}
	for _, excluded in ipairs(excludedRoots) do
		excludedSet[excluded] = true
	end

	local function isExcluded(inst)
		local current = inst
		while current do
			if excludedSet[current] then return true end
			current = current.Parent
		end
		return false
	end
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
	local function rgb(c) return {c.R, c.G, c.B} end
	-- Read an optional property without breaking older Studio versions.
	local function optionalProperty(inst, property)
		local ok, value = pcall(function() return inst[property] end)
		return ok and value or nil
	end
	-- Serialize Content safely; object-backed content is not an asset ID.
	-- Empty Content (SourceType None) is nil, as it references nothing.
	local function contentData(inst, property)
		local value = optionalProperty(inst, property)
		if value == nil then return nil end
		local uri = optionalProperty(value, "Uri")
		local object = optionalProperty(value, "Object")
		local hasUri = type(uri) == "string" and uri ~= ""
		local hasObject = typeof(object) == "Instance"
		if not hasUri and not hasObject then return nil end
		local source = optionalProperty(value, "SourceType")
		local result = { Kind = typeof(source) == "EnumItem" and source.Name or "Unknown" }
		if hasUri then result.Uri = uri end
		if hasObject then
			result.ObjectClass = object.ClassName
			result.ObjectPath = fullpath(object)
		end
		return result
	end
	local MATERIAL_MAPS = {"ColorMap", "NormalMap", "RoughnessMap", "MetalnessMap"}
	-- Collect legacy maps and newer content-backed SurfaceAppearance fields.
	local function surfaceData(inst)
		local sa = inst:FindFirstChildOfClass("SurfaceAppearance")
		if not sa and inst:IsA("SpecialMesh") and inst.Parent then
			sa = inst.Parent:FindFirstChildOfClass("SurfaceAppearance")
		end
		local maps = {ColorMap="", NormalMap="", RoughnessMap="", MetalnessMap=""}
		if not sa then return maps, nil end
		local appearance = {
			Color = rgb(sa.Color), AlphaMode = sa.AlphaMode.Name,
		}
		for _, name in ipairs(MATERIAL_MAPS) do
			local val = optionalProperty(sa, name)
			if nonempty(val) then maps[name] = val end
			appearance[name] = maps[name]
			appearance[name .. "Content"] = contentData(sa, name .. "Content")
		end
		for _, name in ipairs({"TexturePackContent", "EmissiveMaskContent"}) do
			appearance[name] = contentData(sa, name)
		end
		appearance.EmissiveStrength = optionalProperty(sa, "EmissiveStrength")
		local tint = optionalProperty(sa, "EmissiveTint")
		if tint then appearance.EmissiveTint = rgb(tint) end
		local resample = optionalProperty(sa, "ResampleMode")
		if resample then appearance.ResampleMode = resample.Name end
		return maps, appearance
	end
	-- Read the asset ID of a file-backed mesh.
	local function meshIdOf(inst)
		if inst:IsA("MeshPart") then
			return nonempty(optionalProperty(inst, "MeshId"))
		end
		if inst:IsA("SpecialMesh") and inst.MeshType == Enum.MeshType.FileMesh then
			return nonempty(inst.MeshId)
		end
		return nil
	end
	local MaterialService = game:GetService("MaterialService")
	local variantCache = {}
	local materialDiagnostics = {
		MeshesWithSurfaceAppearance = 0, MeshesWithTextureId = 0,
		MeshesWithMaterialVariant = 0, MeshesWithNoTextureReference = 0,
		UnresolvedMaterialVariants = 0, MeshesWithContentReferences = 0,
	}
	-- Resolve explicit variants and global overrides without losing missing names.
	local function materialVariantOf(part)
		local explicit = nonempty(part.MaterialVariant)
		local override = nil
		local ok, name = pcall(function()
			return MaterialService:GetBaseMaterialOverride(part.Material)
		end)
		-- with no override set, Studio gives the material's own name back
		if ok and name ~= part.Material.Name then override = nonempty(name) end
		local chosen = explicit or override
		if not chosen then return nil end
		local key = chosen .. "\0" .. part.Material.Name
		if variantCache[key] == nil then
			local resolved = nil
			local got, found = pcall(function()
				return MaterialService:GetMaterialVariant(part.Material, chosen)
			end)
			if got and found then resolved = found end
			if not resolved then
				for _, v in ipairs(MaterialService:GetDescendants()) do
					if v:IsA("MaterialVariant") and v.Name == chosen and v.BaseMaterial == part.Material then
						resolved = v
						break
					end
				end
			end
			if resolved then
				variantCache[key] = {
					Name = resolved.Name, Resolved = true, StudsPerTile = resolved.StudsPerTile,
					ColorMap = resolved.ColorMap, NormalMap = resolved.NormalMap,
					RoughnessMap = resolved.RoughnessMap, MetalnessMap = resolved.MetalnessMap,
				}
			else
				variantCache[key] = { Name = chosen, Resolved = false }
			end
		end
		local info = table.clone(variantCache[key])
		info.Source = explicit and "Explicit" or "BaseMaterialOverride"
		info.BaseMaterial = part.Material.Name
		if explicit and override then info.BaseMaterialOverride = override end
		return info
	end
	-- Capture appearance on each mesh, keeping differently styled instances distinct.
	local function getOrAddMesh(inst)
		local id = meshIdOf(inst)
		if not id then return nil end
		local part = inst:IsA("BasePart") and inst or inst.Parent
		local maps, appearance = surfaceData(inst)
		local legacyTexture
		if inst:IsA("MeshPart") then
			legacyTexture = optionalProperty(inst, "TextureID")
		else
			legacyTexture = inst.TextureId
		end
		legacyTexture = nonempty(legacyTexture) or ""
		local textureContent = inst:IsA("MeshPart") and contentData(inst, "TextureContent") or nil
		local tex = nonempty(maps.ColorMap) or legacyTexture
		local variant = part and part:IsA("BasePart") and materialVariantOf(part) or nil
		local material = part and part:IsA("BasePart") and part.Material.Name or nil
		local tint = part and part:IsA("BasePart") and rgb(part.Color) or nil
		local transparency = part and part:IsA("BasePart") and part.Transparency or nil
		local reflectance = part and part:IsA("BasePart") and part.Reflectance or nil
		local source = appearance and "SurfaceAppearance"
			or (variant and variant.Resolved and "MaterialVariant")
			or (tex ~= "" and "MeshTexture") or (material and "BaseMaterial") or "Unknown"
		-- false stands in for nil so every field keeps its place in the key
		local signature = {id, tex, legacyTexture, maps, appearance or false,
			textureContent or false, material or false, variant or false, tint or false,
			transparency or false, reflectance or false}
		local key = HttpService:JSONEncode(signature)
		if not meshByKey[key] then
			local newId = #meshes + 1
			meshByKey[key] = newId
			local hasContent = textureContent ~= nil
			if appearance then
				for _, field in ipairs({"ColorMapContent", "NormalMapContent", "RoughnessMapContent",
					"MetalnessMapContent", "TexturePackContent", "EmissiveMaskContent"}) do
					if appearance[field] ~= nil then hasContent = true break end
				end
			end
			if appearance then materialDiagnostics.MeshesWithSurfaceAppearance += 1 end
			if legacyTexture ~= "" then materialDiagnostics.MeshesWithTextureId += 1 end
			if variant then
				materialDiagnostics.MeshesWithMaterialVariant += 1
				if not variant.Resolved then materialDiagnostics.UnresolvedMaterialVariants += 1 end
			end
			if tex == "" and not hasContent and not (variant and variant.Resolved and
				(variant.ColorMap ~= "" or variant.NormalMap ~= "" or variant.RoughnessMap ~= "" or variant.MetalnessMap ~= "")) then
				materialDiagnostics.MeshesWithNoTextureReference += 1
			end
			if hasContent then materialDiagnostics.MeshesWithContentReferences += 1 end
			table.insert(meshes, {
				Id = newId, Name = inst.Name, MeshId = id, TextureId = tex,
				Folder = fullpath(inst.Parent),
				ColorMap = maps.ColorMap, NormalMap = maps.NormalMap,
				RoughnessMap = maps.RoughnessMap, MetalnessMap = maps.MetalnessMap,
				LegacyTextureId = legacyTexture, TextureContent = textureContent,
				SurfaceAppearance = appearance, Material = material,
				MaterialVariant = variant, Color = tint,
				Transparency = transparency, Reflectance = reflectance,
				MaterialSource = source,
			})
		end
		return meshByKey[key]
	end
	-- Export a pose and its nested poses and face controls.
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
			if ch.Name == "FaceControls" and (ch:IsA("Folder") or ch:IsA("Pose")) then
				data.FaceControls = data.FaceControls or {}
				for _, n in ipairs(ch:GetChildren()) do
					if n:IsA("NumberPose") then
						table.insert(data.FaceControls, {
							Name = n.Name, Value = n.Value,
							EasingStyle = n.EasingStyle.Name, EasingDirection = n.EasingDirection.Name,
						})
					end
				end
			end
		end
		return data
	end
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
	-- Serialize a supported curve and its keyframe channels.
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
	-- Flatten a curve animation into named channels and find its duration.
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
	-- Load an animation clip with a timeout to avoid hanging the export.
	local function loadClip(animationId)
		local done, ok, clip = false, false, nil
		task.spawn(function()
			ok, clip = pcall(function()
				return AnimationClipProvider:GetAnimationClipAsync(animationId)
			end)
			done = true
		end)
		local waitStarted = os.clock()
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
			task.wait(0.1)
		end
		return ok, clip
	end
	-- Cache and serialize loaded keyframe or curve animations.
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
	-- Add a joint and its transforms to the model's rig data.
	local function addJoint(m, inst, part0, part1, c0, c1, transform, kind)
		table.insert(m.Joints, {
			Name=inst.Name, Path=fullpath(inst),
			Part0=part0 and fullpath(part0) or "",
			Part1=part1 and fullpath(part1) or "",
			C0=cfdata(c0), C1=cfdata(c1),
			Transform=cfdata(transform),
			Kind=kind,
		})
	end
	-- Convert effect values into JSON-safe primitives and keypoint arrays.
	local function effectValue(value)
		local kind = typeof(value)
		if kind == "boolean" or kind == "number" or kind == "string" then return value end
		if kind == "EnumItem" then return value.Name end
		if kind == "Color3" then return rgb(value) end
		if kind == "Vector2" then return {value.X, value.Y} end
		if kind == "Vector3" then return v3(value) end
		if kind == "NumberRange" then return {value.Min, value.Max} end
		if kind == "Rect" then return {value.Min.X, value.Min.Y, value.Max.X, value.Max.Y} end
		if kind == "NumberSequence" then
			local keys = {}
			for _, key in ipairs(value.Keypoints) do
				table.insert(keys, {Time=key.Time, Value=key.Value, Envelope=key.Envelope})
			end
			return keys
		end
		if kind == "ColorSequence" then
			local keys = {}
			for _, key in ipairs(value.Keypoints) do
				table.insert(keys, {Time=key.Time, Value=rgb(key.Value)})
			end
			return keys
		end
		return nil
	end

	local EFFECT_PROPERTIES = {
		ParticleEmitter = {
			"Enabled", "Texture", "Color", "Transparency", "Size", "Rate", "Lifetime", "Speed",
			"Rotation", "RotSpeed", "SpreadAngle", "EmissionDirection", "Acceleration", "Drag",
			"VelocityInheritance", "LockedToPart", "Orientation", "ZOffset", "LightEmission",
			"LightInfluence", "Brightness", "Squash", "Shape", "ShapeStyle", "ShapeInOut",
			"FlipbookLayout", "FlipbookMode", "FlipbookFramerate", "FlipbookStartRandom",
			"TimeScale", "WindAffectsDrag", "LocalTransparencyModifier",
		},
		Beam = {
			"Enabled", "Texture", "Color", "Transparency", "Width0", "Width1", "CurveSize0",
			"CurveSize1", "TextureSpeed", "TextureLength", "TextureMode", "Segments",
			"FaceCamera", "LightEmission", "LightInfluence", "Brightness", "ZOffset",
		},
		Trail = {
			"Enabled", "Texture", "Color", "Transparency", "Lifetime", "MinLength",
			"WidthScale", "TextureMode", "TextureLength", "FaceCamera", "LightEmission",
			"LightInfluence", "Brightness", "MaxLength",
		},
		Fire = {"Enabled", "Color", "SecondaryColor", "Heat", "Size", "TimeScale"},
		Smoke = {"Enabled", "Color", "Opacity", "RiseVelocity", "Size", "TimeScale"},
		Sparkles = {"Enabled", "SparkleColor", "TimeScale"},
	}

	-- Capture effect settings and attachment links without reading private assets.
	local function exportEffect(inst)
		local propertyNames = EFFECT_PROPERTIES[inst.ClassName]
		if not propertyNames then return nil end
		local effect = {
			ClassName=inst.ClassName, Name=inst.Name, Path=fullpath(inst),
			ParentPath=inst.Parent and fullpath(inst.Parent) or "",
		}
		for _, property in ipairs(propertyNames) do
			-- read directly, not with optionalProperty, which turns false
			-- (Enabled, LockedToPart, FaceCamera) into nil
			local ok, value = pcall(function() return inst[property] end)
			if ok and value ~= nil then
				effect[property] = effectValue(value)
			end
		end
		if inst:IsA("Beam") or inst:IsA("Trail") then
			for _, property in ipairs({"Attachment0", "Attachment1"}) do
				local attachment = optionalProperty(inst, property)
				if attachment and attachment:IsA("Attachment") then
					effect[property .. "Path"] = fullpath(attachment)
				end
			end
		end
		return effect
	end

	-- Export model geometry, rigging, effects, attachments, and animations.
	local function exportRig(model)
		local m = {
			Name=model.Name, Path=fullpath(model), Scale=model:GetScale(),
			Parts={}, Joints={}, Bones={}, Animations={}, Attachments={}, Effects={},
			Pivot=cfdata(model:GetPivot()),
			PrimaryPartPath=model.PrimaryPart and fullpath(model.PrimaryPart) or nil,
		}
		do
			local clothing = {}
			local shirt = model:FindFirstChildOfClass("Shirt")
			if shirt and nonempty(shirt.ShirtTemplate) then
				clothing.ShirtTemplate, clothing.ShirtColor = shirt.ShirtTemplate, rgb(shirt.Color3)
			end
			local pants = model:FindFirstChildOfClass("Pants")
			if pants and nonempty(pants.PantsTemplate) then
				clothing.PantsTemplate, clothing.PantsColor = pants.PantsTemplate, rgb(pants.Color3)
			end
			local graphic = model:FindFirstChildOfClass("ShirtGraphic")
			if graphic and nonempty(graphic.Graphic) then
				clothing.Graphic, clothing.GraphicColor = graphic.Graphic, rgb(graphic.Color3)
			end
			if next(clothing) then m.Clothing = clothing end
		end
		local animSeen = {}
		for _, inst in ipairs(model:GetDescendants()) do
			if isExcluded(inst) then continue end
			local effect = exportEffect(inst)
			if effect then table.insert(m.Effects, effect) end
			if inst:IsA("BasePart") then
				local meshRef = getOrAddMesh(inst)
				local sm = inst:FindFirstChildOfClass("SpecialMesh")
				if not meshRef and sm then meshRef = getOrAddMesh(sm) end
				-- A FileMesh has its own scale/offset, independent of its parent Part.
				-- Other SpecialMesh types also affect the visible shape.
				local specialMesh = nil
				if sm then
					specialMesh = {
						MeshType = sm.MeshType.Name,
						Scale = v3(sm.Scale),
						Offset = v3(sm.Offset),
					}
				end
				local decals = {}
				for _, ch in ipairs(inst:GetChildren()) do
					if ch:IsA("Decal") and nonempty(ch.Texture) then
						local decal = {
							ClassName = ch.ClassName, Texture = ch.Texture, Face = ch.Face.Name,
							Color = rgb(ch.Color3), Transparency = ch.Transparency, ZIndex = ch.ZIndex,
						}
						if ch:IsA("Texture") then
							decal.StudsPerTileU, decal.StudsPerTileV = ch.StudsPerTileU, ch.StudsPerTileV
							decal.OffsetStudsU, decal.OffsetStudsV = ch.OffsetStudsU, ch.OffsetStudsV
						end
						table.insert(decals, decal)
					end
				end
				if #decals == 0 then decals = nil end
				local lights = {}
				local function addLight(light, at)
					local record = {
						ClassName = light.ClassName, Enabled = light.Enabled,
						Brightness = light.Brightness, Color = rgb(light.Color),
						Range = light.Range, Shadows = light.Shadows, CFrame = at,
					}
					if light:IsA("SpotLight") or light:IsA("SurfaceLight") then
						record.Angle, record.Face = light.Angle, light.Face.Name
					end
					table.insert(lights, record)
				end
				for _, ch in ipairs(inst:GetChildren()) do
					if ch:IsA("Light") then
						addLight(ch, nil)
					elseif ch:IsA("Attachment") then
						for _, light in ipairs(ch:GetChildren()) do
							if light:IsA("Light") then addLight(light, cfdata(ch.CFrame)) end
						end
					end
				end
				if #lights == 0 then lights = nil end
				local _, surfaceAppearance = surfaceData(inst)
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
					Pivot=cfdata(inst:GetPivot()),
					PivotOffset=cfdata(inst.PivotOffset),
					Color=rgb(inst.Color),
					BrickColor=inst.BrickColor.Name,
					Material=inst.Material.Name,
					Transparency=inst.Transparency,
					Reflectance=inst.Reflectance,
					SpecialMesh=specialMesh,
					SurfaceAppearance=surfaceAppearance,
					MaterialVariant=materialVariantOf(inst),
					Lights=lights,
					Decals=decals,
				})
			elseif inst:IsA("Attachment") and not inst:IsA("Bone") then
				-- Preserve attachment transforms and paths for effects and constraints.
				local attachment = {
					Name=inst.Name,
					Path=fullpath(inst),
					ParentPath=inst.Parent and fullpath(inst.Parent) or "",
					CFrame=cfdata(inst.CFrame),
					WorldCFrame=cfdata(inst.WorldCFrame),
					Visible=inst.Visible,
				}
				table.insert(m.Attachments, attachment)
			elseif inst:IsA("Motor6D") then
				addJoint(m, inst, inst.Part0, inst.Part1, inst.C0, inst.C1, inst.Transform, "Motor6D")
			elseif inst:IsA("AnimationConstraint") then
				local a0, a1 = inst.Attachment0, inst.Attachment1
				local part0 = a0 and a0.Parent
				local part1 = a1 and a1.Parent
				if part0 and part1 and part0:IsA("BasePart") and part1:IsA("BasePart") then
					addJoint(m, inst, part0, part1, a0.CFrame, a1.CFrame, inst.Transform, inst.ClassName)
				end
			elseif inst:IsA("JointInstance") then
				-- Weld, ManualWeld, Snap, Glue, Motor: Part1 is held at Part0 * C0 * C1^-1
				if inst.Part0 and inst.Part1 then
					addJoint(m, inst, inst.Part0, inst.Part1, inst.C0, inst.C1, CFrame.identity, inst.ClassName)
				end
			elseif inst:IsA("WeldConstraint") then
				-- keeps the parts as they are: Part1 at Part0 times their current offset
				if inst.Part0 and inst.Part1 and inst.Enabled then
					addJoint(m, inst, inst.Part0, inst.Part1, inst.Part0.CFrame:ToObjectSpace(inst.Part1.CFrame),
						CFrame.identity, CFrame.identity, inst.ClassName)
				end
			elseif inst:IsA("RigidConstraint") then
				local a0, a1 = inst.Attachment0, inst.Attachment1
				local part0 = a0 and a0.Parent
				local part1 = a1 and a1.Parent
				if inst.Enabled and part0 and part1 and part0:IsA("BasePart") and part1:IsA("BasePart") then
					addJoint(m, inst, part0, part1, a0.CFrame, a1.CFrame, CFrame.identity, inst.ClassName)
				end
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
	-- Export a model without letting one broken model stop the entire run.
	local function addModel(model)
		local ok, m = pcall(exportRig, model)
		if not ok then
			issue("ModelSkipped", ("skipped the model %s: %s"):format(model:GetFullName(), tostring(m)),
				{ Path = model:GetFullName() })
			return
		end
		table.insert(models, m)
	end
	-- Discover models containing geometry, animations, or visual effects.
	local function modelFor(inst, root)
		local candidate = nil
		local current = inst
		while current do
			if current:IsA("Model") then
				candidate = current
			end
			if current == root then break end
			current = current.Parent
		end
		return candidate
	end

	-- Scan meshes once and export each discovered model only once.
	for _, root in ipairs(roots) do
		if isExcluded(root) then continue end
		local list = root:GetDescendants()
		table.insert(list, 1, root)
		for _, inst in ipairs(list) do
			if isExcluded(inst) then continue end
			if meshIdOf(inst) then
				local ok, err = pcall(getOrAddMesh, inst)
				if not ok then
					issue("MeshSkipped", ("skipped the mesh of %s: %s"):format(inst:GetFullName(), tostring(err)),
						{ Path = inst:GetFullName() })
				end
			end
			if inst:IsA("BasePart") or inst:IsA("Animation") or EFFECT_PROPERTIES[inst.ClassName] then
				local model = modelFor(inst, root)
				if model and not modelSeen[model] and not isExcluded(model) then
					modelSeen[model] = true
					addModel(model)
				end
			end
		end
		-- An explicitly selected model is exported even if it has no parts or animations.
		if root:IsA("Model") and not modelSeen[root] then
			modelSeen[root] = true
			addModel(root)
		end
	end
	local payload = {
		SchemaVersion = SCHEMA,
		GameName = gameName(),
		PlaceId = game.PlaceId,
		Meshes = meshes,
		Models = models,
		MaterialDiagnostics = materialDiagnostics,
	}
	log(("Exported %d models and %d meshes in %.1fs."):format(#models, #meshes, os.clock() - started))
	if #meshes == 0 and #models == 0 then
		issue("NothingToExport", "nothing to export: no meshes or models in "
			.. ((options.roots or useSelection) and "what was picked" or "the game outside Workspace")
			.. ". Select a model (or anything inside one) in the Explorer and run the line again")
		return
	end
	if options.send == false then
		return payload
	end
	local encoded, json = pcall(function() return HttpService:JSONEncode(payload) end)
	if not encoded then
		error("could not turn the export into JSON: " .. tostring(json), 0)
	end

	local sent, result = sendToMeshvault(json)
	if sent then
		log("Export sent to Meshvault.")
	else
		issue("SendFailed", "could not send the export to Meshvault: " .. tostring(result))
		-- so the export is not lost: chunks to copy into a .json file and import. Studio's
		-- Output can cut long lines short, so each chunk is kept small
		log("printing the export here instead: copy the chunks below into a .json file and import it in Meshvault")
		local PRINT_CHUNK = 12000
		local pieces = math.max(1, math.ceil(#json / PRINT_CHUNK))
		for i = 1, pieces do
			print(("=== JSON CHUNK %d/%d ==="):format(i, pieces))
			print(json:sub((i - 1) * PRINT_CHUNK + 1, math.min(i * PRINT_CHUNK, #json)))
		end
		print("=== END JSON EXPORT ===")
	end
	return payload
end

-- Reset warnings and return the export, warnings, and reusable sender.
return function(options)
	issues = {}
	local ok, result = xpcall(run, function(e)
		return debug.traceback(tostring(e), 2)
	end, options or {})
	local export = ok and result or nil
	if not ok then
		issue("Error", "the export stopped with an error: " .. tostring(result))
		warn(TAG .. " please report this, with the lines above, to Meshvault's developers "
			.. "(exporter schema " .. SCHEMA .. ", Meshvault " .. MESHVAULT_VERSION .. ")")
	end
	return issues, export, send
end
