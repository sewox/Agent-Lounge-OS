//! A2A Atomic Core — `agent_sessions`, `idempotency_keys`, `a2a_tasks` + atomik kabul.
//!
//! ## Kolon ↔ struct eşlemesi
//! | SQLite `a2a_tasks` | `LoungeTask` alanı |
//! |---|---|
//! | `id` | `id` |
//! | `root_id` | `root_id` (kanonik; JSON alias `root_task_id`) |
//! | `parent_id` | `parent_task_id` (kanonik; JSON alias `parent_id`) |
//! | `session_id` | `session_id` (kanonik; JSON alias `source_session_id`) |
//! | `hop_count` | `hop_count` |
//! | `idempotency_key` | `idempotency_key` (kapsam: session_id veya source_agent + project_id) |
//! | `claimed_by` | yield lease sahibi MCP oturumu (`target_agent` bağlı) |
//! | `result_json` | `lounge_yield_result` çıktısı |
//!
//! ## Yield kuralları (PR-3 review)
//! - Yalnız `target_agent` ile `agent_sessions` üzerinden bağlanmış oturum
//!   (veya zaten `claimed_by` sahibi) yield edebilir.
//! - Durum: `QUEUED` (PENDING) / `DISPATCHED` / `EXECUTING` / `WAIT_TIMEOUT_REACHED`.
//! - Terminal (`Completed`/`Failed`/`Cancelled`/`Expired`/`Timeout`) → ret.
//!   `NeedsHuman` soft: cancel izinli; wait `needs_human` bayrağı döner.
//!
//! Görev satırı ve idempotency kaydı **tek SQLite transaction** içinde yazılır.
//! Idempotency kapsamı (PR-3): `session:<mcp_session_id>` varsa spoof edilebilir
//! `source_agent` yerine oturum kimliği; yoksa legacy `agent:<source_agent>`.
//!
//! ## `PRAGMA foreign_keys=ON`
//! SQLite’da foreign key zorlaması **bağlantı (connection) düzeyinde**dir; process-global
//! değildir. `migrate_a2a` bu bağlantıda `PRAGMA foreign_keys=ON` çalıştırır; aynı
//! `ExperienceStore` / paylaşılan `Connection` üzerindeki sonraki işlemler FK’yi görür.
//! Yeni bir `Connection::open` ile açılan bağlantıda varsayılan **OFF** kalır — migrate
//! veya açık `PRAGMA` gerekir. `session_lock.session_id → agent_sessions(id)` FK’si
//! yalnızca bu pragma açıkken geçerlidir.
//!
//! ## `session_lock` / `acquire_session_lock`
//! MCP oturum kimliğiyle bağlandı (PR-3): wait/yield sahiplik kontrolleri
//! `session_id` / `claimed_by` üzerinden yapılır.
//!
//! Şema sürümü: settings `a2a.schema_version`.

use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

use crate::models::{now_rfc3339, AgentSession, LoungeTask, TaskStatus, DEFAULT_MAX_HOPS};

/// Mutlak hop tavanı — env / `with_max_hops` bunu aşamaz.
pub const ABSOLUTE_MAX_HOPS: u32 = 64;

/// Settings anahtarı — `migrate_a2a` sürümü.
pub const A2A_SCHEMA_VERSION_KEY: &str = "a2a.schema_version";
/// v3: session-scoped idempotency + result_json + claimed_by.
/// v4: orphan TTL — last_wait_at / backgrounded_at / result_ready_at.
pub const A2A_SCHEMA_VERSION: &str = "4";

/// Varsayılan ajan sessizlik süresi — EXECUTING/DISPATCHED zombi → NEEDS_HUMAN.
pub const DEFAULT_AGENT_SILENCE: Duration = Duration::from_secs(120);

/// Sessizlik tarayıcı aralığı varsayılanı.
pub const DEFAULT_SILENCE_SCAN_INTERVAL: Duration = Duration::from_secs(15);

/// Backgrounded sonuç hazır ama hiç sorgulanmadı → EXPIRED (çalışanı öldürmez).
pub const DEFAULT_RESULT_ORPHAN_TTL: Duration = Duration::from_secs(30 * 60);
/// Sahipsiz tamamlanmamış görev → EXPIRED + control.stop.
pub const DEFAULT_INCOMPLETE_ORPHAN_TTL: Duration = Duration::from_secs(30 * 60);
/// must_deliver mutlak üst sınır — alınmayan sonuç → FAILED(abandoned).
pub const DEFAULT_MUST_DELIVER_TTL: Duration = Duration::from_secs(24 * 3600);
/// must_deliver kota: oturum başına eşzamanlı.
pub const MUST_DELIVER_MAX_PER_SESSION: u64 = 5;
/// must_deliver kota: kaynak ajan başına açık toplam.
pub const MUST_DELIVER_MAX_PER_AGENT: u64 = 10;

/// Settings / env: sonuç orphan TTL (sn).
#[allow(dead_code)]
pub const SETTING_RESULT_ORPHAN_TTL_SECS: &str = "mcp.result_orphan_ttl_secs";
pub const ENV_RESULT_ORPHAN_TTL_SECS: &str = "LOUNGE_RESULT_ORPHAN_TTL_SECS";
/// Settings / env: incomplete orphan TTL (sn).
#[allow(dead_code)]
pub const SETTING_INCOMPLETE_ORPHAN_TTL_SECS: &str = "mcp.incomplete_orphan_ttl_secs";
pub const ENV_INCOMPLETE_ORPHAN_TTL_SECS: &str = "LOUNGE_INCOMPLETE_ORPHAN_TTL_SECS";
pub const ENV_MUST_DELIVER_TTL_SECS: &str = "LOUNGE_MUST_DELIVER_TTL_SECS";

/// Idempotency anahtar TTL (GC) — silence watchdog periyodunda `gc_idempotency_keys`.
pub const DEFAULT_IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 3600);

/// poll_after_secs başlangıç / çarpan / tavan (long_running / backgrounded).
pub const POLL_AFTER_SECS_START: u64 = 15;
pub const POLL_AFTER_SECS_MAX: u64 = 60;
pub const POLL_AFTER_GROWTH: f64 = 1.5;

/// `LOUNGE_MAX_HOPS` yoksa veya geçersizse [`DEFAULT_MAX_HOPS`]; üst tavan [`ABSOLUTE_MAX_HOPS`].
pub fn configured_max_hops() -> u32 {
    let raw = std::env::var("LOUNGE_MAX_HOPS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_HOPS);
    raw.min(ABSOLUTE_MAX_HOPS)
}

/// `LOUNGE_AGENT_SILENCE_SECS` — zombi eşiği.
pub fn configured_agent_silence() -> Duration {
    std::env::var("LOUNGE_AGENT_SILENCE_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_AGENT_SILENCE)
}

/// `LOUNGE_SILENCE_SCAN_SECS` — periyodik tarama aralığı.
pub fn configured_silence_scan_interval() -> Duration {
    std::env::var("LOUNGE_SILENCE_SCAN_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_SILENCE_SCAN_INTERVAL)
}

/// Backgrounded + sonuç hazır, hiç `lounge_wait_task` yok → EXPIRED.
pub fn configured_result_orphan_ttl() -> Duration {
    std::env::var(ENV_RESULT_ORPHAN_TTL_SECS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RESULT_ORPHAN_TTL)
}

/// Sahipsiz tamamlanmamış → EXPIRED + control.stop.
pub fn configured_incomplete_orphan_ttl() -> Duration {
    std::env::var(ENV_INCOMPLETE_ORPHAN_TTL_SECS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INCOMPLETE_ORPHAN_TTL)
}

/// must_deliver alınmayan sonuç → FAILED(abandoned) üst sınırı.
pub fn configured_must_deliver_ttl() -> Duration {
    std::env::var(ENV_MUST_DELIVER_TTL_SECS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_MUST_DELIVER_TTL)
}

/// Boş wait sonrası bir sonraki poll_after_secs (15 → ×1.5 → … ≤60).
pub fn next_poll_after_secs(empty_wait_count: u32) -> u64 {
    if empty_wait_count == 0 {
        return POLL_AFTER_SECS_START;
    }
    let mut v = POLL_AFTER_SECS_START as f64;
    for _ in 0..empty_wait_count {
        v *= POLL_AFTER_GROWTH;
    }
    v.floor().min(POLL_AFTER_SECS_MAX as f64) as u64
}

/// 32 bayt rastgele → hex token (yalnız bir kez istemciye; DB'de hash).
pub fn generate_task_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom_fill(&mut bytes);
    hex_encode(&bytes)
}

pub fn hash_task_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex_encode(&hasher.finalize())
}

/// Sabit zamanlı hash karşılaştırması (token loglanmaz).
pub fn task_token_matches(stored_hash: &str, presented: &str) -> bool {
    let presented_hash = hash_task_token(presented);
    ct_eq_hex(stored_hash.as_bytes(), presented_hash.as_bytes())
}

fn ct_eq_hex(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn getrandom_fill(buf: &mut [u8]) {
    // uuid crate zaten getrandom kullanır; Uuid baytlarından doldur.
    let mut i = 0;
    while i < buf.len() {
        let u = uuid::Uuid::new_v4();
        let b = u.as_bytes();
        let n = (buf.len() - i).min(b.len());
        buf[i..i + n].copy_from_slice(&b[..n]);
        i += n;
    }
}

/// must_deliver kota kontrolü — aşımda Err (çağıran JSON-RPC -32029 üretir).
pub fn check_must_deliver_quota(
    conn: &Connection,
    session_id: &str,
    source_agent: &str,
) -> Result<()> {
    let session_open: i64 = conn.query_row(
        r#"
        SELECT COUNT(*) FROM a2a_tasks
        WHERE must_deliver = 1
          AND session_id = ?1
          AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'EXPIRED', 'TIMEOUT')
        "#,
        params![session_id],
        |row| row.get(0),
    )?;
    if session_open as u64 >= MUST_DELIVER_MAX_PER_SESSION {
        anyhow::bail!(
            "MCP_RPC:-32029:must_deliver kota (oturum): {}/{} açık — yeni must_deliver reddedildi",
            session_open,
            MUST_DELIVER_MAX_PER_SESSION
        );
    }
    let agent_open: i64 = conn.query_row(
        r#"
        SELECT COUNT(*) FROM a2a_tasks
        WHERE must_deliver = 1
          AND source_agent = ?1
          AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'EXPIRED', 'TIMEOUT')
        "#,
        params![source_agent],
        |row| row.get(0),
    )?;
    if agent_open as u64 >= MUST_DELIVER_MAX_PER_AGENT {
        anyhow::bail!(
            "MCP_RPC:-32029:must_deliver kota (ajan): {}/{} açık — yeni must_deliver reddedildi",
            agent_open,
            MUST_DELIVER_MAX_PER_AGENT
        );
    }
    Ok(())
}

