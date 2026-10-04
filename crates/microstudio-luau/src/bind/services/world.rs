// TweenService, PhysicsService, LogService, ContentProvider and TeleportService
//
// a tween has no class in the local tree (the generated table only lists services), so Create
// hands back a userdata shaped like one instead of a datamodel instance

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{
    AnyUserData, Error as LuaError, Lua, MetaMethod, Result as LuaResult, Table, UserData,
    UserDataFields, UserDataMethods, Value,
};
use microstudio_datamodel::{
    AttributeValue, DataModel, EventArg, InstanceId, PendingEvent, PropertyType, Signal, SignalId,
};
use microstudio_services::LogLevel;

use crate::bind::datatype::{enum_item, enum_name, EasingDirection, EasingStyle, LuaTweenInfo};
use crate::bind::instance::{instance_of, LuaInstance};
use crate::bind::services::{methods, require_class};
use crate::bind::signal::signal_table;
use crate::convert::{lua_err, lua_to_attribute, take_instance};
use crate::scheduler::{Entry, ResumeKind};
use crate::vm::Ctx;

const DEFAULT_GROUP: &str = "Default";
const MAX_COLLISION_GROUPS: i64 = 32;
// the property PhysicsService keeps collision groups on
const COLLISION_GROUP_PROPERTY: &str = "CollisionGroup";
// the string properties PreloadAsync collects asset ids from
const ASSET_PROPERTIES: [&str; 6] = [
    "Image",
    "Texture",
    "SoundId",
    "MeshId",
    "TextureId",
    "AnimationId",
];
const TELEPORT_MESSAGE: &str = "MicroStudio does not load a second place";

pub fn install(lua: &Lua, _ctx: &Ctx) -> LuaResult<()> {
    // service state with no instance to live on; the registry survives a world reset
    for key in [
        "microstudio.world_warned",
        "microstudio.physics",
        "microstudio.content",
        "microstudio.teleport",
    ] {
        lua.set_named_registry_value(key, lua.create_table()?)?;
    }

    // the default collision group exists from the first script, like roblox
    {
        let physics: Table = lua.named_registry_value("microstudio.physics")?;
        let groups = lua.create_table()?;
        groups.set(1, DEFAULT_GROUP)?;
        physics.set("groups", groups)?;
        physics.set("collidable", lua.create_table()?)?;
    }
    {
        let content: Table = lua.named_registry_value("microstudio.content")?;
        content.set("signals", lua.create_table()?)?;
        content.set("status", lua.create_table()?)?;
    }
    {
        let teleport: Table = lua.named_registry_value("microstudio.teleport")?;
        teleport.set("requests", lua.create_table()?)?;
        teleport.set("settings", lua.create_table()?)?;
    }

    let methods = methods(lua)?;
    install_tween(lua, &methods)?;
    install_physics(lua, &methods)?;
    install_content(lua, &methods)?;
    install_log(lua, &methods)?;
    install_teleport(lua, &methods)?;
    Ok(())
}

// one note per situation: a loop calling the same path must not flood the log
fn warn_once(lua: &Lua, ctx: &Ctx, key: &str, message: impl Into<String>) -> LuaResult<()> {
    let noted: Table = lua.named_registry_value("microstudio.world_warned")?;
    if noted.get::<Option<bool>>(key)?.is_some() {
        return Ok(());
    }
    noted.set(key, true)?;
    ctx.state.borrow_mut().scheduler.warn(message);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// TweenService

struct TweenState {
    target: InstanceId,
    // name, the value the tween lands on, and the value a reversed pass goes back to
    properties: Vec<(String, AttributeValue, Option<AttributeValue>)>,
    time: f64,
    delay: f64,
    repeats: i64,
    repeats_left: i64,
    reverses: bool,
    forward: bool,
    playback: &'static str,
    // a tween cancelled before it ever played is left alone, so remember whether it ran
    started: bool,
    timer: Option<usize>,
    completed: Signal,
}

struct LuaTween {
    ctx: Ctx,
    state: Rc<RefCell<TweenState>>,
}

impl UserData for LuaTween {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("PlaybackState", |_, this| {
            Ok(this.state.borrow().playback.to_string())
        });
        fields.add_field_method_get("Completed", |lua, this| {
            let signal = this.state.borrow().completed.clone();
            Ok(Value::Table(signal_table(lua, &this.ctx, &signal)?))
        });
        // typeof() reports Instance for a tween, the way roblox does
        fields.add_meta_field(MetaMethod::Type, "Instance");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Play", |lua, this, ()| {
            // play always starts from the beginning: nothing is interpolated, so there is no
            // remaining time to keep from an earlier run
            let (time, delay) = {
                let mut state = this.state.borrow_mut();
                if matches!(state.playback, "Completed" | "Cancelled") {
                    return Ok(());
                }
                if let Some(timer) = state.timer.take() {
                    this.ctx.state.borrow_mut().scheduler.cancel(timer);
                }
                state.playback = "Playing";
                state.forward = true;
                state.repeats_left = state.repeats;
                state.started = true;
                (state.time, state.delay)
            };
            // a zero length tween with no delay lands at once, anything else waits on the clock
            if time <= 0.0 && delay <= 0.0 {
                run_pass(lua, &this.ctx, &this.state)
            } else {
                schedule_completion(lua, &this.ctx, this.state.clone(), delay + time)
            }
        });
        methods.add_method("Pause", |_, this, ()| {
            let mut state = this.state.borrow_mut();
            if state.playback != "Playing" {
                return Ok(());
            }
            if let Some(timer) = state.timer.take() {
                this.ctx.state.borrow_mut().scheduler.cancel(timer);
            }
            state.playback = "Paused";
            Ok(())
        });
        methods.add_method("Cancel", |lua, this, ()| {
            let (signal, started) = {
                let mut state = this.state.borrow_mut();
                if matches!(state.playback, "Completed" | "Cancelled") {
                    return Ok(());
                }
                if let Some(timer) = state.timer.take() {
                    this.ctx.state.borrow_mut().scheduler.cancel(timer);
                }
                let started = state.started;
                state.playback = "Cancelled";
                (state.completed.clone(), started)
            };
            if !started {
                // cancelling a tween that never played changes nothing but its state
                return Ok(());
            }
            // roblox delivers Completed for a cancelled tween that had begun
            notify_completed(lua, &this.ctx, &signal, "Cancelled")
        });
        methods.add_meta_method(MetaMethod::ToString, |_, _, ()| Ok("Tween"));
    }
}

