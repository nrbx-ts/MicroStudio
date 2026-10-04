--!nocheck
-- depends on the world so the specs have something worth asserting: it listens to
-- a service and counts what arrives

local Players = game:GetService("Players")

local Scoreboard = {}
Scoreboard.__index = Scoreboard

function Scoreboard.new()
	return setmetatable({ entries = {}, joined = 0 }, Scoreboard)
end

function Scoreboard:track()
	self.connection = Players.PlayerAdded:Connect(function(player)
		self.joined += 1
		self.entries[player.Name] = 0
	end)
	return self
end

function Scoreboard:score(name, points)
	if self.entries[name] == nil then
		error(`no entry for {name}`, 2)
	end
	self.entries[name] += points
	return self.entries[name]
end

return Scoreboard
