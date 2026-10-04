// DataStoreService, MemoryStoreService and MessagingService
//
// data stores persist as plain json, one file per store name, so a user can seed them
// the way miniflare seeds a local binding:
//
//   .microstudio/datastores/<store name>.json
//     { "<scope>": { "<key>": <value> } }
//
// memory stores and messaging topics are session state: roblox drops both when the last
// server of a session stops, and a local runtime has exactly one session.

use mlua::{
    AnyUserData, Error as LuaError, Function, Lua, LuaSerdeExt, Result as LuaResult, Table, Value,
};
use microstudio_datamodel::{AttributeValue, EventArg, PendingEvent};
use microstudio_services::Store;
use serde_json::{json, Value as Json};

use crate::bind::instance::{instance_of, LuaInstance};
use crate::bind::services::{methods, require_class};
use crate::convert::lua_err;
use crate::vm::{self, Ctx};

// the scope is not a declared property, so it travels as an attribute
const SCOPE_ATTRIBUTE: &str = "MicroStudioScope";
// roblox caps a data store key at 50 characters
const MAX_KEY_CHARS: usize = 50;
// the expiry roblox documents when none is given: 45 days for maps, 30 for queue items
const MAP_TTL: f64 = 45.0 * 24.0 * 60.0 * 60.0;
const QUEUE_TTL: f64 = 30.0 * 24.0 * 60.0 * 60.0;
// a single message is capped at 1KB, and a topic at 80 characters
const MAX_MESSAGE_BYTES: usize = 1024;
const MAX_TOPIC_CHARS: usize = 80;

pub fn install(lua: &Lua, ctx: &Ctx) -> LuaResult<()> {
    let methods = methods(lua)?;

    lua.set_named_registry_value("microstudio.datastores", lua.create_table()?)?;
    lua.set_named_registry_value("microstudio.datastore_files", lua.create_table()?)?;
    lua.set_named_registry_value("microstudio.memory_stores", lua.create_table()?)?;
    lua.set_named_registry_value("microstudio.deviations", lua.create_table()?)?;

    install_data_store_service(lua, &methods)?;
    install_data_store(lua, &methods)?;
    install_memory_store_service(lua, &methods)?;
    install_memory_store(lua, &methods)?;
    install_messaging_service(lua, &methods)?;
    // last, so the shared names win over anything a single store registered
    install_shared_methods(lua, &methods)?;

    let _ = ctx;
    Ok(())
}

fn install_data_store_service(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "GetDataStore",
        lua.create_function(
            |lua,
             (ud, name, scope, _options): (AnyUserData, String, Option<String>, Option<Value>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "DataStoreService")?;
                store_instance(
                    lua,
                    &this.ctx,
                    "DataStore",
                    &name,
                    scope.as_deref().unwrap_or("global"),
                )
            },
        )?,
    )?;

    methods.set(
        "GetGlobalDataStore",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "DataStoreService")?;
            store_instance(lua, &this.ctx, "DataStore", "global", "global")
        })?,
    )?;

    methods.set(
        "GetOrderedDataStore",
        lua.create_function(
            |lua, (ud, name, scope): (AnyUserData, String, Option<String>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "DataStoreService")?;
                store_instance(
                    lua,
                    &this.ctx,
                    "OrderedDataStore",
                    &name,
                    scope.as_deref().unwrap_or("global"),
                )
            },
        )?,
    )?;

    methods.set(
        "GetRequestBudgetForRequestType",
        lua.create_function(|_, (ud, _kind): (AnyUserData, Option<Value>)| {
            let this = instance_of(&ud)?;
            require_class(&this, "DataStoreService")?;
            // the budget exists to throttle a real backend, and nothing here is remote
            Ok(60.0)
        })?,
    )?;

    methods.set(
        "ListDataStoresAsync",
        lua.create_function(
            |lua, (ud, prefix, _page_size): (AnyUserData, Option<String>, Option<f64>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "DataStoreService")?;
                // the state directory is the whole universe of stores
                let stores = {
                    let state = this.ctx.state.borrow();
                    state.world.store.list_json("datastores").map_err(lua_err)?
                };
                let names: Vec<Value> = stores
                    .into_iter()
                    .map(|(name, _)| name)
                    .filter(|name| prefix.as_deref().is_none_or(|prefix| name.starts_with(prefix)))
                    .filter_map(|name| lua.create_string(name).ok())
                    .map(Value::String)
                    .collect();
                Ok(Value::Table(pages_table(lua, names, 50)?))
            },
        )?,
    )?;

    Ok(())
}

