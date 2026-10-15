//! Health-1a dead-symbol filters (H2).
//!
//! Path exclusions (docs/build/generated/gitignore/examples/tests/benches) and
//! entry-point exclusions (tauri command, trait impl, Next.js, tests, JSX).
//! Surviving findings split into Kritik / Potansiyel with a rule id.
//!
//! Absolute paths are used for detection; callers mask home prefixes only when
//! emitting results to the user.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::models::DeadSymbol;

pub const SEVERITY_KRITIK: &str = "Kritik";
pub const SEVERITY_POTANSIYEL: &str = "Potansiyel";

pub const RULE_BROKEN_MISSING_TARGET: &str = "broken-missing-target";
pub const RULE_UNUSED_NO_INBOUND: &str = "unused-no-inbound";

pub const RULE_FILTER_DOCS: &str = "filter-docs";
pub const RULE_FILTER_BUILD: &str = "filter-build";
pub const RULE_FILTER_GENERATED: &str = "filter-generated";
pub const RULE_FILTER_GITIGNORE: &str = "filter-gitignore";
pub const RULE_FILTER_EXAMPLES_TESTS_BENCHES: &str = "filter-examples-tests-benches";
pub const RULE_FILTER_TAURI_COMMAND: &str = "filter-tauri-command";
pub const RULE_FILTER_TRAIT_IMPL: &str = "filter-trait-impl";
pub const RULE_FILTER_NEXT_EXPORT: &str = "filter-next-export";
pub const RULE_FILTER_TEST: &str = "filter-test";
pub const RULE_FILTER_JSX_COMPONENT: &str = "filter-jsx-component";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterDecision {
    Keep {
        severity: &'static str,
        rule: &'static str,
    },
    Exclude {
        rule: &'static str,
    },
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
        let rel = relativize_to_repo(&self.repo_root, file);
        let normalized = normalize_slashes(&rel);
        if is_docs_path(&normalized) {
            return Some(RULE_FILTER_DOCS);
        }
        if is_build_path(&normalized) {
            return Some(RULE_FILTER_BUILD);
        }
        if is_generated_path(&normalized) || file_has_generated_marker(&self.repo_root, file) {
            return Some(RULE_FILTER_GENERATED);
        }
        if is_examples_tests_benches_path(&normalized) {
            return Some(RULE_FILTER_EXAMPLES_TESTS_BENCHES);
        }
        if matches_gitignore(&normalized, &self.gitignore_globs) {
            return Some(RULE_FILTER_GITIGNORE);
        }
        None
    }
}

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

fn relativize_to_repo(repo_root: &Path, file: &str) -> String {
    let path = PathBuf::from(file);
    if path.is_absolute() {
        if let Ok(stripped) = path.strip_prefix(repo_root) {
            return stripped.to_string_lossy().replace('\\', "/");
        }
        // Also try canonicalized roots.
        if let (Ok(root), Ok(abs)) = (repo_root.canonicalize(), path.canonicalize()) {
            if let Ok(stripped) = abs.strip_prefix(root) {
                return stripped.to_string_lossy().replace('\\', "/");
            }
        }
    }
    normalize_slashes(file)
}

fn normalize_slashes(path: &str) -> String {
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

fn is_examples_tests_benches_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let segs = path_segments(&lower);
    // Top-level (or nested) Cargo/JS convention dirs — not `src/.../tests.rs` files.
    segs.iter()
        .any(|s| s == "examples" || s == "benches" || s == "tests")
}

fn is_generated_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = Path::new(&lower)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if name.contains(".generated.")
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
    {
        return true;
    }
    false
}

fn file_has_generated_marker(repo_root: &Path, file: &str) -> bool {
    let path = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        repo_root.join(file)
    };
    let Ok(head) = fs::read_to_string(path) else {
        return false;
    };
    head.chars()
        .take(512)
        .collect::<String>()
        .contains("@generated")
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
            Some(idx) => rest = &rest[idx + part.len()..],
            None => return false,
        }
    }
    rest.is_empty() || pattern.ends_with('*')
}

