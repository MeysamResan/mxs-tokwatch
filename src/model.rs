use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// A deliberately narrow, credential-free cache of account usage information.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageSnapshot {
    pub fetched_at: u64,
    pub pools: Vec<UsagePool>,
    pub reset_credits: Option<ResetCredits>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsagePool {
    pub id: String,
    pub name: String,
    pub windows: Vec<UsageWindow>,
    pub plan: Option<String>,
    pub credits: Option<ExtraCredits>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageWindow {
    pub key: String,
    pub kind: String,
    pub used_percent: Option<f64>,
    pub duration_mins: Option<u64>,
    pub resets_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResetCredits {
    /// The service's count is authoritative; detail rows can be capped.
    pub available_count: Option<u64>,
    /// None means details were unavailable, while an empty list is known empty.
    /// A None entry represents an available reset without an expiry date.
    pub expirations: Option<Vec<Option<u64>>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExtraCredits {
    pub balance: Option<String>,
    pub unlimited: Option<bool>,
    pub has_credits: Option<bool>,
}

impl UsageSnapshot {
    /// Accepts the result of account/rateLimits/read, not the JSON-RPC envelope.
    pub fn from_response(value: Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "Codex returned an invalid usage response.".to_string())?;
        let mut pools = Vec::new();
        if let Some(map) = object.get("rateLimitsByLimitId").and_then(Value::as_object) {
            for (id, value) in map {
                if let Some(pool) = parse_pool(id, value) {
                    pools.push(pool);
                }
            }
        }
        // Older Codex versions only return the single-bucket view. It is also
        // useful if a newer server supplies an empty or unusable map.
        if pools.is_empty() {
            if let Some(value) = object.get("rateLimits") {
                let id = value
                    .get("limitId")
                    .and_then(Value::as_str)
                    .unwrap_or("codex");
                if let Some(pool) = parse_pool(id, value) {
                    pools.push(pool);
                }
            }
        }
        pools.sort_by(|a, b| {
            (!a.id.eq_ignore_ascii_case("codex"))
                .cmp(&(!b.id.eq_ignore_ascii_case("codex")))
                .then_with(|| a.id.cmp(&b.id))
        });
        let reset_credits = object.get("rateLimitResetCredits").and_then(parse_resets);
        if pools.is_empty() && reset_credits.is_none() {
            return Err("Codex has not returned usage information for this account.".to_string());
        }
        Ok(Self {
            fetched_at: unix_now(),
            pools,
            reset_credits,
        })
    }

    /// Retain an explicit choice; otherwise show the most restrictive known
    /// window in the main Codex pool. Unknown readings never become zero.
    pub fn selected_window(&self, key: Option<&str>) -> Option<(&UsagePool, &UsageWindow)> {
        if let Some(key) = key {
            for pool in &self.pools {
                if let Some(window) = pool.windows.iter().find(|window| window.key == key) {
                    return Some((pool, window));
                }
            }
        }
        let pool = self.pools.iter().find(|pool| !pool.windows.is_empty())?;
        let window = pool
            .windows
            .iter()
            .filter(|window| window.remaining_percent().is_some())
            .min_by_key(|window| window.remaining_percent())
            .or_else(|| pool.windows.first())?;
        Some((pool, window))
    }

    pub fn plan(&self) -> Option<&str> {
        self.pools.iter().find_map(|pool| pool.plan.as_deref())
    }

    pub fn extra_credits(&self) -> Option<&ExtraCredits> {
        self.pools.iter().find_map(|pool| pool.credits.as_ref())
    }

    pub fn age_seconds(&self, now: u64) -> u64 {
        now.saturating_sub(self.fetched_at)
    }

    pub fn is_stale(&self, now: u64, poll_seconds: u64) -> bool {
        self.fetched_at > now.saturating_add(60)
            || self.age_seconds(now) > poll_seconds.saturating_mul(2).max(180)
    }
}

impl UsageWindow {
    pub fn remaining_percent(&self) -> Option<u8> {
        self.used_percent
            .filter(|value| value.is_finite())
            .map(|used| (100.0 - used.clamp(0.0, 100.0)).floor() as u8)
    }

    pub fn duration_label(&self) -> String {
        match self.duration_mins {
            Some(10080) => "Weekly".to_string(),
            Some(1440) => "Daily".to_string(),
            Some(minutes) if minutes > 0 && minutes % 1440 == 0 => {
                format!("{}-day", minutes / 1440)
            }
            Some(minutes) if minutes > 0 && minutes % 60 == 0 => {
                format!("{}-hour", minutes / 60)
            }
            Some(minutes) if minutes > 0 => format!("{minutes}-minute"),
            _ => "Usage window".to_string(),
        }
    }

    pub fn reset_countdown(&self, now: u64) -> String {
        countdown(self.resets_at, now)
    }
}

impl ResetCredits {
    /// This is the earliest *known* expiry; the service may truncate details.
    pub fn next_expiry(&self) -> Option<u64> {
        if self.available_count == Some(0) {
            return None;
        }
        self.expirations.as_ref()?.iter().flatten().copied().min()
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn countdown(target: Option<u64>, now: u64) -> String {
    let Some(target) = target else {
        return "Unavailable".to_string();
    };
    let seconds = target.saturating_sub(now);
    if seconds == 0 {
        return "Due now".to_string();
    }
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours:02}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".to_string()
    }
}

fn parse_pool(id: &str, value: &Value) -> Option<UsagePool> {
    let object = value.as_object()?;
    let id = clean_text(id, 128)?;
    let name = object
        .get("limitName")
        .and_then(Value::as_str)
        .and_then(|text| clean_text(text, 128))
        .unwrap_or_else(|| {
            if id.eq_ignore_ascii_case("codex") {
                "Codex".to_string()
            } else {
                id.clone()
            }
        });
    let windows = ["primary", "secondary"]
        .iter()
        .filter_map(|kind| {
            let value = object.get(*kind)?.as_object()?;
            let used_percent = value
                .get("usedPercent")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite());
            let duration_mins = value
                .get("windowDurationMins")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0);
            let resets_at = value
                .get("resetsAt")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0);
            if used_percent.is_none() && duration_mins.is_none() && resets_at.is_none() {
                return None;
            }
            Some(UsageWindow {
                key: format!("{id}:{kind}"),
                kind: (*kind).to_string(),
                used_percent,
                duration_mins,
                resets_at,
            })
        })
        .collect::<Vec<_>>();
    let plan = object
        .get("planType")
        .and_then(Value::as_str)
        .and_then(|text| clean_text(text, 48));
    let credits = object.get("credits").and_then(parse_extra_credits);
    if windows.is_empty() && plan.is_none() && credits.is_none() {
        return None;
    }
    Some(UsagePool {
        id,
        name,
        windows,
        plan,
        credits,
    })
}

