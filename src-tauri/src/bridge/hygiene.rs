//! Experience content hygiene — mask transient/sensitive fragments before persist
//! and before/after LLM title generation.
//!
//! Masking (not rejection): IP:port (private and public), device serials, tokens/keys,
//! absolute home paths. Masked writes are flagged `reviewed=0` and return a warning
//! to the agent. Existing rows are NOT redacted without explicit approval (OPEN).

use std::sync::OnceLock;

use regex::Regex;

fn re_ip_port() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Any IPv4:port (private + public). Port capture kept for `<device-ip>:<port>`.
    RE.get_or_init(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}:(\d{1,5})\b").expect("ip:port regex"))
}

fn re_serial_labeled() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b((?:serial(?:[\s_=-]*number)?|device[\s_-]*id|android[\s_-]*id)[\s:=#-]*)([A-Z0-9]{8,})\b",
        )
        .expect("serial label regex")
    })
}

fn re_serial_bare() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Case-insensitive 16-hex device serials.
    RE.get_or_init(|| Regex::new(r"(?i)\b([A-F0-9]{16})\b").expect("bare serial regex"))
}

fn re_token() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Avoid r#"..."# — embedded quotes terminate those delimiters.
        Regex::new(concat!(
            r"(?i)\b(?:",
            r"bearer\s+[A-Za-z0-9\-._~+/]+=*|",
            r"sk-[A-Za-z0-9]{16,}|",
            r"ghp_[A-Za-z0-9]{20,}|",
            r"xox[baprs]-[A-Za-z0-9-]{10,}|",
            r"SU[A-Za-z0-9]{48,}|",
            r"(?:nats[_-]?seed|nkey[_-]?seed|seed)\s*[:=]\s*[A-Za-z0-9]{32,}|",
            r"(?:api[_-]?key|access[_-]?token|secret|auth[_-]?token)\s*[:=]\s*",
            r"\S{12,}",
            r")",
        ))
        .expect("token regex")
    })
}

fn re_home_path() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // /Users/<u>, /home/<u>, /root(/…), /var/home/<u>, C:\Users\<u>, C:/Users/<u>
        Regex::new(concat!(
            r"(?i)(?:",
            r"/Users/[^/\s]+",
            r"|/var/home/[^/\s]+",
            r"|/home/[^/\s]+",
            r"|/root(?:/[^/\s]+)?",
            r"|(?:[A-Z]:)?[/\\]Users[/\\][^/\\\s]+",
            r")",
        ))
        .expect("home path regex")
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HygieneResult {
    pub text: String,
    pub masked: bool,
    pub warnings: Vec<String>,
}

/// Apply all hygiene masks. Idempotent on already-masked placeholders.
pub fn scrub(input: &str) -> HygieneResult {
    let mut text = input.to_string();
    let mut warnings = Vec::new();
    let mut masked = false;

    let next = re_ip_port()
        .replace_all(&text, "<device-ip>:$1")
        .into_owned();
    if next != text {
        masked = true;
        warnings.push("masked IP:port as <device-ip>:<port>".into());
        text = next;
    }

    let next = re_token()
        .replace_all(&text, "<redacted-token>")
        .into_owned();
    if next != text {
        masked = true;
        warnings.push("masked token/key material".into());
        text = next;
    }

    let next = re_serial_labeled()
        .replace_all(&text, "${1}<device-serial>")
        .into_owned();
    let next = re_serial_bare()
        .replace_all(&next, "<device-serial>")
        .into_owned();
    if next != text {
        masked = true;
        warnings.push("masked device serial".into());
        text = next;
    }

    let next = re_home_path()
        .replace_all(&text, |_caps: &regex::Captures| "<home>")
        .into_owned();
    if next != text {
        masked = true;
        warnings.push("masked absolute home path".into());
        text = next;
    }

    HygieneResult {
        text,
        masked,
        warnings,
    }
}

/// Placeholders written by [`scrub`] — used to detect hygiene-flagged rows.
pub const HYGIENE_PLACEHOLDERS: &[&str] = &[
    "<device-ip>",
    "<redacted-token>",
    "<device-serial>",
    "<home>",
];

