
use serde_json::{json, Value};
use microstudio_datamodel::{AttributeValue, InstanceId};
use microstudio_luau::vm::{EvalMode, Vm, VmError, VmOptions};
use microstudio_luau::{ClockKind, RealClock, VirtualClock};
use microstudio_services::{LogEntry, RunServiceState, ScriptError, Store};
use mlua::Lua;

use crate::protocol::PlaceEntry;

pub use microstudio_luau::vm::RuntimeState;

fn normalize_class(class_name: &str) -> String {
    if class_name.is_empty() {
        return "Folder".to_string();
    }
    if microstudio_datamodel::classes::is_known(class_name) {
        class_name.to_string()
    } else {
        "Folder".to_string()
    }
}

// rojo typed form: {"Bool": true} / {"Vector3": [1, 2, 3]}, bare scalars too
fn rojo_property(value: &Value) -> Option<AttributeValue> {
    let (kind, inner) = match value {
        Value::Object(map) if map.len() == 1 => {
            let (key, inner) = map.iter().next()?;
            (key.as_str(), inner)
        }
        other => ("", other),
    };

    match (kind, inner) {
        (_, Value::Bool(v)) => Some(AttributeValue::Bool(*v)),
        (_, Value::Number(n)) => Some(AttributeValue::Number(n.as_f64()?)),
        ("Content", Value::String(s)) | (_, Value::String(s)) => {
            Some(AttributeValue::String(s.clone()))
        }
        ("Vector3", Value::Array(items)) if items.len() == 3 => Some(AttributeValue::Vector3(
            microstudio_types::Vector3::new(
                items[0].as_f64()?,
                items[1].as_f64()?,
                items[2].as_f64()?,
            ),
        )),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub clock: ClockKind,
    pub epoch: f64,
    pub run_service: RunServiceState,
    // where simulated services persist; RuntimeOptions::default() keeps everything in memory
    pub store: Store,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            clock: ClockKind::Virtual,
            epoch: VmOptions::default().epoch,
            run_service: RunServiceState::test(),
            store: Store::ephemeral(),
        }
    }
}

impl RuntimeOptions {
    pub fn dev() -> Self {
        Self {
            clock: ClockKind::Real,
            run_service: RunServiceState::dev(),
            ..Self::default()
        }
    }

    pub fn test() -> Self {
        Self::default()
    }
}

pub struct Runtime {
    vm: Vm,
}

impl Runtime {
    pub fn new(options: RuntimeOptions) -> Result<Self, VmError> {
        let vm = Vm::new(VmOptions {
            clock: options.clock,
            run_service: options.run_service,
            store: options.store,
            epoch: options.epoch,
            step_budget: microstudio_luau::vm::DEFAULT_STEP_BUDGET,
        })?;
        Ok(Self { vm })
    }

    pub fn vm(&self) -> &Vm {
        &self.vm
    }

    pub fn lua(&self) -> &Lua {
        self.vm.lua()
    }

    pub fn now(&self) -> f64 {
        self.vm.now()
    }

