// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};
use tessera_system::visibility::AutoHideMode;

use super::visibility::AutoHidePreference;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ToolbarPreferences {
    telemetry_enabled: bool,
    #[serde(skip_serializing_if = "is_never")]
    auto_hide: AutoHidePreference,
}

impl Default for ToolbarPreferences {
    fn default() -> Self {
        Self {
            telemetry_enabled: false,
            auto_hide: default_auto_hide(),
        }
    }
}

impl ToolbarPreferences {
    pub(crate) fn telemetry_enabled(&self) -> bool {
        self.telemetry_enabled
    }

    pub(crate) fn with_telemetry_enabled(mut self, enabled: bool) -> Self {
        self.telemetry_enabled = enabled;
        self
    }

    pub(crate) fn auto_hide(&self) -> AutoHideMode {
        self.auto_hide.mode()
    }

    pub(crate) fn with_auto_hide(self, mode: AutoHideMode) -> Self {
        Self {
            auto_hide: AutoHidePreference::new(mode),
            ..self
        }
    }

    pub(crate) fn is_default(&self) -> bool {
        !self.telemetry_enabled && self.auto_hide() == AutoHideMode::Never
    }
}

fn default_auto_hide() -> AutoHidePreference {
    AutoHidePreference::new(AutoHideMode::Never)
}

fn is_never(preference: &AutoHidePreference) -> bool {
    preference.mode() == AutoHideMode::Never
}

impl<'de> Deserialize<'de> for ToolbarPreferences {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            telemetry_enabled: bool,
            #[serde(default = "default_auto_hide")]
            auto_hide: AutoHidePreference,
        }

        struct ToolbarObject;

        impl<'de> Visitor<'de> for ToolbarObject {
            type Value = ToolbarPreferences;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a toolbar preferences object")
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let fields = Fields::deserialize(MapAccessDeserializer::new(map))?;
                Ok(ToolbarPreferences {
                    telemetry_enabled: fields.telemetry_enabled,
                    auto_hide: fields.auto_hide,
                })
            }
        }

        deserializer.deserialize_map(ToolbarObject)
    }
}
