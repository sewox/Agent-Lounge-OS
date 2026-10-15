//! Health-1a dead-symbol filters (H2).
//!
//! Excludes docs/build/generated/.gitignore paths. Marks entry-point-like symbols
//! (tauri commands, trait impls, Next.js exports, tests, JSX components) as not-dead.
//! Surviving findings are split into Kritik / Potansiyel with a rule id.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::models::DeadSymbol;

/// Severity for confirmed findings.
pub const SEVERITY_KRITIK: &str = "Kritik";
pub const SEVERITY_POTANSIYEL: &str = "Potansiyel";

/// Finding rules (attached to surviving symbols).
pub const RULE_BROKEN_MISSING_TARGET: &str = "broken-missing-target";
pub const RULE_UNUSED_NO_INBOUND: &str = "unused-no-inbound";

/// Exclusion rules (why a candidate was dropped).
pub const RULE_FILTER_DOCS: &str = "filter-docs";
pub const RULE_FILTER_BUILD: &str = "filter-build";
pub const RULE_FILTER_GENERATED: &str = "filter-generated";
pub const RULE_FILTER_GITIGNORE: &str = "filter-gitignore";
pub const RULE_FILTER_TAURI_COMMAND: &str = "filter-tauri-command";
pub const RULE_FILTER_TRAIT_IMPL: &str = "filter-trait-impl";
pub const RULE_FILTER_NEXT_EXPORT: &str = "filter-next-export";
pub const RULE_FILTER_TEST: &str = "filter-test";
pub const RULE_FILTER_JSX_COMPONENT: &str = "filter-jsx-component";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterDecision {
    /// Keep as a finding with severity + rule.
    Keep {
        severity: &'static str,
        rule: &'static str,
    },
    /// Drop — not a real dead symbol.
    Exclude { rule: &'static str },
}

#[derive(Debug, Clone, Default)]
pub struct HealthFilterContext {
    pub repo_root: PathBuf,
    gitignore_globs: Vec<String>,
}

impl HealthFilterContext {
    pub fn for_repo(repo_root: impl Into<PathBuf>) -> Self {
        let repo_root = repo_root.into();
        let gitignore_globs = load_gitignore_globs(&repo_root);
        Self {
            repo_root,
            gitignore_globs,
        }
    }

    pub fn decide(&self, symbol: &DeadSymbol) -> FilterDecision {
        if let Some(rule) = self.path_exclusion(symbol.file.as_deref()) {
            return FilterDecision::Exclude { rule };
        }
        if let Some(rule) = entrypoint_exclusion(symbol, &self.repo_root) {
            return FilterDecision::Exclude { rule };
        }
        if symbol.kind == "broken" {
            FilterDecision::Keep {
                severity: SEVERITY_KRITIK,
                rule: RULE_BROKEN_MISSING_TARGET,
            }
        } else {
            FilterDecision::Keep {
                severity: SEVERITY_POTANSIYEL,
                rule: RULE_UNUSED_NO_INBOUND,
            }
        }
    }

    fn path_exclusion(&self, file: Option<&str>) -> Option<&'static str> {
        let file = file?.trim();
        if file.is_empty() {
            return None;
        }
        let normalized = normalize_rel(file);
        if is_docs_path(&normalized) {
            return Some(RULE_FILTER_DOCS);
        }
        if is_build_path(&normalized) {
            return Some(RULE_FILTER_BUILD);
        }
        if is_generated_path(&normalized) {
            return Some(RULE_FILTER_GENERATED);
        }
        if matches_gitignore(&normalized, &self.gitignore_globs) {
            return Some(RULE_FILTER_GITIGNORE);
        }
        None
    }
}

/// Apply filters; attach severity/rule; split into Kritik / Potansiyel.
pub fn split_findings(
    ctx: &HealthFilterContext,
    candidates: Vec<DeadSymbol>,
) -> (Vec<DeadSymbol>, Vec<DeadSymbol>) {
    let mut kritik = Vec::new();
    let mut potansiyel = Vec::new();
    for mut symbol in candidates {
        match ctx.decide(&symbol) {
            FilterDecision::Exclude { .. } => continue,
            FilterDecision::Keep { severity, rule } => {
                symbol.severity = Some(severity.to_string());
                symbol.rule = Some(rule.to_string());
                if severity == SEVERITY_KRITIK {
                    kritik.push(symbol);
                } else {
                    potansiyel.push(symbol);
                }
            }
        }
    }
    (kritik, potansiyel)
}

fn normalize_rel(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn path_segments(path: &str) -> Vec<String> {
    Path::new(path)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().replace('\\', "/")),
            _ => None,
        })
        .collect()
}

