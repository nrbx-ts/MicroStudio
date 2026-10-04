--!nocheck
-- example spec, run by `microstudio test examples/testing`: the project file maps
-- src onto ReplicatedStorage, so require sees the module as the game does

local Scoreboard = require(game.ReplicatedStorage.scoreboard)

describe("Scoreboard", function()
	local board

	beforeEach(function()
		board = Scoreboard.new():track()
	end)

	it("starts empty", function()
		expect(board.entries).toEqual({})
		expect(board.joined).toBe(0)
	end)

	it("notices a player joining", function()
		expect(#game:GetService("Players"):GetPlayers()).toBe(0)

		addPlayer("Eddie")

		expect(board.joined).toBe(1)
		expect(board.entries.Eddie).toBe(0)
	end)

	it("scores a known player", function()
		board.entries.Eddie = 0
		expect(board:score("Eddie", 3)).toBe(3)
		expect(board:score("Eddie", 4)).toBe(7)
	end)

	it("refuses an unknown player", function()
		expect(function()
			board:score("Nobody", 1)
		end).toThrow("no entry for Nobody")
	end)

	it("keeps tests in separate worlds", function()
		-- beforeEach builds a fresh board, the runner keeps the world between tests
		expect(board.entries).toEqual({})
	end)

	it.todo("resets at the end of a round")
end)
