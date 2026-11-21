// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The services, how each is asked for usage and what its answer means.

use serde_json::Value;

use crate::clock::parse_timestamp;

/// A service whose limits can be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
    OpenRouter,
    DeepSeek,
}

impl Provider {
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::OpenRouter, Self::DeepSeek];

    /// The name settings use.
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenRouter => "openrouter",
            Self::DeepSeek => "deepseek",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenRouter => "OpenRouter",
            Self::DeepSeek => "DeepSeek",
        }
    }

    /// The file holding the sign-in of the service's own tool, for the
    /// services that are asked with it.
    pub fn credentials_file(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("~/.claude/.credentials.json"),
            Self::Codex => Some("~/.codex/auth.json"),
            Self::OpenRouter | Self::DeepSeek => None,
        }
    }

    /// The setting holding the API key, for the services asked with one.
    pub fn key_setting(self) -> Option<&'static str> {
        match self {
            Self::OpenRouter => Some("openrouter_key"),
            Self::DeepSeek => Some("deepseek_key"),
            Self::Claude | Self::Codex => None,
        }
    }

    /// The request for the usage. `secret` is the content of the
    /// credentials file or the API key; `now` is in seconds.
    pub fn call(self, secret: &str, now: u64) -> Result<Call, String> {
        match self {
            Self::Claude => claude_call(secret, now),
            Self::Codex => codex_call(secret),
            Self::OpenRouter => Ok(Call::get("https://openrouter.ai/api/v1/key", secret.trim())),
            Self::DeepSeek => Ok(Call::get(
                "https://api.deepseek.com/user/balance",
                secret.trim(),
            )),
        }
    }

    /// What the body of a successful answer says.
    pub fn report(self, body: &str) -> Result<Report, String> {
        let answer: Value =
            serde_json::from_str(body).map_err(|_| "the answer is not JSON".to_owned())?;
        let report = match self {
            Self::Claude => claude_report(&answer),
            Self::Codex => codex_report(&answer),
            Self::OpenRouter => openrouter_report(&answer),
            Self::DeepSeek => deepseek_report(&answer),
        };
        report.ok_or_else(|| "the answer has no usage in a known form".to_owned())
    }
}

/// A request for usage.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub url: &'static str,
    pub headers: Vec<(&'static str, String)>,
    /// The plan, when the credentials already name it.
    pub plan: Option<String>,
}

impl Call {
    fn get(url: &'static str, token: &str) -> Self {
        Self {
            url,
            headers: vec![
                ("Authorization", format!("Bearer {token}")),
                ("Accept", "application/json".to_owned()),
            ],
            plan: None,
        }
    }
}

/// One limit or balance.
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    pub label: String,
    /// The share used, 0 to 1, when the limit is a share.
    pub used: Option<f32>,
    /// The amount in the service's own terms, when it gives one.
    pub amount: Option<String>,
    /// When the limit starts over, in seconds since 1970.
    pub resets_at: Option<u64>,
}

/// The usage of one account.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub plan: Option<String>,
    pub meters: Vec<Meter>,
}

/// Why a service did not answer with usage, by HTTP status.
pub fn refusal(provider: Provider, status: u16) -> String {
    match (status, provider.credentials_file()) {
        (401 | 403, Some(_)) => format!(
            "The sign-in was refused (HTTP {status}); using {} renews it.",
            match provider {
                Provider::Claude => "Claude Code",
                _ => "Codex",
            }
        ),
        (401 | 403, None) => format!("The API key was refused (HTTP {status})."),
        (429, _) => "Asked too often (HTTP 429); trying again later.".to_owned(),
        _ => format!("The service answered HTTP {status}."),
    }
}

fn claude_call(credentials: &str, now: u64) -> Result<Call, String> {
    let credentials: Value = serde_json::from_str(credentials)
        .map_err(|_| "The Claude Code credentials are not JSON.".to_owned())?;
    let sign_in = &credentials["claudeAiOauth"];
    let token = sign_in["accessToken"]
        .as_str()
        .ok_or("Claude Code is not signed in with a subscription.")?;
    // The token is not renewed here: that would race with Claude Code.
    if let Some(expires_ms) = sign_in["expiresAt"].as_u64()
        && expires_ms / 1000 <= now
    {
        return Err("The Claude Code sign-in has expired; using Claude Code renews it.".into());
    }
    let mut call = Call::get("https://api.anthropic.com/api/oauth/usage", token);
    call.headers
        .push(("anthropic-beta", "oauth-2025-04-20".to_owned()));
    call.plan = sign_in["subscriptionType"].as_str().map(plan_name);
    Ok(call)
}

fn claude_report(answer: &Value) -> Option<Report> {
    let meters: Vec<Meter> = answer
        .as_object()?
        .iter()
        .filter_map(|(key, window)| {
            let used = window.get("utilization")?.as_f64()?;
            let label = match key.as_str() {
                "five_hour" => "5-hour session".to_owned(),
                "seven_day" => "Week, all models".to_owned(),
                "seven_day_opus" => "Week, Opus".to_owned(),
                "seven_day_sonnet" => "Week, Sonnet".to_owned(),
                other => other.replace('_', " "),
            };
            Some(Meter {
                label,
                used: Some(share(used)),
                amount: None,
                resets_at: window["resets_at"].as_str().and_then(parse_timestamp),
            })
        })
        .collect();
    (!meters.is_empty()).then_some(Report { plan: None, meters })
}