fn is_docs_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let segs = path_segments(&lower);
    segs.iter()
        .any(|s| s == "docs" || s == "doc" || s == "documentation")
        || lower.contains("/docs/")
        || lower.starts_with("docs/")
}

fn is_build_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let segs = path_segments(&lower);
    const BUILD_DIRS: &[&str] = &[
        "target",
        "dist",
        "build",
        "out",
        ".next",
        "node_modules",
        "coverage",
        ".turbo",
        "vendor",
        "__pycache__",
        ".vercel",
        "storybook-static",
    ];
    segs.iter().any(|s| BUILD_DIRS.contains(&s.as_str()))
}

fn is_generated_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = Path::new(&lower)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    name.contains(".generated.")
        || name.ends_with(".generated.rs")
        || name.ends_with(".generated.ts")
        || name.ends_with(".generated.js")
        || name.ends_with("_pb.rs")
        || name.ends_with("_pb.js")
        || name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name == "routetree.gen.ts"
        || name.ends_with(".gen.ts")
        || name.ends_with(".gen.js")
        || lower.contains("/generated/")
        || lower.contains("/.generated/")
}

fn load_gitignore_globs(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(".gitignore");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_start_matches('/').replace('\\', "/"))
        .collect()
}

fn matches_gitignore(path: &str, globs: &[String]) -> bool {
    let path = path.trim_start_matches('/');
    for raw in globs {
        let pattern = raw.trim_start_matches('/');
        if pattern.is_empty() {
            continue;
        }
        if pattern.ends_with('/') {
            let dir = pattern.trim_end_matches('/');
            if path == dir || path.starts_with(&format!("{dir}/")) {
                return true;
            }
            continue;
        }
        if pattern.contains('*') {
            if simple_glob_match(pattern, path) {
                return true;
            }
            // Also match basename-only globs (e.g. `*.log`).
            if let Some(base) = Path::new(path).file_name().and_then(|s| s.to_str()) {
                if simple_glob_match(pattern, base) {
                    return true;
                }
            }
            continue;
        }
        if path == pattern || path.starts_with(&format!("{pattern}/")) {
            return true;
        }
    }
    false
}

/// Minimal `*` glob (no `**` recursion beyond path suffix).
fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    if !text.starts_with(parts[0]) {
        return false;
    }
    let mut rest = &text[parts[0].len()..];
    for (i, part) in parts.iter().enumerate().skip(1) {
        if part.is_empty() {
            if i == parts.len() - 1 {
                return true;
            }
            continue;
        }
        match rest.find(part) {
            Some(idx) => {
                rest = &rest[idx + part.len()..];
            }
            None => return false,
        }
    }
    rest.is_empty() || pattern.ends_with('*')
}

fn entrypoint_exclusion(symbol: &DeadSymbol, repo_root: &Path) -> Option<&'static str> {
    let name = symbol.name.as_str();
    let file = symbol.file.as_deref().unwrap_or("");
    let normalized = normalize_rel(file);
    let lower = normalized.to_ascii_lowercase();

    if is_test_symbol(name, &lower) {
        return Some(RULE_FILTER_TEST);
    }
    if is_next_export(name, &lower) {
        return Some(RULE_FILTER_NEXT_EXPORT);
    }
    if is_jsx_component(name, &lower) {
        return Some(RULE_FILTER_JSX_COMPONENT);
    }

    // Source-backed checks (positive when file exists).
    if let Some(src) = read_symbol_window(repo_root, &normalized, symbol.line) {
        if looks_like_tauri_command(&src, name) {
            return Some(RULE_FILTER_TAURI_COMMAND);
        }
        if looks_like_trait_impl(&src, name) {
            return Some(RULE_FILTER_TRAIT_IMPL);
        }
    } else {
        // Fallback heuristics without source (detail / path cues).
        if let Some(detail) = symbol.detail.as_deref() {
            let d = detail.to_ascii_lowercase();
            if d.contains("tauri::command") || d.contains("#[tauri::command]") {
                return Some(RULE_FILTER_TAURI_COMMAND);
            }
            if d.contains("trait impl") || (d.contains("impl ") && d.contains(" for ")) {
                return Some(RULE_FILTER_TRAIT_IMPL);
            }
        }
    }

    None
}

fn is_test_symbol(name: &str, file_lower: &str) -> bool {
    let base = name.rsplit("::").next().unwrap_or(name);
    if base.starts_with("test_") || base.ends_with("_test") || base.starts_with("it_") {
        return true;
    }
    if file_lower.contains("/__tests__/")
        || file_lower.contains("/tests/")
        || file_lower.ends_with("_test.rs")
        || file_lower.ends_with(".test.ts")
        || file_lower.ends_with(".test.tsx")
        || file_lower.ends_with(".test.js")
        || file_lower.ends_with(".spec.ts")
        || file_lower.ends_with(".spec.tsx")
        || file_lower.ends_with(".spec.js")
    {
        return true;
    }
    false
}

