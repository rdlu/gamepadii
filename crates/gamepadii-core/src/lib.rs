//! gamepadii core: GP2040-CE discovery and mode detection, the web-config
//! client, and later profiles.
//!
//! Layering (each layer only depends on the ones before it):
//! `discovery` → `transport` / `channel` → `client` → `commands` / `flow`. `commands` returns
//! plain serde data and nothing here prints; front ends own rendering.

pub mod api;
pub mod buttons;
pub mod cache;
pub mod channel;
pub mod client;
pub mod commands;
pub mod discovery;
pub mod error;
pub mod flow;
pub mod mode;
pub mod sim;
pub mod sysfs;
pub mod timestamp;
pub mod transport;
pub mod wait;

pub use error::{Error, Result};
