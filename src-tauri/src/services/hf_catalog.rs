//! Küratörlü Hugging Face GGUF katalogu + Hub zenginleştirme + RAM filtresi.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use serde::Deserialize;

use super::hardware;
use crate::models::{DeviceProfile, HfModelOffer, RecommendedModels};

const HUB_SEARCH: &str =
    "https://huggingface.co/api/models?filter=gguf&pipeline_tag=text-generation&sort=downloads&limit=80";
const HUB_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy)]
struct CatalogEntry {
    hf_id: &'static str,
    name: &'static str,
    family: &'static str,
    params: &'static str,
    estimated_ram_gb: f32,
    min_ram_gb: f32,
}

/// LMR/Ollama `hf.co/{org}/{repo}` ile çekilen instruct GGUF'ler.
const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        hf_id: "bartowski/Llama-3.2-1B-Instruct-GGUF",
        name: "Llama 3.2 1B Instruct",
        family: "Llama",
        params: "1B",
        estimated_ram_gb: 1.2,
        min_ram_gb: 4.0,
    },
    CatalogEntry {
        hf_id: "bartowski/Llama-3.2-3B-Instruct-GGUF",
        name: "Llama 3.2 3B Instruct",
        family: "Llama",
        params: "3B",
        estimated_ram_gb: 2.5,
        min_ram_gb: 6.0,
    },
    CatalogEntry {
        hf_id: "bartowski/Qwen2.5-3B-Instruct-GGUF",
        name: "Qwen2.5 3B Instruct",
        family: "Qwen",
        params: "3B",
        estimated_ram_gb: 2.6,
        min_ram_gb: 6.0,
    },
    CatalogEntry {
        hf_id: "bartowski/Qwen2.5-7B-Instruct-GGUF",
        name: "Qwen2.5 7B Instruct",
        family: "Qwen",
        params: "7B",
        estimated_ram_gb: 5.0,
        min_ram_gb: 8.0,
    },
    CatalogEntry {
        hf_id: "bartowski/Phi-3.5-mini-instruct-GGUF",
        name: "Phi-3.5 Mini Instruct",
        family: "Phi",
        params: "3.8B",
        estimated_ram_gb: 2.8,
        min_ram_gb: 6.0,
    },
    CatalogEntry {
        hf_id: "bartowski/gemma-2-2b-it-GGUF",
        name: "Gemma 2 2B IT",
        family: "Gemma",
        params: "2B",
        estimated_ram_gb: 1.8,
        min_ram_gb: 4.0,
    },
    CatalogEntry {
        hf_id: "bartowski/Qwen2.5-14B-Instruct-GGUF",
        name: "Qwen2.5 14B Instruct",
        family: "Qwen",
        params: "14B",
        estimated_ram_gb: 9.0,
        min_ram_gb: 16.0,
    },
];

#[derive(Debug, Clone, Default)]
pub struct HubMeta {
    pub downloads: u64,
    pub gated: bool,
    pub missing: bool,
}

#[derive(Debug, Deserialize)]
struct HubModel {
    id: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    gated: HubGated,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HubGated {
    Flag(bool),
    Mode(String),
}

impl Default for HubGated {
    fn default() -> Self {
        Self::Flag(false)
    }
}

impl HubGated {
    fn is_gated(&self) -> bool {
        match self {
            Self::Flag(value) => *value,
            Self::Mode(mode) => !mode.is_empty() && mode != "false",
        }
    }
}

pub fn normalize_pull_name(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("model id boş");
    }
    let name = if trimmed.starts_with("hf.co/") {
        trimmed.to_string()
    } else {
        format!("hf.co/{trimmed}")
    };
    let rest = name.strip_prefix("hf.co/").unwrap_or(name.as_str());
    if rest.trim().is_empty() {
        bail!("model id boş");
    }
    Ok(name)
}

pub fn strip_hf_prefix(raw: &str) -> &str {
    raw.trim().trim_start_matches("hf.co/")
}

/// Ollama 0.34 Hugging Face Xet CDN (`us.aws.cdn.hf.co`) yönlendirmesini reddeder.
pub fn is_hf_redirect_block(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("blocked redirect")
        || lower.contains("cdn.hf.co")
        || lower.contains("xet-bridge")
        || lower.contains("xethub.hf.co")
}

pub fn with_quant_tag(pull_name: &str, quant: &str) -> String {
    if pull_name.contains(':') {
        pull_name.to_string()
    } else {
        format!("{pull_name}:{quant}")
    }
}

/// Bartowski GGUF: `{repo-without-GGUF}-Q4_K_M.gguf`
pub fn gguf_filename_for(hf_id: &str) -> String {
    let id = strip_hf_prefix(hf_id);
    let repo = id.rsplit('/').next().unwrap_or(id);
    let stem = repo
        .strip_suffix("-GGUF")
        .or_else(|| repo.strip_suffix("-gguf"))
        .unwrap_or(repo);
    format!("{stem}-Q4_K_M.gguf")
}

