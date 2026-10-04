// stdout is json lines only, anything else breaks the driver

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::protocol::{
    AddMockPlayerParams, AdvanceTimeParams, EvalParams, LoadTreeParams, PathParams, Request,
    RpcError, RunScriptParams, SetClockParams, TreeParams, PROTOCOL_VERSION,
};
use crate::runtime::Runtime;
use microstudio_luau::ClockKind;
use microstudio_luau::vm::EvalMode;

pub struct Server {
    runtime: Runtime,
}

impl Server {
    pub fn new(runtime: Runtime) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        let mut shutdown = false;
        for line in input.lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    eprintln!("[microstudio-runtime] stdin error: {error}");
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<Request>(&line) {
                Ok(request) => {
                    let id = request.id;
                    shutdown = request.method == "shutdown";
                    match self.handle(&request) {
                        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
                        Err(error) => json!({ "id": id, "ok": false, "error": error }),
                    }
                    .to_string()
                }
                Err(error) => json!({
                    "id": 0,
                    "ok": false,
                    "error": { "message": format!("malformed request: {error}") },
                })
                .to_string(),
            };

            // flush output before the response, or callers race the notification
            self.flush_output(&mut output)?;
            writeln!(output, "{response}")?;
            output.flush()?;

            if shutdown {
                break;
            }
        }
        Ok(())
    }

    fn handle(&mut self, request: &Request) -> Result<Value, RpcError> {
        match request.method.as_str() {
            "hello" => Ok(self.hello()),
            "eval" => {
                let params: EvalParams = parse(&request.params)?;
                let value = self
                    .runtime
                    .eval_named(
                        &params.code,
                        EvalMode::parse(params.mode.as_deref()),
                        params.name.as_deref().unwrap_or("MicroStudio.repl"),
                    )
                    .map_err(|error| to_rpc_error(error.to_string()))?;
                Ok(json!({ "value": value }))
            }
            "drain" => {
                self.runtime
                    .drain()
                    .map_err(|error| to_rpc_error(error.to_string()))?;
                Ok(json!({ "time": self.runtime.now() }))
            }
            "resetWorld" => {
                self.runtime
                    .reset_world()
                    .map_err(|error| to_rpc_error(error.to_string()))?;
                Ok(json!({ "time": self.runtime.now() }))
            }
            "runScripts" => {
                let params: RunScriptParams = parse(&request.params)?;
                self.run_scripts(params)
            }
            "advanceTime" => {
                let params: AdvanceTimeParams = parse(&request.params)?;
                self.runtime
                    .advance_time(params.seconds)
                    .map_err(|error| to_rpc_error(error.to_string()))?;
                Ok(json!({ "time": self.runtime.now() }))
            }
            "pump" => {
                self.runtime
                    .pump()
                    .map_err(|error| to_rpc_error(error.to_string()))?;
                Ok(json!({ "time": self.runtime.now() }))
            }
            "addMockPlayer" => {
                let params: AddMockPlayerParams = parse(&request.params)?;
                let player = self
                    .runtime
                    .add_mock_player(&params.name, params.with_character, params.user_id)
                    .map_err(|error| to_rpc_error(error.to_string()))?;
                Ok(json!({ "player": self.runtime.instance_json(player) }))
            }
            "getTree" => {
                let params: TreeParams = parse(&request.params)?;
                Ok(json!({
                    "root": self.runtime.tree(params.path.as_deref(), params.depth),
                    "time": self.runtime.now(),
                }))
            }
            "inspect" => {
                let params: PathParams = parse(&request.params)?;
                match self.runtime.find_by_path(&params.path) {
                    Some(id) => Ok(json!({ "instance": self.runtime.instance_json(id) })),
                    None => Err(to_rpc_error(format!("'{}' was not found", params.path))),
                }
            }
            "listServices" => {
                let services: Vec<&str> = microstudio_datamodel::api::service_names();
                Ok(json!({ "services": services }))
            }
            "stats" => Ok(self.runtime.stats()),
            "loadTree" => {
                let params: LoadTreeParams = parse(&request.params)?;
                let created = self
                    .runtime
                    .load_tree(&params.entries)
                    .map_err(|error| to_rpc_error(error.to_string()))?;
                Ok(json!({ "created": created }))
            }
            "setClock" => {
                let params: SetClockParams = parse(&request.params)?;
                match params.kind.as_str() {
                    "virtual" => self.runtime.set_clock(ClockKind::Virtual),
                    "real" => self.runtime.set_clock(ClockKind::Real),
                    other => {
                        return Err(to_rpc_error(format!(
                            "unknown clock '{other}' (expected 'virtual' or 'real')"
                        )))
                    }
                }
                Ok(json!({ "clock": self.runtime.clock_kind_of() }))
            }
            "shutdown" => Ok(json!({ "bye": true })),
            other => Err(to_rpc_error(format!("unknown method '{other}'"))),
        }
    }

    fn hello(&self) -> Value {
        json!({
            "protocol": PROTOCOL_VERSION,
            "clock": self.runtime.clock_kind_of(),
            "luau": self.runtime.luau_version(),
            "services": microstudio_datamodel::api::service_names(),
        })
    }

    fn run_scripts(&mut self, params: RunScriptParams) -> Result<Value, RpcError> {
        if let Some(source) = params.source {
            let name = params.name.unwrap_or_else(|| "MicroStudioSource".to_string());
            let parent = match params.parent.as_deref() {
                Some(path) => Some(
                    self.runtime
                        .find_by_path(path)
                        .ok_or_else(|| to_rpc_error(format!("'{path}' was not found")))?,
                ),
                None => self.runtime.find_by_path("game.ServerScriptService"),
            };
            self.runtime
                .run_source(&name, &source, parent)
                .map_err(|error| to_rpc_error(error.to_string()))?;
            return Ok(json!({ "scripts": [name] }));
        }

        if let Some(path) = params.path.as_deref() {
            let id = self
                .runtime
                .find_by_path(path)
                .ok_or_else(|| to_rpc_error(format!("'{path}' was not found")))?;
            let name = {
                let state = self.runtime.vm().ctx().state.borrow();
                state.world.dm.path(id, false)
            };
            self.runtime
                .vm()
                .run_script(id)
                .map_err(|error| to_rpc_error(error.to_string()))?;
            return Ok(json!({ "scripts": [name] }));
        }

        let scripts = self
            .runtime
            .run_server_scripts()
            .map_err(|error| to_rpc_error(error.to_string()))?;
        let names: Vec<String> = {
            let state = self.runtime.vm().ctx().state.borrow();
            scripts
                .iter()
                .map(|id| state.world.dm.path(*id, false))
                .collect()
        };
        Ok(json!({ "scripts": names }))
    }

    fn flush_output(&mut self, output: &mut impl Write) -> std::io::Result<()> {
        for warning in self.runtime.take_warnings() {
            writeln!(
                output,
                "{}",
                json!({ "method": "warn", "params": { "message": warning } })
            )?;
        }
        for log in self.runtime.take_logs() {
            writeln!(
                output,
                "{}",
                json!({
                    "method": "log",
                    "params": { "level": log.level.as_str(), "text": log.text, "source": log.source },
                })
            )?;
        }
        for error in self.runtime.take_errors() {
            writeln!(
                output,
                "{}",
                json!({
                    "method": "error",
                    "params": {
                        "location": error.location,
                        "message": error.message,
                        "traceback": error.traceback,
                        "rendered": error.render(),
                    },
                })
            )?;
        }
        Ok(())
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, RpcError> {
    serde_json::from_value(params.clone()).map_err(|error| to_rpc_error(error.to_string()))
}

fn to_rpc_error(message: String) -> RpcError {
    RpcError {
        message,
        location: None,
        traceback: None,
    }
}
