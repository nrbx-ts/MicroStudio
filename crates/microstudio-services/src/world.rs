use microstudio_datamodel::{DataModel, InstanceId};

use crate::collection::CollectionState;
use crate::memory::MemoryStoreState;
use crate::players::PlayersState;
use crate::run_service::RunServiceState;
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Print,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            LogLevel::Print => "print",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub level: LogLevel,
    pub text: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    pub location: String,
    pub message: String,
    pub traceback: String,
}

impl ScriptError {
    pub fn render(&self) -> String {
        let mut out = format!("[server] {}\nError: {}\nStack:", self.location, self.message);
        for line in self.traceback.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            out.push_str("\n  ");
            out.push_str(line);
        }
        out
    }
}

// datamodel owns the tree; service states hold state not on an instance
pub struct World {
    pub dm: DataModel,
    pub players: PlayersState,
    pub collection: CollectionState,
    pub run_service: RunServiceState,
    // where simulated services keep their data; ephemeral unless the runtime said otherwise
    pub store: Store,
    // MemoryStoreService data, session scoped on purpose
    pub memory: MemoryStoreState,
    pub logs: Vec<LogEntry>,
    pub errors: Vec<ScriptError>,
    // mirrored from scheduler so logs can be stamped without touching the VM
    pub time: f64,
    pub streaming: bool,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self::with_store(Store::ephemeral())
    }

    pub fn with_store(store: Store) -> Self {
        Self {
            dm: DataModel::new(),
            players: PlayersState::default(),
            collection: CollectionState::default(),
            run_service: RunServiceState::default(),
            store,
            memory: MemoryStoreState::default(),
            logs: Vec::new(),
            errors: Vec::new(),
            time: 0.0,
            streaming: false,
        }
    }

    pub fn get_service(&mut self, name: &str) -> Result<InstanceId, microstudio_datamodel::DataModelError> {
        self.dm.get_service(name)
    }

    // eager creation so service ids are stable from the first script
    pub fn bootstrap(&mut self) -> Result<(), microstudio_datamodel::DataModelError> {
        self.dm.bootstrap_default_services()?;
        Ok(())
    }

    pub fn log(&mut self, level: LogLevel, text: impl Into<String>, source: Option<String>) {
        self.logs.push(LogEntry {
            level,
            text: text.into(),
            source,
        });
    }

    pub fn print(&mut self, text: impl Into<String>, source: Option<String>) {
        self.log(LogLevel::Print, text, source);
    }

    pub fn record_error(&mut self, error: ScriptError) {
        self.log(
            LogLevel::Error,
            error.render(),
            Some(error.location.clone()),
        );
        self.errors.push(error);
    }

    pub fn take_logs(&mut self) -> Vec<LogEntry> {
        std::mem::take(&mut self.logs)
    }

    pub fn take_errors(&mut self) -> Vec<ScriptError> {
        std::mem::take(&mut self.errors)
    }

    pub fn game(&self) -> InstanceId {
        self.dm.game()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_materialises_the_v1_services() {
        let mut world = World::new();
        world.bootstrap().unwrap();
        for name in microstudio_datamodel::DEFAULT_SERVICES {
            assert!(world.dm.has_service(name), "{name} was not created");
        }
    }

    #[test]
    fn logs_are_ordered_and_drainable() {
        let mut world = World::new();
        world.print("one", None);
        world.print("two", None);
        let logs = world.take_logs();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].text, "one");
        assert!(world.take_logs().is_empty());
    }

    #[test]
    fn errors_render_in_the_documented_shape() {
        let error = ScriptError {
            location: "ServerScriptService.Server:42".into(),
            message: "attempt to index nil with 'Position'".into(),
            traceback: "ServerScriptService.Server:42\nTrainController:18".into(),
        };
        let rendered = error.render();
        assert!(rendered.starts_with("[server] ServerScriptService.Server:42\nError: attempt to index nil with 'Position'\nStack:"));
        assert!(rendered.contains("\n  TrainController:18"));
    }
}