/// True when text still contains a hygiene mask placeholder.
pub fn text_has_hygiene_mask(text: &str) -> bool {
    HYGIENE_PLACEHOLDERS.iter().any(|p| text.contains(p))
}

/// SQL predicate (no leading AND) matching hygiene-masked experience text columns.
/// `prefix` is an optional table alias (`""` or `"e"`); never wrap expressions.
pub fn hygiene_masked_sql(prefix: &str) -> String {
    let p = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}.")
    };
    let cols = [
        format!("{p}topic"),
        format!("{p}solution_summary"),
        format!("{p}adr_record"),
        format!("COALESCE({p}original_content,'')"),
        format!("COALESCE({p}title,'')"),
    ];
    let mut parts = Vec::new();
    for expr in &cols {
        for ph in HYGIENE_PLACEHOLDERS {
            parts.push(format!("{expr} LIKE '%{ph}%'"));
        }
    }
    format!("({})", parts.join(" OR "))
}

/// Tag limits (documented constants).
pub const MAX_TAGS: usize = 16;
pub const MAX_TAG_LEN: usize = 32;

/// Normalize agent-supplied tags: lowercase, trim, dedupe, drop source tags,
/// enforce count/length limits. Returns `(tags, source_parts)`.
///
/// `source_parts` collects `mcp`/`external` moved out of tags. Join with
/// [`format_source_metadata`] for the DB/API `source` field.
pub fn normalize_tags(raw: &[String]) -> (Vec<String>, Vec<String>) {
    let mut source = Vec::new();
    let mut tags = Vec::new();
    for tag in raw {
        let t = tag.trim().to_ascii_lowercase();
        if t.is_empty() {
            continue;
        }
        if t == "mcp" || t == "external" {
            if !source.iter().any(|s| s == &t) {
                source.push(t);
            }
            continue;
        }
        let clipped: String = t.chars().take(MAX_TAG_LEN).collect();
        if !tags.iter().any(|s| s == &clipped) {
            tags.push(clipped);
        }
        if tags.len() >= MAX_TAGS {
            break;
        }
    }
    (tags, source)
}

