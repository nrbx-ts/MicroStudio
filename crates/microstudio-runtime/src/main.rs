// stdio json-rpc sidecar spawned by the typescript driver

use std::io::{self, BufReader, Write};

use microstudio_luau::ClockKind;
use microstudio_runtime::{Runtime, RuntimeOptions, Server};

const HELP: &str = "\
microstudio-runtime -- headless Roblox runtime sidecar

USAGE:
    microstudio-runtime [OPTIONS]

OPTIONS:
    --clock <virtual|real>   Time source. `virtual` (default) is deterministic and
                             only advances via the advanceTime method; `real` uses
                             wall time so `task.wait` actually waits.
    --state-dir <path>       Where simulated services keep their json. Defaults to
                             $MICROSTUDIO_STATE_DIR, then <project>/.microstudio when
                             $MICROSTUDIO_PROJECT_DIR is set. With neither, service
                             data stays in memory and nothing is written.
    --headless               Report RunService:IsStudio() == false.
    --help                   Print this message.
    --version                Print the version.

The process speaks newline-delimited JSON-RPC on stdin/stdout. Diagnostics and
warnings go to stderr; stdout carries protocol messages only.
";

fn main() -> std::io::Result<()> {
    let mut clock = ClockKind::Virtual;
    let mut run_service = microstudio_services::RunServiceState::dev();
    let mut state_dir: Option<std::ffi::OsString> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--clock" => match args.next().as_deref() {
                Some("real") => clock = ClockKind::Real,
                Some("virtual") => clock = ClockKind::Virtual,
                other => {
                    eprintln!("error: --clock expects 'virtual' or 'real', got {other:?}");
                    std::process::exit(2);
                }
            },
            "--state-dir" => match args.next() {
                Some(dir) => state_dir = Some(dir.into()),
                None => {
                    eprintln!("error: --state-dir expects a path\n\n{HELP}");
                    std::process::exit(2);
                }
            },
            "--headless" => {
                clock = ClockKind::Virtual;
                run_service = microstudio_services::RunServiceState::headless();
            }
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("microstudio-runtime {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => {
                eprintln!("error: unknown argument {other:?}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }

    // the flag wins, then MICROSTUDIO_STATE_DIR, then <project>/.microstudio, else memory only
    let store = match state_dir {
        Some(dir) => microstudio_services::Store::resolve(Some(dir), None),
        None => microstudio_services::Store::from_env(),
    };

    let runtime = Runtime::new(RuntimeOptions {
        clock,
        run_service,
        store,
        ..RuntimeOptions::default()
    })
    .map_err(|error| io::Error::other(error.to_string()))?;

    let mut server = Server::new(runtime);
    let stdin = io::stdin();
    let stdout = io::stdout();
    server.serve(BufReader::new(stdin.lock()), stdout.lock())?;
    stdout.lock().flush()?;
    Ok(())
}