pub fn migrate_a2a(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_sessions (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            app_kind TEXT NOT NULL,
            native_id TEXT,
            native_ref TEXT,
            workspace_path TEXT NOT NULL,
            is_primary INTEGER NOT NULL DEFAULT 1,
            state TEXT NOT NULL DEFAULT 'unknown',
            owner TEXT NOT NULL DEFAULT 'lounge',
            created_by TEXT NOT NULL,
            last_seen TEXT NOT NULL,
            replaced_by TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_project_agent
            ON agent_sessions(project_id, agent_id);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_workspace
            ON agent_sessions(workspace_path);

        CREATE TABLE IF NOT EXISTS session_lock (
            session_id TEXT PRIMARY KEY,
            holder TEXT NOT NULL,
            locked_at TEXT NOT NULL,
            FOREIGN KEY(session_id) REFERENCES agent_sessions(id)
        );

        CREATE TABLE IF NOT EXISTS a2a_tasks (
            id TEXT PRIMARY KEY,
            root_id TEXT NOT NULL,
            parent_id TEXT,
            project_id TEXT NOT NULL,
            source_agent TEXT NOT NULL,
            target_agent TEXT,
            status TEXT NOT NULL DEFAULT 'QUEUED',
            hop_count INTEGER NOT NULL DEFAULT 0,
            idempotency_key TEXT,
            session_id TEXT,
            source_verified INTEGER NOT NULL DEFAULT 0,
            summary TEXT NOT NULL DEFAULT '',
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_root ON a2a_tasks(root_id);
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_parent ON a2a_tasks(parent_id);
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_status ON a2a_tasks(status);
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_updated ON a2a_tasks(updated_at);
        "#,
    )
    .context("a2a schema migrate base")?;

    migrate_idempotency_table(conn)?;
    migrate_a2a_task_columns(conn)?;

    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![A2A_SCHEMA_VERSION_KEY, A2A_SCHEMA_VERSION],
    )?;
    Ok(())
}

/// Idempotency kapsam anahtarı — oturum varsa spoof’a kapalı.
pub fn idempotency_scope(task: &LoungeTask) -> String {
    match task
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(sid) => format!("session:{sid}"),
        None => format!("agent:{}", task.source_agent),
    }
}

