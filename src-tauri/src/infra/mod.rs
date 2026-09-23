//! Yerel servis API'lerinden kota, NATS bus ve sistem keşfi.

pub mod autodiscover;
pub mod bus;
pub mod quotas;

pub use autodiscover::scan_system;
pub use bus::BusManager;
pub use quotas::{
    check_quota, limit_policy_percent, probe_quotas, quota_blocked_for, quota_exhausted_for,
    quota_matches_agent,
};
