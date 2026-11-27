// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Time zones by any of their names: the identifier of the IANA database
//! ("Europe/Berlin"), the name Windows gives the zone ("W. Europe Standard
//! Time"), or a city ("Berlin", "Tokyo", "São Paulo"), in any letters. The
//! one table of zones of Tessera: the clock, the calendar and the launcher
//! all ask here.
//!
//! The table pairs each zone of Windows with the IANA identifiers that the
//! Unicode CLDR maps to it (its `windowsZones` file), the first of them
//! the one CLDR names for the whole world; a short list adds cities that
//! name no zone themselves. What time it is in a zone is asked of Windows,
//! which keeps the rules of daylight saving time for every year; a zone
//! Windows does not have, as on an old installation, tells no time.

use chrono::{NaiveDateTime, Utc};

use crate::platform::SystemZone;

/// A time zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    /// The name Windows gives it, as "Tokyo Standard Time".
    pub windows_id: String,
    /// What the time zone settings of Windows call it, as "(UTC+09:00)
    /// Osaka, Sapporo, Tokyo"; the name Windows gives it where Windows
    /// does not have it.
    pub display: String,
    /// The IANA identifier it was found by, or else the one CLDR names for
    /// its Windows zone; `None` for a zone of Windows CLDR does not map.
    pub iana: Option<String>,
}

/// Finds the zone `name` names: an IANA identifier, a zone of Windows or a
/// city, in any letters, with spaces or underscores between words.
pub fn find(name: &str) -> Option<Zone> {
    let (windows_id, iana) = lookup(name)?;
    Some(zone(windows_id.to_owned(), iana.map(str::to_owned)))
}

/// Every zone Windows has, in its order, which is by offset; the zones of
/// the table where Windows gives none.
#[allow(dead_code, reason = "for the `time` runner of the launcher")]
pub fn all() -> Vec<Zone> {
    let system = SystemZone::all();
    if system.is_empty() {
        return { ZONES.iter() }
            .map(|(windows_id, ianas)| {
                zone(
                    (*windows_id).to_owned(),
                    ianas.first().copied().map(str::to_owned),
                )
            })
            .collect();
    }
    { system.iter() }
        .map(|system| Zone {
            iana: primary_iana(&system.id).map(str::to_owned),
            windows_id: system.id.clone(),
            display: system.display.clone(),
        })
        .collect()
}

/// The zone of Windows `windows_id`, with what Windows calls it.
fn zone(windows_id: String, iana: Option<String>) -> Zone {
    let display = SystemZone::named(&windows_id)
        .map_or_else(|| windows_id.clone(), |system| system.display.clone());
    Zone {
        windows_id,
        display,
        iana,
    }
}

impl Zone {
    /// The time of day in the zone now.
    pub fn now(&self) -> Option<NaiveDateTime> {
        self.local_of(Utc::now().naive_utc())
    }

    /// How many minutes the zone is ahead of universal time at the
    /// universal time `at_utc`, less than 0 where it is behind.
    pub fn offset_minutes(&self, at_utc: NaiveDateTime) -> Option<i32> {
        let local = self.local_of(at_utc)?;
        i32::try_from((local - at_utc).num_minutes()).ok()
    }

    /// The time of day in `to` when it is `local` in this zone.
    #[allow(dead_code, reason = "for the `time` runner of the launcher")]
    pub fn convert(&self, local: NaiveDateTime, to: &Zone) -> Option<NaiveDateTime> {
        let utc = SystemZone::named(&self.windows_id)?.utc_of(local)?;
        to.local_of(utc)
    }

    /// What the zone is called beside a time: the city of its IANA
    /// identifier, or what Windows calls it.
    pub fn label(&self) -> String {
        match &self.iana {
            Some(iana) if iana.contains('/') && !iana.starts_with("Etc/") => city_of(iana),
            _ => self.display.clone(),
        }
    }

    /// How far the zone is ahead of universal time now, as "UTC+05:30".
    pub fn offset_now(&self) -> Option<String> {
        let now = Utc::now().naive_utc();
        self.offset_minutes(now).map(offset_text)
    }

    fn local_of(&self, utc: NaiveDateTime) -> Option<NaiveDateTime> {
        SystemZone::named(&self.windows_id)?.local_of(utc)
    }
}

