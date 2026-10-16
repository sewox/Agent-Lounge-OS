//! H6: Linux distro + package-manager detection via `/etc/os-release`.
//!
//! Never assumes apt. Maps ID/ID_LIKE to apt, dnf/yum, zypper, pacman, apk;
//! falls back to `command -v` only when os-release does not map.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OsReleaseInfo {
    pub id: Option<String>,
    pub id_like: Option<String>,
    /// apt | dnf | yum | zypper | pacman | apk | none | unknown | n/a
    pub package_manager: String,
}

/// Parse os-release text (fixture-friendly).
pub fn parse_os_release(text: &str) -> (Option<String>, Option<String>) {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        map.insert(key.trim().to_ascii_uppercase(), value);
    }
    (
        map.get("ID").cloned().filter(|s| !s.is_empty()),
        map.get("ID_LIKE").cloned().filter(|s| !s.is_empty()),
    )
}

/// Map distro id / id_like → package manager family (no PATH probing).
pub fn package_manager_from_ids(id: Option<&str>, id_like: Option<&str>) -> Option<&'static str> {
    if let Some(pm) = pm_from_single_id(id.unwrap_or("")) {
        return Some(pm);
    }
    let like = id_like.unwrap_or("").to_ascii_lowercase();
    for token in like.split_whitespace() {
        if let Some(pm) = pm_from_single_id(token) {
            return Some(pm);
        }
    }
    None
}

fn pm_from_single_id(id: &str) -> Option<&'static str> {
    let id = id.trim().trim_matches('"').trim_matches('\'').to_ascii_lowercase();
    if id.is_empty() {
        return None;
    }
    // opensuse* / suse*
    if id.starts_with("opensuse") || id.starts_with("sles") || id.starts_with("suse") {
        return Some("zypper");
    }
    match id.as_str() {
        "debian" | "ubuntu" | "linuxmint" | "pop" | "elementary" | "raspbian" | "neon" => {
            Some("apt")
        }
        "fedora" | "rhel" | "centos" | "rocky" | "almalinux" | "ol" | "amzn" | "nobara" => {
            // Prefer dnf when both exist; callers may refine via which_pm_binary.
            Some("dnf")
        }
        "arch" | "manjaro" | "endeavouros" | "garuda" | "artix" => Some("pacman"),
        "alpine" | "postmarketos" => Some("apk"),
        _ => None,
    }
}

/// Refine fedora-family `dnf` → `yum` when only yum is on PATH.
pub fn refine_rpm_pm(pm: &str, has_dnf: bool, has_yum: bool) -> String {
    if pm == "dnf" || pm == "yum" {
        if has_dnf {
            return "dnf".into();
        }
        if has_yum {
            return "yum".into();
        }
        return "dnf".into(); // id said rpm family; binary may be absent in fixtures
    }
    pm.to_string()
}

/// Fallback when os-release did not map: probe binaries in fixed order.
pub fn package_manager_from_path(
    has_apt: bool,
    has_dnf: bool,
    has_yum: bool,
    has_zypper: bool,
    has_pacman: bool,
    has_apk: bool,
) -> &'static str {
    if has_apt {
        "apt"
    } else if has_dnf {
        "dnf"
    } else if has_yum {
        "yum"
    } else if has_zypper {
        "zypper"
    } else if has_pacman {
        "pacman"
    } else if has_apk {
        "apk"
    } else {
        "none"
    }
}

fn which_exists(name: &str) -> bool {
    crate::services::probe::find_executable(name).is_some()
}

/// Build [`OsReleaseInfo`] from os-release file contents + optional PATH probes.
pub fn detect_from_os_release_text(
    text: Option<&str>,
    probe_path: bool,
) -> OsReleaseInfo {
    let (id, id_like) = match text {
        Some(t) => parse_os_release(t),
        None => (None, None),
    };

    let mapped = package_manager_from_ids(id.as_deref(), id_like.as_deref());
    let package_manager = if let Some(pm) = mapped {
        if probe_path && (pm == "dnf" || pm == "yum") {
            refine_rpm_pm(pm, which_exists("dnf"), which_exists("yum"))
        } else {
            pm.to_string()
        }
    } else if text.is_none() {
        // Non-Linux callers pass None.
        "n/a".into()
    } else if probe_path {
        package_manager_from_path(
            which_exists("apt-get") || which_exists("apt"),
            which_exists("dnf"),
            which_exists("yum"),
            which_exists("zypper"),
            which_exists("pacman"),
            which_exists("apk"),
        )
        .to_string()
    } else {
        // Unmapped ID (or empty os-release) without PATH probing.
        "unknown".into()
    };

    OsReleaseInfo {
        id,
        id_like,
        package_manager,
    }
}

