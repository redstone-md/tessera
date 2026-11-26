// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is asked of Open-Meteo and how its answer reads.

use serde_json::Value;

/// Where the air is asked about, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    latitude: f64,
    longitude: f64,
}

impl Place {
    /// The place of the settings `latitude` and `longitude`.
    pub fn of(latitude: Option<&str>, longitude: Option<&str>) -> Result<Self, String> {
        let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
            return Err("Set `latitude` and `longitude` to say where.".to_owned());
        };
        let degrees = |text: &str, name: &str, limit: f64| {
            text.trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.abs() <= limit)
                .ok_or_else(|| {
                    format!(
                        "`{name}` is a number from -{limit} to {limit}, not `{}`.",
                        text.trim()
                    )
                })
        };
        Ok(Self {
            latitude: degrees(latitude, "latitude", 90.)?,
            longitude: degrees(longitude, "longitude", 180.)?,
        })
    }

    pub fn address(self) -> String {
        format!(
            "https://air-quality-api.open-meteo.com/v1/air-quality?latitude={:.2}&longitude={:.2}&current=european_aqi,pm2_5,pm10,ozone",
            self.latitude, self.longitude
        )
    }
}

/// The air at the place, in µg/m³ but for the index.
#[derive(Clone, Debug, PartialEq)]
pub struct Air {
    pub index: Option<f64>,
    pub pm2_5: Option<f64>,
    pub pm10: Option<f64>,
    pub ozone: Option<f64>,
    /// When it was measured or forecast, as `2026-11-26T22:00`, in UTC.
    pub time: String,
}

impl Air {
    pub fn read(body: &str) -> Result<Self, String> {
        let answer: Value = serde_json::from_str(body)
            .map_err(|_| "Open-Meteo's answer is unreadable.".to_owned())?;
        let current = &answer["current"];
        if !current.is_object() {
            return Err("Open-Meteo gave no air quality for the place.".to_owned());
        }
        Ok(Self {
            index: current["european_aqi"].as_f64(),
            pm2_5: current["pm2_5"].as_f64(),
            pm10: current["pm10"].as_f64(),
            ozone: current["ozone"].as_f64(),
            time: current["time"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// What a value of the European Air Quality Index means.
pub fn verdict(index: f64) -> &'static str {
    match index {
        ..=20. => "Good",
        ..=40. => "Fair",
        ..=60. => "Moderate",
        ..=80. => "Poor",
        ..=100. => "Very poor",
        _ => "Extremely poor",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_place_is_checked_and_rounded() {
        let place = Place::of(Some("52.5201"), Some(" 13.405")).unwrap();
        assert!(place.address().contains("latitude=52.52&longitude=13.40"));
        assert!(Place::of(None, Some("1")).is_err());
        assert!(
            Place::of(Some("91"), Some("1"))
                .unwrap_err()
                .contains("latitude")
        );
        assert!(
            Place::of(Some("1"), Some("east"))
                .unwrap_err()
                .contains("longitude")
        );
    }

    #[test]
    fn the_answer_is_read() {
        let body = r#"{"current":{"time":"2026-11-26T22:00","european_aqi":36,"pm2_5":13.0,"pm10":14.5,"ozone":21.0}}"#;
        let air = Air::read(body).unwrap();
        assert_eq!(air.index, Some(36.));
        assert_eq!((air.pm10, air.ozone), (Some(14.5), Some(21.)));
        assert_eq!(air.time, "2026-11-26T22:00");
        assert!(Air::read(r#"{"error":true}"#).is_err());
        assert!(Air::read("<html>").is_err());
    }

    #[test]
    fn index_values_have_names() {
        assert_eq!(verdict(12.), "Good");
        assert_eq!(verdict(36.), "Fair");
        assert_eq!(verdict(60.), "Moderate");
        assert_eq!(verdict(140.), "Extremely poor");
    }
}
