// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::error::Error;
use std::io::{self, Write};

use tessera_core::Rect;
use tessera_windows::diagnose_window;

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

/// Surface kinds this diagnostic knows how to match, with the exact window
/// titles read from the current Slint sources (ui/toolbar.slint:16,
/// ui/dock.slint:21, ui/launcher.slint:24). A caption match is only a
/// diagnostic heuristic for narrowing output — it is never ownership or
/// security authorization, and no matched window is ever mutated.
pub const SURFACE_TITLES: [(&str, &str); 3] = [
    ("toolbar", "Tessera toolbar"),
    ("dock", "Tessera dock"),
    ("launcher", "Tessera applications"),
];

/// One matched surface row plus the diagnostic outcome for its window.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfaceRow {
    kind: &'static str,
    pid: u32,
    window_id: u64,
    monitor_id: Option<u64>,
    bounds: Rect,
    title: &'static str,
    class_name: String,
    dpi: Option<u32>,
    style: Option<u32>,
    ex_style: Option<u32>,
    show_command: Option<u32>,
    /// `Some(reason)` when the per-window diagnosis was unavailable or
    /// incomplete; `None` only for a fully diagnosed, fresh row.
    unavailable: Option<String>,
}

/// Collects the observed monitor/window facts needed for surface matching.
#[derive(Debug, Clone)]
pub struct SurfaceScan {
    monitors: Vec<(u64, Rect, Rect)>,
    rows: Vec<SurfaceRow>,
    warnings: Vec<(u64, &'static str, u32)>,
}

/// Captures one read-only observation and enriches matched surface windows
/// with native DPI/styles/placement. A window that vanished between the
/// snapshot and its per-window diagnostics is reported as `stale` rather than
/// guessed at; that makes the CLI result red (stale) instead of vacuous.
/// The observation error stays typed; only the CLI layer formats it.
pub fn scan_surfaces() -> Result<SurfaceScan, tessera_windows::ObservationError> {
    let snapshot = tessera_windows::observe()?;
    let monitors = snapshot
        .monitors()
        .iter()
        .map(|m| (m.id().value(), m.bounds(), m.work_area()))
        .collect();
    let warnings = snapshot
        .warnings()
        .iter()
        .map(|w| (w.window_id().value(), w.operation(), w.code()))
        .collect();

    let mut rows = Vec::new();
    for (kind, title) in SURFACE_TITLES {
        for window in snapshot.windows().iter().filter(|w| w.title() == title) {
            let diagnosed = diagnose_window(window.id(), window.process_id()).ok();
            let (dpi, style, ex_style, show_command) = match diagnosed {
                Some(d) => (d.dpi(), d.style(), d.ex_style(), d.show_command()),
                None => (None, None, None, None),
            };
            // A partially available diagnosis is still incomplete: a fresh
            // report requires every field to have been read successfully.
            let unavailable = match &diagnosed {
                None => Some("window vanished or pid changed".to_owned()),
                Some(_) if dpi.is_none() => Some("DPI unavailable".to_owned()),
                Some(_) if style.is_none() => Some("style unavailable".to_owned()),
                Some(_) if ex_style.is_none() => Some("ex-style unavailable".to_owned()),
                Some(_) if show_command.is_none() => Some("placement unavailable".to_owned()),
                _ => None,
            };
            rows.push(SurfaceRow {
                kind,
                pid: window.process_id(),
                window_id: window.id().value(),
                monitor_id: window.monitor_id().map(|m| m.value()),
                bounds: window.bounds(),
                title,
                class_name: window.class_name().to_owned(),
                dpi,
                style,
                ex_style,
                show_command,
                unavailable,
            });
        }
    }
    Ok(SurfaceScan {
        monitors,
        rows,
        warnings,
    })
}

/// Why an expected toolbar rectangle could not be computed from real inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContractError {
    /// DPI 0 (or yielding a zero-height bar) cannot define a real contract.
    InvalidDpi,
    /// The contract rectangle does not fit `Rect`'s extent bounds.
    Unrepresentable,
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDpi => write!(f, "invalid DPI (0) cannot define the toolbar contract"),
            Self::Unrepresentable => write!(f, "expected toolbar rect is unrepresentable"),
        }
    }
}

impl Error for ContractError {}

