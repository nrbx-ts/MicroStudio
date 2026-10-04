// tables not userdata: Signal:Wait stays writable and can yield

use mlua::{Function, Lua, Result as LuaResult, Table, Value};
use microstudio_datamodel::{Signal, SignalId};

use crate::vm::{Ctx, LuaListener};

// identity-stable table, created on first use
pub fn signal_table(lua: &Lua, ctx: &Ctx, signal: &Signal) -> LuaResult<Table> {
    if let Some(table) = ctx.state.borrow().signal_tables.get(&signal.id()) {
        return Ok(table.clone());
    }

    let meta: Table = lua.named_registry_value("microstudio.signal_meta")?;
    let table = lua.create_table()?;
    table.raw_set("__microstudio_signal", signal.id().0)?;
    table.set_metatable(Some(meta))?;

    let mut state = ctx.state.borrow_mut();
    state.signals.insert(signal.id(), signal.clone());
    state.signal_tables.insert(signal.id(), table.clone());
    Ok(table)
}

pub fn connection_table(lua: &Lua, ctx: &Ctx, handle: u64, signal_id: SignalId) -> LuaResult<Table> {
    let meta: Table = lua.named_registry_value("microstudio.connection_meta")?;
    let table = lua.create_table()?;
    table.raw_set("__microstudio_connection", handle)?;
    table.raw_set("__microstudio_signal", signal_id.0)?;
    table.set_metatable(Some(meta))?;
    ctx.state
        .borrow_mut()
        .connection_tables
        .insert(handle, table.clone());
    Ok(table)
}

pub fn connect(ctx: &Ctx, signal_id: SignalId, function: Function, once: bool) -> u64 {
    let owner = ctx.current_script();
    let mut state = ctx.state.borrow_mut();
    let handle = state.next_connection;
    state.next_connection += 1;
    state
        .listeners
        .entry(signal_id)
        .or_default()
        .push((
            handle,
            LuaListener {
                function,
                owner,
                once,
            },
        ));
    state.connection_owner.insert(handle, signal_id);
    handle
}

// also drops any thread parked on the listener
pub fn disconnect(ctx: &Ctx, handle: u64) {
    let mut state = ctx.state.borrow_mut();
    state.connection_tables.remove(&handle);
    let Some(signal_id) = state.connection_owner.remove(&handle) else {
        return;
    };
    let mut now_empty = false;
    if let Some(list) = state.listeners.get_mut(&signal_id) {
        list.retain(|(h, _)| *h != handle);
        now_empty = list.is_empty();
    }
    if now_empty {
        state.listeners.remove(&signal_id);
    }
}

pub fn is_connected(ctx: &Ctx, handle: u64) -> bool {
    ctx.state.borrow().connection_owner.contains_key(&handle)
}

// primitives the prelude signal metatables call
pub fn install(lua: &Lua, ctx: &Ctx) -> LuaResult<()> {
    let globals = lua.globals();

    let connect_ctx = ctx.clone();
    globals.set(
        "__microstudio_signal_connect",
        lua.create_function(move |lua, (signal, function, once): (Table, Function, bool)| {
            let signal_id = SignalId(signal.raw_get::<u64>("__microstudio_signal")?);
            let handle = connect(&connect_ctx, signal_id, function, once);
            connection_table(lua, &connect_ctx, handle, signal_id)
        })?,
    )?;

    let disconnect_ctx = ctx.clone();
    globals.set(
        "__microstudio_connection_disconnect",
        lua.create_function(move |_, connection: Table| {
            let handle = connection.raw_get::<u64>("__microstudio_connection")?;
            disconnect(&disconnect_ctx, handle);
            Ok(())
        })?,
    )?;

    let connected_ctx = ctx.clone();
    globals.set(
        "__microstudio_connection_connected",
        lua.create_function(move |_, connection: Table| {
            let handle = connection.raw_get::<u64>("__microstudio_connection")?;
            Ok(is_connected(&connected_ctx, handle))
        })?,
    )?;

    let _ = Value::Nil;
    Ok(())
}
