// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What Open-Meteo answers, and how it is put into words.

use serde_json::Value;

/// The scale temperatures are asked for and shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    Celsius,
    Fahrenheit,
}

impl Scale {
    /// What the setting `unit` names; nothing for a word it does not know.
    pub fn named(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "celsius" | "c" => Some(Self::Celsius),
            "fahrenheit" | "f" => Some(Self::Fahrenheit),
            _ => None,
        }
    }

    /// What Open-Meteo calls the scale.
    fn asked(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Fahrenheit => "fahrenheit",
        }
    }

    pub fn letter(self) -> &'static str {
        match self {
            Self::Celsius => "C",
            Self::Fahrenheit => "F",
        }
    }
}

/// A place on the Earth, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub latitude: f64,
    pub longitude: f64,
}

impl Place {
    /// The place the settings `latitude` and `longitude` name, or what to
    /// tell the user.
    pub fn of(latitude: Option<&str>, longitude: Option<&str>) -> Result<Self, String> {
        let degrees = |text: Option<&str>, most: f64| {
            text.and_then(|text| text.trim().trim_matches('"').parse::<f64>().ok())
                .filter(|degrees| degrees.abs() <= most)
        };
        match (degrees(latitude, 90.), degrees(longitude, 180.)) {
            (Some(latitude), Some(longitude)) => Ok(Self {
                latitude,
                longitude,
            }),
            _ => {
                Err("Set `latitude` and `longitude` in [extension.weather] of config.toml.".into())
            }
        }
    }

    /// The address that answers with the weather here. The place is given
    /// to a hundredth of a degree, about a kilometre, which is as fine as
    /// the forecast is.
    pub fn address(self, scale: Scale) -> String {
        format!(
            "https://api.open-meteo.com/v1/forecast?latitude={:.2}&longitude={:.2}\
             &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m\
             &daily=weather_code,temperature_2m_max,temperature_2m_min\
             &timezone=auto&forecast_days=4&temperature_unit={}",
            self.latitude,
            self.longitude,
            scale.asked()
        )
    }
}

/// The weather of one day.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    /// The date as `YYYY-MM-DD`, in the time of the place.
    pub date: String,
    pub code: u64,
    pub high: f64,
    pub low: f64,
}

/// The weather now and on the days that follow.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub temperature: f64,
    pub feels: f64,
    pub humidity: f64,
    pub wind: f64,
    /// The unit of `wind` as the answer names it.
    pub wind_unit: String,
    pub code: u64,
    /// Today first.
    pub days: Vec<Day>,
}

impl Report {
    /// Reads the answer of Open-Meteo.
    pub fn read(body: &str) -> Result<Self, String> {
        let answer: Value =
            serde_json::from_str(body).map_err(|_| "The answer is not understood.".to_owned())?;
        let missing = || "The answer lacks the weather.".to_owned();
        let current = &answer["current"];
        let number = |key: &str| current[key].as_f64().ok_or_else(missing);
        let daily = &answer["daily"];
        let dates = daily["time"].as_array().ok_or_else(missing)?;
        let days = dates
            .iter()
            .enumerate()
            .filter_map(|(day, date)| {
                Some(Day {
                    date: date.as_str()?.to_owned(),
                    code: daily["weather_code"][day].as_u64()?,
                    high: daily["temperature_2m_max"][day].as_f64()?,
                    low: daily["temperature_2m_min"][day].as_f64()?,
                })
            })
            .collect();
        Ok(Self {
            temperature: number("temperature_2m")?,
            feels: number("apparent_temperature")?,
            humidity: number("relative_humidity_2m")?,
            wind: number("wind_speed_10m")?,
            wind_unit: answer["current_units"]["wind_speed_10m"]
                .as_str()
                .unwrap_or("km/h")
                .to_owned(),
            code: current["weather_code"].as_u64().ok_or_else(missing)?,
            days,
        })
    }
}

/// The weather a code of the World Meteorological Organization stands
/// for, as Open-Meteo uses them.
pub fn sky(code: u64) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 | 77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm, hail",
        _ => "Unknown",
    }
}

/// A temperature in whole degrees, without a minus before a zero.
pub fn degrees(temperature: f64) -> String {
    let whole = temperature.round() as i64;
    format!("{whole}°")
}

/// What the day `place` days after today is called: its weekday from the
/// third day on.
pub fn day_name(place: usize, date: &str) -> &'static str {
    match place {
        0 => "Today",
        1 => "Tomorrow",
        _ => weekday(date).unwrap_or("Later"),
    }
}

