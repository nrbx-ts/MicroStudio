--!nocheck
-- test framework injected into a spec's world; state survives a world reset

local state = __microstudio_test_state()

local function freshState()
	return {
		root = {
			name = "",
			fullName = "",
			children = {},
			beforeAll = {},
			afterAll = {},
			beforeEach = {},
			afterEach = {},
		},
		stack = {},
		tests = {},
		assertions = 0,
	}
end

-- clears the registry so two spec files in one runtime do not share tests
function __microstudio_test_reset_registry()
	for key, value in pairs(freshState()) do
		state[key] = value
	end
	return true
end

if state.root == nil then
	__microstudio_test_reset_registry()
end

local root = state.root

local function joinName(parent, name)
	if parent == nil or parent.fullName == "" then
		return name
	end
	return parent.fullName .. " > " .. name
end

local function currentSuite()
	return state.stack[#state.stack] or root
end

local function suiteChain(test)
	local chain = {}
	local node = test.parent
	while node ~= nil and node ~= root do
		table.insert(chain, 1, node)
		node = node.parent
	end
	return chain
end

local function depthOf(test)
	return #suiteChain(test)
end

-- renders a value for a failure message
local function inspect(value, depth, seen)
	depth = depth or 0
	if type(value) == "string" then
		return string.format("%q", value)
	end
	if type(value) ~= "table" then
		if typeof(value) == "Instance" then
			return string.format("Instance<%s> %s", value.ClassName, value.Name)
		end
		return tostring(value)
	end
	if depth >= 3 then
		return "{...}"
	end
	seen = seen or {}
	if seen[value] then
		return "<cycle>"
	end
	seen[value] = true

	local parts = {}
	local count = #value
	if count > 0 then
		for index = 1, count do
			table.insert(parts, inspect(value[index], depth + 1, seen))
		end
		return "[" .. table.concat(parts, ", ") .. "]"
	end

	local keys = {}
	for key in pairs(value) do
		table.insert(keys, key)
	end
	if #keys == 0 then
		return "{}"
	end
	table.sort(keys, function(a, b)
		return tostring(a) < tostring(b)
	end)
	for _, key in ipairs(keys) do
		table.insert(parts, tostring(key) .. " = " .. inspect(value[key], depth + 1, seen))
	end
	return "{ " .. table.concat(parts, ", ") .. " }"
end

local function deepEqual(a, b, seen)
	if a == b then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	seen = seen or {}
	seen[a] = seen[a] or {}
	if seen[a][b] then
		return true
	end
	seen[a][b] = true

	for key, value in pairs(a) do
		if not deepEqual(value, b[key], seen) then
			return false
		end
	end
	for key in pairs(b) do
		if a[key] == nil then
			return false
		end
	end
	return true
end

-- finds where two values diverge, to name the key in the failure
local function firstDifference(a, b, path)
	if a == b then
		return nil
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return path or "<value>", a, b
	end
	for key, value in pairs(a) do
		local childPath = path == nil and tostring(key) or (path .. "." .. tostring(key))
		local found, left, right = firstDifference(value, b[key], childPath)
		if found ~= nil then
			return found, left, right
		end
	end
	for key, value in pairs(b) do
		if a[key] == nil then
			local childPath = path == nil and tostring(key) or (path .. "." .. tostring(key))
			return childPath, nil, value
		end
	end
	return nil
end

-- must be called from an error handler; the failing frames are gone afterwards
-- drops the framework's own frames
local function cleanTraceback()
	if debug == nil or debug.traceback == nil then
		return nil
	end
	local prefix = __microstudio_test_framework_chunk or "[framework]"
	if type(prefix) ~= "string" then
		prefix = "[framework]"
	end

	local frames = {}
	for line in string.gmatch(debug.traceback("", 2), "[^\n]+") do
		local frame = (line:gsub("^%s+", ""))
		if
			frame ~= ""
			and frame:sub(1, #prefix) ~= prefix
			and string.find(frame, "MicroStudio.repl", 1, true) == nil
		then
			table.insert(frames, frame)
		end
	end

	if #frames == 0 then
		return nil
	end
	return table.concat(frames, "\n")
end

-- splits `file:12: message` into location and message
local function splitLocation(message)
	local text = tostring(message)
	for index = 1, #text do
		if text:sub(index, index) == ":" then
			local digits = text:match("^(%d+)", index + 1)
			if digits ~= nil and text:sub(index + 1 + #digits, index + 1 + #digits) == ":" then
				local location = text:sub(1, index + #digits)
				if #location > 1 then
					return location, (text:sub(index + 2 + #digits):gsub("^%s+", ""))
				end
			end
		end
	end
	return nil, text
end

local function matches(text, pattern)
	if pattern == nil then
		return true
	end
	return string.find(text, pattern, 1, true) ~= nil or string.find(text, pattern) ~= nil
end

-- returns a failure record or nil
-- built inside the handler so the traceback keeps its failing frames
local function callCaught(fn)
	local record = nil
	xpcall(fn, function(err)
		local text = tostring(err)
		local location, message = splitLocation(text)
		record = {
			location = location,
			message = message,
			text = text,
			traceback = cleanTraceback(),
		}
		return err
	end)
	return record
end

local function makeMatchers(value, negated, message)
	local api = {}

	local function check(passed, describe)
		state.assertions = state.assertions + 1
		if negated then
			passed = not passed
		end
		if not passed then
			local prefix = message ~= nil and (tostring(message) .. ": ") or ""
			-- level 3 blames the spec, not this file
			error(prefix .. describe, 3)
		end
	end

	local function negate(word)
		return negated and ("not " .. word) or word
	end

	function api.toBe(expected)
		check(
			value == expected,
			string.format("Expected %s to %s %s", inspect(value), negate("be"), inspect(expected))
		)
	end

	function api.toEqual(expected)
		if deepEqual(value, expected) then
			check(true, "")
			return
		end
		local path, left, right = firstDifference(value, expected)
		local detail = ""
		if path ~= nil and path ~= "<value>" then
			detail = string.format(" (%s is %s, expected %s)", path, inspect(left), inspect(right))
		end
		check(
			false,
			string.format("Expected %s to %s %s%s", inspect(value), negate("equal"), inspect(expected), detail)
		)
	end

	function api.toBeCloseTo(expected, places)
		local precision = places or 2
		local tolerance = (10 ^ -precision) / 2
		local passed = type(value) == "number"
			and type(expected) == "number"
			and math.abs(value - expected) < tolerance
		check(
			passed,
			string.format(
				"Expected %s to %s %s (within %d decimal places)",
				inspect(value),
				negate("be close to"),
				inspect(expected),
				precision
			)
		)
	end

	function api.toBeTruthy()
		check(value and true or false, string.format("Expected %s to %s truthy", inspect(value), negate("be")))
	end

	function api.toBeFalsy()
		check(not value, string.format("Expected %s to %s falsy", inspect(value), negate("be")))
	end

	function api.toBeNil()
		check(value == nil, string.format("Expected %s to %s nil", inspect(value), negate("be")))
	end

	function api.toBeType(expected)
		local actual = typeof(value)
		check(
			actual == expected,
			string.format("Expected %s to %s type %q", inspect(value), negate("be"), expected)
		)
	end

	function api.toBeA(class_name)
		local actual = typeof(value) == "Instance" and value.ClassName or nil
		local passed = actual ~= nil and (actual == class_name or value:IsA(class_name))
		check(passed, string.format("Expected %s to %s a %s", inspect(value), negate("be"), class_name))
	end

	function api.toHaveLength(expected)
		local length = (type(value) == "table" or type(value) == "string") and #value or nil
		check(
			length == expected,
			string.format("Expected %s to %s length %s", inspect(value), negate("have"), inspect(expected))
		)
	end

	function api.toContain(item)
		local found = false
		if type(value) == "string" then
			found = string.find(value, tostring(item), 1, true) ~= nil
		elseif type(value) == "table" then
			for _, entry in ipairs(value) do
				if entry == item or deepEqual(entry, item) then
					found = true
					break
				end
			end
			if not found and value[item] ~= nil then
				found = true
			end
		end
		check(found, string.format("Expected %s to %s %s", inspect(value), negate("contain"), inspect(item)))
	end

	-- literal fallback so a `(` in the pattern still works
	function api.toMatch(pattern)
		local passed = type(value) == "string" and type(pattern) == "string" and matches(value, pattern)
		check(passed, string.format("Expected %s to %s %s", inspect(value), negate("match"), inspect(pattern)))
	end

	function api.toThrow(pattern)
		if type(value) ~= "function" then
			check(false, string.format("Expected %s to be a function, so it can throw", inspect(value)))
			return
		end
		local ok, err = pcall(value)
		if ok then
			check(false, string.format("Expected the function to %s", negate("throw")))
			return
		end
		check(matches(tostring(err), pattern), string.format(
			"Expected the thrown error %s to %s %s",
			inspect(tostring(err)),
			negate("match"),
			inspect(pattern)
		))
	end

	function api.toHaveProperty(name, expected)
		local actual
		local ok = pcall(function()
			actual = value[name]
		end)
		if not ok or (expected == nil and actual == nil) then
			check(false, string.format("Expected %s to %s property %q", inspect(value), negate("have"), name))
			return
		end
		if expected == nil then
			check(true, "")
			return
		end
		check(deepEqual(actual, expected), string.format(
			"Expected %s.%s to %s %s, got %s",
			inspect(value),
			tostring(name),
			negate("equal"),
			inspect(expected),
			inspect(actual)
		))
	end

	local function comparison(name, operator)
		return function(expected)
			local passed = type(value) == "number" and type(expected) == "number" and operator(value, expected)
			check(
				passed,
				string.format("Expected %s to %s %s", inspect(value), negate("be " .. name), inspect(expected))
			)
		end
	end

	api.toBeGreaterThan = comparison("greater than", function(a, b)
		return a > b
	end)
	api.toBeGreaterThanOrEqual = comparison("greater than or equal to", function(a, b)
		return a >= b
	end)
	api.toBeLessThan = comparison("less than", function(a, b)
		return a < b
	end)
	api.toBeLessThanOrEqual = comparison("less than or equal to", function(a, b)
		return a <= b
	end)

	return api
end

-- `not` is a keyword, so the negated chain is `never` (or `isNot`)
function expect(value, message)
	local positive = makeMatchers(value, false, message)
	local negative = makeMatchers(value, true, message)
	positive.never = negative
	positive.isNot = negative
	return positive
end

local function addSuite(name, fn, mode)
	local parent = currentSuite()
	local suite = {
		name = name,
		fullName = joinName(parent, name),
		parent = parent,
		children = {},
		beforeAll = {},
		afterAll = {},
		beforeEach = {},
		afterEach = {},
		mode = mode or "run",
	}
	table.insert(parent.children, suite)

	table.insert(state.stack, suite)
	local broken = callCaught(fn)
	table.remove(state.stack)

	if broken ~= nil then
		-- a suite that cannot be described becomes a failing test, not a crash
		local failure = {
			name = name,
			fullName = joinName(parent, name),
			parent = parent,
			mode = "run",
			broken = {
				location = broken.location,
				message = broken.message or "the suite body failed",
				traceback = broken.traceback,
			},
		}
		table.insert(parent.children, failure)
		table.insert(state.tests, failure)
		failure.index = #state.tests
	end

	return suite
end

local function addTest(name, fn, mode)
	local suite = currentSuite()
	local test = {
		name = name,
		fullName = joinName(suite, name),
		parent = suite,
		fn = fn,
		mode = mode or "run",
	}
	table.insert(suite.children, test)
	table.insert(state.tests, test)
	test.index = #state.tests
	return test
end

-- callable tables: Luau functions cannot carry .skip/.only/.todo
it = setmetatable({}, {
	__call = function(_, name, fn)
		return addTest(name, fn)
	end,
})

function it.skip(name, fn)
	return addTest(name, fn, "skip")
end

function it.only(name, fn)
	return addTest(name, fn, "only")
end

function it.todo(name)
	return addTest(name, nil, "todo")
end

describe = setmetatable({}, {
	__call = function(_, name, fn)
		return addSuite(name, fn)
	end,
})

function describe.skip(name, fn)
	return addSuite(name, fn, "skip")
end

function describe.only(name, fn)
	return addSuite(name, fn, "only")
end

-- alias for `it`
test = it

local function hook(kind)
	return function(fn)
		table.insert(currentSuite()[kind], fn)
	end
end

beforeAll = hook("beforeAll")
afterAll = hook("afterAll")
beforeEach = hook("beforeEach")
afterEach = hook("afterEach")

-- empties the world
function reset()
	return __microstudio_reset_world()
end

-- fires PlayerAdded like a real join
function addPlayer(name)
	return __microstudio_add_player(name)
end

-- the declared mode, before .only filtering
local function declared(test)
	return test.declaredMode or test.mode
end

local function hasOnly()
	for _, test in ipairs(state.tests) do
		if declared(test) == "only" then
			return true
		end
	end
	return false
end

-- true when an ancestor suite has this mode
local function inherited(test, mode)
	local node = test.parent
	while node ~= nil and node ~= root do
		if declared(node) == mode then
			return true
		end
		node = node.parent
	end
	return false
end

-- the mode a test runs with: .only narrows, a skipped suite skips
-- written back so __microstudio_test_run honours it; declaredMode keeps the original
local function effectiveMode(test, only)
	local mode = declared(test)
	if inherited(test, "skip") then
		return "skip"
	end
	if mode == "skip" or mode == "todo" then
		return mode
	end
	if only then
		return (mode == "only" or inherited(test, "only")) and "run" or "skip"
	end
	return mode
end

function __microstudio_test_plan()
	local only = hasOnly()
	local entries = {}

	for _, test in ipairs(state.tests) do
		local mode = effectiveMode(test, only)
		test.declaredMode = declared(test)
		test.mode = mode
		table.insert(entries, {
			index = test.index,
			name = test.name,
			fullName = test.fullName,
			mode = mode,
			depth = depthOf(test),
		})
	end

	return { entries = entries }
end

local function hookMessage(failure)
	local detail = failure.location ~= nil and (failure.location .. ": " .. failure.message)
		or failure.message
	return failure.hook .. " failed: " .. detail
end

local function runHooks(list, kind, failures)
	for _, fn in ipairs(list) do
		local broken = callCaught(fn)
		if broken ~= nil then
			table.insert(failures, {
				hook = kind,
				location = broken.location,
				message = broken.message,
				traceback = broken.traceback,
			})
		end
	end
end

-- true when no later test is in this suite, so afterAll is due
local function isLastOfSuite(suite, index)
	for position = index + 1, #state.tests do
		local node = state.tests[position].parent
		while node ~= nil and node ~= root do
			if node == suite then
				return false
			end
			node = node.parent
		end
	end
	return true
end

function __microstudio_test_run(index)
	local test = state.tests[index]
	if test == nil then
		return { status = "failed", message = "no test at index " .. tostring(index), assertions = 0 }
	end

	local startedAt = time()
	local assertionsBefore = state.assertions

	if test.mode == "skip" or test.mode == "todo" then
		return {
			status = test.mode == "todo" and "todo" or "skipped",
			assertions = 0,
			durationSeconds = time() - startedAt,
		}
	end

	local chain = suiteChain(test)
	local setupFailures, teardownFailures = {}, {}

	for _, suite in ipairs(chain) do
		if not suite.beforeAllRun then
			suite.beforeAllRun = true
			runHooks(suite.beforeAll, "beforeAll", setupFailures)
		end
	end
	for _, suite in ipairs(chain) do
		runHooks(suite.beforeEach, "beforeEach", setupFailures)
	end

	local status = "passed"
	local message, traceback_ = nil, nil

	-- a failed setup leaves the world half-built, so the body does not run
	-- matches vitest/jest: a broken fixture must not look like a broken feature
	if #setupFailures > 0 then
		local failure = setupFailures[1]
		status = "failed"
		message = hookMessage(failure)
		traceback_ = failure.traceback
	elseif test.broken ~= nil then
		status = "failed"
		message = test.broken.location ~= nil
				and (test.broken.location .. ": " .. test.broken.message)
			or test.broken.message
		traceback_ = test.broken.traceback
	elseif type(test.fn) ~= "function" then
		status = "failed"
		message = "the test has no function body"
	else
		local broken = callCaught(test.fn)
		if broken ~= nil then
			status = "failed"
			message = broken.location ~= nil and (broken.location .. ": " .. broken.message)
				or broken.message
			traceback_ = broken.traceback
		end
	end

	for position = #chain, 1, -1 do
		runHooks(chain[position].afterEach, "afterEach", teardownFailures)
	end
	for position = #chain, 1, -1 do
		local suite = chain[position]
		if not suite.afterAllRun and isLastOfSuite(suite, index) then
			suite.afterAllRun = true
			runHooks(suite.afterAll, "afterAll", teardownFailures)
		end
	end

	if #teardownFailures > 0 and status == "passed" then
		local failure = teardownFailures[1]
		status = "failed"
		message = hookMessage(failure)
		traceback_ = failure.traceback
	end

	local hookFailures = {}
	for _, failure in ipairs(setupFailures) do
		table.insert(hookFailures, failure)
	end
	for _, failure in ipairs(teardownFailures) do
		table.insert(hookFailures, failure)
	end

	return {
		status = status,
		message = message,
		traceback = traceback_,
		assertions = state.assertions - assertionsBefore,
		durationSeconds = time() - startedAt,
		hookFailures = hookFailures,
	}
end

return { version = 1 }
