// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Conversions of units: `5 km to mi`, `100 f in c`, `2 GiB in MB`,
//! `90 min to h`, `30 мм в дюймы`, and of currencies, `100 usd to eur`.
//!
//! A conversion is a number, a unit, one of `to`, `in`, `as`, `into`,
//! `в`, `->` or `=`, and another unit of the same kind: length, mass,
//! temperature, capacity, area, speed, size of data or time. Units are
//! known by their symbols and their names in English and Russian. kB,
//! MB and GB are powers of 1000, KiB, MiB and GiB powers of 1024.
//!
//! Currencies are known by their ISO codes and a few signs (`$`, `€`,
//! `£`, `¥`). Their rates are those the European Central Bank publishes
//! every working day, kept in `rates.xml` in the folder of Tessera in
//! `%LOCALAPPDATA%` and fetched again from the bank once they are older
//! than 12 hours, unless the widget option `currency = false` keeps
//! Tessera from fetching; then the kept file is used, however old, and
//! without one currencies are said to be unavailable.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::config::config_dir;
use crate::extension::http;

use super::calc::format_digits;
use super::{Answer, AnswerAction, Query, Runner, fill};

const ICON_UNITS: &str = "\u{e7f8}";
const ICON_MONEY: &str = "\u{e8c7}";

/// How sure the runner is of a conversion, and of one between
/// currencies.
const SCORE: u16 = 900;
const SCORE_CURRENCY: u16 = 880;

/// The significant digits of a converted value.
const DIGITS: i32 = 7;

/// Where the rates are published, and the most of them read.
const RATES_ADDRESS: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
const RATES_LIMIT: u64 = 64 * 1024;

/// The file the rates are kept in.
const RATES_FILE: &str = "rates.xml";

/// How old the rates may be before they are fetched again, and how long
/// a failed fetch is not tried again.
const RATES_FRESH: Duration = Duration::from_secs(12 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// The words that stand between the two units.
const SEPARATORS: [&str; 9] = ["to", "in", "as", "into", "в", "во", "->", "=", "→"];

/// What a unit measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dimension {
    Length,
    Mass,
    Temperature,
    Capacity,
    Area,
    Speed,
    Data,
    Time,
}

impl Dimension {
    fn name(self) -> &'static str {
        match self {
            Self::Length => "Length",
            Self::Mass => "Weight",
            Self::Temperature => "Temperature",
            Self::Capacity => "Capacity",
            Self::Area => "Area",
            Self::Speed => "Speed",
            Self::Data => "Data size",
            Self::Time => "Duration",
        }
    }
}

/// A unit: the symbol it is shown with, the names it is known by,
/// written in lowercase without spaces, what it measures, and how a
/// value of it is made one of the base unit of its dimension: times
/// `factor`, plus `offset`.
struct Unit {
    symbol: &'static str,
    names: &'static [&'static str],
    dimension: Dimension,
    factor: f64,
    offset: f64,
}

const fn unit(
    symbol: &'static str,
    names: &'static [&'static str],
    dimension: Dimension,
    factor: f64,
) -> Unit {
    Unit {
        symbol,
        names,
        dimension,
        factor,
        offset: 0.,
    }
}

use Dimension::{Area, Capacity, Data, Length, Mass, Speed, Temperature, Time};

