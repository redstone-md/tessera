#![forbid(unsafe_code)]

mod inspection;

use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::process::ExitCode;

use tessera_core::{MainStack, Rect, Window, WindowId, WindowMode};

const HELP: &str = "Tessera — desktop environment foundation

Usage: tessera demo
       tessera inspect
       tessera --help

demo     Calculate a layout for synthetic windows.
inspect  Read real Windows monitors and visible windows (Windows only).

Neither command moves windows or replaces Explorer.";

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let result = match arguments.as_slice() {
        [] => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        [command] if command == "--help" || command == "-h" => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        [command] if command == "demo" => demo(io::stdout().lock()),
        [command] if command == "inspect" => inspection::inspect(io::stdout().lock()),
        _ => {
            eprintln!("tessera: unsupported arguments\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tessera: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Exercises the real domain interface without any desktop side effects.
fn demo(mut output: impl Write) -> Result<(), Box<dyn Error>> {
    let area = Rect::new(-1920, 0, 1920, 1040)?;
    let windows = [
        Window::new(WindowId::new(1), WindowMode::Tiled),
        Window::new(WindowId::new(2), WindowMode::Floating),
        Window::new(WindowId::new(3), WindowMode::Tiled),
        Window::new(WindowId::new(4), WindowMode::Fullscreen),
        Window::new(WindowId::new(5), WindowMode::Tiled),
    ];
    let placements = MainStack::default().arrange(area, &windows)?;

    writeln!(
        output,
        "Tessera layout demo (synthetic windows; no desktop changes)"
    )?;
    writeln!(
        output,
        "Work area: x={} y={} width={} height={}",
        area.x(),
        area.y(),
        area.width(),
        area.height()
    )?;
    writeln!(output, "Floating and fullscreen windows are excluded.")?;
    for placement in placements {
        let rect = placement.rect();
        writeln!(
            output,
            "window {}: x={} y={} width={} height={}",
            placement.window_id().value(),
            rect.x(),
            rect.y(),
            rect.width(),
            rect.height()
        )?;
    }
    Ok(())
}
