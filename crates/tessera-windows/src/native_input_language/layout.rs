// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only registry metadata. This never activates an HKL to obtain its KLID.

pub(super) trait LayoutRegistry {
    fn keys(&self) -> Result<Vec<String>, u32>;
    fn value(&self, key: &str, name: &str) -> Option<String>;
    fn indirect(&self, source: &str) -> Option<String>;
}

fn klid<R: LayoutRegistry>(registry: &R, layout: usize) -> Option<String> {
    let bits = layout as u32; // HKL's documented low/high words, not pointer bits.
    let device = (bits >> 16) as u16;
    let language = bits as u16;
    if device & 0xf000 == 0xe000 {
        // Legacy IME identifiers retain their E-device word in the registry key.
        return Some(format!("{bits:08X}"));
    }
    if device & 0xf000 == 0xf000 {
        // Special keyboard devices encode a registry "Layout Id" in 12 bits.
        // Do not invent a language-based key if the installed mapping is absent
        // or ambiguous. KLID language can differ from the input language.
        let wanted = device & 0x0fff;
        let mut found = None;
        for key in registry.keys().ok()? {
            if key.len() != 8 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                continue;
            }
            let Some(value) = registry.value(&key, "Layout Id") else {
                continue;
            };
            if u16::from_str_radix(value.trim(), 16).ok() == Some(wanted) {
                if found.is_some() {
                    return None;
                }
                found = Some(key.to_ascii_uppercase());
            }
        }
        return found;
    }
    let identifier = if device == 0 { language } else { device };
    Some(format!("{identifier:08X}"))
}

pub(super) fn description<R: LayoutRegistry>(registry: &R, layout: usize) -> String {
    let Some(key) = klid(registry, layout) else {
        return String::new();
    };
    if let Some(source) = registry.value(&key, "Layout Display Name")
        && !source.is_empty()
        && let Some(label) = registry.indirect(&source).filter(|label| !label.is_empty())
    {
        return label;
    }
    registry.value(&key, "Layout Text").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Registry {
        values: HashMap<(String, String), String>,
        resolved: HashMap<String, String>,
    }
    impl Registry {
        fn put(&mut self, key: &str, name: &str, value: &str) {
            self.values.insert((key.into(), name.into()), value.into());
        }
    }
    impl LayoutRegistry for Registry {
        fn keys(&self) -> Result<Vec<String>, u32> {
            let mut keys: Vec<_> = self.values.keys().map(|(key, _)| key.clone()).collect();
            keys.sort();
            keys.dedup();
            Ok(keys)
        }
        fn value(&self, key: &str, name: &str) -> Option<String> {
            self.values.get(&(key.into(), name.into())).cloned()
        }
        fn indirect(&self, source: &str) -> Option<String> {
            self.resolved.get(source).cloned()
        }
    }

    #[test]
    fn ordinary_device_word_ime_and_special_registry_mapping() {
        let mut registry = Registry::default();
        registry.put("00000409", "Layout Text", "US");
        registry.put("00000407", "Layout Text", "Deutsch");
        registry.put("E0010411", "Layout Text", "日本語 IME");
        registry.put("00010409", "Layout Id", "0002");
        registry.put("00010409", "Layout Text", "US Dvorak");
        assert_eq!(description(&registry, 0x04090409), "US");
        assert_eq!(description(&registry, 0x04070409), "Deutsch");
        assert_eq!(description(&registry, 0xe0010411), "日本語 IME");
        assert_eq!(description(&registry, 0xf0020809), "US Dvorak");
        assert_eq!(description(&registry, 0xf0030409), "");
    }

    #[test]
    fn indirect_label_precedes_layout_text_and_failure_falls_back() {
        let mut registry = Registry::default();
        registry.put("00000409", "Layout Display Name", "@keyboard.dll,-123");
        registry.put("00000409", "Layout Text", "US");
        assert_eq!(description(&registry, 0x04090409), "US");
        registry
            .resolved
            .insert("@keyboard.dll,-123".into(), "Clavier — 日本語".into());
        assert_eq!(description(&registry, 0x04090409), "Clavier — 日本語");
        registry
            .resolved
            .insert("@keyboard.dll,-123".into(), String::new());
        assert_eq!(description(&registry, 0x04090409), "US");
    }

    #[test]
    fn ambiguous_special_layout_id_and_absent_metadata_stay_unavailable() {
        let mut registry = Registry::default();
        registry.put("00010409", "Layout Id", "0002");
        registry.put("00020409", "Layout Id", "0002");
        registry.put("00010409", "Layout Text", "Not a guessed fallback");
        assert_eq!(description(&registry, 0xf0020409), "");
        assert_eq!(description(&registry, 0x04090409), "");
    }
}