/// The units, of each dimension the base one first: the metre, the
/// kilogram, the kelvin, the litre, the square metre, the metre per
/// second, the byte and the second.
#[rustfmt::skip]
const UNITS: &[Unit] = &[
    unit("m", &["m", "meter", "meters", "metre", "metres", "м", "метр", "метра", "метров"], Length, 1.),
    unit("nm", &["nm", "nanometer", "nanometers", "нм"], Length, 1e-9),
    unit("µm", &["µm", "um", "micrometer", "micron", "мкм"], Length, 1e-6),
    unit("mm", &["mm", "millimeter", "millimeters", "millimetre", "мм", "миллиметр", "миллиметров"], Length, 1e-3),
    unit("cm", &["cm", "centimeter", "centimeters", "centimetre", "см", "сантиметр", "сантиметров"], Length, 1e-2),
    unit("dm", &["dm", "decimeter", "дм"], Length, 0.1),
    unit("km", &["km", "kilometer", "kilometers", "kilometre", "kilometres", "км", "километр", "километра", "километров"], Length, 1e3),
    unit("in", &["in", "inch", "inches", "\"", "дюйм", "дюйма", "дюймов", "дюймы"], Length, 0.0254),
    unit("ft", &["ft", "foot", "feet", "'", "фут", "фута", "футов", "футы"], Length, 0.3048),
    unit("yd", &["yd", "yard", "yards", "ярд", "ярда", "ярдов"], Length, 0.9144),
    unit("mi", &["mi", "mile", "miles", "миля", "мили", "миль"], Length, 1609.344),
    unit("nmi", &["nmi", "nauticalmile", "nauticalmiles", "морскаямиля", "морскихмиль"], Length, 1852.),
    unit("au", &["au", "astronomicalunit"], Length, 1.495_978_707e11),
    unit("ly", &["ly", "lightyear", "lightyears", "световойгод", "световыхлет"], Length, 9.460_730_472_580_8e15),
    unit("kg", &["kg", "kilogram", "kilograms", "kilo", "kilos", "кг", "килограмм", "килограмма", "килограммов"], Mass, 1.),
    unit("µg", &["µg", "ug", "microgram", "мкг"], Mass, 1e-9),
    unit("mg", &["mg", "milligram", "milligrams", "мг", "миллиграмм"], Mass, 1e-6),
    unit("g", &["g", "gram", "grams", "gramme", "г", "гр", "грамм", "грамма", "граммов"], Mass, 1e-3),
    unit("t", &["t", "tonne", "tonnes", "ton", "tons", "т", "тонна", "тонны", "тонн"], Mass, 1e3),
    unit("lb", &["lb", "lbs", "pound", "pounds", "фунт", "фунта", "фунтов", "фунты"], Mass, 0.453_592_37),
    unit("oz", &["oz", "ounce", "ounces", "унция", "унции", "унций"], Mass, 0.028_349_523_125),
    unit("st", &["st", "stone", "stones", "стоун"], Mass, 6.350_293_18),
    unit("ct", &["ct", "carat", "carats", "карат"], Mass, 2e-4),
    unit("K", &["k", "kelvin", "kelvins", "к", "кельвин", "кельвина", "кельвинов"], Temperature, 1.),
    Unit { symbol: "°C", names: &["c", "celsius", "degc", "цельсий", "цельсия", "градусцельсия"], dimension: Temperature, factor: 1., offset: 273.15 },
    Unit { symbol: "°F", names: &["f", "fahrenheit", "degf", "ф", "фаренгейт", "фаренгейта"], dimension: Temperature, factor: 5. / 9., offset: 459.67 * 5. / 9. },
    unit("l", &["l", "liter", "liters", "litre", "litres", "л", "литр", "литра", "литров"], Capacity, 1.),
    unit("ml", &["ml", "milliliter", "milliliters", "millilitre", "мл", "миллилитр", "миллилитров"], Capacity, 1e-3),
    unit("cl", &["cl", "centiliter", "сл"], Capacity, 1e-2),
    unit("dl", &["dl", "deciliter", "дл"], Capacity, 0.1),
    unit("m³", &["m3", "m³", "cubicmeter", "cubicmeters", "м3", "м³", "кубометр", "кубометров"], Capacity, 1e3),
    unit("cm³", &["cm3", "cm³", "cc", "см3", "см³"], Capacity, 1e-3),
    unit("gal", &["gal", "gallon", "gallons", "usgal", "галлон", "галлона", "галлонов"], Capacity, 3.785_411_784),
    unit("imp gal", &["impgal", "ukgal", "imperialgallon"], Capacity, 4.546_09),
    unit("qt", &["qt", "quart", "quarts", "кварта"], Capacity, 0.946_352_946),
    unit("pt", &["pt", "pint", "pints", "пинта", "пинты"], Capacity, 0.473_176_473),
    unit("cup", &["cup", "cups", "чашка"], Capacity, 0.236_588_236_5),
    unit("fl oz", &["floz", "fluidounce", "fluidounces"], Capacity, 0.029_573_529_562_5),
    unit("tbsp", &["tbsp", "tablespoon", "tablespoons", "столоваяложка"], Capacity, 0.014_786_764_781_25),
    unit("tsp", &["tsp", "teaspoon", "teaspoons", "чайнаяложка"], Capacity, 0.004_928_921_593_75),
    unit("m²", &["m2", "m²", "sqm", "squaremeter", "squaremeters", "м2", "м²", "квм"], Area, 1.),
    unit("mm²", &["mm2", "mm²", "мм2"], Area, 1e-6),
    unit("cm²", &["cm2", "cm²", "sqcm", "см2", "см²"], Area, 1e-4),
    unit("km²", &["km2", "km²", "sqkm", "км2", "км²", "квкм"], Area, 1e6),
    unit("ha", &["ha", "hectare", "hectares", "га", "гектар", "гектара", "гектаров"], Area, 1e4),
    unit("a", &["are", "ares", "сотка", "сотки", "соток", "ар"], Area, 100.),
    unit("acre", &["acre", "acres", "ac", "акр", "акра", "акров"], Area, 4_046.856_422_4),
    unit("ft²", &["ft2", "ft²", "sqft", "squarefoot", "squarefeet"], Area, 0.092_903_04),
    unit("in²", &["in2", "in²", "sqin"], Area, 6.4516e-4),
    unit("yd²", &["yd2", "yd²", "sqyd"], Area, 0.836_127_36),
    unit("mi²", &["mi2", "mi²", "sqmi"], Area, 2_589_988.110_336),
    unit("m/s", &["m/s", "mps", "м/с"], Speed, 1.),
    unit("km/h", &["km/h", "kmh", "kph", "kmph", "км/ч", "кмч"], Speed, 1. / 3.6),
    unit("mph", &["mph", "mi/h", "миль/ч"], Speed, 0.447_04),
    unit("kn", &["kn", "kt", "knot", "knots", "узел", "узла", "узлов"], Speed, 1852. / 3600.),
    unit("ft/s", &["ft/s", "fps"], Speed, 0.3048),
    unit("B", &["b", "byte", "bytes", "б", "байт", "байта", "байтов"], Data, 1.),
    unit("bit", &["bit", "bits", "бит", "бита", "битов"], Data, 0.125),
    unit("kB", &["kb", "kilobyte", "kilobytes", "кб", "килобайт", "килобайта"], Data, 1e3),
    unit("MB", &["mb", "megabyte", "megabytes", "мб", "мегабайт", "мегабайта"], Data, 1e6),
    unit("GB", &["gb", "gigabyte", "gigabytes", "гб", "гигабайт", "гигабайта"], Data, 1e9),
    unit("TB", &["tb", "terabyte", "terabytes", "тб", "терабайт", "терабайта"], Data, 1e12),
    unit("PB", &["pb", "petabyte", "petabytes", "пб", "петабайт"], Data, 1e15),
    unit("KiB", &["kib", "kibibyte", "кибибайт"], Data, 1024.),
    unit("MiB", &["mib", "mebibyte", "мебибайт"], Data, 1_048_576.),
    unit("GiB", &["gib", "gibibyte", "гибибайт"], Data, 1_073_741_824.),
    unit("TiB", &["tib", "tebibyte", "тебибайт"], Data, 1_099_511_627_776.),
    unit("kbit", &["kbit", "kbits", "kilobit", "кбит"], Data, 125.),
    unit("Mbit", &["mbit", "mbits", "megabit", "мбит"], Data, 125_000.),
    unit("Gbit", &["gbit", "gbits", "gigabit", "гбит"], Data, 125_000_000.),
    unit("s", &["s", "sec", "secs", "second", "seconds", "с", "сек", "секунда", "секунды", "секунд"], Time, 1.),
    unit("ns", &["ns", "nanosecond", "nanoseconds", "нс"], Time, 1e-9),
    unit("µs", &["µs", "us", "microsecond", "microseconds", "мкс"], Time, 1e-6),
    unit("ms", &["ms", "millisecond", "milliseconds", "мс", "миллисекунда", "миллисекунд"], Time, 1e-3),
    unit("min", &["min", "mins", "minute", "minutes", "мин", "минута", "минуты", "минут"], Time, 60.),
    unit("h", &["h", "hr", "hrs", "hour", "hours", "ч", "час", "часа", "часов"], Time, 3600.),
    unit("d", &["d", "day", "days", "д", "день", "дня", "дней", "сутки", "суток"], Time, 86_400.),
    unit("wk", &["wk", "week", "weeks", "нед", "неделя", "недели", "недель"], Time, 604_800.),
    unit("mo", &["mo", "month", "months", "мес", "месяц", "месяца", "месяцев"], Time, 2_629_746.),
    unit("yr", &["yr", "y", "year", "years", "г.", "год", "года", "лет"], Time, 31_556_952.),
];