/// `minutes` ahead of universal time written as "UTC+05:30", "UTC-03:00"
/// or "UTC" for none.
pub fn offset_text(minutes: i32) -> String {
    if minutes == 0 {
        return "UTC".to_owned();
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let minutes = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// `name` as names are compared: in small letters, with a space where an
/// underscore or a hyphen is, and without the space around a slash.
fn key(name: &str) -> String {
    let spaced: String = { name.trim().chars() }
        .map(|letter| match letter {
            '_' | '-' => ' ',
            letter => letter,
        })
        .flat_map(char::to_lowercase)
        .collect();
    let words: Vec<&str> = spaced.split_whitespace().collect();
    words.join(" ").replace(" / ", "/")
}

/// The zone of Windows `name` names, with the IANA identifier it names or
/// stands for.
fn lookup(name: &str) -> Option<(&'static str, Option<&'static str>)> {
    let wanted = key(name);
    if wanted.is_empty() {
        return None;
    }
    // A zone of Windows by its name.
    if let Some((windows_id, ianas)) = ZONES
        .iter()
        .find(|(windows_id, _)| key(windows_id) == wanted)
    {
        return Some((windows_id, ianas.first().copied()));
    }
    // An IANA identifier, or one of the old names that stand for one.
    let iana = { ALIASES.iter() }
        .find(|(alias, _)| key(alias) == wanted)
        .map(|(_, iana)| *iana);
    let iana = iana.or_else(|| {
        { ZONES.iter() }
            .flat_map(|(_, ianas)| ianas.iter())
            .find(|iana| key(iana) == wanted)
            .copied()
    });
    if let Some(iana) = iana {
        return windows_of(iana).map(|windows_id| (windows_id, Some(iana)));
    }
    // A city: one of the list, or the city of an identifier.
    let city = { CITIES.iter() }
        .find(|(city, _)| key(city) == wanted)
        .map(|(_, iana)| *iana);
    let city = city.or_else(|| {
        { ZONES.iter() }
            .flat_map(|(_, ianas)| ianas.iter())
            .filter(|iana| !iana.starts_with("Etc/"))
            .find(|iana| key(&city_of(iana)) == wanted)
            .copied()
    });
    city.and_then(|iana| windows_of(iana).map(|windows_id| (windows_id, Some(iana))))
}

/// The zone of Windows CLDR maps the IANA identifier `iana` to.
fn windows_of(iana: &str) -> Option<&'static str> {
    { ZONES.iter() }
        .find(|(_, ianas)| ianas.contains(&iana))
        .map(|(windows_id, _)| *windows_id)
}

/// The IANA identifier CLDR names for the zone of Windows `windows_id` for
/// the whole world.
fn primary_iana(windows_id: &str) -> Option<&'static str> {
    { ZONES.iter() }
        .find(|(id, _)| id.eq_ignore_ascii_case(windows_id))
        .and_then(|(_, ianas)| ianas.first().copied())
}

/// The city of the IANA identifier `iana`, as CLDR writes it: its last
/// part with spaces, under the name of today where the identifier keeps
/// an old one.
fn city_of(iana: &str) -> String {
    if let Some((_, city)) = RENAMED.iter().find(|(old, _)| *old == iana) {
        return (*city).to_owned();
    }
    iana.rsplit('/').next().unwrap_or(iana).replace('_', " ")
}

/// The identifiers whose city CLDR calls by another name.
const RENAMED: &[(&str, &str)] = &[
    ("Asia/Calcutta", "Kolkata"),
    ("Asia/Saigon", "Ho Chi Minh City"),
    ("Asia/Katmandu", "Kathmandu"),
    ("Asia/Rangoon", "Yangon"),
    ("Europe/Kiev", "Kyiv"),
    ("America/Godthab", "Nuuk"),
    ("Pacific/Truk", "Chuuk"),
    ("Pacific/Ponape", "Pohnpei"),
    ("Africa/Asmera", "Asmara"),
    ("Atlantic/Faeroe", "Faroe"),
    ("America/Coral_Harbour", "Atikokan"),
    ("Pacific/Enderbury", "Kanton"),
    ("America/Buenos_Aires", "Buenos Aires"),
    ("America/Indianapolis", "Indianapolis"),
    ("America/Louisville", "Louisville"),
    ("Asia/Macau", "Macao"),
    ("America/St_Johns", "St. John’s"),
    ("Europe/Busingen", "Büsingen"),
    ("America/Sao_Paulo", "São Paulo"),
    ("America/Bogota", "Bogotá"),
    ("Atlantic/Reykjavik", "Reykjavík"),
    ("Europe/Zurich", "Zürich"),
    ("America/Cancun", "Cancún"),
    ("America/Merida", "Mérida"),
    ("America/Asuncion", "Asunción"),
    ("America/Cuiaba", "Cuiabá"),
    ("America/Belem", "Belém"),
    ("America/Maceio", "Maceió"),
    ("America/Araguaina", "Araguaína"),
    ("America/Santarem", "Santarém"),
    ("America/Curacao", "Curaçao"),
    ("America/St_Barthelemy", "St. Barthélemy"),
    ("Africa/Sao_Tome", "São Tomé"),
    ("Africa/Lome", "Lomé"),
    ("Africa/Ndjamena", "N’Djamena"),
    ("Pacific/Galapagos", "Galápagos"),
    ("America/Port-au-Prince", "Port-au-Prince"),
    ("America/Ciudad_Juarez", "Ciudad Juárez"),
];