fn install_data_store(lua: &Lua, methods: &Table) -> LuaResult<()> {
    // GetAsync, SetAsync, UpdateAsync and RemoveAsync are shared with the memory store,
    // so install_shared_methods registers those once and dispatches on the receiver

    methods.set(
        "IncrementAsync",
        lua.create_function(|lua, (ud, key, delta): (AnyUserData, String, Option<f64>)| {
            let this = instance_of(&ud)?;
            require_store(&this)?;
            check_key(&key)?;
            let entries = store_entries(lua, &this)?;
            let current = match entries.get::<Value>(key.as_str())? {
                Value::Nil => 0.0,
                other => match number_of(&other) {
                    Some(number) => number,
                    None => {
                        return Err(LuaError::runtime(format!(
                            "DataStore:IncrementAsync expects a number at {key:?}, found {}",
                            other.type_name()
                        )))
                    }
                },
            };
            let value = Value::Number(current + delta.unwrap_or(1.0));
            entries.set(key.as_str(), value.clone())?;
            save_store(lua, &this)?;
            Ok(value)
        })?,
    )?;

    methods.set(
        "ListKeysAsync",
        lua.create_function(
            |lua, (ud, prefix, _page_size): (AnyUserData, Option<String>, Option<f64>)| {
                let this = instance_of(&ud)?;
                require_store(&this)?;
                let entries = store_entries(lua, &this)?;
                let mut keys: Vec<String> = entries
                    .pairs::<String, Value>()
                    .flatten()
                    .filter(|(key, _)| {
                        prefix.as_deref().is_none_or(|prefix| key.starts_with(prefix))
                    })
                    .map(|(key, _)| key)
                    .collect();
                keys.sort();
                let rows: Vec<Value> = keys
                    .into_iter()
                    .filter_map(|key| lua.create_string(key).ok())
                    .map(Value::String)
                    .collect();
                Ok(Value::Table(pages_table(lua, rows, 50)?))
            },
        )?,
    )?;

    // only an ordered store keeps its entries sorted, so only it can page through them
    methods.set(
        "GetSortedAsync",
        lua.create_function(
            |lua,
             (ud, ascending, page_size, min_value, max_value): (
                AnyUserData,
                Option<bool>,
                Option<f64>,
                Option<f64>,
                Option<f64>,
            )| {
                let this = instance_of(&ud)?;
                require_class(&this, "OrderedDataStore")?;
                let entries = store_entries(lua, &this)?;
                let mut rows: Vec<(String, f64)> = entries
                    .pairs::<String, Value>()
                    .flatten()
                    .filter_map(|(key, value)| number_of(&value).map(|number| (key, number)))
                    .filter(|(_, number)| {
                        min_value.is_none_or(|min| *number >= min)
                            && max_value.is_none_or(|max| *number < max)
                    })
                    .collect();
                rows.sort_by(|(left_key, left), (right_key, right)| {
                    left.partial_cmp(right)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| left_key.cmp(right_key))
                });
                if !ascending.unwrap_or(true) {
                    rows.reverse();
                }

                let page: Vec<Value> = rows
                    .into_iter()
                    .map(|(key, value)| {
                        let row = lua.create_table()?;
                        row.set("key", key)?;
                        row.set("value", value)?;
                        Ok(Value::Table(row))
                    })
                    .collect::<LuaResult<Vec<Value>>>()?;
                let size = page_size.unwrap_or(50.0).max(1.0) as usize;
                Ok(Value::Table(pages_table(lua, page, size)?))
            },
        )?,
    )?;

    Ok(())
}

fn install_memory_store_service(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "GetQueue",
        lua.create_function(
            |lua, (ud, name, _invisibility): (AnyUserData, String, Option<f64>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "MemoryStoreService")?;
                store_instance(lua, &this.ctx, "MemoryStoreQueue", &name, "queue")
            },
        )?,
    )?;

    methods.set(
        "GetSortedMap",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "MemoryStoreService")?;
            store_instance(lua, &this.ctx, "MemoryStoreSortedMap", &name, "map")
        })?,
    )?;

    Ok(())
}

