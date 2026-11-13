mod config;
mod run;

use std::{process::ExitCode, sync::Arc};

use ferrocord::{Context, EventBus};

#[derive(Clone, Debug, PartialEq)]
struct DiagnosticEvent;

const USAGE: &str = "Usage: ferrocore check | ferrocore run --config <file> [--] <task> | ferrocore run --config -- <file> <task>";

fn check() -> bool {
    let context = Context::new();
    context.insert(Arc::new(EventBus::new()));
    let Some(bus) = context.get::<EventBus>() else {
        return false;
    };
    let mut receiver = bus.subscribe::<DiagnosticEvent>();
    bus.publish(DiagnosticEvent) == 1 && receiver.try_recv() == Ok(DiagnosticEvent)
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        [arg] if arg == "--help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        [arg] if arg == "check" => {
            if check() {
                println!("ferrocord integration: ok");
                ExitCode::SUCCESS
            } else {
                eprintln!("ferrocord integration: failed");
                ExitCode::FAILURE
            }
        }
        [command, option, rest @ ..] if command == "run" && option == "--config" => {
            let values = match rest {
                [path, task]
                    if !path.to_string_lossy().starts_with('-')
                        && !task.to_string_lossy().starts_with('-') => Some((path, task)),
                [path, end, task]
                    if end == "--" && !path.to_string_lossy().starts_with('-') => Some((path, task)),
                [end, path, task] if end == "--" => Some((path, task)),
                _ => None,
            };
            let Some((path, task)) = values.filter(|(path, task)| !path.is_empty() && !task.is_empty()) else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            match config::Config::load(std::path::Path::new(path)) {
                Ok(config) => {
                    let Some(task) = task.to_str() else {
                        eprintln!("task must be valid UTF-8");
                        return ExitCode::FAILURE;
                    };
                    let runtime = match tokio::runtime::Runtime::new() {
                        Ok(runtime) => runtime,
                        Err(_) => { eprintln!("runtime initialization failed"); return ExitCode::FAILURE; }
                    };
                    match runtime.block_on(run::run(config, task.to_owned())) {
                        Ok(answer) => { println!("{answer}"); ExitCode::SUCCESS }
                        Err(error) => { eprintln!("run failed: {error}"); ExitCode::FAILURE }
                    }
                }
                Err(error) => {
                    eprintln!("configuration error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
