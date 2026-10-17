//! Strict `agentlounge://` deep-link allowlist parser and handlers.
//!
//! Allowed actions only:
//! - `open?route=<allowlisted path>` — navigation, no approval
//! - `dispatch?agent=<registered id>&text=<≤2000 chars>` — always
//!   `source_verified=false` → SourceUnverified PENDING_APPROVAL
//!
//! Halt/Resume, shell, path execution, and config writes are never accepted.
//! Rejections are logged without query parameter values.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SCHEME: &str = "agentlounge";
pub const SCHEME_PREFIX: &str = "agentlounge://";
pub const MAX_DISPATCH_TEXT_CHARS: usize = 2000;
pub const DEEP_LINK_SOURCE_AGENT: &str = "deep-link";
pub const DEEP_LINK_OPEN_EVENT: &str = "lounge://deep-link-open";
pub const EXTERNAL_TRIGGER_REASON: &str =
    "Dış Kaynaklı Tetikleme · source_verified=false — deep-link dispatch · PENDING_APPROVAL";

/// Routes that `open` may navigate to (exact match after decode).
pub const ALLOWED_OPEN_ROUTES: &[&str] = &[
    "/",
    "/dashboard",
    "/stream",
    "/vault",
    "/health",
    "/fleet",
    "/telemetry",
    "/quotas",
    "/settings",
    "/onboarding",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DeepLinkAction {
    Open { route: String },
    Dispatch { agent: String, text: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkReject {
    BadScheme,
    UnknownAction,
    UnknownParam { action: &'static str },
    MissingParam { name: &'static str },
    InvalidRoute,
    TextTooLong,
    ControlCharacter,
    EmptyValue { name: &'static str },
    Malformed,
}

impl DeepLinkReject {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BadScheme => "bad_scheme",
            Self::UnknownAction => "unknown_action",
            Self::UnknownParam { .. } => "unknown_param",
            Self::MissingParam { .. } => "missing_param",
            Self::InvalidRoute => "invalid_route",
            Self::TextTooLong => "text_too_long",
            Self::ControlCharacter => "control_character",
            Self::EmptyValue { .. } => "empty_value",
            Self::Malformed => "malformed",
        }
    }
}

/// Parse a deep-link URL. On rejection, callers must log **without** param values.
pub fn parse_deep_link(raw: &str) -> Result<DeepLinkAction, DeepLinkReject> {
    // Reject control characters on the raw input before whitespace trim (trailing
    // `\n` must not silently disappear into a valid open/dispatch).
    if contains_control_chars(raw) {
        return Err(DeepLinkReject::ControlCharacter);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DeepLinkReject::Malformed);
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with(SCHEME_PREFIX) {
        return Err(DeepLinkReject::BadScheme);
    }
    let rest = &trimmed[SCHEME_PREFIX.len()..];
    let (action_raw, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };
    let action = action_raw.trim_matches('/').to_ascii_lowercase();
    if action.is_empty() {
        return Err(DeepLinkReject::UnknownAction);
    }
    // Reject fragments and nested schemes.
    if action.contains(':') || action.contains('#') || action.contains('/') {
        return Err(DeepLinkReject::UnknownAction);
    }

    let params = parse_query(query)?;

    match action.as_str() {
        "open" => parse_open(params),
        "dispatch" => parse_dispatch(params),
        "halt" | "resume" | "shell" | "exec" | "run" | "config" => {
            Err(DeepLinkReject::UnknownAction)
        }
        _ => Err(DeepLinkReject::UnknownAction),
    }
}

fn parse_open(params: BTreeMap<String, String>) -> Result<DeepLinkAction, DeepLinkReject> {
    let allowed = ["route"];
    for key in params.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(DeepLinkReject::UnknownParam { action: "open" });
        }
    }
    let route = params
        .get("route")
        .ok_or(DeepLinkReject::MissingParam { name: "route" })?;
    if route.is_empty() {
        return Err(DeepLinkReject::EmptyValue { name: "route" });
    }
    if contains_control_chars(route) {
        return Err(DeepLinkReject::ControlCharacter);
    }
    if !ALLOWED_OPEN_ROUTES.contains(&route.as_str()) {
        return Err(DeepLinkReject::InvalidRoute);
    }
    Ok(DeepLinkAction::Open {
        route: route.clone(),
    })
}

fn parse_dispatch(params: BTreeMap<String, String>) -> Result<DeepLinkAction, DeepLinkReject> {
    let allowed = ["agent", "text"];
    for key in params.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(DeepLinkReject::UnknownParam { action: "dispatch" });
        }
    }
    let agent = params
        .get("agent")
        .ok_or(DeepLinkReject::MissingParam { name: "agent" })?;
    let text = params
        .get("text")
        .ok_or(DeepLinkReject::MissingParam { name: "text" })?;
    if agent.is_empty() {
        return Err(DeepLinkReject::EmptyValue { name: "agent" });
    }
    if text.is_empty() {
        return Err(DeepLinkReject::EmptyValue { name: "text" });
    }
    if contains_control_chars(agent) || contains_control_chars(text) {
        return Err(DeepLinkReject::ControlCharacter);
    }
    if text.chars().count() > MAX_DISPATCH_TEXT_CHARS {
        return Err(DeepLinkReject::TextTooLong);
    }
    // Reject path / shell injection shape even when percent-encoded length is ok.
    if agent.contains('/') || agent.contains('\\') || agent.contains('\0') {
        return Err(DeepLinkReject::ControlCharacter);
    }
    Ok(DeepLinkAction::Dispatch {
        agent: agent.clone(),
        text: text.clone(),
    })
}