/// The weekday of the date `YYYY-MM-DD`.
fn weekday(date: &str) -> Option<&'static str> {
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // The days since 1970-01-01, a Thursday, counted in years that start
    // in March so that the leap day comes last.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    const NAMES: [&str; 7] = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ];
    Some(NAMES[days.rem_euclid(7) as usize])
}

/// How long ago `then` was, seen from `now`, both in seconds.
pub fn ago(now: u64, then: u64) -> String {
    match now.saturating_sub(then) / 60 {
        0 => "just now".to_owned(),
        minutes @ 1..60 => format!("{minutes} min ago"),
        minutes => format!("{} h ago", minutes / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"current_units":{"wind_speed_10m":"km/h"},
        "current":{"temperature_2m":6.0,"apparent_temperature":2.0,
            "relative_humidity_2m":82,"weather_code":61,"wind_speed_10m":16.3},
        "daily":{"time":["2026-11-21","2026-11-22","2026-11-23"],
            "weather_code":[80,61,77],
            "temperature_2m_max":[6.7,4.9,3.2],
            "temperature_2m_min":[0.6,2.0,-0.6]}}"#;

    #[test]
    fn an_answer_is_read() {
        let report = Report::read(ANSWER).unwrap();
        assert_eq!(report.temperature, 6.);
        assert_eq!(report.feels, 2.);
        assert_eq!(report.humidity, 82.);
        assert_eq!(report.wind_unit, "km/h");
        assert_eq!(sky(report.code), "Rain");
        assert_eq!(report.days.len(), 3);
        assert_eq!(report.days[2].date, "2026-11-23");
        assert_eq!(report.days[2].low, -0.6);
    }

    #[test]
    fn an_answer_without_weather_is_refused() {
        assert!(Report::read("{}").is_err());
        assert!(Report::read("not json").is_err());
        assert!(Report::read(r#"{"error":true,"reason":"x"}"#).is_err());
    }

    #[test]
    fn a_place_needs_both_numbers_within_the_earth() {
        let place = Place::of(Some("52.52"), Some("13.41")).unwrap();
        assert_eq!(place.latitude, 52.52);
        assert!(Place::of(Some("\"-33.9\""), Some(" 151.2 ")).is_ok());
        assert!(Place::of(Some("52.52"), None).is_err());
        assert!(Place::of(Some("91"), Some("0")).is_err());
        assert!(Place::of(Some("0"), Some("181")).is_err());
        assert!(Place::of(Some("north"), Some("0")).is_err());
    }

    #[test]
    fn the_address_names_the_place_and_the_scale() {
        let place = Place {
            latitude: 52.5234,
            longitude: -13.4,
        };
        let address = place.address(Scale::Fahrenheit);
        assert!(address.starts_with("https://api.open-meteo.com/v1/forecast?latitude=52.52&"));
        assert!(address.contains("&longitude=-13.40&"));
        assert!(address.ends_with("&temperature_unit=fahrenheit"));
        assert!(!address.contains(' '));
    }

    #[test]
    fn scales_are_named() {
        assert_eq!(Scale::named("Fahrenheit"), Some(Scale::Fahrenheit));
        assert_eq!(Scale::named("c"), Some(Scale::Celsius));
        assert_eq!(Scale::named("kelvin"), None);
    }

    #[test]
    fn degrees_are_whole_and_zero_has_no_sign() {
        assert_eq!(degrees(6.7), "7°");
        assert_eq!(degrees(-0.4), "0°");
        assert_eq!(degrees(-1.5), "-2°");
    }

    #[test]
    fn days_are_named() {
        assert_eq!(day_name(0, "2026-11-21"), "Today");
        assert_eq!(day_name(1, "2026-11-22"), "Tomorrow");
        assert_eq!(day_name(2, "2026-11-23"), "Monday");
        assert_eq!(day_name(2, "1970-01-01"), "Thursday");
        assert_eq!(day_name(2, "2024-02-29"), "Thursday");
        assert_eq!(day_name(2, "2000-01-01"), "Saturday");
        assert_eq!(day_name(2, "soon"), "Later");
    }

    #[test]
    fn the_age_of_an_answer_is_told() {
        assert_eq!(ago(100, 90), "just now");
        assert_eq!(ago(1000, 400), "10 min ago");
        assert_eq!(ago(8000, 0), "2 h ago");
        assert_eq!(ago(0, 50), "just now");
    }
}
