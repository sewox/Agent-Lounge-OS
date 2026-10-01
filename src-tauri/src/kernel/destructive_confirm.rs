//! One-shot confirmation tokens for PolicyGate-blocked destructive commands (F3).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::policy_gate::{ActionSource, DestructiveClass};

const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize)]
pub struct DestructivePendingEvent {
    pub id: String,
    pub kind: String,
    pub command: String,
    pub pattern: String,
    pub source: String,
    pub class: String,
    pub command_hash: String,
}

#[derive(Debug, Clone)]
struct PendingToken {
    command_hash: String,
    #[allow(dead_code)]
    command: String,
    #[allow(dead_code)]
    class: DestructiveClass,
    #[allow(dead_code)]
    source: ActionSource,
    created: Instant,
    /// Set after `confirm_destructive(id)` — allows one matching spawn.
    confirmed: bool,
    consumed: bool,
}

struct Registry {
    pending: HashMap<String, PendingToken>,
    /// Test / observer capture of emitted events.
    emitted: Vec<DestructivePendingEvent>,
    /// Optional NATS subject publishes (command JSON).
    nats_emitted: Vec<String>,
}

impl Registry {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
            emitted: Vec::new(),
            nats_emitted: Vec::new(),
        }
    }
}

fn registry() -> &'static Mutex<Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Registry::new()))
}

/// Optional OS/UI emitter installed at app startup.
type EmitFn = Box<dyn Fn(&DestructivePendingEvent) + Send + Sync>;

fn emitter_slot() -> &'static Mutex<Option<EmitFn>> {
    static SLOT: OnceLock<Mutex<Option<EmitFn>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn install_emitter<F>(f: F)
where
    F: Fn(&DestructivePendingEvent) + Send + Sync + 'static,
{
    *emitter_slot().lock().expect("emitter lock") = Some(Box::new(f));
}

pub fn clear_emitter() {
    *emitter_slot().lock().expect("emitter lock") = None;
}

/// Stable hash of program + args (order-sensitive).
pub fn command_hash(program: &str, args: &[impl AsRef<str>]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(program.as_bytes());
    hasher.update([0]);
    for a in args {
        hasher.update(a.as_ref().as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Register a blocked destructive command: emit approval_pending + return token id.
/// Hash uses the same program + argv as [`GuardedCommand`] evaluate (F18).
pub fn register_pending(
    program: &str,
    args: &[String],
    class: DestructiveClass,
    source: ActionSource,
) -> DestructivePendingEvent {
    let id = Uuid::new_v4().to_string();
    let hash = command_hash(program, args);
    let command = std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    let event = DestructivePendingEvent {
        id: id.clone(),
        kind: "destructive".into(),
        command: command.clone(),
        pattern: format!("{class:?}"),
        source: format!("{source:?}").to_ascii_lowercase(),
        class: format!("{class:?}"),
        command_hash: hash.clone(),
    };

    {
        let mut reg = registry().lock().expect("registry");
        reg.pending.insert(
            id.clone(),
            PendingToken {
                command_hash: hash,
                command,
                class,
                source,
                created: Instant::now(),
                confirmed: false,
                consumed: false,
            },
        );
        reg.emitted.push(event.clone());
        reg.nats_emitted
            .push(serde_json::to_string(&event).unwrap_or_default());
    }

    if let Some(emit) = emitter_slot().lock().expect("emitter").as_ref() {
        emit(&event);
    }

    event
}

/// User confirmed the destructive approval — token becomes single-use runnable.
pub fn confirm_destructive(id: &str) -> Result<()> {
    let mut reg = registry().lock().expect("registry");
    let Some(token) = reg.pending.get_mut(id) else {
        bail!("unknown destructive confirmation id");
    };
    if token.created.elapsed() > TOKEN_TTL {
        reg.pending.remove(id);
        bail!("destructive confirmation expired");
    }
    if token.consumed {
        bail!("destructive confirmation already used");
    }
    token.confirmed = true;
    Ok(())
}

/// Returns true (and consumes the token) when a confirmed matching hash exists.
pub fn take_confirmed_allowance(command_hash: &str) -> bool {
    let mut reg = registry().lock().expect("registry");
    reg.pending.retain(|_, t| t.created.elapsed() <= TOKEN_TTL);
    let key = reg
        .pending
        .iter()
        .find(|(_, t)| t.confirmed && !t.consumed && t.command_hash == command_hash)
        .map(|(k, _)| k.clone());
    let Some(key) = key else {
        return false;
    };
    if let Some(token) = reg.pending.get_mut(&key) {
        token.consumed = true;
        return true;
    }
    false
}

/// Reject replaying a consumed / unknown token.
pub fn assert_token_not_reusable(id: &str) -> Result<()> {
    let reg = registry().lock().expect("registry");
    match reg.pending.get(id) {
        None => bail!("destructive confirmation unknown or expired"),
        Some(t) if t.consumed => bail!("destructive confirmation already used"),
        Some(t) if t.created.elapsed() > TOKEN_TTL => bail!("destructive confirmation expired"),
        Some(_) => Ok(()),
    }
}

pub fn take_emitted_for_tests() -> Vec<DestructivePendingEvent> {
    let mut reg = registry().lock().expect("registry");
    std::mem::take(&mut reg.emitted)
}

pub fn take_nats_emitted_for_tests() -> Vec<String> {
    let mut reg = registry().lock().expect("registry");
    std::mem::take(&mut reg.nats_emitted)
}

pub fn reset_for_tests() {
    let mut reg = registry().lock().expect("registry");
    *reg = Registry::new();
    clear_emitter();
}

/// Serializes tests that share the process-global destructive registry.
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::guarded_command::GuardedCommand;
    use crate::kernel::policy_gate::ActionSource;
    use std::sync::{Mutex, OnceLock};

    /// Global registry is process-wide; serialize these tests to avoid races under
    /// `cargo test` parallelism (seen as flaky `emitted.len() == 2` on macOS CI).
    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn destructive_confirm_flow_single_use() {
        let _guard = test_lock().lock().expect("destructive confirm test lock");
        reset_for_tests();
        let args = vec!["-rf".into(), "/tmp/x".into()];
        let event = register_pending("rm", &args, DestructiveClass::PosixRm, ActionSource::Agent);
        let emitted = take_emitted_for_tests();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].kind, "destructive");
        assert!(!take_nats_emitted_for_tests().is_empty());

        // Blocked before confirm.
        let err = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/x"])
            .source(ActionSource::Agent)
            .output();
        assert!(err.is_err());

        confirm_destructive(&event.id).unwrap();

        // After confirm, same program+args hash may run once.
        // Use a dry-run path: take_confirmed_allowance directly + spawn echo stand-in.
        let hash = command_hash("rm", &["-rf", "/tmp/x"]);
        assert!(take_confirmed_allowance(&hash));
        // Replay allowance denied.
        assert!(!take_confirmed_allowance(&hash));
        assert!(confirm_destructive(&event.id).is_err());
    }

    #[test]
    fn confirm_hash_matches_argv_with_spaces() {
        let _guard = test_lock().lock().expect("destructive confirm test lock");
        reset_for_tests();
        let args = vec!["-rf".into(), "/tmp/my dir".into()];
        let event = register_pending("rm", &args, DestructiveClass::PosixRm, ActionSource::User);
        assert_eq!(
            event.command_hash,
            command_hash("rm", &["-rf", "/tmp/my dir"])
        );
        confirm_destructive(&event.id).unwrap();
        assert!(take_confirmed_allowance(&event.command_hash));
    }
}