fn entrypoint_exclusion(symbol: &DeadSymbol, repo_root: &Path) -> Option<&'static str> {
    let name = symbol.name.as_str();
    let file = symbol.file.as_deref().unwrap_or("");
    let rel = relativize_to_repo(repo_root, file);
    let lower = rel.to_ascii_lowercase();

    if is_test_symbol(name, &lower) {
        return Some(RULE_FILTER_TEST);
    }
    if is_next_export(name, &lower) {
        return Some(RULE_FILTER_NEXT_EXPORT);
    }
    if is_jsx_component(name, &lower) {
        return Some(RULE_FILTER_JSX_COMPONENT);
    }

    if let Some(src) = read_source_file(repo_root, file) {
        // `#[test]` without a `test_`/`_test` name prefix — exact attr above the fn.
        if looks_like_rust_test_attr(&src, name, symbol.line) {
            return Some(RULE_FILTER_TEST);
        }
        if looks_like_tauri_command(&src, name, symbol.line) {
            return Some(RULE_FILTER_TAURI_COMMAND);
        }
        if looks_like_trait_impl(&src, name, symbol.line) {
            return Some(RULE_FILTER_TRAIT_IMPL);
        }
    }

    None
}

fn is_test_symbol(name: &str, file_lower: &str) -> bool {
    let base = symbol_base(name);
    if base.starts_with("test_") || base.ends_with("_test") || base.starts_with("it_") {
        return true;
    }
    // File-level test conventions (not the `tests/` directory — that is examples/tests/benches).
    if file_lower.contains("/__tests__/")
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

/// `#[test]` (or `#[tokio::test]`) must sit directly above the fn — covers names
/// without a `test_` prefix.
pub fn looks_like_rust_test_attr(src: &str, name: &str, line: Option<i64>) -> bool {
    let base = symbol_base(name);
    let lines: Vec<&str> = src.lines().collect();
    let Some(fn_idx) = find_exact_fn_line(&lines, base, line) else {
        return false;
    };
    let mut i = fn_idx;
    while i > 0 {
        i -= 1;
        let t = lines[i].trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if t.starts_with("#[") && t.ends_with(']') {
            if t == "#[test]"
                || t.starts_with("#[test(")
                || t == "#[tokio::test]"
                || t.starts_with("#[tokio::test(")
            {
                return true;
            }
            continue;
        }
        break;
    }
    false
}

fn is_next_export(name: &str, file_lower: &str) -> bool {
    let base = symbol_base(name);
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
        && (matches!(
            base,
            "default"
                | "GET"
                | "POST"
                | "PUT"
                | "PATCH"
                | "DELETE"
                | "HEAD"
                | "OPTIONS"
                | "generateMetadata"
                | "generateStaticParams"
                | "metadata"
        ) || is_pascal_case(base))
    {
        return true;
    }
    matches!(
        base,
        "generateMetadata" | "generateStaticParams" | "generateViewport"
    )
}

fn is_jsx_component(name: &str, file_lower: &str) -> bool {
    if !(file_lower.ends_with(".tsx") || file_lower.ends_with(".jsx")) {
        return false;
    }
    is_pascal_case(symbol_base(name))
}

fn is_pascal_case(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => name.chars().all(|c| c.is_ascii_alphanumeric()),
        _ => false,
    }
}

fn symbol_base(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

fn read_source_file(repo_root: &Path, file: &str) -> Option<String> {
    let path = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        repo_root.join(file)
    };
    fs::read_to_string(path).ok()
}

/// `#[tauri::command]` must sit directly above the fn (allowing only blank / doc /
/// other attribute lines). Name match is exact (`fn name(`), not a substring.
pub fn looks_like_tauri_command(src: &str, name: &str, line: Option<i64>) -> bool {
    let base = symbol_base(name);
    let lines: Vec<&str> = src.lines().collect();
    let Some(fn_idx) = find_exact_fn_line(&lines, base, line) else {
        return false;
    };
    // Walk upward: skip blank, `///`/`//!`/`//`, and `#[...]` attrs. Require
    // `#[tauri::command]` (or `#[tauri::command(...)]`) among those attrs, and
    // stop when we hit a non-attr code line.
    let mut saw_tauri = false;
    let mut i = fn_idx;
    while i > 0 {
        i -= 1;
        let t = lines[i].trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if t.starts_with("#[") && t.ends_with(']') {
            if t.starts_with("#[tauri::command]") || t.starts_with("#[tauri::command(") {
                saw_tauri = true;
            }
            continue;
        }
        break;
    }
    saw_tauri
}

