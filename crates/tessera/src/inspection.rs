use std::error::Error;
use std::io::{self, Write};

use tessera_core::Rect;

/// Reads the desktop before emitting output; a fatal observation error never
/// masquerades as a successful empty snapshot.
pub fn inspect(mut output: impl Write) -> Result<(), Box<dyn Error>> {
    let snapshot = tessera_windows::observe()?;
    writeln!(
        output,
        "Tessera desktop snapshot (read-only; physical pixels)"
    )?;
    writeln!(output, "Monitors: {}", snapshot.monitors().len())?;
    for monitor in snapshot.monitors() {
        writeln!(
            output,
            "monitor 0x{:X} primary={} device={:?}",
            monitor.id().value(),
            monitor.primary(),
            monitor.device_name()
        )?;
        write_rect(&mut output, "bounds", monitor.bounds())?;
        write_rect(&mut output, "work-area", monitor.work_area())?;
    }

    writeln!(output, "Windows: {}", snapshot.windows().len())?;
    for window in snapshot.windows() {
        let monitor = window
            .monitor_id()
            .map_or_else(|| "none".to_owned(), |id| format!("0x{:X}", id.value()));
        let cloaked = match window.cloaked() {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        };
        writeln!(
            output,
            "window 0x{:X} pid={} monitor={monitor}",
            window.id().value(),
            window.process_id()
        )?;
        writeln!(
            output,
            "  minimized={} maximized={} cloaked={cloaked} tool={} owned={} covers-monitor={}",
            window.minimized(),
            window.maximized(),
            window.tool_window(),
            window.owned(),
            window.covers_monitor()
        )?;
        // Debug formatting quotes and escapes captions/classes supplied by
        // other processes, including newlines and terminal escape sequences.
        writeln!(output, "  title: {:?}", window.title())?;
        writeln!(output, "  class: {:?}", window.class_name())?;
        write_rect(&mut output, "bounds", window.bounds())?;
    }

    writeln!(output, "Warnings: {}", snapshot.warnings().len())?;
    for warning in snapshot.warnings() {
        writeln!(
            output,
            "warning window=0x{:X} operation={} code=0x{:08X}",
            warning.window_id().value(),
            warning.operation(),
            warning.code()
        )?;
    }
    Ok(())
}

fn write_rect(output: &mut impl Write, label: &str, rect: Rect) -> io::Result<()> {
    writeln!(
        output,
        "  {label}: x={} y={} width={} height={}",
        rect.x(),
        rect.y(),
        rect.width(),
        rect.height()
    )
}