fn install_tween(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "Create",
        lua.create_function(
            |lua, (ud, target, info, properties): (AnyUserData, AnyUserData, LuaTweenInfo, Table)| {
                let this = instance_of(&ud)?;
                require_class(&this, "TweenService")?;
                let instance = instance_of(&target)?;
                let info = info.get();
                let ctx = this.ctx.clone();

                let class = {
                    let state = ctx.state.borrow();
                    state
                        .world
                        .dm
                        .class_of(instance.id)
                        .unwrap_or("<destroyed>")
                        .to_string()
                };

                let mut entries = Vec::new();
                for pair in properties.pairs::<String, Value>() {
                    entries.push(pair?);
                }

                let mut planned = Vec::new();
                for (name, value) in entries {
                    let Some(kind) = microstudio_datamodel::property_type(&class, &name) else {
                        return Err(LuaError::runtime(format!(
                            "TweenService:Create: {class} has no property named {name}"
                        )));
                    };
                    let goal = lua_to_attribute(lua, &value).map_err(|error| {
                        LuaError::runtime(format!("TweenService:Create: {name}: {error}"))
                    })?;
                    // the current value is where a reversed pass walks back to
                    let current = {
                        let state = ctx.state.borrow();
                        state.world.dm.get_property(instance.id, &name)
                    };
                    planned.push((name, goal, current.or_else(|| zero_value(kind))));
                }

                // detached signal: a tween is not part of the tree
                let completed = ctx.state.borrow_mut().world.dm.new_signal("Completed");

                if info.repeat_count < 0 {
                    warn_once(
                        lua,
                        &ctx,
                        "tween.endless",
                        "TweenService:Create: an endless tween (RepeatCount < 0) plays once \
                         locally",
                    )?;
                }
                warn_once(
                    lua,
                    &ctx,
                    "tween.interpolation",
                    "TweenService:Create: tweened properties jump to their goal when the tween \
                     finishes; MicroStudio does not interpolate every frame",
                )?;

                let time = info.time.max(0.0);
                let delay = info.delay_time.max(0.0);
                let repeats = info.repeat_count.max(0);
                let state = Rc::new(RefCell::new(TweenState {
                    target: instance.id,
                    properties: planned,
                    time,
                    delay,
                    repeats,
                    repeats_left: repeats,
                    reverses: info.reverses,
                    forward: true,
                    // roblox reports Begin here; the local states fold that into Paused
                    playback: "Paused",
                    started: false,
                    timer: None,
                    completed,
                }));

                // Create only builds the tween: nothing is written and no timer starts until Play
                lua.create_userdata(LuaTween { ctx, state })
            },
        )?,
    )?;

    methods.set(
        "GetValue",
        lua.create_function(
            |_, (ud, alpha, style, direction): (AnyUserData, f64, Value, Value)| {
                let this = instance_of(&ud)?;
                require_class(&this, "TweenService")?;
                let style = easing_style(&style)?;
                let direction = easing_direction(&direction)?;
                Ok(ease(style, direction, alpha))
            },
        )?,
    )?;
    Ok(())
}

// one pass of a tween: land the current side, then repeat or finish
fn run_pass(lua: &Lua, ctx: &Ctx, state: &Rc<RefCell<TweenState>>) -> LuaResult<()> {
    let (target, writes, repeats_left, time, signal) = {
        let current = state.borrow();
        if current.playback != "Playing" {
            return Ok(());
        }
        let mut writes = Vec::new();
        for (name, goal, previous) in &current.properties {
            let value = if current.forward {
                Some(goal.clone())
            } else {
                previous.clone()
            };
            if let Some(value) = value {
                writes.push((name.clone(), value));
            }
        }
        (
            current.target,
            writes,
            current.repeats_left,
            current.time,
            current.completed.clone(),
        )
    };

    {
        let mut runtime = ctx.state.borrow_mut();
        for (name, value) in writes {
            // a destroyed target is not worth raising over: the write simply does not land
            let _ = runtime.world.dm.set_property(target, &name, value);
        }
    }

    let repeat = repeats_left > 0;
    {
        let mut current = state.borrow_mut();
        current.timer = None;
        if repeat {
            current.repeats_left -= 1;
            if current.reverses {
                current.forward = !current.forward;
            }
        } else {
            current.playback = "Completed";
        }
    }

    if repeat {
        schedule_completion(lua, ctx, state.clone(), time)
    } else {
        notify_completed(lua, ctx, &signal, "Completed")
    }
}

// the timer lives on the scheduler, so completion follows the virtual clock
fn schedule_completion(
    lua: &Lua,
    ctx: &Ctx,
    state: Rc<RefCell<TweenState>>,
    delay: f64,
) -> LuaResult<()> {
    let inner_ctx = ctx.clone();
    let inner_state = state.clone();
    let function = lua.create_function(move |lua, ()| run_pass(lua, &inner_ctx, &inner_state))?;
    let thread = lua.create_thread(function)?;
    state.borrow_mut().timer = Some(thread.state() as usize);
    // read the owner before the borrow: current_script borrows the same state
    let owner = ctx.current_script();
    ctx.state.borrow_mut().scheduler.push_delay(
        delay,
        Entry {
            thread,
            owner,
            resume: ResumeKind::None,
        },
    );
    Ok(())
}

// handed to the scheduler rather than fired inline, so a listener connected straight after
// Create still hears a zero length tween finish
fn notify_completed(
    lua: &Lua,
    ctx: &Ctx,
    signal: &Signal,
    playback: &'static str,
) -> LuaResult<()> {
    let inner_ctx = ctx.clone();
    let signal = signal.clone();
    let function = lua.create_function(move |lua, ()| {
        let event = PendingEvent {
            signal: signal.clone(),
            // an enum argument reads back as its name locally, like an enum property value
            args: vec![EventArg::Text(playback.to_string())],
        };
        crate::vm::dispatch(lua, &inner_ctx, vec![event])?;
        Ok(())
    })?;
    let thread = lua.create_thread(function)?;
    let owner = ctx.current_script();
    ctx.state.borrow_mut().scheduler.push_ready(Entry {
        thread,
        owner,
        resume: ResumeKind::None,
    });
    Ok(())
}

// what a property reads as before anything sets it, so a reversed tween has somewhere to go
fn zero_value(kind: PropertyType) -> Option<AttributeValue> {
    match kind {
        PropertyType::Bool => Some(AttributeValue::Bool(false)),
        PropertyType::Number => Some(AttributeValue::Number(0.0)),
        _ => None,
    }
}

fn easing_style(value: &Value) -> LuaResult<EasingStyle> {
    let found = match value {
        Value::Integer(number) => EasingStyle::from_number(*number as f64),
        Value::Number(number) => EasingStyle::from_number(*number),
        other => EasingStyle::from_name(&enum_name(other)?),
    };
    found.ok_or_else(|| LuaError::runtime("GetValue expects an Enum.EasingStyle"))
}

fn easing_direction(value: &Value) -> LuaResult<EasingDirection> {
    let found = match value {
        Value::Integer(number) => EasingDirection::from_number(*number as f64),
        Value::Number(number) => EasingDirection::from_number(*number),
        other => EasingDirection::from_name(&enum_name(other)?),
    };
    found.ok_or_else(|| LuaError::runtime("GetValue expects an Enum.EasingDirection"))
}

