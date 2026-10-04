//! MCP oturum kimliği doğrulama ve sınırlar.

/// `Mcp-Session-Id` — path-safe, kısa.
pub const SESSION_ID_MAX: usize = 64;
pub const SESSION_ID_PATTERN: &str = r"^[A-Za-z0-9._-]{1,64}$";

/// Hub oturum map üst sınırı.
pub const MAX_MCP_SESSIONS: usize = 256;
/// Uçuştaki tools/call üst sınırı (tüm oturumlar).
pub const MAX_IN_FLIGHT: usize = 128;

/// Geçerli istemci oturum id'si mi?
pub fn is_valid_session_id(raw: &str) -> bool {
    let s = raw.trim();
    if s.is_empty() || s.len() > SESSION_ID_MAX {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Geçersiz/eksik → yeni sunucu UUID.
pub fn normalize_or_mint_session_id(raw: Option<&str>) -> String {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if is_valid_session_id(s) => s.to_string(),
        _ => uuid::Uuid::new_v4().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_safe_ids() {
        assert!(is_valid_session_id("abc-123_X.y"));
        assert!(is_valid_session_id(&"a".repeat(64)));
    }

    #[test]
    fn rejects_unsafe() {
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("../etc"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id(&"x".repeat(65)));
        assert!(!is_valid_session_id("has space"));
    }

    #[test]
    fn mints_when_invalid() {
        let id = normalize_or_mint_session_id(Some("../x"));
        assert!(is_valid_session_id(&id));
        assert_ne!(id, "../x");
    }
}
