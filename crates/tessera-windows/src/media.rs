// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! OS-current media facade. Only its owner worker touches native sessions.

use std::sync::Arc;

#[cfg(not(windows))]
use tessera_system::media::MediaErrorKind;
use tessera_system::media::{
    MediaCommandCompletion, MediaError, MediaEvent, MediaHost, MediaReadCompletion, MediaRequest,
};

#[cfg(any(windows, test))]
#[path = "media/actor.rs"]
pub(crate) mod actor;

/// A detached owner worker with bounded admission and one shared read/command
/// flight. Construction never waits for WinRT; native initialization errors are
/// completion/watch results. Only a subsequent read may retry initialization.
/// Last facade/subscription drop requests retirement, never joins the worker.
#[derive(Clone)]
pub struct MediaService {
    #[cfg(any(windows, test))]
    queue: Arc<actor::QueueLifetime>,
}

impl MediaService {
    pub fn new() -> Result<Self, MediaError> {
        #[cfg(windows)]
        {
            actor::spawn(crate::native_media::NativeMediaDriver::new)
        }
        #[cfg(not(windows))]
        {
            Err(unsupported())
        }
    }
}

#[cfg(not(windows))]
fn unsupported() -> MediaError {
    MediaError::new(MediaErrorKind::Unsupported, "Native media requires Windows")
}

impl MediaHost for MediaService {
    fn read(&self, completion: MediaReadCompletion) -> Result<(), MediaError> {
        #[cfg(any(windows, test))]
        {
            self.queue.read(completion)
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = completion;
            Err(unsupported())
        }
    }

    fn execute(
        &self,
        command: MediaRequest,
        completion: MediaCommandCompletion,
    ) -> Result<(), MediaError> {
        #[cfg(any(windows, test))]
        {
            self.queue.execute(command, completion)
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = (command, completion);
            Err(unsupported())
        }
    }

    fn subscribe(
        &self,
        changed: Arc<dyn Fn(MediaEvent) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, MediaError> {
        #[cfg(any(windows, test))]
        {
            self.queue.subscribe(changed)
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = changed;
            Err(unsupported())
        }
    }
}
