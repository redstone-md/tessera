// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct DockPreferences {
    media_enabled: bool,
}

impl DockPreferences {
    pub(crate) fn media_enabled(&self) -> bool {
        self.media_enabled
    }

    pub(crate) fn with_media_enabled(self, media_enabled: bool) -> Self {
        Self { media_enabled }
    }
}

impl<'de> Deserialize<'de> for DockPreferences {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            media_enabled: bool,
        }

        struct DockObject;

        impl<'de> Visitor<'de> for DockObject {
            type Value = DockPreferences;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a dock preferences object")
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let fields = Fields::deserialize(MapAccessDeserializer::new(map))?;
                Ok(DockPreferences {
                    media_enabled: fields.media_enabled,
                })
            }
        }

        // Do not admit Serde's positional struct-array representation on disk.
        deserializer.deserialize_map(DockObject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_defaults_disabled_and_both_persisted_values_round_trip() {
        let defaults = DockPreferences::default();
        assert!(!defaults.media_enabled());
        for enabled in [false, true] {
            let preferences = defaults.with_media_enabled(enabled);
            assert_eq!(preferences.media_enabled(), enabled);
            let expected = format!(r#"{{"media_enabled":{enabled}}}"#);
            assert_eq!(serde_json::to_string(&preferences).unwrap(), expected);
            assert_eq!(
                serde_json::from_str::<DockPreferences>(&expected).unwrap(),
                preferences
            );
        }
        assert!(!defaults.media_enabled());
    }

    #[test]
    fn dock_requires_one_boolean_field_in_a_strict_object() {
        for invalid in [
            "null",
            "[]",
            "[false]",
            "[true]",
            r#""false""#,
            "false",
            "0",
            "{}",
            r#"{"media_enabled":null}"#,
            r#"{"media_enabled":0}"#,
            r#"{"media_enabled":1}"#,
            r#"{"media_enabled":"false"}"#,
            r#"{"media_enabled":"true"}"#,
            r#"{"media_enabled":[]}"#,
            r#"{"media_enabled":{}}"#,
            r#"{"media_enabled":false,"extra":true}"#,
            r#"{"media_enabled":false,"media_enabled":true}"#,
            r#"{"media_enabled":false,"media_enabled":false}"#,
        ] {
            assert!(
                serde_json::from_str::<DockPreferences>(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
