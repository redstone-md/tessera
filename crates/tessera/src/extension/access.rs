// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What an extension may reach outside its sandbox.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use super::devices;
use crate::platform::PairedDevice;

/// The most text `read-file` returns.
const FILE_LIMIT: u64 = 1 << 20;

/// The most text `save-state` keeps.
const STATE_LIMIT: usize = 64 << 10;

/// What the manifest of an extension asks to be allowed.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// The hosts the extension may send requests to, over HTTPS.
    #[serde(default)]
    network: Vec<String>,
    /// The files the extension may read; `~` is the home directory.
    #[serde(default)]
    files: Vec<String>,
    /// The kinds of devices the extension may be told of.
    #[serde(default)]
    devices: Vec<DeviceKind>,
}

/// A kind of device an extension can ask about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Bluetooth,
}

impl DeviceKind {
    /// What a manifest calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Bluetooth => "bluetooth",
        }
    }
}

impl Permissions {
    /// The hosts the extension asks to contact.
    pub fn network(&self) -> &[String] {
        &self.network
    }

    /// The files the extension asks to read.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// The kinds of devices the extension asks to be told of.
    pub fn devices(&self) -> &[DeviceKind] {
        &self.devices
    }

    /// Whether a request to `url` is permitted.
    pub fn check_url(&self, url: &str) -> Result<(), String> {
        let host =
            https_host(url).ok_or("only https:// URLs of a plain host name can be fetched")?;
        if self
            .network
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host))
        {
            Ok(())
        } else {
            Err(format!("the manifest does not permit contacting `{host}`"))
        }
    }

    /// Where the permitted file `path` is.
    fn file(&self, path: &str) -> Result<PathBuf, String> {
        if !self.files.iter().any(|allowed| allowed == path) {
            return Err(format!("the manifest does not permit reading `{path}`"));
        }
        match path.strip_prefix("~/") {
            Some(below_home) => std::env::var_os("USERPROFILE")
                .map(|home| PathBuf::from(home).join(below_home))
                .ok_or_else(|| "the home directory is unknown".to_owned()),
            None => Ok(PathBuf::from(path)),
        }
    }
}

/// The host of an HTTPS URL. URLs with a port, credentials or anything
/// else beside a host name before the path are not accepted.
fn https_host(url: &str) -> Option<&str> {
    let authority = url
        .strip_prefix("https://")?
        .split(['/', '?', '#'])
        .next()?;
    let is_name = !authority.is_empty()
        && authority
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    is_name.then_some(authority)
}

/// The resources of one extension.
#[derive(Debug, Default)]
pub struct Access {
    pub permissions: Permissions,
    /// The `[extension.<id>]` table of the configuration.
    pub settings: HashMap<String, String>,
    /// Where `save-state` writes; `None` keeps nothing.
    pub state_file: Option<PathBuf>,
    /// Stands in for the Bluetooth devices of the machine.
    #[cfg(test)]
    pub bluetooth: Option<Vec<PairedDevice>>,
}

impl Access {
    pub fn read_file(&self, path: &str) -> Result<String, String> {
        let file = self.permissions.file(path)?;
        let size = fs::metadata(&file)
            .map_err(|error| error.to_string())?
            .len();
        if size > FILE_LIMIT {
            return Err(format!("`{path}` is larger than {FILE_LIMIT} bytes"));
        }
        fs::read_to_string(&file).map_err(|error| error.to_string())
    }

    /// The paired Bluetooth devices, as they were read last.
    pub fn bluetooth(&self) -> Result<Vec<PairedDevice>, String> {
        if !self.permissions.devices.contains(&DeviceKind::Bluetooth) {
            return Err("the manifest does not permit asking about Bluetooth devices".to_owned());
        }
        #[cfg(test)]
        if let Some(devices) = &self.bluetooth {
            return Ok(devices.clone());
        }
        devices::bluetooth()
    }

    pub fn load_state(&self) -> String {
        self.state_file
            .as_ref()
            .and_then(|file| fs::read_to_string(file).ok())
            .unwrap_or_default()
    }

    pub fn save_state(&self, state: &str) -> Result<(), String> {
        if state.len() > STATE_LIMIT {
            return Err(format!("the state is larger than {STATE_LIMIT} bytes"));
        }
        let file = self
            .state_file
            .as_ref()
            .ok_or("there is nowhere to keep state")?;
        fs::write(file, state).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissions(network: &[&str], files: &[&str]) -> Permissions {
        Permissions {
            network: network.iter().map(|&host| host.to_owned()).collect(),
            files: files.iter().map(|&file| file.to_owned()).collect(),
            devices: Vec::new(),
        }
    }

    #[test]
    fn devices_are_told_of_only_when_the_manifest_asks() {
        let device = PairedDevice {
            name: "Mouse".to_owned(),
            connected: true,
            battery: Some(80),
            audio: None,
        };
        let mut access = Access {
            bluetooth: Some(vec![device.clone()]),
            ..Access::default()
        };
        assert!(access.bluetooth().is_err());
        access.permissions = toml::from_str("devices = [\"bluetooth\"]").unwrap();
        assert_eq!(access.bluetooth(), Ok(vec![device]));
        assert!(toml::from_str::<Permissions>("devices = [\"usb\"]").is_err());
    }

    #[test]
    fn only_listed_hosts_are_reached_over_https() {
        let permissions = permissions(&["api.example.com"], &[]);
        assert!(
            permissions
                .check_url("https://api.example.com/v1?x=1")
                .is_ok()
        );
        assert!(permissions.check_url("https://API.example.com").is_ok());
        for url in [
            "http://api.example.com/",
            "https://example.com/",
            "https://api.example.com.evil.test/",
            "https://api.example.com@evil.test/",
            "https://evil.test\\@api.example.com/",
            "https://api.example.com:8443/",
            "https:///",
            "file:///C:/secret",
        ] {
            assert!(permissions.check_url(url).is_err(), "{url} was permitted");
        }
    }

    #[test]
    fn only_listed_files_are_read() {
        let directory = std::env::temp_dir().join(format!("tessera-access-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let listed = directory.join("listed.txt");
        let other = directory.join("other.txt");
        fs::write(&listed, "content").unwrap();
        fs::write(&other, "secret").unwrap();

        let access = Access {
            permissions: permissions(&[], &[listed.to_str().unwrap()]),
            ..Access::default()
        };
        let read = access.read_file(listed.to_str().unwrap());
        let refused = access.read_file(other.to_str().unwrap());
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(read.as_deref(), Ok("content"));
        assert!(refused.is_err());
    }

    #[test]
    fn state_survives_and_is_limited() {
        let file = std::env::temp_dir().join(format!("tessera-state-{}.txt", std::process::id()));
        let access = Access {
            state_file: Some(file.clone()),
            ..Access::default()
        };
        assert_eq!(access.load_state(), "");
        access.save_state("kept").unwrap();
        assert_eq!(access.load_state(), "kept");
        assert!(access.save_state(&"x".repeat(STATE_LIMIT + 1)).is_err());
        fs::remove_file(&file).unwrap();
    }
}
