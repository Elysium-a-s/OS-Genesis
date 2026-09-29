use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Apply,
    Revert,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredConfirmation {
    Provider,
    Device,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorDecision {
    pub schema_version: String,
    pub decision_id: Uuid,
    pub issuer: String,
    pub household_id: String,
    pub central_unit_id: Uuid,
    pub subject_id: Uuid,
    pub device_id: String,
    pub capability_id: String,
    pub requested_value: bool,
    pub operation: Operation,
    pub valid_from: String,
    pub expires_at: String,
    pub reason_code: String,
    pub idempotency_key: String,
    pub required_confirmation: RequiredConfirmation,
}

impl BehaviorDecision {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let decision: Self = serde_json::from_str(raw).map_err(|_| "invalid decision JSON")?;
        decision.validate()?;
        Ok(decision)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != "1.0" || self.issuer != "behavior-engine" {
            return Err("unsupported decision version or issuer".into());
        }
        for id in [
            &self.household_id,
            &self.device_id,
            &self.capability_id,
            &self.idempotency_key,
        ] {
            if id.is_empty()
                || id.len() > 128
                || !id.as_bytes()[0].is_ascii_alphanumeric()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
            {
                return Err("invalid opaque identifier".into());
            }
        }
        if self.reason_code.is_empty()
            || self.reason_code.len() > 64
            || !self.reason_code.as_bytes()[0].is_ascii_lowercase()
            || !self
                .reason_code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err("invalid reason code".into());
        }
        let from = parse_utc(&self.valid_from)?;
        let until = parse_utc(&self.expires_at)?;
        if from >= until {
            return Err("invalid decision time window".into());
        }
        Ok(())
    }

    pub fn active_at(&self, now: DateTime<Utc>) -> bool {
        let Ok(from) = parse_utc(&self.valid_from) else {
            return false;
        };
        let Ok(until) = parse_utc(&self.expires_at) else {
            return false;
        };
        from <= now && now < until
    }
}

fn parse_utc(raw: &str) -> Result<DateTime<Utc>, String> {
    if !raw.ends_with('Z') {
        return Err("decision timestamps must be UTC".into());
    }
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| "invalid decision timestamp".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example() -> String {
        json!({
            "schema_version":"1.0",
            "decision_id":"ff77bdb0-70af-4f2a-a913-76609b66761b",
            "issuer":"behavior-engine",
            "household_id":"pilot-home",
            "central_unit_id":"ad19a578-21e2-453f-a57c-1913350be34e",
            "subject_id":"64582f6b-38a5-48dd-9ed4-ae02949c7740",
            "device_id":"ha:light.living",
            "capability_id":"power",
            "requested_value":true,
            "operation":"apply",
            "valid_from":"2026-09-29T18:00:00Z",
            "expires_at":"2026-09-29T19:00:00Z",
            "reason_code":"goal_verified",
            "idempotency_key":"behavior:ff77bdb0-70af-4f2a-a913-76609b66761b",
            "required_confirmation":"device"
        })
        .to_string()
    }

    #[test]
    fn accepts_valid_decision_and_checks_window() {
        let decision = BehaviorDecision::parse(&example()).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-09-29T18:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(decision.active_at(now));
    }

    #[test]
    fn rejects_unknown_fields_and_inverted_window() {
        let mut value: serde_json::Value = serde_json::from_str(&example()).unwrap();
        value["access_token"] = json!("secret");
        assert!(BehaviorDecision::parse(&value.to_string()).is_err());
        value.as_object_mut().unwrap().remove("access_token");
        value["expires_at"] = json!("2026-09-29T17:00:00Z");
        assert!(BehaviorDecision::parse(&value.to_string()).is_err());
    }
}