fn is_next_export(name: &str, file_lower: &str) -> bool {
    let base = name.rsplit("::").next().unwrap_or(name);
    let base = base.rsplit('/').next().unwrap_or(base);
    const NEXT_FILES: &[&str] = &[
        "page.tsx",
        "page.ts",
        "page.jsx",
        "page.js",
        "layout.tsx",
        "layout.ts",
        "layout.jsx",
        "layout.js",
        "route.ts",
        "route.js",
        "loading.tsx",
        "loading.ts",
        "error.tsx",
        "error.ts",
        "template.tsx",
        "default.tsx",
        "not-found.tsx",
        "middleware.ts",
    ];
    let file_name = Path::new(file_lower)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if NEXT_FILES.contains(&file_name)
        && (base == "default"
            || base == "GET"
            || base == "POST"
            || base == "PUT"
            || base == "PATCH"
            || base == "DELETE"
            || base == "HEAD"
            || base == "OPTIONS"
            || base == "generateMetadata"
            || base == "generateStaticParams"
            || base == "metadata"
            || is_pascal_case(base))
    {
        return true;
    }
    // Named Next.js convention exports even outside page files.
    matches!(
        base,
        "generateMetadata" | "generateStaticParams" | "generateViewport"
    )
}

fn is_jsx_component(name: &str, file_lower: &str) -> bool {
    if !(file_lower.ends_with(".tsx") || file_lower.ends_with(".jsx")) {
        return false;
    }
    let base = name.rsplit("::").next().unwrap_or(name);
    let base = base.rsplit('.').next().unwrap_or(base);
    is_pascal_case(base)
}

fn is_pascal_case(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => name.chars().all(|c| c.is_ascii_alphanumeric()),
        _ => false,
    }
}

fn read_symbol_window(repo_root: &Path, rel: &str, line: Option<i64>) -> Option<String> {
    let path = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        repo_root.join(rel)
    };
    let text = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let idx = line.unwrap_or(1).max(1) as usize;
    let start = idx.saturating_sub(12);
    let end = (idx + 4).min(lines.len());
    Some(lines[start..end].join("\n"))
}

fn looks_like_tauri_command(window: &str, name: &str) -> bool {
    let base = name.rsplit("::").next().unwrap_or(name);
    if !window.contains("#[tauri::command]") && !window.contains("#[tauri::command(") {
        return false;
    }
    // Attribute should precede a fn with this name in the window.
    window.contains(&format!("fn {base}"))
        || window.contains(&format!("async fn {base}"))
        || window.contains(&format!("pub fn {base}"))
        || window.contains(&format!("pub async fn {base}"))
}

fn looks_like_trait_impl(window: &str, name: &str) -> bool {
    let base = name.rsplit("::").next().unwrap_or(name);
    let has_impl_for = window.lines().any(|line| {
        let t = line.trim();
        t.starts_with("impl ") && t.contains(" for ")
    });
    if !has_impl_for {
        return false;
    }
    window.contains(&format!("fn {base}"))
        || window.contains(&format!("async fn {base}"))
        || window.contains(&format!("pub fn {base}"))
}

/// Mask absolute home prefixes as `~/...` for logs / exported paths.
pub fn mask_home_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let home = dirs_home();
    if let Some(home) = home {
        let home_s = home.replace('\\', "/");
        if normalized == home_s || normalized.starts_with(&(home_s.clone() + "/")) {
            let rest = normalized[home_s.len()..].trim_start_matches('/');
            return if rest.is_empty() {
                "~/".into()
            } else {
                format!("~/{rest}")
            };
        }
    }
    // Also mask common env forms embedded mid-string.
    if let Ok(home) = std::env::var("HOME") {
        let home = home.replace('\\', "/");
        if !home.is_empty() && normalized.contains(&home) {
            return normalized.replace(&home, "~");
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let home = home.replace('\\', "/");
        if !home.is_empty() && normalized.contains(&home) {
            return normalized.replace(&home, "~");
        }
    }
    normalized
}

