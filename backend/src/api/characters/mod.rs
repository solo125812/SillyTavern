//! Character endpoints — Phase 4 reads + Phase 5 writes.
//!
//! Split into sub-modules for manageability:
//! - `reads`   — GET handlers (`/all`, `/get`, `/chats`)
//! - `writes`  — POST handlers (create, edit, delete, import, export, …)
//! - `helpers` — Shared utilities (formatting, deep merge, file helpers)

pub(crate) mod helpers;
mod imports;
pub(crate) mod reads;
mod writes;

#[cfg(test)]
mod tests;

// Re-export all public handlers so `router.rs` can reference
// `characters::get_all_characters`, `characters::create_character`, etc.
// without knowing the sub-module structure.
pub use reads::*;
pub use writes::*;
