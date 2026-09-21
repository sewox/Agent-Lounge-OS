//! Cihaz RAM / mimari profili — LMR model bütçesi.

use sysinfo::System;

use crate::models::DeviceProfile;

const GIB: f32 = 1024.0 * 1024.0 * 1024.0;
const OS_RESERVE_GB: f32 = 6.0;
const MIN_BUDGET_GB: f32 = 4.0;
const RATIO: f32 = 0.45;

pub fn device_profile() -> DeviceProfile {
    let mut sys = System::new();
    sys.refresh_memory();
    profile_from_bytes(
        sys.total_memory(),
        sys.available_memory(),
        std::env::consts::ARCH,
        is_apple_silicon(),
        ram_budget_override(),
    )
}

pub fn ram_budget_override() -> Option<f32> {
    std::env::var("LOUNGE_LMR_RAM_BUDGET_GB")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0.0)
}

pub fn is_apple_silicon() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// `LOUNGE_LMR_RAM_BUDGET_GB` yoksa `min(max(4, total − 6), total × 0.45)`.
pub fn usable_budget_gb(total_gb: f32, env_override: Option<f32>) -> f32 {
    if let Some(value) = env_override {
        return value.max(1.0);
    }
    if total_gb <= 0.0 {
        return MIN_BUDGET_GB;
    }
    let headroom = (total_gb - OS_RESERVE_GB).max(MIN_BUDGET_GB);
    let ratio = total_gb * RATIO;
    let budget = headroom.min(ratio).max(MIN_BUDGET_GB.min(total_gb));
    budget.min(total_gb).max(1.0)
}

pub fn recommended_upper(budget_gb: f32) -> &'static str {
    if budget_gb >= 10.0 {
        "14B Q4"
    } else if budget_gb >= 6.0 {
        "8B Q4"
    } else if budget_gb >= 4.0 {
        "3B Q4"
    } else {
        "1B Q4"
    }
}

pub fn profile_from_bytes(
    total_bytes: u64,
    available_bytes: u64,
    arch: &str,
    apple_silicon: bool,
    env_override: Option<f32>,
) -> DeviceProfile {
    let total_ram_gb = bytes_to_gb(total_bytes);
    let available_ram_gb = bytes_to_gb(available_bytes);
    let usable_budget_gb = usable_budget_gb(total_ram_gb, env_override);
    let recommended_upper = recommended_upper(usable_budget_gb).to_string();
    let metal = apple_silicon;
    let chip = if apple_silicon {
        "Apple Silicon".to_string()
    } else {
        arch.to_string()
    };
    let memory_noun = if apple_silicon {
        "birleşik bellek"
    } else {
        "RAM"
    };
    let summary = format!(
        "{:.0} GB {memory_noun} · {chip} · önerilen üst sınır {recommended_upper}",
        total_ram_gb.round()
    );
    DeviceProfile {
        total_ram_gb,
        available_ram_gb,
        usable_budget_gb,
        arch: arch.to_string(),
        apple_silicon,
        metal,
        summary,
        recommended_upper,
    }
}

fn bytes_to_gb(bytes: u64) -> f32 {
    bytes as f32 / GIB
}

#[cfg(test)]
mod tests {
    use super::*;

    const EIGHT_GIB: u64 = 8 * 1024 * 1024 * 1024;
    const THREE_GIB: u64 = 3 * 1024 * 1024 * 1024;

    #[test]
    fn eight_gb_budget_is_four() {
        let budget = usable_budget_gb(8.0, None);
        assert!((budget - 4.0).abs() < 0.05, "budget={budget}");
        assert_eq!(recommended_upper(budget), "3B Q4");
    }

    #[test]
    fn sixteen_gb_summary_caps_at_8b() {
        let profile = profile_from_bytes(
            16 * 1024 * 1024 * 1024,
            10 * 1024 * 1024 * 1024,
            "aarch64",
            true,
            None,
        );
        assert!(profile.apple_silicon);
        assert!(profile.metal);
        assert!(profile.usable_budget_gb > 6.0);
        assert!(profile.usable_budget_gb < 9.0);
        assert_eq!(profile.recommended_upper, "8B Q4");
        assert!(profile.summary.contains("16 GB birleşik bellek"));
        assert!(profile.summary.contains("Apple Silicon"));
        assert!(profile.summary.contains("8B Q4"));
    }

    #[test]
    fn env_override_wins() {
        let profile = profile_from_bytes(EIGHT_GIB, THREE_GIB, "x86_64", false, Some(12.0));
        assert!((profile.usable_budget_gb - 12.0).abs() < 0.01);
        assert!(!profile.metal);
        assert_eq!(profile.arch, "x86_64");
    }
}
