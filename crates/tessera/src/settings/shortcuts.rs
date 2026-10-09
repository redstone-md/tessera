// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tessera_system::shortcuts::{KeyChord, KeyModifiers, ShortcutConfig};

#[cfg(test)]
mod tests;

/// Persistence owns only saved configuration, never capture, hook or profile state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ShortcutPreferences(ShortcutConfig);

impl ShortcutPreferences {
    pub(super) fn config(&self) -> &ShortcutConfig {
        &self.0
    }

    pub(super) fn new(config: ShortcutConfig) -> Self {
        Self(config)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredModifiers {
    control: bool,
    alt: bool,
    shift: bool,
    win: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredChord {
    key: u16,
    #[serde(deserialize_with = "deserialize_object")]
    modifiers: StoredModifiers,
}

/// Serde's derived structs also accept arrays. All persisted shortcut rows are maps.
fn deserialize_object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a shortcut configuration object")
        }

        fn visit_map<M>(self, map: M) -> Result<T, M::Error>
        where
            M: MapAccess<'de>,
        {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_map(Object(PhantomData))
}

struct NullableChord(Option<StoredChord>);

impl<'de> Deserialize<'de> for NullableChord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Nullable;

        impl<'de> Visitor<'de> for Nullable {
            type Value = NullableChord;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("null or a shortcut chord object")
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(NullableChord(None))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(NullableChord(None))
            }

            fn visit_some<D: Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                deserialize_object(deserializer).map(|chord| NullableChord(Some(chord)))
            }
        }

        deserializer.deserialize_option(Nullable)
    }
}

impl Serialize for ShortcutPreferences {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Fields {
            enabled: bool,
            settings_override: Option<StoredChord>,
        }

        let settings_override = match self.0.settings_override() {
            None => None,
            Some(KeyChord::Chord { key, modifiers }) => Some(StoredChord {
                key: *key,
                modifiers: StoredModifiers {
                    control: modifiers.control,
                    alt: modifiers.alt,
                    shift: modifiers.shift,
                    win: modifiers.win,
                },
            }),
            Some(KeyChord::BareWin) => {
                return Err(serde::ser::Error::custom(
                    "The launcher shortcut is read-only",
                ));
            }
        };
        Fields {
            enabled: self.0.enabled(),
            settings_override,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ShortcutPreferences {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ShortcutsObject;

        impl<'de> Visitor<'de> for ShortcutsObject {
            type Value = ShortcutPreferences;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a saved shortcuts object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut enabled = None;
                let mut settings_override = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "enabled" => {
                            if enabled.is_some() {
                                return Err(serde::de::Error::duplicate_field("enabled"));
                            }
                            enabled = Some(map.next_value::<bool>()?);
                        }
                        "settings_override" => {
                            if settings_override.is_some() {
                                return Err(serde::de::Error::duplicate_field("settings_override"));
                            }
                            settings_override = Some(map.next_value::<NullableChord>()?.0);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                &key,
                                &["enabled", "settings_override"],
                            ));
                        }
                    }
                }
                let enabled = enabled.ok_or_else(|| serde::de::Error::missing_field("enabled"))?;
                let stored = settings_override
                    .ok_or_else(|| serde::de::Error::missing_field("settings_override"))?;
                let chord = stored
                    .map(|chord| {
                        KeyChord::new(
                            KeyModifiers {
                                control: chord.modifiers.control,
                                alt: chord.modifiers.alt,
                                shift: chord.modifiers.shift,
                                win: chord.modifiers.win,
                            },
                            chord.key,
                        )
                    })
                    .transpose()
                    .map_err(serde::de::Error::custom)?;
                ShortcutConfig::default()
                    .with_enabled(enabled)
                    .with_settings_override(chord)
                    .map(ShortcutPreferences)
                    .map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_map(ShortcutsObject)
    }
}