/// Cities that name no zone of their own, with the zone they are in.
const CITIES: &[(&str, &str)] = &[
    ("Washington", "America/New_York"),
    ("Boston", "America/New_York"),
    ("Philadelphia", "America/New_York"),
    ("Atlanta", "America/New_York"),
    ("Miami", "America/New_York"),
    ("Montreal", "America/Toronto"),
    ("Ottawa", "America/Toronto"),
    ("San Francisco", "America/Los_Angeles"),
    ("Seattle", "America/Los_Angeles"),
    ("San Diego", "America/Los_Angeles"),
    ("Las Vegas", "America/Los_Angeles"),
    ("Portland", "America/Los_Angeles"),
    ("Dallas", "America/Chicago"),
    ("Houston", "America/Chicago"),
    ("Austin", "America/Chicago"),
    ("Salt Lake City", "America/Denver"),
    ("Calgary", "America/Edmonton"),
    ("Rio de Janeiro", "America/Sao_Paulo"),
    ("Brasilia", "America/Sao_Paulo"),
    ("Brasília", "America/Sao_Paulo"),
    ("Munich", "Europe/Berlin"),
    ("München", "Europe/Berlin"),
    ("Hamburg", "Europe/Berlin"),
    ("Frankfurt", "Europe/Berlin"),
    ("Cologne", "Europe/Berlin"),
    ("Köln", "Europe/Berlin"),
    ("Geneva", "Europe/Zurich"),
    ("Milan", "Europe/Rome"),
    ("Barcelona", "Europe/Madrid"),
    ("Lyon", "Europe/Paris"),
    ("Krakow", "Europe/Warsaw"),
    ("Kraków", "Europe/Warsaw"),
    ("Kiev", "Europe/Kyiv"),
    ("Kharkiv", "Europe/Kyiv"),
    ("Odesa", "Europe/Kyiv"),
    ("Saint Petersburg", "Europe/Moscow"),
    ("St. Petersburg", "Europe/Moscow"),
    ("Ankara", "Europe/Istanbul"),
    ("Tel Aviv", "Asia/Jerusalem"),
    ("Abu Dhabi", "Asia/Dubai"),
    ("Mecca", "Asia/Riyadh"),
    ("Jeddah", "Asia/Riyadh"),
    ("Doha", "Asia/Qatar"),
    ("Cape Town", "Africa/Johannesburg"),
    ("Delhi", "Asia/Calcutta"),
    ("New Delhi", "Asia/Calcutta"),
    ("Mumbai", "Asia/Calcutta"),
    ("Bangalore", "Asia/Calcutta"),
    ("Bengaluru", "Asia/Calcutta"),
    ("Chennai", "Asia/Calcutta"),
    ("Hyderabad", "Asia/Calcutta"),
    ("Kolkata", "Asia/Calcutta"),
    ("Beijing", "Asia/Shanghai"),
    ("Shenzhen", "Asia/Shanghai"),
    ("Guangzhou", "Asia/Shanghai"),
    ("Chongqing", "Asia/Shanghai"),
    ("Hanoi", "Asia/Saigon"),
    ("Ho Chi Minh City", "Asia/Saigon"),
    ("Osaka", "Asia/Tokyo"),
    ("Kyoto", "Asia/Tokyo"),
    ("Busan", "Asia/Seoul"),
    ("Canberra", "Australia/Sydney"),
    ("Wellington", "Pacific/Auckland"),
    ("UTC", "Etc/UTC"),
    ("GMT", "Etc/GMT"),
];

/// Identifiers of the IANA database that are links to another one, and
/// the identifier the table has for it.
const ALIASES: &[(&str, &str)] = &[
    ("UTC", "Etc/UTC"),
    ("GMT", "Etc/GMT"),
    ("Etc/Universal", "Etc/UTC"),
    ("Etc/Zulu", "Etc/UTC"),
    ("Asia/Kolkata", "Asia/Calcutta"),
    ("Asia/Ho_Chi_Minh", "Asia/Saigon"),
    ("Asia/Kathmandu", "Asia/Katmandu"),
    ("Asia/Yangon", "Asia/Rangoon"),
    ("America/Nuuk", "America/Godthab"),
    ("Pacific/Chuuk", "Pacific/Truk"),
    ("Pacific/Pohnpei", "Pacific/Ponape"),
    ("Africa/Asmara", "Africa/Asmera"),
    ("Atlantic/Faroe", "Atlantic/Faeroe"),
    ("America/Atikokan", "America/Coral_Harbour"),
    ("Pacific/Kanton", "Pacific/Enderbury"),
    ("America/Argentina/Buenos_Aires", "America/Buenos_Aires"),
    ("America/Indiana/Indianapolis", "America/Indianapolis"),
    ("America/Kentucky/Louisville", "America/Louisville"),
    ("Asia/Istanbul", "Europe/Istanbul"),
    ("Asia/Tel_Aviv", "Asia/Jerusalem"),
    ("Asia/Ulan_Bator", "Asia/Ulaanbaatar"),
    ("US/Eastern", "America/New_York"),
    ("US/Central", "America/Chicago"),
    ("US/Mountain", "America/Denver"),
    ("US/Pacific", "America/Los_Angeles"),
    ("US/Alaska", "America/Anchorage"),
    ("US/Hawaii", "Pacific/Honolulu"),
    ("US/Arizona", "America/Phoenix"),
    ("Canada/Eastern", "America/Toronto"),
    ("Canada/Pacific", "America/Vancouver"),
    ("Europe/Uzhgorod", "Europe/Kyiv"),
    ("Europe/Zaporozhye", "Europe/Kyiv"),
    ("Australia/ACT", "Australia/Sydney"),
    ("Australia/NSW", "Australia/Sydney"),
    ("Australia/Victoria", "Australia/Melbourne"),
    ("Australia/Queensland", "Australia/Brisbane"),
    ("Australia/West", "Australia/Perth"),
    ("Australia/South", "Australia/Adelaide"),
    ("Australia/North", "Australia/Darwin"),
    ("Australia/Tasmania", "Australia/Hobart"),
    ("NZ", "Pacific/Auckland"),
    ("Japan", "Asia/Tokyo"),
    ("PRC", "Asia/Shanghai"),
    ("ROK", "Asia/Seoul"),
    ("Singapore", "Asia/Singapore"),
    ("Hongkong", "Asia/Hong_Kong"),
    ("Iceland", "Atlantic/Reykjavik"),
    ("Egypt", "Africa/Cairo"),
    ("Israel", "Asia/Jerusalem"),
    ("Iran", "Asia/Tehran"),
    ("Turkey", "Europe/Istanbul"),
    ("Poland", "Europe/Warsaw"),
    ("Portugal", "Europe/Lisbon"),
    ("Eire", "Europe/Dublin"),
    ("GB", "Europe/London"),
    ("W-SU", "Europe/Moscow"),
    ("Cuba", "America/Havana"),
    ("Jamaica", "America/Jamaica"),
];