fn parse_query(query: Option<&str>) -> Result<BTreeMap<String, String>, DeepLinkReject> {
    let mut map = BTreeMap::new();
    let Some(q) = query.filter(|s| !s.is_empty()) else {
        return Ok(map);
    };
    if contains_control_chars(q) {
        return Err(DeepLinkReject::ControlCharacter);
    }
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => return Err(DeepLinkReject::Malformed),
        };
        let key = percent_decode(k)?;
        let val = percent_decode(v)?;
        if key.is_empty() {
            return Err(DeepLinkReject::Malformed);
        }
        if map.insert(key, val).is_some() {
            // Duplicate keys are ambiguous — reject.
            return Err(DeepLinkReject::Malformed);
        }
    }
    Ok(map)
}

fn percent_decode(input: &str) -> Result<String, DeepLinkReject> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(DeepLinkReject::Malformed);
                }
                let h = from_hex(bytes[i + 1])?;
                let l = from_hex(bytes[i + 2])?;
                out.push((h << 4) | l);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    let s = String::from_utf8(out).map_err(|_| DeepLinkReject::Malformed)?;
    if contains_control_chars(&s) {
        return Err(DeepLinkReject::ControlCharacter);
    }
    Ok(s)
}

fn from_hex(b: u8) -> Result<u8, DeepLinkReject> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(DeepLinkReject::Malformed),
    }
}

