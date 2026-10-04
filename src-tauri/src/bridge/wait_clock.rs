//! Enjekte edilebilir bekleme saati — testlerde gerçek duvar saati yok.
//!
//! Üretim: [`SystemWaitClock`] → `tokio::time::sleep`.
//! Test: [`ManualWaitClock`] → `advance` ile süre ilerletilir.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

pub type WaitFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub trait WaitClock: Send + Sync {
    /// Monotonik milisaniye (test/üretim karşılaştırması için).
    fn now_ms(&self) -> u64;
    fn sleep(&self, duration: Duration) -> WaitFuture<'_>;
}

/// Üretim saati — gerçek tokio sleep.
#[derive(Debug, Default, Clone)]
pub struct SystemWaitClock;

impl WaitClock for SystemWaitClock {
    fn now_ms(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn sleep(&self, duration: Duration) -> WaitFuture<'_> {
        Box::pin(async move {
            if !duration.is_zero() {
                tokio::time::sleep(duration).await;
            }
        })
    }
}

/// Test saati — `advance` çağrılınca bekleyen sleep’ler uyanır.
#[derive(Debug)]
pub struct ManualWaitClock {
    now_ms: AtomicU64,
    waiters: Mutex<Vec<(u64, Arc<Notify>)>>,
}

impl Default for ManualWaitClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualWaitClock {
    pub fn new() -> Self {
        Self {
            now_ms: AtomicU64::new(0),
            waiters: Mutex::new(Vec::new()),
        }
    }

    pub fn advance(&self, by: Duration) {
        let add = by.as_millis() as u64;
        let now = self.now_ms.fetch_add(add, Ordering::SeqCst) + add;
        let mut waiters = self.waiters.lock().expect("manual clock waiters");
        waiters.retain(|(deadline, notify)| {
            if now >= *deadline {
                notify.notify_waiters();
                false
            } else {
                true
            }
        });
    }

    pub fn set_ms(&self, ms: u64) {
        self.now_ms.store(ms, Ordering::SeqCst);
        let mut waiters = self.waiters.lock().expect("manual clock waiters");
        waiters.retain(|(deadline, notify)| {
            if ms >= *deadline {
                notify.notify_waiters();
                false
            } else {
                true
            }
        });
    }
}

impl WaitClock for ManualWaitClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn sleep(&self, duration: Duration) -> WaitFuture<'_> {
        let ms = duration.as_millis() as u64;
        Box::pin(async move {
            if ms == 0 {
                return;
            }
            let deadline = self.now_ms() + ms;
            let notify = Arc::new(Notify::new());
            {
                let mut waiters = self.waiters.lock().expect("manual clock waiters");
                if self.now_ms() >= deadline {
                    return;
                }
                waiters.push((deadline, notify.clone()));
            }
            notify.notified().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_clock_sleep_wakes_on_advance() {
        let clock = Arc::new(ManualWaitClock::new());
        let c = clock.clone();
        let handle = tokio::spawn(async move {
            c.sleep(Duration::from_secs(10)).await;
        });
        tokio::task::yield_now().await;
        assert!(!handle.is_finished());
        clock.advance(Duration::from_secs(10));
        handle.await.unwrap();
        assert_eq!(clock.now_ms(), 10_000);
    }
}