fn codex_call(auth: &str) -> Result<Call, String> {
    let auth: Value =
        serde_json::from_str(auth).map_err(|_| "The Codex credentials are not JSON.".to_owned())?;
    let tokens = &auth["tokens"];
    let token = tokens["access_token"]
        .as_str()
        .ok_or("Codex is not signed in with a ChatGPT account.")?;
    let mut call = Call::get("https://chatgpt.com/backend-api/wham/usage", token);
    if let Some(account) = tokens["account_id"].as_str() {
        call.headers
            .push(("ChatGPT-Account-Id", account.to_owned()));
    }
    Ok(call)
}

fn codex_report(answer: &Value) -> Option<Report> {
    let limits = answer.get("rate_limit")?;
    let meters: Vec<Meter> = ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| {
            let window = limits.get(key)?;
            let used = window.get("used_percent")?.as_f64()?;
            let label = match window["limit_window_seconds"].as_u64() {
                Some(seconds) => window_name(seconds),
                None => key.replace('_', " "),
            };
            Some(Meter {
                label,
                used: Some(share(used)),
                amount: None,
                resets_at: window["reset_at"].as_u64(),
            })
        })
        .collect();
    (!meters.is_empty()).then_some(Report {
        plan: answer["plan_type"].as_str().map(plan_name),
        meters,
    })
}

fn openrouter_report(answer: &Value) -> Option<Report> {
    let key = answer.get("data")?;
    let used = key.get("usage")?.as_f64()?;
    let limit = key["limit"].as_f64().filter(|&limit| limit > 0.);
    let meter = Meter {
        label: "Credits".to_owned(),
        used: limit.map(|limit| (used / limit).clamp(0., 1.) as f32),
        amount: Some(match limit {
            Some(limit) => format!("${used:.2} of ${limit:.2} used"),
            None => format!("${used:.2} used, no limit on the key"),
        }),
        resets_at: None,
    };
    let plan = key["is_free_tier"]
        .as_bool()
        .map(|free| if free { "Free tier" } else { "Paid" }.to_owned());
    Some(Report {
        plan,
        meters: vec![meter],
    })
}

fn deepseek_report(answer: &Value) -> Option<Report> {
    let usable = answer["is_available"].as_bool().unwrap_or(true);
    let meters: Vec<Meter> = answer
        .get("balance_infos")?
        .as_array()?
        .iter()
        .filter_map(|balance| {
            let total = balance.get("total_balance")?.as_str()?;
            let currency = balance["currency"].as_str().unwrap_or_default();
            let note = if usable { "" } else { ", too low to use" };
            Some(Meter {
                label: "Balance".to_owned(),
                used: None,
                amount: Some(format!("{total} {currency}{note}").replace(" ,", ",")),
                resets_at: None,
            })
        })
        .collect();
    (!meters.is_empty()).then_some(Report {
        plan: Some("Prepaid".to_owned()),
        meters,
    })
}

/// A share from a percentage.
fn share(percent: f64) -> f32 {
    (percent / 100.).clamp(0., 1.) as f32
}

/// `max` becomes `Max plan`.
fn plan_name(plan: &str) -> String {
    let mut letters = plan.chars();
    match letters.next() {
        Some(first) => format!("{}{} plan", first.to_uppercase(), letters.as_str()),
        None => "Plan".to_owned(),
    }
}