fn dirs_home() -> Option<String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|v| v.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sym(name: &str, kind: &str, file: &str) -> DeadSymbol {
        DeadSymbol {
            name: name.into(),
            kind: kind.into(),
            file: Some(file.into()),
            line: Some(10),
            ..DeadSymbol::default()
        }
    }

    #[test]
    fn t2_docs_build_generated_gitignore_positive_and_negative() {
        let dir = tempfile_dir("filter-path");
        fs::write(
            dir.join(".gitignore"),
            "secrets/\n*.log\n# comment\nignored_out/\n",
        )
        .unwrap();
        let ctx = HealthFilterContext::for_repo(&dir);

        // Positive exclusions
        assert!(matches!(
            ctx.decide(&sym("a", "unused", "docs/guide.md")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_DOCS
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("b", "unused", "target/debug/foo.rs")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_BUILD
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("c", "unused", "src/api.generated.ts")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_GENERATED
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("d", "unused", "secrets/token.txt")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_GITIGNORE
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("e", "unused", "app.log")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_GITIGNORE
            }
        ));

        // Negative: normal source stays
        let keep = ctx.decide(&sym("orphan", "unused", "src/lib/orphan.rs"));
        assert!(matches!(
            keep,
            FilterDecision::Keep {
                severity: SEVERITY_POTANSIYEL,
                rule: RULE_UNUSED_NO_INBOUND
            }
        ));
        let keep_b = ctx.decide(&sym("ghost", "broken", "src/lib/call.rs"));
        assert!(matches!(
            keep_b,
            FilterDecision::Keep {
                severity: SEVERITY_KRITIK,
                rule: RULE_BROKEN_MISSING_TARGET
            }
        ));
    }

    #[test]
    fn t2_tauri_command_trait_impl_next_test_jsx() {
        let dir = tempfile_dir("filter-entry");
        // tauri command — positive
        let tauri_file = dir.join("src-tauri/src/cmds.rs");
        fs::create_dir_all(tauri_file.parent().unwrap()).unwrap();
        fs::write(
            &tauri_file,
            "#[tauri::command]\npub async fn index_workspace(path: String) -> Result<(), String> { Ok(()) }\n",
        )
        .unwrap();
        // trait impl — positive
        let trait_file = dir.join("src/trait_demo.rs");
        fs::create_dir_all(trait_file.parent().unwrap()).unwrap();
        fs::write(
            &trait_file,
            "impl Display for Foo {\n    fn fmt(&self, f: &mut Formatter) -> Result { Ok(()) }\n}\n",
        )
        .unwrap();
        let ctx = HealthFilterContext::for_repo(&dir);

        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "index_workspace".into(),
                kind: "unused".into(),
                file: Some("src-tauri/src/cmds.rs".into()),
                line: Some(2),
                ..DeadSymbol::default()
            }),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TAURI_COMMAND
            }
        ));
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "fmt".into(),
                kind: "unused".into(),
                file: Some("src/trait_demo.rs".into()),
                line: Some(2),
                ..DeadSymbol::default()
            }),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TRAIT_IMPL
            }
        ));

        // Next.js export — positive
        assert!(matches!(
            ctx.decide(&sym("default", "unused", "src/app/page.tsx")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_NEXT_EXPORT
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("generateMetadata", "unused", "src/app/layout.tsx")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_NEXT_EXPORT
            }
        ));

        // Test — positive
        assert!(matches!(
            ctx.decide(&sym("test_parse", "unused", "src/lib.rs")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TEST
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("helper", "unused", "src/foo.spec.ts")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TEST
            }
        ));

        // JSX component — positive
        assert!(matches!(
            ctx.decide(&sym(
                "DeadSymbolsPanel",
                "unused",
                "src/components/panel.tsx"
            )),
            FilterDecision::Exclude {
                rule: RULE_FILTER_JSX_COMPONENT
            }
        ));

        // Negatives: ordinary unused Rust fn / camelCase js helper stay
        assert!(matches!(
            ctx.decide(&sym("compute_score", "unused", "src/score.rs")),
            FilterDecision::Keep {
                severity: SEVERITY_POTANSIYEL,
                ..
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("formatDate", "unused", "src/utils/date.ts")),
            FilterDecision::Keep {
                severity: SEVERITY_POTANSIYEL,
                ..
            }
        ));
        // Non-command near tauri file but different name
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "helper_only".into(),
                kind: "unused".into(),
                file: Some("src-tauri/src/cmds.rs".into()),
                line: Some(2),
                ..DeadSymbol::default()
            }),
            FilterDecision::Keep { .. }
        ));
    }

    #[test]
    fn mask_home_path_replaces_prefix() {
        let home = dirs_home().expect("home");
        let full = format!("{home}/projects/Agent-Lounge-OS/src/lib.rs");
        let masked = mask_home_path(&full);
        assert!(masked.starts_with("~/"), "{masked}");
        assert!(!masked.contains(&home), "{masked}");
    }

    fn tempfile_dir(label: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "al-health-filter-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut marker = fs::File::create(dir.join(".al-test")).unwrap();
        let _ = writeln!(marker, "ok");
        dir
    }
}
