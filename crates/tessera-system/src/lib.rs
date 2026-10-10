// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Portable system capability contracts, independent of native and UI types.

#![forbid(unsafe_code)]

mod audio_devices;

pub mod application_menu;
pub mod battery;
pub mod bluetooth;
pub mod file_search;
pub mod input_language;
pub mod media;
pub mod network;
pub mod profile;
pub mod search;
pub mod shortcuts;
pub mod startup;
pub mod telemetry;
pub mod visibility;
pub mod web_search;

/// Gregorian calendar projection and asynchronous real-date/locale capability.
pub mod calendar;

/// Explicit reserved Dock effects, independent of application identities.
pub mod dock_utilities;

/// Fresh display geometry and source-selected monitor presentation scaling.
pub mod display_context;

/// Current-user known-folder observations and identity-bound opening.
pub mod folders;

/// Explicit typed session/power requests, separate from display observation.
pub mod power;

/// Read-only pending-update key hints, separate from power request authority.
pub mod power_updates;

/// Confirmed Recycle Bin information, fixed opening and scoped change hints.
pub mod recycle_bin;

/// Explicit confirmed Recycle Bin mutation, separate from observation.
pub mod recycle_bin_mutation;

/// Default-multimedia audio endpoints and their asynchronous host interface.
pub mod audio {
    pub use crate::audio_devices::*;
    use std::fmt;
    use std::sync::Arc;

    /// The default multimedia endpoint direction, not a communications role.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum AudioFlow {
        Output,
        Input,
    }

    /// An opaque native endpoint identity. Never interpret it as a path.
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct EndpointId(String);

    impl EndpointId {
        /// Accepts a nonempty identity without normalizing its contents.
        pub fn new(value: String) -> Result<Self, AudioError> {
            if value.is_empty() {
                return Err(AudioError::new(
                    AudioErrorKind::InvalidValue,
                    "Audio endpoint identity must not be empty",
                ));
            }
            Ok(Self(value))
        }

        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    /// A finite endpoint volume scalar in the inclusive range `0..=1`.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Volume(f32);

    impl Volume {
        pub fn from_scalar(value: f32) -> Result<Self, AudioError> {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(AudioError::new(
                    AudioErrorKind::InvalidValue,
                    "Audio volume must be finite and between 0 and 1",
                ));
            }
            Ok(Self(value))
        }

        pub fn scalar(&self) -> f32 {
            self.0
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum AudioErrorKind {
        Unsupported,
        AccessDenied,
        DeviceChanged,
        InvalidValue,
        Stopped,
        Other,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct AudioError {
        pub kind: AudioErrorKind,
        pub message: String,
    }

    impl AudioError {
        pub fn new(kind: AudioErrorKind, message: impl Into<String>) -> Self {
            Self {
                kind,
                message: message.into(),
            }
        }
    }

    impl fmt::Display for AudioError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for AudioError {}

    #[derive(Clone, Debug, PartialEq)]
    pub struct AudioEndpoint {
        pub id: EndpointId,
        pub volume: Volume,
        pub muted: bool,
    }

    /// Absence is distinct from a failure to observe an endpoint.
    #[derive(Clone, Debug, PartialEq)]
    pub enum EndpointState {
        Ready(AudioEndpoint),
        Absent,
        Unavailable(AudioError),
    }

    /// Independently observed output and input; neither is a placeholder.
    #[derive(Clone, Debug, PartialEq)]
    pub struct AudioSnapshot {
        pub output: EndpointState,
        pub input: EndpointState,
    }

    /// Absolute changes to the current default multimedia endpoint.
    ///
    /// The host revalidates `expected_id` before changing state and reports
    /// `DeviceChanged` if it no longer identifies the current default endpoint.
    #[derive(Clone, Debug, PartialEq)]
    pub enum AudioCommand {
        SetVolume {
            flow: AudioFlow,
            expected_id: EndpointId,
            volume: Volume,
        },
        SetMuted {
            flow: AudioFlow,
            expected_id: EndpointId,
            muted: bool,
        },
    }

    /// Watch setup and snapshot invalidation notifications.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum AudioEvent {
        Changed,
        WatchReady,
        WatchUnavailable(AudioError),
    }

    /// One accepted request's result, delivered on any thread, possibly inline.
    pub type AudioCompletion = Box<dyn FnOnce(Result<AudioSnapshot, AudioError>) + Send + 'static>;

    /// Nonblocking seam for native audio and recording adapters.
    ///
    /// Every method returns promptly without waiting for hardware I/O. An `Ok`
    /// from `read` or `execute` accepts the request and guarantees exactly one
    /// completion; an immediate `Err` guarantees the callback will not run.
    /// Callbacks and notifications may run synchronously or on a worker thread.
    /// UI callers always post them to their event loop and generation-gate them.
    pub trait AudioHost: Send + Sync + 'static {
        fn read(&self, completion: AudioCompletion) -> Result<(), AudioError>;

        /// Completion carries the confirmed post-command snapshot, or an error.
        fn execute(
            &self,
            command: AudioCommand,
            completion: AudioCompletion,
        ) -> Result<(), AudioError>;

        /// Opts into device/session inventory without changing legacy default-route reads.
        fn supports_devices(&self) -> bool {
            false
        }

        /// Complete device/session inventory, independent of the default-route snapshot.
        fn read_devices(&self, _completion: AudioDevicesCompletion) -> Result<(), AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "Audio device inventory is unavailable",
            ))
        }

        /// Scoped native mutation; completion carries actual readback, including partial roles.
        fn execute_device(
            &self,
            _command: AudioDeviceCommand,
            _completion: AudioDevicesCompletion,
        ) -> Result<(), AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "Audio device controls are unavailable",
            ))
        }

        /// `Changed` invalidates the snapshot; callers request a fresh read.
        ///
        /// A guard accepts asynchronous watch setup, not live-update readiness.
        /// Emit `WatchReady` only after native callbacks are registered, or
        /// `WatchUnavailable` if setup fails. `None` means unsupported watching:
        /// refresh on open or manually. An immediate error emits no events.
        ///
        /// Dropping the guard suppresses new delivery and queues native cleanup.
        /// Already queued notifications may arrive; consumers generation-gate.
        fn subscribe(
            &self,
            _changed: Arc<dyn Fn(AudioEvent) + Send + Sync>,
        ) -> Result<Option<Box<dyn Send>>, AudioError> {
            Ok(None)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{AudioErrorKind, EndpointId, Volume};

        #[test]
        fn volume_accepts_inclusive_boundaries() {
            for value in [0.0, -0.0, 0.5, 1.0] {
                let volume = Volume::from_scalar(value).expect("valid volume scalar");
                assert_eq!(volume.scalar(), value);
            }
        }

        #[test]
        fn volume_rejects_nonfinite_and_out_of_range_scalars() {
            for value in [
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                -f32::EPSILON,
                1.0 + f32::EPSILON,
            ] {
                let error = Volume::from_scalar(value).expect_err("invalid volume scalar");
                assert_eq!(error.kind, AudioErrorKind::InvalidValue);
            }
        }

        #[test]
        fn endpoint_id_rejects_empty_identity() {
            let error = EndpointId::new(String::new()).expect_err("empty endpoint identity");
            assert_eq!(error.kind, AudioErrorKind::InvalidValue);
        }
    }
}
