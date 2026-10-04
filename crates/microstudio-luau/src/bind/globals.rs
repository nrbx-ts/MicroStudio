// task.spawn/delay use a real luau thread; a rust wrapper would block yields

use mlua::{
    Error as LuaError, Function, Lua, MultiValue, Result as LuaResult, Table, Thread, Value,
};
use microstudio_datamodel::InstanceId;
use microstudio_services::LogLevel;

use crate::scheduler::{Entry, ResumeKind};
use crate::vm::{self, Ctx, ResumeOutcome};

fn packed_args(table: &Table) -> LuaResult<MultiValue> {
    let count = match table.raw_get::<Option<i64>>("n")? {
        Some(n) => n.max(0) as usize,
        None => table.raw_len(),
    };
    let mut args = MultiValue::new();
    for index in 1..=count {
        args.push_back(table.raw_get::<Value>(index)?);
    }
    Ok(args)
}

pub fn install(lua: &Lua, ctx: &Ctx) -> LuaResult<()> {
    let globals = lua.globals();

    let instance = lua.create_table()?;
    let new_ctx = ctx.clone();
    instance.set(
        "new",
        lua.create_function(move |lua, (class_name, parent): (String, Option<Value>)| {
            let id = {
                let mut state = new_ctx.state.borrow_mut();
                state
                    .world
                    .dm
                    .create_instance(&class_name)
                    .map_err(crate::convert::lua_err)?
            };
            let value = vm::instance_value(lua, &new_ctx, id)?;
            if let Some(parent) = parent.filter(|p| !p.is_nil()) {
                let parent_id = crate::convert::take_instance(&parent)?;
                let events = new_ctx
                    .state
                    .borrow_mut()
                    .world
                    .dm
                    .set_parent(id, Some(parent_id))
                    .map_err(crate::convert::lua_err)?;
                vm::dispatch(lua, &new_ctx, events)?;
            }
            Ok(value)
        })?,
    )?;
    globals.set("Instance", instance)?;

    let log_ctx = ctx.clone();
    globals.set(
        "__microstudio_log",
        lua.create_function(move |lua, (level, args): (String, MultiValue)| {
            let text = args
                .iter()
                .map(|value| crate::convert::tostring_for_log(lua, value))
                .collect::<Vec<_>>()
                .join(" ");
            let source = log_ctx.current_script().map(|id| vm::script_name(&log_ctx, Some(id)));
            let level = match level.as_str() {
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                _ => LogLevel::Print,
            };
            log_ctx.state.borrow_mut().world.log(level, text, source);
            Ok(())
        })?,
    )?;

    let clock_ctx = ctx.clone();
    globals.set(
        "__microstudio_clock",
        lua.create_function(move |_, ()| Ok(clock_ctx.now()))?,
    )?;
    let epoch_ctx = ctx.clone();
    globals.set(
        "__microstudio_epoch",
        lua.create_function(move |_, ()| Ok(epoch_ctx.state.borrow().epoch))?,
    )?;

    // reset() gives each test a clean world; assertion state lives in the registry
    let reset_ctx = ctx.clone();
    globals.set(
        "__microstudio_reset_world",
        lua.create_function(move |lua, ()| {
            vm::reset_world(lua, &reset_ctx).map_err(|error| LuaError::runtime(error.to_string()))
        })?,
    )?;

    globals.set(
        "__microstudio_test_state",
        lua.create_function(|lua, ()| {
            // same table every time: a world reset must not lose the pending test list
            if let Ok(existing) = lua.named_registry_value::<Table>("microstudio.test_state") {
                return Ok(existing);
            }
            let state = lua.create_table()?;
            lua.set_named_registry_value("microstudio.test_state", state.clone())?;
            Ok(state)
        })?,
    )?;

    // test support: same path as REPL :player and TS addMockPlayer
    let player_ctx = ctx.clone();
    globals.set(
        "__microstudio_add_player",
        lua.create_function(move |lua, name: Option<String>| {
            let name = name.unwrap_or_else(|| {
                let state = player_ctx.state.borrow();
                format!("Player{}", state.world.players.players().len() + 1)
            });
            let (player, events) = {
                let mut state = player_ctx.state.borrow_mut();
                state
                    .world
                    .add_mock_player(
                        &name,
                        microstudio_services::PlayerOptions {
                            with_character: false,
                            user_id: None,
                        },
                    )
                    .map_err(|error| LuaError::runtime(error.to_string()))?
            };
            vm::dispatch(lua, &player_ctx, events)
                .map_err(|error| LuaError::runtime(error.to_string()))?;
            vm::instance_value(lua, &player_ctx, player)
        })?,
    )?;

    let spawn_ctx = ctx.clone();
    globals.set(
        "__microstudio_task_run",
        lua.create_function(
            move |lua, (kind, function, args): (String, Function, Table)| {
                let args = packed_args(&args)?;
                let thread = lua.create_thread(function)?;
                let owner = spawn_ctx.current_script();
                let entry = Entry {
                    thread: thread.clone(),
                    owner,
                    resume: ResumeKind::None,
                };
                match kind.as_str() {
                    "defer" => {
                        let entry = Entry {
                            thread: thread.clone(),
                            owner,
                            resume: ResumeKind::Values(args),
                        };
                        spawn_ctx.state.borrow_mut().scheduler.push_deferred(entry);
                    }
                    _ => {
                        // task.spawn runs immediately, like roblox
                        vm::resume_thread(lua, &spawn_ctx, entry, args)?;
                    }
                }
                Ok(thread)
            },
        )?,
    )?;

    let delay_ctx = ctx.clone();
    globals.set(
        "__microstudio_task_delay",
        lua.create_function(
            move |lua, (seconds, function, args): (f64, Function, Table)| {
                let args = packed_args(&args)?;
                let thread = lua.create_thread(function)?;
                let owner = delay_ctx.current_script();
                let entry = Entry {
                    thread: thread.clone(),
                    owner,
                    resume: ResumeKind::Values(args),
                };
                delay_ctx
                    .state
                    .borrow_mut()
                    .scheduler
                    .push_delay(seconds, entry);
                let _ = lua;
                Ok(thread)
            },
        )?,
    )?;

    let cancel_ctx = ctx.clone();
    globals.set(
        "__microstudio_task_cancel",
        lua.create_function(move |_, thread: Thread| {
            cancel_ctx
                .state
                .borrow_mut()
                .scheduler
                .cancel(thread.state() as usize);
            Ok(())
        })?,
    )?;

    let require_ctx = ctx.clone();
    globals.set(
        "__microstudio_require",
        lua.create_function(move |lua, target: Value| {
            if let Value::Integer(asset_id) = target {
                return Err(LuaError::runtime(format!(
                    "MicroStudio cannot require by asset id ({asset_id})"
                )));
            }
            let id = crate::convert::take_instance(&target)?;
            require_module(lua, &require_ctx, id)
        })?,
    )?;

    Ok(())
}

