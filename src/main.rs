use std::{process::ExitCode, sync::Arc};

use ferrocord::{Context, EventBus};

#[derive(Clone, Debug, PartialEq)]
struct DiagnosticEvent;

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
            println!("Usage: ferrocore check");
            ExitCode::SUCCESS
        }
        [arg] if arg == "--help" => {
            println!("Usage: ferrocore check");
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
        _ => {
            eprintln!("Usage: ferrocore check");
            ExitCode::from(2)
        }
    }
}