fn install_memory_store(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "AddAsync",
        lua.create_function(
            |lua,
             (ud, value, expiration, priority): (
                AnyUserData,
                Value,
                Option<f64>,
                Option<f64>,
            )| {
                let this = instance_of(&ud)?;
                require_class(&this, "MemoryStoreQueue")?;
                let name = instance_name(&this);
                let json = encode_value(lua, &value)?;
                let expires = this.ctx.now() + expiration.unwrap_or(QUEUE_TTL);
                let id = {
                    let mut state = this.ctx.state.borrow_mut();
                    state.world.memory.queue_add(
                        &format!("queue:{name}"),
                        json,
                        priority.unwrap_or(0.0),
                        Some(expires),
                    )
                };
                Ok(lua.create_string(id.to_string())?)
            },
        )?,
    )?;

    methods.set(
        "ReadAsync",
        lua.create_function(
            |lua,
             (ud, count, all_or_nothing, _wait): (
                AnyUserData,
                Option<f64>,
                Option<bool>,
                Option<f64>,
            )| {
                let this = instance_of(&ud)?;
                require_class(&this, "MemoryStoreQueue")?;
                let name = instance_name(&this);
                let count = count.unwrap_or(1.0).max(0.0) as usize;
                let now = this.ctx.now();
                let items = {
                    let mut state = this.ctx.state.borrow_mut();
                    state
                        .world
                        .memory
                        .queue_read(&format!("queue:{name}"), count, now)
                };
                // all-or-nothing asks for a full page or nothing at all
                let items = if all_or_nothing.unwrap_or(false) && items.len() < count {
                    Vec::new()
                } else {
                    items
                };

                let out = lua.create_table()?;
                for (index, (id, value)) in items.into_iter().enumerate() {
                    let row = lua.create_table()?;
                    row.set("id", id.to_string())?;
                    row.set("data", lua.to_value(&value)?)?;
                    out.set(index + 1, row)?;
                }
                Ok(out)
            },
        )?,
    )?;

    // queue and map share GetAsync/SetAsync/UpdateAsync/RemoveAsync with a data store,
    // so install_shared_methods registers those once and dispatches on the receiver

    methods.set(
        "GetRangeAsync",
        lua.create_function(
            |lua,
             (ud, direction, count, lower, upper): (
                AnyUserData,
                Value,
                Option<f64>,
                Option<Value>,
                Option<Value>,
            )| {
                let this = instance_of(&ud)?;
                require_class(&this, "MemoryStoreSortedMap")?;
                let name = instance_name(&this);
                let descending = match enum_name(&direction).as_deref() {
                    Some("Descending") => true,
                    Some("Ascending") | None => false,
                    Some(other) => {
                        return Err(LuaError::runtime(format!(
                            "GetRangeAsync expects an Enum.SortDirection, got {other}"
                        )))
                    }
                };
                let lower = lower.map(|value| encode_value(lua, &value)).transpose()?;
                let upper = upper.map(|value| encode_value(lua, &value)).transpose()?;
                let now = this.ctx.now();
                let rows = {
                    let mut state = this.ctx.state.borrow_mut();
                    state.world.memory.sorted_map_range(
                        &name,
                        descending,
                        count.unwrap_or(100.0).max(1.0) as usize,
                        lower.as_ref(),
                        upper.as_ref(),
                        now,
                    )
                };

                let out = lua.create_table()?;
                for (index, (key, value)) in rows.into_iter().enumerate() {
                    let row = lua.create_table()?;
                    row.set("key", key)?;
                    row.set("value", lua.to_value(&value)?)?;
                    out.set(index + 1, row)?;
                }
                Ok(out)
            },
        )?,
    )?;

    Ok(())
}

// a name more than one store shares is registered once, and the receiver decides
fn install_shared_methods(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "GetAsync",
        lua.create_function(|lua, (ud, key): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            match class_of(&this).as_str() {
                "DataStore" | "OrderedDataStore" => datastore_get(lua, &this, &key),
                "MemoryStoreSortedMap" => memory_map_get(lua, &this, &key),
                other => Err(not_a_store("GetAsync", other)),
            }
        })?,
    )?;

    methods.set(
        "SetAsync",
        lua.create_function(
            |lua, (ud, key, value, extra): (AnyUserData, String, Value, Option<Value>)| {
                let this = instance_of(&ud)?;
                match class_of(&this).as_str() {
                    // a data store tags the write with user ids, a memory map with an expiry
                    "DataStore" | "OrderedDataStore" => datastore_set(lua, &this, &key, value),
                    "MemoryStoreSortedMap" => {
                        memory_map_set(lua, &this, &key, value, as_seconds(&extra))
                    }
                    other => Err(not_a_store("SetAsync", other)),
                }
            },
        )?,
    )?;

    methods.set(
        "UpdateAsync",
        lua.create_function(
            |lua,
             (ud, key, transform, extra): (AnyUserData, String, Function, Option<Value>)| {
                let this = instance_of(&ud)?;
                match class_of(&this).as_str() {
                    "DataStore" | "OrderedDataStore" => datastore_update(lua, &this, &key, transform),
                    "MemoryStoreSortedMap" => {
                        memory_map_update(lua, &this, &key, transform, as_seconds(&extra))
                    }
                    other => Err(not_a_store("UpdateAsync", other)),
                }
            },
        )?,
    )?;

    methods.set(
        "RemoveAsync",
        lua.create_function(|lua, (ud, key): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            match class_of(&this).as_str() {
                // a queue remove takes the item id a read handed out
                "MemoryStoreQueue" => queue_remove(&this, &key),
                "DataStore" | "OrderedDataStore" => datastore_remove(lua, &this, &key),
                "MemoryStoreSortedMap" => memory_map_remove(lua, &this, &key),
                other => Err(not_a_store("RemoveAsync", other)),
            }
        })?,
    )?;

    Ok(())
}

fn datastore_get(lua: &Lua, this: &LuaInstance, key: &str) -> LuaResult<Value> {
    require_store(this)?;
    check_key(key)?;
    let entries = store_entries(lua, this)?;
    Ok(entries.get::<Value>(key)?)
}

fn datastore_set(lua: &Lua, this: &LuaInstance, key: &str, value: Value) -> LuaResult<()> {
    require_store(this)?;
    check_key(key)?;
    if value.is_nil() {
        return Err(LuaError::runtime(
            "DataStore:SetAsync cannot store nil, use RemoveAsync to delete a key",
        ));
    }
    encode_value(lua, &value)?;
    let entries = store_entries(lua, this)?;
    entries.set(key, value)?;
    save_store(lua, this)
}

