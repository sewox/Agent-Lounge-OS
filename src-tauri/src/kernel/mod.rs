//! Görev dağıtıcı ve ajan orkestrasyon mantığı (Lounge Kernel).

pub mod dispatcher;

pub use dispatcher::{default_model_lock, Dispatcher};
