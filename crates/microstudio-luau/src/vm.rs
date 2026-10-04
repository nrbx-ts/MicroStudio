// never call into luau while a RuntimeState borrow is held: it can re-enter and panic
// each mutation is: borrow, mutate, collect events, drop, then dispatch

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use mlua::{Function, Lua, MultiValue, Result as LuaResult, Table, Value};
use microstudio_datamodel::{InstanceId, PendingEvent, Signal, SignalId};
use microstudio_services::{RunServiceState, Store, World};

use crate::clock::{Clock, ClockKind, RealClock, VirtualClock};
use crate::convert::{self, InstanceCache};
use crate::prelude::PRELUDE;
use crate::scheduler::{Entry, ResumeKind, Scheduler, FRAME_TIME};

// cap on scheduler steps: a self-rescheduling task.wait loop would spin forever
pub const DEFAULT_STEP_BUDGET: u64 = 500_000;

#[derive(Debug, Clone)]
pub struct VmOptions {
    pub clock: ClockKind,
    pub run_service: RunServiceState,
    // simulated service data; ephemeral by default so a test never writes to disk
    pub store: Store,
    // fixed for reproducible runs; reported by tick()
    pub epoch: f64,
    pub step_budget: u64,
}

impl Default for VmOptions {
    fn default() -> Self {
        Self {
            clock: ClockKind::Virtual,
            run_service: RunServiceState::test(),
            store: Store::ephemeral(),
            epoch: 1_700_000_000.0,
            step_budget: DEFAULT_STEP_BUDGET,
        }
    }
}

pub struct LuaListener {
    pub function: Function,
    pub owner: Option<InstanceId>,
    pub once: bool,
}

pub struct RuntimeState {
    pub world: World,
    pub scheduler: Scheduler,
    // keyed by id; tables must stay identity-stable
    pub signals: HashMap<SignalId, Signal>,
    pub signal_tables: HashMap<SignalId, Table>,
    pub listeners: HashMap<SignalId, Vec<(u64, LuaListener)>>,
    // connection handle -> its signal
    pub connection_owner: HashMap<u64, SignalId>,
    pub connection_tables: HashMap<u64, Table>,
    pub next_connection: u64,
    pub cache: InstanceCache,
    // require results cached per ModuleScript
    pub modules: HashMap<InstanceId, Value>,
    // completion values of chunks the driver is waiting on, keyed by thread address
    // a chunk the scheduler resumes can't return its value any other way
    pub completed: HashMap<usize, Value>,
    pub epoch: f64,
    pub step_budget: u64,
    // set by advanceTime when the step budget runs out
    pub budget_exhausted: bool,
}

#[derive(Clone)]
pub struct Ctx {
    pub state: Rc<RefCell<RuntimeState>>,
}

impl Ctx {
    pub fn new(state: RuntimeState) -> Self {
        Self {
            state: Rc::new(RefCell::new(state)),
        }
    }

    pub fn current_script(&self) -> Option<InstanceId> {
        self.state.borrow().scheduler.current_script()
    }

    pub fn now(&self) -> f64 {
        self.state.borrow().scheduler.now()
    }