/// The currencies the European Central Bank gives rates for, against
/// the euro.
const ECB_CURRENCIES: [&str; 31] = [
    "EUR", "USD", "JPY", "BGN", "CZK", "DKK", "GBP", "HUF", "PLN", "RON", "SEK", "CHF", "ISK",
    "NOK", "TRY", "AUD", "BRL", "CAD", "CNY", "HKD", "IDR", "ILS", "INR", "KRW", "MXN", "MYR",
    "NZD", "PHP", "SGD", "THB", "ZAR",
];

/// Currencies that are known to be such but have no rate of the bank.
const OTHER_CURRENCIES: [&str; 16] = [
    "RUB", "UAH", "BYN", "KZT", "UZS", "GEL", "AMD", "AZN", "AED", "SAR", "EGP", "ARS", "CLP",
    "COP", "VND", "TWD",
];

/// The signs that stand for currencies.
const CURRENCY_SIGNS: [(&str, &str); 8] = [
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
    ("₽", "RUB"),
    ("₴", "UAH"),
    ("₹", "INR"),
    ("₩", "KRW"),
];

/// A unit as it is written, in lowercase and without spaces; a `°`
/// before a letter of a temperature is dropped, and a Cyrillic `с`
/// after it is Celsius rather than seconds.
fn normalized(text: &str) -> String {
    let text: String = text.to_lowercase().split_whitespace().collect();
    match text.strip_prefix('°') {
        Some("с") => "c".to_owned(),
        Some(rest) => rest.to_owned(),
        None => text,
    }
}

