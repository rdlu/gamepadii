//! gamepadii core: GP2040-CE discovery and mode detection, and later the
//! web-config client and profiles.
//!
//! Layering (each layer only depends on the ones before it):
//! `discovery` → `transport` → `client` → `commands`. `commands` returns
//! plain serde data and nothing here prints; front ends own rendering.

pub mod commands;
pub mod discovery;
pub mod error;
pub mod mode;
pub mod sysfs;

pub use error::{Error, Result};
