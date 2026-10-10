// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DockMiddleClickAction {
    #[default]
    NewInstance,
    Minimize,
    Close,
}

impl<'de> Deserialize<'de> for DockMiddleClickAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ActionString;

        impl<'de> Visitor<'de> for ActionString {
            type Value = DockMiddleClickAction;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a dock middle-click action string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "new_instance" => Ok(DockMiddleClickAction::NewInstance),
                    "minimize" => Ok(DockMiddleClickAction::Minimize),
                    "close" => Ok(DockMiddleClickAction::Close),
                    _ => Err(E::unknown_variant(
                        value,
                        &["new_instance", "minimize", "close"],
                    )),
                }
            }
        }

        deserializer.deserialize_str(ActionString)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct DockPreferences {
    media_enabled: bool,
    #[serde(skip_serializing_if = "is_unlocked")]
    locked: bool,
    #[serde(skip_serializing_if = "is_new_instance")]
    middle_click: DockMiddleClickAction,
}

impl DockPreferences {
    pub(crate) fn media_enabled(&self) -> bool {
        self.media_enabled
    }

    pub(crate) fn with_media_enabled(self, media_enabled: bool) -> Self {
        Self {
            media_enabled,
            ..self
        }
    }

    pub(crate) fn locked(&self) -> bool {
        self.locked
    }

    pub(crate) fn with_locked(self, locked: bool) -> Self {
        Self { locked, ..self }
    }

    pub(crate) fn middle_click(&self) -> DockMiddleClickAction {
        self.middle_click
    }

    pub(crate) fn with_middle_click(self, middle_click: DockMiddleClickAction) -> Self {
        Self {
            middle_click,
            ..self
        }
    }
}

fn is_unlocked(locked: &bool) -> bool {
    !*locked
}

fn is_new_instance(action: &DockMiddleClickAction) -> bool {
    *action == DockMiddleClickAction::NewInstance
}

/// Older records admit only their original required media preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LegacyDockPreferences {
    media_enabled: bool,
}

impl LegacyDockPreferences {
    pub(crate) fn media_enabled(&self) -> bool {
        self.media_enabled
    }
}

impl From<LegacyDockPreferences> for DockPreferences {
    fn from(legacy: LegacyDockPreferences) -> Self {
        Self::default().with_media_enabled(legacy.media_enabled)
    }
}

impl<'de> Deserialize<'de> for LegacyDockPreferences {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            media_enabled: bool,
        }

        struct LegacyDockObject;

        impl<'de> Visitor<'de> for LegacyDockObject {
            type Value = LegacyDockPreferences;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a legacy dock preferences object")
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let fields = Fields::deserialize(MapAccessDeserializer::new(map))?;
                Ok(LegacyDockPreferences {
                    media_enabled: fields.media_enabled,
                })
            }
        }

        deserializer.deserialize_map(LegacyDockObject)
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
            #[serde(default)]
            locked: bool,
            #[serde(default)]
            middle_click: DockMiddleClickAction,
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
                    locked: fields.locked,
                    middle_click: fields.middle_click,
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