fn unit_named(text: &str) -> Option<&'static Unit> {
    let name = normalized(text);
    UNITS
        .iter()
        .find(|unit| unit.names.contains(&name.as_str()))
}

/// The ISO code of the currency `text` names.
fn currency_named(text: &str) -> Option<&'static str> {
    let trimmed = text.trim();
    if let Some(&(_, code)) = CURRENCY_SIGNS.iter().find(|(sign, _)| *sign == trimmed) {
        return Some(code);
    }
    let upper = trimmed.to_uppercase();
    { ECB_CURRENCIES.iter().chain(&OTHER_CURRENCIES) }
        .find(|code| **code == upper)
        .copied()
}

/// A number at the start of `text`, with a point or a comma before its
/// fraction, and what follows it.
fn leading_number(text: &str) -> Option<(f64, &str)> {
    let text = text.trim_start();
    let end = { text.char_indices() }
        .find(|&(at, c)| !(c.is_ascii_digit() || c == '.' || c == ',' || (at == 0 && c == '-')))
        .map_or(text.len(), |(at, _)| at);
    let number = text[..end].replace(',', ".");
    let value: f64 = number.parse().ok()?;
    Some((value, &text[end..]))
}

/// What a conversion asks for: the value, and the two units as they are
/// written.
fn split(text: &str) -> Option<(f64, String, String)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    // The last word is the unit asked for, never the separator: `in` is
    // both, as in `5 ft to in`.
    let at = { words[..words.len().saturating_sub(1)].iter() }
        .rposition(|word| SEPARATORS.contains(&word.to_lowercase().as_str()))?;
    let (before, after) = (words[..at].join(" "), words[at + 1..].join(" "));
    if after.is_empty() {
        return None;
    }
    // A sign of a currency may stand before the number, as in $20.
    let (sign, before) = match CURRENCY_SIGNS
        .iter()
        .find(|(sign, _)| before.starts_with(sign))
    {
        Some((sign, _)) => (Some(*sign), &before[sign.len()..]),
        None => (None, before.as_str()),
    };
    let (value, unit) = leading_number(before)?;
    let unit = match sign {
        Some(sign) if unit.trim().is_empty() => sign.to_owned(),
        Some(_) => return None,
        None => unit.trim().to_owned(),
    };
    (!unit.is_empty()).then_some((value, unit, after))
}

