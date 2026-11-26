// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is asked of ExchangeRate-API and how its answer reads.

use serde_json::Value;
use tessera_extension_kit::settings::parse_list;

/// The base currency and the currencies to show, as upper-case codes.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub base: String,
    pub symbols: Vec<String>,
}

fn code(text: &str) -> Result<String, String> {
    let code = text.trim().to_ascii_uppercase();
    match code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_uppercase()) {
        true => Ok(code),
        false => Err(format!(
            "`{}` is not a currency code such as EUR.",
            text.trim()
        )),
    }
}

impl Query {
    pub fn parse(base: &str, symbols: &str) -> Result<Self, String> {
        let symbols = parse_list(symbols)
            .iter()
            .map(|symbol| code(symbol))
            .collect::<Result<Vec<_>, _>>()?;
        if symbols.is_empty() {
            return Err("Name currencies in the setting `symbols`, as \"EUR, GBP\".".to_owned());
        }
        Ok(Self {
            base: code(base)?,
            symbols,
        })
    }

    pub fn address(&self) -> String {
        format!("https://open.er-api.com/v6/latest/{}", self.base)
    }
}

/// The rates of the asked currencies, and when they were last updated.
#[derive(Clone, Debug, PartialEq)]
pub struct Rates {
    /// Each currency with what one unit of the base buys of it, when the
    /// service knows it.
    pub rates: Vec<(String, Option<f64>)>,
    /// Seconds since 1970.
    pub updated: Option<u64>,
}

pub fn read(body: &str, query: &Query) -> Result<Rates, String> {
    let answer: Value = serde_json::from_str(body)
        .map_err(|_| "The answer of ExchangeRate-API is unreadable.".to_owned())?;
    if answer["result"] != "success" {
        return Err(match answer["error-type"].as_str() {
            Some("unsupported-code") => format!("ExchangeRate-API does not know {}.", query.base),
            Some(kind) => format!("ExchangeRate-API answered: {kind}."),
            None => "ExchangeRate-API gave no rates.".to_owned(),
        });
    }
    let rates = query.symbols.iter().map(|symbol| {
        let rate = answer["rates"][symbol].as_f64().filter(|rate| *rate > 0.);
        (symbol.clone(), rate)
    });
    Ok(Rates {
        rates: rates.collect(),
        updated: answer["time_last_update_unix"].as_u64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_checked() {
        let query = Query::parse("usd", "eur; gbp,JPY").unwrap();
        assert_eq!(query.base, "USD");
        assert_eq!(query.symbols, ["EUR", "GBP", "JPY"]);
        assert_eq!(query.address(), "https://open.er-api.com/v6/latest/USD");
        assert!(Query::parse("dollar", "EUR").is_err());
        assert!(Query::parse("USD", "").is_err());
        assert!(Query::parse("USD", "EU").unwrap_err().contains("EU"));
    }

    #[test]
    fn rates_are_read_and_failures_explained() {
        let query = Query::parse("USD", "EUR, XXX").unwrap();
        let body = r#"{"result":"success","time_last_update_unix":1795651351,"rates":{"USD":1,"EUR":0.9241}}"#;
        let rates = read(body, &query).unwrap();
        assert_eq!(rates.updated, Some(1_795_651_351));
        assert_eq!(
            rates.rates,
            [("EUR".to_owned(), Some(0.9241)), ("XXX".to_owned(), None)]
        );
        let unknown = r#"{"result":"error","error-type":"unsupported-code"}"#;
        assert!(
            read(unknown, &query)
                .unwrap_err()
                .contains("does not know USD")
        );
        assert!(read("nope", &query).is_err());
    }
}
