//! Public observation contract on unsupported platforms.

#![cfg(not(windows))]

use tessera_windows::observe;

#[test]
fn observe_is_unsupported_off_windows() {
    assert!(matches!(
        observe(),
        Err(tessera_windows::ObservationError::UnsupportedPlatform)
    ));
}
