// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What an extension may reach outside its sandbox.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use super::devices;
use crate::i18n::Language;
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
    /// The servers of the local network the extension may send requests
    /// to over plain HTTP, each a host name or address and its port, as
    /// `nas.local:8080`.
    #[serde(default)]
    local: Vec<String>,
    /// The files the extension may read; `~` is the home directory.
    #[serde(default)]
    files: Vec<String>,
    /// The kinds of devices the extension may be told of.
    #[serde(default)]
    devices: Vec<DeviceKind>,
    /// What of the machine and the shell the extension may read and do.
    #[serde(default)]
    system: Vec<SystemKind>,
    /// The actions of Tessera the extension may do, by name.
    #[serde(default)]
    actions: Vec<String>,
}

/// Something of the machine or the shell an extension can ask to read or
/// to do, under `[permissions] system`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemKind {
    /// What the media player plays.
    Media,
    /// Playing, pausing and skipping media.
    MediaControl,
    /// The processor, memory, battery and network meters.
    Meters,
    /// The network connection Windows prefers.
    Network,
    /// The list of the open windows.
    Windows,
    /// Bringing windows in front, minimizing, closing and moving them.
    WindowControl,
    /// The workspaces.
    Workspaces,
    /// The colors of the theme.
    Theme,
    /// Opening and closing its own popups from its code.
    Popups,
    /// Notices among the notifications.
    Notify,
}

impl SystemKind {
    pub const ALL: [Self; 10] = [
        Self::Media,
        Self::MediaControl,
        Self::Meters,
        Self::Network,
        Self::Windows,
        Self::WindowControl,
        Self::Workspaces,
        Self::Theme,
        Self::Popups,
        Self::Notify,
    ];

    /// What a manifest calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Media => "media",
            Self::MediaControl => "media-control",
            Self::Meters => "meters",
            Self::Network => "network",
            Self::Windows => "windows",
            Self::WindowControl => "window-control",
            Self::Workspaces => "workspaces",
            Self::Theme => "theme",
            Self::Popups => "popups",
            Self::Notify => "notify",
        }
    }

    /// What it allows, as the end of a sentence that starts "Asks to".
    fn wish(self) -> &'static str {
        match self {
            Self::Media => "know what media is playing",
            Self::MediaControl => "play, pause and skip media",
            Self::Meters => "read the processor, memory, battery and network meters",
            Self::Network => "know the network connection",
            Self::Windows => "list the open windows",
            Self::WindowControl => "bring windows in front, minimize, close and move them",
            Self::Workspaces => "know the workspaces",
            Self::Theme => "read the colors of the theme",
            Self::Popups => "open its popups by itself",
            Self::Notify => "post notices",
        }
    }
}

/// What a widget of an extension can watch, under `watch` of its
/// `[[widget]]` entry: it is drawn again when that changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WatchKind {
    Media,
    Meters,
    Network,
    Windows,
    Workspaces,
    Theme,
}

impl WatchKind {
    pub const ALL: [Self; 6] = [
        Self::Media,
        Self::Meters,
        Self::Network,
        Self::Windows,
        Self::Workspaces,
        Self::Theme,
    ];

    /// What the manifest must permit for the widget to watch it.
    pub fn needs(self) -> SystemKind {
        match self {
            Self::Media => SystemKind::Media,
            Self::Meters => SystemKind::Meters,
            Self::Network => SystemKind::Network,
            Self::Windows => SystemKind::Windows,
            Self::Workspaces => SystemKind::Workspaces,
            Self::Theme => SystemKind::Theme,
        }
    }
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
    /// What the extension asks to contact: the hosts reached over HTTPS
    /// by name, the servers of the local network as `http://` addresses.
    pub fn network(&self) -> Vec<String> {
        let local = self.local.iter().map(|server| format!("http://{server}"));
        self.network.iter().cloned().chain(local).collect()
    }

    /// The files the extension asks to read.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// The kinds of devices the extension asks to be told of.
    pub fn devices(&self) -> &[DeviceKind] {
        &self.devices
    }

    /// What of the machine and the shell the extension asks for.
    pub fn system(&self) -> &[SystemKind] {
        &self.system
    }

    /// Fails, saying why, unless the manifest permits `kind`.
    pub fn check_system(&self, kind: SystemKind) -> Result<(), String> {
        match self.system.contains(&kind) {
            true => Ok(()),
            false => Err(format!(
                "the manifest does not permit `{}`; list it under `[permissions] system`",
                kind.name()
            )),
        }
    }

    /// Fails, saying why, unless the manifest permits the action `name`.
    pub fn check_action(&self, name: &str) -> Result<(), String> {
        match self.actions.iter().any(|allowed| allowed == name) {
            true => Ok(()),
            false => Err(format!(
                "the manifest does not permit the action `{name}`; list it under `[permissions] actions`"
            )),
        }
    }

