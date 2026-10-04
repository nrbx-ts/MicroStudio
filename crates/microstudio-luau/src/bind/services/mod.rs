// registers into the shared Instance method table; each checks its receiver class

use mlua::{AnyUserData, Error as LuaError, Lua, Result as LuaResult, Table, Value};
use microstudio_services::players::{get_player_by_user_id, get_player_from_character};
use microstudio_services::PlayerOptions;

use crate::bind::instance::{instance_of, LuaInstance};
use crate::convert::lua_err;
use crate::vm::{self, Ctx};

pub mod assets;
pub mod data;
pub mod http;
pub mod social;
pub mod world;

// every service registers here, so a real method always beats the generated fake
pub(crate) fn methods(lua: &Lua) -> LuaResult<Table> {
    lua.named_registry_value("microstudio.instance_methods")
}

pub(crate) fn require_class(this: &LuaInstance, expected: &str) -> LuaResult<()> {
    let class = this
        .ctx
        .state
        .borrow()
        .world
        .dm
        .class_of(this.id)
        .unwrap_or("<destroyed>")
        .to_string();
    if class == expected {
        Ok(())
    } else {
        Err(LuaError::runtime(format!(
            "this method is only valid on {expected}, not {class}"
        )))
    }
}

pub fn install(lua: &Lua, ctx: &Ctx) -> LuaResult<()> {
    let methods: Table = lua.named_registry_value("microstudio.instance_methods")?;

    methods.set(
        "GetPlayers",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "Players")?;
            let ids = {
                let state = this.ctx.state.borrow();
                state.world.players.players().to_vec()
            };
            let table = lua.create_table()?;
            for (index, id) in ids.into_iter().enumerate() {
                table.set(index + 1, vm::instance_value(lua, &this.ctx, id)?)?;
            }
            Ok(table)
        })?,
    )?;

    methods.set(
        "GetPlayerByUserId",
        lua.create_function(|lua, (ud, user_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "Players")?;
            let found = {
                let state = this.ctx.state.borrow();
                get_player_by_user_id(&state.world.dm, &state.world.players, user_id as i64)
            };
            match found {
                Some(id) => vm::instance_value(lua, &this.ctx, id),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    methods.set(
        "GetPlayerFromCharacter",
        lua.create_function(|lua, (ud, character): (AnyUserData, Value)| {
            let this = instance_of(&ud)?;
            require_class(&this, "Players")?;
            let character = crate::convert::take_instance(&character)?;
            let found = {
                let state = this.ctx.state.borrow();
                get_player_from_character(&state.world.dm, &state.world.players, character)
            };
            match found {
                Some(id) => vm::instance_value(lua, &this.ctx, id),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    methods.set(
        "AddMockPlayer",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "Players")?;
            add_mock_player(lua, &this.ctx, &name, PlayerOptions::default())
        })?,
    )?;

    methods.set(
        "CreateMockPlayer",
        lua.create_function(
            |lua, (ud, name, with_character): (AnyUserData, String, Option<bool>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "Players")?;
                add_mock_player(
                    lua,
                    &this.ctx,
                    &name,
                    PlayerOptions {
                        with_character: with_character.unwrap_or(false),
                        user_id: None,
                    },
                )
            },
        )?,
    )?;

    for (name, pick) in [
        ("IsStudio", 0usize),
        ("IsServer", 1),
        ("IsClient", 2),
        ("IsRunning", 3),
        ("IsEdit", 4),
    ] {
        methods.set(
            name,
            lua.create_function(move |_, ud: AnyUserData| {
                let this = instance_of(&ud)?;
                require_class(&this, "RunService")?;
                let flags = this.ctx.state.borrow().world.run_service;
                Ok(match pick {
                    0 => flags.is_studio,
                    1 => flags.is_server,
                    2 => flags.is_client,
                    3 => flags.is_running,
                    _ => flags.is_edit,
                })
            })?,
        )?;
    }

    methods.set(
        "GetTagged",
        lua.create_function(|lua, (ud, tag): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "CollectionService")?;
            let ids = {
                let state = this.ctx.state.borrow();
                microstudio_services::collection::get_tagged(&state.world.dm, &tag)
            };
            let table = lua.create_table()?;
            for (index, id) in ids.into_iter().enumerate() {
                table.set(index + 1, vm::instance_value(lua, &this.ctx, id)?)?;
            }
            Ok(table)
        })?,
    )?;

    methods.set(
        "GetAllTags",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "CollectionService")?;
            let tags = {
                let state = this.ctx.state.borrow();
                microstudio_services::collection::get_all_tags(&state.world.dm)
            };
            let table = lua.create_table()?;
            for (index, tag) in tags.into_iter().enumerate() {
                table.set(index + 1, tag)?;
            }
            Ok(table)
        })?,
    )?;

    methods.set(
        "GetInstanceAddedSignal",
        lua.create_function(|lua, (ud, tag): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "CollectionService")?;
            let signal = this
                .ctx
                .state
                .borrow_mut()
                .world
                .collection_added_signal(&tag)
                .map_err(lua_err)?;
            Ok(Value::Table(crate::bind::signal::signal_table(
                lua,
                &this.ctx,
                &signal,
            )?))
        })?,
    )?;

    methods.set(
        "GetInstanceRemovedSignal",
        lua.create_function(|lua, (ud, tag): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "CollectionService")?;
            let signal = this
                .ctx
                .state
                .borrow_mut()
                .world
                .collection_removed_signal(&tag)
                .map_err(lua_err)?;
            Ok(Value::Table(crate::bind::signal::signal_table(
                lua,
                &this.ctx,
                &signal,
            )?))
        })?,
    )?;

    lua.set_named_registry_value("microstudio.instance_methods", methods)?;

    http::install(lua, ctx)?;
    data::install(lua, ctx)?;
    world::install(lua, ctx)?;
    social::install(lua, ctx)?;
    assets::install(lua, ctx)?;
    Ok(())
}

fn add_mock_player(lua: &Lua, ctx: &Ctx, name: &str, options: PlayerOptions) -> LuaResult<Value> {
    let (player, events) = {
        let mut state = ctx.state.borrow_mut();
        state.world.add_mock_player(name, options).map_err(lua_err)?
    };
    vm::dispatch(lua, ctx, events)?;
    vm::instance_value(lua, ctx, player)
}
