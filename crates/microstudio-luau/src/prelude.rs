// anything that yields lives here: mlua can't unwind a yield through rust

pub const PRELUDE: &str = r#"
-- runtime prelude, loaded once before any user script
local pack = table.pack

function print(...) __microstudio_log("print", ...) end
function warn(...) __microstudio_log("warn", ...) end

function tick() return __microstudio_epoch() + __microstudio_clock() end
function time() return __microstudio_clock() end
function elapsedTime() return __microstudio_clock() end
os.clock = function() return __microstudio_clock() end
os.time = function() return math.floor(__microstudio_epoch() + __microstudio_clock()) end

task = {}
local DEFAULT_WAIT = 1 / 60

function task.wait(seconds)
    if seconds == nil then seconds = DEFAULT_WAIT end
    return coroutine.yield({ __microstudio = "wait", seconds = seconds })
end

function task.spawn(fn, ...)
    if type(fn) ~= "function" then
        error("invalid argument #1 to 'spawn' (function expected)", 2)
    end
    return __microstudio_task_run("spawn", fn, pack(...))
end

function task.defer(fn, ...)
    if type(fn) ~= "function" then
        error("invalid argument #1 to 'defer' (function expected)", 2)
    end
    return __microstudio_task_run("defer", fn, pack(...))
end

function task.delay(seconds, fn, ...)
    if type(fn) ~= "function" then
        error("invalid argument #2 to 'delay' (function expected)", 2)
    end
    return __microstudio_task_delay(seconds, fn, pack(...))
end

function task.cancel(thread)
    if thread ~= nil then __microstudio_task_cancel(thread) end
end

-- legacy spellings
function wait(seconds) return task.wait(seconds) end
function spawn(fn, ...) return task.spawn(fn, ...) end
function delay(seconds, fn, ...) return task.delay(seconds, fn, ...) end

-- enums are materialised on demand, values assigned per type in first-use order
local enumTypes = {}

local function getEnumType(name)
    local existing = enumTypes[name]
    if existing then return existing end

    local items = {}
    local nextValue = 0
    local enumType = { Name = name }

    local proxy = {}
    proxy.GetEnumItems = function()
        local out = {}
        for _, item in pairs(items) do
            table.insert(out, item)
        end
        table.sort(out, function(a, b) return a.Value < b.Value end)
        return out
    end
    setmetatable(proxy, {
        __index = function(_, itemName)
            local item = items[itemName]
            if item then return item end
            item = setmetatable(
                { Name = itemName, Value = nextValue, EnumType = enumType },
                { __tostring = function() return "Enum." .. name .. "." .. itemName end }
            )
            nextValue = nextValue + 1
            items[itemName] = item
            return item
        end,
        __tostring = function() return "Enum." .. name end,
    })

    enumTypes[name] = proxy
    return proxy
end

Enum = setmetatable({}, {
    __index = function(self, name)
        local enumType = getEnumType(name)
        rawset(self, name, enumType)
        return enumType
    end,
})

local SignalMeta = {}
SignalMeta.__index = SignalMeta

function SignalMeta:Connect(fn)
    if type(fn) ~= "function" then
        error("invalid argument #1 to 'Connect' (function expected)", 2)
    end
    return __microstudio_signal_connect(self, fn, false)
end

function SignalMeta:Once(fn)
    if type(fn) ~= "function" then
        error("invalid argument #1 to 'Once' (function expected)", 2)
    end
    return __microstudio_signal_connect(self, fn, true)
end

function SignalMeta:Wait()
    -- the fired arguments come back as the yield's return values
    local packed = pack(coroutine.yield({ __microstudio = "signal", signal = self }))
    return table.unpack(packed, 1, packed.n)
end

local ConnectionMeta = {}
ConnectionMeta.__index = function(self, key)
    if key == "Disconnect" then
        return ConnectionMeta.Disconnect
    elseif key == "Connected" then
        return __microstudio_connection_connected(self)
    end
    return nil
end

function ConnectionMeta.Disconnect(self)
    __microstudio_connection_disconnect(self)
end

local LuaInstanceMethods = {}

function LuaInstanceMethods:WaitForChild(name, timeout)
    if type(name) ~= "string" then
        error("invalid argument #1 to 'WaitForChild' (string expected)", 2)
    end
    -- return it if it is already there, only yield when it has to
    local existing = self:FindFirstChild(name)
    if existing ~= nil then
        return existing
    end

    local started = time()
    while true do
        local child = self:FindFirstChild(name)
        if child ~= nil then
            return child
        end
        if timeout ~= nil and (time() - started) >= timeout then
            return nil
        end
        self.ChildAdded:Wait()
    end
end

-- require goes to the rust loader, which caches per modulescript
function require(target)
    return __microstudio_require(target)
end

return {
    signal_meta = SignalMeta,
    connection_meta = ConnectionMeta,
    instance_methods = LuaInstanceMethods,
}
"#;
