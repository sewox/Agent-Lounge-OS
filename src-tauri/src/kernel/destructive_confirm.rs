//! One-shot confirmation tokens for PolicyGate-blocked destructive commands (F3).
//! Pending tokens are tracked in FIFO insertion order for the confirmation UI.

use std::collections::{HashMap, VecDeque};
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
    /// FIFO insertion order of pending ids (oldest first).
    order: VecDeque<String>,
    /// Full event payloads keyed by id (for UI list).
    events: HashMap<String, DestructivePendingEvent>,
    /// Test / observer capture of emitted events.
    emitted: Vec<DestructivePendingEvent>,
    /// Optional NATS subject publishes (command JSON).
    nats_emitted: Vec<String>,
}

impl Registry {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
            order: VecDeque::new(),
            events: HashMap::new(),
            emitted: Vec::new(),
            nats_emitted: Vec::new(),
        }
    }

    fn purge_expired_locked(&mut self, now_elapsed: impl Fn(&PendingToken) -> bool) -> Vec<String> {
        let expired: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, t)| now_elapsed(t))
            .map(|(k, _)| k.clone())
            .collect();
        for id in &expired {
            self.pending.remove(id);
            self.events.remove(id);
            self.order.retain(|x| x != id);
        }
        expired
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

/// Clear the OS-notification pending slot for this confirm id (id-matched).
/// Called on confirm / reject / expiry so Focused/Reopen cannot re-raise with a stale id.
fn clear_notification_pending_slot(id: &str) {
    crate::services::clear_pending_approval_if_matches(id);
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
        // Drop expired before inserting so the UI list stays accurate.
        let expired = reg.purge_expired_locked(|t| t.created.elapsed() > TOKEN_TTL);
        drop(expired);
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
        reg.events.insert(id.clone(), event.clone());
        reg.order.push_back(id.clone());
        reg.emitted.push(event.clone());
        reg.nats_emitted
            .push(serde_json::to_string(&event).unwrap_or_default());
    }

    if let Some(emit) = emitter_slot().lock().expect("emitter").as_ref() {
        emit(&event);
    }

    event
}

/// FIFO list of pending (unconfirmed, unconsumed, unexpired) destructive events.
pub fn list_pending_destructive() -> Vec<DestructivePendingEvent> {
    let mut reg = registry().lock().expect("registry");
    let expired = reg.purge_expired_locked(|t| t.created.elapsed() > TOKEN_TTL);
    drop(expired);
    reg.order
        .iter()
        .filter_map(|id| {
            let token = reg.pending.get(id)?;
            if token.confirmed || token.consumed {
                return None;
            }
            reg.events.get(id).cloned()
        })
        .collect()
}

/// User confirmed the destructive approval — token becomes single-use runnable.
pub fn confirm_destructive(id: &str) -> Result<()> {
    let outcome: Result<(), &'static str> = {
        let mut reg = registry().lock().expect("registry");
        if !reg.pending.contains_key(id) {
            Err("unknown")
        } else if reg
            .pending
            .get(id)
            .is_some_and(|t| t.created.elapsed() > TOKEN_TTL)
        {
            reg.pending.remove(id);
            reg.events.remove(id);
            reg.order.retain(|x| x != id);
            Err("expired")
        } else if reg.pending.get(id).is_some_and(|t| t.consumed) {
            Err("consumed")
        } else {
            if let Some(token) = reg.pending.get_mut(id) {
                token.confirmed = true;
            }
            // Confirmed tokens leave the UI list (but stay until consumed/spawn).
            reg.events.remove(id);
            reg.order.retain(|x| x != id);
            Ok(())
        }
    };
    // Always clear the notification slot for this id (success, expired, or unknown).
    clear_notification_pending_slot(id);
    match outcome {
        Ok(()) => Ok(()),
        Err("unknown") => bail!("unknown destructive confirmation id"),
        Err("expired") => bail!("destructive confirmation expired"),
        Err("consumed") => bail!("destructive confirmation already used"),
        Err(_) => bail!("unknown destructive confirmation id"),
    }
}

