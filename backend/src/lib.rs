//! SillyTavern Rust sidecar — library crate.
//!
//! Declares all modules and re-exports the public router builder
//! so that both the binary and integration tests can use it.

pub mod api;
pub mod config;
pub mod error;
pub mod http;
pub mod storage;