// caches the result per module instance like roblox
fn require_module(lua: &Lua, ctx: &Ctx, id: InstanceId) -> LuaResult<Value> {
    if let Some(cached) = ctx.state.borrow().modules.get(&id) {
        return Ok(cached.clone());
    }

    let (source, name) = {
        let state = ctx.state.borrow();
        if !state.world.dm.is_a(id, "ModuleScript") {
            return Err(LuaError::runtime(format!(
                "require expects a ModuleScript, got {}",
                state.world.dm.class_of(id).unwrap_or("<destroyed>")
            )));
        }
        let source = state
            .world
            .dm
            .get_property(id, "Source")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        (source, state.world.dm.path(id, false))
    };

    if source.trim().is_empty() {
        ctx.state.borrow_mut().modules.insert(id, Value::Nil);
        return Ok(Value::Nil);
    }

    let function = lua
        .load(&source)
        .set_name(format!("@{name}"))
        .into_function()?;
    let thread = lua.create_thread(function)?;
    let mut entry = Entry {
        thread,
        owner: Some(id),
        resume: ResumeKind::None,
    };

    match vm::resume_once(lua, ctx, &mut entry, MultiValue::new())? {
        ResumeOutcome::Finished(value) => {
            ctx.state.borrow_mut().modules.insert(id, value.clone());
            Ok(value)
        }
        ResumeOutcome::Errored => Err(LuaError::runtime(format!("{name} failed to load"))),
        ResumeOutcome::Yielded(_) => Err(LuaError::runtime(format!(
            "{name} yielded while being required, which MicroStudio does not support"
        ))),
    }
}
