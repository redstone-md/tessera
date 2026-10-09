// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Passive physical-pointer hints and monitor-local bar visibility policy.
//! No native identity, registration options, activation or keyboard interception
//! crosses this interface. Callers supply already-authorized window candidates.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tessera_core::Rect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Closed, redacted native operation identifiers: no arbitrary provider text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerNativeOperation {
    CreateStopEvent,
    SignalStopEvent,
    CloseStopEvent,
    StartThread,
    InstallHook,
    RemoveHook,
    MessagePump,
    ReadCursor,
    SetDpiContext,
    RestoreDpiContext,
    Callback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerWatchError {
    Unsupported,
    Busy,
    Unavailable,
    Stopped,
    Native {
        operation: PointerNativeOperation,
        code: u32,
    },
}

impl fmt::Display for PointerWatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("Passive pointer watching is unsupported"),
            Self::Busy => formatter.write_str("A passive pointer watch is already active"),
            Self::Unavailable => formatter.write_str("Passive pointer watching is unavailable"),
            Self::Stopped => formatter.write_str("Passive pointer watching has stopped"),
            Self::Native { operation, code } => write!(
                formatter,
                "Passive pointer operation {operation:?} failed (native code 0x{code:08x})"
            ),
        }
    }
}

impl std::error::Error for PointerWatchError {}

/// Read-only startup capability snapshot, not a primary/per-monitor touch fact.
/// Any confirmed touch device conservatively disables autohide everywhere.
/// Absence requires a successful native device query, never an ambiguous zero.
/// Device capability changes are not subscribed: a fresh watch is needed for a
/// new snapshot. This is explicitly not dynamic source touch-primary parity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PointerEnvironment {
    pub touch_capable: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerWatchEvent {
    Position(PhysicalPoint),
    Environment(PointerEnvironment),
    Failed(PointerWatchError),
}

/// Drop closes admission promptly and idempotently; it must never join a worker.
/// An already admitted callback may finish after retirement. Consumers must be
/// cheap enqueue-only and independently reject stale generations.
pub trait PointerWatchGuard: Send + 'static {}

pub type PointerWatchCompletion = Box<dyn FnOnce(Result<(), PointerWatchError>) + Send + 'static>;
pub type PointerWatchCallback = Arc<dyn Fn(PointerWatchEvent) + Send + Sync + 'static>;

/// Construction is inert. `Ok` accepts exactly one readiness completion, not a
/// successful registration. Immediate `Err` accepts no callbacks. Readiness and
/// positions are distinct: failure never invents a point or successful readiness.
/// A second concurrent native watch is Busy. Hiding a bar must not retire this
/// capability. A low-level hook can be silently removed by Windows after a
/// timeout; readiness does not prove indefinite delivery and no idle polling is
/// authorized by this interface.
pub trait PointerHost: Send + Sync + 'static {
    fn watch(
        &self,
        events: PointerWatchCallback,
        ready: PointerWatchCompletion,
    ) -> Result<Box<dyn PointerWatchGuard>, PointerWatchError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisibilityWindow {
    pub bounds: Rect,
    pub belongs_to_monitor: bool,
    pub minimized: bool,
}

pub struct VisibilityFacts<'a> {
    pub monitor: Rect,
    pub hitbox: Rect,
    pub foreground_interactable: bool,
    pub windows: &'a [VisibilityWindow],
}

/// Source overlap is ANY eligible window, gated by an eligible foreground;
/// it is not just the foreground's rectangle. Touching edges do not overlap.
/// Eligibility remains the parent's existing native authorization policy, which
/// is not an assertion of identical source interactable classification.
pub fn any_window_overlaps(facts: &VisibilityFacts<'_>) -> bool {
    facts.foreground_interactable
        && facts.windows.iter().any(|window| {
            window.belongs_to_monitor
                && !window.minimized
                && strictly_intersects(facts.hitbox, window.bounds)
        })
}