// the standard easing curves; the direction folds the shape around
fn ease(style: EasingStyle, direction: EasingDirection, alpha: f64) -> f64 {
    let a = alpha.clamp(0.0, 1.0);
    let pi = std::f64::consts::PI;
    match style {
        EasingStyle::Linear => a,
        EasingStyle::Sine => match direction {
            EasingDirection::In => 1.0 - (a * pi / 2.0).cos(),
            EasingDirection::Out => (a * pi / 2.0).sin(),
            EasingDirection::InOut => -((pi * a).cos() - 1.0) / 2.0,
        },
        EasingStyle::Quad => power(a, direction, 2.0),
        EasingStyle::Cubic => power(a, direction, 3.0),
        EasingStyle::Quart => power(a, direction, 4.0),
        EasingStyle::Quint => power(a, direction, 5.0),
        EasingStyle::Exponential => match direction {
            EasingDirection::In => {
                if a == 0.0 {
                    0.0
                } else {
                    2.0_f64.powf(10.0 * a - 10.0)
                }
            }
            EasingDirection::Out => {
                if a == 1.0 {
                    1.0
                } else {
                    1.0 - 2.0_f64.powf(-10.0 * a)
                }
            }
            EasingDirection::InOut => {
                if a == 0.0 {
                    0.0
                } else if a == 1.0 {
                    1.0
                } else if a < 0.5 {
                    2.0_f64.powf(20.0 * a - 10.0) / 2.0
                } else {
                    (2.0 - 2.0_f64.powf(-20.0 * a + 10.0)) / 2.0
                }
            }
        },
        EasingStyle::Circ => match direction {
            EasingDirection::In => 1.0 - (1.0 - a * a).sqrt(),
            EasingDirection::Out => (1.0 - (a - 1.0) * (a - 1.0)).sqrt(),
            EasingDirection::InOut => {
                if a < 0.5 {
                    (1.0 - (1.0 - (2.0 * a) * (2.0 * a)).sqrt()) / 2.0
                } else {
                    ((1.0 - (-2.0 * a + 2.0) * (-2.0 * a + 2.0)).sqrt() + 1.0) / 2.0
                }
            }
        },
        EasingStyle::Back => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            match direction {
                EasingDirection::In => c3 * a * a * a - c1 * a * a,
                EasingDirection::Out => {
                    1.0 + c3 * (a - 1.0).powi(3) + c1 * (a - 1.0).powi(2)
                }
                EasingDirection::InOut => {
                    let c2 = c1 * 1.525;
                    if a < 0.5 {
                        ((2.0 * a).powi(2) * ((c2 + 1.0) * 2.0 * a - c2)) / 2.0
                    } else {
                        ((2.0 * a - 2.0).powi(2) * ((c2 + 1.0) * (2.0 * a - 2.0) + c2) + 2.0) / 2.0
                    }
                }
            }
        }
        EasingStyle::Bounce => match direction {
            EasingDirection::In => 1.0 - bounce_out(1.0 - a),
            EasingDirection::Out => bounce_out(a),
            EasingDirection::InOut => {
                if a < 0.5 {
                    (1.0 - bounce_out(1.0 - 2.0 * a)) / 2.0
                } else {
                    (1.0 + bounce_out(2.0 * a - 1.0)) / 2.0
                }
            }
        },
        EasingStyle::Elastic => {
            let c4 = 2.0 * pi / 3.0;
            match direction {
                EasingDirection::In => {
                    if a == 0.0 {
                        0.0
                    } else if a == 1.0 {
                        1.0
                    } else {
                        -2.0_f64.powf(10.0 * a - 10.0) * ((a * 10.0 - 10.75) * c4).sin()
                    }
                }
                EasingDirection::Out => {
                    if a == 0.0 {
                        0.0
                    } else if a == 1.0 {
                        1.0
                    } else {
                        2.0_f64.powf(-10.0 * a) * ((a * 10.0 - 0.75) * c4).sin() + 1.0
                    }
                }
                EasingDirection::InOut => {
                    if a == 0.0 {
                        0.0
                    } else if a == 1.0 {
                        1.0
                    } else {
                        let c5 = 2.0 * pi / 4.5;
                        if a < 0.5 {
                            -(2.0_f64.powf(20.0 * a - 10.0) * ((20.0 * a - 11.125) * c5).sin())
                                / 2.0
                        } else {
                            (2.0_f64.powf(-20.0 * a + 10.0) * ((20.0 * a - 11.125) * c5).sin())
                                / 2.0
                                + 1.0
                        }
                    }
                }
            }
        }
    }
}

fn power(a: f64, direction: EasingDirection, exponent: f64) -> f64 {
    match direction {
        EasingDirection::In => a.powf(exponent),
        EasingDirection::Out => 1.0 - (1.0 - a).powf(exponent),
        EasingDirection::InOut => {
            if a < 0.5 {
                (2.0 * a).powf(exponent) / 2.0
            } else {
                1.0 - (-2.0 * a + 2.0).powf(exponent) / 2.0
            }
        }
    }
}

fn bounce_out(a: f64) -> f64 {
    let n1 = 7.5625;
    let d1 = 2.75;
    if a < 1.0 / d1 {
        n1 * a * a
    } else if a < 2.0 / d1 {
        let a = a - 1.5 / d1;
        n1 * a * a + 0.75
    } else if a < 2.5 / d1 {
        let a = a - 2.25 / d1;
        n1 * a * a + 0.9375
    } else {
        let a = a - 2.625 / d1;
        n1 * a * a + 0.984375
    }
}

// ---------------------------------------------------------------------------------------------
// PhysicsService

fn physics_state(lua: &Lua) -> LuaResult<Table> {
    lua.named_registry_value("microstudio.physics")
}

fn group_names(state: &Table) -> LuaResult<Vec<String>> {
    let groups: Table = state.get("groups")?;
    let mut names = Vec::new();
    for index in 1..=groups.raw_len() {
        names.push(groups.get::<String>(index)?);
    }
    Ok(names)
}

fn write_group_names(lua: &Lua, state: &Table, names: &[String]) -> LuaResult<()> {
    let groups = lua.create_table()?;
    for (index, name) in names.iter().enumerate() {
        groups.set(index + 1, name.as_str())?;
    }
    state.set("groups", groups)
}

fn group_exists(state: &Table, name: &str) -> LuaResult<bool> {
    Ok(group_names(state)?.iter().any(|group| group == name))
}

fn ensure_group(state: &Table, name: &str) -> LuaResult<()> {
    if group_exists(state, name)? {
        Ok(())
    } else {
        Err(LuaError::runtime("collision group does not exist"))
    }
}

// an unordered pair key, so both directions of a lookup hit the same entry
fn pair_key(first: &str, second: &str) -> String {
    if first <= second {
        format!("{first}\u{0}{second}")
    } else {
        format!("{second}\u{0}{first}")
    }
}

fn split_pair(key: &str) -> (&str, &str) {
    match key.split_once('\u{0}') {
        Some((first, second)) => (first, second),
        None => (key, ""),
    }
}

// a part with nothing set, or an empty name, belongs to the default group, like roblox
fn part_group(dm: &DataModel, id: InstanceId) -> String {
    match dm.get_property(id, COLLISION_GROUP_PROPERTY) {
        Some(AttributeValue::String(name)) if !name.is_empty() => name,
        _ => DEFAULT_GROUP.to_string(),
    }
}

fn set_part_group(ctx: &Ctx, part: InstanceId, name: &str) -> LuaResult<()> {
    ctx.state
        .borrow_mut()
        .world
        .dm
        .set_property(
            part,
            COLLISION_GROUP_PROPERTY,
            AttributeValue::String(name.to_string()),
        )
        .map_err(lua_err)
}