/// `value` of `from` in `to`, where both measure the same.
fn convert(value: f64, from: &Unit, to: &Unit) -> Option<f64> {
    if from.dimension != to.dimension {
        return None;
    }
    let base = value * from.factor + from.offset;
    Some((base - to.offset) / to.factor)
}

/// The rates of the bank: euros of a day in each currency.
#[derive(Clone, Debug, PartialEq)]
struct Rates {
    /// The day they are of, as the bank writes it: 2026-11-26.
    day: String,
    per_euro: HashMap<String, f64>,
}

impl Rates {
    /// The rates of the daily file of the bank, read without a parser of
    /// XML: the day from `time='…'`, each rate from `currency='…'` and
    /// `rate='…'` of one element.
    fn parse(text: &str) -> Option<Self> {
        let attribute = |element: &str, name: &str| -> Option<String> {
            let at = element.find(&format!("{name}="))? + name.len() + 1;
            let quote = element[at..].chars().next()?;
            let rest = &element[at + 1..];
            Some(rest[..rest.find(quote)?].to_owned())
        };
        let mut day = None;
        let mut per_euro = HashMap::new();
        for element in text
            .split('<')
            .filter(|element| element.starts_with("Cube"))
        {
            if let Some(time) = attribute(element, "time") {
                day = Some(time);
            }
            if let (Some(currency), Some(rate)) =
                (attribute(element, "currency"), attribute(element, "rate"))
                && let Ok(rate) = rate.parse::<f64>()
                && rate > 0.
            {
                per_euro.insert(currency.to_uppercase(), rate);
            }
        }
        per_euro.insert("EUR".to_owned(), 1.);
        (per_euro.len() > 1).then(|| Self {
            day: day.unwrap_or_default(),
            per_euro,
        })
    }

    /// `value` in `from` in `to`, where the bank gives both.
    fn convert(&self, value: f64, from: &str, to: &str) -> Option<f64> {
        Some(value / self.per_euro.get(from)? * self.per_euro.get(to)?)
    }
}

/// What is known of the rates.
enum RatesState {
    Ready(Rates),
    Fetching,
    /// None can be had, and why, as a text to say.
    Unavailable(&'static str),
}

/// The rates read last, with when they were published or written.
static RATES: Mutex<Option<(SystemTime, Rates)>> = Mutex::new(None);

/// Whether the rates are being fetched, and when a fetch failed last.
static FETCHING: AtomicBool = AtomicBool::new(false);
static FAILED: Mutex<Option<Instant>> = Mutex::new(None);

/// The file the rates are kept in.
fn rates_file() -> Option<PathBuf> {
    let local =
        std::env::var_os("LOCALAPPDATA").map(|folder| PathBuf::from(folder).join("tessera"));
    Some(local.or_else(config_dir)?.join(RATES_FILE))
}

/// The rates as far as they can be had now; with `fetch`, rates that are
/// too old are fetched again on a thread of their own, the old ones
/// being used meanwhile.
fn rates(fetch: bool) -> RatesState {
    let mut kept = RATES.lock().unwrap_or_else(PoisonError::into_inner);
    if kept.is_none()
        && let Some(file) = rates_file()
        && let Ok(text) = fs::read_to_string(&file)
        && let Some(rates) = Rates::parse(&text)
    {
        let written = { fs::metadata(&file) }
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        *kept = Some((written, rates));
    }
    let fresh = kept
        .as_ref()
        .is_some_and(|(written, _)| written.elapsed().is_ok_and(|age| age < RATES_FRESH));
    let rates = kept.as_ref().map(|(_, rates)| rates.clone());
    drop(kept);
    let failed = { FAILED.lock().unwrap_or_else(PoisonError::into_inner) }
        .is_some_and(|at| at.elapsed() < RETRY_AFTER);
    if fetch && !fresh && !failed && !FETCHING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(fetch_rates);
    }
    match rates {
        Some(rates) => RatesState::Ready(rates),
        None if !fetch => RatesState::Unavailable("Fetching them is switched off"),
        None if failed => RatesState::Unavailable("They could not be fetched"),
        None => RatesState::Fetching,
    }
}

