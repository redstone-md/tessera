// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ToolbarPreferences {
    telemetry_enabled: bool,
}

impl ToolbarPreferences {
    pub(crate) fn telemetry_enabled(&self) -> bool {
        self.telemetry_enabled
    }

    pub(crate) fn with_telemetry_enabled(mut self, enabled: bool) -> Self {
        self.telemetry_enabled = enabled;
        self
    }

    pub(crate) fn is_default(&self) -> bool {
        !self.telemetry_enabled
    }
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
                })
            }
        }

        deserializer.deserialize_map(ToolbarObject)
    }
}