/// Contract result for one toolbar: which dimension diverged and by how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolbarMismatch {
    pub expected_top: i32,
    pub actual_top: i32,
    pub expected_height: u32,
    pub actual_height: u32,
    pub expected_width: u32,
    pub actual_width: u32,
    pub expected_left: i32,
    pub actual_left: i32,
}

impl ToolbarMismatch {
    pub fn deltas(&self) -> [(String, i64); 4] {
        [
            (
                "top".into(),
                i64::from(self.actual_top) - i64::from(self.expected_top),
            ),
            (
                "height".into(),
                i64::from(self.actual_height) - i64::from(self.expected_height),
            ),
            (
                "width".into(),
                i64::from(self.actual_width) - i64::from(self.expected_width),
            ),
            (
                "left".into(),
                i64::from(self.actual_left) - i64::from(self.expected_left),
            ),
        ]
    }

    /// Only the diverging axes, in fixed order (top, height, width, left).
    pub fn divergences(&self) -> Vec<(String, i64)> {
        self.deltas()
            .into_iter()
            .filter(|(_, delta)| *delta != 0)
            .collect()
    }
}

/// Pure contract check: the Tessera toolbar sits at the **monitor bounds top
/// edge** (never the work-area top), spans the full monitor width, and is
/// `round(TOOLBAR_HEIGHT_LOGICAL * native DPI / 96)` physical pixels tall.
///
/// Invalid input (DPI 0, or an extent unrepresentable as a `Rect`) is an
/// error — never fabricated from a fallback or a panic.
pub fn expected_toolbar_rect(monitor_bounds: Rect, dpi: u32) -> Result<Rect, ContractError> {
    if dpi == 0 {
        return Err(ContractError::InvalidDpi);
    }
    let scale = f64::from(dpi) / 96.0;
    let height = (f64::from(tessera_core::TOOLBAR_HEIGHT_LOGICAL) * scale).round() as u32;
    if height == 0 {
        return Err(ContractError::InvalidDpi);
    }
    Rect::new(
        monitor_bounds.x(),
        monitor_bounds.y(),
        monitor_bounds.width(),
        height,
    )
    .map_err(|_| ContractError::Unrepresentable)
}

pub fn check_toolbar(
    actual: Rect,
    monitor_bounds: Rect,
    dpi: u32,
) -> Result<Option<ToolbarMismatch>, ContractError> {
    let expected = expected_toolbar_rect(monitor_bounds, dpi)?;
    if actual == expected {
        return Ok(None);
    }
    Ok(Some(ToolbarMismatch {
        expected_top: expected.y(),
        actual_top: actual.y(),
        expected_height: expected.height(),
        actual_height: actual.height(),
        expected_width: expected.width(),
        actual_width: actual.width(),
        expected_left: expected.x(),
        actual_left: actual.x(),
    }))
}