// every part that used `from` moves to `to`; only renaming and unregistering call this
fn reassign_parts(ctx: &Ctx, from: &str, to: &str) {
    let mut runtime = ctx.state.borrow_mut();
    let game = runtime.world.dm.game();
    let ids = runtime.world.dm.get_descendants(game);
    for id in ids {
        if part_group(&runtime.world.dm, id) != from {
            continue;
        }
        // a part that has gone is not worth raising over: the write simply does not land
        let _ = runtime.world.dm.set_property(
            id,
            COLLISION_GROUP_PROPERTY,
            AttributeValue::String(to.to_string()),
        );
    }
}

fn rename_parts(ctx: &Ctx, state: &Table, from: &str, to: &str) -> LuaResult<()> {
    let collidable: Table = state.get("collidable")?;
    let mut moved = Vec::new();
    for pair in collidable.pairs::<String, bool>() {
        let (key, value) = pair?;
        let (first, second) = split_pair(&key);
        if first == from || second == from {
            moved.push((key, value));
        }
    }
    for (key, value) in moved {
        let (first, second) = split_pair(&key);
        let first = if first == from { to } else { first };
        let second = if second == from { to } else { second };
        collidable.set(key.as_str(), Value::Nil)?;
        collidable.set(pair_key(first, second).as_str(), value)?;
    }
    reassign_parts(ctx, from, to);
    Ok(())
}

fn install_physics(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "RegisterCollisionGroup",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            if group_exists(&state, &name)? {
                return Err(LuaError::runtime("collision group already exists"));
            }
            let mut names = group_names(&state)?;
            names.push(name);
            write_group_names(lua, &state, &names)
        })?,
    )?;

    methods.set(
        "UnregisterCollisionGroup",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            ensure_group(&state, &name)?;
            let mut names = group_names(&state)?;
            names.retain(|group| group != &name);
            write_group_names(lua, &state, &names)?;
            rename_parts(&this.ctx, &state, &name, DEFAULT_GROUP)
        })?,
    )?;

    methods.set(
        "IsCollisionGroupRegistered",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            group_exists(&state, &name)
        })?,
    )?;

    for name in ["GetRegisteredCollisionGroups", "GetCollisionGroups"] {
        methods.set(
            name,
            lua.create_function(|lua, ud: AnyUserData| {
                let this = instance_of(&ud)?;
                require_class(&this, "PhysicsService")?;
                let state = physics_state(lua)?;
                let out = lua.create_table()?;
                for (index, group) in group_names(&state)?.iter().enumerate() {
                    out.set(index + 1, group.as_str())?;
                }
                Ok(out)
            })?,
        )?;
    }

    methods.set(
        "RenameCollisionGroup",
        lua.create_function(|lua, (ud, from, to): (AnyUserData, String, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            ensure_group(&state, &from)?;
            if group_exists(&state, &to)? {
                return Err(LuaError::runtime("collision group already exists"));
            }
            let names: Vec<String> = group_names(&state)?
                .into_iter()
                .map(|group| if group == from { to.clone() } else { group })
                .collect();
            write_group_names(lua, &state, &names)?;
            rename_parts(&this.ctx, &state, &from, &to)
        })?,
    )?;

    methods.set(
        "CollisionGroupSetCollidable",
        lua.create_function(
            |lua, (ud, first, second, collidable): (AnyUserData, String, String, bool)| {
                let this = instance_of(&ud)?;
                require_class(&this, "PhysicsService")?;
                let state = physics_state(lua)?;
                ensure_group(&state, &first)?;
                ensure_group(&state, &second)?;
                let pairs: Table = state.get("collidable")?;
                let key = pair_key(&first, &second);
                if collidable {
                    // collidable is the default, so only the exceptions are stored
                    pairs.set(key.as_str(), Value::Nil)?;
                } else {
                    pairs.set(key.as_str(), false)?;
                }
                Ok(())
            },
        )?,
    )?;

    methods.set(
        "CollisionGroupsAreCollidable",
        lua.create_function(|lua, (ud, first, second): (AnyUserData, String, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            ensure_group(&state, &first)?;
            ensure_group(&state, &second)?;
            let pairs: Table = state.get("collidable")?;
            Ok(pairs
                .get::<Option<bool>>(pair_key(&first, &second).as_str())?
                .unwrap_or(true))
        })?,
    )?;

    methods.set(
        "GetMaxCollisionGroups",
        lua.create_function(|_, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            Ok(MAX_COLLISION_GROUPS)
        })?,
    )?;

    methods.set(
        "CollisionGroupContainsPart",
        lua.create_function(|_, (ud, name, part): (AnyUserData, String, Value)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let part = take_instance(&part)?;
            let assigned = {
                let state = this.ctx.state.borrow();
                part_group(&state.world.dm, part)
            };
            // the group lives on BasePart.CollisionGroup, so no tree walk is needed here
            Ok(assigned == name)
        })?,
    )?;

    methods.set(
        "SetPartCollisionGroup",
        lua.create_function(|lua, (ud, part, name): (AnyUserData, Value, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let part = take_instance(&part)?;
            let state = physics_state(lua)?;
            ensure_group(&state, &name)?;
            set_part_group(&this.ctx, part, &name)
        })?,
    )?;

    // the deprecated family works off the same table, so old code keeps running
    methods.set(
        "CreateCollisionGroup",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            if group_exists(&state, &name)? {
                return Err(LuaError::runtime("collision group already exists"));
            }
            let mut names = group_names(&state)?;
            names.push(name);
            write_group_names(lua, &state, &names)?;
            Ok(names.len() as i64)
        })?,
    )?;

    methods.set(
        "GetCollisionGroupId",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            let names = group_names(&state)?;
            match names.iter().position(|group| group == &name) {
                Some(index) => Ok(index as i64 + 1),
                None => Err(LuaError::runtime("collision group does not exist")),
            }
        })?,
    )?;

    methods.set(
        "GetCollisionGroupName",
        lua.create_function(|lua, (ud, id): (AnyUserData, i64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            let names = group_names(&state)?;
            if id < 1 || id as usize > names.len() {
                return Err(LuaError::runtime("collision group does not exist"));
            }
            Ok(names[id as usize - 1].clone())
        })?,
    )?;

    methods.set(
        "RemoveCollisionGroup",
        lua.create_function(|lua, (ud, group): (AnyUserData, Value)| {
            let this = instance_of(&ud)?;
            require_class(&this, "PhysicsService")?;
            let state = physics_state(lua)?;
            let names = group_names(&state)?;
            // the old spelling took an id, the new one a name
            let found = match &group {
                Value::String(text) => Some(text.to_str()?.to_string()),
                Value::Integer(id) if *id >= 1 => names.get(*id as usize - 1).cloned(),
                Value::Integer(_) => None,
                other => {
                    return Err(LuaError::runtime(format!(
                        "RemoveCollisionGroup expects a name or an id, got {}",
                        other.type_name()
                    )))
                }
            };
            let found = found.ok_or_else(|| LuaError::runtime("collision group does not exist"))?;
            let mut remaining = names;
            remaining.retain(|name| name != &found);
            write_group_names(lua, &state, &remaining)?;
            rename_parts(&this.ctx, &state, &found, DEFAULT_GROUP)
        })?,
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------------------------
// ContentProvider

fn asset_signal(lua: &Lua, ctx: &Ctx, content: &Table, asset: &str) -> LuaResult<Signal> {
    let signals: Table = content.get("signals")?;
    if let Some(id) = signals.get::<Option<u64>>(asset)? {
        let existing = ctx.state.borrow().signals.get(&SignalId(id)).cloned();
        if let Some(signal) = existing {
            return Ok(signal);
        }
    }
    let signal = ctx.state.borrow_mut().world.dm.new_signal("AssetFetchStatusChanged");
    signal_table(lua, ctx, &signal)?;
    signals.set(asset, signal.id().0)?;
    Ok(signal)
}

// the instance and everything under it, properties and attributes alike: locally an asset id
// can only be stored as an attribute on most classes
fn collect_assets(dm: &DataModel, id: InstanceId, out: &mut Vec<String>) {
    let mut nodes = vec![id];
    nodes.extend(dm.get_descendants(id));
    for node in nodes {
        for name in ASSET_PROPERTIES {
            let found = dm
                .get_property(node, name)
                .or_else(|| dm.get_attribute(node, name));
            let Some(AttributeValue::String(text)) = found else {
                continue;
            };
            if !text.starts_with("rbxassetid://") || out.iter().any(|seen| seen == &text) {
                continue;
            }
            out.push(text);
        }
    }
}

fn install_content(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "PreloadAsync",
        lua.create_function(|lua, (ud, instances): (AnyUserData, Value)| {
            let this = instance_of(&ud)?;
            require_class(&this, "ContentProvider")?;
            let ctx = this.ctx.clone();
            warn_once(
                lua,
                &ctx,
                "content.preload",
                "ContentProvider:PreloadAsync does not download: assets are recorded and \
                 reported as fetched at once",
            )?;

            let requested = instance_list(&instances).map_err(|error| {
                LuaError::runtime(format!("ContentProvider:PreloadAsync: {error}"))
            })?;
            let mut assets = Vec::new();
            {
                let state = ctx.state.borrow();
                for id in requested {
                    collect_assets(&state.world.dm, id, &mut assets);
                }
            }

            let content: Table = lua.named_registry_value("microstudio.content")?;
            let status: Table = content.get("status")?;
            let mut events = Vec::new();
            for asset in assets {
                status.set(asset.as_str(), "Success")?;
                let signal = asset_signal(lua, &ctx, &content, &asset)?;
                events.push(PendingEvent {
                    signal,
                    args: vec![
                        EventArg::Text(asset),
                        EventArg::Text("Success".into()),
                    ],
                });
            }
            Ok(crate::vm::dispatch(lua, &ctx, events)?)
        })?,
    )?;

    methods.set(
        "GetAssetFetchStatusChangedSignal",
        lua.create_function(|lua, (ud, asset): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "ContentProvider")?;
            let content: Table = lua.named_registry_value("microstudio.content")?;
            let signal = asset_signal(lua, &this.ctx, &content, &asset)?;
            Ok(Value::Table(signal_table(lua, &this.ctx, &signal)?))
        })?,
    )?;

    methods.set(
        "GetAssetFetchStatus",
        lua.create_function(|lua, (ud, asset): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "ContentProvider")?;
            let content: Table = lua.named_registry_value("microstudio.content")?;
            let status: Table = content.get("status")?;
            let fetched = status.get::<Option<String>>(asset.as_str())?.is_some();
            enum_item(
                lua,
                "AssetFetchStatus",
                if fetched { "Success" } else { "None" },
            )
        })?,
    )?;

    for name in ["GetFailedRequests", "GetDetailedFailedRequests", "ListEncryptedAssets"] {
        methods.set(
            name,
            lua.create_function(|lua, ud: AnyUserData| {
                let this = instance_of(&ud)?;
                require_class(&this, "ContentProvider")?;
                lua.create_table()
            })?,
        )?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------------------------
// LogService

fn message_type(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Print => "MessageOutput",
        LogLevel::Info => "MessageInfo",
        LogLevel::Warn => "MessageWarning",
        LogLevel::Error => "MessageError",
    }
}

