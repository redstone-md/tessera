// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Cache a successfully queued native provider, never a failed initialization.

use std::sync::Arc;

use parking_lot::Mutex;
use tessera_system::audio::{AudioError, AudioHost};

#[derive(Default)]
pub(crate) struct AudioProvider {
    ready: Mutex<Option<Arc<dyn AudioHost>>>,
}

impl AudioProvider {
    /// Called only on explicit popup open/retry. Factory acceptance is prompt;
    /// the provider owns asynchronous hardware initialization and requests.
    pub(crate) fn get(
        &self,
        create: impl FnOnce() -> Result<Arc<dyn AudioHost>, AudioError>,
    ) -> Result<Arc<dyn AudioHost>, AudioError> {
        let mut ready = self.ready.lock();
        if let Some(host) = &*ready {
            return Ok(Arc::clone(host));
        }
        let host = create()?;
        *ready = Some(Arc::clone(&host));
        Ok(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_system::audio::{AudioCommand, AudioCompletion, AudioErrorKind};

    struct RecordingHost;
    impl AudioHost for RecordingHost {
        fn read(&self, _completion: AudioCompletion) -> Result<(), AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "No hardware in cache fixture",
            ))
        }
        fn execute(
            &self,
            _command: AudioCommand,
            _completion: AudioCompletion,
        ) -> Result<(), AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "No hardware in cache fixture",
            ))
        }
    }

    #[test]
    fn failed_factory_is_retryable_and_success_is_cached() {
        let provider = AudioProvider::default();
        let failure = AudioError::new(AudioErrorKind::Other, "Worker creation temporarily failed");
        let mut attempts = 0;
        let first = provider.get(|| {
            attempts += 1;
            Err(failure.clone())
        });
        assert_eq!(first.err(), Some(failure));
        let host: Arc<dyn AudioHost> = Arc::new(RecordingHost);
        let retry = provider
            .get(|| {
                attempts += 1;
                Ok(Arc::clone(&host))
            })
            .unwrap();
        assert!(Arc::ptr_eq(&retry, &host));
        let reused = provider
            .get(|| panic!("a ready provider must not be recreated"))
            .unwrap();
        assert!(Arc::ptr_eq(&reused, &host));
        assert_eq!(attempts, 2);
    }
}