    /// What the extension asks to be allowed, one wish in `language` for
    /// each kind, each the end of a sentence that starts "Asks to": the
    /// hosts, the files and the devices, then the machine and the shell,
    /// then the actions. The marketplace asks with these before it
    /// installs an extension.
    #[allow(
        dead_code,
        reason = "the install prompt of the marketplace lists these"
    )]
    pub fn wishes(&self, language: Language) -> Vec<String> {
        let say = |text| language.say(text);
        let mut wishes = Vec::new();
        let network = self.network();
        if !network.is_empty() {
            wishes.push(format!("{} {}", say("contact"), network.join(", ")));
        }
        if !self.files.is_empty() {
            wishes.push(format!("{} {}", say("read"), self.files.join(", ")));
        }
        if !self.devices.is_empty() {
            let kinds: Vec<&str> = self.devices.iter().map(|kind| kind.name()).collect();
            let kinds = kinds.join(", ");
            // English names the kinds in the middle of the phrase.
            wishes.push(match language {
                Language::English => format!("be told of the {kinds} devices"),
                _ => format!("{} {kinds}", say("be told of the devices")),
            });
        }
        // In the order of the kinds, each once, whatever the manifest says.
        for kind in SystemKind::ALL {
            if self.system.contains(&kind) {
                wishes.push(say(kind.wish()).to_owned());
            }
        }
        if !self.actions.is_empty() {
            let actions = self.actions.join(", ");
            wishes.push(format!("{} {actions}", say("do the actions")));
        }
        wishes
    }

    /// Whether a request to `url` is permitted.
    pub fn check_url(&self, url: &str) -> Result<(), String> {
        if let Some(server) = local_server(url) {
            let listed = |allowed: &String| allowed.eq_ignore_ascii_case(server);
            return match self.local.iter().any(listed) {
                true => Ok(()),
                false => Err(format!(
                    "the manifest does not permit contacting `{server}`"
                )),
            };
        }
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

/// The host and port of a plain HTTP URL, as `nas.local:8080`. URLs
/// without a port, with credentials or with anything else before the path
/// are not accepted.
fn local_server(url: &str) -> Option<&str> {
    let authority = url.strip_prefix("http://")?.split(['/', '?', '#']).next()?;
    let (host, port) = authority.split_once(':')?;
    let is_name = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    let is_port = port.bytes().all(|byte| byte.is_ascii_digit()) && port.parse::<u16>().is_ok();
    (is_name && is_port).then_some(authority)
}

/// The resources of one extension.
#[derive(Debug, Default)]
pub struct Access {
    pub permissions: Permissions,
    /// The `[extension.<id>]` table of the configuration.
    pub settings: HashMap<String, String>,
    /// Where `save-state` writes; `None` keeps nothing.
    pub state_file: Option<PathBuf>,
    /// The names of the popups of the manifest, as it names them.
    pub popups: Vec<String>,
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
            local: Vec::new(),
            files: files.iter().map(|&file| file.to_owned()).collect(),
            devices: Vec::new(),
            system: Vec::new(),
            actions: Vec::new(),
        }
    }

    #[test]
    fn the_machine_and_the_shell_are_reached_only_as_the_manifest_permits() {
        let permissions: Permissions =
            toml::from_str("system = [\"media\", \"window-control\"]\nactions = [\"popup\"]")
                .unwrap();
        assert_eq!(permissions.check_system(SystemKind::Media), Ok(()));
        assert_eq!(permissions.check_system(SystemKind::WindowControl), Ok(()));
        let refused = permissions.check_system(SystemKind::Windows).unwrap_err();
        assert!(refused.contains("`windows`"), "{refused}");
        assert_eq!(permissions.check_action("popup"), Ok(()));
        assert!(permissions.check_action("quit").is_err());
        assert!(permissions.check_action("Popup").is_err());
        assert!(toml::from_str::<Permissions>("system = [\"registry\"]").is_err());
        for kind in SystemKind::ALL {
            let text = format!("system = [\"{}\"]", kind.name());
            let read: Permissions = toml::from_str(&text).unwrap();
            assert_eq!(read.system(), [kind]);
        }
    }

    #[test]
    fn every_wish_is_said_once_in_order() {
        let permissions: Permissions = toml::from_str(concat!(
            "network = [\"api.example.com\"]\n",
            "devices = [\"bluetooth\"]\n",
            "system = [\"notify\", \"media\", \"media\"]\n",
            "actions = [\"popup\", \"media\"]\n",
        ))
        .unwrap();
        assert_eq!(
            permissions.wishes(Language::English),
            [
                "contact api.example.com",
                "be told of the bluetooth devices",
                "know what media is playing",
                "post notices",
                "do the actions popup, media",
            ]
        );
        assert!(Permissions::default().wishes(Language::English).is_empty());
    }

    #[test]
    fn every_kind_that_can_be_watched_needs_its_permission() {
        let mut needed: Vec<SystemKind> = WatchKind::ALL.iter().map(|kind| kind.needs()).collect();
        needed.dedup();
        assert_eq!(needed.len(), WatchKind::ALL.len());
        assert!(needed.iter().all(|kind| SystemKind::ALL.contains(kind)));
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
    fn only_listed_local_servers_are_reached_over_plain_http() {
        let permissions = Permissions {
            local: vec!["nas.local:8080".to_owned()],
            ..Permissions::default()
        };
        assert!(
            permissions
                .check_url("http://nas.local:8080/api?x=1")
                .is_ok()
        );
        assert!(permissions.check_url("http://NAS.local:8080").is_ok());
        assert_eq!(permissions.network(), ["http://nas.local:8080"]);
        for url in [
            "http://nas.local/",
            "http://nas.local:8081/",
            "http://nas.local:8080@evil.test:8080/",
            "http://evil.test:8080/",
            "http://nas.local:+8080/",
            "https://nas.local:8080/",
            "https://nas.local/",
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
