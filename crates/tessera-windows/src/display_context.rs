// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Independent native display reads and notifications. No application HWND is used.
use std::sync::Arc;
use tessera_system::display_context::{DisplayContextError, DisplayContextHost};

/// Construction does not enumerate displays, allocate windows, or initialize WinRT.
pub fn native_display_context_host() -> Result<Arc<dyn DisplayContextHost>, DisplayContextError> {
    #[cfg(windows)]
    {
        Ok(Arc::new(CompositeDisplayContextHost {
            read: crate::native_display_context::NativeDisplayContextHost::default(),
            watch: crate::native_display_watch::watch,
        }))
    }
    #[cfg(not(windows))]
    {
        Err(DisplayContextError::Unsupported)
    }
}

#[cfg(any(windows, test))]
type WatchFactory = fn(
    tessera_system::display_context::DisplayContextWatchCallback,
    tessera_system::display_context::DisplayContextWatchReady,
) -> Result<
    Box<dyn tessera_system::display_context::DisplayContextWatchGuard>,
    DisplayContextError,
>;

#[cfg(any(windows, test))]
pub(crate) struct CompositeDisplayContextHost<R> {
    pub(crate) read: R,
    pub(crate) watch: WatchFactory,
}

#[cfg(any(windows, test))]
impl<R: DisplayContextHost> DisplayContextHost for CompositeDisplayContextHost<R> {
    fn read(
        &self,
        completion: tessera_system::display_context::DisplayContextCompletion,
    ) -> Result<(), DisplayContextError> {
        self.read.read(completion)
    }

    fn read_selected(
        &self,
        selection: tessera_system::display_context::DisplaySelection,
        completion: tessera_system::display_context::DisplayContextCompletion,
    ) -> Result<(), DisplayContextError> {
        self.read.read_selected(selection, completion)
    }

    fn watch(
        &self,
        on_event: tessera_system::display_context::DisplayContextWatchCallback,
        on_ready: tessera_system::display_context::DisplayContextWatchReady,
    ) -> Result<
        Box<dyn tessera_system::display_context::DisplayContextWatchGuard>,
        DisplayContextError,
    > {
        (self.watch)(on_event, on_ready)
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    #[test]
    fn unsupported_is_not_an_empty_observation() {
        assert!(matches!(
            super::native_display_context_host(),
            Err(tessera_system::display_context::DisplayContextError::Unsupported)
        ));
    }
}

#[cfg(test)]
mod selected_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tessera_core::Rect;
    use tessera_system::display_context::{
        DisplayContextCompletion, DisplayContextWatchCallback, DisplayContextWatchGuard,
        DisplayContextWatchReady, DisplayLayout, DisplaySelection,
    };

    struct PointReader {
        reads: Arc<AtomicUsize>,
    }

    impl DisplayContextHost for PointReader {
        fn read(&self, _: DisplayContextCompletion) -> Result<(), DisplayContextError> {
            panic!("selected facade read must not use the legacy read");
        }

        fn read_selected(
            &self,
            selection: DisplaySelection,
            completion: DisplayContextCompletion,
        ) -> Result<(), DisplayContextError> {
            assert_eq!(selection, DisplaySelection::AtPoint { x: -1, y: -1 });
            self.reads.fetch_add(1, Ordering::AcqRel);
            let bounds = Rect::new(-100, -100, 100, 100).unwrap();
            completion(DisplayLayout::new(bounds, bounds, 1.375, selection).map(Some));
            Ok(())
        }
    }

    fn unsupported_watch(
        _: DisplayContextWatchCallback,
        _: DisplayContextWatchReady,
    ) -> Result<Box<dyn DisplayContextWatchGuard>, DisplayContextError> {
        Err(DisplayContextError::Unsupported)
    }

    #[test]
    fn composite_forwards_point_selection_without_using_legacy_or_watch() {
        let reads = Arc::new(AtomicUsize::new(0));
        let host = CompositeDisplayContextHost {
            read: PointReader {
                reads: reads.clone(),
            },
            watch: unsupported_watch,
        };
        let callbacks = Arc::new(AtomicUsize::new(0));
        let count = callbacks.clone();
        let requested = DisplaySelection::AtPoint { x: -1, y: -1 };
        host.read_selected(
            requested,
            Box::new(move |result| {
                let layout = result.unwrap().unwrap();
                assert_eq!(layout.selection(), requested);
                assert_eq!(
                    layout.selected_bounds(),
                    Rect::new(-100, -100, 100, 100).unwrap()
                );
                assert_eq!(layout.presentation_scale(), 1.375);
                count.fetch_add(1, Ordering::AcqRel);
            }),
        )
        .unwrap();
        assert_eq!(reads.load(Ordering::Acquire), 1);
        assert_eq!(callbacks.load(Ordering::Acquire), 1);
    }
}