fn datastore_update(
    lua: &Lua,
    this: &LuaInstance,
    key: &str,
    transform: Function,
) -> LuaResult<Value> {
    require_store(this)?;
    check_key(key)?;
    let entries = store_entries(lua, this)?;
    let current: Value = entries.get(key)?;
    let updated: Value = transform.call(current)?;
    // a transform that returns nil cancels the write, as roblox documents
    if updated.is_nil() {
        return Ok(Value::Nil);
    }
    encode_value(lua, &updated)?;
    entries.set(key, updated.clone())?;
    save_store(lua, this)?;
    Ok(updated)
}

fn datastore_remove(lua: &Lua, this: &LuaInstance, key: &str) -> LuaResult<Value> {
    require_store(this)?;
    check_key(key)?;
    let entries = store_entries(lua, this)?;
    let removed: Value = entries.get(key)?;
    if removed.is_nil() {
        return Ok(Value::Nil);
    }
    entries.set(key, Value::Nil)?;
    save_store(lua, this)?;
    Ok(removed)
}

// a queue remove hands nothing back, a store remove hands back what it took away
fn queue_remove(this: &LuaInstance, id: &str) -> LuaResult<Value> {
    require_class(this, "MemoryStoreQueue")?;
    let item: u64 = id.parse().map_err(|_| {
        LuaError::runtime(format!("MemoryStoreQueue:RemoveAsync: {id:?} is not an item id"))
    })?;
    let name = instance_name(this);
    let now = this.ctx.now();
    this.ctx
        .state
        .borrow_mut()
        .world
        .memory
        .queue_remove(&format!("queue:{name}"), item, now);
    Ok(Value::Nil)
}

fn memory_map_set(
    lua: &Lua,
    this: &LuaInstance,
    key: &str,
    value: Value,
    expiration: Option<f64>,
) -> LuaResult<()> {
    require_class(this, "MemoryStoreSortedMap")?;
    let name = instance_name(this);
    let json = encode_value(lua, &value)?;
    let expires = this.ctx.now() + expiration.unwrap_or(MAP_TTL);
    this.ctx
        .state
        .borrow_mut()
        .world
        .memory
        .sorted_map_set(&name, key, json, Some(expires))
        .map_err(LuaError::runtime)
}

fn memory_map_get(lua: &Lua, this: &LuaInstance, key: &str) -> LuaResult<Value> {
    require_class(this, "MemoryStoreSortedMap")?;
    let name = instance_name(this);
    let now = this.ctx.now();
    let found = {
        let mut state = this.ctx.state.borrow_mut();
        state.world.memory.sorted_map_get(&name, key, now)
    };
    match found {
        Some(value) => lua.to_value(&value),
        None => Ok(Value::Nil),
    }
}

fn memory_map_update(
    lua: &Lua,
    this: &LuaInstance,
    key: &str,
    transform: Function,
    expiration: Option<f64>,
) -> LuaResult<Value> {
    require_class(this, "MemoryStoreSortedMap")?;
    let name = instance_name(this);
    let now = this.ctx.now();
    let current = memory_map_get(lua, this, key)?;
    let updated: Value = transform.call(current)?;
    if updated.is_nil() {
        return Ok(Value::Nil);
    }
    let json = encode_value(lua, &updated)?;
    let expires = now + expiration.unwrap_or(MAP_TTL);
    this.ctx
        .state
        .borrow_mut()
        .world
        .memory
        .sorted_map_set(&name, key, json, Some(expires))
        .map_err(LuaError::runtime)?;
    Ok(updated)
}

fn memory_map_remove(lua: &Lua, this: &LuaInstance, key: &str) -> LuaResult<Value> {
    require_class(this, "MemoryStoreSortedMap")?;
    let name = instance_name(this);
    let now = this.ctx.now();
    let removed = {
        let mut state = this.ctx.state.borrow_mut();
        state.world.memory.sorted_map_remove(&name, key, now)
    };
    match removed {
        Some(value) => lua.to_value(&value),
        None => Ok(Value::Nil),
    }
}

fn class_of(this: &LuaInstance) -> String {
    this.ctx
        .state
        .borrow()
        .world
        .dm
        .class_of(this.id)
        .unwrap_or("<destroyed>")
        .to_string()
}

fn not_a_store(method: &str, class: &str) -> LuaError {
    LuaError::runtime(format!("{method} is not valid on {class}"))
}

// luau hands back an integral number as an integer, so both kinds count as a number
fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => Some(*number),
        Value::Integer(number) => Some(*number as f64),
        _ => None,
    }
}

// the fourth argument differs by store: a number of seconds, or a table of user ids
fn as_seconds(value: &Option<Value>) -> Option<f64> {
    value.as_ref().and_then(number_of)
}

