//! The gamepadii front ends: command line parsing and rendering, and the
//! TUI. Everything
//! lives here so tests can reach it; `main.rs` only dispatches.

pub mod cli;
pub mod output;
pub mod progress;
pub mod tui;
