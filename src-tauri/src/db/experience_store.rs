//! DecisionGate `KNOWLEDGE_HIT` sonrası Cross-Project Memory: SQLite + vektör + AST.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::embedding::{lexical_embedding, tokenize};
use super::vector_memory;
use super::ExperienceStore;
use crate::models::{CodeSnippet, ExperienceContext, LoungeTask};

const DEFAULT_LIMIT: usize = 4;
const MAX_SNIPPETS: usize = 4;
const SNIPPET_RADIUS: usize = 16;
const MAX_SNIPPET_CHARS: usize = 900;
const MAX_PROMPT_CHARS: usize = 4000;
const BODY_SNIPPETS: usize = 2;
const KNOWLEDGE_HIT_THRESHOLD: f32 = 0.3;

#[derive(Debug, Clone, Default)]
pub struct FastRetrieveQuery {
    pub project_id: String,
    pub text: String,
    pub embedding: Option<Vec<f32>>,
    pub knowledge_hit: f32,
    pub ast_refs: Vec<String>,
    pub limit: Option<usize>,
}

impl FastRetrieveQuery {
    pub fn from_task(task: &LoungeTask, knowledge_hit: f32, embedding: Option<Vec<f32>>) -> Self {
        Self {
            project_id: task.project_id.clone(),
            text: task.summary.clone(),
            embedding,
            knowledge_hit,
            ast_refs: task.ast_refs.clone(),
            limit: Some(DEFAULT_LIMIT),
        }
    }
}

pub fn knowledge_hit_triggers(knowledge_hit: f32, subject: &str, query: &str) -> bool {
    knowledge_hit >= KNOWLEDGE_HIT_THRESHOLD
        && subject == crate::models::TASK_REQUESTED
        && !query.trim().is_empty()
}

pub async fn fast_retrieve(
    store: &ExperienceStore,
    query: FastRetrieveQuery,
) -> Result<ExperienceContext> {
    store.fast_retrieve(query).await
}

impl ExperienceStore {
    pub async fn fast_retrieve(&self, query: FastRetrieveQuery) -> Result<ExperienceContext> {
        let limit = query.limit.unwrap_or(DEFAULT_LIMIT).max(1);
        let embedding = query
            .embedding
            .clone()
            .filter(|row| !row.is_empty())
            .unwrap_or_else(|| lexical_embedding(&query.text));

        let sqlite = self
            .similar_cross(query.project_id.clone(), embedding.clone(), Some(limit * 2))
            .await
            .context("fast_retrieve sqlite")?;
        let remote = vector_memory::search_remote(&embedding, limit).await;
        let experiences = vector_memory::merge_hits(sqlite, remote, limit);

        let mut needle = query.text.clone();
        for hit in &experiences {
            needle.push(' ');
            needle.push_str(&hit.topic);
            needle.push(' ');
            needle.push_str(&hit.solution_summary);
        }
        for ast_ref in &query.ast_refs {
            needle.push(' ');
            needle.push_str(ast_ref);
        }

        let snippets = self
            .ast_snippets(&needle, &query.ast_refs, MAX_SNIPPETS)
            .await
            .unwrap_or_else(|err| {
                log::debug!("AST snippet atlandı: {err}");
                Vec::new()
            });

        let mut context = ExperienceContext {
            experiences,
            snippets,
            knowledge_hit: Some(query.knowledge_hit),
        };
        trim_prompt_budget(&mut context);
        Ok(context)
    }

    async fn ast_snippets(
        &self,
        needle: &str,
        ast_refs: &[String],
        limit: usize,
    ) -> Result<Vec<CodeSnippet>> {
        let conn = self.conn.clone();
        let needle = needle.to_string();
        let ast_refs = ast_refs.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            ast_snippets_blocking(&conn, &needle, &ast_refs, limit)
        })
        .await
        .context("ast snippet join")?
    }
}