    pub fn clock_kind(&self) -> &'static str {
        self.vm.clock_kind()
    }

    pub fn eval(&self, code: &str) -> Result<Value, VmError> {
        let value = self.vm.eval(code)?;
        Ok(self.value_to_json(&value))
    }

    // auto mode echoes a bare expression such as workspace
    pub fn eval_mode(&self, code: &str, mode: EvalMode) -> Result<Value, VmError> {
        let value = self.vm.eval_mode(code, mode)?;
        Ok(self.value_to_json(&value))
    }

    // name appears in error locations, e.g. script.luau:12
    pub fn eval_named(&self, code: &str, mode: EvalMode, name: &str) -> Result<Value, VmError> {
        let value = self.vm.eval_named(code, mode, name)?;
        Ok(self.value_to_json(&value))
    }

    pub fn drain(&self) -> Result<(), VmError> {
        self.vm.run_until_idle()
    }

    // keeps the vm alive: cheap test isolation, not a new sidecar each time
    pub fn reset_world(&self) -> Result<(), VmError> {
        self.vm.reset_world()
    }

    pub fn advance_time(&self, seconds: f64) -> Result<(), VmError> {
        self.vm.advance_time(seconds)
    }

    // pump for wall-clock sessions; advanceTime is the deterministic path
    pub fn pump(&self) -> Result<(), VmError> {
        let now = self.vm.ctx().state.borrow().scheduler.now();
        let due = {
            let mut state = self.vm.ctx().state.borrow_mut();
            state.scheduler.take_due(now)
        };
        for entry in due {
            microstudio_luau::vm::resume_thread(
                self.vm.lua(),
                self.vm.ctx(),
                entry,
                mlua::MultiValue::new(),
            )?;
        }
        self.vm.run_ready()
    }

    pub fn run_server_scripts(&self) -> Result<Vec<InstanceId>, VmError> {
        self.vm.run_server_scripts()
    }

    // errors carry a script location
    pub fn run_source(
        &self,
        name: &str,
        source: &str,
        parent: Option<InstanceId>,
    ) -> Result<Option<InstanceId>, VmError> {
        let id = {
            let mut state = self.vm.ctx().state.borrow_mut();
            let script = state.world.dm.create_internal("Script")?;
            state.world.dm.set_name(script, name)?;
            state
                .world
                .dm
                .set_property(script, "Source", AttributeValue::String(source.to_string()))?;
            if let Some(parent) = parent {
                let events = state.world.dm.set_parent(script, Some(parent))?;
                drop(state);
                self.vm.dispatch(events)?;
            }
            script
        };
        self.vm.run_script(id)?;
        Ok(Some(id))
    }

    // entries arrive parent-first; missing folders are created along the path
    pub fn load_tree(&self, entries: &[PlaceEntry]) -> Result<Vec<String>, VmError> {
        use microstudio_datamodel::{classes, DataModel};

        fn ensure_child(
            dm: &mut DataModel,
            parent: InstanceId,
            name: &str,
            class_name: &str,
        ) -> Result<InstanceId, microstudio_datamodel::DataModelError> {
            if let Some(existing) = dm.find_first_child(parent, name) {
                return Ok(existing);
            }
            let descriptor = classes::descriptor(class_name);
            let is_service = descriptor.map(|d| d.is_service()).unwrap_or(false)
                || classes::descriptor(name)
                    .map(|d| d.is_service())
                    .unwrap_or(false);
            let id = if !is_service && descriptor.map(|d| d.is_creatable()).unwrap_or(false) {
                dm.create_instance(class_name)?
            } else {
                dm.get_service(name)?
            };
            dm.set_name(id, name)?;
            Ok(id)
        }

        let mut created = Vec::new();
        for entry in entries {
            let mut segments: Vec<&str> = entry
                .path
                .split('.')
                .filter(|segment| !segment.is_empty())
                .collect();
            // only a leading game is the root: game.server.luau keeps its game segment
            if segments.first() == Some(&"game") {
                segments.remove(0);
            }
            if segments.is_empty() {
                continue;
            }
            let class_name = normalize_class(&entry.class_name);
            let mut current = self.game();
            let mut events = Vec::new();

            for (index, segment) in segments.iter().enumerate() {
                let is_last = index == segments.len() - 1;
                let wanted = if is_last { class_name.as_str() } else { "Folder" };
                let before = {
                    let state = self.vm.ctx().state.borrow();
                    state.world.dm.find_first_child(current, segment)
                };

                let id = {
                    let mut state = self.vm.ctx().state.borrow_mut();
                    let id = ensure_child(&mut state.world.dm, current, segment, wanted)?;
                    if is_last {
                        if let Some(source) = &entry.source {
                            state.world.dm.set_property(
                                id,
                                "Source",
                                AttributeValue::String(source.clone()),
                            )?;
                        }
                        for (name, value) in &entry.properties {
                            if let Some(converted) = rojo_property(value) {
                                state
                                    .world
                                    .dm
                                    .set_property(id, name, converted)
                                    .ok();
                            }
                        }
                    }
                    id
                };

                if before.is_none() && id != current {
                    let mut parent_events = {
                        let mut state = self.vm.ctx().state.borrow_mut();
                        state.world.dm.set_parent(id, Some(current))?
                    };
                    events.append(&mut parent_events);
                }
                current = id;
            }

            self.vm.dispatch(events)?;
            created.push(entry.path.clone());
        }
        Ok(created)
    }

    pub fn add_mock_player(
        &self,
        name: &str,
        with_character: bool,
        user_id: Option<i64>,
    ) -> Result<InstanceId, VmError> {
        let (player, events) = {
            let mut state = self.vm.ctx().state.borrow_mut();
            state.world.add_mock_player(
                name,
                microstudio_services::PlayerOptions {
                    with_character,
                    user_id,
                },
            )?
        };
        self.vm.dispatch(events)?;
        Ok(player)
    }

    pub fn take_logs(&self) -> Vec<LogEntry> {
        self.vm.ctx().state.borrow_mut().world.take_logs()
    }

    pub fn take_errors(&self) -> Vec<ScriptError> {
        self.vm.ctx().state.borrow_mut().world.take_errors()
    }

    pub fn take_warnings(&self) -> Vec<String> {
        self.vm.take_warnings()
    }

    pub fn clock_kind_of(&self) -> &'static str {
        match self.vm.clock_kind() {
            "real" => "real",
            _ => "virtual",
        }
    }

    // linked luau revision, e.g. "Luau 0.740"
    pub fn luau_version(&self) -> String {
        self.vm.luau_version()
    }

    // only meaningful before a run starts
    pub fn set_clock(&self, kind: ClockKind) {
        let clock: Box<dyn microstudio_luau::Clock> = match kind {
            ClockKind::Virtual => Box::new(VirtualClock::new()),
            ClockKind::Real => Box::new(RealClock::new()),
        };
        self.vm.ctx().state.borrow_mut().scheduler.set_clock(clock);
    }

    pub fn stats(&self) -> Value {
        let state = self.vm.ctx().state.borrow();
        json!({
            "time": state.scheduler.now(),
            "clock": state.scheduler.clock_kind(),
            "pendingWork": state.scheduler.ready_len()
                + state.scheduler.timer_count()
                + state.scheduler.waiter_count(),
            "instances": state.world.dm.all_ids().len(),
            "logs": state.world.logs.len(),
            "errors": state.world.errors.len(),
        })
    }


    pub fn find_by_path(&self, path: &str) -> Option<InstanceId> {
        let state = self.vm.ctx().state.borrow();
        let dm = &state.world.dm;
        let mut parts = path.split('.').filter(|part| !part.is_empty());

        let first = parts.next()?;
        let mut current = if first == "game" {
            dm.game()
        } else if first == "workspace" {
            dm.find_first_child(dm.game(), "Workspace")?
        } else {
            dm.find_first_child(dm.game(), first)?
        };

        for part in parts {
            current = dm.find_first_child(current, part)?;
        }
        Some(current)
    }

    pub fn game(&self) -> InstanceId {
        self.vm.ctx().state.borrow().world.game()
    }


    pub fn instance_json(&self, id: InstanceId) -> Value {
        let state = self.vm.ctx().state.borrow();
        let dm = &state.world.dm;
        let class = dm.class_of(id).unwrap_or("<destroyed>");
        json!({
            "id": id.0,
            "name": dm.name_of(id).unwrap_or(""),
            "className": class,
            "path": dm.path(id, true),
            "childCount": dm.children_of(id).len(),
            "attributes": self.attributes_json(id),
        })
    }

    fn attributes_json(&self, id: InstanceId) -> Value {
        let state = self.vm.ctx().state.borrow();
        let mut map = serde_json::Map::new();
        for (name, value) in state.world.dm.get_attributes(id) {
            map.insert(name, attribute_to_json(&value));
        }
        Value::Object(map)
    }

    pub fn tree_json(&self, root: InstanceId, depth: usize) -> Value {
        let state = self.vm.ctx().state.borrow();
        subtree_json(&state, root, depth)
    }

    pub fn tree(&self, path: Option<&str>, depth: usize) -> Value {
        let root = match path {
            Some(path) => match self.find_by_path(path) {
                Some(id) => id,
                None => return json!({ "error": format!("'{path}' was not found") }),
            },
            None => self.game(),
        };
        self.tree_json(root, depth)
    }

    pub fn value_to_json(&self, value: &mlua::Value) -> Value {
        let state = self.vm.ctx().state.borrow();
        lua_to_json(self.vm.lua(), &state, value, 0)
    }
}