fn contains_control_chars(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Log a rejection **without** including any parameter values.
pub fn log_rejection(reason: &DeepLinkReject) {
    match reason {
        DeepLinkReject::UnknownParam { action } => {
            log::warn!(
                "deep-link rejected: {} (action={action}; param values omitted)",
                reason.as_str()
            );
        }
        DeepLinkReject::MissingParam { name } | DeepLinkReject::EmptyValue { name } => {
            log::warn!(
                "deep-link rejected: {} (param={name}; values omitted)",
                reason.as_str()
            );
        }
        other => {
            log::warn!(
                "deep-link rejected: {} (param values omitted)",
                other.as_str()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_open_and_dispatch() {
        let open = parse_deep_link("agentlounge://open?route=/health").unwrap();
        assert_eq!(
            open,
            DeepLinkAction::Open {
                route: "/health".into()
            }
        );
        let d = parse_deep_link("agentlounge://dispatch?agent=cursor&text=hello%20world").unwrap();
        assert_eq!(
            d,
            DeepLinkAction::Dispatch {
                agent: "cursor".into(),
                text: "hello world".into()
            }
        );
    }

    #[test]
    fn rejects_unknown_scheme_and_actions() {
        assert_eq!(
            parse_deep_link("lounge://open?route=/health").unwrap_err(),
            DeepLinkReject::BadScheme
        );
        assert_eq!(
            parse_deep_link("agentlounge://halt?reason=x").unwrap_err(),
            DeepLinkReject::UnknownAction
        );
        assert_eq!(
            parse_deep_link("agentlounge://resume").unwrap_err(),
            DeepLinkReject::UnknownAction
        );
        assert_eq!(
            parse_deep_link("agentlounge://shell?cmd=ls").unwrap_err(),
            DeepLinkReject::UnknownAction
        );
        assert_eq!(
            parse_deep_link("agentlounge://exec?path=/bin/sh").unwrap_err(),
            DeepLinkReject::UnknownAction
        );
    }

    #[test]
    fn rejects_unknown_params_and_injection() {
        assert_eq!(
            parse_deep_link("agentlounge://open?route=/health&extra=1").unwrap_err(),
            DeepLinkReject::UnknownParam { action: "open" }
        );
        assert_eq!(
            parse_deep_link("agentlounge://dispatch?agent=cursor&text=hi&task=rm%20-rf%20/")
                .unwrap_err(),
            DeepLinkReject::UnknownParam { action: "dispatch" }
        );
        // Classic injection as the sole text is still just text — but unknown `task=` is rejected.
        assert!(parse_deep_link("agentlounge://dispatch?agent=cursor&text=rm%20-rf%20/").is_ok());
    }

    #[test]
    fn rejects_invalid_route_and_overlength() {
        assert_eq!(
            parse_deep_link("agentlounge://open?route=/etc/passwd").unwrap_err(),
            DeepLinkReject::InvalidRoute
        );
        assert_eq!(
            parse_deep_link("agentlounge://open?route=..%2Fvault").unwrap_err(),
            DeepLinkReject::InvalidRoute
        );
        let long = "a".repeat(MAX_DISPATCH_TEXT_CHARS + 1);
        let url = format!("agentlounge://dispatch?agent=cursor&text={long}");
        assert_eq!(
            parse_deep_link(&url).unwrap_err(),
            DeepLinkReject::TextTooLong
        );
    }

    #[test]
    fn rejects_control_characters() {
        assert_eq!(
            parse_deep_link("agentlounge://open?route=/health\n").unwrap_err(),
            DeepLinkReject::ControlCharacter
        );
        assert_eq!(
            parse_deep_link("agentlounge://dispatch?agent=cursor&text=hi%00x").unwrap_err(),
            DeepLinkReject::ControlCharacter
        );
    }

    #[test]
    fn percent_encoding_roundtrip() {
        let d = parse_deep_link(
            "agentlounge://dispatch?agent=grok_bot&text=%C3%A7al%C4%B1%C5%9Fma%20plan%C4%B1",
        )
        .unwrap();
        match d {
            DeepLinkAction::Dispatch { agent, text } => {
                assert_eq!(agent, "grok_bot");
                assert_eq!(text, "çalışma planı");
            }
            _ => panic!("expected dispatch"),
        }
    }

    #[test]
    fn max_length_boundary_ok() {
        let text = "b".repeat(MAX_DISPATCH_TEXT_CHARS);
        let url = format!("agentlounge://dispatch?agent=cursor&text={text}");
        assert!(parse_deep_link(&url).is_ok());
    }

    #[test]
    fn reject_reason_labels_omit_values() {
        // Ensure as_str never embeds user-supplied path/command fragments.
        for r in [
            DeepLinkReject::BadScheme,
            DeepLinkReject::UnknownAction,
            DeepLinkReject::UnknownParam { action: "open" },
            DeepLinkReject::MissingParam { name: "route" },
            DeepLinkReject::InvalidRoute,
            DeepLinkReject::TextTooLong,
            DeepLinkReject::ControlCharacter,
            DeepLinkReject::EmptyValue { name: "text" },
            DeepLinkReject::Malformed,
        ] {
            let label = r.as_str();
            assert!(!label.contains("rm -rf"), "{label}");
            assert!(!label.contains("/etc"), "{label}");
            assert!(!label.contains('\\'), "{label}");
        }
    }
}