fn trim_prompt_budget(context: &mut ExperienceContext) {
    for (index, snippet) in context.snippets.iter_mut().enumerate() {
        if index >= BODY_SNIPPETS {
            snippet.body.clear();
        } else if snippet.body.len() > MAX_SNIPPET_CHARS {
            snippet.body.truncate(MAX_SNIPPET_CHARS);
            snippet.body.push('…');
        }
    }
    let mut block = context.prompt_block();
    while block.len() > MAX_PROMPT_CHARS {
        if let Some(last) = context.snippets.pop() {
            let _ = last;
        } else if context.experiences.len() > 1 {
            context.experiences.pop();
        } else {
            break;
        }
        block = context.prompt_block();
    }
}

struct AstCandidate {
    project_id: String,
    repo_path: String,
    name: String,
    file_path: Option<String>,
    line: Option<i64>,
    kind: String,
    target: Option<String>,
    ref_count: i64,
    score: u32,
}

fn ast_snippets_blocking(
    conn: &Connection,
    needle: &str,
    ast_refs: &[String],
    limit: usize,
) -> Result<Vec<CodeSnippet>> {
    let tokens = query_tokens(needle, ast_refs);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT project_id, repo_path, name, file_path, line, detail, target, ref_count
        FROM project_index
        WHERE kind = 'node'
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AstCandidate {
            project_id: row.get(0)?,
            repo_path: row.get(1)?,
            name: row.get(2)?,
            file_path: row.get(3)?,
            line: row.get(4)?,
            kind: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            target: row.get(6)?,
            ref_count: row.get(7)?,
            score: 0,
        })
    })?;

    let mut ranked = Vec::new();
    for row in rows {
        let mut candidate = row?;
        candidate.score = score_candidate(&candidate, &tokens, ast_refs);
        if candidate.score > 0 {
            ranked.push(candidate);
        }
    }
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.ref_count.cmp(&a.ref_count))
    });
    ranked.truncate(limit.max(MAX_SNIPPETS));

    let mut snippets = Vec::new();
    for (index, candidate) in ranked.into_iter().take(limit).enumerate() {
        let include_body = index < BODY_SNIPPETS;
        snippets.push(snippet_from_candidate(&candidate, include_body));
    }
    Ok(snippets)
}

fn query_tokens(needle: &str, ast_refs: &[String]) -> Vec<String> {
    let mut tokens = tokenize(needle);
    for ast_ref in ast_refs {
        tokens.extend(tokenize(ast_ref));
        if !ast_ref.trim().is_empty() {
            tokens.push(ast_ref.to_lowercase());
        }
    }
    tokens.retain(|token| token.len() > 2 && !is_stop(token));
    tokens.sort();
    tokens.dedup();
    tokens
}

fn score_candidate(candidate: &AstCandidate, tokens: &[String], ast_refs: &[String]) -> u32 {
    let hay = format!(
        "{} {} {} {}",
        candidate.name,
        candidate.target.as_deref().unwrap_or(""),
        candidate.file_path.as_deref().unwrap_or(""),
        candidate.kind
    )
    .to_lowercase();
    let mut score = 0u32;
    for ast_ref in ast_refs {
        let needle = ast_ref.to_lowercase();
        if !needle.is_empty() && hay.contains(&needle) {
            score += 8;
        }
    }
    for token in tokens {
        if hay.contains(token) {
            score += 2;
        }
    }
    if score > 0 && candidate.ref_count > 0 {
        score += candidate.ref_count.min(5) as u32;
    }
    score
}

fn snippet_from_candidate(candidate: &AstCandidate, include_body: bool) -> CodeSnippet {
    let file = candidate.file_path.clone().unwrap_or_default();
    let body = if include_body {
        read_snippet(
            Path::new(&candidate.repo_path),
            &file,
            candidate.line.unwrap_or(1),
            SNIPPET_RADIUS,
            MAX_SNIPPET_CHARS,
        )
        .unwrap_or_default()
    } else {
        String::new()
    };
    CodeSnippet {
        project_id: candidate.project_id.clone(),
        file,
        line: candidate.line,
        symbol: candidate
            .target
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| candidate.name.clone()),
        kind: if candidate.kind.is_empty() {
            "node".into()
        } else {
            candidate.kind.clone()
        },
        body,
    }
}