fn migrate_a2a_task_columns(conn: &Connection) -> Result<()> {
    let cols = column_names_fallback(conn, "a2a_tasks")?;
    if !cols.iter().any(|c| c == "result_json") {
        conn.execute("ALTER TABLE a2a_tasks ADD COLUMN result_json TEXT", [])?;
    }
    if !cols.iter().any(|c| c == "claimed_by") {
        conn.execute("ALTER TABLE a2a_tasks ADD COLUMN claimed_by TEXT", [])?;
    }
    if !cols.iter().any(|c| c == "last_wait_at") {
        conn.execute("ALTER TABLE a2a_tasks ADD COLUMN last_wait_at TEXT", [])?;
    }
    if !cols.iter().any(|c| c == "backgrounded_at") {
        conn.execute("ALTER TABLE a2a_tasks ADD COLUMN backgrounded_at TEXT", [])?;
    }
    if !cols.iter().any(|c| c == "result_ready_at") {
        conn.execute("ALTER TABLE a2a_tasks ADD COLUMN result_ready_at TEXT", [])?;
    }
    if !cols.iter().any(|c| c == "task_token_hash") {
        conn.execute("ALTER TABLE a2a_tasks ADD COLUMN task_token_hash TEXT", [])?;
    }
    if !cols.iter().any(|c| c == "must_deliver") {
        conn.execute(
            "ALTER TABLE a2a_tasks ADD COLUMN must_deliver INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !cols.iter().any(|c| c == "long_running") {
        conn.execute(
            "ALTER TABLE a2a_tasks ADD COLUMN long_running INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !cols.iter().any(|c| c == "empty_wait_count") {
        conn.execute(
            "ALTER TABLE a2a_tasks ADD COLUMN empty_wait_count INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn migrate_idempotency_table(conn: &Connection) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='idempotency_keys'",
        [],
        |row| row.get::<_, i64>(0).map(|n| n > 0),
    )?;
    if !exists {
        conn.execute_batch(
            r#"
            CREATE TABLE idempotency_keys (
                scope TEXT NOT NULL,
                project_id TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                task_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (scope, project_id, idempotency_key)
            );
            CREATE INDEX IF NOT EXISTS idx_idempotency_created
                ON idempotency_keys(created_at);
            "#,
        )?;
        return Ok(());
    }

    let cols = column_names_fallback(conn, "idempotency_keys")?;
    let has_scope_pk = cols.iter().any(|c| c == "scope")
        && cols.iter().any(|c| c == "project_id")
        && cols.iter().any(|c| c == "idempotency_key");
    if has_scope_pk {
        return Ok(());
    }

    // v2 (source_agent+project+key) veya daha eski → scope kolonuna taşı.
    let tx = conn
        .unchecked_transaction()
        .context("idempotency rebuild txn")?;
    tx.execute_batch(
        r#"
        ALTER TABLE idempotency_keys RENAME TO idempotency_keys_legacy;
        CREATE TABLE idempotency_keys (
            scope TEXT NOT NULL,
            project_id TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            task_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (scope, project_id, idempotency_key)
        );
        CREATE INDEX IF NOT EXISTS idx_idempotency_created
            ON idempotency_keys(created_at);
        "#,
    )
    .context("idempotency rebuild create")?;

    let legacy_cols = column_names_fallback(&tx, "idempotency_keys_legacy")?;
    let legacy_has_agent = legacy_cols.iter().any(|c| c == "source_agent")
        && legacy_cols.iter().any(|c| c == "project_id");
    if legacy_has_agent {
        tx.execute_batch(
            r#"
            INSERT OR IGNORE INTO idempotency_keys
                (scope, project_id, idempotency_key, task_id, created_at)
            SELECT
                'agent:' || COALESCE(NULLIF(source_agent, ''), 'unknown'),
                COALESCE(NULLIF(project_id, ''), 'unknown'),
                idempotency_key,
                task_id,
                created_at
            FROM idempotency_keys_legacy;
            "#,
        )
        .context("idempotency rebuild copy scoped")?;
    } else if legacy_cols.iter().any(|c| c == "scope") {
        tx.execute_batch(
            r#"
            INSERT OR IGNORE INTO idempotency_keys
                (scope, project_id, idempotency_key, task_id, created_at)
            SELECT scope, project_id, idempotency_key, task_id, created_at
            FROM idempotency_keys_legacy;
            "#,
        )
        .context("idempotency rebuild copy scope")?;
    } else {
        tx.execute_batch(
            r#"
            INSERT OR IGNORE INTO idempotency_keys
                (scope, project_id, idempotency_key, task_id, created_at)
            SELECT
                'agent:unknown',
                'unknown',
                idempotency_key,
                task_id,
                created_at
            FROM idempotency_keys_legacy;
            "#,
        )
        .context("idempotency rebuild copy legacy")?;
    }
    tx.execute_batch("DROP TABLE idempotency_keys_legacy;")?;
    tx.commit().context("idempotency rebuild commit")?;
    Ok(())
}

fn column_names_fallback(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum AdmitOutcome {
    Accepted(LoungeTask),
    /// Aynı (source, project, key) — orijinal görev; hata değil.
    Replay {
        existing_task_id: String,
        status: TaskStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitError {
    HopLimitExceeded { hop_count: u32, max_hops: u32 },
    ParentMissing { parent_id: String },
    ParentInvalid { parent_id: String, reason: String },
    IdConflict { task_id: String },
    Storage { message: String },
}

impl std::fmt::Display for AdmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HopLimitExceeded {
                hop_count,
                max_hops,
            } => write!(
                f,
                "hop limiti aşıldı: hop_count={hop_count} max_hops={max_hops}"
            ),
            Self::ParentMissing { parent_id } => {
                write!(
                    f,
                    "parent_id zinciri kırık: parent bulunamadı ({parent_id})"
                )
            }
            Self::ParentInvalid { parent_id, reason } => {
                write!(f, "parent_id geçersiz ({parent_id}): {reason}")
            }
            Self::IdConflict { task_id } => {
                write!(f, "a2a_tasks id çakışması: {task_id}")
            }
            Self::Storage { message } => write!(f, "a2a storage hatası: {message}"),
        }
    }
}

impl std::error::Error for AdmitError {}

fn storage_err(err: impl ToString) -> AdmitError {
    AdmitError::Storage {
        message: err.to_string(),
    }
}

/// Sunucu tarafı parent/hop/root normalizasyonu (istemci hop_count / root_id yok sayılır).
pub fn normalize_lineage(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
) -> Result<(), AdmitError> {
    let max_hops = max_hops.clamp(1, ABSOLUTE_MAX_HOPS);
    let claimed_parent = task.parent_task_id.clone();
    match claimed_parent {
        Some(parent_id) => {
            let parent = match load_task_row(conn, &parent_id) {
                Ok(Some(parent)) => parent,
                Ok(None) => {
                    // Uçuştaki (henüz a2a_tasks'ta olmayan) parent — net red.
                    // Belge: parent admit edilmeden çocuk kabul edilmez (fail-closed).
                    return Err(AdmitError::ParentMissing { parent_id });
                }
                Err(err) => {
                    log::error!("parent load failed ({parent_id}): {err}");
                    return Err(storage_err(err));
                }
            };
            if parent.project_id != task.project_id {
                return Err(AdmitError::ParentInvalid {
                    parent_id: parent_id.clone(),
                    reason: format!(
                        "project_id uyuşmazlığı parent={} child={}",
                        parent.project_id, task.project_id
                    ),
                });
            }
            if matches!(
                parent.status,
                TaskStatus::Failed
                    | TaskStatus::Expired
                    | TaskStatus::NeedsHuman
                    | TaskStatus::Timeout
                    | TaskStatus::Cancelled
            ) {
                return Err(AdmitError::ParentInvalid {
                    parent_id: parent_id.clone(),
                    reason: format!("terminal durum: {}", parent.status.as_str()),
                });
            }
            // source_agent: çocuk, ebeveynin hedefi veya aynı proje ajanı olmalı — gevşek:
            // parent.source_agent veya parent.target_agent ile ilişkisiz ise yine de hop zinciri
            // server-side; yalnızca proje + durum sert kapı. Oturum bağı: MCP admit.
            task.parent_task_id = Some(parent.id.clone());
            task.root_id = Some(parent.effective_root_id().to_string());
            task.hop_count = parent.hop_count.saturating_add(1);
        }
        None => {
            task.parent_task_id = None;
            // Client root_id'ye güvenme — rootsuz görevde root_id = id.
            task.root_id = Some(task.id.clone());
            task.hop_count = 0;
        }
    }

    if task.hop_count >= max_hops {
        return Err(AdmitError::HopLimitExceeded {
            hop_count: task.hop_count,
            max_hops,
        });
    }
    Ok(())
}

/// Görev + idempotency kaydını tek transaction'da yazar.
pub fn admit_task_atomic(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
) -> Result<AdmitOutcome, AdmitError> {
    admit_task_atomic_inner(conn, task, max_hops, /*skip_lookup*/ false)
}

/// Test / yarış: optimistic lookup atlanır — UNIQUE/PK hatası txn rollback'ini kanıtlar.
#[cfg(test)]
pub fn admit_task_atomic_skip_lookup(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
) -> Result<AdmitOutcome, AdmitError> {
    admit_task_atomic_inner(conn, task, max_hops, true)
}

fn admit_task_atomic_inner(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
    #[cfg_attr(not(test), allow(unused_variables))] skip_lookup: bool,
) -> Result<AdmitOutcome, AdmitError> {
    normalize_lineage(conn, task, max_hops)?;

    let key = task
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string);

    if let Some(ref k) = key {
        #[cfg(test)]
        let do_lookup = !skip_lookup;
        #[cfg(not(test))]
        let do_lookup = true;
        let scope = idempotency_scope(task);
        if do_lookup {
            match lookup_idempotency(conn, &scope, &task.project_id, k) {
                Ok(Some(existing)) => {
                    let status = task_status(conn, &existing)
                        .map_err(storage_err)?
                        .unwrap_or(TaskStatus::Queued);
                    return Ok(AdmitOutcome::Replay {
                        existing_task_id: existing,
                        status,
                    });
                }
                Ok(None) => {}
                Err(err) => return Err(storage_err(err)),
            }
        }
    }

    let tx = conn.unchecked_transaction().map_err(storage_err)?;

    if let Err(err) = insert_task_row(&tx, task) {
        let _ = tx.rollback();
        if is_unique_violation(&err) {
            return Err(AdmitError::IdConflict {
                task_id: task.id.clone(),
            });
        }
        return Err(storage_err(err));
    }

    if let Some(ref k) = key {
        if let Err(err) = insert_idempotency_row(&tx, k, task) {
            let _ = tx.rollback();
            if is_unique_violation(&err) {
                // Gerçek UNIQUE — task satırı geri alındı; replay veya yarış.
                let scope = idempotency_scope(task);
                let existing = lookup_idempotency(conn, &scope, &task.project_id, k)
                    .ok()
                    .flatten();
                if let Some(existing) = existing {
                    let status = task_status(conn, &existing)
                        .unwrap_or(None)
                        .unwrap_or(TaskStatus::Queued);
                    return Ok(AdmitOutcome::Replay {
                        existing_task_id: existing,
                        status,
                    });
                }
                return Err(AdmitError::Storage {
                    message: format!("idempotency UNIQUE ihlali ama mevcut satır okunamadı: {err}"),
                });
            }
            return Err(storage_err(err));
        }
    }

    tx.commit().map_err(storage_err)?;
    Ok(AdmitOutcome::Accepted(task.clone()))
}

fn is_unique_violation(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(code, _) => {
            code.code == rusqlite::ErrorCode::ConstraintViolation
        }
        _ => false,
    }
}

fn insert_task_row(tx: &Transaction<'_>, task: &LoungeTask) -> rusqlite::Result<usize> {
    let now = now_rfc3339();
    let payload = serde_json::to_string(task)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let must_deliver = if task.must_deliver { 1 } else { 0 };
    let long_running = if task.long_running { 1 } else { 0 };
    tx.execute(
        r#"
        INSERT INTO a2a_tasks (
            id, root_id, parent_id, project_id, source_agent, target_agent,
            status, hop_count, idempotency_key, session_id, source_verified,
            summary, payload_json, created_at, updated_at,
            must_deliver, long_running, task_token_hash
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15,
            ?16, ?17, ?18
        )
        "#,
        params![
            task.id,
            task.effective_root_id(),
            task.parent_task_id,
            task.project_id,
            task.source_agent,
            task.target_agent,
            task.status.as_str(),
            task.hop_count as i64,
            task.idempotency_key,
            task.session_id,
            if task.source_verified { 1 } else { 0 },
            task.summary,
            payload,
            task.created_at,
            now,
            must_deliver,
            long_running,
            task.task_token_hash,
        ],
    )
}

fn insert_idempotency_row(
    tx: &Transaction<'_>,
    key: &str,
    task: &LoungeTask,
) -> rusqlite::Result<usize> {
    let scope = idempotency_scope(task);
    tx.execute(
        r#"
        INSERT INTO idempotency_keys
            (scope, project_id, idempotency_key, task_id, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![scope, task.project_id, key, task.id, now_rfc3339(),],
    )
}

pub fn lookup_idempotency(
    conn: &Connection,
    scope: &str,
    project_id: &str,
    key: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            r#"SELECT task_id FROM idempotency_keys
               WHERE scope = ?1 AND project_id = ?2 AND idempotency_key = ?3"#,
            params![scope, project_id, key],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

/// Reddet / zaman aşımı / FAILED — anahtar yanmasın (yeniden denenebilsin).
pub fn release_idempotency_for_task(conn: &Connection, task_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM idempotency_keys WHERE task_id = ?1",
        params![task_id],
    )?;
    Ok(())
}

pub fn gc_idempotency_keys(conn: &Connection, older_than: &str) -> Result<u64> {
    let n = conn.execute(
        "DELETE FROM idempotency_keys WHERE created_at < ?1",
        params![older_than],
    )?;
    Ok(n as u64)
}

/// Görev sonucu (yield) — JSON metin.
pub fn load_task_result(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT result_json FROM a2a_tasks WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

pub fn load_task_session_id(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT session_id FROM a2a_tasks WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

pub fn load_claimed_by(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT claimed_by FROM a2a_tasks WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// İlk claim kazanır; aynı oturum yeniden claim edebilir.
pub fn claim_task(conn: &Connection, task_id: &str, session_id: &str) -> Result<bool> {
    let now = now_rfc3339();
    let current: Option<String> = conn
        .query_row(
            "SELECT claimed_by FROM a2a_tasks WHERE id = ?1",
            params![task_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    match current {
        None => {
            let n = conn.execute(
                "UPDATE a2a_tasks SET claimed_by = ?1, updated_at = ?2 WHERE id = ?3 AND claimed_by IS NULL",
                params![session_id, now, task_id],
            )?;
            Ok(n > 0)
        }
        Some(ref owner) if owner == session_id => Ok(true),
        Some(_) => Ok(false),
    }
}

/// Yield edilebilir ara durumlar (PENDING≈QUEUED).
pub fn is_yieldable_status(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Queued
            | TaskStatus::Dispatched
            | TaskStatus::Executing
            | TaskStatus::WaitTimeoutReached
    )
}

/// Oturum, görevin `target_agent`'ına `agent_sessions` ile bağlı mı?
pub fn session_bound_to_target(
    conn: &Connection,
    session_id: &str,
    target_agent: Option<&str>,
) -> Result<bool> {
    let Some(target) = target_agent.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    let found: Option<i64> = conn
        .query_row(
            r#"
            SELECT 1 FROM agent_sessions
            WHERE id = ?1 AND lower(agent_id) = lower(?2)
            LIMIT 1
            "#,
            params![session_id, target],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Yield sonucu yaz + status Completed/Failed.
///
/// Yetki: `claimed_by == session` **veya** (boş claim + `target_agent` oturum bağları).
/// Durum: yalnız yieldable; terminal satırlar ezilemez.
pub fn yield_task_result(
    conn: &Connection,
    task_id: &str,
    session_id: &str,
    status: TaskStatus,
    result_json: &str,
) -> Result<()> {
    if !matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
        anyhow::bail!("yield status completed|failed olmalı");
    }
    let task = load_task_row(conn, task_id)?
        .ok_or_else(|| anyhow::anyhow!("yield: görev bulunamadı: {task_id}"))?;
    let current = task_status(conn, task_id)?.unwrap_or(task.status.clone());
    if current.is_terminal() || matches!(current, TaskStatus::Cancelled) {
        anyhow::bail!(
            "yield reddedildi: görev terminal durumda ({})",
            current.as_str()
        );
    }
    if !is_yieldable_status(current.clone()) {
        anyhow::bail!(
            "yield reddedildi: durum {} yield edilemez (QUEUED|DISPATCHED|EXECUTING|WAIT_TIMEOUT_REACHED)",
            current.as_str()
        );
    }

    let claimed = load_claimed_by(conn, task_id)?;
    match claimed {
        Some(ref owner) if owner == session_id => {}
        Some(_) => anyhow::bail!("yetkisiz oturum: görev başka oturum tarafından claim edilmiş"),
        None => {
            if !session_bound_to_target(conn, session_id, task.target_agent.as_deref())? {
                anyhow::bail!(
                    "yetkisiz oturum: yield yalnız target_agent'a bağlı oturumdan kabul edilir"
                );
            }
            if !claim_task(conn, task_id, session_id)? {
                anyhow::bail!("yetkisiz oturum: claim alınamadı");
            }
        }
    }

    let now = now_rfc3339();
    let n = conn.execute(
        r#"
        UPDATE a2a_tasks
        SET result_json = ?1, status = ?2, updated_at = ?3, claimed_by = ?4,
            result_ready_at = COALESCE(result_ready_at, ?3)
        WHERE id = ?5
          AND status IN ('QUEUED', 'DISPATCHED', 'EXECUTING', 'WAIT_TIMEOUT_REACHED')
        "#,
        params![result_json, status.as_str(), now, session_id, task_id],
    )?;
    if n == 0 {
        anyhow::bail!("yield: görev bulunamadı veya durum yarışında terminal oldu: {task_id}");
    }
    // payload_json içindeki status'u da senkron tut.
    if let Ok(Some(mut updated)) = load_task_row(conn, task_id) {
        updated.status = status.clone();
        let _ = rewrite_payload(conn, &updated);
    }
    if matches!(status, TaskStatus::Failed) {
        let _ = release_idempotency_for_task(conn, task_id);
    }
    Ok(())
}

fn rewrite_payload(conn: &Connection, task: &LoungeTask) -> Result<()> {
    let payload = serde_json::to_string(task)?;
    conn.execute(
        "UPDATE a2a_tasks SET payload_json = ?1 WHERE id = ?2",
        params![payload, task.id],
    )?;
    Ok(())
}

/// Kaynak oturum wait/okuma yetkisi — aynı oturum VEYA geçerli task_token.
pub fn session_can_read_task(conn: &Connection, task_id: &str, session_id: &str) -> Result<bool> {
    session_or_token_can_read(conn, task_id, session_id, None)
}

pub fn session_or_token_can_read(
    conn: &Connection,
    task_id: &str,
    session_id: &str,
    task_token: Option<&str>,
) -> Result<bool> {
    let owner = load_task_session_id(conn, task_id)?;
    if let Some(ref sid) = owner {
        if sid == session_id {
            return Ok(true);
        }
    }
    // Yeniden bağlanma: düz token sunulursa hash sabit-zamanlı doğrulanır.
    if let Some(presented) = task_token.map(str::trim).filter(|s| !s.is_empty()) {
        let stored: Option<String> = conn
            .query_row(
                "SELECT task_token_hash FROM a2a_tasks WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(hash) = stored.filter(|h| !h.is_empty()) {
            return Ok(task_token_matches(&hash, presented));
        }
    }
    // Oturumsuz (legacy NATS) görevler — okuma açık değil (fail-closed PR-3).
    Ok(false)
}

/// Kullanıcı iptali.
pub fn cancel_task(conn: &Connection, task_id: &str, session_id: &str) -> Result<()> {
    if !session_can_read_task(conn, task_id, session_id)? {
        anyhow::bail!("yetkisiz oturum: iptal reddedildi");
    }
    let current = task_status(conn, task_id)?.unwrap_or(TaskStatus::Queued);
    if current.is_terminal() && !matches!(current, TaskStatus::NeedsHuman) {
        return Ok(());
    }
    update_task_status(conn, task_id, TaskStatus::Cancelled)?;
    let _ = release_idempotency_for_task(conn, task_id);
    Ok(())
}

#[cfg(test)]
mod _gc_smoke {
    #[test]
    fn gc_compiles_and_runs() {
        let store = crate::db::ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let n = super::gc_idempotency_keys(&conn, "2099-01-01T00:00:00.000Z").unwrap();
        assert_eq!(n, 0);
    }
}

pub fn load_task_row(conn: &Connection, id: &str) -> Result<Option<LoungeTask>> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM a2a_tasks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    match payload {
        Some(raw) => {
            let mut task: LoungeTask = serde_json::from_str(&raw)?;
            // Status kolonu kanonik — payload eski kalabilir.
            if let Some(status) = task_status(conn, id)? {
                task.status = status;
            }
            // source_verified kolonu da kanonik tut.
            let verified: Option<i64> = conn
                .query_row(
                    "SELECT source_verified FROM a2a_tasks WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(v) = verified {
                task.source_verified = v != 0;
            }
            Ok(Some(task))
        }
        None => Ok(None),
    }
}

pub fn task_status(conn: &Connection, id: &str) -> Result<Option<TaskStatus>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT status FROM a2a_tasks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw.map(|s| TaskStatus::parse(&s)))
}

pub(crate) fn update_task_status(conn: &Connection, id: &str, status: TaskStatus) -> Result<()> {
    let n = conn.execute(
        "UPDATE a2a_tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status.as_str(), now_rfc3339(), id],
    )?;
    if n == 0 {
        // Admit atlanmış olabilir — sessizce yok sayma; logla.
        log::debug!(
            "a2a_tasks status güncellemesi: satır yok id={id} → {}",
            status.as_str()
        );
        return Ok(());
    }
    if let Some(mut task) = load_task_row(conn, id)? {
        task.status = status;
        let payload = serde_json::to_string(&task)?;
        conn.execute(
            "UPDATE a2a_tasks SET payload_json = ?1 WHERE id = ?2",
            params![payload, id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn touch_task_updated_at(conn: &Connection, id: &str, updated_at: &str) -> Result<()> {
    conn.execute(
        "UPDATE a2a_tasks SET updated_at = ?1 WHERE id = ?2",
        params![updated_at, id],
    )?;
    Ok(())
}

/// Worker heartbeat — hedef ajana DISPATCHED görevlerin `updated_at` yenilemesi.
pub fn touch_dispatched_for_target(conn: &Connection, target_agent: &str) -> Result<u64> {
    let target = target_agent.trim().to_ascii_lowercase();
    let n = conn.execute(
        r#"
        UPDATE a2a_tasks
        SET updated_at = ?1
        WHERE status = 'DISPATCHED'
          AND lower(COALESCE(target_agent, '')) = ?2
        "#,
        params![now_rfc3339(), target],
    )?;
    Ok(n as u64)
}

/// EXECUTING / DISPATCHED / RECOVERY_PENDING / QUEUED / WAIT_TIMEOUT_REACHED sessizliği
/// → NEEDS_HUMAN. `PENDING_APPROVAL` bilinçli olarak dışarıda (kullanıcı onayı bekleniyor).
/// `WAIT_TIMEOUT_REACHED`: MCP backgrounded — worker ölürse sonsuz still_running olmasın.
pub fn mark_silent_tasks_needs_human(
    conn: &Connection,
    now_rfc3339: &str,
    silence: Duration,
) -> Result<Vec<String>> {
    let silence_secs = silence.as_secs() as i64;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, updated_at FROM a2a_tasks
        WHERE status IN ('EXECUTING', 'DISPATCHED', 'RECOVERY_PENDING', 'QUEUED', 'WAIT_TIMEOUT_REACHED')
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let now = match chrono::DateTime::parse_from_rfc3339(now_rfc3339) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(err) => {
            log::warn!("silence scan: now parse failed ({now_rfc3339}): {err}");
            return Ok(Vec::new());
        }
    };

    let mut marked = Vec::new();
    for row in rows {
        let (id, updated_at) = row?;
        let updated = match chrono::DateTime::parse_from_rfc3339(&updated_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(err) => {
                log::warn!("silence scan: updated_at parse failed id={id} raw={updated_at}: {err}");
                continue;
            }
        };
        let age = now.signed_duration_since(updated);
        if age.num_seconds() >= silence_secs {
            update_task_status(conn, &id, TaskStatus::NeedsHuman)?;
            marked.push(id);
        }
    }
    Ok(marked)
}

/// `lounge_wait_task` çağrıldığında dokunulan zaman damgası.
pub fn touch_last_wait(conn: &Connection, task_id: &str) -> Result<()> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE a2a_tasks SET last_wait_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![now, task_id],
    )?;
    Ok(())
}

/// MCP oturumu koptu — agent_sessions.state = disconnected.
pub fn mark_session_disconnected(conn: &Connection, session_id: &str) -> Result<()> {
    let now = now_rfc3339();
    conn.execute(
        r#"
        UPDATE agent_sessions
        SET state = 'disconnected', last_seen = ?1
        WHERE id = ?2
        "#,
        params![now, session_id],
    )?;
    Ok(())
}

fn parse_rfc3339_age_secs(now: &str, then: &str) -> Option<i64> {
    let now_dt = chrono::DateTime::parse_from_rfc3339(now)
        .ok()?
        .with_timezone(&chrono::Utc);
    let then_dt = chrono::DateTime::parse_from_rfc3339(then)
        .ok()?
        .with_timezone(&chrono::Utc);
    Some(now_dt.signed_duration_since(then_dt).num_seconds())
}

/// Sonuç orphan: backgrounded → tamamlandı, N dk hiç wait yok → EXPIRED.
/// Çalışan (sonuçsuz) görevleri öldürmez.
pub fn expire_result_orphans(
    conn: &Connection,
    now_rfc3339: &str,
    ttl: Duration,
) -> Result<Vec<String>> {
    let ttl_secs = ttl.as_secs() as i64;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, result_ready_at, last_wait_at, backgrounded_at, status
        FROM a2a_tasks
        WHERE backgrounded_at IS NOT NULL
          AND result_ready_at IS NOT NULL
          AND result_json IS NOT NULL
          AND status IN ('COMPLETED', 'FAILED')
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut expired = Vec::new();
    for row in rows {
        let (id, ready_at, last_wait, _bg, _st) = row?;
        let Some(ready) = ready_at else { continue };
        // Wait sonucu hazır olduktan sonra geldiyse orphan değil.
        if let Some(ref lw) = last_wait {
            if let (Some(ready_age), Some(wait_age)) = (
                parse_rfc3339_age_secs(now_rfc3339, &ready),
                parse_rfc3339_age_secs(now_rfc3339, lw),
            ) {
                // last_wait daha yeni (küçük age) → sorgulandı.
                if wait_age <= ready_age {
                    continue;
                }
            }
        }
        let Some(age) = parse_rfc3339_age_secs(now_rfc3339, &ready) else {
            continue;
        };
        if age >= ttl_secs {
            update_task_status(conn, &id, TaskStatus::Expired)?;
            expired.push(id);
        }
    }
    Ok(expired)
}

/// Incomplete orphan: çağıran oturum yok/disconnected + N dk sorgu yok → EXPIRED.
/// Dönüş: control.stop uygulanacak task_id listesi (henüz terminal olmayanlar).
pub fn expire_incomplete_orphans(
    conn: &Connection,
    now_rfc3339: &str,
    ttl: Duration,
) -> Result<Vec<(String, Option<String>)>> {
    let ttl_secs = ttl.as_secs() as i64;
    let mut stmt = conn.prepare(
        r#"
        SELECT t.id, t.session_id, t.updated_at, t.last_wait_at, t.created_at,
               s.state AS session_state
        FROM a2a_tasks t
        LEFT JOIN agent_sessions s ON s.id = t.session_id
        WHERE t.status IN (
            'QUEUED', 'DISPATCHED', 'EXECUTING', 'WAIT_TIMEOUT_REACHED',
            'RECOVERY_PENDING', 'PENDING_APPROVAL'
        )
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, session_id, updated_at, last_wait, created_at, session_state) = row?;
        let session_gone = match (session_id.as_deref(), session_state.as_deref()) {
            (None, _) => true,
            (Some(_), None) => true, // session satırı yok
            (Some(_), Some(st)) => {
                let lower = st.to_ascii_lowercase();
                lower == "disconnected" || lower == "stale"
            }
        };
        if !session_gone {
            continue;
        }
        let anchor = last_wait.as_deref().unwrap_or(updated_at.as_str());
        let anchor = if anchor.is_empty() {
            created_at.as_str()
        } else {
            anchor
        };
        let Some(age) = parse_rfc3339_age_secs(now_rfc3339, anchor) else {
            continue;
        };
        if age >= ttl_secs {
            update_task_status(conn, &id, TaskStatus::Expired)?;
            out.push((id, session_id));
        }
    }
    Ok(out)
}

/// must_deliver: hazır sonuç veya açık görev TTL aşımı → FAILED + reason abandoned.
pub fn abandon_must_deliver_orphans(
    conn: &Connection,
    now_rfc3339: &str,
    ttl: Duration,
) -> Result<Vec<String>> {
    let ttl_secs = ttl.as_secs() as i64;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, COALESCE(result_ready_at, created_at), status
        FROM a2a_tasks
        WHERE must_deliver = 1
          AND status NOT IN ('FAILED', 'CANCELLED', 'EXPIRED', 'TIMEOUT')
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut abandoned = Vec::new();
    for row in rows {
        let (id, anchor, status) = row?;
        // COMPLETED ama hiç alınmamış (last_wait yok veya result_ready'den eski) → abandon.
        // Açık görevler de TTL ile abandon.
        let Some(age) = parse_rfc3339_age_secs(now_rfc3339, &anchor) else {
            continue;
        };
        if age < ttl_secs {
            continue;
        }
        if status == "COMPLETED" {
            let last_wait: Option<String> = conn
                .query_row(
                    "SELECT last_wait_at FROM a2a_tasks WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if let Some(ref lw) = last_wait {
                if let Some(wait_age) = parse_rfc3339_age_secs(now_rfc3339, lw) {
                    if wait_age <= age {
                        continue; // alındı
                    }
                }
            }
        }
        let envelope = serde_json::json!({
            "reason": "abandoned",
            "message": "must_deliver TTL aşıldı — sonuç alınmadı",
            "abandoned_at": now_rfc3339,
        });
        let raw = serde_json::to_string(&envelope)?;
        conn.execute(
            r#"
            UPDATE a2a_tasks
            SET status = 'FAILED', result_json = COALESCE(result_json, ?1),
                updated_at = ?2, result_ready_at = COALESCE(result_ready_at, ?2)
            WHERE id = ?3
            "#,
            params![raw, now_rfc3339, id],
        )?;
        abandoned.push(id);
    }
    Ok(abandoned)
}

/// Hub / DB agent_sessions üst sınırı (MCP in-memory ile aynı).
pub const MAX_AGENT_SESSIONS: usize = 256;

/// PR-3 — MCP initialize üretim yolu da yazar; üst sınırda eski satırlar temizlenir.
pub fn upsert_agent_session(conn: &Connection, session: &AgentSession) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO agent_sessions (
            id, project_id, agent_id, app_kind, native_id, native_ref,
            workspace_path, is_primary, state, owner, created_by, last_seen, replaced_by
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        ON CONFLICT(id) DO UPDATE SET
            native_id = excluded.native_id,
            native_ref = excluded.native_ref,
            state = excluded.state,
            last_seen = excluded.last_seen,
            replaced_by = excluded.replaced_by,
            is_primary = excluded.is_primary
        "#,
        params![
            session.id,
            session.project_id,
            session.agent_id,
            session.app_kind,
            session.native_id,
            session.native_ref,
            session.workspace_path,
            if session.is_primary { 1 } else { 0 },
            session.state,
            session.owner,
            session.created_by,
            session.last_seen,
            session.replaced_by,
        ],
    )?;
    prune_agent_sessions(conn, MAX_AGENT_SESSIONS, &session.id)?;
    Ok(())
}

/// `last_seen` FIFO: en eski bağlantısız oturumları sil (canlı `keep` düşmesin).
pub fn prune_agent_sessions(conn: &Connection, max: usize, keep: &str) -> Result<u64> {
    let count = count_agent_sessions(conn)? as usize;
    if count <= max {
        return Ok(0);
    }
    let excess = (count - max) as i64;
    // FK: session_lock → agent_sessions; önce kilitleri temizle.
    // SQLite: aynı tabloya DELETE+subquery için iç içe SELECT gerekir.
    conn.execute(
        r#"
        DELETE FROM session_lock WHERE session_id IN (
            SELECT id FROM (
                SELECT id FROM agent_sessions
                WHERE id != ?1
                ORDER BY last_seen ASC
                LIMIT ?2
            )
        )
        "#,
        params![keep, excess],
    )?;
    let n = conn.execute(
        r#"
        DELETE FROM agent_sessions WHERE id IN (
            SELECT id FROM (
                SELECT id FROM agent_sessions
                WHERE id != ?1
                ORDER BY last_seen ASC
                LIMIT ?2
            )
        )
        "#,
        params![keep, excess],
    )?;
    Ok(n as u64)
}

/// PR-3 hazırlığı — üretim yoluna bağlı değil.
pub fn acquire_session_lock(conn: &Connection, session_id: &str, holder: &str) -> Result<bool> {
    let now = now_rfc3339();
    match conn.execute(
        "INSERT INTO session_lock (session_id, holder, locked_at) VALUES (?1, ?2, ?3)",
        params![session_id, holder, now],
    ) {
        Ok(_) => Ok(true),
        Err(err) if is_unique_violation(&err) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// PR-3 hazırlığı — üretim yoluna bağlı değil.
pub fn release_session_lock(conn: &Connection, session_id: &str, holder: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM session_lock WHERE session_id = ?1 AND holder = ?2",
        params![session_id, holder],
    )?;
    Ok(())
}

#[cfg(test)]
mod _session_lock_smoke {
    use super::*;
    use crate::db::ExperienceStore;

    #[test]
    fn session_lock_roundtrip() {
        let store = ExperienceStore::memory().unwrap();
        let s = AgentSession::new("p", "cursor", "gui", "/tmp/ws", "user_pin");
        {
            let conn = store.conn.lock().unwrap();
            upsert_agent_session(&conn, &s).unwrap();
            assert!(acquire_session_lock(&conn, &s.id, "holder-a").unwrap());
            assert!(!acquire_session_lock(&conn, &s.id, "holder-b").unwrap());
            release_session_lock(&conn, &s.id, "holder-a").unwrap();
            assert!(acquire_session_lock(&conn, &s.id, "holder-b").unwrap());
        }
    }
}

pub fn count_agent_sessions(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM agent_sessions", [], |row| row.get(0))?;
    Ok(n as u64)
}

#[cfg(test)]
fn count_a2a_tasks(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM a2a_tasks", [], |row| row.get(0))?;
    Ok(n as u64)
}

/// Oturum başına açık (non-terminal) görev sayısı.
pub fn count_open_tasks_for_session(conn: &Connection, session_id: &str) -> Result<u64> {
    let n: i64 = conn.query_row(
        r#"
        SELECT COUNT(*) FROM a2a_tasks
        WHERE session_id = ?1
          AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'EXPIRED', 'TIMEOUT', 'NEEDS_HUMAN')
        "#,
        params![session_id],
        |row| row.get(0),
    )?;
    Ok(n as u64)
}

#[cfg(test)]
fn count_idempotency_keys(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM idempotency_keys", [], |row| {
        row.get(0)
    })?;
    Ok(n as u64)
}

/// ExperienceStore üzerinden A2A işlemleri.
impl crate::db::ExperienceStore {
    pub fn admit_a2a_task(
        &self,
        task: &mut LoungeTask,
        max_hops: u32,
    ) -> Result<AdmitOutcome, AdmitError> {
        let conn = self.conn.lock().expect("experience db lock");
        admit_task_atomic(&conn, task, max_hops)
    }

    /// Workflow / lifecycle — NATS payload yerine DB kaydı.
    pub fn load_a2a_task(&self, id: &str) -> Result<Option<LoungeTask>> {
        let conn = self.conn.lock().expect("experience db lock");
        load_task_row(&conn, id)
    }

    pub fn a2a_task_status(&self, id: &str) -> Result<Option<TaskStatus>> {
        let conn = self.conn.lock().expect("experience db lock");
        task_status(&conn, id)
    }

    pub(crate) fn set_a2a_task_status(&self, id: &str, status: TaskStatus) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        update_task_status(&conn, id, status)
    }

    pub fn recover_silent_a2a_tasks(
        &self,
        now_rfc3339: &str,
        silence: Duration,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("experience db lock");
        mark_silent_tasks_needs_human(&conn, now_rfc3339, silence)
    }

    #[cfg(test)]
    pub(crate) fn touch_a2a_updated_at(&self, id: &str, updated_at: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        touch_task_updated_at(&conn, id, updated_at)
    }

    /// Worker heartbeat → DISPATCHED görevlerin `updated_at` yenilemesi.
    pub fn touch_dispatched_for_agent(&self, target_agent: &str) -> Result<u64> {
        let conn = self.conn.lock().expect("experience db lock");
        touch_dispatched_for_target(&conn, target_agent)
    }

    pub fn upsert_session(&self, session: &AgentSession) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        upsert_agent_session(&conn, session)
    }

    pub fn session_count(&self) -> Result<u64> {
        let conn = self.conn.lock().expect("experience db lock");
        count_agent_sessions(&conn)
    }

    pub fn count_open_a2a_tasks_for_session(&self, session_id: &str) -> Result<u64> {
        let conn = self.conn.lock().expect("experience db lock");
        count_open_tasks_for_session(&conn, session_id)
    }

    pub fn release_a2a_idempotency(&self, task_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        release_idempotency_for_task(&conn, task_id)
    }

    pub fn gc_a2a_idempotency(&self, older_than: &str) -> Result<u64> {
        let conn = self.conn.lock().expect("experience db lock");
        gc_idempotency_keys(&conn, older_than)
    }

    pub fn try_lock_session(&self, session_id: &str, holder: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("experience db lock");
        acquire_session_lock(&conn, session_id, holder)
    }

    pub fn unlock_session(&self, session_id: &str, holder: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        release_session_lock(&conn, session_id, holder)
    }

    pub fn a2a_task_result(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("experience db lock");
        load_task_result(&conn, id)
    }

    pub fn claim_a2a_task(&self, task_id: &str, session_id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("experience db lock");
        claim_task(&conn, task_id, session_id)
    }

    pub fn yield_a2a_result(
        &self,
        task_id: &str,
        session_id: &str,
        status: TaskStatus,
        result_json: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        yield_task_result(&conn, task_id, session_id, status, result_json)
    }

    /// NATS `lounge.task.completed|failed` — claim kontrolü yok; yalnız result_json yaz.
    pub fn store_a2a_bus_result(&self, task_id: &str, result_json: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        let now = now_rfc3339();
        let n = conn.execute(
            r#"
            UPDATE a2a_tasks
            SET result_json = ?1, updated_at = ?2,
                result_ready_at = COALESCE(result_ready_at, ?2)
            WHERE id = ?3
            "#,
            params![result_json, now, task_id],
        )?;
        if n == 0 {
            anyhow::bail!("bus result: görev bulunamadı: {task_id}");
        }
        Ok(())
    }

    /// Sonuç + durum tek transaction (P1-A: NATS wake yarışında result null olmasın).
    pub fn complete_a2a_with_result(
        &self,
        task_id: &str,
        status: TaskStatus,
        result_json: Option<&str>,
        release_idempotency: bool,
    ) -> Result<()> {
        if !matches!(
            status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            anyhow::bail!("complete_a2a_with_result: terminal status gerekli");
        }
        let conn = self.conn.lock().expect("experience db lock");
        let tx = conn.unchecked_transaction()?;
        let now = now_rfc3339();
        if let Some(raw) = result_json.filter(|s| !s.is_empty()) {
            tx.execute(
                r#"
                UPDATE a2a_tasks
                SET result_json = ?1, status = ?2, updated_at = ?3,
                    result_ready_at = COALESCE(result_ready_at, ?3)
                WHERE id = ?4
                "#,
                params![raw, status.as_str(), now, task_id],
            )?;
        } else {
            tx.execute(
                "UPDATE a2a_tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, task_id],
            )?;
        }
        if let Ok(Some(mut task)) = load_task_row(&tx, task_id) {
            task.status = status.clone();
            let _ = rewrite_payload(&tx, &task);
        }
        if release_idempotency {
            let _ = release_idempotency_for_task(&tx, task_id);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn session_can_read_a2a_task(&self, task_id: &str, session_id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("experience db lock");
        session_can_read_task(&conn, task_id, session_id)
    }

    pub fn session_or_token_can_read_a2a_task(
        &self,
        task_id: &str,
        session_id: &str,
        task_token: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("experience db lock");
        session_or_token_can_read(&conn, task_id, session_id, task_token)
    }

    pub fn cancel_a2a_task(&self, task_id: &str, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        cancel_task(&conn, task_id, session_id)
    }

    /// Atomik backgrounded geçişi. `true` = bu çağrı kazandı; `false` = terminal/zaten backgrounded
    /// (eşik anında tamamlanan görevde çift yanıt yok).
    pub fn try_mark_a2a_wait_timeout(&self, task_id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("experience db lock");
        let now = now_rfc3339();
        let n = conn.execute(
            r#"
            UPDATE a2a_tasks
            SET status = ?1, updated_at = ?2, backgrounded_at = COALESCE(backgrounded_at, ?2)
            WHERE id = ?3
              AND status IN ('QUEUED', 'DISPATCHED', 'EXECUTING', 'RECOVERY_PENDING', 'PENDING_APPROVAL')
            "#,
            params![TaskStatus::WaitTimeoutReached.as_str(), now, task_id],
        )?;
        if n > 0 {
            if let Some(mut task) = load_task_row(&conn, task_id)? {
                task.status = TaskStatus::WaitTimeoutReached;
                let _ = rewrite_payload(&conn, &task);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn mark_a2a_wait_timeout(&self, task_id: &str) -> Result<()> {
        let _ = self.try_mark_a2a_wait_timeout(task_id)?;
        Ok(())
    }

    pub fn touch_a2a_last_wait(&self, task_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        touch_last_wait(&conn, task_id)
    }

    /// Boş wait sayacını artır; yeni poll_after_secs döner.
    pub fn bump_a2a_empty_wait(&self, task_id: &str) -> Result<u64> {
        let conn = self.conn.lock().expect("experience db lock");
        let now = now_rfc3339();
        conn.execute(
            r#"
            UPDATE a2a_tasks
            SET empty_wait_count = empty_wait_count + 1, last_wait_at = ?1, updated_at = ?1
            WHERE id = ?2
            "#,
            params![now, task_id],
        )?;
        let count: i64 = conn.query_row(
            "SELECT empty_wait_count FROM a2a_tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        )?;
        // count artık artırılmış; next_poll önceki boş dönüş sayısına göre (count-1).
        Ok(next_poll_after_secs(count.saturating_sub(1).max(0) as u32))
    }

    pub fn a2a_flags(&self, task_id: &str) -> Result<(bool, bool)> {
        let conn = self.conn.lock().expect("experience db lock");
        let row: (i64, i64) = conn.query_row(
            "SELECT COALESCE(must_deliver,0), COALESCE(long_running,0) FROM a2a_tasks WHERE id = ?1",
            params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((row.0 != 0, row.1 != 0))
    }

    pub fn set_a2a_task_token_hash(&self, task_id: &str, hash: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        conn.execute(
            "UPDATE a2a_tasks SET task_token_hash = ?1 WHERE id = ?2",
            params![hash, task_id],
        )?;
        Ok(())
    }

    pub fn check_must_deliver_quota(&self, session_id: &str, source_agent: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        check_must_deliver_quota(&conn, session_id, source_agent)
    }

    pub fn peek_a2a_idempotency(
        &self,
        session_id: &str,
        project_id: &str,
        key: &str,
    ) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("experience db lock");
        let scope = format!("session:{session_id}");
        lookup_idempotency(&conn, &scope, project_id, key)
    }

    /// must_deliver: result_ready veya oluşturulma + TTL → FAILED(abandoned).
    pub fn abandon_stale_must_deliver(
        &self,
        now_rfc3339: &str,
        ttl: Duration,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("experience db lock");
        abandon_must_deliver_orphans(&conn, now_rfc3339, ttl)
    }

    pub fn mark_mcp_session_disconnected(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        mark_session_disconnected(&conn, session_id)
    }

    /// PR-3b orphan TTL taraması.
    /// `result_expired`: backgrounded+hazır, wait yok.
    /// `incomplete_expired`: (id, session_id) — control.stop için.
    #[allow(clippy::type_complexity)]
    pub fn expire_a2a_orphans(
        &self,
        now_rfc3339: &str,
        result_ttl: Duration,
        incomplete_ttl: Duration,
    ) -> Result<(Vec<String>, Vec<(String, Option<String>)>)> {
        let conn = self.conn.lock().expect("experience db lock");
        let result_expired = expire_result_orphans(&conn, now_rfc3339, result_ttl)?;
        let incomplete_expired = expire_incomplete_orphans(&conn, now_rfc3339, incomplete_ttl)?;
        Ok((result_expired, incomplete_expired))
    }

    pub fn pragma_foreign_keys(&self) -> Result<bool> {
        let conn = self.conn.lock().expect("experience db lock");
        // migrate sets ON; query current connection setting.
        let v: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        Ok(v != 0)
    }

    pub fn a2a_schema_version(&self) -> Result<Option<String>> {
        Ok(self.get_setting_sync(A2A_SCHEMA_VERSION_KEY)?.or(None))
    }
}

impl crate::db::ExperienceStore {
    fn get_setting_sync(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("experience db lock");
        Ok(conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod experiences_bridge {
    use super::*;
    /// Double-migrate smoke — idempotent.
    #[test]
    fn remigrate_twice() {
        let store = crate::db::ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        migrate_a2a(&conn).unwrap();
        migrate_a2a(&conn).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn migration_creates_a2a_tables_and_is_idempotent() {
        let store = ExperienceStore::memory().unwrap();
        let cols = store.table_columns("agent_sessions").unwrap();
        assert!(cols.iter().any(|c| c == "workspace_path"));
        let id_cols = store.table_columns("idempotency_keys").unwrap();
        assert!(id_cols.iter().any(|c| c == "scope"));
        assert!(id_cols.iter().any(|c| c == "project_id"));
        let task_cols = store.table_columns("a2a_tasks").unwrap();
        assert!(task_cols.iter().any(|c| c == "result_json"));
        assert!(task_cols.iter().any(|c| c == "claimed_by"));
        // ikinci migrate
        {
            let conn = store.conn.lock().unwrap();
            migrate_a2a(&conn).unwrap();
            migrate_a2a(&conn).unwrap();
        }
        assert_eq!(
            store.a2a_schema_version().unwrap().as_deref(),
            Some(A2A_SCHEMA_VERSION)
        );
        assert!(
            store.pragma_foreign_keys().unwrap(),
            "PRAGMA foreign_keys ON olmalı"
        );
    }

    #[test]
    fn hop_chain_blocked_at_max_hops() {
        let store = ExperienceStore::memory().unwrap();
        let max = 10u32;
        let mut prev = LoungeTask::new("a", "p", "root");
        match store.admit_a2a_task(&mut prev, max).unwrap() {
            AdmitOutcome::Accepted(_) => {}
            other => panic!("expected Accepted, got {other:?}"),
        }
        assert_eq!(prev.hop_count, 0);
        assert_eq!(prev.root_id.as_deref(), Some(prev.id.as_str()));

        for hop in 1..max {
            let mut child = LoungeTask::new("b", "p", format!("hop-{hop}"));
            child.parent_task_id = Some(prev.id.clone());
            child.hop_count = 999;
            child.root_id = Some("client-spoof-root".into());
            store.admit_a2a_task(&mut child, max).unwrap();
            assert_eq!(child.hop_count, hop);
            assert_eq!(child.root_id.as_deref(), Some(prev.effective_root_id()));
            prev = child;
        }

        let mut over = LoungeTask::new("c", "p", "too-deep");
        over.parent_task_id = Some(prev.id.clone());
        let err = store.admit_a2a_task(&mut over, max).unwrap_err();
        assert!(matches!(
            err,
            AdmitError::HopLimitExceeded {
                hop_count: 10,
                max_hops: 10
            }
        ));
    }

    #[test]
    fn rootless_task_forces_root_id_to_self() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "rootless");
        task.root_id = Some("spoofed-root".into());
        store.admit_a2a_task(&mut task, 10).unwrap();
        assert_eq!(task.root_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(task.hop_count, 0);
    }

    #[test]
    fn duplicate_idempotency_key_returns_replay() {
        let store = ExperienceStore::memory().unwrap();
        let mut t1 = LoungeTask::new("a", "p", "first");
        t1.idempotency_key = Some("same-key".into());
        store.admit_a2a_task(&mut t1, 10).unwrap();

        let mut t2 = LoungeTask::new("a", "p", "second");
        t2.idempotency_key = Some("same-key".into());
        match store.admit_a2a_task(&mut t2, 10).unwrap() {
            AdmitOutcome::Replay {
                existing_task_id, ..
            } => assert_eq!(existing_task_id, t1.id),
            AdmitOutcome::Accepted(_) => panic!("expected Replay"),
        }
        // farklı source → ayrı agent: kapsam
        let mut t3 = LoungeTask::new("other-agent", "p", "third");
        t3.idempotency_key = Some("same-key".into());
        assert!(matches!(
            store.admit_a2a_task(&mut t3, 10).unwrap(),
            AdmitOutcome::Accepted(_)
        ));
    }

    #[test]
    fn hop_and_session_idempotency_interact() {
        let store = ExperienceStore::memory().unwrap();
        let mut root = LoungeTask::new("mcp:cursor", "p", "root");
        root.session_id = Some("sess-h".into());
        root.idempotency_key = Some("root-k".into());
        root.source_verified = true;
        store.admit_a2a_task(&mut root, 3).unwrap();

        let mut child = LoungeTask::new("mcp:worker", "p", "child");
        child.session_id = Some("sess-h".into());
        child.parent_task_id = Some(root.id.clone());
        child.idempotency_key = Some("child-k".into());
        child.source_verified = true;
        store.admit_a2a_task(&mut child, 3).unwrap();
        assert_eq!(child.hop_count, 1);
        assert_eq!(child.root_id.as_deref(), Some(root.id.as_str()));

        // Aynı oturum + aynı key → replay (hop artırılmaz / yeni satır yok).
        let mut replay = LoungeTask::new("mcp:spoof", "p", "child-again");
        replay.session_id = Some("sess-h".into());
        replay.parent_task_id = Some(root.id.clone());
        replay.idempotency_key = Some("child-k".into());
        match store.admit_a2a_task(&mut replay, 3).unwrap() {
            AdmitOutcome::Replay {
                existing_task_id, ..
            } => assert_eq!(existing_task_id, child.id),
            AdmitOutcome::Accepted(_) => panic!("expected Replay"),
        }

        // Hop limiti session + key'den bağımsız uygulanır.
        let mut mid = child;
        for hop in 2..3 {
            let mut next = LoungeTask::new("mcp:worker", "p", format!("h{hop}"));
            next.session_id = Some("sess-h".into());
            next.parent_task_id = Some(mid.id.clone());
            next.idempotency_key = Some(format!("k-{hop}"));
            store.admit_a2a_task(&mut next, 3).unwrap();
            assert_eq!(next.hop_count, hop);
            mid = next;
        }
        let mut over = LoungeTask::new("mcp:worker", "p", "over");
        over.session_id = Some("sess-h".into());
        over.parent_task_id = Some(mid.id.clone());
        over.idempotency_key = Some("over-k".into());
        assert!(matches!(
            store.admit_a2a_task(&mut over, 3).unwrap_err(),
            AdmitError::HopLimitExceeded { .. }
        ));
    }

    #[test]
    fn session_scoped_idempotency_blocks_source_agent_spoof() {
        let store = ExperienceStore::memory().unwrap();
        let mut t1 = LoungeTask::new("spoofable-a", "p", "first");
        t1.session_id = Some("sess-1".into());
        t1.idempotency_key = Some("k".into());
        store.admit_a2a_task(&mut t1, 10).unwrap();

        // Aynı oturum, farklı source_agent spoof — replay (oturum kapsamı).
        let mut t2 = LoungeTask::new("spoofable-B", "p", "second");
        t2.session_id = Some("sess-1".into());
        t2.idempotency_key = Some("k".into());
        assert!(matches!(
            store.admit_a2a_task(&mut t2, 10).unwrap(),
            AdmitOutcome::Replay { .. }
        ));

        // Farklı oturum — kabul.
        let mut t3 = LoungeTask::new("spoofable-a", "p", "third");
        t3.session_id = Some("sess-2".into());
        t3.idempotency_key = Some("k".into());
        assert!(matches!(
            store.admit_a2a_task(&mut t3, 10).unwrap(),
            AdmitOutcome::Accepted(_)
        ));
    }

    #[test]
    fn yield_and_read_require_session_ownership() {
        let store = ExperienceStore::memory().unwrap();
        let mut t = LoungeTask::new("mcp:cursor", "p", "work");
        t.session_id = Some("caller-sess".into());
        t.target_agent = Some("worker".into());
        store.admit_a2a_task(&mut t, 10).unwrap();

        assert!(store
            .session_can_read_a2a_task(&t.id, "caller-sess")
            .unwrap());
        assert!(!store
            .session_can_read_a2a_task(&t.id, "other-sess")
            .unwrap());

        // Bağsız oturum yield edemez (eski test bunu meşru sayıyordu — yanlış).
        let unbound = store
            .yield_a2a_result(
                &t.id,
                "worker-sess",
                TaskStatus::Completed,
                r#"{"ok":true}"#,
            )
            .unwrap_err();
        assert!(
            unbound.to_string().contains("yetkisiz"),
            "expected yetkisiz unbound, got {unbound}"
        );

        let mut sess = AgentSession::new("p", "worker", "worker", "/tmp", "test");
        sess.id = "worker-sess".into();
        store.upsert_session(&sess).unwrap();

        store
            .yield_a2a_result(
                &t.id,
                "worker-sess",
                TaskStatus::Completed,
                r#"{"ok":true}"#,
            )
            .unwrap();
        assert_eq!(
            store.a2a_task_result(&t.id).unwrap().as_deref(),
            Some(r#"{"ok":true}"#)
        );
        // Terminal görev ezilemez.
        let terminal = store
            .yield_a2a_result(&t.id, "worker-sess", TaskStatus::Failed, "{}")
            .unwrap_err();
        assert!(
            terminal.to_string().contains("terminal")
                || terminal.to_string().contains("reddedildi"),
            "expected terminal reject, got {terminal}"
        );
        // İzinsiz oturum.
        let err = store
            .yield_a2a_result(&t.id, "intruder", TaskStatus::Completed, "{}")
            .unwrap_err();
        assert!(
            err.to_string().contains("yetkisiz") || err.to_string().contains("terminal"),
            "expected yetkisiz/terminal, got {err}"
        );
    }

    #[test]
    fn yield_rejects_non_yieldable_and_unbound() {
        let store = ExperienceStore::memory().unwrap();
        let mut t = LoungeTask::new("mcp:cursor", "p", "work");
        t.session_id = Some("caller".into());
        t.target_agent = Some("worker".into());
        store.admit_a2a_task(&mut t, 10).unwrap();
        store
            .set_a2a_task_status(&t.id, TaskStatus::PendingApproval)
            .unwrap();
        let mut sess = AgentSession::new("p", "worker", "worker", "/tmp", "test");
        sess.id = "worker-sess".into();
        store.upsert_session(&sess).unwrap();
        let err = store
            .yield_a2a_result(&t.id, "worker-sess", TaskStatus::Completed, "{}")
            .unwrap_err();
        assert!(
            err.to_string().contains("yield edilemez") || err.to_string().contains("reddedildi"),
            "got {err}"
        );
    }

    #[test]
    fn unique_violation_rolls_back_task_row() {
        let store = ExperienceStore::memory().unwrap();
        // Önceden dolu idempotency satırı.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                r#"INSERT INTO idempotency_keys
                   (scope, project_id, idempotency_key, task_id, created_at)
                   VALUES ('agent:a', 'p', 'race-key', 'existing-task', ?1)"#,
                params![now_rfc3339()],
            )
            .unwrap();
        }
        let mut task = LoungeTask::new("a", "p", "should-rollback");
        task.idempotency_key = Some("race-key".into());
        let conn = store.conn.lock().unwrap();
        // Lookup atla → gerçek UNIQUE INSERT hatası.
        let outcome = admit_task_atomic_skip_lookup(&conn, &mut task, 10).unwrap();
        // existing-task a2a_tasks'ta yok; UNIQUE sonrası mevcut satır okunur → Replay
        // veya Storage. Mevcut key existing-task'a işaret ediyor.
        match outcome {
            AdmitOutcome::Replay {
                existing_task_id, ..
            } => assert_eq!(existing_task_id, "existing-task"),
            AdmitOutcome::Accepted(_) => panic!("should not accept"),
        }
        assert_eq!(
            count_a2a_tasks(&conn).unwrap(),
            0,
            "UNIQUE sonrası task satırı geri alınmalı"
        );
        assert_eq!(count_idempotency_keys(&conn).unwrap(), 1);
    }

    #[test]
    fn concurrent_same_key_admits_exactly_one_task() {
        let store = ExperienceStore::memory().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for i in 0..2 {
            let store = store.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                let mut task = LoungeTask::new("a", "p", format!("race-{i}"));
                task.idempotency_key = Some("concurrent-key".into());
                barrier.wait();
                store.admit_a2a_task(&mut task, 10)
            }));
        }
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let accepted = results
            .iter()
            .filter(|r| matches!(r, Ok(AdmitOutcome::Accepted(_))))
            .count();
        let replayed = results
            .iter()
            .filter(|r| matches!(r, Ok(AdmitOutcome::Replay { .. })))
            .count();
        assert_eq!(accepted, 1, "tam bir Accepted beklenir: {results:?}");
        assert_eq!(replayed, 1, "diğeri Replay olmalı: {results:?}");
        let conn = store.conn.lock().unwrap();
        assert_eq!(count_a2a_tasks(&conn).unwrap(), 1);
        assert_eq!(count_idempotency_keys(&conn).unwrap(), 1);
    }

    #[test]
    fn silent_executing_task_becomes_needs_human() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "zombie");
        store.admit_a2a_task(&mut task, 10).unwrap();
        store
            .set_a2a_task_status(&task.id, TaskStatus::Executing)
            .unwrap();
        store
            .touch_a2a_updated_at(&task.id, "2026-10-04T12:00:00.000Z")
            .unwrap();

        let marked = store
            .recover_silent_a2a_tasks("2026-10-04T12:03:00.000Z", Duration::from_secs(120))
            .unwrap();
        assert_eq!(marked, vec![task.id.clone()]);
        assert_eq!(
            store.a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::NeedsHuman)
        );
    }

    #[test]
    fn fresh_executing_task_not_marked_needs_human() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "alive");
        store.admit_a2a_task(&mut task, 10).unwrap();
        store
            .set_a2a_task_status(&task.id, TaskStatus::Executing)
            .unwrap();
        store
            .touch_a2a_updated_at(&task.id, "2026-10-04T12:02:30.000Z")
            .unwrap();
        let marked = store
            .recover_silent_a2a_tasks("2026-10-04T12:03:00.000Z", Duration::from_secs(120))
            .unwrap();
        assert!(marked.is_empty());
    }

    #[test]
    fn agent_session_registry_count() {
        let store = ExperienceStore::memory().unwrap();
        assert_eq!(store.session_count().unwrap(), 0);
        let s = AgentSession::new("p", "cursor", "gui", "/tmp/ws", "user_pin");
        store.upsert_session(&s).unwrap();
        assert_eq!(store.session_count().unwrap(), 1);
    }

    #[test]
    fn agent_sessions_pruned_to_cap_keeping_newest() {
        let store = ExperienceStore::memory().unwrap();
        // Küçük tavan ile spam → en yeni keep kalır.
        {
            let conn = store.conn.lock().unwrap();
            for i in 0..5 {
                let mut s = AgentSession::new("p", "worker", "mcp", "", "mcp_meta");
                s.id = format!("sess-{i}");
                s.last_seen = format!("2026-10-04T12:00:0{i}.000Z");
                upsert_agent_session(&conn, &s).unwrap();
            }
            let mut keep = AgentSession::new("p", "worker", "mcp", "", "mcp_meta");
            keep.id = "sess-keep".into();
            keep.last_seen = "2026-10-04T13:00:00.000Z".into();
            upsert_agent_session(&conn, &keep).unwrap();
            let pruned = prune_agent_sessions(&conn, 3, "sess-keep").unwrap();
            assert!(pruned >= 3, "expected prune, got {pruned}");
            assert_eq!(count_agent_sessions(&conn).unwrap(), 3);
            let keep_exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM agent_sessions WHERE id = 'sess-keep'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(keep_exists, 1);
        }
    }

    #[test]
    fn parent_terminal_status_rejected() {
        let store = ExperienceStore::memory().unwrap();
        let mut parent = LoungeTask::new("a", "p", "parent");
        store.admit_a2a_task(&mut parent, 10).unwrap();
        store
            .set_a2a_task_status(&parent.id, TaskStatus::Failed)
            .unwrap();
        let mut child = LoungeTask::new("b", "p", "child");
        child.parent_task_id = Some(parent.id.clone());
        let err = store.admit_a2a_task(&mut child, 10).unwrap_err();
        assert!(matches!(err, AdmitError::ParentInvalid { .. }));
    }

    #[test]
    fn parent_project_mismatch_rejected() {
        let store = ExperienceStore::memory().unwrap();
        let mut parent = LoungeTask::new("a", "proj-a", "parent");
        store.admit_a2a_task(&mut parent, 10).unwrap();
        let mut child = LoungeTask::new("b", "proj-b", "child");
        child.parent_task_id = Some(parent.id.clone());
        let err = store.admit_a2a_task(&mut child, 10).unwrap_err();
        assert!(matches!(err, AdmitError::ParentInvalid { .. }));
    }

    #[test]
    fn release_idempotency_allows_retry_after_failure() {
        let store = ExperienceStore::memory().unwrap();
        let mut t1 = LoungeTask::new("a", "p", "first");
        t1.idempotency_key = Some("retry-key".into());
        store.admit_a2a_task(&mut t1, 10).unwrap();
        store
            .set_a2a_task_status(&t1.id, TaskStatus::Failed)
            .unwrap();
        store.release_a2a_idempotency(&t1.id).unwrap();
        let mut t2 = LoungeTask::new("a", "p", "retry");
        t2.idempotency_key = Some("retry-key".into());
        assert!(matches!(
            store.admit_a2a_task(&mut t2, 10).unwrap(),
            AdmitOutcome::Accepted(_)
        ));
    }

    #[test]
    fn max_hops_clamped_to_absolute_ceiling() {
        assert_eq!(ABSOLUTE_MAX_HOPS, 64);
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "x");
        // 100 istenirse normalize clamp ile kabul (hop 0 < 64)
        store.admit_a2a_task(&mut task, 100).unwrap();
    }

    #[test]
    fn in_flight_parent_missing_is_documented_fail_closed() {
        let store = ExperienceStore::memory().unwrap();
        let mut child = LoungeTask::new("b", "p", "orphan");
        child.parent_task_id = Some("not-yet-admitted".into());
        let err = store.admit_a2a_task(&mut child, 10).unwrap_err();
        assert!(matches!(err, AdmitError::ParentMissing { .. }));
    }

    #[test]
    fn legacy_single_pk_idempotency_migrates_preserving_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL
            );
            CREATE TABLE idempotency_keys (
                idempotency_key TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO idempotency_keys (idempotency_key, task_id, created_at) VALUES (?1, ?2, ?3)",
            params![
                "legacy-key-1",
                "task-legacy-1",
                "2026-01-01T00:00:00.000Z"
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO idempotency_keys (idempotency_key, task_id, created_at) VALUES (?1, ?2, ?3)",
            params![
                "legacy-key-2",
                "task-legacy-2",
                "2026-01-02T00:00:00.000Z"
            ],
        )
        .unwrap();

        migrate_a2a(&conn).unwrap();

        let cols = column_names_fallback(&conn, "idempotency_keys").unwrap();
        assert!(cols.iter().any(|c| c == "scope"));
        assert!(cols.iter().any(|c| c == "project_id"));
        assert_eq!(count_idempotency_keys(&conn).unwrap(), 2);

        let task_id: String = conn
            .query_row(
                r#"SELECT task_id FROM idempotency_keys
                   WHERE scope = 'agent:unknown' AND project_id = 'unknown'
                     AND idempotency_key = 'legacy-key-1'"#,
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(task_id, "task-legacy-1");

        // İkinci migrate no-op + veri korunur
        migrate_a2a(&conn).unwrap();
        assert_eq!(count_idempotency_keys(&conn).unwrap(), 2);
    }

    #[test]
    fn silent_queued_becomes_needs_human_but_pending_approval_does_not() {
        let store = ExperienceStore::memory().unwrap();
        let mut queued = LoungeTask::new("a", "p", "stuck-queued");
        store.admit_a2a_task(&mut queued, 10).unwrap();
        // admit sonrası QUEUED
        store
            .touch_a2a_updated_at(&queued.id, "2026-10-04T12:00:00.000Z")
            .unwrap();

        let mut pending = LoungeTask::new("a", "p", "awaiting-user");
        store.admit_a2a_task(&mut pending, 10).unwrap();
        store
            .set_a2a_task_status(&pending.id, TaskStatus::PendingApproval)
            .unwrap();
        store
            .touch_a2a_updated_at(&pending.id, "2026-10-04T12:00:00.000Z")
            .unwrap();

        let marked = store
            .recover_silent_a2a_tasks("2026-10-04T12:03:00.000Z", Duration::from_secs(120))
            .unwrap();
        assert_eq!(marked, vec![queued.id.clone()]);
        assert_eq!(
            store.a2a_task_status(&queued.id).unwrap(),
            Some(TaskStatus::NeedsHuman)
        );
        assert_eq!(
            store.a2a_task_status(&pending.id).unwrap(),
            Some(TaskStatus::PendingApproval)
        );
    }

    #[test]
    fn touch_dispatched_for_target_refreshes_updated_at() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "delegated");
        task.target_agent = Some("grok-tester".into());
        store.admit_a2a_task(&mut task, 10).unwrap();
        store
            .set_a2a_task_status(&task.id, TaskStatus::Dispatched)
            .unwrap();
        store
            .touch_a2a_updated_at(&task.id, "2026-10-04T10:00:00.000Z")
            .unwrap();
        let n = store.touch_dispatched_for_agent("grok-tester").unwrap();
        assert_eq!(n, 1);
        let marked = store
            .recover_silent_a2a_tasks("2026-10-04T10:01:00.000Z", Duration::from_secs(120))
            .unwrap();
        assert!(marked.is_empty(), "heartbeat sonrası sessiz sayılmamalı");
    }

    #[test]
    fn result_orphan_expires_after_ttl_without_wait() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("mcp:cursor", "p", "bg-done");
        task.session_id = Some("sess-r".into());
        store.admit_a2a_task(&mut task, 10).unwrap();
        store.mark_a2a_wait_timeout(&task.id).unwrap();
        // Worker sonucu yazdı (backgrounded sonrası).
        store
            .complete_a2a_with_result(
                &task.id,
                TaskStatus::Completed,
                Some(r#"{"ok":true}"#),
                false,
            )
            .unwrap();
        // result_ready_at şimdi; 11 dk sonra expire.
        let (expired, incomplete) = store
            .expire_a2a_orphans(
                "2099-01-01T00:11:00.000Z", // far future relative — use fixed
                Duration::from_secs(600),
                Duration::from_secs(1800),
            )
            .unwrap();
        // now is in the future vs result_ready_at (real now) — age huge → expired
        assert!(
            expired.contains(&task.id),
            "result orphan expire: {expired:?}"
        );
        assert!(incomplete.is_empty());
        assert_eq!(
            store.a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Expired)
        );
    }

    #[test]
    fn result_orphan_skipped_if_waited() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("mcp:cursor", "p", "bg-waited");
        task.session_id = Some("sess-w".into());
        store.admit_a2a_task(&mut task, 10).unwrap();
        store.mark_a2a_wait_timeout(&task.id).unwrap();
        store
            .complete_a2a_with_result(
                &task.id,
                TaskStatus::Completed,
                Some(r#"{"ok":true}"#),
                false,
            )
            .unwrap();
        store.touch_a2a_last_wait(&task.id).unwrap();
        let (expired, _) = store
            .expire_a2a_orphans(
                "2099-01-01T00:00:00.000Z",
                Duration::from_secs(1),
                Duration::from_secs(1800),
            )
            .unwrap();
        assert!(
            !expired.contains(&task.id),
            "wait sonrası result orphan olmamalı"
        );
        assert_eq!(
            store.a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Completed)
        );
    }

    #[test]
    fn incomplete_orphan_expires_when_session_disconnected() {
        let store = ExperienceStore::memory().unwrap();
        let mut sess = AgentSession::new("p", "cursor", "mcp", "/tmp", "mcp_meta");
        sess.id = "sess-gone".into();
        store.upsert_session(&sess).unwrap();
        let mut task = LoungeTask::new("mcp:cursor", "p", "orph");
        task.session_id = Some("sess-gone".into());
        store.admit_a2a_task(&mut task, 10).unwrap();
        store
            .set_a2a_task_status(&task.id, TaskStatus::WaitTimeoutReached)
            .unwrap();
        store.mark_mcp_session_disconnected("sess-gone").unwrap();
        // updated_at = now; force old updated_at
        store
            .touch_a2a_updated_at(&task.id, "2026-10-04T12:00:00.000Z")
            .unwrap();
        let (_r, incomplete) = store
            .expire_a2a_orphans(
                "2026-10-04T12:35:00.000Z",
                Duration::from_secs(600),
                Duration::from_secs(30 * 60),
            )
            .unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].0, task.id);
        assert_eq!(
            store.a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Expired)
        );
    }

    #[test]
    fn running_task_not_killed_by_result_orphan_ttl() {
        let store = ExperienceStore::memory().unwrap();
        let mut sess = AgentSession::new("p", "cursor", "mcp", "/tmp", "mcp_meta");
        sess.id = "sess-run".into();
        sess.state = "active".into();
        store.upsert_session(&sess).unwrap();
        let mut task = LoungeTask::new("mcp:cursor", "p", "still-run");
        task.session_id = Some("sess-run".into());
        store.admit_a2a_task(&mut task, 10).unwrap();
        store.mark_a2a_wait_timeout(&task.id).unwrap();
        // Sonuç yok — result orphan uygulanmaz; oturum active → incomplete yok.
        let (expired, incomplete) = store
            .expire_a2a_orphans(
                "2099-01-01T00:00:00.000Z",
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(expired.is_empty());
        assert!(incomplete.is_empty());
        assert_eq!(
            store.a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::WaitTimeoutReached)
        );
    }
}
