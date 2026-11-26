// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is asked of CoinGecko and how its answer reads.

use serde_json::Value;
use tessera_extension_kit::format;
use tessera_extension_kit::settings::parse_list;

/// The coins and the currency to ask for.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub coins: Vec<String>,
    pub currency: String,
}

impl Query {
    /// Reads coins such as `bitcoin, ethereum` and a currency such as
    /// `usd`.
    pub fn parse(coins: &str, currency: &str) -> Result<Self, String> {
        let coins: Vec<String> = parse_list(coins)
            .into_iter()
            .map(|coin| coin.to_ascii_lowercase())
            .collect();
        if coins.is_empty() {
            return Err("Name coins in the setting `coins`, as \"bitcoin, ethereum\".".to_owned());
        }
        let id = |text: &str| {
            text.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        };
        if let Some(wrong) = coins.iter().find(|coin| !id(coin)) {
            return Err(format!("`{wrong}` is not a CoinGecko id such as bitcoin."));
        }
        let currency = currency.trim().to_ascii_lowercase();
        if !(2..=6).contains(&currency.len()) || !currency.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(format!("`{currency}` is not a currency such as usd."));
        }
        Ok(Self { coins, currency })
    }

    pub fn address(&self) -> String {
        format!(
            "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies={}&include_24hr_change=true",
            self.coins.join(","),
            self.currency
        )
    }

    /// `price` in the currency, as `$60,538` or `169.72 CHF`.
    pub fn money(&self, price: f64) -> String {
        let amount = format::amount(price);
        match self.currency.as_str() {
            "usd" => format!("${amount}"),
            "eur" => format!("€{amount}"),
            "gbp" => format!("£{amount}"),
            "jpy" | "cny" => format!("¥{amount}"),
            "inr" => format!("₹{amount}"),
            "btc" => format!("₿{amount}"),
            other => format!("{amount} {}", other.to_ascii_uppercase()),
        }
    }
}

/// A coin's price and its change over the last day, in percent.
#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    pub coin: String,
    pub price: f64,
    pub change: Option<f64>,
}

impl Quote {
    /// A short name for a bar: the ticker of a well-known coin, the id of
    /// any other.
    pub fn ticker(&self) -> String {
        let known = match self.coin.as_str() {
            "bitcoin" => "BTC",
            "ethereum" => "ETH",
            "tether" => "USDT",
            "binancecoin" => "BNB",
            "solana" => "SOL",
            "ripple" => "XRP",
            "dogecoin" => "DOGE",
            "cardano" => "ADA",
            "tron" => "TRX",
            "polkadot" => "DOT",
            "litecoin" => "LTC",
            "monero" => "XMR",
            other => return format::title(other),
        };
        known.to_owned()
    }

    pub fn name(&self) -> String {
        format::title(&self.coin)
    }
}

/// Reads the answer to `query`, in the order of its coins. Coins that
/// CoinGecko does not know are left out; when it knows none, that is the
/// error.
pub fn read(body: &str, query: &Query) -> Result<Vec<Quote>, String> {
    let answer: Value =
        serde_json::from_str(body).map_err(|_| "CoinGecko's answer is unreadable.".to_owned())?;
    let change_key = format!("{}_24h_change", query.currency);
    let quotes: Vec<Quote> = query
        .coins
        .iter()
        .filter_map(|coin| {
            let prices = answer.get(coin)?;
            Some(Quote {
                coin: coin.clone(),
                price: prices.get(&query.currency)?.as_f64()?,
                change: prices.get(&change_key).and_then(Value::as_f64),
            })
        })
        .collect();
    if quotes.is_empty() {
        return Err(format!(
            "CoinGecko knows none of {} in {}.",
            query.coins.join(", "),
            query.currency.to_ascii_uppercase()
        ));
    }
    Ok(quotes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"bitcoin":{"usd":60538,"usd_24h_change":0.788},"ethereum":{"usd":1696.72,"usd_24h_change":-1.25}}"#;

    #[test]
    fn the_settings_are_checked() {
        let query = Query::parse("Bitcoin, ethereum, wat", " USD ").unwrap();
        assert_eq!(query.coins, ["bitcoin", "ethereum", "wat"]);
        assert!(
            query
                .address()
                .contains("ids=bitcoin,ethereum,wat&vs_currencies=usd")
        );
        assert!(Query::parse("", "usd").is_err());
        assert!(Query::parse("bit coin", "usd").is_err());
        assert!(Query::parse("bitcoin", "us dollars").is_err());
    }

    #[test]
    fn the_answer_is_read_in_order_of_the_settings() {
        let query = Query::parse("ethereum, wat, bitcoin", "usd").unwrap();
        let quotes = read(ANSWER, &query).unwrap();
        assert_eq!(quotes.len(), 2);
        assert_eq!(
            (quotes[0].ticker(), quotes[0].change),
            ("ETH".to_owned(), Some(-1.25))
        );
        assert_eq!(query.money(quotes[1].price), "$60,538");
        assert_eq!(
            Query::parse("x", "chf").unwrap().money(169.72),
            "169.72 CHF"
        );
        assert!(read("{}", &query).unwrap_err().contains("none"));
        assert!(read("<html>", &query).is_err());
    }
}