/// Trait-impl methods: fn must sit inside an `impl Trait for Type { ... }` block,
/// with an exact name match (not `fn fmt_helper` for `fmt`).
pub fn looks_like_trait_impl(src: &str, name: &str, line: Option<i64>) -> bool {
    let base = symbol_base(name);
    let lines: Vec<&str> = src.lines().collect();
    let Some(fn_idx) = find_exact_fn_line(&lines, base, line) else {
        return false;
    };
    enclosing_trait_impl(&lines, fn_idx).is_some()
}

fn find_exact_fn_line(lines: &[&str], base: &str, line: Option<i64>) -> Option<usize> {
    let candidates: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line_text)| {
            if exact_fn_signature(line_text, base) {
                Some(idx)
            } else {
                None
            }
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    if let Some(lineno) = line {
        let want = (lineno.max(1) as usize).saturating_sub(1);
        // Prefer the declaration at/near the reported line.
        if let Some(&idx) = candidates.iter().find(|&&idx| idx == want) {
            return Some(idx);
        }
        // Allow small drift (±2) from tree-sitter vs source.
        if let Some(&idx) = candidates.iter().find(|&&idx| idx.abs_diff(want) <= 2) {
            return Some(idx);
        }
    }
    // Unique exact match in the file.
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }
    None
}

fn exact_fn_signature(line: &str, base: &str) -> bool {
    let t = line.trim();
    // Strip visibility / async / const / unsafe prefixes.
    let mut rest = t;
    for prefix in [
        "pub(crate) ",
        "pub(super) ",
        "pub ",
        "const ",
        "async ",
        "unsafe ",
    ] {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            rest = stripped.trim_start();
        }
    }
    let Some(after_fn) = rest.strip_prefix("fn ") else {
        return false;
    };
    let after_fn = after_fn.trim_start();
    // Exact name then `(` or `<` (generics) — reject `fn fmt_helper`.
    if let Some(rest) = after_fn.strip_prefix(base) {
        matches!(rest.chars().next(), Some('(') | Some('<') | None)
            || rest.starts_with(" (")
            || rest.starts_with(" <")
    } else {
        false
    }
}

fn enclosing_trait_impl(lines: &[&str], fn_idx: usize) -> Option<usize> {
    for (start, end) in trait_impl_ranges(lines) {
        if fn_idx > start && fn_idx <= end {
            return Some(start);
        }
    }
    None
}

fn trait_impl_ranges(lines: &[&str]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_trait_impl_header(lines[i].trim()) {
            i += 1;
            continue;
        }
        let start = i;
        let mut depth = 0i32;
        let mut started = false;
        let mut j = i;
        while j < lines.len() {
            for ch in lines[j].chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        started = true;
                    }
                    '}' => {
                        depth -= 1;
                        if started && depth == 0 {
                            ranges.push((start, j));
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if started && depth == 0 {
                break;
            }
            j += 1;
        }
        i = j.saturating_add(1).max(i + 1);
    }
    ranges
}

fn is_trait_impl_header(line: &str) -> bool {
    let t = line.trim();
    let rest = t.strip_prefix("pub ").unwrap_or(t);
    rest.starts_with("impl ") && rest.contains(" for ")
}

/// Mask absolute home prefixes as `~/...` for emitted output only.
///
/// On Windows, repo paths from `canonicalize` may carry a `\\?\` prefix, an 8.3
/// short name (`RUNNER~1`), or a differently-cased drive letter than
/// `USERPROFILE`/`HOME`. Both sides are normalized via
/// [`crate::db::canonicalize_repo_path`] (strips `\\?\`, resolves short names,
/// case-folds on Windows) before the prefix compare.
pub fn mask_home_path(path: &str) -> String {
    let display = display_normalize_path(path);
    let path_key = path_home_compare_key(path);
    if path_key.is_empty() {
        return display;
    }

    for home_raw in home_env_candidates() {
        let home_key = path_home_compare_key(&home_raw);
        if home_key.is_empty() {
            continue;
        }
        if path_key == home_key {
            return "~/".into();
        }
        let prefix = format!("{}/", home_key.trim_end_matches('/'));
        if let Some(rest) = path_key.strip_prefix(&prefix) {
            // Preserve relative segment separators as `/` (already slash-unified).
            return format!("~/{rest}");
        }
    }
    display
}

/// Display form when not masked: strip `\\?\`, unify slashes (no case fold).
fn display_normalize_path(path: &str) -> String {
    crate::db::strip_windows_extended_prefix(path).replace('\\', "/")
}

