--!strict
-- stands in for the compiled output of src/shared/util.ts: rbxtsc writes this in a
-- real project, the example ships it so it runs with no toolchain

local util = {}

function util.greet(name: string): string
	return string.format("hello, %s", name)
end

function util.sum(values: { number }): number
	local total = 0
	for _, value in values do
		total += value
	end
	return total
end

return util