/// Fetches the rates from the bank and keeps them, in memory and in the
/// file. A failure is reported, and no fetch is tried for a while.
fn fetch_rates() {
    let fetched = http::download(RATES_ADDRESS, RATES_LIMIT).and_then(|bytes| {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let rates =
            Rates::parse(&text).ok_or_else(|| "the file of the bank has no rates".to_owned())?;
        Ok((text, rates))
    });
    match fetched {
        Ok((text, rates)) => {
            if let Some(file) = rates_file() {
                let written = { file.parent().map(fs::create_dir_all) }
                    .unwrap_or(Ok(()))
                    .and_then(|()| fs::write(&file, text));
                if let Err(error) = written {
                    report!(
                        "the exchange rates were not kept in {}: {error}",
                        file.display()
                    );
                }
            }
            *RATES.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((SystemTime::now(), rates));
        }
        Err(error) => {
            report!("the exchange rates were not fetched: {error}");
            *FAILED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        }
    }
    FETCHING.store(false, Ordering::Release);
}

pub struct Units {
    /// Whether the rates of currencies may be fetched.
    fetch: bool,
}

impl Units {
    pub fn new(fetch: bool) -> Self {
        Self { fetch }
    }

    fn currency(&self, value: f64, from: &str, to: &str, query: &Query) -> Answer {
        let typed = format!("{} {from}", format_digits(value, DIGITS));
        let unavailable = |why: &'static str| {
            let title = query.say("Exchange rates are unavailable");
            Answer::new(
                title,
                query.say(why),
                ICON_MONEY,
                SCORE_CURRENCY,
                AnswerAction::Nothing,
            )
        };
        let known = |code: &str| ECB_CURRENCIES.contains(&code);
        if !known(from) || !known(to) {
            return unavailable("The European Central Bank gives no rate for this currency");
        }
        match rates(self.fetch) {
            RatesState::Ready(rates) => match rates.convert(value, from, to) {
                Some(converted) => {
                    let result = format!("{converted:.2} {to}");
                    let detail = fill(
                        query.say("{} · rate of the European Central Bank of {}"),
                        &[&typed, &rates.day],
                    );
                    Answer::new(format!("= {result}"), detail, ICON_MONEY, SCORE_CURRENCY, {
                        AnswerAction::Copy(format!("{converted:.2}"))
                    })
                    .copying(format!("{converted:.2}"))
                }
                None => unavailable("The European Central Bank gives no rate for this currency"),
            },
            RatesState::Fetching => {
                let title = query.say("Exchange rates are being fetched…");
                let detail = query.say("European Central Bank");
                Answer::new(
                    title,
                    detail,
                    ICON_MONEY,
                    SCORE_CURRENCY,
                    AnswerAction::Nothing,
                )
                .waiting()
            }
            RatesState::Unavailable(why) => unavailable(why),
        }
    }
}

