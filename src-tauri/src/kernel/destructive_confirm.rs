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

pub fn command_hash_line(command_line: &str) -> String {
    let parts: Vec<&str> = command_line.split_whitespace().collect();
    if parts.is_empty() {
        return command_hash("", &[] as &[&str]);
    }
    command_hash(parts[0], &parts[1..])
}

/// Register a blocked destructive command: emit approval_pending + return token id.
pub fn register_pending(
    command: &str,
    class: DestructiveClass,
    source: ActionSource,
) -> DestructivePendingEvent {
    let id = Uuid::new_v4().to_string();
    let hash = command_hash_line(command);
    let event = DestructivePendingEvent {
        id: id.clone(),
        kind: "destructive".into(),
        command: command.to_string(),
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
                command: command.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::guarded_command::GuardedCommand;
    use crate::kernel::policy_gate::ActionSource;

    #[test]
    fn destructive_confirm_flow_single_use() {
        reset_for_tests();
        let cmd = "rm -rf /tmp/x";
        let event = register_pending(cmd, DestructiveClass::PosixRm, ActionSource::Agent);
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
}
