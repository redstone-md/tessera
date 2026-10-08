// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Retain successfully accepted native providers, never failed initialization.

use std::sync::Arc;

use parking_lot::Mutex;

pub(crate) struct Provider<T: ?Sized> {
    ready: Mutex<Option<Arc<T>>>,
}

impl<T: ?Sized> Default for Provider<T> {
    fn default() -> Self {
        Self {
            ready: Mutex::new(None),
        }
    }
}

impl<T: ?Sized> Provider<T> {
    /// Called only on explicit popup open/retry. Factory acceptance is prompt;
    /// the provider owns asynchronous native initialization and requests.
    pub(crate) fn get<E>(&self, create: impl FnOnce() -> Result<Arc<T>, E>) -> Result<Arc<T>, E> {
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
    use tessera_system::audio::{
        AudioCommand, AudioCompletion, AudioError, AudioErrorKind, AudioHost,
    };

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
        let provider = Provider::<dyn AudioHost>::default();
        let failure = AudioError::new(AudioErrorKind::Other, "Worker creation temporarily failed");
        let mut attempts = 0;
        let first = provider.get(|| {
            attempts += 1;
            Err(failure.clone())
        });
        assert_eq!(first.err(), Some(failure));
        let host: Arc<dyn AudioHost> = Arc::new(RecordingHost);
        let retry = provider
            .get::<AudioError>(|| {
                attempts += 1;
                Ok(Arc::clone(&host))
            })
            .unwrap();
        assert!(Arc::ptr_eq(&retry, &host));
        let reused = provider
            .get::<AudioError>(|| panic!("a ready provider must not be recreated"))
            .unwrap();
        assert!(Arc::ptr_eq(&reused, &host));
        assert_eq!(attempts, 2);
    }
}
