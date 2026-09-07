use std::{env, fs, path::PathBuf, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use reqwest::blocking::{Client, RequestBuilder};
use serde::Deserialize;
use serde_json::Value;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";

#[derive(Clone, Debug, Default)]
pub struct UsageWindow {
    pub used_percent: f64,
    pub reset_at: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct UsageSnapshot {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub primary: Option<UsageWindow>,
    pub secondary: Option<UsageWindow>,
    pub reset_credit_count: Option<u32>,
    pub fetched_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct ResetCreditsSnapshot {
    pub available_count: u32,
    pub earliest_expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawUsageResponse {
    email: Option<String>,
    plan_type: Option<String>,
    rate_limit: Option<RawRateLimit>,
    rate_limit_reset_credits: Option<RawResetCreditsSummary>,
}

#[derive(Debug, Deserialize)]
struct RawRateLimit {
    primary_window: Option<RawWindow>,
    secondary_window: Option<RawWindow>,
}

#[derive(Debug, Deserialize)]
struct RawWindow {
    used_percent: Option<f64>,
    reset_at: Option<i64>,
    reset_after_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawResetCreditsSummary {
    available_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RawResetCreditsResponse {
    available_count: Option<u32>,
    #[serde(default)]
    credits: Vec<RawResetCredit>,
}

#[derive(Debug, Deserialize)]
struct RawResetCredit {
    status: Option<String>,
    expires_at: Option<String>,
}

struct AuthContext {
    access_token: String,
    account_id: Option<String>,
}

pub fn fetch_usage() -> Result<UsageSnapshot, String> {
    let auth = load_auth_context()?;
    let client = http_client()?;
    let response = base_get(&client, USAGE_URL, &auth)
        .send()
        .map_err(|error| format!("Usage request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("Cannot read usage response: {error}"))?;

    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "Codex login has expired; open Codex and sign in again".to_string(),
            403 => "OpenAI rejected the usage request (HTTP 403)".to_string(),
            code => format!("Usage request returned HTTP {code}"),
        });
    }

    parse_usage(&body)
}

pub fn fetch_reset_credits() -> Result<ResetCreditsSnapshot, String> {
    let auth = load_auth_context()?;
    let client = http_client()?;
    let response = base_get(&client, RESET_CREDITS_URL, &auth)
        .header("oai-product-sku", "CODEX")
        .header("originator", "Codex Desktop")
        .send()
        .map_err(|error| format!("Reset-credit request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("Cannot read reset-credit response: {error}"))?;

    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "Codex login expired while reading reset credits".to_string(),
            403 => "OpenAI rejected the reset-credit request (HTTP 403)".to_string(),
            429 => "OpenAI rate-limited the reset-credit request (HTTP 429)".to_string(),
            code => format!("Reset-credit request returned HTTP {code}"),
        });
    }

    parse_reset_credits(&body)
}

pub fn auth_file_path() -> PathBuf {
    if let Some(codex_home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(codex_home).join("auth.json");
    }

    if let Some(user_profile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return PathBuf::from(user_profile).join(".codex").join("auth.json");
    }

    PathBuf::from(".codex").join("auth.json")
}

fn load_auth_context() -> Result<AuthContext, String> {
    let auth_path = auth_file_path();
    let auth_text = fs::read_to_string(&auth_path).map_err(|error| {
        format!(
            "Cannot read Codex auth file: {} ({error})",
            auth_path.display()
        )
    })?;
    let auth: Value = serde_json::from_str(&auth_text)
        .map_err(|error| format!("Invalid Codex auth file: {error}"))?;

    let access_token = nested_string(&auth, &["tokens", "access_token"])
        .or_else(|| nested_string(&auth, &["access_token"]))
        .ok_or_else(|| "Codex access token was not found".to_string())?;

    let account_id = nested_string(&auth, &["tokens", "account_id"])
        .or_else(|| nested_string(&auth, &["tokens", "chatgpt_account_id"]))
        .or_else(|| nested_string(&auth, &["chatgpt_account_id"]))
        .or_else(|| jwt_claim(&access_token, "chatgpt_account_id"))
        .or_else(|| {
            nested_string(&auth, &["tokens", "id_token"])
                .and_then(|token| jwt_claim(&token, "chatgpt_account_id"))
        });

    Ok(AuthContext {
        access_token,
        account_id,
    })
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Cannot create HTTP client: {error}"))
}

fn base_get(client: &Client, url: &str, auth: &AuthContext) -> RequestBuilder {
    let mut request = client
        .get(url)
        .bearer_auth(&auth.access_token)
        .header("Accept", "application/json")
        .header("Referer", "https://chatgpt.com/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36",
        );

    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    request
}

fn parse_usage(body: &str) -> Result<UsageSnapshot, String> {
    let raw: RawUsageResponse =
        serde_json::from_str(body).map_err(|error| format!("Invalid usage response: {error}"))?;
    let now = chrono::Utc::now().timestamp();
    let rate_limit = raw.rate_limit;

    Ok(UsageSnapshot {
        email: clean_string(raw.email),
        plan_type: clean_string(raw.plan_type),
        primary: rate_limit
            .as_ref()
            .and_then(|limit| limit.primary_window.as_ref())
            .and_then(|window| normalize_window(window, now)),
        secondary: rate_limit
            .as_ref()
            .and_then(|limit| limit.secondary_window.as_ref())
            .and_then(|window| normalize_window(window, now)),
        reset_credit_count: raw
            .rate_limit_reset_credits
            .and_then(|summary| summary.available_count),
        fetched_at: now,
    })
}

fn parse_reset_credits(body: &str) -> Result<ResetCreditsSnapshot, String> {
    let raw: RawResetCreditsResponse = serde_json::from_str(body)
        .map_err(|error| format!("Invalid reset-credit response: {error}"))?;

    let available = raw
        .credits
        .iter()
        .filter(|credit| {
            credit
                .status
                .as_deref()
                .map(|status| status.eq_ignore_ascii_case("available"))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    let earliest_expires_at = available
        .iter()
        .filter_map(|credit| credit.expires_at.as_deref())
        .filter_map(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp())
        .min();

    Ok(ResetCreditsSnapshot {
        available_count: raw.available_count.unwrap_or(available.len() as u32),
        earliest_expires_at,
    })
}

fn normalize_window(window: &RawWindow, now: i64) -> Option<UsageWindow> {
    let used_percent = window.used_percent?.clamp(0.0, 100.0);
    let reset_at = window.reset_at.map(normalize_timestamp).or_else(|| {
        window
            .reset_after_seconds
            .filter(|value| *value >= 0)
            .map(|value| now + value)
    });

    Some(UsageWindow {
        used_percent,
        reset_at,
    })
}

fn normalize_timestamp(timestamp: i64) -> i64 {
    if timestamp > 1_000_000_000_000 {
        timestamp / 1000
    } else {
        timestamp
    }
}

fn clean_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn nested_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn jwt_claim(token: &str, claim: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;

    value
        .get(claim)
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("https://api.openai.com/auth")
                .and_then(|nested| nested.get(claim))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primary_and_secondary_windows() {
        let payload = r#"{
            "email": "person@example.com",
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 23,
                    "reset_at": 1900000000
                },
                "secondary_window": {
                    "used_percent": 61.5,
                    "reset_after_seconds": 3600
                }
            },
            "rate_limit_reset_credits": { "available_count": 2 }
        }"#;

        let snapshot = parse_usage(payload).expect("usage response should parse");
        assert_eq!(snapshot.email.as_deref(), Some("person@example.com"));
        assert_eq!(snapshot.plan_type.as_deref(), Some("plus"));
        assert_eq!(snapshot.reset_credit_count, Some(2));
        assert_eq!(snapshot.primary.unwrap().used_percent, 23.0);
        assert_eq!(snapshot.secondary.unwrap().used_percent, 61.5);
    }

    #[test]
    fn parses_available_reset_credit_expiry() {
        let payload = r#"{
            "available_count": 2,
            "credits": [
                {"status":"available","expires_at":"2026-09-20T12:00:00Z"},
                {"status":"available","expires_at":"2026-09-18T08:30:00Z"},
                {"status":"consumed","expires_at":"2026-09-10T00:00:00Z"}
            ]
        }"#;
        let snapshot = parse_reset_credits(payload).expect("reset credits should parse");
        assert_eq!(snapshot.available_count, 2);
        assert_eq!(snapshot.earliest_expires_at, Some(1789720200));
    }

    #[test]
    fn extracts_account_id_from_jwt() {
        let payload = URL_SAFE_NO_PAD.encode(r#"{"chatgpt_account_id":"acct_test"}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(
            jwt_claim(&token, "chatgpt_account_id").as_deref(),
            Some("acct_test")
        );
    }
}