pub fn strictly_intersects(first: Rect, second: Rect) -> bool {
    first.x() < second.right()
        && second.x() < first.right()
        && first.y() < second.bottom()
        && second.y() < first.bottom()
}

/// Full physical monitor edges, inclusive ±2px tolerance. Orthogonal spans are
/// half-open; bottom/right use the final pixel before the exclusive bound.
/// Exactly one result follows source Top → Bottom → Left → Right precedence.
pub fn edge_at(monitor: Rect, point: PhysicalPoint) -> Option<Edge> {
    let x = i64::from(point.x);
    let y = i64::from(point.y);
    let left = i64::from(monitor.x());
    let top = i64::from(monitor.y());
    let right = i64::from(monitor.right());
    let bottom = i64::from(monitor.bottom());
    let near = |value: i64, edge: i64| (value - edge).abs() <= 2;
    if (left..right).contains(&x) && near(y, top) {
        Some(Edge::Top)
    } else if (left..right).contains(&x) && near(y, bottom - 1) {
        Some(Edge::Bottom)
    } else if (top..bottom).contains(&y) && near(x, left) {
        Some(Edge::Left)
    } else if (top..bottom).contains(&y) && near(x, right - 1) {
        Some(Edge::Right)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AutoHideMode {
    Never,
    Always,
    #[default]
    OnOverlap,
}

pub const DEFAULT_SHOW_DELAY: Duration = Duration::from_millis(100);
pub const DEFAULT_HIDE_DELAY: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarConfig {
    pub mode: AutoHideMode,
    pub show_delay: Duration,
    pub hide_delay: Duration,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            mode: AutoHideMode::OnOverlap,
            show_delay: DEFAULT_SHOW_DELAY,
            hide_delay: DEFAULT_HIDE_DELAY,
        }
    }
}

/// `None` means unavailable, not false. Own-focus and primary-touch protection
/// are mandatory before enabling autohide. Attention/workspace booleans are
/// explicit optional overrides, not fabricated claims of native observation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BarFacts {
    pub overlap: Option<bool>,
    pub confirmed_fullscreen: bool,
    pub own_focus: Option<bool>,
    pub touch_primary: Option<bool>,
    pub dragging: bool,
    pub attention_reveal: bool,
    pub workspace_switching: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisibilityInputs {
    pub config: BarConfig,
    pub facts: BarFacts,
    pub pointer_ready: bool,
    /// Known false is different from no physical pointer observation.
    pub selected_edge: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisibilityDecision {
    Keep,
    ShowNow,
    ShowAfter(Duration),
    HideAfter(Duration),
    HideNow,
}

/// Fullscreen is an independent outer suppression and beats every reveal.
/// Missing protection/overlap/pointer evidence fails visible, never hidden.
/// Workspace switching cancels a pending timer and freezes the autohide result.
pub fn decide_visibility(inputs: VisibilityInputs) -> VisibilityDecision {
    let facts = inputs.facts;
    if facts.confirmed_fullscreen {
        return VisibilityDecision::HideNow;
    }
    if inputs.config.mode == AutoHideMode::Never
        || facts.dragging
        || facts.touch_primary == Some(true)
        || facts.attention_reveal
    {
        return VisibilityDecision::ShowNow;
    }
    if !inputs.pointer_ready
        || inputs.selected_edge.is_none()
        || facts.touch_primary.is_none()
        || facts.own_focus.is_none()
        || (inputs.config.mode == AutoHideMode::OnOverlap && facts.overlap.is_none())
    {
        return VisibilityDecision::ShowNow;
    }
    if facts.workspace_switching {
        return VisibilityDecision::Keep;
    }
    let reveal = facts.own_focus == Some(true)
        || inputs.selected_edge == Some(true)
        || (inputs.config.mode == AutoHideMode::OnOverlap && facts.overlap == Some(false));
    if reveal {
        VisibilityDecision::ShowAfter(inputs.config.show_delay)
    } else {
        VisibilityDecision::HideAfter(inputs.config.hide_delay)
    }
}

#[cfg(test)]
mod tests;