/// Read `/etc/os-release` (or `path`) and detect package manager.
pub fn detect_linux_os_release(path: &Path) -> OsReleaseInfo {
    let text = std::fs::read_to_string(path).ok();
    detect_from_os_release_text(text.as_deref(), true)
}

/// Host platform snapshot for lounge_health.
pub fn platform_health() -> crate::models::PlatformHealthReport {
    platform_health_with(detect_linux_os_release)
}

pub fn platform_health_with(
    linux_detect: impl FnOnce(&Path) -> OsReleaseInfo,
) -> crate::models::PlatformHealthReport {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };
    let arch = std::env::consts::ARCH.to_string();

    if os != "linux" {
        return crate::models::PlatformHealthReport {
            os: os.into(),
            arch,
            distro_id: None,
            distro_id_like: None,
            package_manager: "n/a".into(),
        };
    }

    let info = linux_detect(Path::new("/etc/os-release"));
    crate::models::PlatformHealthReport {
        os: os.into(),
        arch,
        distro_id: info.id,
        distro_id_like: info.id_like,
        package_manager: info.package_manager,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &[(&str, &str, &str)] = &[
        (
            "debian",
            "ID=debian\nID_LIKE=debian\n",
            "apt",
        ),
        (
            "ubuntu",
            "ID=ubuntu\nID_LIKE=debian\n",
            "apt",
        ),
        (
            "fedora",
            "ID=fedora\nID_LIKE=\"rhel fedora\"\n",
            "dnf",
        ),
        (
            "centos",
            "ID=\"centos\"\nID_LIKE=\"rhel fedora\"\n",
            "dnf",
        ),
        (
            "opensuse",
            "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"suse opensuse\"\n",
            "zypper",
        ),
        (
            "arch",
            "ID=arch\nID_LIKE=archlinux\n",
            "pacman",
        ),
        (
            "alpine",
            "ID=alpine\nID_LIKE=alpine\n",
            "apk",
        ),
        (
            "unknown",
            "ID=exoticdistro\nNAME=Exotic\n",
            "unknown",
        ),
    ];

    #[test]
    fn t6_os_release_fixtures_map_package_managers() {
        for (name, text, expect_pm) in FIXTURES {
            let info = detect_from_os_release_text(Some(text), false);
            assert_eq!(
                info.package_manager.as_str(),
                *expect_pm,
                "fixture {name}: id={:?} like={:?}",
                info.id,
                info.id_like
            );
            assert!(
                info.id.is_some() || *name == "unknown",
                "fixture {name} should parse ID"
            );
        }
    }

    #[test]
    fn unknown_without_id_is_unknown() {
        let info = detect_from_os_release_text(Some("# empty\n"), false);
        assert_eq!(info.package_manager, "unknown");
    }

    #[test]
    fn non_linux_text_none_is_na() {
        let info = detect_from_os_release_text(None, false);
        assert_eq!(info.package_manager, "n/a");
    }

    #[test]
    fn refine_rpm_prefers_dnf_then_yum() {
        assert_eq!(refine_rpm_pm("dnf", true, true), "dnf");
        assert_eq!(refine_rpm_pm("dnf", false, true), "yum");
        assert_eq!(refine_rpm_pm("apt", false, true), "apt");
    }

    #[test]
    fn path_fallback_order() {
        assert_eq!(
            package_manager_from_path(false, false, false, false, false, true),
            "apk"
        );
        assert_eq!(
            package_manager_from_path(false, false, false, false, false, false),
            "none"
        );
    }

    #[test]
    fn platform_health_with_fixture_on_linux_shape() {
        let report = platform_health_with(|_| detect_from_os_release_text(
            Some("ID=ubuntu\nID_LIKE=debian\n"),
            false,
        ));
        assert!(!report.os.is_empty());
        assert!(!report.arch.is_empty());
        if cfg!(target_os = "linux") {
            assert_eq!(report.distro_id.as_deref(), Some("ubuntu"));
            assert_eq!(report.package_manager, "apt");
        } else {
            assert_eq!(report.package_manager, "n/a");
        }
    }
}