fn subtree_json(state: &std::cell::Ref<'_, RuntimeState>, id: InstanceId, depth: usize) -> Value {
    let dm = &state.world.dm;
    let mut node = json!({
        "id": id.0,
        "name": dm.name_of(id).unwrap_or(""),
        "className": dm.class_of(id).unwrap_or("<destroyed>"),
        "path": dm.path(id, true),
    });

    if depth == 0 {
        let count = dm.children_of(id).len();
        if count > 0 {
            node["childCount"] = json!(count);
        }
        return node;
    }

    let children: Vec<Value> = dm
        .children_of(id)
        .iter()
        .map(|child| subtree_json(state, *child, depth - 1))
        .collect();
    if !children.is_empty() {
        node["children"] = Value::Array(children);
    }
    node
}

fn attribute_to_json(value: &AttributeValue) -> Value {
    json!({
        "type": value.type_name(),
        "value": value_string(value),
    })
}

fn value_string(value: &AttributeValue) -> String {
    match value {
        AttributeValue::Bool(v) => v.to_string(),
        AttributeValue::Number(v) => microstudio_luau::bind::datatype::num(*v),
        AttributeValue::String(v) => v.clone(),
        AttributeValue::Instance(id) => format!("InstanceId({})", id.0),
        other => format!("{:?}", other),
    }
}