/// The zones of Windows, each with the IANA identifiers CLDR maps to it,
/// the one for the whole world first.
const ZONES: &[(&str, &[&str])] = &[
    ("Dateline Standard Time", &["Etc/GMT+12"]),
    (
        "UTC-11",
        &[
            "Etc/GMT+11",
            "Pacific/Pago_Pago",
            "Pacific/Niue",
            "Pacific/Midway",
        ],
    ),
    ("Aleutian Standard Time", &["America/Adak"]),
    (
        "Hawaiian Standard Time",
        &[
            "Pacific/Honolulu",
            "Pacific/Rarotonga",
            "Pacific/Tahiti",
            "Etc/GMT+10",
        ],
    ),
    ("Marquesas Standard Time", &["Pacific/Marquesas"]),
    (
        "Alaskan Standard Time",
        &[
            "America/Anchorage",
            "America/Juneau",
            "America/Metlakatla",
            "America/Nome",
            "America/Sitka",
            "America/Yakutat",
        ],
    ),
    ("UTC-09", &["Etc/GMT+9", "Pacific/Gambier"]),
    ("Pacific Standard Time (Mexico)", &["America/Tijuana"]),
    ("UTC-08", &["Etc/GMT+8", "Pacific/Pitcairn"]),
    (
        "Pacific Standard Time",
        &["America/Los_Angeles", "America/Vancouver", "PST8PDT"],
    ),
    (
        "US Mountain Standard Time",
        &[
            "America/Phoenix",
            "America/Creston",
            "America/Dawson_Creek",
            "America/Fort_Nelson",
            "America/Hermosillo",
            "Etc/GMT+7",
        ],
    ),
    ("Mountain Standard Time (Mexico)", &["America/Mazatlan"]),
    (
        "Mountain Standard Time",
        &[
            "America/Denver",
            "America/Edmonton",
            "America/Cambridge_Bay",
            "America/Inuvik",
            "America/Ciudad_Juarez",
            "America/Boise",
            "MST7MDT",
        ],
    ),
    (
        "Yukon Standard Time",
        &["America/Whitehorse", "America/Dawson"],
    ),
    (
        "Central America Standard Time",
        &[
            "America/Guatemala",
            "America/Belize",
            "America/Costa_Rica",
            "Pacific/Galapagos",
            "America/Tegucigalpa",
            "America/Managua",
            "America/El_Salvador",
            "Etc/GMT+6",
        ],
    ),
    (
        "Central Standard Time",
        &[
            "America/Chicago",
            "America/Winnipeg",
            "America/Rankin_Inlet",
            "America/Resolute",
            "America/Matamoros",
            "America/Indiana/Knox",
            "America/Indiana/Tell_City",
            "America/Menominee",
            "America/North_Dakota/Beulah",
            "America/North_Dakota/Center",
            "America/North_Dakota/New_Salem",
            "CST6CDT",
        ],
    ),
    ("Easter Island Standard Time", &["Pacific/Easter"]),
    (
        "Central Standard Time (Mexico)",
        &[
            "America/Mexico_City",
            "America/Bahia_Banderas",
            "America/Merida",
            "America/Monterrey",
            "America/Chihuahua",
        ],
    ),
    (
        "Canada Central Standard Time",
        &["America/Regina", "America/Swift_Current"],
    ),
    (
        "SA Pacific Standard Time",
        &[
            "America/Bogota",
            "America/Rio_Branco",
            "America/Eirunepe",
            "America/Coral_Harbour",
            "America/Guayaquil",
            "America/Jamaica",
            "America/Cayman",
            "America/Panama",
            "America/Lima",
            "Etc/GMT+5",
        ],
    ),
    ("Eastern Standard Time (Mexico)", &["America/Cancun"]),
    (
        "Eastern Standard Time",
        &[
            "America/New_York",
            "America/Nassau",
            "America/Toronto",
            "America/Iqaluit",
            "America/Detroit",
            "America/Indiana/Petersburg",
            "America/Indiana/Vincennes",
            "America/Indiana/Winamac",
            "America/Kentucky/Monticello",
            "America/Louisville",
            "EST5EDT",
        ],
    ),
    ("Haiti Standard Time", &["America/Port-au-Prince"]),
    ("Cuba Standard Time", &["America/Havana"]),
    (
        "US Eastern Standard Time",
        &[
            "America/Indianapolis",
            "America/Indiana/Marengo",
            "America/Indiana/Vevay",
        ],
    ),
    ("Turks And Caicos Standard Time", &["America/Grand_Turk"]),
    ("Paraguay Standard Time", &["America/Asuncion"]),
    (
        "Atlantic Standard Time",
        &[
            "America/Halifax",
            "Atlantic/Bermuda",
            "America/Glace_Bay",
            "America/Goose_Bay",
            "America/Moncton",
            "America/Thule",
        ],
    ),
    ("Venezuela Standard Time", &["America/Caracas"]),
    (
        "Central Brazilian Standard Time",
        &["America/Cuiaba", "America/Campo_Grande"],
    ),
    (
        "SA Western Standard Time",
        &[
            "America/La_Paz",
            "America/Antigua",
            "America/Anguilla",
            "America/Aruba",
            "America/Barbados",
            "America/St_Barthelemy",
            "America/Kralendijk",
            "America/Manaus",
            "America/Boa_Vista",
            "America/Porto_Velho",
            "America/Blanc-Sablon",
            "America/Curacao",
            "America/Dominica",
            "America/Santo_Domingo",
            "America/Grenada",
            "America/Guadeloupe",
            "America/Guyana",
            "America/St_Kitts",
            "America/St_Lucia",
            "America/Marigot",
            "America/Martinique",
            "America/Montserrat",
            "America/Puerto_Rico",
            "America/Lower_Princes",
            "America/Port_of_Spain",
            "America/St_Vincent",
            "America/Tortola",
            "America/St_Thomas",
            "Etc/GMT+4",
        ],
    ),
    ("Pacific SA Standard Time", &["America/Santiago"]),
    ("Newfoundland Standard Time", &["America/St_Johns"]),
    ("Tocantins Standard Time", &["America/Araguaina"]),
    ("E. South America Standard Time", &["America/Sao_Paulo"]),
    (
        "SA Eastern Standard Time",
        &[
            "America/Cayenne",
            "Antarctica/Rothera",
            "Antarctica/Palmer",
            "America/Fortaleza",
            "America/Belem",
            "America/Maceio",
            "America/Recife",
            "America/Santarem",
            "Atlantic/Stanley",
            "America/Paramaribo",
            "Etc/GMT+3",
        ],
    ),
    (
        "Argentina Standard Time",
        &[
            "America/Buenos_Aires",
            "America/Argentina/La_Rioja",
            "America/Argentina/Rio_Gallegos",
            "America/Argentina/Salta",
            "America/Argentina/San_Juan",
            "America/Argentina/San_Luis",
            "America/Argentina/Tucuman",
            "America/Argentina/Ushuaia",
            "America/Catamarca",
            "America/Cordoba",
            "America/Jujuy",
            "America/Mendoza",
        ],
    ),
    ("Greenland Standard Time", &["America/Godthab"]),
    ("Montevideo Standard Time", &["America/Montevideo"]),
    ("Magallanes Standard Time", &["America/Punta_Arenas"]),
    ("Saint Pierre Standard Time", &["America/Miquelon"]),
    ("Bahia Standard Time", &["America/Bahia"]),
    (
        "UTC-02",
        &["Etc/GMT+2", "America/Noronha", "Atlantic/South_Georgia"],
    ),
    (
        "Azores Standard Time",
        &["Atlantic/Azores", "America/Scoresbysund"],
    ),
    (
        "Cape Verde Standard Time",
        &["Atlantic/Cape_Verde", "Etc/GMT+1"],
    ),
    ("UTC", &["Etc/UTC", "Etc/GMT", "America/Danmarkshavn"]),
    (
        "GMT Standard Time",
        &[
            "Europe/London",
            "Atlantic/Canary",
            "Atlantic/Faeroe",
            "Europe/Guernsey",
            "Europe/Dublin",
            "Europe/Isle_of_Man",
            "Europe/Jersey",
            "Europe/Lisbon",
            "Atlantic/Madeira",
        ],
    ),
    (
        "Greenwich Standard Time",
        &[
            "Atlantic/Reykjavik",
            "Africa/Ouagadougou",
            "Africa/Abidjan",
            "Africa/Accra",
            "Africa/Banjul",
            "Africa/Conakry",
            "Africa/Bissau",
            "Africa/Monrovia",
            "Africa/Bamako",
            "Africa/Nouakchott",
            "Atlantic/St_Helena",
            "Africa/Freetown",
            "Africa/Dakar",
            "Africa/Lome",
        ],
    ),
    ("Sao Tome Standard Time", &["Africa/Sao_Tome"]),
    (
        "Morocco Standard Time",
        &["Africa/Casablanca", "Africa/El_Aaiun"],
    ),
    (
        "W. Europe Standard Time",
        &[
            "Europe/Berlin",
            "Europe/Andorra",
            "Europe/Vienna",
            "Europe/Zurich",
            "Europe/Busingen",
            "Europe/Gibraltar",
            "Europe/Rome",
            "Europe/Vaduz",
            "Europe/Luxembourg",
            "Europe/Monaco",
            "Europe/Malta",
            "Europe/Amsterdam",
            "Europe/Oslo",
            "Europe/Stockholm",
            "Arctic/Longyearbyen",
            "Europe/San_Marino",
            "Europe/Vatican",
        ],
    ),
    (
        "Central Europe Standard Time",
        &[
            "Europe/Budapest",
            "Europe/Tirane",
            "Europe/Prague",
            "Europe/Podgorica",
            "Europe/Belgrade",
            "Europe/Ljubljana",
            "Europe/Bratislava",
        ],
    ),
    (
        "Romance Standard Time",
        &[
            "Europe/Paris",
            "Europe/Brussels",
            "Europe/Copenhagen",
            "Europe/Madrid",
            "Africa/Ceuta",
        ],
    ),
    (
        "Central European Standard Time",
        &[
            "Europe/Warsaw",
            "Europe/Sarajevo",
            "Europe/Zagreb",
            "Europe/Skopje",
        ],
    ),
    (
        "W. Central Africa Standard Time",
        &[
            "Africa/Lagos",
            "Africa/Luanda",
            "Africa/Porto-Novo",
            "Africa/Kinshasa",
            "Africa/Bangui",
            "Africa/Brazzaville",
            "Africa/Douala",
            "Africa/Algiers",
            "Africa/Libreville",
            "Africa/Malabo",
            "Africa/Niamey",
            "Africa/Ndjamena",
            "Africa/Tunis",
            "Etc/GMT-1",
        ],
    ),
    ("Jordan Standard Time", &["Asia/Amman"]),
    (
        "GTB Standard Time",
        &[
            "Europe/Bucharest",
            "Asia/Nicosia",
            "Asia/Famagusta",
            "Europe/Athens",
        ],
    ),
    ("Middle East Standard Time", &["Asia/Beirut"]),
    ("Egypt Standard Time", &["Africa/Cairo"]),
    ("E. Europe Standard Time", &["Europe/Chisinau"]),
    ("Syria Standard Time", &["Asia/Damascus"]),
    ("West Bank Standard Time", &["Asia/Hebron", "Asia/Gaza"]),
    (
        "South Africa Standard Time",
        &[
            "Africa/Johannesburg",
            "Africa/Bujumbura",
            "Africa/Gaborone",
            "Africa/Lubumbashi",
            "Africa/Maseru",
            "Africa/Blantyre",
            "Africa/Maputo",
            "Africa/Kigali",
            "Africa/Mbabane",
            "Africa/Lusaka",
            "Africa/Harare",
            "Etc/GMT-2",
        ],
    ),
    (
        "FLE Standard Time",
        &[
            "Europe/Kiev",
            "Europe/Kyiv",
            "Europe/Mariehamn",
            "Europe/Sofia",
            "Europe/Tallinn",
            "Europe/Helsinki",
            "Europe/Vilnius",
            "Europe/Riga",
        ],
    ),
    ("Israel Standard Time", &["Asia/Jerusalem"]),
    ("South Sudan Standard Time", &["Africa/Juba"]),
    ("Kaliningrad Standard Time", &["Europe/Kaliningrad"]),
    ("Sudan Standard Time", &["Africa/Khartoum"]),
    ("Libya Standard Time", &["Africa/Tripoli"]),
    ("Namibia Standard Time", &["Africa/Windhoek"]),
    ("Arabic Standard Time", &["Asia/Baghdad"]),
    ("Turkey Standard Time", &["Europe/Istanbul"]),
    (
        "Arab Standard Time",
        &[
            "Asia/Riyadh",
            "Asia/Bahrain",
            "Asia/Kuwait",
            "Asia/Qatar",
            "Asia/Aden",
        ],
    ),
    ("Belarus Standard Time", &["Europe/Minsk"]),
    (
        "Russian Standard Time",
        &["Europe/Moscow", "Europe/Kirov", "Europe/Simferopol"],
    ),
    (
        "E. Africa Standard Time",
        &[
            "Africa/Nairobi",
            "Antarctica/Syowa",
            "Africa/Djibouti",
            "Africa/Asmera",
            "Africa/Addis_Ababa",
            "Indian/Comoro",
            "Indian/Antananarivo",
            "Africa/Mogadishu",
            "Africa/Dar_es_Salaam",
            "Africa/Kampala",
            "Indian/Mayotte",
            "Etc/GMT-3",
        ],
    ),
    ("Volgograd Standard Time", &["Europe/Volgograd"]),
    ("Iran Standard Time", &["Asia/Tehran"]),
    (
        "Arabian Standard Time",
        &["Asia/Dubai", "Asia/Muscat", "Etc/GMT-4"],
    ),
    (
        "Astrakhan Standard Time",
        &["Europe/Astrakhan", "Europe/Ulyanovsk"],
    ),
    ("Azerbaijan Standard Time", &["Asia/Baku"]),
    ("Russia Time Zone 3", &["Europe/Samara"]),
    (
        "Mauritius Standard Time",
        &["Indian/Mauritius", "Indian/Reunion", "Indian/Mahe"],
    ),
    ("Saratov Standard Time", &["Europe/Saratov"]),
    ("Georgian Standard Time", &["Asia/Tbilisi"]),
    ("Caucasus Standard Time", &["Asia/Yerevan"]),
    ("Afghanistan Standard Time", &["Asia/Kabul"]),
    (
        "West Asia Standard Time",
        &[
            "Asia/Tashkent",
            "Antarctica/Mawson",
            "Asia/Oral",
            "Asia/Aqtau",
            "Asia/Aqtobe",
            "Asia/Atyrau",
            "Indian/Maldives",
            "Indian/Kerguelen",
            "Asia/Dushanbe",
            "Asia/Ashgabat",
            "Asia/Samarkand",
            "Etc/GMT-5",
        ],
    ),
    ("Ekaterinburg Standard Time", &["Asia/Yekaterinburg"]),
    ("Pakistan Standard Time", &["Asia/Karachi"]),
    ("Qyzylorda Standard Time", &["Asia/Qyzylorda"]),
    ("India Standard Time", &["Asia/Calcutta"]),
    ("Sri Lanka Standard Time", &["Asia/Colombo"]),
    ("Nepal Standard Time", &["Asia/Katmandu"]),
    (
        "Central Asia Standard Time",
        &[
            "Asia/Bishkek",
            "Asia/Almaty",
            "Asia/Qostanay",
            "Antarctica/Vostok",
            "Asia/Urumqi",
            "Indian/Chagos",
            "Etc/GMT-6",
        ],
    ),
    ("Bangladesh Standard Time", &["Asia/Dhaka", "Asia/Thimphu"]),
    ("Omsk Standard Time", &["Asia/Omsk"]),
    ("Myanmar Standard Time", &["Asia/Rangoon", "Indian/Cocos"]),
    (
        "SE Asia Standard Time",
        &[
            "Asia/Bangkok",
            "Antarctica/Davis",
            "Indian/Christmas",
            "Asia/Jakarta",
            "Asia/Pontianak",
            "Asia/Phnom_Penh",
            "Asia/Vientiane",
            "Asia/Saigon",
            "Etc/GMT-7",
        ],
    ),
    ("Altai Standard Time", &["Asia/Barnaul"]),
    ("W. Mongolia Standard Time", &["Asia/Hovd"]),
    (
        "North Asia Standard Time",
        &["Asia/Krasnoyarsk", "Asia/Novokuznetsk"],
    ),
    ("N. Central Asia Standard Time", &["Asia/Novosibirsk"]),
    ("Tomsk Standard Time", &["Asia/Tomsk"]),
    (
        "China Standard Time",
        &["Asia/Shanghai", "Asia/Hong_Kong", "Asia/Macau"],
    ),
    ("North Asia East Standard Time", &["Asia/Irkutsk"]),
    (
        "Singapore Standard Time",
        &[
            "Asia/Singapore",
            "Asia/Brunei",
            "Asia/Makassar",
            "Asia/Kuala_Lumpur",
            "Asia/Kuching",
            "Asia/Manila",
            "Etc/GMT-8",
        ],
    ),
    ("W. Australia Standard Time", &["Australia/Perth"]),
    ("Taipei Standard Time", &["Asia/Taipei"]),
    (
        "Ulaanbaatar Standard Time",
        &["Asia/Ulaanbaatar", "Asia/Choibalsan"],
    ),
    ("Aus Central W. Standard Time", &["Australia/Eucla"]),
    ("Transbaikal Standard Time", &["Asia/Chita"]),
    (
        "Tokyo Standard Time",
        &[
            "Asia/Tokyo",
            "Asia/Jayapura",
            "Pacific/Palau",
            "Asia/Dili",
            "Etc/GMT-9",
        ],
    ),
    ("North Korea Standard Time", &["Asia/Pyongyang"]),
    ("Korea Standard Time", &["Asia/Seoul"]),
    ("Yakutsk Standard Time", &["Asia/Yakutsk", "Asia/Khandyga"]),
    (
        "Cen. Australia Standard Time",
        &["Australia/Adelaide", "Australia/Broken_Hill"],
    ),
    ("AUS Central Standard Time", &["Australia/Darwin"]),
    (
        "E. Australia Standard Time",
        &["Australia/Brisbane", "Australia/Lindeman"],
    ),
    (
        "AUS Eastern Standard Time",
        &["Australia/Sydney", "Australia/Melbourne"],
    ),
    (
        "West Pacific Standard Time",
        &[
            "Pacific/Port_Moresby",
            "Antarctica/DumontDUrville",
            "Pacific/Truk",
            "Pacific/Guam",
            "Pacific/Saipan",
            "Etc/GMT-10",
        ],
    ),
    (
        "Tasmania Standard Time",
        &["Australia/Hobart", "Antarctica/Macquarie"],
    ),
    (
        "Vladivostok Standard Time",
        &["Asia/Vladivostok", "Asia/Ust-Nera"],
    ),
    ("Lord Howe Standard Time", &["Australia/Lord_Howe"]),
    ("Bougainville Standard Time", &["Pacific/Bougainville"]),
    ("Russia Time Zone 10", &["Asia/Srednekolymsk"]),
    ("Magadan Standard Time", &["Asia/Magadan"]),
    ("Norfolk Standard Time", &["Pacific/Norfolk"]),
    ("Sakhalin Standard Time", &["Asia/Sakhalin"]),
    (
        "Central Pacific Standard Time",
        &[
            "Pacific/Guadalcanal",
            "Antarctica/Casey",
            "Pacific/Ponape",
            "Pacific/Kosrae",
            "Pacific/Noumea",
            "Pacific/Efate",
            "Etc/GMT-11",
        ],
    ),
    ("Russia Time Zone 11", &["Asia/Kamchatka", "Asia/Anadyr"]),
    (
        "New Zealand Standard Time",
        &["Pacific/Auckland", "Antarctica/McMurdo"],
    ),
    (
        "UTC+12",
        &[
            "Etc/GMT-12",
            "Pacific/Tarawa",
            "Pacific/Majuro",
            "Pacific/Kwajalein",
            "Pacific/Nauru",
            "Pacific/Funafuti",
            "Pacific/Wake",
            "Pacific/Wallis",
        ],
    ),
    ("Fiji Standard Time", &["Pacific/Fiji"]),
    ("Chatham Islands Standard Time", &["Pacific/Chatham"]),
    (
        "UTC+13",
        &["Etc/GMT-13", "Pacific/Enderbury", "Pacific/Fakaofo"],
    ),
    ("Tonga Standard Time", &["Pacific/Tongatapu"]),
    ("Samoa Standard Time", &["Pacific/Apia"]),
    (
        "Line Islands Standard Time",
        &["Pacific/Kiritimati", "Etc/GMT-14"],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_is_found_by_any_of_its_names() {
        let berlin = Some(("W. Europe Standard Time", Some("Europe/Berlin")));
        assert_eq!(lookup("Europe/Berlin"), berlin);
        assert_eq!(lookup("europe/berlin"), berlin);
        assert_eq!(lookup("Berlin"), berlin);
        assert_eq!(lookup("  BERLIN "), berlin);
        assert_eq!(
            lookup("w. europe standard time"),
            Some(("W. Europe Standard Time", Some("Europe/Berlin")))
        );
        assert_eq!(
            lookup("Tokyo"),
            Some(("Tokyo Standard Time", Some("Asia/Tokyo")))
        );
        assert_eq!(lookup("Atlantis"), None);
        assert_eq!(lookup(""), None);
    }

    #[test]
    fn cities_and_links_lead_to_their_zone() {
        assert_eq!(
            lookup("New York"),
            Some(("Eastern Standard Time", Some("America/New_York")))
        );
        assert_eq!(
            lookup("new_york"),
            Some(("Eastern Standard Time", Some("America/New_York")))
        );
        assert_eq!(
            lookup("Asia/Kolkata"),
            Some(("India Standard Time", Some("Asia/Calcutta")))
        );
        assert_eq!(
            lookup("Mumbai"),
            Some(("India Standard Time", Some("Asia/Calcutta")))
        );
        assert_eq!(
            lookup("São Paulo"),
            Some(("E. South America Standard Time", Some("America/Sao_Paulo")))
        );
        assert_eq!(
            lookup("Kyiv"),
            Some(("FLE Standard Time", Some("Europe/Kiev")))
        );
        assert_eq!(
            lookup("Port-au-Prince"),
            Some(("Haiti Standard Time", Some("America/Port-au-Prince")))
        );
        assert_eq!(lookup("UTC"), Some(("UTC", Some("Etc/UTC"))));
    }

    #[test]
    fn every_identifier_belongs_to_one_zone() {
        let mut seen = std::collections::HashSet::new();
        for (windows_id, ianas) in ZONES {
            assert!(!ianas.is_empty(), "{windows_id} maps to nothing");
            for iana in *ianas {
                assert!(seen.insert(*iana), "{iana} is in two zones");
            }
        }
        for (_, iana) in ALIASES.iter().chain(CITIES) {
            assert!(windows_of(iana).is_some(), "{iana} is in no zone");
        }
    }

    #[test]
    fn a_city_is_written_as_cldr_writes_it() {
        assert_eq!(city_of("America/New_York"), "New York");
        assert_eq!(city_of("Asia/Calcutta"), "Kolkata");
        assert_eq!(city_of("America/Argentina/Ushuaia"), "Ushuaia");
        assert_eq!(primary_iana("tokyo standard time"), Some("Asia/Tokyo"));
    }

    #[test]
    fn an_offset_is_written_with_hours_and_minutes() {
        assert_eq!(offset_text(0), "UTC");
        assert_eq!(offset_text(60), "UTC+01:00");
        assert_eq!(offset_text(330), "UTC+05:30");
        assert_eq!(offset_text(-210), "UTC-03:30");
        assert_eq!(offset_text(-600), "UTC-10:00");
    }
}