/// Comparison key for home-prefix matching (canonical when the path exists).
fn path_home_compare_key(raw: &str) -> String {
    let key = crate::db::canonicalize_repo_path(raw);
    if !key.is_empty() {
        return key;
    }
    // Nonexistent path — lexical normalize still strips `\\?\` / unifies slashes.
    let stripped = crate::db::strip_windows_extended_prefix(raw);
    let mut s = stripped.replace('\\', "/");
    s = s.trim_end_matches('/').to_string();
    if cfg!(windows) {
        s = s.to_ascii_lowercase();
    }
    s
}

fn home_env_candidates() -> Vec<String> {
    let mut out = Vec::new();
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            let t = v.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
        }
    }
    // HOMEDRIVE + HOMEPATH (classic Windows profile).
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        let combined = format!("{}{}", drive.trim(), path.trim());
        if !combined.is_empty() && !out.iter().any(|h| h.eq_ignore_ascii_case(&combined)) {
            out.push(combined);
        }
    }
    out
}

#[cfg(test)]
fn dirs_home() -> Option<String> {
    home_env_candidates().into_iter().next()
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

    #[test]
    fn every_path_filter_has_positive_and_negative() {
        let dir = tempfile_dir("path-rules");
        fs::write(dir.join(".gitignore"), "secrets/\n*.log\n").unwrap();
        let ctx = HealthFilterContext::for_repo(&dir);

        // docs
        assert!(matches!(
            ctx.decide(&sym("a", "unused", "docs/guide.md")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_DOCS
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("a", "unused", "src/docs_helper.rs")),
            FilterDecision::Keep { .. }
        ));

        // build
        assert!(matches!(
            ctx.decide(&sym("b", "unused", "target/debug/foo.rs")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_BUILD
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("b", "unused", "src/build_plan.rs")),
            FilterDecision::Keep { .. }
        ));

        // generated
        assert!(matches!(
            ctx.decide(&sym("c", "unused", "src/api.generated.ts")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_GENERATED
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("c", "unused", "src/api.ts")),
            FilterDecision::Keep { .. }
        ));

        // gitignore
        assert!(matches!(
            ctx.decide(&sym("d", "unused", "secrets/token.txt")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_GITIGNORE
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("d", "unused", "src/token.txt")),
            FilterDecision::Keep { .. }
        ));

        // examples / tests / benches class
        assert!(matches!(
            ctx.decide(&sym("e", "unused", "examples/demo.rs")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_EXAMPLES_TESTS_BENCHES
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("e", "unused", "tests/integration.rs")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_EXAMPLES_TESTS_BENCHES
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("e", "unused", "benches/hot.rs")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_EXAMPLES_TESTS_BENCHES
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("e", "unused", "src/lib.rs")),
            FilterDecision::Keep { .. }
        ));
    }

    #[test]
    fn every_entrypoint_filter_has_positive_and_negative() {
        let dir = tempfile_dir("entry-rules");

        // --- tauri command ---
        let tauri_file = dir.join("src-tauri/src/cmds.rs");
        fs::create_dir_all(tauri_file.parent().unwrap()).unwrap();
        fs::write(
            &tauri_file,
            r#"
#[tauri::command]
pub async fn index_workspace(path: String) -> Result<(), String> { Ok(()) }

pub async fn index_workspace_helper() {}

/// Nearby attr must not bleed onto the next fn.
#[tauri::command]
pub fn open_graph() {}

pub fn open_graph_helper() {}
"#,
        )
        .unwrap();
        let ctx = HealthFilterContext::for_repo(&dir);

        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "index_workspace".into(),
                kind: "unused".into(),
                file: Some(tauri_file.to_string_lossy().into()),
                line: Some(3),
                ..DeadSymbol::default()
            }),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TAURI_COMMAND
            }
        ));
        // Negative: same file, no attribute directly above, exact name differs.
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "index_workspace_helper".into(),
                kind: "unused".into(),
                file: Some(tauri_file.to_string_lossy().into()),
                line: Some(5),
                ..DeadSymbol::default()
            }),
            FilterDecision::Keep { .. }
        ));
        // Negative: substring would have matched `open_graph` inside helper — must not.
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "open_graph_helper".into(),
                kind: "unused".into(),
                file: Some(tauri_file.to_string_lossy().into()),
                line: Some(12),
                ..DeadSymbol::default()
            }),
            FilterDecision::Keep { .. }
        ));
        // Negative: #[tauri::command] is NOT directly above (intervening non-attr code).
        assert!(!looks_like_tauri_command(
            "#[tauri::command]\npub fn real_cmd() {}\n\npub fn orphan() {}\n",
            "orphan",
            Some(4),
        ));

        // --- trait impl ---
        let trait_file = dir.join("src/trait_demo.rs");
        fs::create_dir_all(trait_file.parent().unwrap()).unwrap();
        fs::write(
            &trait_file,
            r#"
impl Display for Foo {
    fn fmt(&self, f: &mut Formatter) -> Result { Ok(()) }
    fn fmt_helper(&self) {}
}

fn fmt() {}

impl Foo {
    fn inherent_only(&self) {}
}
"#,
        )
        .unwrap();

        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "fmt".into(),
                kind: "unused".into(),
                file: Some(trait_file.to_string_lossy().into()),
                line: Some(3),
                ..DeadSymbol::default()
            }),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TRAIT_IMPL
            }
        ));
        // Still inside trait impl → excluded (entrypoint), but exact-name
        // matching is covered by exact_fn_rejects_substring_names.
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "fmt_helper".into(),
                kind: "unused".into(),
                file: Some(trait_file.to_string_lossy().into()),
                line: Some(4),
                ..DeadSymbol::default()
            }),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TRAIT_IMPL
            }
        ));
        // Negative: free fn `fmt` outside trait impl.
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "fmt".into(),
                kind: "unused".into(),
                file: Some(trait_file.to_string_lossy().into()),
                line: Some(7),
                ..DeadSymbol::default()
            }),
            FilterDecision::Keep { .. }
        ));
        // Negative: inherent impl (no `for`) — not a trait impl.
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "inherent_only".into(),
                kind: "unused".into(),
                file: Some(trait_file.to_string_lossy().into()),
                line: Some(10),
                ..DeadSymbol::default()
            }),
            FilterDecision::Keep { .. }
        ));
        // Negative: exact name — `fn fmt_helper` must not satisfy lookup for `fmt`.
        assert!(!exact_fn_signature("    fn fmt_helper(&self) {}", "fmt"));
        assert!(!looks_like_trait_impl(
            "impl Display for Foo {\n    fn fmt_helper(&self) {}\n}\n",
            "fmt",
            Some(2)
        ));

        // --- Next.js ---
        assert!(matches!(
            ctx.decide(&sym("default", "unused", "src/app/page.tsx")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_NEXT_EXPORT
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("helper", "unused", "src/app/utils.ts")),
            FilterDecision::Keep { .. }
        ));

        // --- test ---
        assert!(matches!(
            ctx.decide(&sym("test_parse", "unused", "src/lib.rs")),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TEST
            }
        ));
        assert!(matches!(
            ctx.decide(&sym("parse", "unused", "src/lib.rs")),
            FilterDecision::Keep { .. }
        ));
        // `#[test]` without a test_ prefix (file must not end with `_test.rs`).
        let bare_attr = dir.join("src/attr_only.rs");
        fs::write(
            &bare_attr,
            "#[test]\nfn paging_boundaries() {}\n\nfn helper() {}\n",
        )
        .unwrap();
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "paging_boundaries".into(),
                kind: "unused".into(),
                file: Some(bare_attr.to_string_lossy().into()),
                line: Some(2),
                ..DeadSymbol::default()
            }),
            FilterDecision::Exclude {
                rule: RULE_FILTER_TEST
            }
        ));
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "helper".into(),
                kind: "unused".into(),
                file: Some(bare_attr.to_string_lossy().into()),
                line: Some(4),
                ..DeadSymbol::default()
            }),
            FilterDecision::Keep { .. }
        ));

        // --- JSX ---
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
        assert!(matches!(
            ctx.decide(&sym("formatDate", "unused", "src/components/panel.tsx")),
            FilterDecision::Keep { .. }
        ));
    }

    #[test]
    fn absolute_paths_under_repo_root_are_filtered() {
        let dir = tempfile_dir("abs-path");
        let abs_docs = dir.join("docs").join("readme.md");
        fs::create_dir_all(abs_docs.parent().unwrap()).unwrap();
        fs::write(&abs_docs, "x").unwrap();
        let abs_src = dir.join("src").join("lib.rs");
        fs::create_dir_all(abs_src.parent().unwrap()).unwrap();
        fs::write(&abs_src, "fn orphan() {}").unwrap();
        let ctx = HealthFilterContext::for_repo(&dir);

        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "in_docs".into(),
                kind: "unused".into(),
                file: Some(abs_docs.to_string_lossy().into()),
                line: Some(1),
                ..DeadSymbol::default()
            }),
            FilterDecision::Exclude {
                rule: RULE_FILTER_DOCS
            }
        ));
        assert!(matches!(
            ctx.decide(&DeadSymbol {
                name: "orphan".into(),
                kind: "unused".into(),
                file: Some(abs_src.to_string_lossy().into()),
                line: Some(1),
                ..DeadSymbol::default()
            }),
            FilterDecision::Keep {
                severity: SEVERITY_POTANSIYEL,
                ..
            }
        ));
    }

    #[test]
    fn mask_home_path_only_prefix_not_contains() {
        let home = dirs_home().expect("home");
        let full = format!("{home}/projects/Agent-Lounge-OS/src/lib.rs");
        let masked = mask_home_path(&full);
        assert!(masked.starts_with("~/"), "{masked}");
        assert!(!masked.contains(&home), "{masked}");
        // Must not mangle a different user path that merely contains the home string.
        let other = format!("{home}2/extra/src/lib.rs");
        assert_eq!(mask_home_path(&other), other.replace('\\', "/"));
    }

    #[test]
    fn mask_home_path_strips_verbatim_prefix_lexically() {
        // Lexical path (may not exist): `\\?\` / `//?/` must still match home.
        let home = dirs_home().expect("home");
        let home_slash = home.replace('\\', "/");
        let verbatim = format!(
            r"\\?\{}\projects\Agent-Lounge-OS\src\lib.rs",
            home.trim_end_matches(['/', '\\'])
        );
        let masked = mask_home_path(&verbatim);
        assert!(
            masked.starts_with("~/"),
            "verbatim prefix must mask: {masked} (home={home_slash})"
        );
        let slash_verbatim = format!("//?/{home_slash}/projects/x.rs");
        let masked2 = mask_home_path(&slash_verbatim);
        assert!(
            masked2.starts_with("~/"),
            "//?/ prefix must mask: {masked2}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn mask_home_path_windows_verbatim_case_and_resolved_temp() {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .expect("USERPROFILE/HOME");
        let probe = PathBuf::from(&home)
            .join("al-mask-probe-8452")
            .join("nested");
        fs::create_dir_all(&probe).unwrap();

        // Canonical form often includes `\\?\` and resolves 8.3 components.
        let canon = probe.canonicalize().expect("canonicalize probe");
        let canon_s = canon.to_string_lossy().into_owned();
        let masked = mask_home_path(&canon_s);
        assert!(
            masked.starts_with("~/"),
            "canonical/verbatim must mask: {masked} from {canon_s}"
        );
        assert_eq!(masked, "~/al-mask-probe-8452/nested");

        // Drive-letter case flip (C: vs c:).
        let flipped: String = {
            let mut chars: Vec<char> = home.chars().collect();
            if let Some(c) = chars.first_mut() {
                if c.is_ascii_uppercase() {
                    *c = c.to_ascii_lowercase();
                } else if c.is_ascii_lowercase() {
                    *c = c.to_ascii_uppercase();
                }
            }
            chars.into_iter().collect()
        };
        let cased = PathBuf::from(&flipped)
            .join("al-mask-probe-8452")
            .join("nested");
        let masked_case = mask_home_path(&cased.to_string_lossy());
        assert!(
            masked_case.starts_with("~/"),
            "drive-letter case must mask: {masked_case}"
        );

        // Temp dir is typically under the profile and may use short names (RUNNER~1).
        let tmp = std::env::temp_dir();
        if let Ok(tc) = tmp.canonicalize() {
            let ts = tc.to_string_lossy().into_owned();
            let masked_tmp = mask_home_path(&ts);
            assert!(
                masked_tmp.starts_with("~/"),
                "temp under profile must mask (short-name safe): {masked_tmp} from {ts}"
            );
        }

        let _ = fs::remove_dir_all(PathBuf::from(&home).join("al-mask-probe-8452"));
    }

    #[test]
    fn exact_fn_rejects_substring_names() {
        assert!(exact_fn_signature("    fn fmt(&self) {}", "fmt"));
        assert!(!exact_fn_signature("    fn fmt_helper(&self) {}", "fmt"));
        assert!(exact_fn_signature(
            "pub async fn index_workspace(",
            "index_workspace"
        ));
        assert!(!exact_fn_signature(
            "pub async fn index_workspace_helper(",
            "index_workspace"
        ));
    }
}