/// What to call a limit that starts over every `seconds`.
fn window_name(seconds: u64) -> String {
    match seconds {
        604_800 => "Week".to_owned(),
        seconds if seconds % 86_400 == 0 => format!("{} days", seconds / 86_400),
        seconds if seconds % 3600 == 0 => format!("{} hours", seconds / 3600),
        seconds => format!("{} minutes", seconds / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_772_361_000;

    fn header<'a>(call: &'a Call, name: &str) -> Option<&'a str> {
        let found = call.headers.iter().find(|(header, _)| *header == name);
        found.map(|(_, value)| value.as_str())
    }

    #[test]
    fn claude_is_asked_with_the_sign_in_of_claude_code() {
        let credentials = r#"{"claudeAiOauth": {"accessToken": "token-a",
            "refreshToken": "token-r", "expiresAt": 1772361001000,
            "subscriptionType": "max"}}"#;
        let call = Provider::Claude.call(credentials, NOW).unwrap();
        assert_eq!(call.url, "https://api.anthropic.com/api/oauth/usage");
        assert_eq!(header(&call, "Authorization"), Some("Bearer token-a"));
        assert_eq!(header(&call, "anthropic-beta"), Some("oauth-2025-04-20"));
        assert_eq!(call.plan.as_deref(), Some("Max plan"));
        assert!(!format!("{call:?}").contains("token-r"));
    }

    #[test]
    fn an_expired_or_missing_claude_sign_in_is_not_used() {
        let expired = r#"{"claudeAiOauth": {"accessToken": "a", "expiresAt": 1772361000000}}"#;
        assert!(Provider::Claude.call(expired, NOW).is_err());
        assert!(Provider::Claude.call("{}", NOW).is_err());
        assert!(Provider::Claude.call("not json", NOW).is_err());
    }

    #[test]
    fn claude_usage_lists_each_window() {
        let body = r#"{
            "five_hour": {"utilization": 42.0, "resets_at": "2026-03-01T12:30:00.000000+00:00"},
            "seven_day": {"utilization": 7.5, "resets_at": null},
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": false}
        }"#;
        let report = Provider::Claude.report(body).unwrap();
        assert_eq!(report.plan, None);
        let [session, week] = report.meters.as_slice() else {
            panic!("expected two meters, got {:?}", report.meters);
        };
        assert_eq!(session.label, "5-hour session");
        assert_eq!(session.used, Some(0.42));
        assert_eq!(session.resets_at, Some(NOW + 7200));
        assert_eq!(week.label, "Week, all models");
        assert_eq!(week.used, Some(0.075));
        assert_eq!(week.resets_at, None);
    }

    #[test]
    fn codex_is_asked_for_the_signed_in_account() {
        let auth = r#"{"OPENAI_API_KEY": null,
            "tokens": {"access_token": "token-a", "account_id": "account-1"}}"#;
        let call = Provider::Codex.call(auth, NOW).unwrap();
        assert_eq!(call.url, "https://chatgpt.com/backend-api/wham/usage");
        assert_eq!(header(&call, "Authorization"), Some("Bearer token-a"));
        assert_eq!(header(&call, "ChatGPT-Account-Id"), Some("account-1"));
        assert!(
            Provider::Codex
                .call(r#"{"OPENAI_API_KEY": "sk"}"#, NOW)
                .is_err()
        );
    }

    #[test]
    fn codex_usage_lists_both_windows_and_the_plan() {
        let body = r#"{"plan_type": "plus", "rate_limit": {
            "primary_window": {"used_percent": 12, "limit_window_seconds": 18000,
                "reset_after_seconds": 600, "reset_at": 1772361600},
            "secondary_window": {"used_percent": 150, "limit_window_seconds": 604800,
                "reset_at": 1772900000}}}"#;
        let report = Provider::Codex.report(body).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Plus plan"));
        assert_eq!(report.meters[0].label, "5 hours");
        assert_eq!(report.meters[0].used, Some(0.12));
        assert_eq!(report.meters[0].resets_at, Some(1_772_361_600));
        assert_eq!(report.meters[1].label, "Week");
        assert_eq!(report.meters[1].used, Some(1.));
    }

    #[test]
    fn openrouter_reports_credits_with_and_without_a_limit() {
        let limited = r#"{"data": {"label": "k", "usage": 2.5, "limit": 10,
            "is_free_tier": false}}"#;
        let report = Provider::OpenRouter.report(limited).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Paid"));
        assert_eq!(report.meters[0].used, Some(0.25));
        assert_eq!(
            report.meters[0].amount.as_deref(),
            Some("$2.50 of $10.00 used")
        );

        let unlimited = r#"{"data": {"usage": 0.5, "limit": null, "is_free_tier": true}}"#;
        let report = Provider::OpenRouter.report(unlimited).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Free tier"));
        assert_eq!(report.meters[0].used, None);
        assert_eq!(
            report.meters[0].amount.as_deref(),
            Some("$0.50 used, no limit on the key")
        );
    }

    #[test]
    fn deepseek_reports_a_balance_per_currency() {
        let body = r#"{"is_available": true, "balance_infos": [
            {"currency": "USD", "total_balance": "110.00", "granted_balance": "10.00",
             "topped_up_balance": "100.00"}]}"#;
        let report = Provider::DeepSeek.report(body).unwrap();
        assert_eq!(report.meters[0].amount.as_deref(), Some("110.00 USD"));
        assert_eq!(report.meters[0].used, None);

        let empty = r#"{"is_available": false, "balance_infos": [
            {"currency": "CNY", "total_balance": "0.00"}]}"#;
        let report = Provider::DeepSeek.report(empty).unwrap();
        assert_eq!(
            report.meters[0].amount.as_deref(),
            Some("0.00 CNY, too low to use")
        );
    }

    #[test]
    fn answers_in_an_unknown_form_are_errors() {
        for provider in Provider::ALL {
            assert!(provider.report("<html>").is_err());
            assert!(provider.report("{}").is_err());
            assert!(provider.report(r#"{"error": {"message": "no"}}"#).is_err());
        }
    }

    #[test]
    fn refusals_say_what_to_do() {
        assert!(refusal(Provider::Claude, 401).contains("Claude Code"));
        assert!(refusal(Provider::Codex, 403).contains("Codex"));
        assert!(refusal(Provider::DeepSeek, 401).contains("API key"));
        assert!(refusal(Provider::OpenRouter, 500).contains("500"));
    }
}
