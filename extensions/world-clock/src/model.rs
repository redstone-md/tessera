// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Places and their fixed offsets from UTC.

use tessera_extension_kit::settings::parse_pairs;
use tessera_extension_kit::time::{Local, parse_offset};

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub name: String,
    /// Seconds off UTC.
    pub offset: i64,
}

impl Zone {
    pub fn local(&self, now: u64) -> Local {
        Local::at(now, self.offset)
    }
}

/// Reads places written as `Tokyo=+9; Mumbai=+5:30`, in that order.
pub fn parse_zones(text: &str) -> Result<Vec<Zone>, String> {
    parse_pairs(text)?
        .into_iter()
        .map(|(name, offset)| match parse_offset(&offset) {
            Some(offset) => Ok(Zone { name, offset }),
            None => Err(format!(
                "`{offset}` of {name} is not an offset such as +9, -5 or +5:30"
            )),
        })
        .collect()
}

/// How the day at a place stands to the day at home.
pub fn relative_day(place: i64, home: i64) -> String {
    match place - home {
        0 => "today".to_owned(),
        1 => "tomorrow".to_owned(),
        -1 => "yesterday".to_owned(),
        days => format!("{days:+} days"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_are_read_in_order() {
        let zones = parse_zones("Tokyo=+9; London=0; Adelaide=+9:30").unwrap();
        let names: Vec<&str> = zones.iter().map(|zone| &*zone.name).collect();
        assert_eq!(names, ["Tokyo", "London", "Adelaide"]);
        assert_eq!(zones[2].offset, 9 * 3600 + 1800);
        assert!(parse_zones("Mars=+30").unwrap_err().contains("Mars"));
    }

    #[test]
    fn days_are_told_apart_from_home() {
        // 2026-11-26 20:00 UTC.
        let now = 20_783 * 86_400 + 20 * 3600;
        let tokyo = Zone {
            name: "Tokyo".to_owned(),
            offset: 9 * 3600,
        };
        let local = tokyo.local(now);
        assert_eq!(local.clock(), "05:00");
        let home = Local::at(now, -5 * 3600).day;
        assert_eq!(relative_day(local.day, home), "tomorrow");
        assert_eq!(relative_day(home - 1, home), "yesterday");
        assert_eq!(relative_day(home, home), "today");
    }
}