fn read_snippet(
    repo: &Path,
    file: &str,
    line: i64,
    radius: usize,
    max_chars: usize,
) -> Option<String> {
    if file.is_empty() {
        return None;
    }
    let path = {
        let given = PathBuf::from(file);
        if given.is_absolute() {
            given
        } else {
            repo.join(file)
        }
    };
    let source = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let focus = (line.max(1) as usize)
        .saturating_sub(1)
        .min(lines.len() - 1);
    let start = focus.saturating_sub(radius);
    let end = (focus + radius + 1).min(lines.len());
    let mut body = lines[start..end].join("\n");
    if body.len() > max_chars {
        body.truncate(max_chars);
        body.push('…');
    }
    Some(body)
}

fn is_stop(token: &str) -> bool {
    matches!(
        token,
        "the"
            | "and"
            | "for"
            | "with"
            | "from"
            | "this"
            | "that"
            | "bir"
            | "ile"
            | "icin"
            | "için"
            | "olan"
            | "nedir"
            | "index"
            | "code"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AstNode, ExperienceOutcome, ExperienceRecord, IndexGraph, LoungeTask, AGENT_PROMPT,
        TASK_REQUESTED,
    };
    use std::fs;
    use std::time::Instant;

    #[test]
    fn knowledge_hit_gates_retrieve() {
        assert!(!knowledge_hit_triggers(
            0.1,
            TASK_REQUESTED,
            "dispatcher nats"
        ));
        assert!(knowledge_hit_triggers(
            0.8,
            TASK_REQUESTED,
            "dispatcher nats"
        ));
        assert!(!knowledge_hit_triggers(
            0.8,
            AGENT_PROMPT,
            "dispatcher nats"
        ));
        assert!(!knowledge_hit_triggers(0.8, TASK_REQUESTED, "  "));
    }

    #[tokio::test]
    async fn fast_retrieve_cross_project_and_ast_snippets() {
        let root = std::env::temp_dir().join(format!(
            "lounge-fast-retrieve-{}",
            Instant::now().elapsed().as_nanos()
        ));
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("dispatcher.rs"),
            "fn ignore() {}\nfn listen_once() {\n    nats::connect(\"nats://127.0.0.1:4222\");\n}\nfn other() {}\n",
        )
        .unwrap();

        let store = ExperienceStore::memory().unwrap();
        let other = LoungeTask::new("claude", "other-os", "NATS dispatcher dinleyici");
        store
            .insert_record(ExperienceRecord::from_task(
                &other,
                "spawn_blocking + mpsc ile NATS listen",
                "sync nats client blocking thread",
                ExperienceOutcome::Success,
                vec![],
            ))
            .await
            .unwrap();

        store
            .save_project_index(IndexGraph {
                project: "agent-lounge-os".into(),
                repo_path: root.to_string_lossy().into_owned(),
                nodes: vec![
                    AstNode {
                        id: "listen_once".into(),
                        name: "listen_once".into(),
                        kind: "fn".into(),
                        file: Some("src/dispatcher.rs".into()),
                        line: Some(2),
                        ref_count: 4,
                    },
                    AstNode {
                        id: "unrelated".into(),
                        name: "quota_pump".into(),
                        kind: "fn".into(),
                        file: Some("src/quota.rs".into()),
                        line: Some(1),
                        ref_count: 1,
                    },
                ],
                ..IndexGraph::default()
            })
            .await
            .unwrap();

        let context = store
            .fast_retrieve(FastRetrieveQuery {
                project_id: "agent-lounge-os".into(),
                text: "dispatcher NATS mesajlarını dinle listen_once".into(),
                embedding: None,
                knowledge_hit: 0.86,
                ast_refs: vec!["listen_once".into()],
                limit: Some(3),
            })
            .await
            .unwrap();

        assert!(
            !context.experiences.is_empty(),
            "çapraz proje tecrübesi gelmeli"
        );
        assert_eq!(context.experiences[0].project_id, "other-os");
        assert!(context
            .snippets
            .iter()
            .any(|row| row.symbol.contains("listen_once")));
        assert!(
            context.prompt_block().contains("Cross-Project Memory"),
            "system prompt eklentisi"
        );
        assert!(
            !context.prompt_block().contains("quota_pump"),
            "ilgisiz AST düğümü token yememelidir"
        );
        assert!(context.prompt_block().len() <= MAX_PROMPT_CHARS);
        let _ = fs::remove_dir_all(root);
    }
}