fn parse_extra_credits(value: &Value) -> Option<ExtraCredits> {
    let object = value.as_object()?;
    let balance = object.get("balance").and_then(|value| match value {
        Value::String(text) => clean_text(text, 64),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    });
    let unlimited = object.get("unlimited").and_then(Value::as_bool);
    let has_credits = object.get("hasCredits").and_then(Value::as_bool);
    if balance.is_none() && unlimited.is_none() && has_credits.is_none() {
        return None;
    }
    Some(ExtraCredits {
        balance,
        unlimited,
        has_credits,
    })
}

fn parse_resets(value: &Value) -> Option<ResetCredits> {
    let object = value.as_object()?;
    let available_count = object.get("availableCount").and_then(Value::as_u64);
    let expirations = object.get("credits").and_then(Value::as_array).map(|rows| {
        rows.iter()
            .filter(|row| row.get("status").and_then(Value::as_str) == Some("available"))
            .map(|row| {
                row.get("expiresAt")
                    .and_then(Value::as_u64)
                    .filter(|value| *value > 0)
            })
            .collect::<Vec<_>>()
    });
    if available_count.is_none() && expirations.is_none() {
        return None;
    }
    Some(ResetCredits {
        available_count,
        expirations,
    })
}

fn clean_text(text: &str, max_chars: usize) -> Option<String> {
    let text: String = text
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn current_schema_prefers_map_and_keeps_authoritative_reset_count() {
        let snapshot = UsageSnapshot::from_response(json!({
            "rateLimits": {"primary": {"usedPercent": 1}},
            "rateLimitsByLimitId": {
                "spark": {"primary": {"usedPercent": 10, "windowDurationMins": 300}},
                "codex": {"primary": {"usedPercent": 73.2, "windowDurationMins": 10080, "resetsAt": 1789148169}, "secondary": null,
                    "planType": "pro", "credits": {"balance": "35.27", "hasCredits": true, "unlimited": false}}
            },
            "rateLimitResetCredits": {"availableCount": 2, "credits": [{"id":"do-not-cache", "status":"available", "expiresAt":1791092050}]},
            "accessToken": "do-not-cache"
        })).unwrap();
        let (pool, window) = snapshot.selected_window(None).unwrap();
        assert_eq!(pool.id, "codex");
        assert_eq!(window.remaining_percent(), Some(26));
        assert_eq!(window.duration_label(), "Weekly");
        assert_eq!(window.resets_at, Some(1789148169));
        assert_eq!(snapshot.plan(), Some("pro"));
        assert_eq!(
            snapshot.extra_credits().unwrap().balance.as_deref(),
            Some("35.27")
        );
        let resets = snapshot.reset_credits.as_ref().unwrap();
        assert_eq!(resets.available_count, Some(2));
        assert_eq!(resets.expirations.as_ref().unwrap().len(), 1);
        assert_eq!(resets.next_expiry(), Some(1791092050));
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("do-not-cache")
        );
    }

    #[test]
    fn unknown_fields_are_not_zero_and_optional_windows_are_preserved() {
        let snapshot = UsageSnapshot::from_response(json!({"rateLimits": {
            "primary": {"usedPercent": null,"windowDurationMins": 300,"resetsAt":null},
            "secondary": {"usedPercent":0,"windowDurationMins":null,"resetsAt":null}
        }, "rateLimitResetCredits": null}))
        .unwrap();
        let pool = &snapshot.pools[0];
        assert_eq!(pool.windows[0].remaining_percent(), None);
        assert_eq!(pool.windows[0].duration_label(), "5-hour");
        assert_eq!(pool.windows[1].remaining_percent(), Some(100));
        assert_eq!(pool.windows[1].duration_label(), "Usage window");
        assert_eq!(pool.windows[0].reset_countdown(0), "Unavailable");
        assert!(snapshot.reset_credits.is_none());
    }

    #[test]
    fn empty_map_falls_back_and_percentages_are_clamped() {
        let snapshot = UsageSnapshot::from_response(json!({"rateLimitsByLimitId":{},"rateLimits":{
            "primary":{"usedPercent":-12},"secondary":{"usedPercent":150}
        }}))
        .unwrap();
        assert_eq!(snapshot.pools[0].windows[0].remaining_percent(), Some(100));
        assert_eq!(
            snapshot
                .selected_window(None)
                .unwrap()
                .1
                .remaining_percent(),
            Some(0)
        );
        assert_eq!(
            snapshot
                .selected_window(Some("codex:primary"))
                .unwrap()
                .1
                .remaining_percent(),
            Some(100)
        );
    }

    #[test]
    fn no_data_is_an_error() {
        for value in [
            json!(null),
            json!({}),
            json!({"rateLimits":null}),
            json!({"rateLimits":{"primary":{"usedPercent":"bad"}}}),
        ] {
            assert!(UsageSnapshot::from_response(value).is_err());
        }
    }

    #[test]
    fn reset_details_distinguish_unknown_empty_and_consumed() {
        let unknown = parse_resets(&json!({"availableCount":2,"credits":null})).unwrap();
        assert_eq!(unknown.available_count, Some(2));
        assert!(unknown.expirations.is_none());
        let empty = parse_resets(&json!({"availableCount":0,"credits":[]})).unwrap();
        assert_eq!(empty.expirations, Some(vec![]));
        let mixed = parse_resets(&json!({"credits":[
            {"status":"consumed","expiresAt":1},
            {"status":"available","expiresAt":300},
            {"status":"available","expiresAt":null},
            {"status":"available","expiresAt":200}
        ]}))
        .unwrap();
        assert_eq!(mixed.available_count, None);
        assert_eq!(mixed.next_expiry(), Some(200));
        assert_eq!(mixed.expirations, Some(vec![Some(300), None, Some(200)]));
    }

    #[test]
    fn countdown_does_not_claim_allowance_reset() {
        assert_eq!(countdown(Some(100), 101), "Due now");
        assert_eq!(countdown(Some(101), 100), "<1m");
        assert_eq!(countdown(Some(3600), 0), "1h 00m");
        assert_eq!(countdown(Some(86400 * 4 + 3600 * 8), 0), "4d 08h");
    }

    #[test]
    fn cache_age_handles_clock_changes() {
        let snapshot = UsageSnapshot {
            fetched_at: 1000,
            pools: vec![],
            reset_credits: None,
        };
        assert!(!snapshot.is_stale(1100, 120));
        assert!(snapshot.is_stale(1241, 120));
        assert!(snapshot.is_stale(800, 120));
    }
}