/// Prints the explicit `inspect --check-surfaces` report. Returns `Err` when
/// the toolbar contract is violated, the diagnostic evidence is stale, or no
/// toolbar was observed: success requires a real, freshly diagnosed toolbar
/// that matches the contract.
pub fn check_surfaces(mut output: impl Write) -> Result<(), CheckError> {
    let scan = scan_surfaces().map_err(CheckError::Observation)?;
    writeln!(output, "Tessera surface check (read-only; physical pixels)")?;
    for (id, bounds, work_area) in &scan.monitors {
        writeln!(output, "monitor 0x{id}")?;
        write_rect(&mut output, "bounds", *bounds)?;
        write_rect(&mut output, "work-area", *work_area)?;
    }
    for (id, operation, code) in &scan.warnings {
        writeln!(
            output,
            "warning window=0x{id:X} operation={operation} code=0x{code:08X}"
        )?;
    }
    writeln!(output, "Surfaces: {}", scan.rows.len())?;
    for row in &scan.rows {
        let monitor = row
            .monitor_id
            .map_or_else(|| "none".to_owned(), |id| format!("0x{id:X}"));
        writeln!(
            output,
            "surface kind={} pid={} window=0x{:X} monitor={monitor}",
            row.kind, row.pid, row.window_id
        )?;
        if let Some(reason) = &row.unavailable {
            writeln!(output, "  diagnostic unavailable: {reason}")?;
        }
        // Class names come from other processes; Debug keeps them inert.
        writeln!(
            output,
            "  title: {:?} class: {:?}",
            row.title, row.class_name
        )?;
        write_rect(&mut output, "bounds", row.bounds)?;
        writeln!(
            output,
            "  dpi={:?} style={:?} ex-style={:?} show-cmd={:?}",
            row.dpi, row.style, row.ex_style, row.show_command
        )?;
    }

    let toolbar = verify_toolbar(&scan)?;
    let dpi = toolbar
        .dpi
        .expect("verify_toolbar requires complete diagnostics");
    let &(_, bounds, _) = scan
        .monitors
        .iter()
        .find(|(id, _, _)| Some(*id) == toolbar.monitor_id)
        .expect("verify_toolbar requires the matched monitor");
    let verdict = check_toolbar(toolbar.bounds, bounds, dpi)
        .expect("verify_toolbar rejects invalid DPI before this point");
    match verdict {
        None => {
            writeln!(
                output,
                "toolbar contract PASS: monitor-bounds top, full width, round({} * dpi/96) height",
                tessera_core::TOOLBAR_HEIGHT_LOGICAL
            )?;
            Ok(())
        }
        Some(mismatch) => {
            writeln!(output, "toolbar contract FAIL")?;
            writeln!(
                output,
                "  expected: x={} y={} width={} height={} (dpi={dpi})",
                mismatch.expected_left,
                mismatch.expected_top,
                mismatch.expected_width,
                mismatch.expected_height
            )?;
            writeln!(
                output,
                "  actual:   x={} y={} width={} height={}",
                mismatch.actual_left,
                mismatch.actual_top,
                mismatch.actual_width,
                mismatch.actual_height
            )?;
            let deltas = mismatch
                .divergences()
                .into_iter()
                .map(|(axis, delta)| format!("{axis} delta {delta:+}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(output, "  delta: {deltas}")?;
            writeln!(
                output,
                "  note: native DPI/styles/placement captured; Slint scale_factor and AppBar \
                 (SHAppBarMessage) answers are NOT observable through this snapshot and remain \
                 unverified evidence."
            )?;
            Err(CheckError::GeometryMismatch(Box::new(mismatch)))
        }
    }
}

/// Shared verification: exactly one fresh toolbar row whose monitor is known.
/// Used by `check_surfaces` and the portable unit tests.
fn verify_toolbar(scan: &SurfaceScan) -> Result<&SurfaceRow, CheckError> {
    let toolbars: Vec<&SurfaceRow> = scan
        .rows
        .iter()
        .filter(|row| row.kind == "toolbar")
        .collect();
    match toolbars.len() {
        0 => return Err(CheckError::NoToolbar),
        n if n > 1 => return Err(CheckError::AmbiguousToolbar(n)),
        _ => {}
    }
    let toolbar = toolbars[0];
    if let Some(reason) = &toolbar.unavailable {
        return Err(CheckError::Incomplete {
            window_id: toolbar.window_id,
            reason: reason.clone(),
        });
    }
    Ok(toolbar)
}

/// Failure modes of `inspect --check-surfaces`; each maps to a red CLI exit.
#[derive(Debug)]
#[non_exhaustive]
pub enum CheckError {
    NoToolbar,
    AmbiguousToolbar(usize),
    /// The toolbar window was observed but its per-window diagnosis is
    /// missing or incomplete (vanished, pid changed, or a query failed).
    Incomplete {
        window_id: u64,
        reason: String,
    },
    GeometryMismatch(Box<ToolbarMismatch>),
    /// Desktop observation itself failed (typed; includes
    /// `UnsupportedPlatform`).
    Observation(tessera_windows::ObservationError),
    /// Writing the report failed.
    Io(std::io::Error),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoToolbar => write!(f, "no Tessera toolbar window was observed"),
            Self::AmbiguousToolbar(count) => {
                write!(
                    f,
                    "{count} windows matched the toolbar caption; refusing to guess"
                )
            }
            Self::Incomplete { window_id, reason } => write!(
                f,
                "diagnostic for window 0x{window_id:X} is incomplete/stale: {reason}"
            ),
            Self::GeometryMismatch(mismatch) => write!(
                f,
                "toolbar geometry violates the monitor-origin/logical-height contract: {:?}",
                mismatch.divergences()
            ),
            Self::Observation(error) => write!(f, "desktop observation failed: {error}"),
            Self::Io(error) => write!(f, "report write failed: {error}"),
        }
    }
}

impl Error for CheckError {}

impl From<io::Error> for CheckError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
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

#[cfg(test)]
mod tests;