fn install_log(lua: &Lua, methods: &Table) -> LuaResult<()> {
    // MessageOut and ServerMessageOut stay generated signals: the runtime's own print path does
    // not raise instance events, so a listener here never hears log output
    methods.set(
        "GetLogHistory",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "LogService")?;
            let (logs, now, epoch) = {
                let state = this.ctx.state.borrow();
                (state.world.logs.clone(), state.scheduler.now(), state.epoch)
            };
            let out = lua.create_table()?;
            for (index, entry) in logs.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("message", entry.text.as_str())?;
                row.set("messageType", enum_item(lua, "MessageType", message_type(entry.level))?)?;
                // entries carry no time of their own, so they report the current virtual time
                row.set("timestamp", ((epoch + now) * 1000.0).floor())?;
                out.set(index + 1, row)?;
            }
            Ok(out)
        })?,
    )?;

    methods.set(
        "ClearOutput",
        lua.create_function(|_, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "LogService")?;
            this.ctx.state.borrow_mut().world.logs.clear();
            Ok(())
        })?,
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------------------------
// TeleportService

fn user_id(dm: &DataModel, id: InstanceId) -> f64 {
    dm.get_property(id, "UserId")
        .and_then(|value| value.as_number())
        .unwrap_or(0.0)
}

// an array of Instances, or one on its own
fn instance_list(value: &Value) -> LuaResult<Vec<InstanceId>> {
    match value {
        Value::Table(table) => {
            let mut ids = Vec::new();
            for entry in table.clone().sequence_values::<Value>() {
                let entry = entry?;
                if !entry.is_nil() {
                    ids.push(take_instance(&entry)?);
                }
            }
            Ok(ids)
        }
        other => Ok(vec![take_instance(other)?]),
    }
}

fn teleport_players(
    lua: &Lua,
    this: &LuaInstance,
    place_id: f64,
    players: Vec<InstanceId>,
    data: Option<Value>,
) -> LuaResult<()> {
    let ctx = &this.ctx;
    let user_ids = {
        let state = ctx.state.borrow();
        players
            .iter()
            .map(|player| user_id(&state.world.dm, *player))
            .collect::<Vec<_>>()
    };

    // record the request: a real server hands it to the teleport layer
    let registry: Table = lua.named_registry_value("microstudio.teleport")?;
    let requests: Table = registry.get("requests")?;
    let record = lua.create_table()?;
    record.set("placeId", place_id)?;
    let ids = lua.create_table()?;
    for (index, id) in user_ids.iter().enumerate() {
        ids.set(index + 1, *id)?;
    }
    record.set("userIds", ids)?;
    record.set("teleportData", data.unwrap_or(Value::Nil))?;
    requests.set(requests.raw_len() as i64 + 1, record)?;

    warn_once(
        lua,
        ctx,
        "teleport",
        format!("TeleportService: {TELEPORT_MESSAGE}; the request is recorded and \
                 TeleportInitFailed fires"),
    )?;

    // a teleport that cannot start reports through TeleportInitFailed, which is this case
    let signal = ctx
        .state
        .borrow_mut()
        .world
        .dm
        .named_signal(this.id, "TeleportInitFailed")
        .map_err(lua_err)?;
    let mut events = Vec::new();
    for player in players {
        events.push(PendingEvent {
            signal: signal.clone(),
            args: vec![
                EventArg::Instance(player),
                // an enum argument reads back as its name locally
                EventArg::Text("Failure".into()),
                EventArg::Text(TELEPORT_MESSAGE.into()),
                EventArg::Number(place_id),
            ],
        });
    }
    Ok(crate::vm::dispatch(lua, ctx, events)?)
}