/// User rejected the destructive approval — drop the token and clear the notification slot.
pub fn reject_destructive(id: &str) -> Result<()> {
    let removed = {
        let mut reg = registry().lock().expect("registry");
        let had = reg.pending.remove(id).is_some();
        reg.events.remove(id);
        reg.order.retain(|x| x != id);
        had
    };
    clear_notification_pending_slot(id);
    if removed {
        Ok(())
    } else {
        bail!("unknown destructive confirmation id")
    }
}

/// Returns true (and consumes the token) when a confirmed matching hash exists.
pub fn take_confirmed_allowance(command_hash: &str) -> bool {
    let (expired, key) = {
        let mut reg = registry().lock().expect("registry");
        let expired = reg.purge_expired_locked(|t| t.created.elapsed() > TOKEN_TTL);
        let key = reg
            .pending
            .iter()
            .find(|(_, t)| t.confirmed && !t.consumed && t.command_hash == command_hash)
            .map(|(k, _)| k.clone());
        if let Some(ref k) = key {
            if let Some(token) = reg.pending.get_mut(k) {
                token.consumed = true;
            }
            reg.events.remove(k);
            reg.order.retain(|x| x != k);
        }
        (expired, key)
    };
    for id in expired {
        clear_notification_pending_slot(&id);
    }
    if let Some(ref k) = key {
        clear_notification_pending_slot(k);
    }
    key.is_some()
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
    use crate::services::{
        approval_notify::pending_slot_test_lock, pending_approval_task_id,
        set_pending_approval_task_id, should_focus_on_activation,
    };

    #[test]
    fn destructive_confirm_flow_single_use() {
        let _guard = test_lock();
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
        let _guard = test_lock();
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

    #[test]
    fn confirm_and_reject_clear_notification_pending_slot() {
        let _guard = test_lock();
        let _slot = pending_slot_test_lock();
        reset_for_tests();

        let args = vec!["-rf".into(), "/tmp/slot-confirm".into()];
        let event = register_pending("rm", &args, DestructiveClass::PosixRm, ActionSource::Agent);
        set_pending_approval_task_id(Some(event.id.clone()));
        assert!(should_focus_on_activation());
        confirm_destructive(&event.id).unwrap();
        assert_eq!(pending_approval_task_id(), None);
        assert!(!should_focus_on_activation());

        let args2 = vec!["-rf".into(), "/tmp/slot-reject".into()];
        let event2 = register_pending("rm", &args2, DestructiveClass::PosixRm, ActionSource::User);
        set_pending_approval_task_id(Some(event2.id.clone()));
        assert!(should_focus_on_activation());
        reject_destructive(&event2.id).unwrap();
        assert_eq!(pending_approval_task_id(), None);
        assert!(!should_focus_on_activation());

        // Mismatched id must not clear a different pending approval.
        set_pending_approval_task_id(Some("live-other".into()));
        let args3 = vec!["-rf".into(), "/tmp/slot-mismatch".into()];
        let event3 = register_pending("rm", &args3, DestructiveClass::PosixRm, ActionSource::Agent);
        confirm_destructive(&event3.id).unwrap();
        assert_eq!(pending_approval_task_id().as_deref(), Some("live-other"));
        set_pending_approval_task_id(None);
    }

    #[test]
    fn fifo_list_preserves_order_and_survives_newer_resolve() {
        let _guard = test_lock();
        reset_for_tests();
        let a = register_pending(
            "rm",
            &["-rf".into(), "/tmp/a".into()],
            DestructiveClass::PosixRm,
            ActionSource::Agent,
        );
        let b = register_pending(
            "git",
            &["reset".into(), "--hard".into()],
            DestructiveClass::GitResetHard,
            ActionSource::User,
        );
        let list = list_pending_destructive();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, a.id);
        assert_eq!(list[1].id, b.id);

        // Resolving the newer must leave the older in the FIFO list.
        confirm_destructive(&b.id).unwrap();
        let after = list_pending_destructive();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, a.id);

        reject_destructive(&a.id).unwrap();
        assert!(list_pending_destructive().is_empty());
    }

    #[test]
    fn reject_returns_error_for_unknown_id() {
        let _guard = test_lock();
        reset_for_tests();
        let err = reject_destructive("missing-id").unwrap_err();
        assert!(format!("{err}").contains("unknown"));
    }
}
