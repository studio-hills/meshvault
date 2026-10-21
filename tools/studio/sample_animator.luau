-- Samples every clip of the models in TEST_MODELS in Studio's own Animator, on
-- temporary copies in Workspace (the Animator only drives joints there): each
-- Motor6D/AnimationConstraint Transform, Bone.Transform and every part's CFrame
-- relative to the root part, at fixed times. Posted to the stand-in link
-- (tools/studio/stand_in_link.py, /samples/<_G.MVSamplesName or "studio_samples">)
-- for comparison with Meshvault by the ignored test matches_studio_samples.
-- Each clip gets a fresh copy: Studio leaves parts a clip no longer drives where the
-- previous clip put them, which would show up as differences that are not there.
--
-- The test place: each model under TEST_MODELS has an Animations folder of Animation
-- instances (one named "Broken" is skipped). Run in the Command Bar, in Edit mode:
--   loadstring(game:GetService("HttpService"):GetAsync(
--     "http://localhost:34876/file/sample_animator.luau"))()
-- (HttpService.HttpEnabled must be on for that fetch.)
local HttpService = game:GetService("HttpService")
local TEST_MODELS = game.ReplicatedStorage.MeshvaultTest
local STAND_IN = "http://localhost:34876"
local SAMPLES = 12
local result = { Models = {} }

local function cf(c)
	local t = { c:GetComponents() }
	for i, v in t do t[i] = math.round(v * 1e5) / 1e5 end
	return t
end
local function relative(model, inst)
	return (inst:GetFullName():sub(#model:GetFullName() + 2):gsub("%.", "/"))
end

-- a copy of the original in Workspace, its captured joint poses restored
local function stage(original)
	local model = original:Clone()
	model.Parent = workspace
	local animator = model:FindFirstChildWhichIsA("Animator", true)
	if not animator then
		-- a model with clips but nothing to play them: borrow a controller
		local controller = Instance.new("AnimationController")
		animator = Instance.new("Animator", controller)
		controller.Parent = model
	end
	-- Motor6D.Transform is not copied by Clone
	local captured = {}
	for _, d in original:GetDescendants() do
		if d:IsA("Motor6D") or d:IsA("AnimationConstraint") then captured[relative(original, d)] = d.Transform end
	end
	local motors, bones, parts = {}, {}, {}
	for _, d in model:GetDescendants() do
		if d:IsA("Motor6D") or d:IsA("AnimationConstraint") then
			d.Transform = captured[relative(model, d)] or d.Transform
			table.insert(motors, d)
		elseif d:IsA("Bone") then
			table.insert(bones, d)
		elseif d:IsA("BasePart") then
			parts[relative(model, d)] = d
		end
	end
	local root = model:FindFirstChild("HumanoidRootPart") or model.PrimaryPart or model:FindFirstChildWhichIsA("BasePart", true)
	local function capture()
		local j, b, p = {}, {}, {}
		for _, m in motors do j[m.Name] = cf(m.Transform) end
		for _, x in bones do b[x.Name] = cf(x.Transform) end
		for path, part in parts do p[path] = cf(root.CFrame:ToObjectSpace(part.CFrame)) end
		return j, b, p
	end
	return model, animator, capture, root and relative(model, root)
end

for _, original in TEST_MODELS:GetChildren() do
	if not original:FindFirstChild("Animations") then continue end
	local model, _, capture, rootPath = stage(original)
	local entry = { Scale = model:GetScale(), Clips = {}, RootPart = rootPath }
	-- the pose before any clip plays (captured transforms)
	entry.Rest = { capture() }
	model:Destroy()
	result.Models[original.Name] = entry
	for _, anim in original.Animations:GetChildren() do
		if not anim:IsA("Animation") or anim.Name == "Broken" then continue end
		local copy, animator, captureCopy = stage(original)
		local track = animator:LoadAnimation(copy.Animations[anim.Name])
		local waited = 0
		while track.Length == 0 and waited < 5 do task.wait(0.1) waited += 0.1 end
		-- a one-frame pose (Mood) has length 0: sample it once, at 0
		track:Play(0, 1, 0)
		local clip = { Length = track.Length, Samples = {} }
		for i = 0, track.Length > 0 and SAMPLES - 1 or 0 do
			local t = track.Length * (i + 0.37) / SAMPLES
			track.TimePosition = t
			animator:StepAnimations(0)
			local j, b, p = captureCopy()
			table.insert(clip.Samples, { Time = t, Joints = j, Bones = b, Parts = p })
		end
		track:Stop(0)
		track:Destroy()
		copy:Destroy()
		entry.Clips[anim.Name] = clip
	end
end

local json = HttpService:JSONEncode(result)
local was = HttpService.HttpEnabled
HttpService.HttpEnabled = true
local ok, err = pcall(HttpService.RequestAsync, HttpService, {
	Url = STAND_IN .. "/samples/" .. (_G.MVSamplesName or "studio_samples"), Method = "POST",
	Headers = { ["X-Meshvault-Export"] = HttpService:GenerateGUID(false), ["X-Meshvault-Chunk"] = "1/1" },
	Body = json,
})
HttpService.HttpEnabled = was
return ("%s, %d KB, HttpEnabled=%s"):format(ok and "sent" or tostring(err), #json // 1024, tostring(HttpService.HttpEnabled))
