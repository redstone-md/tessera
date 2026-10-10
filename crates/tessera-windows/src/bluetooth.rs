// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Passive Bluetooth reads with optional exact-radio control on explicit intent.
use std::sync::Arc;
use tessera_system::bluetooth::BluetoothHost;

/// Creates admission only; no enumeration or consent. Legacy reads own one
/// scoped worker/apartment. Opt-in controls retain exact radios on their owner;
/// dropping this host closes admission without joining accepted work.
pub fn native_bluetooth_host() -> Arc<dyn BluetoothHost> {
    #[cfg(windows)]
    {
        Arc::new(crate::native_bluetooth::ControlledBluetoothHost::default())
    }
    #[cfg(not(windows))]
    {
        Arc::new(UnsupportedBluetoothHost)
    }
}

#[cfg(not(windows))]
struct UnsupportedBluetoothHost;

#[cfg(not(windows))]
impl BluetoothHost for UnsupportedBluetoothHost {
    fn read(
        &self,
        _completion: tessera_system::bluetooth::BluetoothReadCompletion,
    ) -> Result<(), tessera_system::bluetooth::BluetoothError> {
        use tessera_system::bluetooth::{BluetoothError, BluetoothErrorKind};
        Err(BluetoothError::new(
            BluetoothErrorKind::Unsupported,
            "Bluetooth observation requires Windows",
        ))
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tessera_system::bluetooth::BluetoothErrorKind;

    #[test]
    fn unsupported_rejects_without_completion_or_fake_snapshot() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let result = native_bluetooth_host().read(Box::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(result.unwrap_err().kind, BluetoothErrorKind::Unsupported);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