fn install_messaging_service(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "PublishAsync",
        lua.create_function(|lua, (ud, topic, message): (AnyUserData, String, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "MessagingService")?;
            check_topic(&topic)?;
            if message.len() > MAX_MESSAGE_BYTES {
                return Err(LuaError::runtime(format!(
                    "message is {} bytes, the limit is {MAX_MESSAGE_BYTES}",
                    message.len()
                )));
            }
            note_once(
                lua,
                &this.ctx,
                "MessagingService",
                "MessagingService is local: a message reaches subscribers in this runtime only",
            )?;

            let signal = this
                .ctx
                .state
                .borrow_mut()
                .world
                .dm
                .named_signal(this.id, &topic)
                .map_err(lua_err)?;
            vm::dispatch(
                lua,
                &this.ctx,
                vec![PendingEvent {
                    signal,
                    args: vec![EventArg::Text(message)],
                }],
            )
            .map_err(lua_err)?;
            Ok(())
        })?,
    )?;

    methods.set(
        "SubscribeAsync",
        lua.create_function(|lua, (ud, topic, callback): (AnyUserData, String, Function)| {
            let this = instance_of(&ud)?;
            require_class(&this, "MessagingService")?;
            check_topic(&topic)?;

            let signal = this
                .ctx
                .state
                .borrow_mut()
                .world
                .dm
                .named_signal(this.id, &topic)
                .map_err(lua_err)?;

            // the callback takes the message dictionary roblox hands out, so wrap it
            let wrapper_ctx = this.ctx.clone();
            let wrapper = lua.create_function(move |lua, message: String| {
                let payload = lua.create_table()?;
                payload.set("Data", message)?;
                payload.set("Sent", wrapper_ctx.now())?;
                callback.call::<()>(payload)
            })?;

            let handle = crate::bind::signal::connect(&this.ctx, signal.id(), wrapper, false);
            let connection =
                crate::bind::signal::connection_table(lua, &this.ctx, handle, signal.id())?;
            Ok(Value::Table(connection))
        })?,
    )?;

    Ok(())
}

// one instance per name and scope, so a script sees the same object back
fn store_instance(lua: &Lua, ctx: &Ctx, class: &str, name: &str, scope: &str) -> LuaResult<Value> {
    if name.chars().count() > MAX_KEY_CHARS {
        return Err(LuaError::runtime(format!(
            "a store name is limited to {MAX_KEY_CHARS} characters, got {}",
            name.chars().count()
        )));
    }
    let key = format!("{class}\u{1}{name}\u{1}{scope}");
    // memory stores are session state, data stores are backed by the state directory
    let registry = if class.starts_with("MemoryStore") {
        "microstudio.memory_stores"
    } else {
        "microstudio.datastores"
    };
    let cache: Table = lua.named_registry_value(registry)?;
    if let Some(existing) = cache.get::<Option<Value>>(key.as_str())? {
        return Ok(existing);
    }

    let id = {
        let mut state = ctx.state.borrow_mut();
        state.world.dm.create_internal(class).map_err(lua_err)?
    };
    let events = {
        let mut state = ctx.state.borrow_mut();
        let dm = &mut state.world.dm;
        dm.set_name(id, name).map_err(lua_err)?;
        dm.set_attribute(
            id,
            SCOPE_ATTRIBUTE,
            Some(AttributeValue::String(scope.to_string())),
        )
        .map_err(lua_err)?
    };
    vm::dispatch(lua, ctx, events).map_err(lua_err)?;

    let value = vm::instance_value(lua, ctx, id)?;
    cache.set(key.as_str(), value.clone())?;
    Ok(value)
}

// a store method is shared by DataStore and OrderedDataStore
fn require_store(this: &LuaInstance) -> LuaResult<()> {
    let class = this
        .ctx
        .state
        .borrow()
        .world
        .dm
        .class_of(this.id)
        .unwrap_or("<destroyed>")
        .to_string();
    match class.as_str() {
        "DataStore" | "OrderedDataStore" => Ok(()),
        other => Err(LuaError::runtime(format!(
            "this method is only valid on a DataStore, not {other}"
        ))),
    }
}

fn instance_name(this: &LuaInstance) -> String {
    this.ctx
        .state
        .borrow()
        .world
        .dm
        .name_of(this.id)
        .unwrap_or("")
        .to_string()
}

fn check_key(key: &str) -> LuaResult<()> {
    if key.is_empty() {
        return Err(LuaError::runtime("a data store key cannot be empty"));
    }
    if key.chars().count() > MAX_KEY_CHARS {
        return Err(LuaError::runtime(format!(
            "key {key:?} is {} characters, the limit is {MAX_KEY_CHARS}",
            key.chars().count()
        )));
    }
    Ok(())
}

fn check_topic(topic: &str) -> LuaResult<()> {
    if topic.is_empty() {
        return Err(LuaError::runtime("a messaging topic cannot be empty"));
    }
    if topic.chars().count() > MAX_TOPIC_CHARS {
        return Err(LuaError::runtime(format!(
            "topic {topic:?} is {} characters, the limit is {MAX_TOPIC_CHARS}",
            topic.chars().count()
        )));
    }
    Ok(())
}

// roblox stores json, so anything that cannot be encoded is rejected before it is written
fn encode_value(lua: &Lua, value: &Value) -> LuaResult<Json> {
    lua.from_value(value.clone())
        .map_err(|error| LuaError::runtime(format!("value cannot be stored: {error}")))
}

fn enum_name(value: &Value) -> Option<String> {
    let table = match value {
        Value::Table(table) => table,
        _ => return None,
    };
    table.get::<Option<String>>("Name").ok().flatten()
}

fn store_path(name: &str) -> String {
    format!("datastores/{}.json", Store::file_name(name))
}

