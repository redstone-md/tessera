// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! User configuration: which surfaces exist and what they hold.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use serde::Deserialize;

use crate::platform::ScreenEdge;

const DEFAULT: &str = include_str!("default_config.toml");

/// The directory user configuration is read from, when the system has one.
pub fn config_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("tessera"))
}

/// Reads `file` from the configuration directory. A missing file is `None`;
/// an unreadable or invalid one is reported and also `None`.
pub fn read_user_file<T: for<'de> Deserialize<'de>>(file: &str) -> Option<T> {
    let path = config_dir()?.join(file);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("tessera: {} is ignored: {error}", path.display());
            return None;
        }
    };
    toml::from_str(&text)
        .inspect_err(|error| eprintln!("tessera: {} is ignored: {error}", path.display()))
        .ok()
}

/// Which displays a surface appears on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Displays {
    #[default]
    All,
    Primary,
}

/// One strip docked to a screen edge, and the widgets in its slots.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceConfig {
    pub edge: ScreenEdge,
    /// Reserved screen space, in logical pixels.
    pub thickness: f32,
    #[serde(default)]
    pub displays: Displays,
    #[serde(default)]
    pub start: Vec<String>,
    #[serde(default)]
    pub center: Vec<String>,
    #[serde(default)]
    pub end: Vec<String>,
}

/// Everything the user can configure.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Name of the theme file to apply over the built-in theme.
    pub theme: Option<String>,
    #[serde(default, rename = "surface")]
    pub surfaces: Vec<SurfaceConfig>,
}

impl Config {
    /// The user's configuration, or the built-in one when there is none or
    /// it cannot be used.
    pub fn load() -> Self {
        read_user_file("config.toml").unwrap_or_else(Self::builtin)
    }

    fn builtin() -> Self {
        toml::from_str(DEFAULT).expect("the built-in configuration is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_configuration_describes_a_bar_and_a_dock() {
        let config = Config::builtin();
        assert_eq!(config.theme, None);
        let [bar, dock] = config.surfaces.as_slice() else {
            panic!("expected two surfaces, got {}", config.surfaces.len());
        };
        assert_eq!(bar.edge, ScreenEdge::Top);
        assert_eq!(bar.center, ["clock"]);
        assert_eq!(bar.end, ["quit"]);
        assert_eq!(dock.edge, ScreenEdge::Bottom);
        assert_eq!(dock.displays, Displays::All);
        assert_eq!(dock.center, ["windows"]);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = "[[surface]]\nedge = \"top\"\nthickness = 40\ncentre = [\"clock\"]\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }
}