    // drop the guard before running luau
    pub fn with_world<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.state.borrow_mut().world)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error(transparent)]
    Lua(#[from] mlua::Error),
    #[error(transparent)]
    DataModel(#[from] microstudio_datamodel::DataModelError),
}

impl From<VmError> for mlua::Error {
    fn from(value: VmError) -> Self {
        match value {
            VmError::Lua(err) => err,
            other => mlua::Error::runtime(other.to_string()),
        }
    }
}

pub enum ResumeOutcome {
    Finished(Value),
    Yielded(Value),
    Errored,
}

pub struct Vm {
    lua: Lua,
    ctx: Ctx,
}

// installs game/workspace and the shared tables scripts expect
pub fn install_globals(lua: &Lua, ctx: &Ctx) -> Result<(), VmError> {
    let globals = lua.globals();
    let game_id = ctx.state.borrow().world.game();
    let game = instance_userdata(lua, ctx, game_id)?;
    globals.set("game", game.clone())?;
    globals.set("Game", game)?;

    let workspace_id = {
        let mut state = ctx.state.borrow_mut();
        state.world.get_service("Workspace")?
    };
    let workspace = instance_userdata(lua, ctx, workspace_id)?;
    globals.set("workspace", workspace.clone())?;
    globals.set("Workspace", workspace)?;

    // _G and shared are the same table, like roblox
    let shared = lua.create_table()?;
    globals.set("_G", shared.clone())?;
    globals.set("shared", shared)?;
    Ok(())
}

// in place; see Vm::reset_world for what survives
pub fn reset_world(lua: &Lua, ctx: &Ctx) -> Result<(), VmError> {
    let options = lua
        .app_data_ref::<VmOptions>()
        .map(|options| options.clone())
        .unwrap_or_default();

    {
        let mut state = ctx.state.borrow_mut();
        *state = fresh_state(&options)?;
    }

    // game/workspace are new instances, rebuild the globals pointing at them
    install_globals(lua, ctx)
}

fn fresh_state(options: &VmOptions) -> Result<RuntimeState, VmError> {
    let mut world = World::with_store(options.store.clone());
    world.bootstrap()?;
    world.run_service = options.run_service;
    world.streaming = true;

    let clock: Box<dyn Clock> = match options.clock {
        ClockKind::Virtual => Box::new(VirtualClock::new()),
        ClockKind::Real => Box::new(RealClock::new()),
    };

    Ok(RuntimeState {
        world,
        scheduler: Scheduler::new(clock),
        signals: HashMap::new(),
        signal_tables: HashMap::new(),
        listeners: HashMap::new(),
        connection_owner: HashMap::new(),
        connection_tables: HashMap::new(),
        next_connection: 1,
        cache: InstanceCache::default(),
        modules: HashMap::new(),
        completed: HashMap::new(),
        epoch: options.epoch,
        step_budget: options.step_budget,
        budget_exhausted: false,
    })
}

impl Vm {
    pub fn new(options: VmOptions) -> Result<Self, VmError> {
        let lua = Lua::new();
        lua.set_app_data(options.clone());

        let ctx = Ctx::new(fresh_state(&options)?);

        crate::bind::datatype::install(&lua)?;
        crate::bind::globals::install(&lua, &ctx)?;
        crate::bind::instance::install(&lua, &ctx)?;
        crate::bind::services::install(&lua, &ctx)?;
        crate::bind::signal::install(&lua, &ctx)?;

        let vm = Vm { lua, ctx };
        vm.install_globals()?;
        vm.install_prelude()?;
        Ok(vm)
    }

    // keeps the Lua VM; instances, signals, listeners, parked threads, require cache all reset
    // registry values (prelude, bindings, test bookkeeping) survive
    pub fn reset_world(&self) -> Result<(), VmError> {
        reset_world(&self.lua, &self.ctx)
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    pub fn now(&self) -> f64 {
        self.ctx.now()
    }

    pub fn clock_kind(&self) -> &'static str {
        self.ctx.state.borrow().scheduler.clock_kind()
    }

    pub fn epoch(&self) -> f64 {
        self.ctx.state.borrow().epoch
    }

    // reads _VERSION; reflects the compiled luau, not a manifest claim
    pub fn luau_version(&self) -> String {
        self.lua
            .globals()
            .get::<mlua::LuaString>("_VERSION")
            .map(|version| version.to_string_lossy().to_string())
            .unwrap_or_else(|_| "Luau (unknown)".to_string())
    }

    fn install_globals(&self) -> Result<(), VmError> {
        install_globals(&self.lua, &self.ctx)
    }

    fn install_prelude(&self) -> Result<(), VmError> {
        let prelude: Table = self
            .lua
            .load(PRELUDE)
            .set_name("@MicroStudio/Prelude")
            .eval()?;
        self.lua.set_named_registry_value(
            "microstudio.signal_meta",
            prelude.get::<Table>("signal_meta")?,
        )?;
        self.lua.set_named_registry_value(
            "microstudio.connection_meta",
            prelude.get::<Table>("connection_meta")?,
        )?;
        self.lua.set_named_registry_value(
            "microstudio.lua_instance_methods",
            prelude.get::<Table>("instance_methods")?,
        )?;
        Ok(())
    }

    pub fn run_script(&self, id: InstanceId) -> Result<(), VmError> {
        run_script(&self.lua, &self.ctx, id)
    }

    pub fn run_server_scripts(&self) -> Result<Vec<InstanceId>, VmError> {
        run_server_scripts(&self.lua, &self.ctx)
    }

    pub fn eval(&self, code: &str) -> Result<Value, VmError> {
        eval(&self.lua, &self.ctx, code)
    }

    pub fn eval_mode(&self, code: &str, mode: EvalMode) -> Result<Value, VmError> {
        eval_mode(&self.lua, &self.ctx, code, mode)
    }

    pub fn eval_named(&self, code: &str, mode: EvalMode, name: &str) -> Result<Value, VmError> {
        eval_named(&self.lua, &self.ctx, code, mode, name)
    }

    pub fn run_until_idle(&self) -> Result<(), VmError> {
        run_until_idle(&self.lua, &self.ctx)
    }

    pub fn advance_time(&self, seconds: f64) -> Result<(), VmError> {
        advance_time(&self.lua, &self.ctx, seconds)
    }

    pub fn run_ready(&self) -> Result<(), VmError> {
        run_ready(&self.lua, &self.ctx)
    }

    pub fn dispatch(&self, events: Vec<PendingEvent>) -> Result<(), VmError> {
        dispatch(&self.lua, &self.ctx, events)
    }

    pub fn pending_work(&self) -> usize {
        let state = self.ctx.state.borrow();
        state.scheduler.ready_len() + state.scheduler.timer_count() + state.scheduler.waiter_count()
    }

    pub fn take_warnings(&self) -> Vec<String> {
        self.ctx.state.borrow_mut().scheduler.take_warnings()
    }
}

pub fn instance_userdata(lua: &Lua, ctx: &Ctx, id: InstanceId) -> LuaResult<mlua::AnyUserData> {
    if let Some(existing) = ctx.state.borrow().cache.existing(id) {
        return Ok(existing);
    }
    let ud = lua.create_userdata(crate::bind::instance::LuaInstance {
        id,
        ctx: ctx.clone(),
    })?;
    ctx.state.borrow_mut().cache.insert(id, ud.clone());
    Ok(ud)
}

pub fn instance_value(lua: &Lua, ctx: &Ctx, id: InstanceId) -> LuaResult<Value> {
    Ok(Value::UserData(instance_userdata(lua, ctx, id)?))
}

pub fn run_ready(lua: &Lua, ctx: &Ctx) -> Result<(), VmError> {
    loop {
        let entry = {
            let mut state = ctx.state.borrow_mut();
            state
                .scheduler
                .pop_ready()
                .or_else(|| state.scheduler.pop_deferred())
        };
        match entry {
            Some(entry) => resume_thread(lua, ctx, entry, MultiValue::new())?,
            None => return Ok(()),
        }
    }
}

pub fn advance_time(lua: &Lua, ctx: &Ctx, seconds: f64) -> Result<(), VmError> {
    let seconds = seconds.max(0.0);
    let is_virtual = ctx.state.borrow().scheduler.clock_is_virtual();

    if !is_virtual {
        std::thread::sleep(Duration::from_secs_f64(seconds));
        let now = ctx.state.borrow().scheduler.now();
        let due = {
            let mut state = ctx.state.borrow_mut();
            state.scheduler.take_due(now)
        };
        for entry in due {
            resume_thread(lua, ctx, entry, MultiValue::new())?;
        }
        return run_ready(lua, ctx);
    }

    let target = ctx.state.borrow().scheduler.now() + seconds;
    let budget = ctx.state.borrow().step_budget;
    let mut steps = 0u64;

    loop {
        run_ready(lua, ctx)?;

        let deadline = ctx.state.borrow().scheduler.next_deadline();
        match deadline {
            Some(deadline) if deadline <= target => {
                let due = {
                    let mut state = ctx.state.borrow_mut();
                    state.scheduler.advance_clock_to(deadline);
                    state.world.time = deadline;
                    state.scheduler.take_due(deadline)
                };
                for entry in due {
                    resume_thread(lua, ctx, entry, MultiValue::new())?;
                }
            }
            _ => break,
        }

        steps += 1;
        if steps >= budget {
            let mut state = ctx.state.borrow_mut();
            state.budget_exhausted = true;
            state.scheduler.warn(format!(
                "advanceTime stopped after {budget} scheduler steps; a task is probably \
                 rescheduling itself in a tight loop"
            ));
            break;
        }
    }

    {
        let mut state = ctx.state.borrow_mut();
        state.scheduler.advance_clock_to(target);
        state.world.time = target;
    }
    run_ready(lua, ctx)
}

// drains scheduled work before the process exits, so a script behaves like a program
// parked signal waiters don't count as work and end the loop
pub fn run_until_idle(lua: &Lua, ctx: &Ctx) -> Result<(), VmError> {
    run_ready(lua, ctx)?;

    let budget = ctx.state.borrow().step_budget;
    for _ in 0..budget {
        let delta = {
            let state = ctx.state.borrow();
            let now = state.scheduler.now();
            state
                .scheduler
                .next_deadline()
                .map(|deadline| (deadline - now).max(0.0))
        };
        match delta {
            Some(delta) => advance_time(lua, ctx, delta)?,
            None => break,
        }
    }

    if ctx.state.borrow().scheduler.next_deadline().is_some() {
        let mut state = ctx.state.borrow_mut();
        state.budget_exhausted = true;
        state.scheduler.warn(format!(
            "run stopped after {budget} scheduler steps; a task is probably \
             rescheduling itself in a tight loop"
        ));
    }
    Ok(())
}

// roblox order: listeners (connection order) first, then parked :Wait()ers
// listeners run as their own threads so a yielding handler works
pub fn dispatch(lua: &Lua, ctx: &Ctx, events: Vec<PendingEvent>) -> Result<(), VmError> {
    for event in events {
        let signal_id = event.signal.id();

        {
            let mut state = ctx.state.borrow_mut();
            state
                .signals
                .entry(signal_id)
                .or_insert_with(|| event.signal.clone());
        }

        let listeners = {
            let state = ctx.state.borrow();
            state
                .listeners
                .get(&signal_id)
                .map(|list| {
                    list.iter()
                        .map(|(handle, listener)| {
                            (*handle, listener.function.clone(), listener.owner, listener.once)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        for (handle, function, owner, once) in listeners {
            if once {
                crate::bind::signal::disconnect(ctx, handle);
            }
            let args = event_args_multi(lua, ctx, &event)?;
            let thread = lua.create_thread(function)?;
            resume_thread(
                lua,
                ctx,
                Entry {
                    thread,
                    owner,
                    resume: ResumeKind::None,
                },
                args,
            )?;
        }

        let waiters = {
            let mut state = ctx.state.borrow_mut();
            state.scheduler.take_waiters(signal_id)
        };
        for entry in waiters {
            let args = event_args_multi(lua, ctx, &event)?;
            resume_thread(lua, ctx, entry, args)?;
        }

        // rust-side listeners, for tests and services
        event.signal.fire(&event.args);
    }
    Ok(())
}

pub fn event_args_multi(lua: &Lua, ctx: &Ctx, event: &PendingEvent) -> LuaResult<MultiValue> {
    let mut state = ctx.state.borrow_mut();
    convert::event_args_to_multi(lua, &mut state.cache, ctx, &event.args)
}

pub fn resume_once(
    lua: &Lua,
    ctx: &Ctx,
    entry: &mut Entry,
    args: MultiValue,
) -> Result<ResumeOutcome, VmError> {
    let owner = entry.owner;
    let ptr = entry.thread.state() as usize;
    {
        let mut state = ctx.state.borrow_mut();
        state.scheduler.push_script(owner);
        state.world.time = state.scheduler.now();
    }
    let previous = set_script_global(lua, ctx, owner)?;

    let resume = std::mem::replace(&mut entry.resume, ResumeKind::None);
    let args = match resume {
        ResumeKind::Elapsed { since } => {
            let now = ctx.state.borrow().scheduler.now();
            let mut multi = MultiValue::new();
            multi.push_back(Value::Number(now - since));
            multi
        }
        ResumeKind::Values(multi) => multi,
        ResumeKind::None => args,
    };

    let outcome = entry.thread.resume::<Value>(args);

    lua.globals().set("script", previous)?;
    {
        let mut state = ctx.state.borrow_mut();
        state.scheduler.pop_script();
    }

    match outcome {
        Ok(value) => {
            if entry.thread.is_resumable() {
                Ok(ResumeOutcome::Yielded(value))
            } else {
                // may finish here after the scheduler resumed it, not in run_chunk
                let mut state = ctx.state.borrow_mut();
                if state.completed.contains_key(&ptr) {
                    state.completed.insert(ptr, value.clone());
                }
                drop(state);
                Ok(ResumeOutcome::Finished(value))
            }
        }
        Err(error) => {
            let location = script_name(ctx, owner);
            let formatted = convert::format_error(&error, &location);
            let mut state = ctx.state.borrow_mut();
            state.world.record_error(formatted);
            Ok(ResumeOutcome::Errored)
        }
    }
}

pub fn resume_thread(lua: &Lua, ctx: &Ctx, mut entry: Entry, args: MultiValue) -> Result<(), VmError> {
    match resume_once(lua, ctx, &mut entry, args)? {
        ResumeOutcome::Finished(_) | ResumeOutcome::Errored => Ok(()),
        ResumeOutcome::Yielded(value) => handle_yield(ctx, &entry, value),
    }
}

// own thread so it may yield
pub fn call_in_thread(
    lua: &Lua,
    ctx: &Ctx,
    function: Function,
    owner: Option<InstanceId>,
    args: MultiValue,
) -> Result<(), VmError> {
    let thread = lua.create_thread(function)?;
    resume_thread(
        lua,
        ctx,
        Entry {
            thread,
            owner,
            resume: ResumeKind::None,
        },
        args,
    )
}

fn handle_yield(ctx: &Ctx, entry: &Entry, value: Value) -> Result<(), VmError> {
    let Value::Table(ref table) = value else {
        ctx.state.borrow_mut().scheduler.warn(format!(
            "script yielded a {}, which MicroStudio cannot resume; the thread stays suspended",
            value.type_name()
        ));
        return Ok(());
    };

    let tag: Option<String> = table.get("__microstudio")?;
    match tag.as_deref() {
        Some("wait") => {
            let seconds = table.get::<Option<f64>>("seconds")?.unwrap_or(FRAME_TIME);
            let mut state = ctx.state.borrow_mut();
            let now = state.scheduler.now();
            state.scheduler.push_delay(
                seconds,
                Entry {
                    thread: entry.thread.clone(),
                    owner: entry.owner,
                    resume: ResumeKind::Elapsed { since: now },
                },
            );
            Ok(())
        }
        Some("signal") => {
            let signal_table = table.get::<Table>("signal")?;
            let signal_id = signal_table.raw_get::<u64>("__microstudio_signal")?;
            let mut state = ctx.state.borrow_mut();
            state.scheduler.park_on_signal(
                SignalId(signal_id),
                Entry {
                    thread: entry.thread.clone(),
                    owner: entry.owner,
                    resume: ResumeKind::None,
                },
            );
            Ok(())
        }
        _ => {
            ctx.state.borrow_mut().scheduler.warn(
                "script called coroutine.yield directly; the thread stays suspended until \
                 something resumes it (MicroStudio has no generic resume API in V1)"
                    .to_string(),
            );
            Ok(())
        }
    }
}

fn set_script_global(lua: &Lua, ctx: &Ctx, owner: Option<InstanceId>) -> LuaResult<Value> {
    let globals = lua.globals();
    let previous: Value = globals.get("script")?;
    match owner {
        Some(id) => globals.set("script", instance_value(lua, ctx, id)?)?,
        None => globals.set("script", Value::Nil)?,
    }
    Ok(previous)
}

pub fn script_name(ctx: &Ctx, owner: Option<InstanceId>) -> String {
    let state = ctx.state.borrow();
    match owner {
        Some(id) => state.world.dm.path(id, false),
        None => "MicroStudio".to_string(),
    }
}

pub fn run_script(lua: &Lua, ctx: &Ctx, id: InstanceId) -> Result<(), VmError> {
    let (source, name) = {
        let state = ctx.state.borrow();
        let data = state.world.dm.get(id)?;
        if data.properties.get("Disabled") == Some(&microstudio_datamodel::AttributeValue::Bool(true)) {
            return Ok(());
        }
        let source = data
            .properties
            .get("Source")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        (source, state.world.dm.path(id, false))
    };

    if source.trim().is_empty() {
        return Ok(());
    }

    let function = lua
        .load(&source)
        .set_name(format!("@{name}"))
        .into_function()?;
    call_in_thread(lua, ctx, function, Some(id), MultiValue::new())
}

// enabled server Scripts under ServerScriptService, tree order
pub fn run_server_scripts(lua: &Lua, ctx: &Ctx) -> Result<Vec<InstanceId>, VmError> {
    let scripts = {
        let state = ctx.state.borrow();
        let dm = &state.world.dm;
        let Some(container) = dm.find_first_child(dm.game(), "ServerScriptService") else {
            return Ok(Vec::new());
        };
        dm.get_descendants(container)
            .into_iter()
            .filter(|id| {
                let is_script = dm.is_a(*id, "Script") && !dm.is_a(*id, "LocalScript");
                let enabled = dm.get_property(*id, "Disabled")
                    != Some(microstudio_datamodel::AttributeValue::Bool(true));
                is_script && enabled
            })
            .collect::<Vec<_>>()
    };

    for script in &scripts {
        run_script(lua, ctx, *script)?;
    }
    Ok(scripts)
}

// Auto is for the REPL: a bare expression should echo, not be rejected
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EvalMode {
    #[default]
    Statement,
    Expression,
    Auto,
}

impl EvalMode {
    // unknown wire values are statements
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("expression") => EvalMode::Expression,
            Some("auto") => EvalMode::Auto,
            _ => EvalMode::Statement,
        }
    }
}

pub fn eval(lua: &Lua, ctx: &Ctx, code: &str) -> Result<Value, VmError> {
    eval_mode(lua, ctx, code, EvalMode::Statement)
}

pub fn eval_mode(lua: &Lua, ctx: &Ctx, code: &str, mode: EvalMode) -> Result<Value, VmError> {
    eval_named(lua, ctx, code, mode, "MicroStudio.repl")
}

// name is what error locations report
pub fn eval_named(
    lua: &Lua,
    ctx: &Ctx,
    code: &str,
    mode: EvalMode,
    name: &str,
) -> Result<Value, VmError> {
    let chunk_name = format!("@{name}");
    let compile = |source: String| lua.load(source).set_name(chunk_name.clone()).into_function();
    let function = match mode {
        EvalMode::Statement => compile(code.to_string())?,
        EvalMode::Expression => compile(format!("return {code}"))?,
        EvalMode::Auto => {
            // compile as expression first; compiling has no side effects, no double-run
            match compile(format!("return {code}")) {
                Ok(function) => function,
                Err(_) => compile(code.to_string())?,
            }
        }
    };
    run_chunk(lua, ctx, function)
}

fn run_chunk(lua: &Lua, ctx: &Ctx, function: Function) -> Result<Value, VmError> {
    let thread = lua.create_thread(function)?;
    let ptr = thread.state() as usize;
    let mut entry = Entry {
        thread: thread.clone(),
        owner: None,
        resume: ResumeKind::None,
    };

    // if it yields the scheduler resumes it; the value comes back via completed
    ctx.state.borrow_mut().completed.insert(ptr, Value::Nil);

    let finish = |ctx: &Ctx| -> Value {
        ctx.state
            .borrow_mut()
            .completed
            .remove(&ptr)
            .unwrap_or(Value::Nil)
    };

    match resume_once(lua, ctx, &mut entry, MultiValue::new())? {
        ResumeOutcome::Finished(value) => {
            finish(ctx);
            return Ok(value);
        }
        ResumeOutcome::Errored => {
            finish(ctx);
            return Ok(Value::Nil);
        }
        // must park here, else the yield is dropped and the rest never runs
        ResumeOutcome::Yielded(value) => handle_yield(ctx, &entry, value)?,
    }

    for _ in 0..1024 {
        run_ready(lua, ctx)?;
        if !thread.is_resumable() {
            break;
        }
        let delta = {
            let state = ctx.state.borrow();
            let now = state.scheduler.now();
            state
                .scheduler
                .next_deadline()
                .map(|deadline| (deadline - now).max(0.0))
        };
        match delta {
            Some(delta) => advance_time(lua, ctx, delta)?,
            None => break,
        }
    }

    Ok(finish(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm() -> Vm {
        Vm::new(VmOptions::default()).expect("vm boots")
    }

    #[test]
    fn boots_with_default_services() {
        let vm = vm();
        assert_eq!(vm.clock_kind(), "virtual");
        assert_eq!(vm.now(), 0.0);
    }

    #[test]
    fn evaluates_luau() {
        let vm = vm();
        let value = vm.eval("return 1 + 2").unwrap();
        assert_eq!(value, Value::Integer(3));
    }

    #[test]
    fn instances_round_trip_through_luau() {
        let vm = vm();
        vm.eval(
            r#"
            local part = Instance.new("Part")
            part.Name = "TestPart"
            part.Parent = workspace
            assert(workspace:FindFirstChild("TestPart") == part, "identity must hold")
            assert(part.ClassName == "Part")
            assert(part:IsA("BasePart"))
            assert(typeof(part) == "Instance")
            assert(part.Size == Vector3.new(4, 1, 2))
            part.Size = Vector3.new(1, 2, 3)
            assert(part.Size.Y == 2)
            _G.__part = part
            "#,
        )
        .unwrap();

        let state = vm.ctx.state.borrow();
        let ws = state.world.dm.find_first_child(state.world.dm.game(), "Workspace").unwrap();
        let part = state.world.dm.find_first_child(ws, "TestPart").unwrap();
        assert_eq!(
            state.world.dm.get_property(part, "Size"),
            Some(microstudio_datamodel::AttributeValue::Vector3(
                microstudio_types::Vector3::new(1.0, 2.0, 3.0)
            ))
        );
        assert!(state.world.errors.is_empty(), "{:?}", state.world.errors);
    }

    #[test]
    fn signals_fire_listeners_synchronously() {
        let vm = vm();
        vm.eval(
            r#"
            local folder = Instance.new("Folder")
            folder.Parent = workspace
            _G.__fired = false
            folder.ChildAdded:Connect(function(child)
                _G.__fired = child.Name
            end)
            local part = Instance.new("Part")
            part.Name = "Child"
            part.Parent = folder
            "#,
        )
        .unwrap();
        let fired = vm.eval("return _G.__fired").unwrap();
        assert_eq!(fired, Value::String(vm.lua().create_string("Child").unwrap()));
    }

    #[test]
    fn method_calls_on_connections_work() {
        let vm = vm();
        vm.eval(
            r#"
            local folder = Instance.new("Folder")
            folder.Parent = workspace
            _G.__count = 0
            local conn = folder.ChildAdded:Connect(function() _G.__count = _G.__count + 1 end)
            local a = Instance.new("Part") a.Parent = folder
            local b = Instance.new("Part") b.Parent = folder
            conn:Disconnect()
            local c = Instance.new("Part") c.Parent = folder
            "#,
        )
        .unwrap();
        assert_eq!(vm.eval("return _G.__count").unwrap(), Value::Integer(2));
    }

    #[test]
    fn task_delay_uses_the_virtual_clock() {
        let vm = vm();
        vm.eval("_G.__log = {}; task.delay(5, function() table.insert(_G.__log, 'later') end)")
            .unwrap();
        assert!(vm.eval("return #_G.__log").unwrap() == Value::Integer(0));
        vm.advance_time(5.0).unwrap();
        assert_eq!(vm.eval("return _G.__log[1]").unwrap(), Value::String(vm.lua().create_string("later").unwrap()));
    }

    #[test]
    fn task_wait_returns_elapsed_simulated_time() {
        let vm = vm();
        vm.eval(
            r#"
            _G.__elapsed = nil
            task.spawn(function()
                local dt = task.wait(2)
                _G.__elapsed = dt
            end)
            "#,
        )
        .unwrap();
        vm.advance_time(2.0).unwrap();
        let elapsed = vm.eval("return _G.__elapsed").unwrap();
        assert_eq!(elapsed, Value::Number(2.0));
    }

    #[test]
    fn signal_wait_resumes_the_thread() {
        let vm = vm();
        vm.eval(
            r#"
            _G.__got = nil
            local folder = Instance.new("Folder")
            folder.Parent = workspace
            task.spawn(function()
                local child = folder.ChildAdded:Wait()
                _G.__got = child.Name
            end)
            "#,
        )
        .unwrap();
        vm.eval("local p = Instance.new('Part') p.Name = 'Arrived' p.Parent = workspace.Folder")
            .unwrap();
        assert_eq!(
            vm.eval("return _G.__got").unwrap(),
            Value::String(vm.lua().create_string("Arrived").unwrap())
        );
    }

    #[test]
    fn stringified_values() {
        let vm = vm();
        vm.eval(
            r#"
            local part = Instance.new("Part")
            part.Name = "Widget"
            _G.__lines = {}
            print(Vector3.new(1, 2, 3), Color3.new(1, 1, 1), UDim.new(1, 2))
            print(part, workspace, nil, true, 1.5)
            print(CFrame.new(0, 5, 0), BrickColor.new("Really red"))
            "#,
        )
        .unwrap();
        let state = vm.ctx.state.borrow();
        let logs: Vec<&str> = state
            .world
            .logs
            .iter()
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(logs[0], "1, 2, 3 1, 1, 1 {1, 2}");
        assert_eq!(logs[1], "Widget Workspace nil true 1.5");
        // roblox prints every cframe component and just the palette name
        assert_eq!(logs[2], "0, 5, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1 Really red");
    }

    #[test]
    fn reset_world_clears_everything_that_leaks_between_tests() {
        let vm = vm();
        vm.eval(
            r#"
            local part = Instance.new("Part")
            part.Name = "Leaked"
            part.Parent = workspace
            workspace.ChildAdded:Connect(function() _G.__fired = (_G.__fired or 0) + 1 end)
            _G.__module = Instance.new("ModuleScript")
            _G.__module.Name = "Counter"
            _G.__module.Source = "return { n = 1 }"
            _G.__module.Parent = game:GetService("ReplicatedStorage")
            require(_G.__module)
            task.delay(30, function() _G.__late = true end)
            "#,
        )
        .unwrap();
        assert_eq!(
            vm.eval("return require(game.ReplicatedStorage.Counter).n").unwrap(),
            Value::Integer(1)
        );

        vm.reset_world().unwrap();

        // fresh world: no instances, listeners or parked timers
        assert_eq!(
            vm.eval("return #workspace:GetChildren()").unwrap(),
            Value::Integer(0)
        );
        assert_eq!(
            vm.eval("return #game:GetService('ReplicatedStorage'):GetChildren()")
                .unwrap(),
            Value::Integer(0)
        );
        // workspace/game still wired up with new ids
        assert_eq!(
            vm.eval("return workspace == game:GetService('Workspace')").unwrap(),
            Value::Boolean(true)
        );

        // ids get reused; a surviving require cache would return the old module
        vm.eval(
            r#"
            local counter = Instance.new("ModuleScript")
            counter.Name = "Counter"
            counter.Source = "return { n = 2 }"
            counter.Parent = game:GetService("ReplicatedStorage")
            "#,
        )
        .unwrap();
        assert_eq!(
            vm.eval("return require(game.ReplicatedStorage.Counter).n").unwrap(),
            Value::Integer(2)
        );

        // vm intact: scripts still run
        vm.eval("local part = Instance.new('Part') part.Parent = workspace").unwrap();
        assert_eq!(
            vm.eval("return #workspace:GetChildren()").unwrap(),
            Value::Integer(1)
        );

        // scheduled work dropped with the old scheduler
        vm.advance_time(60.0).unwrap();
        assert_eq!(vm.eval("return _G.__late").unwrap(), Value::Nil);
    }

    #[test]
    fn a_test_can_make_a_player_join() {
        let vm = vm();
        vm.eval(
            r#"
            _G.__joined = nil
            game:GetService("Players").PlayerAdded:Connect(function(player)
                _G.__joined = player.Name
            end)
            local player = __microstudio_add_player("Eddie")
            assert(player.ClassName == "Player")
            assert(player.Name == "Eddie")
            assert(_G.__joined == "Eddie", "PlayerAdded must fire")
            assert(#game:GetService("Players"):GetPlayers() == 1)
            "#,
        )
        .unwrap();

        // name optional; a second join does not collide
        vm.eval(
            r#"
            local second = __microstudio_add_player()
            assert(second.Name == "Player1", second.Name)
            "#,
        )
        .unwrap();

        // players are world state; reset clears them too
        vm.reset_world().unwrap();
        assert_eq!(
            vm.eval("return #game:GetService('Players'):GetPlayers()")
                .unwrap(),
            Value::Integer(0)
        );
    }

    #[test]
    fn reset_world_keeps_the_lua_registry() {        let vm = vm();
        // test framework bookkeeping lives here, must survive a between-test reset
        vm.lua()
            .set_named_registry_value("microstudio.check", 42)
            .unwrap();

        vm.reset_world().unwrap();

        assert_eq!(
            vm.lua()
                .named_registry_value::<i64>("microstudio.check")
                .unwrap(),
            42
        );
        assert!(vm.luau_version().starts_with("Luau"));
    }

    #[test]
    fn a_chunk_that_yields_finishes_and_returns_its_value() {
        let vm = vm();
        let value = vm
            .eval("task.wait(0.5) return 'after the wait'")
            .unwrap();
        assert_eq!(
            value,
            Value::String(vm.lua().create_string("after the wait").unwrap())
        );
        // wait really parked and was driven by the clock
        assert_eq!(vm.now(), 0.5);

        // yielding inside pcall must work: test frameworks isolate failures that way
        let value = vm
            .eval("local ok, result = pcall(function() task.wait(0.25) return 7 end) return { ok, result }")
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table");
        };
        assert_eq!(table.raw_get::<Value>(1).unwrap(), Value::Boolean(true));
        assert_eq!(table.raw_get::<Value>(2).unwrap(), Value::Integer(7));
    }

    #[test]
    fn a_chunk_that_yields_on_a_signal_finishes() {
        let vm = vm();
        // parked on a signal, woken by work the same chunk scheduled
        let value = vm
            .eval(
                r#"
                local part = Instance.new("Part")
                part.Parent = workspace
                task.delay(0.1, function()
                    local child = Instance.new("Part")
                    child.Parent = part
                end)
                local child = part.ChildAdded:Wait()
                return "woken by " .. child.ClassName
                "#,
            )
            .unwrap();
        assert_eq!(
            value,
            Value::String(vm.lua().create_string("woken by Part").unwrap())
        );
    }

    #[test]
    fn runtime_errors_are_reported_with_a_location() {
        let vm = vm();
        vm.eval("local t = nil\nreturn t.Position").unwrap();
        let state = vm.ctx.state.borrow();
        assert_eq!(state.world.errors.len(), 1);
        assert!(state.world.errors[0].location.contains("MicroStudio.repl"));
    }

    #[test]
    fn mock_players_fire_player_added() {
        let vm = vm();
        vm.eval(
            r#"
            _G.__joined = {}
            game:GetService("Players").PlayerAdded:Connect(function(player)
                table.insert(_G.__joined, player.Name)
            end)
            "#,
        )
        .unwrap();

        let events = {
            let mut state = vm.ctx.state.borrow_mut();
            let (_, events) = state
                .world
                .add_mock_player("Eddie", microstudio_services::PlayerOptions::default())
                .unwrap();
            events
        };
        vm.dispatch(events).unwrap();

        assert_eq!(
            vm.eval("return _G.__joined[1]").unwrap(),
            Value::String(vm.lua().create_string("Eddie").unwrap())
        );
        assert_eq!(
            vm.eval("return game:GetService('Players'):GetPlayers()[1].Name").unwrap(),
            Value::String(vm.lua().create_string("Eddie").unwrap())
        );
    }

    #[test]
    fn collection_service_accepts_both_tag_call_shapes() {
        let vm = vm();
        // roblox allows both AddTag arities
        vm.eval(
            r#"
            local cs = game:GetService("CollectionService")
            local part = Instance.new("Part") part.Name = "Checkpoint"
            part.Parent = workspace
            _G.__seen = {}
            cs:GetInstanceAddedSignal("cp"):Connect(function(instance)
                table.insert(_G.__seen, instance.Name)
            end)
            cs:AddTag(part, "cp")
            local other = Instance.new("Part") other.Name = "Other"
            other.Parent = workspace
            other:AddTag("cp")
            "#,
        )
        .unwrap();

        assert_eq!(vm.eval("return #_G.__seen").unwrap(), Value::Integer(2));
        assert_eq!(
            vm.eval("return _G.__seen[1]").unwrap(),
            Value::String(vm.lua().create_string("Checkpoint").unwrap())
        );
        assert_eq!(
            vm.eval("return #game:GetService('CollectionService'):GetTagged('cp')").unwrap(),
            Value::Integer(2)
        );
        assert_eq!(
            vm.eval("return workspace.Checkpoint:HasTag('cp')").unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            vm.eval("return game:GetService('CollectionService'):HasTag(workspace.Other, 'cp')")
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            vm.eval("return workspace.Checkpoint:GetTags()[1]").unwrap(),
            Value::String(vm.lua().create_string("cp").unwrap())
        );
        assert_eq!(
            vm.eval("return #game:GetService('CollectionService'):GetTags(workspace.Checkpoint)")
                .unwrap(),
            Value::Integer(1)
        );
        assert_eq!(
            vm.eval(
                "local cs = game:GetService('CollectionService')
                 cs:RemoveTag(workspace.Checkpoint, 'cp')
                 return workspace.Checkpoint:HasTag('cp')"
            )
            .unwrap(),
            Value::Boolean(false)
        );
    }
}