const MAX_JSON_DEPTH: usize = 8;

fn lua_to_json(
    lua: &Lua,
    state: &std::cell::Ref<'_, RuntimeState>,
    value: &mlua::Value,
    depth: usize,
) -> Value {
    if depth > MAX_JSON_DEPTH {
        return json!("...");
    }
    match value {
        mlua::Value::Nil => Value::Null,
        mlua::Value::Boolean(v) => json!(v),
        mlua::Value::Integer(v) => json!(v),
        mlua::Value::Number(v) => json!(v),
        mlua::Value::String(v) => json!(v.to_string_lossy().to_string()),
        mlua::Value::Function(_) => json!("<function>"),
        mlua::Value::Thread(_) => json!("<thread>"),
        mlua::Value::UserData(ud) => {
            if let Ok(instance) = ud.borrow::<microstudio_luau::bind::instance::LuaInstance>() {
                let dm = &state.world.dm;
                return json!({
                    "__type": "Instance",
                    "id": instance.id.0,
                    "name": dm.name_of(instance.id).unwrap_or(""),
                    "className": dm.class_of(instance.id).unwrap_or("<destroyed>"),
                    "path": dm.path(instance.id, true),
                });
            }
            // mlua coerce_string can't see __tostring; render like roblox does
            json!(microstudio_luau::convert::tostring_for_log(lua, value))
        }
        mlua::Value::Table(table) => {
            // empty table -> [] so it round-trips as an array, not {}
            if table.pairs::<mlua::Value, mlua::Value>().next().is_none() {
                return Value::Array(Vec::new());
            }

            let len = table.raw_len();
            let mut is_array = len > 0;
            if is_array {
                for index in 1..=len {
                    let has: mlua::Value = table.raw_get(index).unwrap_or(mlua::Value::Nil);
                    if matches!(has, mlua::Value::Nil) {
                        is_array = false;
                        break;
                    }
                }
            }

            if is_array {
                let mut out = Vec::with_capacity(len);
                for index in 1..=len {
                    let item: mlua::Value = match table.raw_get(index) {
                        Ok(item) => item,
                        Err(_) => mlua::Value::Nil,
                    };
                    out.push(lua_to_json(lua, state, &item, depth + 1));
                }
                return Value::Array(out);
            }

            let mut map = serde_json::Map::new();
            for pair in table.pairs::<mlua::Value, mlua::Value>().flatten() {
                let (key, item) = pair;
                let key = match &key {
                    mlua::Value::String(s) => s.to_string_lossy().to_string(),
                    other => format!("{}", lua_to_json(lua, state, other, depth + 1)),
                };
                map.insert(key, lua_to_json(lua, state, &item, depth + 1));
            }
            Value::Object(map)
        }
        other => json!(format!("<{}>", other.type_name())),
    }
}

