//! Provider webhook verification and filtering. Verification always covers the
//! original request bytes, before parsing or giving any payload to a model.

use anyhow::{anyhow, bail, Result};
use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::{routines::When, secrets::Secrets};

#[derive(Debug, PartialEq)]
pub enum Event {
    Challenge(String),
    Ignore,
    Run { id: Option<String>, payload: String },
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
}

fn verify(secret: &str, message: &[u8], signature: &str, prefix: &str) -> Result<()> {
    let signature = signature.strip_prefix(prefix).ok_or_else(|| anyhow!("Missing event signature"))?;
    let bytes = hex::decode(signature).map_err(|_| anyhow!("Invalid event signature"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
    mac.update(message);
    mac.verify_slice(&bytes).map_err(|_| anyhow!("Invalid event signature"))
}

pub fn check(when: &When, headers: &HeaderMap, body: &str, secrets: &Secrets, now_secs: i64) -> Result<Event> {
    if body.len() > 256 * 1024 { bail!("Event payload is at most 256 KiB"); }
    if matches!(when, When::Webhook) {
        let id = header(headers, "idempotency-key");
        if id.len() > 256 { bail!("Idempotency key is too long"); }
        return Ok(Event::Run { id: (!id.is_empty()).then(|| id.to_string()), payload: body.to_string() });
    }
    let When::Event { source, event, filters, signing_secret } = when else { bail!("This routine has no event trigger"); };
    let secret = if signing_secret.is_empty() { None } else { Some(secrets.get(signing_secret).ok_or_else(|| anyhow!("Save the signing key before enabling this trigger"))?) };
    match source.as_str() {
        "github" => verify(secret.as_deref().ok_or_else(|| anyhow!("Missing GitHub signing key"))?, body.as_bytes(), header(headers, "x-hub-signature-256"), "sha256=")?,
        "slack" => {
            let timestamp = header(headers, "x-slack-request-timestamp");
            let seconds: i64 = timestamp.parse().map_err(|_| anyhow!("Missing Slack timestamp"))?;
            if now_secs.abs_diff(seconds) > 300 { bail!("Expired Slack event"); }
            verify(secret.as_deref().ok_or_else(|| anyhow!("Missing Slack signing key"))?, format!("v0:{timestamp}:{body}").as_bytes(), header(headers, "x-slack-signature"), "v0=")?;
        }
        "teams" => {
            // Teams Workflows can POST here. This is not a Microsoft Graph
            // subscription and does not claim to authenticate Graph notifications.
            if let Some(secret) = secret { verify(&secret, body.as_bytes(), header(headers, "x-teambot-signature-256"), "sha256=")?; }
        }
        _ => bail!("Unknown event source"),
    }
    let value: Value = serde_json::from_str(body).map_err(|_| anyhow!("An event body must be JSON"))?;
    if source == "slack" && value["type"] == "url_verification" {
        return Ok(Event::Challenge(value["challenge"].as_str().ok_or_else(|| anyhow!("Missing Slack challenge"))?.to_string()));
    }
    let actual = match source.as_str() {
        "github" => header(headers, "x-github-event"),
        "slack" => {
            if value["type"] != "event_callback" || value["event"]["bot_id"].is_string() || value["event"]["subtype"] == "bot_message" { return Ok(Event::Ignore); }
            value["event"]["type"].as_str().unwrap_or_default()
        }
        _ => value["type"].as_str().unwrap_or("message"),
    };
    if actual == "ping" || (!event.is_empty() && event != actual) { return Ok(Event::Ignore); }
    for (pointer, expected) in filters {
        let Some(actual) = value.pointer(pointer) else { return Ok(Event::Ignore) };
        let actual = actual.as_str().map(str::to_string).unwrap_or_else(|| actual.to_string());
        if actual != *expected { return Ok(Event::Ignore); }
    }
    let id = match source.as_str() {
        "github" => header(headers, "x-github-delivery"),
        "slack" => value["event_id"].as_str().unwrap_or_default(),
        _ => header(headers, "idempotency-key"),
    };
    if id.is_empty() && source != "teams" { bail!("Provider event ID is required for deduplication"); }
    if id.len() > 256 { bail!("Event ID is too long"); }
    Ok(Event::Run { id: (!id.is_empty()).then(|| format!("{source}:{id}")), payload: json!({ "source": source, "event": actual, "body": value }).to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(secret: &str, message: &str, prefix: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap(); mac.update(message.as_bytes());
        format!("{prefix}{}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn github_signatures_and_filters_are_checked_before_dispatch() {
        let home = tempfile::tempdir().unwrap(); let secrets = Secrets::open(home.path()); secrets.set("SIGNING", "key-1234").unwrap();
        let when = When::Event { source: "github".into(), event: "push".into(), filters: [("/ref".into(), "refs/heads/main".into())].into(), signing_secret: "SIGNING".into() };
        let body = r#"{"ref":"refs/heads/main"}"#;
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "push".parse().unwrap()); headers.insert("x-github-delivery", "delivery-1".parse().unwrap());
        assert!(check(&when, &headers, body, &secrets, 0).is_err());
        headers.insert("x-hub-signature-256", signature("key-1234", body, "sha256=").parse().unwrap());
        assert!(matches!(check(&when, &headers, body, &secrets, 0).unwrap(), Event::Run { id: Some(id), .. } if id == "github:delivery-1"));
        assert!(check(&when, &headers, "{}", &secrets, 0).is_err());
        headers.insert("x-github-event", "issues".parse().unwrap());
        assert_eq!(check(&when, &headers, body, &secrets, 0).unwrap(), Event::Ignore);
    }

    #[test]
    fn slack_challenges_require_a_fresh_valid_signature() {
        let home = tempfile::tempdir().unwrap(); let secrets = Secrets::open(home.path()); secrets.set("SLACK", "slack-key").unwrap();
        let when = When::Event { source: "slack".into(), event: "message".into(), filters: Default::default(), signing_secret: "SLACK".into() };
        let body = r#"{"type":"url_verification","challenge":"hello"}"#;
        let mut headers = HeaderMap::new(); headers.insert("x-slack-request-timestamp", "1000".parse().unwrap());
        headers.insert("x-slack-signature", signature("slack-key", &format!("v0:1000:{body}"), "v0=").parse().unwrap());
        assert_eq!(check(&when, &headers, body, &secrets, 1001).unwrap(), Event::Challenge("hello".into()));
        assert!(check(&when, &headers, body, &secrets, 1301).is_err());
    }
}
