// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The network connection that Windows reaches the internet through.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Networking::Connectivity::{
    ConnectionProfile, NetworkConnectivityLevel, NetworkInformation,
    NetworkStatusChangedEventHandler,
};
use windows::core::Result as WindowsResult;

/// How a connection reaches its network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// A cable, or anything else that has no signal strength.
    Wired,
    /// Wi-Fi, with a signal of 0 to 5 bars.
    Wireless(u8),
    /// A mobile network, with a signal of 0 to 5 bars.
    Cellular(u8),
}

/// The connection Windows prefers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// What Windows calls the network, such as the name of a Wi-Fi.
    pub name: String,
    pub link: Link,
    /// Whether the internet is reached, rather than only the local network.
    pub internet: bool,
}

/// Counts the changes Windows reported.
static CHANGES: AtomicU64 = AtomicU64::new(0);

impl Network {
    /// The preferred connection; `None` while there is none.
    pub fn current() -> Option<Self> {
        // Windows answers with nothing, which reads as an error, while
        // nothing is connected.
        let profile = NetworkInformation::GetInternetConnectionProfile().ok()?;
        Self::of(&profile).ok()
    }

    fn of(profile: &ConnectionProfile) -> WindowsResult<Self> {
        // Absent for a connection without a signal strength.
        let bars = || {
            profile
                .GetSignalBars()
                .and_then(|bars| bars.Value())
                .unwrap_or(0)
        };
        let link = if profile.IsWlanConnectionProfile()? {
            Link::Wireless(bars())
        } else if profile.IsWwanConnectionProfile()? {
            Link::Cellular(bars())
        } else {
            Link::Wired
        };
        let level = profile.GetNetworkConnectivityLevel()?;
        Ok(Self {
            name: profile.ProfileName()?.to_string(),
            link,
            internet: level == NetworkConnectivityLevel::InternetAccess,
        })
    }

    /// Asks Windows to report when the connection changes, once. Whether
    /// it will is returned.
    pub fn watch() -> bool {
        static WATCHED: OnceLock<bool> = OnceLock::new();
        *WATCHED.get_or_init(|| {
            let handler = NetworkStatusChangedEventHandler::new(|_| {
                CHANGES.fetch_add(1, Ordering::Relaxed);
                Ok(())
            });
            NetworkInformation::NetworkStatusChanged(&handler).is_ok()
        })
    }

    /// A number that grows whenever Windows reports a change.
    pub fn changes() -> u64 {
        CHANGES.load(Ordering::Relaxed)
    }
}