impl Runner for Units {
    fn id(&self) -> &'static str {
        "units"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let Some((value, from, to)) = split(query.text()) else {
            return Vec::new();
        };
        if let (Some(from), Some(to)) = (unit_named(&from), unit_named(&to))
            && let Some(converted) = convert(value, from, to)
        {
            let result = format_digits(converted, DIGITS);
            let shown = format!("{result} {}", to.symbol);
            let typed = format!("{} {}", format_digits(value, DIGITS), from.symbol);
            let detail = fill("{} · {}", &[&typed, query.say(from.dimension.name())]);
            let answer = Answer::new(format!("= {shown}"), detail, ICON_UNITS, SCORE, {
                AnswerAction::Copy(result.clone())
            });
            return vec![answer.copying(result)];
        }
        match (currency_named(&from), currency_named(&to)) {
            (Some(from), Some(to)) => vec![self.currency(value, from, to, query)],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converted(text: &str) -> Option<f64> {
        let (value, from, to) = split(text)?;
        convert(value, unit_named(&from)?, unit_named(&to)?)
    }

    fn near(value: Option<f64>, expected: f64) -> bool {
        value.is_some_and(|value| (value - expected).abs() < 1e-6 * expected.abs().max(1.))
    }

    #[test]
    fn units_of_one_kind_convert() {
        assert!(near(converted("5 km to mi"), 3.106_855_961));
        assert!(near(converted("100 f in c"), 37.777_777_778));
        assert!(near(converted("-40 °C to °F"), -40.));
        assert!(near(converted("0 c to k"), 273.15));
        assert!(near(converted("2 GiB in MB"), 2_147.483_648));
        assert!(near(converted("90 min to h"), 1.5));
        assert!(near(converted("1,5 кг в г"), 1500.));
        assert!(near(converted("30 мм в дюймы"), 1.181_102_362));
        assert!(near(converted("100 km/h to mph"), 62.137_119_224));
        assert!(near(converted("1 ha in m2"), 10_000.));
        assert!(near(converted("1 gal to l"), 3.785_411_784));
        assert!(near(converted("12inch to cm"), 30.48));
        assert!(near(converted("36,6 °с в f"), 97.88));
        assert!(near(converted("5 ft to in"), 60.));
        assert!(near(converted("10 cm in in"), 3.937_007_874));
    }

    #[test]
    fn what_is_no_conversion_is_left_alone() {
        assert_eq!(converted("5 km to kg"), None);
        assert_eq!(converted("5 km to"), None);
        assert_eq!(converted("km to mi"), None);
        assert_eq!(split("open the door to the garden"), None);
        assert!(split("notes in folder").is_none());
    }

    #[test]
    fn currencies_are_known_by_codes_and_signs() {
        assert_eq!(currency_named("usd"), Some("USD"));
        assert_eq!(currency_named("€"), Some("EUR"));
        assert_eq!(currency_named("rub"), Some("RUB"));
        assert_eq!(currency_named("abc"), None);
        let (value, from, to) = split("$20 to eur").unwrap();
        assert_eq!((value, from.as_str(), to.as_str()), (20., "$", "eur"));
        assert!(split("$20 usd to eur").is_none());
    }

    #[test]
    fn the_rates_of_the_bank_are_read() {
        let file = "<?xml version=\"1.0\"?><gesmes:Envelope><Cube>\
            <Cube time='2026-11-26'>\
            <Cube currency='USD' rate='1.0800'/><Cube currency=\"JPY\" rate=\"162.5\"/>\
            </Cube></Cube></gesmes:Envelope>";
        let rates = Rates::parse(file).unwrap();
        assert_eq!(rates.day, "2026-11-26");
        assert_eq!(rates.per_euro.len(), 3);
        assert!(near(rates.convert(108., "USD", "EUR"), 100.));
        assert!(near(rates.convert(1., "USD", "JPY"), 150.462_962_963));
        assert_eq!(rates.convert(1., "USD", "GBP"), None);
        assert_eq!(Rates::parse("<html>nothing</html>"), None);
    }

    #[test]
    fn every_unit_name_names_one_unit() {
        let mut names: Vec<&str> = UNITS
            .iter()
            .flat_map(|unit| unit.names.iter().copied())
            .collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        for unit in UNITS {
            assert!(
                unit.names.iter().all(|name| normalized(name) == *name),
                "{}",
                unit.symbol
            );
        }
    }
}