fn install_teleport(lua: &Lua, methods: &Table) -> LuaResult<()> {
    methods.set(
        "Teleport",
        lua.create_function(
            |lua,
             (ud, place_id, player, data, _screen): (
                AnyUserData,
                f64,
                Value,
                Option<Value>,
                Option<Value>,
            )| {
                let this = instance_of(&ud)?;
                require_class(&this, "TeleportService")?;
                let player = take_instance(&player)?;
                teleport_players(lua, &this, place_id, vec![player], data)
            },
        )?,
    )?;

    methods.set(
        "TeleportAsync",
        lua.create_function(
            |lua, (ud, place_id, players, options): (AnyUserData, f64, Value, Option<Table>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "TeleportService")?;
                let players = instance_list(&players)?;
                let data = options.and_then(|table| {
                    table.get::<Option<Value>>("TeleportData").ok().flatten()
                });
                teleport_players(lua, &this, place_id, players, data)
            },
        )?,
    )?;

    methods.set(
        "TeleportToPlaceInstance",
        lua.create_function(
            |lua,
             (ud, place_id, _instance_id, player, data): (
                AnyUserData,
                f64,
                String,
                Value,
                Option<Value>,
            )| {
                let this = instance_of(&ud)?;
                require_class(&this, "TeleportService")?;
                let player = take_instance(&player)?;
                teleport_players(lua, &this, place_id, vec![player], data)
            },
        )?,
    )?;

    methods.set(
        "TeleportToSpawnByName",
        lua.create_function(
            |lua,
             (ud, place_id, _spawn, player, data): (
                AnyUserData,
                f64,
                String,
                Value,
                Option<Value>,
            )| {
                let this = instance_of(&ud)?;
                require_class(&this, "TeleportService")?;
                let player = take_instance(&player)?;
                teleport_players(lua, &this, place_id, vec![player], data)
            },
        )?,
    )?;

    methods.set(
        "TeleportPartyAsync",
        lua.create_function(
            |lua, (ud, place_id, players, data): (AnyUserData, f64, Value, Option<Value>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "TeleportService")?;
                let players = instance_list(&players)?;
                teleport_players(lua, &this, place_id, players, data)?;
                // a real server answers with the id of the reserved server
                Ok(format!("microstudio-place-{place_id}"))
            },
        )?,
    )?;

    methods.set(
        "GetLocalPlayerTeleportData",
        lua.create_function(|_, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "TeleportService")?;
            Ok(Value::Nil)
        })?,
    )?;

    methods.set(
        "GetTeleportSetting",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "TeleportService")?;
            let registry: Table = lua.named_registry_value("microstudio.teleport")?;
            let settings: Table = registry.get("settings")?;
            settings.get::<Value>(name.as_str())
        })?,
    )?;

    methods.set(
        "SetTeleportSetting",
        lua.create_function(|lua, (ud, name, value): (AnyUserData, String, Value)| {
            let this = instance_of(&ud)?;
            require_class(&this, "TeleportService")?;
            let registry: Table = lua.named_registry_value("microstudio.teleport")?;
            let settings: Table = registry.get("settings")?;
            settings.set(name.as_str(), value)
        })?,
    )?;

    methods.set(
        "ReserveServer",
        lua.create_function(|_, (ud, _place_id): (AnyUserData, f64)| -> LuaResult<()> {
            let this = instance_of(&ud)?;
            require_class(&this, "TeleportService")?;
            Err(LuaError::runtime(
                "TeleportService:ReserveServer is not supported locally",
            ))
        })?,
    )?;

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

    // a runtime error is reported rather than raised at the top level, so ask for it
    fn eval_error(vm: &Vm, code: &str) -> String {
        let source = format!(
            "local ok, message = pcall(function() return {code} end)\n\
             assert(not ok, 'expected an error')\n\
             return tostring(message)"
        );
        eval_string(vm, &source)
    }

    #[test]
    fn tween_create_alone_changes_nothing() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local part = Instance.new("Part")
                part.Transparency = 0
                local tween = game:GetService("TweenService"):Create(
                    part, TweenInfo.new(0), {Transparency = 1})
                local fired = false
                tween.Completed:Connect(function() fired = true end)
                assert(tween.PlaybackState == "Paused", tween.PlaybackState)
                assert(part.Transparency == 0, "create alone must change nothing")
                task.wait(0.1)
                assert(not fired, "create alone must not complete")
                assert(part.Transparency == 0, "create alone must not land")

                tween:Play()
                assert(part.Transparency == 1, "a zero length tween lands when played")
                assert(tween.PlaybackState == "Completed", tween.PlaybackState)
                task.wait(0.1)
                assert(fired, "completed must fire")
                return tween
                "#,
            )
            .unwrap();
        assert!(matches!(value, Value::UserData(_)), "a tween is a userdata");
    }

    #[test]
    fn tween_clock_runs_from_play() {
        let vm = vm();
        vm.eval(
            r#"
            local part = Instance.new("Part")
            part.Transparency = 0
            _G.__tween_part = part
            _G.__tween_fired = false
            local tween = game:GetService("TweenService"):Create(
                part, TweenInfo.new(1), {Transparency = 1})
            _G.__tween = tween
            tween.Completed:Connect(function() _G.__tween_fired = true end)
            assert(tween.PlaybackState == "Paused", tween.PlaybackState)
            assert(part.Transparency == 0, "create alone must change nothing")
            "#,
        )
        .unwrap();

        // time passing before play starts nothing
        vm.advance_time(2.0).unwrap();
        vm.eval(
            r#"
            assert(_G.__tween_part.Transparency == 0, "an unplayed tween must not land")
            assert(not _G.__tween_fired, "an unplayed tween must not complete")
            _G.__tween:Play()
            assert(_G.__tween.PlaybackState == "Playing", _G.__tween.PlaybackState)
            assert(_G.__tween_part.Transparency == 0, "play does not land the values at once")
            "#,
        )
        .unwrap();

        // the second runs from play, not from create
        vm.advance_time(0.5).unwrap();
        vm.eval(r#"assert(_G.__tween_part.Transparency == 0, "half the time is not enough")"#)
            .unwrap();
        vm.advance_time(0.5).unwrap();
        vm.eval(
            r#"
            assert(_G.__tween_part.Transparency == 1, "the value lands when the time is up")
            assert(_G.__tween_fired, "completed must fire")
            "#,
        )
        .unwrap();

        // pause cancels the timer, play starts it again, play while playing restarts it
        vm.eval(
            r#"
            local part = Instance.new("Part")
            part.Transparency = 0
            _G.__pause_part = part
            _G.__pause_tween = game:GetService("TweenService"):Create(
                part, TweenInfo.new(1), {Transparency = 1})
            _G.__pause_tween:Play()
            _G.__pause_tween:Pause()
            assert(_G.__pause_tween.PlaybackState == "Paused", _G.__pause_tween.PlaybackState)
            "#,
        )
        .unwrap();

        vm.advance_time(2.0).unwrap();
        vm.eval(r#"assert(_G.__pause_part.Transparency == 0, "a paused tween must not land")"#)
            .unwrap();

        vm.eval("_G.__pause_tween:Play()").unwrap();
        vm.advance_time(0.5).unwrap();
        vm.eval("_G.__pause_tween:Play()").unwrap();
        vm.advance_time(0.5).unwrap();
        vm.eval(
            r#"assert(_G.__pause_part.Transparency == 0, "play while playing restarts the time")"#,
        )
        .unwrap();
        vm.advance_time(0.5).unwrap();
        vm.eval(
            r#"assert(_G.__pause_part.Transparency == 1, "the restarted tween lands")"#,
        )
        .unwrap();

        // the delay runs before the time
        vm.eval(
            r#"
            local part = Instance.new("Part")
            part.Transparency = 0
            _G.__delay_part = part
            local info = TweenInfo.new(
                1, Enum.EasingStyle.Linear, Enum.EasingDirection.In, 0, false, 0.5)
            game:GetService("TweenService"):Create(part, info, {Transparency = 1}):Play()
            "#,
        )
        .unwrap();
        vm.advance_time(1.0).unwrap();
        vm.eval(r#"assert(_G.__delay_part.Transparency == 0, "the delay runs first")"#)
            .unwrap();
        vm.advance_time(0.5).unwrap();
        vm.eval(
            r#"assert(_G.__delay_part.Transparency == 1, "delay plus time is the deadline")"#,
        )
        .unwrap();
    }

    #[test]
    fn tween_cancel_before_play_only_changes_state() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("TweenService")

            local part = Instance.new("Part")
            _G.__cancel_part = part
            _G.__cancel_fired = false
            local tween = service:Create(part, TweenInfo.new(1), {Transparency = 1})
            tween.Completed:Connect(function() _G.__cancel_fired = true end)
            tween:Cancel()
            assert(tween.PlaybackState == "Cancelled", tween.PlaybackState)
            tween:Play()
            assert(tween.PlaybackState == "Cancelled", "a cancelled tween stays cancelled")

            -- a played tween that is cancelled reports through Completed and never lands
            local played_part = Instance.new("Part")
            _G.__played_part = played_part
            _G.__played_playback = nil
            local played = service:Create(played_part, TweenInfo.new(1), {Transparency = 1})
            played.Completed:Connect(function(state) _G.__played_playback = state end)
            played:Play()
            played:Cancel()
            assert(played.PlaybackState == "Cancelled", played.PlaybackState)
            "#,
        )
        .unwrap();

        vm.advance_time(1.0).unwrap();
        vm.eval(
            r#"
            assert(_G.__cancel_part.Transparency == 0, "a cancelled tween must not land")
            assert(not _G.__cancel_fired, "cancel before play must not fire completed")
            assert(_G.__played_part.Transparency == 0, "a cancelled tween must not land")
            assert(_G.__played_playback == "Cancelled", tostring(_G.__played_playback))
            "#,
        )
        .unwrap();
    }

    #[test]
    fn tween_create_rejects_a_property_the_class_lacks() {
        let vm = vm();
        let message = eval_error(
            &vm,
            r#"game:GetService("TweenService"):Create(
                Instance.new("Part"), TweenInfo.new(0), {NotAProperty = 1})"#,
        );
        assert!(message.contains("no property named NotAProperty"), "{message}");
    }

    #[test]
    fn get_value_eases_each_direction_differently() {
        let vm = vm();
        vm.eval(
            r#"
            local style, direction = Enum.EasingStyle.Sine, Enum.EasingDirection.InOut
            local info = TweenInfo.new(2, style, direction, 3, true, 0.5)
            assert(info.Time == 2, info.Time)
            assert(info.EasingStyle == Enum.EasingStyle.Sine)
            assert(info.EasingDirection == Enum.EasingDirection.InOut)
            assert(info.RepeatCount == 3)
            assert(info.Reverses == true)
            assert(info.DelayTime == 0.5)
            local rendered = "2, Enum.EasingStyle.Sine, Enum.EasingDirection.InOut, 3, true, 0.5"
            assert(tostring(info) == rendered, tostring(info))
            assert(TweenInfo.new(1) == TweenInfo.new(1))
            assert(typeof(info) == "TweenInfo")
            "#,
        )
        .unwrap();

        let value = vm
            .eval(
                r#"
                local service = game:GetService("TweenService")
                return {
                    service:GetValue(0.25, Enum.EasingStyle.Quad, Enum.EasingDirection.In),
                    service:GetValue(0.25, Enum.EasingStyle.Quad, Enum.EasingDirection.Out),
                    service:GetValue(0.5, Enum.EasingStyle.Linear, Enum.EasingDirection.InOut),
                }
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert!((table.get::<f64>(1).unwrap() - 0.0625).abs() < 1e-9);
        assert!((table.get::<f64>(2).unwrap() - 0.4375).abs() < 1e-9);
        assert!((table.get::<f64>(3).unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn easing_curves_reach_both_ends() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("TweenService")
            local styles = {"Linear", "Sine", "Back", "Quad", "Quart", "Quint",
                "Bounce", "Elastic", "Exponential", "Cubic", "Circ"}
            local directions = {"In", "Out", "InOut"}
            for _, style in styles do
                for _, direction in directions do
                    local start = service:GetValue(0, style, direction)
                    local finish = service:GetValue(1, style, direction)
                    local where = style .. "/" .. direction
                    assert(math.abs(start) < 1e-9, where .. " starts at " .. start)
                    assert(math.abs(finish - 1) < 1e-9, where .. " ends at " .. finish)
                end
            end
            "#,
        )
        .unwrap();
    }

    #[test]
    fn collision_groups_register_rename_and_remove() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("PhysicsService")
            assert(service:IsCollisionGroupRegistered("Default"))
            assert(service:GetMaxCollisionGroups() == 32)
            assert(#service:GetRegisteredCollisionGroups() == 1)

            service:RegisterCollisionGroup("Team")
            assert(service:IsCollisionGroupRegistered("Team"))
            assert(#service:GetRegisteredCollisionGroups() == 2)

            local part = Instance.new("Part")
            part.Parent = workspace
            service:SetPartCollisionGroup(part, "Team")

            service:RenameCollisionGroup("Team", "Squad")
            assert(service:IsCollisionGroupRegistered("Squad"))
            assert(not service:IsCollisionGroupRegistered("Team"))
            assert(part.CollisionGroup == "Squad", part.CollisionGroup)

            service:UnregisterCollisionGroup("Squad")
            assert(not service:IsCollisionGroupRegistered("Squad"))
            assert(#service:GetRegisteredCollisionGroups() == 1)
            assert(part.CollisionGroup == "Default", part.CollisionGroup)

            local ok, message = pcall(function()
                return service:UnregisterCollisionGroup("Squad")
            end)
            assert(not ok, "a missing group must raise")
            assert(string.find(message, "collision group does not exist", 1, true), message)
            local again, other = pcall(function()
                return service:RegisterCollisionGroup("Default")
            end)
            assert(not again, "a duplicate group must raise")
            assert(string.find(other, "collision group already exists", 1, true), other)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn collision_group_pairs_start_collidable() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("PhysicsService")
            service:RegisterCollisionGroup("Ghost")
            assert(service:CollisionGroupsAreCollidable("Ghost", "Default"))
            service:CollisionGroupSetCollidable("Ghost", "Default", false)
            assert(not service:CollisionGroupsAreCollidable("Ghost", "Default"))
            assert(not service:CollisionGroupsAreCollidable("Default", "Ghost"))
            service:CollisionGroupSetCollidable("Ghost", "Default", true)
            assert(service:CollisionGroupsAreCollidable("Ghost", "Default"))

            -- a rename keeps the pair with the group
            service:CollisionGroupSetCollidable("Ghost", "Default", false)
            service:RenameCollisionGroup("Ghost", "Phantom")
            assert(not service:CollisionGroupsAreCollidable("Phantom", "Default"))
            "#,
        )
        .unwrap();
    }

    #[test]
    fn parts_carry_their_collision_group_and_old_names_work() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("PhysicsService")
            local part = Instance.new("Part")
            part.Parent = workspace

            local id = service:CreateCollisionGroup("Blue")
            assert(type(id) == "number")
            assert(service:GetCollisionGroupId("Blue") == id)
            assert(service:GetCollisionGroupName(id) == "Blue")
            assert(#service:GetCollisionGroups() == 2)

            -- the ordinary property path and the service agree
            part.CollisionGroup = "Blue"
            assert(part.CollisionGroup == "Blue", part.CollisionGroup)
            assert(service:CollisionGroupContainsPart("Blue", part))
            assert(not service:CollisionGroupContainsPart("Default", part))

            service:SetPartCollisionGroup(part, "Default")
            assert(part.CollisionGroup == "Default", part.CollisionGroup)
            service:SetPartCollisionGroup(part, "Blue")

            -- nothing set, or an empty name, means the default group
            local fresh = Instance.new("Part")
            local unset = fresh.CollisionGroup
            assert(unset == nil or unset == "", tostring(unset))
            assert(service:CollisionGroupContainsPart("Default", fresh))
            fresh.CollisionGroup = ""
            assert(service:CollisionGroupContainsPart("Default", fresh))

            service:RemoveCollisionGroup("Blue")
            assert(not service:IsCollisionGroupRegistered("Blue"))
            assert(part.CollisionGroup == "Default", part.CollisionGroup)
            assert(service:CollisionGroupContainsPart("Default", part))
            "#,
        )
        .unwrap();
    }

    #[test]
    fn preloading_records_assets_and_reports_success() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("ContentProvider")
            local part = Instance.new("Part")
            part:SetAttribute("Image", "rbxassetid://42")
            part:SetAttribute("Texture", "not-an-asset")

            local seen = {}
            service:GetAssetFetchStatusChangedSignal("rbxassetid://42"):Connect(
                function(asset, status) table.insert(seen, asset .. ":" .. status) end)
            service:PreloadAsync({part})

            assert(#seen == 1, "one status per asset, got " .. #seen)
            assert(seen[1] == "rbxassetid://42:Success", seen[1])
            assert(service:GetAssetFetchStatus("rbxassetid://42") == Enum.AssetFetchStatus.Success)
            assert(service:GetAssetFetchStatus("rbxassetid://99") == Enum.AssetFetchStatus.None)
            assert(#service:GetFailedRequests() == 0)
            assert(#service:GetDetailedFailedRequests() == 0)
            assert(#service:ListEncryptedAssets() == 0)
            assert(service.RequestQueueSize == 0)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn preloading_warns_only_once() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("ContentProvider")
            local part = Instance.new("Part")
            part:SetAttribute("Texture", "rbxassetid://7")
            service:PreloadAsync({part})
            service:PreloadAsync({part})
            "#,
        )
        .unwrap();

        let warnings = vm.take_warnings();
        let preload = warnings.iter().filter(|text| text.contains("does not download")).count();
        assert_eq!(preload, 1, "{warnings:?}");
    }

    #[test]
    fn log_history_tracks_print_and_warn_then_clears() {
        let vm = vm();
        vm.eval(
            r#"
            print("first")
            warn("second")
            local history = game:GetService("LogService"):GetLogHistory()
            assert(#history == 2, "expected two entries, got " .. #history)
            assert(history[1].message == "first", history[1].message)
            assert(history[1].messageType == Enum.MessageType.MessageOutput)
            assert(history[2].messageType == Enum.MessageType.MessageWarning)
            assert(type(history[1].timestamp) == "number")
            game:GetService("LogService"):ClearOutput()
            assert(#game:GetService("LogService"):GetLogHistory() == 0)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn teleport_records_the_request_and_fails_it() {
        let vm = vm();
        vm.eval(
            r#"
            local players = game:GetService("Players")
            local alice = players:AddMockPlayer("Alice")
            _G.__alice = alice
            game:GetService("TeleportService").TeleportInitFailed:Connect(
                function(player, result, message, placeId)
                    _G.__teleport = {
                        player = player, result = result, message = message, placeId = placeId}
                end)
            game:GetService("TeleportService"):Teleport(1234, alice, {score = 7})
            "#,
        )
        .unwrap();

        vm.eval(
            r#"
            local record = _G.__teleport
            assert(record ~= nil, "TeleportInitFailed must fire")
            assert(record.player == _G.__alice)
            assert(record.result == "Failure", tostring(record.result))
            assert(record.placeId == 1234)
            assert(string.find(record.message, "second place", 1, true), record.message)
            "#,
        )
        .unwrap();

        let registry: Table = vm
            .lua()
            .named_registry_value("microstudio.teleport")
            .unwrap();
        let requests: Table = registry.get("requests").unwrap();
        assert_eq!(requests.raw_len(), 1);
        let first: Table = requests.get(1).unwrap();
        assert_eq!(first.get::<f64>("placeId").unwrap(), 1234.0);
        let users: Table = first.get("userIds").unwrap();
        assert_eq!(users.get::<f64>(1).unwrap(), 1.0);
        let data: Table = first.get("teleportData").unwrap();
        assert_eq!(data.get::<f64>("score").unwrap(), 7.0);

        let warnings = vm.take_warnings();
        assert!(
            warnings
                .iter()
                .any(|text| text.contains("does not load a second place")),
            "{warnings:?}"
        );
    }

    #[test]
    fn teleport_settings_round_trip_and_reserve_server_raises() {
        let vm = vm();
        vm.eval(
            r#"
            local service = game:GetService("TeleportService")
            assert(service:GetLocalPlayerTeleportData() == nil)
            assert(service:GetTeleportSetting("key") == nil)
            service:SetTeleportSetting("key", {a = 1})
            assert(service:GetTeleportSetting("key").a == 1)
            "#,
        )
        .unwrap();

        let message = eval_error(&vm, r#"game:GetService("TeleportService"):ReserveServer(1234)"#);
        assert!(message.contains("not supported locally"), "{message}");
    }
}
