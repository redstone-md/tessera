// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize};

/// Pinned source choices, independent of the UI and native locale enums.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StartOfWeek {
    #[default]
    Monday,
    Sunday,
    Saturday,
}

impl<'de> Deserialize<'de> for StartOfWeek {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // A persisted choice is a string, never Serde's alternate tagged-unit map.
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "monday" => Ok(Self::Monday),
            "sunday" => Ok(Self::Sunday),
            "saturday" => Ok(Self::Saturday),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["monday", "sunday", "saturday"],
            )),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct GeneralPreferences {
    start_of_week: StartOfWeek,
}

impl GeneralPreferences {
    pub(crate) fn start_of_week(&self) -> StartOfWeek {
        self.start_of_week
    }

    pub(crate) fn with_start_of_week(self, start_of_week: StartOfWeek) -> Self {
        Self { start_of_week }
    }
}

impl<'de> Deserialize<'de> for GeneralPreferences {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            start_of_week: StartOfWeek,
        }

        struct GeneralObject;

        impl<'de> Visitor<'de> for GeneralObject {
            type Value = GeneralPreferences;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a general preferences object")
            }

            fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let fields = Fields::deserialize(MapAccessDeserializer::new(map))?;
                Ok(GeneralPreferences {
                    start_of_week: fields.start_of_week,
                })
            }
        }

        // Derived structs also accept positional arrays. Only a map may reach Fields.
        deserializer.deserialize_map(GeneralObject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_default_and_all_persisted_general_choices_round_trip() {
        assert_eq!(
            GeneralPreferences::default().start_of_week(),
            StartOfWeek::Monday
        );
        for (choice, name) in [
            (StartOfWeek::Monday, "monday"),
            (StartOfWeek::Sunday, "sunday"),
            (StartOfWeek::Saturday, "saturday"),
        ] {
            let preferences = GeneralPreferences::default().with_start_of_week(choice);
            let expected = format!(r#"{{"start_of_week":"{name}"}}"#);
            assert_eq!(serde_json::to_string(&preferences).unwrap(), expected);
            assert_eq!(
                serde_json::from_str::<GeneralPreferences>(&expected).unwrap(),
                preferences
            );
        }
    }

    #[test]
    fn general_is_a_required_strict_object_not_a_positional_or_partial_group() {
        for invalid in [
            "null",
            "[]",
            r#"["monday"]"#,
            r#""monday""#,
            "true",
            "0",
            "{}",
            r#"{"start_of_week":null}"#,
            r#"{"start_of_week":0}"#,
            r#"{"start_of_week":["monday"]}"#,
            r#"{"start_of_week":{"Monday":null}}"#,
            r#"{"start_of_week":{"monday":null}}"#,
            r#"{"start_of_week":true}"#,
            r#"{"start_of_week":"Monday"}"#,
            r#"{"start_of_week":"tuesday"}"#,
            r#"{"start_of_week":"system"}"#,
            r#"{"start_of_week":"monday","language":"en"}"#,
            r#"{"start_of_week":"monday","start_of_week":"sunday"}"#,
            r#"{"start_of_week":"monday","start_of_week":"monday"}"#,
        ] {
            assert!(
                serde_json::from_str::<GeneralPreferences>(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
