//! Yerel servis API'lerinden kota, NATS bus ve sistem keşfi.

pub mod autodiscover;
pub mod bus;
pub mod quotas;

pub use autodiscover::scan_system;
pub use bus::BusManager;
pub use quotas::{probe_quotas, quota_exhausted_for};
