// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tessera_system::visibility::AutoHideMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AutoHidePreference(AutoHideMode);

impl AutoHidePreference {
    pub(super) const fn new(mode: AutoHideMode) -> Self {
        Self(mode)
    }

    pub(super) const fn mode(self) -> AutoHideMode {
        self.0
    }
}

impl Serialize for AutoHidePreference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self.0 {
            AutoHideMode::Never => "never",
            AutoHideMode::Always => "always",
            AutoHideMode::OnOverlap => "on_overlap",
        })
    }
}

impl<'de> Deserialize<'de> for AutoHidePreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ModeString;

        impl<'de> Visitor<'de> for ModeString {
            type Value = AutoHidePreference;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an auto-hide mode string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let mode = match value {
                    "never" => AutoHideMode::Never,
                    "always" => AutoHideMode::Always,
                    "on_overlap" => AutoHideMode::OnOverlap,
                    _ => {
                        return Err(E::unknown_variant(
                            value,
                            &["never", "always", "on_overlap"],
                        ));
                    }
                };
                Ok(AutoHidePreference::new(mode))
            }
        }

        deserializer.deserialize_str(ModeString)
    }
}
