//! Opsiyonel Qdrant / Chroma REST katmanı. Ayarlı değilse SQLite vektörler yeter.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::models::{ExperienceHit, ExperienceRecord};

const COLLECTION: &str = "lounge-experiences";
const TIMEOUT: Duration = Duration::from_millis(400);

static QDRANT_DOWN: AtomicBool = AtomicBool::new(false);
static CHROMA_DOWN: AtomicBool = AtomicBool::new(false);
static HTTP: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> &'static reqwest::Client {
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .expect("vector http client")
    })
}

fn env_url(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn qdrant_url() -> Option<String> {
    env_url("LOUNGE_QDRANT_URL")
}

fn chroma_url() -> Option<String> {
    env_url("LOUNGE_CHROMA_URL")
}

pub fn spawn_upsert(record: ExperienceRecord) {
    if qdrant_url().is_none() && chroma_url().is_none() {
        return;
    }
    if record.embedding.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(err) = upsert_experience(&record).await {
            log::debug!("vektör upsert atlandı: {err}");
        }
    });
}

pub async fn upsert_experience(record: &ExperienceRecord) -> anyhow::Result<()> {
    let mut last = Ok(());
    if let Some(url) = qdrant_url() {
        last = upsert_qdrant(&url, record).await;
    }
    if let Some(url) = chroma_url() {
        last = upsert_chroma(&url, record).await;
    }
    last
}

pub async fn search_remote(query: &[f32], limit: usize) -> Vec<ExperienceHit> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    if let Some(url) = qdrant_url() {
        match search_qdrant(&url, query, limit).await {
            Ok(mut rows) => hits.append(&mut rows),
            Err(err) => log::debug!("qdrant search: {err}"),
        }
    }
    if let Some(url) = chroma_url() {
        match search_chroma(&url, query, limit).await {
            Ok(mut rows) => hits.append(&mut rows),
            Err(err) => log::debug!("chroma search: {err}"),
        }
    }
    hits
}

pub fn merge_hits(
    local: Vec<ExperienceHit>,
    remote: Vec<ExperienceHit>,
    limit: usize,
) -> Vec<ExperienceHit> {
    let mut by_id: std::collections::HashMap<String, ExperienceHit> =
        std::collections::HashMap::new();
    for hit in local.into_iter().chain(remote) {
        match by_id.get(&hit.id) {
            Some(existing) if existing.score >= hit.score => {}
            _ => {
                by_id.insert(hit.id.clone(), hit);
            }
        }
    }
    let mut hits: Vec<_> = by_id.into_values().collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    hits
}

async fn upsert_qdrant(base: &str, record: &ExperienceRecord) -> anyhow::Result<()> {
    if QDRANT_DOWN.load(Ordering::Relaxed) {
        anyhow::bail!("qdrant down");
    }
    let url = format!(
        "{}/collections/{COLLECTION}/points?wait=false",
        base.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "points": [{
            "id": record.id,
            "vector": record.embedding,
            "payload": {
                "project_id": record.project_id,
                "agent_id": record.agent_id,
                "topic": record.topic,
                "solution_summary": record.solution_summary,
                "adr_record": record.adr_record,
            }
        }]
    });
    let response = client()
        .put(&url)
        .json(&body)
        .send()
        .await
        .inspect_err(|_| {
            QDRANT_DOWN.store(true, Ordering::Relaxed);
        })?;
    if !response.status().is_success() {
        anyhow::bail!("qdrant upsert {}", response.status());
    }
    Ok(())
}

async fn search_qdrant(
    base: &str,
    query: &[f32],
    limit: usize,
) -> anyhow::Result<Vec<ExperienceHit>> {
    if QDRANT_DOWN.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }
    let url = format!(
        "{}/collections/{COLLECTION}/points/search",
        base.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "vector": query,
        "limit": limit,
        "with_payload": true,
        "score_threshold": 0.22
    });
    let response = client()
        .post(&url)
        .json(&body)
        .send()
        .await
        .inspect_err(|_| {
            QDRANT_DOWN.store(true, Ordering::Relaxed);
        })?;
    if !response.status().is_success() {
        anyhow::bail!("qdrant search {}", response.status());
    }
    let payload: serde_json::Value = response.json().await?;
    Ok(hits_from_qdrant(&payload))
}