// scope -> key -> value, read once per vm and kept in step with the file on disk
fn store_file(lua: &Lua, ctx: &Ctx, name: &str) -> LuaResult<Table> {
    let path = store_path(name);
    let files: Table = lua.named_registry_value("microstudio.datastore_files")?;
    if let Some(existing) = files.get::<Option<Table>>(path.as_str())? {
        return Ok(existing);
    }

    let json = {
        let state = ctx.state.borrow();
        state
            .world
            .store
            .read_json(&path)
            .map_err(lua_err)?
            // an empty file, or none at all, starts as an empty object
            .unwrap_or_else(|| json!({}))
    };
    let table = match lua.to_value(&json)? {
        // a hand-edited file that is not an object is treated as empty
        Value::Table(table) => table,
        _ => lua.create_table()?,
    };
    files.set(path.as_str(), table.clone())?;
    Ok(table)
}

fn store_entries(lua: &Lua, this: &LuaInstance) -> LuaResult<Table> {
    let name = instance_name(this);
    let scope = {
        let state = this.ctx.state.borrow();
        match state.world.dm.get_attribute(this.id, SCOPE_ATTRIBUTE) {
            Some(AttributeValue::String(scope)) => scope,
            _ => "global".to_string(),
        }
    };

    let file = store_file(lua, &this.ctx, &name)?;
    if let Some(existing) = file.get::<Option<Table>>(scope.as_str())? {
        return Ok(existing);
    }
    let entries = lua.create_table()?;
    file.set(scope.as_str(), entries.clone())?;
    Ok(entries)
}

fn save_store(lua: &Lua, this: &LuaInstance) -> LuaResult<()> {
    let store = this.ctx.state.borrow().world.store.clone();
    if !store.is_persistent() {
        return Ok(());
    }
    let name = instance_name(this);
    let file = store_file(lua, &this.ctx, &name)?;
    let json: Json = lua.from_value(Value::Table(file)).map_err(lua_err)?;
    store.write_json(&store_path(&name), &json).map_err(lua_err)
}