pub fn hf_gguf_resolve_url(hf_id: &str, filename: &str) -> String {
    let id = strip_hf_prefix(hf_id);
    format!("https://huggingface.co/{id}/resolve/main/{filename}")
}

pub fn is_installed(hf_id: &str, pull_name: &str, tags: &[String]) -> bool {
    let hf = hf_id.to_ascii_lowercase();
    let pull = pull_name.to_ascii_lowercase();
    tags.iter().any(|tag| {
        let t = tag.to_ascii_lowercase();
        t == pull || t.contains(&hf) || t.contains(&pull)
    })
}

pub async fn list_recommended(
    device: DeviceProfile,
    installed: &[String],
) -> Result<RecommendedModels> {
    let hub = fetch_hub_meta().await.unwrap_or_default();
    Ok(RecommendedModels {
        offers: offers_for_device(&device, installed, &hub),
        device,
    })
}

pub fn offers_for_device(
    device: &DeviceProfile,
    installed: &[String],
    hub: &HashMap<String, HubMeta>,
) -> Vec<HfModelOffer> {
    let mut offers: Vec<HfModelOffer> = CATALOG
        .iter()
        .filter_map(|entry| offer_from_entry(entry, device, installed, hub))
        .collect();
    offers.sort_by(|a, b| {
        b.recommended
            .cmp(&a.recommended)
            .then(a.heavy.cmp(&b.heavy))
            .then(
                a.estimated_ram_gb
                    .partial_cmp(&b.estimated_ram_gb)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(b.downloads.cmp(&a.downloads))
    });
    offers
}

fn offer_from_entry(
    entry: &CatalogEntry,
    device: &DeviceProfile,
    installed: &[String],
    hub: &HashMap<String, HubMeta>,
) -> Option<HfModelOffer> {
    if let Some(meta) = hub.get(entry.hf_id) {
        if meta.missing || meta.gated {
            return None;
        }
    }
    let pull_name = format!("hf.co/{}", entry.hf_id);
    let installed = is_installed(entry.hf_id, &pull_name, installed);
    let fits = entry.estimated_ram_gb <= device.usable_budget_gb;
    let heavy = !fits;
    let recommended = fits && !heavy;
    let disabled_reason = if heavy && !installed {
        Some(format!(
            "cihaz için ağır (tahmini {:.1} GB > bütçe {:.1} GB)",
            entry.estimated_ram_gb, device.usable_budget_gb
        ))
    } else {
        None
    };
    let downloads = hub.get(entry.hf_id).map(|meta| meta.downloads).unwrap_or(0);
    Some(HfModelOffer {
        id: entry.hf_id.to_string(),
        hf_id: entry.hf_id.to_string(),
        pull_name,
        name: entry.name.to_string(),
        family: entry.family.to_string(),
        params: entry.params.to_string(),
        estimated_ram_gb: entry.estimated_ram_gb,
        min_ram_gb: entry.min_ram_gb,
        downloads,
        recommended,
        installed,
        heavy,
        disabled_reason,
    })
}

pub fn parse_hub_models(raw: &str) -> Result<HashMap<String, HubMeta>> {
    let rows: Vec<HubModel> = serde_json::from_str(raw)?;
    Ok(rows.into_iter().filter_map(hub_meta_from_row).collect())
}

fn hub_meta_from_row(row: HubModel) -> Option<(String, HubMeta)> {
    let id = row.id.filter(|value| !value.is_empty())?;
    Some((
        id,
        HubMeta {
            downloads: row.downloads,
            gated: row.gated.is_gated(),
            missing: false,
        },
    ))
}

async fn fetch_hub_meta() -> Result<HashMap<String, HubMeta>> {
    let client = reqwest::Client::builder().timeout(HUB_TIMEOUT).build()?;
    let response = client.get(HUB_SEARCH).send().await?;
    if !response.status().is_success() {
        bail!("Hugging Face Hub HTTP {}", response.status());
    }
    let body = response.text().await?;
    let mut map = parse_hub_models(&body).unwrap_or_default();
    let missing: Vec<&'static str> = CATALOG
        .iter()
        .map(|entry| entry.hf_id)
        .filter(|id| !map.contains_key(*id))
        .collect();
    if !missing.is_empty() {
        let extras = fetch_hub_ids(&client, &missing).await;
        map.extend(extras);
    }
    Ok(map)
}

async fn fetch_hub_ids(client: &reqwest::Client, ids: &[&str]) -> HashMap<String, HubMeta> {
    let mut map = HashMap::new();
    for id in ids {
        let id = (*id).to_string();
        let url = format!("https://huggingface.co/api/models/{id}");
        let meta = match client.get(&url).send().await {
            Ok(response) if response.status().as_u16() == 404 => HubMeta {
                missing: true,
                ..HubMeta::default()
            },
            Ok(response) if response.status().is_success() => {
                match response.json::<HubModel>().await {
                    Ok(row) => hub_meta_from_row(row)
                        .map(|(_, meta)| meta)
                        .unwrap_or(HubMeta {
                            missing: true,
                            ..HubMeta::default()
                        }),
                    Err(_) => HubMeta::default(),
                }
            }
            Ok(_) | Err(_) => HubMeta::default(),
        };
        map.insert(id, meta);
    }
    map
}

pub fn current_device() -> DeviceProfile {
    hardware::device_profile()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eight_gb_device() -> DeviceProfile {
        hardware::profile_from_bytes(
            8 * 1024 * 1024 * 1024,
            3 * 1024 * 1024 * 1024,
            "aarch64",
            true,
            None,
        )
    }

    #[test]
    fn eight_gb_opens_1b_3b_closes_14b() {
        let device = eight_gb_device();
        let offers = offers_for_device(&device, &[], &HashMap::new());
        let by_params = |params: &str| {
            offers
                .iter()
                .filter(|offer| offer.params == params)
                .collect::<Vec<_>>()
        };
        assert!(by_params("1B")
            .iter()
            .all(|offer| offer.recommended && !offer.heavy));
        assert!(by_params("3B")
            .iter()
            .all(|offer| offer.recommended && !offer.heavy));
        let fourteen = by_params("14B");
        assert!(!fourteen.is_empty());
        assert!(fourteen
            .iter()
            .all(|offer| offer.heavy && !offer.recommended));
        assert!(fourteen.iter().all(|offer| offer
            .disabled_reason
            .as_deref()
            .unwrap_or("")
            .contains("cihaz için ağır")));
    }

    #[test]
    fn gated_or_missing_hub_rows_are_dropped() {
        let device = eight_gb_device();
        let mut hub = HashMap::new();
        hub.insert(
            "bartowski/Llama-3.2-1B-Instruct-GGUF".into(),
            HubMeta {
                downloads: 10,
                gated: true,
                missing: false,
            },
        );
        hub.insert(
            "bartowski/Qwen2.5-14B-Instruct-GGUF".into(),
            HubMeta {
                downloads: 0,
                gated: false,
                missing: true,
            },
        );
        let offers = offers_for_device(&device, &[], &hub);
        assert!(!offers
            .iter()
            .any(|offer| offer.hf_id.contains("Llama-3.2-1B")));
        assert!(!offers.iter().any(|offer| offer.params == "14B"));
    }

    #[test]
    fn parses_hub_json_fixture() {
        let raw = r#"[{"id":"bartowski/Llama-3.2-1B-Instruct-GGUF","downloads":12345,"gated":false},{"id":"org/secret","downloads":1,"gated":"auto"}]"#;
        let map = parse_hub_models(raw).unwrap();
        assert_eq!(map["bartowski/Llama-3.2-1B-Instruct-GGUF"].downloads, 12345);
        assert!(!map["bartowski/Llama-3.2-1B-Instruct-GGUF"].gated);
        assert!(map["org/secret"].gated);
    }

    #[test]
    fn pull_name_requires_hf_co_and_rejects_empty() {
        assert_eq!(
            normalize_pull_name("bartowski/Llama-3.2-1B-Instruct-GGUF").unwrap(),
            "hf.co/bartowski/Llama-3.2-1B-Instruct-GGUF"
        );
        assert!(
            normalize_pull_name("hf.co/bartowski/Llama-3.2-1B-Instruct-GGUF")
                .unwrap()
                .starts_with("hf.co/")
        );
        assert!(normalize_pull_name("").is_err());
        assert!(normalize_pull_name("   ").is_err());
        assert!(normalize_pull_name("hf.co/").is_err());
    }

    #[test]
    fn gemma_q4_filename_matches_hf_cdn_object() {
        assert_eq!(
            gguf_filename_for("bartowski/gemma-2-2b-it-GGUF"),
            "gemma-2-2b-it-Q4_K_M.gguf"
        );
        assert!(
            hf_gguf_resolve_url("bartowski/gemma-2-2b-it-GGUF", "gemma-2-2b-it-Q4_K_M.gguf")
                .starts_with("https://huggingface.co/bartowski/gemma-2-2b-it-GGUF/resolve/main/")
        );
    }

    #[test]
    fn detects_xet_cdn_redirect_block() {
        let err = r#"Head "https://us.aws.cdn.hf.co/xet-bridge-us/abc?filename=gemma-2-2b-it-Q4_K_M.gguf": blocked redirect to a different host"#;
        assert!(is_hf_redirect_block(err));
        assert!(!is_hf_redirect_block("file not found"));
    }

    #[test]
    fn installed_tags_match_case_insensitively() {
        let tags = vec!["hf.co/bartowski/llama-3.2-1b-instruct-gguf:Q4_K_M".into()];
        assert!(is_installed(
            "bartowski/Llama-3.2-1B-Instruct-GGUF",
            "hf.co/bartowski/Llama-3.2-1B-Instruct-GGUF",
            &tags
        ));
    }
}