/// Format provenance tokens for the experience `source` column.
///
/// **Format:** comma-separated lowercase tokens, no spaces (e.g. `mcp,external`).
/// Callers should put `mcp` first when present. Empty input → empty string.
pub fn format_source_metadata(parts: &[String]) -> String {
    parts
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Format optional structured ADR fields into a single record body.
pub fn format_adr(
    context: Option<&str>,
    decision: Option<&str>,
    consequences: Option<&str>,
    alternatives: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(c) = context.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("Context: {c}"));
    }
    if let Some(d) = decision.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("Decision: {d}"));
    }
    if let Some(c) = consequences.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("Consequences: {c}"));
    }
    if let Some(a) = alternatives.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("Alternatives: {a}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Fallback title: truncate to ≤max_chars on a **word boundary**.
/// Never cuts mid-word when at least one full word fits; otherwise hard-caps.
pub fn fallback_title(content: &str, max_chars: usize) -> String {
    let trimmed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return "Untitled".into();
    }
    if trimmed.chars().count() <= max_chars {
        return trimmed;
    }
    let mut out = String::new();
    for word in trimmed.split_whitespace() {
        let next_len = if out.is_empty() {
            word.chars().count()
        } else {
            out.chars().count() + 1 + word.chars().count()
        };
        if next_len > max_chars {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        // Single oversize token: hard cap (no word boundary available).
        trimmed.chars().take(max_chars).collect()
    } else {
        out
    }
}

/// Strip quotes/markdown/prefixes from LLM title output and trim to max_chars.
pub fn clean_title_output(raw: &str, max_chars: usize) -> Option<String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return None;
    }
    if let Some(body) = s.strip_prefix("```").and_then(|b| b.strip_suffix("```")) {
        s = body.trim().to_string();
    }
    s = s
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string();
    for prefix in [
        "Title:",
        "title:",
        "Başlık:",
        "Here is a title:",
        "Here's a title:",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
        }
    }
    let lower = s.to_ascii_lowercase();
    if s.chars().count() < 3
        || lower == "n/a"
        || lower == "none"
        || lower == "null"
        || lower == "untitled"
        || lower.contains("as an ai")
        || lower.contains("i cannot")
        || lower.contains("i can't")
    {
        return None;
    }
    if let Some(line) = s.lines().next() {
        s = line.trim().to_string();
    }
    if s.is_empty() {
        return None;
    }
    Some(fallback_title(&s, max_chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_private_and_public_ip_port() {
        let r = scrub("connect 192.168.1.10:5555 and 8.8.8.8:53");
        assert!(r.masked);
        assert!(r.text.contains("<device-ip>:5555"));
        assert!(r.text.contains("<device-ip>:53"));
        assert!(!r.text.contains("192.168.1.10"));
        assert!(!r.text.contains("8.8.8.8"));
    }

    #[test]
    fn masks_token_variants_and_home() {
        let r = scrub(
            "api_key=sk-abcdefghijklmnopqrstuvwxyz0123 ghp_abcdefghijklmnopqrstuv path=/Users/alice/proj",
        );
        assert!(r.masked);
        assert!(r.text.contains("<redacted-token>"));
        assert!(r.text.contains("<home>"));
        assert!(!r.text.contains("/Users/alice"));
        assert!(!r.text.contains("ghp_"));
    }

    #[test]
    fn masks_extended_home_roots() {
        let samples = [
            "/Users/ci/projects/secret",
            "/home/sercan/dev",
            "/root",
            "/root/.config/lounge",
            "/var/home/fedora/work",
            r"C:\Users\sercan\dev",
            "C:/Users/sercan/dev",
        ];
        for sample in samples {
            let r = scrub(sample);
            assert!(r.masked, "expected mask for {sample}");
            assert!(r.text.contains("<home>"), "{sample} → {}", r.text);
            assert!(!r.text.contains("/Users/"), "{sample}");
            assert!(!r.text.contains("/home/"), "{sample}");
            assert!(!r.text.contains("/var/home/"), "{sample}");
            assert!(!r.text.contains(r"C:\Users"), "{sample}");
            assert!(!r.text.contains("C:/Users"), "{sample}");
            // Bare /root → <home>; /root/foo → <home>/foo already covered by replace.
            if sample == "/root" {
                assert_eq!(r.text, "<home>");
            }
        }
    }

    #[test]
    fn masks_slack_and_nats_seed() {
        let r = scrub(
            "xoxb-1234567890-abcdefghij seed=SUAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        assert!(r.masked);
        assert!(r.text.contains("<redacted-token>"));
        assert!(!r.text.contains("xoxb-"));
        assert!(!r.text.contains("SUAAAA"));
    }

    #[test]
    fn masks_serial_label_and_bare_case_insensitive() {
        let r = scrub("device id: ABCDEF1234567890 and bare abcdef1234567890");
        assert!(r.masked);
        assert_eq!(r.text.matches("<device-serial>").count(), 2);
    }

    #[test]
    fn normalize_moves_source_tags() {
        let (tags, source) = normalize_tags(&[
            "MCP".into(),
            "External".into(),
            "Redis".into(),
            "redis".into(),
            "NATS".into(),
        ]);
        assert_eq!(tags, vec!["redis", "nats"]);
        assert_eq!(source, vec!["mcp", "external"]);
        assert_eq!(format_source_metadata(&source), "mcp,external");
    }

    #[test]
    fn fallback_truncates_on_word_boundary() {
        let long = "word ".repeat(40);
        let t = fallback_title(&long, 80);
        assert!(t.chars().count() <= 80);
        assert!(!t.ends_with(' '));
        // Must end on a full word (no trailing partial token).
        assert!(t.ends_with("word"));
        assert!(t.split_whitespace().all(|w| w == "word"));
    }

    #[test]
    fn clean_title_rejects_garbage() {
        assert!(clean_title_output("n/a", 80).is_none());
        assert!(clean_title_output("As an AI I cannot help", 80).is_none());
        assert_eq!(
            clean_title_output("\"Title: Fix NATS bind\"", 80).as_deref(),
            Some("Fix NATS bind")
        );
    }

    #[test]
    fn format_adr_none_when_empty() {
        assert!(format_adr(None, None, None, None).is_none());
        let adr = format_adr(Some("ctx"), Some("dec"), None, None).unwrap();
        assert!(adr.contains("Context: ctx"));
        assert!(adr.contains("Decision: dec"));
    }
}
