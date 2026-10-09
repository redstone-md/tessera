// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Validated presentation geometry, not a certification of native monitor provenance.
use std::fmt;
use std::sync::Arc;
use tessera_core::Rect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplaySelection {
    Primary,
    FirstFallback,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayLayout {
    desktop_bounds: Rect,
    selected_bounds: Rect,
    presentation_scale: f64,
    selection: DisplaySelection,
}

impl DisplayLayout {
    pub fn new(
        desktop_bounds: Rect,
        selected_bounds: Rect,
        presentation_scale: f64,
        selection: DisplaySelection,
    ) -> Result<Self, DisplayContextError> {
        if !presentation_scale.is_finite()
            || presentation_scale <= 0.0
            || selected_bounds.x() < desktop_bounds.x()
            || selected_bounds.y() < desktop_bounds.y()
            || selected_bounds.right() > desktop_bounds.right()
            || selected_bounds.bottom() > desktop_bounds.bottom()
        {
            return Err(DisplayContextError::InvalidData);
        }
        Ok(Self {
            desktop_bounds,
            selected_bounds,
            presentation_scale,
            selection,
        })
    }

    pub fn desktop_bounds(&self) -> Rect {
        self.desktop_bounds
    }
    pub fn selected_bounds(&self) -> Rect {
        self.selected_bounds
    }
    pub fn presentation_scale(&self) -> f64 {
        self.presentation_scale
    }
    pub fn selection(&self) -> DisplaySelection {
        self.selection
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayContextError {
    Unsupported,
    Busy,
    Unavailable,
    Stopped,
    InvalidData,
    Native { code: u32 },
}

impl fmt::Display for DisplayContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "display context is unsupported",
            Self::Busy => "display context read is busy",
            Self::Unavailable => "display context is unavailable",
            Self::Stopped => "display context watch stopped",
            Self::InvalidData => "display context contains invalid data",
            Self::Native { .. } => "native display context read failed",
        })
    }
}
impl std::error::Error for DisplayContextError {}

/// `None` is a successful observation containing no monitors.
pub type DisplayContextCompletion =
    Box<dyn FnOnce(Result<Option<DisplayLayout>, DisplayContextError>) + Send + 'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayContextWatchEvent {
    Changed,
    Unavailable(DisplayContextError),
}

pub type DisplayContextWatchCallback =
    Arc<dyn Fn(DisplayContextWatchEvent) + Send + Sync + 'static>;
pub type DisplayContextWatchReady =
    Box<dyn FnOnce(Result<(), DisplayContextError>) + Send + 'static>;

/// Drop closes callback admission and requests detached retirement, never a
/// GUI-thread join. A callback admitted before Drop may finish.
pub trait DisplayContextWatchGuard: Send {}

/// Submission is prompt: `Ok` accepts exactly one completion (possibly inline),
/// `Err` accepts zero. Implementations must retire resources and release their
/// flight before calling consumers, allowing a completion to submit another read.
pub trait DisplayContextHost: Send + Sync + 'static {
    fn read(&self, completion: DisplayContextCompletion) -> Result<(), DisplayContextError>;

    /// `Ok` accepts startup and exactly one ready callback (possibly inline);
    /// immediate `Err` accepts no callbacks. Early guard drop reports Stopped.
    /// Ready returns before any Changed; startup hints coalesce into one
    /// mandatory post-ready Changed. Watch failure does not disable reads.
    /// Native startup/retirement can stall; cancellation is not a native timeout.
    fn watch(
        &self,
        _on_event: DisplayContextWatchCallback,
        _on_ready: DisplayContextWatchReady,
    ) -> Result<Box<dyn DisplayContextWatchGuard>, DisplayContextError> {
        Err(DisplayContextError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_containment_and_fractional_scale() {
        let desktop = Rect::new(-1920, -200, 3840, 1400).unwrap();
        let selected = Rect::new(-1920, 0, 1920, 1080).unwrap();
        let layout =
            DisplayLayout::new(desktop, selected, 1.375, DisplaySelection::FirstFallback).unwrap();
        assert_eq!(layout.desktop_bounds(), desktop);
        assert_eq!(layout.selected_bounds(), selected);
        assert_eq!(layout.presentation_scale(), 1.375);
        assert_eq!(layout.selection(), DisplaySelection::FirstFallback);
        for outside in [
            Rect::new(-1921, 0, 1920, 1080).unwrap(),
            Rect::new(0, 0, 1921, 1080).unwrap(),
            Rect::new(0, -201, 10, 10).unwrap(),
            Rect::new(0, 1199, 10, 2).unwrap(),
        ] {
            assert_eq!(
                DisplayLayout::new(desktop, outside, 1.0, DisplaySelection::Primary),
                Err(DisplayContextError::InvalidData)
            );
        }
    }

    #[test]
    fn scale_is_never_defaulted() {
        let rect = Rect::new(0, 0, 1, 1).unwrap();
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                DisplayLayout::new(rect, rect, scale, DisplaySelection::Primary),
                Err(DisplayContextError::InvalidData)
            );
        }
        assert_eq!(
            DisplayContextError::Native { code: 123 }.to_string(),
            "native display context read failed"
        );
    }

    #[test]
    fn display_watch_default_rejects_without_callbacks_and_preserves_read() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct ReadOnly;
        impl DisplayContextHost for ReadOnly {
            fn read(&self, complete: DisplayContextCompletion) -> Result<(), DisplayContextError> {
                complete(Ok(None));
                Ok(())
            }
        }
        let events = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(AtomicUsize::new(0));
        let event_count = events.clone();
        let ready_count = ready.clone();
        assert!(matches!(
            ReadOnly.watch(
                Arc::new(move |_| {
                    event_count.fetch_add(1, Ordering::AcqRel);
                }),
                Box::new(move |_| {
                    ready_count.fetch_add(1, Ordering::AcqRel);
                }),
            ),
            Err(DisplayContextError::Unsupported)
        ));
        assert_eq!(events.load(Ordering::Acquire), 0);
        assert_eq!(ready.load(Ordering::Acquire), 0);
        ReadOnly
            .read(Box::new(|result| assert_eq!(result, Ok(None))))
            .unwrap();
        assert_eq!(
            DisplayContextError::Stopped.to_string(),
            "display context watch stopped"
        );
        let event = DisplayContextWatchEvent::Unavailable(DisplayContextError::Stopped);
        let copied = event;
        assert_eq!(copied, event);
    }
}
