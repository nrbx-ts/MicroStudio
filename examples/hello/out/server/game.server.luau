--!strict
-- stands in for the compiled output of src/server/game.ts: rojo puts it under
-- ServerScriptService.TS, which is why the project file gets read

local ReplicatedStorage = game:GetService("ReplicatedStorage")
local CollectionService = game:GetService("CollectionService")
local Players = game:GetService("Players")
local RunService = game:GetService("RunService")

local util = require(ReplicatedStorage.TS.util)

print(util.greet("MicroStudio"))
print("running on the server:", RunService:IsServer())

-- Signals fire synchronously, so connect before mutating.
CollectionService:GetInstanceAddedSignal("checkpoint"):Connect(function(instance)
	print("tagged a checkpoint:", instance.Name)
end)

Players.PlayerAdded:Connect(function(player)
	print("welcome,", player.Name)
end)

local folder = Instance.new("Folder")
folder.Name = "Checkpoints"
folder.Parent = workspace

local checkpoint = Instance.new("Part")
checkpoint.Name = "Checkpoint1"
checkpoint.Size = Vector3.new(4, 1, 4)
checkpoint.Anchored = true
checkpoint.Parent = folder
checkpoint:SetAttribute("Order", 1)

CollectionService:AddTag(checkpoint, "checkpoint")
print("checkpoint position:", checkpoint.Position)
print("tagged instances:", #CollectionService:GetTagged("checkpoint"))
print("util.sum:", util.sum({ 1, 2, 3 }))

-- task.wait yields to the scheduler: a real half second under dev, simulated and
-- free under test
task.wait(0.5)
print("half a second later")

task.spawn(function()
	local elapsed = task.wait(0.25)
	print(string.format("spawned task resumed after %.2fs", elapsed))
end)