// a pages object: one page at a time, advancing raises once the rows run out
fn pages_table(lua: &Lua, rows: Vec<Value>, page_size: usize) -> LuaResult<Table> {
    let table = lua.create_table()?;
    let all = lua.create_table()?;
    for (index, row) in rows.into_iter().enumerate() {
        all.set(index + 1, row)?;
    }
    table.raw_set("__rows", all)?;
    table.raw_set("__size", page_size.max(1))?;
    table.raw_set("__index", 1)?;
    table.set("IsFinished", false)?;

    table.set(
        "GetCurrentPage",
        lua.create_function(|lua, this: Table| {
            let all: Table = this.raw_get("__rows")?;
            let size: usize = this.raw_get("__size")?;
            let index: usize = this.raw_get("__index")?;
            let start = (index - 1) * size + 1;
            let page = lua.create_table()?;
            for offset in 0..size {
                match all.raw_get::<Value>(start + offset)? {
                    Value::Nil => break,
                    value => page.raw_set(offset + 1, value)?,
                }
            }
            Ok(page)
        })?,
    )?;

    table.set(
        "AdvanceToNextPageAsync",
        lua.create_function(|_, this: Table| {
            let all: Table = this.raw_get("__rows")?;
            let size: usize = this.raw_get("__size")?;
            let index: usize = this.raw_get("__index")?;
            if index * size >= all.raw_len() {
                this.set("IsFinished", true)?;
                return Err(LuaError::runtime(
                    "Pages:AdvanceToNextPageAsync() failed because there are no more pages",
                ));
            }
            this.raw_set("__index", index + 1)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}

// one note per deviation, so a long run does not repeat itself
fn note_once(lua: &Lua, ctx: &Ctx, key: &str, message: &str) -> LuaResult<()> {
    let noted: Table = lua.named_registry_value("microstudio.deviations")?;
    if noted.get::<bool>(key)? {
        return Ok(());
    }
    noted.set(key, true)?;
    ctx.state.borrow_mut().scheduler.warn(message.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{Vm, VmOptions};

    fn vm() -> Vm {
        Vm::new(VmOptions::default()).expect("vm boots")
    }

    fn eval_string(vm: &Vm, code: &str) -> String {
        match vm.eval(code).unwrap() {
            Value::String(text) => text.to_str().unwrap().to_string(),
            other => panic!(
                "expected a string, got {} (recorded errors: {:?})",
                other.type_name(),
                vm.ctx().state.borrow().world.errors
            ),
        }
    }

    fn eval_error(vm: &Vm, code: &str) -> String {
        let source = format!(
            "local ok, message = pcall(function() return {code} end)\n\
             assert(not ok, 'expected an error')\n\
             return tostring(message)"
        );
        eval_string(vm, &source)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("microstudio-data-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn vm_at(dir: &std::path::Path) -> Vm {
        Vm::new(VmOptions {
            store: Store::at(dir),
            ..Default::default()
        })
        .expect("vm boots")
    }

    #[test]
    fn a_value_round_trips_through_a_store() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local store = game:GetService("DataStoreService"):GetDataStore("players")
                assert(store:GetAsync("one") == nil)
                store:SetAsync("one", {coins = 5})
                return {store:GetAsync("one").coins, store.Name, store.ClassName}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<i64>(1).unwrap(), 5);
        assert_eq!(table.get::<String>(2).unwrap(), "players");
        assert_eq!(table.get::<String>(3).unwrap(), "DataStore");
    }

    #[test]
    fn the_same_name_and_scope_hands_back_the_same_store() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local service = game:GetService("DataStoreService")
                local first = service:GetDataStore("a")
                local second = service:GetDataStore("a")
                local other = service:GetDataStore("a", "season2")
                first:SetAsync("k", 1)
                return {
                    first == second,
                    first ~= other,
                    other:GetAsync("k") == nil,
                    service:GetDataStore("a") == first,
                }
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        for index in 1..=4 {
            assert!(table.get::<bool>(index).unwrap(), "check {index} failed");
        }
    }

    #[test]
    fn values_survive_a_new_vm() {
        let dir = scratch("persist");
        {
            let vm = vm_at(&dir);
            vm.eval(
                r#"
                local store = game:GetService("DataStoreService"):GetDataStore("players")
                store:SetAsync("one", {coins = 5})
                "#,
            )
            .unwrap();
        }

        let vm = vm_at(&dir);
        let coins = vm
            .eval(
                r#"return game:GetService("DataStoreService"):GetDataStore("players"):GetAsync("one").coins"#,
            )
            .unwrap();
        assert_eq!(coins, Value::Integer(5));

        // the file is plain json a user can read and edit
        let seeded = std::fs::read_to_string(dir.join("datastores/players.json")).unwrap();
        assert!(seeded.contains("\"one\""), "{seeded}");
        assert!(seeded.contains("global"), "{seeded}");
    }

    #[test]
    fn a_hand_written_file_is_read_as_seed_data() {
        let dir = scratch("seed");
        let store = Store::at(&dir);
        store
            .write_json("datastores/scores.json", &json!({ "global": { "top": 42 } }))
            .unwrap();

        let vm = vm_at(&dir);
        let top = vm
            .eval(
                r#"return game:GetService("DataStoreService"):GetDataStore("scores"):GetAsync("top")"#,
            )
            .unwrap();
        assert_eq!(top, Value::Integer(42));
    }

    #[test]
    fn update_async_sees_the_old_value_and_nil_cancels() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local store = game:GetService("DataStoreService"):GetDataStore("counters")
                store:SetAsync("hits", 2)
                local bumped = store:UpdateAsync("hits", function(old) return old + 3 end)
                local blocked = store:UpdateAsync("hits", function() return nil end)
                return {bumped, store:GetAsync("hits"), blocked}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<i64>(1).unwrap(), 5);
        assert_eq!(table.get::<i64>(2).unwrap(), 5);
        assert!(table.get::<Value>(3).unwrap().is_nil());
    }

    #[test]
    fn increment_counts_up_and_remove_hands_back_the_old_value() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local store = game:GetService("DataStoreService"):GetDataStore("counters")
                local first = store:IncrementAsync("n", 2)
                local second = store:IncrementAsync("n")
                local removed = store:RemoveAsync("n")
                return {first, second, removed, store:GetAsync("n") == nil}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<f64>(1).unwrap(), 2.0);
        assert_eq!(table.get::<f64>(2).unwrap(), 3.0);
        assert_eq!(table.get::<f64>(3).unwrap(), 3.0);
        assert!(table.get::<bool>(4).unwrap());
    }

    #[test]
    fn bad_keys_and_unstorable_values_raise() {
        let vm = vm();
        let long_key = eval_error(
            &vm,
            r#"game:GetService("DataStoreService"):GetDataStore("d"):SetAsync(string.rep("k", 51), 1)"#,
        );
        assert!(long_key.contains("limit"), "{long_key}");

        let nil_value = eval_error(
            &vm,
            r#"game:GetService("DataStoreService"):GetDataStore("d"):SetAsync("k", nil)"#,
        );
        assert!(nil_value.contains("cannot store nil"), "{nil_value}");

        let function_value = eval_error(
            &vm,
            r#"game:GetService("DataStoreService"):GetDataStore("d"):SetAsync("k", function() end)"#,
        );
        assert!(
            function_value.contains("cannot be stored"),
            "{function_value}"
        );
    }

    #[test]
    fn an_ordered_store_sorts_and_pages() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local store = game:GetService("DataStoreService"):GetOrderedDataStore("scores")
                store:SetAsync("a", 3)
                store:SetAsync("b", 1)
                store:SetAsync("c", 2)

                local ascending = store:GetSortedAsync(true, 2)
                local page = ascending:GetCurrentPage()
                local first, second = page[1].key, page[2].key
                ascending:AdvanceToNextPageAsync()
                local third = ascending:GetCurrentPage()[1].key

                local descending = store:GetSortedAsync(false, 10)
                local top = descending:GetCurrentPage()[1].key

                local bounded = store:GetSortedAsync(true, 10, 2, 3)
                local only = bounded:GetCurrentPage()[1].key
                return {first, page[1].value, second, third, top, only}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<String>(1).unwrap(), "b");
        assert_eq!(table.get::<f64>(2).unwrap(), 1.0);
        assert_eq!(table.get::<String>(3).unwrap(), "c");
        assert_eq!(table.get::<String>(4).unwrap(), "a");
        assert_eq!(table.get::<String>(5).unwrap(), "a");
        // minValue is inclusive and maxValue exclusive
        assert_eq!(table.get::<String>(6).unwrap(), "c");
    }

    #[test]
    fn advancing_past_the_last_page_raises() {
        let vm = vm();
        // eval_error wraps the code in `return ...`, so hand it one expression
        let message = eval_error(
            &vm,
            r#"(function()
                local store = game:GetService("DataStoreService"):GetOrderedDataStore("scores")
                store:SetAsync("a", 1)
                return store:GetSortedAsync(true, 10):AdvanceToNextPageAsync()
            end)()"#,
        );
        assert!(message.contains("no more pages"), "{message}");
    }

    #[test]
    fn a_memory_store_map_expires_on_the_virtual_clock() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local map = game:GetService("MemoryStoreService"):GetSortedMap("scores")
                map:SetAsync("a", 1, 10)
                local before = map:GetAsync("a")
                local updated = map:UpdateAsync("a", function(old) return old + 1 end, 10)
                return {before, updated}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<f64>(1).unwrap(), 1.0);
        assert_eq!(table.get::<f64>(2).unwrap(), 2.0);

        vm.advance_time(11.0).unwrap();
        let gone = vm
            .eval(
                r#"return game:GetService("MemoryStoreService"):GetSortedMap("scores"):GetAsync("a") == nil"#,
            )
            .unwrap();
        assert_eq!(gone, Value::Boolean(true));
    }

    #[test]
    fn a_memory_store_range_follows_sort_direction() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local map = game:GetService("MemoryStoreService"):GetSortedMap("scores")
                map:SetAsync("a", 3, 60)
                map:SetAsync("b", 1, 60)
                map:SetAsync("c", 2, 60)
                local up = map:GetRangeAsync(Enum.SortDirection.Ascending, 10)
                local down = map:GetRangeAsync(Enum.SortDirection.Descending, 10)
                return {up[1].key, up[3].key, down[1].key, down[3].key}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<String>(1).unwrap(), "b");
        assert_eq!(table.get::<String>(2).unwrap(), "a");
        assert_eq!(table.get::<String>(3).unwrap(), "a");
        assert_eq!(table.get::<String>(4).unwrap(), "b");
    }

    #[test]
    fn a_queue_reads_by_priority_and_removes_by_id() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local queue = game:GetService("MemoryStoreService"):GetQueue("jobs")
                queue:AddAsync("low", 60, 0)
                local high_id = queue:AddAsync("high", 60, 5)

                local items = queue:ReadAsync(2)
                local first, second = items[1].data, items[2].data

                queue:RemoveAsync(high_id)
                local after = queue:ReadAsync(2)
                return {first, second, #after, after[1].data}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<String>(1).unwrap(), "high");
        assert_eq!(table.get::<String>(2).unwrap(), "low");
        assert_eq!(table.get::<i64>(3).unwrap(), 1);
        assert_eq!(table.get::<String>(4).unwrap(), "low");
    }

    #[test]
    fn a_topic_delivers_the_message_dictionary() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local messaging = game:GetService("MessagingService")
                local seen = nil
                local connection = messaging:SubscribeAsync("updates", function(payload)
                    seen = payload.Data
                end)
                messaging:PublishAsync("updates", "hello")
                connection:Disconnect()
                messaging:PublishAsync("updates", "ignored")
                return {seen, connection.Connected}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<String>(1).unwrap(), "hello");
        assert!(!table.get::<bool>(2).unwrap());
    }

    #[test]
    fn a_long_message_is_rejected() {
        let vm = vm();
        let message = eval_error(
            &vm,
            r#"game:GetService("MessagingService"):PublishAsync("topic", string.rep("x", 1025))"#,
        );
        assert!(message.contains("limit"), "{message}");
    }

    #[test]
    fn a_store_method_on_another_service_raises() {
        let vm = vm();
        let message = eval_error(&vm, r#"game:GetService("Players"):GetAsync("k")"#);
        assert!(message.contains("is not valid on"), "{message}");
    }

    #[test]
    fn listing_stores_reports_the_files_on_disk() {
        let dir = scratch("listing");
        let store = Store::at(&dir);
        store
            .write_json("datastores/scores.json", &json!({ "global": {} }))
            .unwrap();
        store
            .write_json("datastores/saves.json", &json!({ "global": {} }))
            .unwrap();

        let vm = vm_at(&dir);
        let value = vm
            .eval(
                r#"
                local pages = game:GetService("DataStoreService"):ListDataStoresAsync()
                local page = pages:GetCurrentPage()
                return {#page, page[1], page[2], pages.IsFinished}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert_eq!(table.get::<i64>(1).unwrap(), 2);
        assert_eq!(table.get::<String>(2).unwrap(), "saves");
        assert_eq!(table.get::<String>(3).unwrap(), "scores");
    }
}