fn hits_from_qdrant(payload: &serde_json::Value) -> Vec<ExperienceHit> {
    let Some(rows) = payload.get("result").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            })?;
            let score = row.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let data = row
                .get("payload")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Some(ExperienceHit {
                id,
                project_id: string_field(&data, "project_id"),
                agent_id: string_field(&data, "agent_id"),
                topic: string_field(&data, "topic"),
                solution_summary: string_field(&data, "solution_summary"),
                adr_record: string_field(&data, "adr_record"),
                score,
                source: "qdrant".into(),
            })
        })
        .collect()
}

async fn upsert_chroma(base: &str, record: &ExperienceRecord) -> anyhow::Result<()> {
    if CHROMA_DOWN.load(Ordering::Relaxed) {
        anyhow::bail!("chroma down");
    }
    let url = format!(
        "{}/api/v1/collections/{COLLECTION}/upsert",
        base.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "ids": [record.id],
        "embeddings": [record.embedding],
        "metadatas": [{
            "project_id": record.project_id,
            "agent_id": record.agent_id,
            "topic": record.topic,
            "solution_summary": record.solution_summary,
            "adr_record": record.adr_record,
        }]
    });
    let response = client()
        .post(&url)
        .json(&body)
        .send()
        .await
        .inspect_err(|_| {
            CHROMA_DOWN.store(true, Ordering::Relaxed);
        })?;
    if !response.status().is_success() {
        anyhow::bail!("chroma upsert {}", response.status());
    }
    Ok(())
}

async fn search_chroma(
    base: &str,
    query: &[f32],
    limit: usize,
) -> anyhow::Result<Vec<ExperienceHit>> {
    if CHROMA_DOWN.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }
    let url = format!(
        "{}/api/v1/collections/{COLLECTION}/query",
        base.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "query_embeddings": [query],
        "n_results": limit
    });
    let response = client()
        .post(&url)
        .json(&body)
        .send()
        .await
        .inspect_err(|_| {
            CHROMA_DOWN.store(true, Ordering::Relaxed);
        })?;
    if !response.status().is_success() {
        anyhow::bail!("chroma search {}", response.status());
    }
    let payload: serde_json::Value = response.json().await?;
    Ok(hits_from_chroma(&payload))
}

fn hits_from_chroma(payload: &serde_json::Value) -> Vec<ExperienceHit> {
    let ids = payload
        .pointer("/ids/0")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let distances = payload
        .pointer("/distances/0")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let metas = payload
        .pointer("/metadatas/0")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    ids.iter()
        .enumerate()
        .filter_map(|(index, id)| {
            let id = id.as_str()?.to_string();
            let distance = distances.get(index).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            let score = (1.0 - distance).clamp(0.0, 1.0);
            let data = metas.get(index).cloned().unwrap_or(serde_json::Value::Null);
            Some(ExperienceHit {
                id,
                project_id: string_field(&data, "project_id"),
                agent_id: string_field(&data, "agent_id"),
                topic: string_field(&data, "topic"),
                solution_summary: string_field(&data, "solution_summary"),
                adr_record: string_field(&data, "adr_record"),
                score,
                source: "chroma".into(),
            })
        })
        .collect()
}

fn string_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_higher_score_and_limit() {
        let local = vec![ExperienceHit {
            id: "a".into(),
            project_id: "p1".into(),
            agent_id: "cursor".into(),
            topic: "nats".into(),
            solution_summary: "local".into(),
            adr_record: "adr".into(),
            score: 0.4,
            source: "sqlite".into(),
        }];
        let remote = vec![
            ExperienceHit {
                id: "a".into(),
                project_id: "p1".into(),
                agent_id: "cursor".into(),
                topic: "nats".into(),
                solution_summary: "qdrant".into(),
                adr_record: "adr".into(),
                score: 0.8,
                source: "qdrant".into(),
            },
            ExperienceHit {
                id: "b".into(),
                project_id: "p2".into(),
                agent_id: "claude".into(),
                topic: "other".into(),
                solution_summary: "chroma".into(),
                adr_record: "adr".into(),
                score: 0.5,
                source: "chroma".into(),
            },
        ];
        let merged = merge_hits(local, remote, 2);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a");
        assert_eq!(merged[0].source, "qdrant");
        assert_eq!(merged[0].score, 0.8);
        assert_eq!(merged[1].id, "b");
    }
}
