//! API endpoint modules.
//!
//! Each sub-module corresponds to a route group from the SillyTavern
//! HTTP contract. Modules are added as stubs and fleshed out in
//! subsequent migration phases.

pub mod router;

// Phase 1 — Global & Static
pub mod global;

// Phase 2 — Low-Risk Reads
pub mod thumbnails;
pub mod avatars;
pub mod backgrounds;
pub mod images;
pub mod themes;

// Phase 3 — Simple Writes
// (avatars, backgrounds, images extended with write handlers)
pub mod files;
pub mod sprites;
pub mod assets;
pub mod moving_ui;
pub mod quick_replies;

// Phase 4 — Character Reads
pub mod characters;

// Phase 5 — Character Writes
// (characters extended with write handlers)

// Phase 6 — Chats
pub mod chats;

// Phase 7 — Groups & World Info
pub mod groups;
pub mod worldinfo;

// Phase 8 — Settings, Presets, Secrets, Users, Tokenizers, Extensions, Content
pub mod settings;
pub mod presets;
pub mod secrets;
pub mod users_public;
pub mod users_private;
pub mod users_admin;
pub mod tokenizers;
pub mod extensions;
pub mod content;

// Phase 9 — Metadata, Backups, Stats, Vectors, Search, Translate, Classify, Caption
pub mod image_metadata;
pub mod backups;
pub mod stats;
pub mod vector;
pub mod search;
pub mod translate;
pub mod classify;
pub mod caption;

// Phase 10 — Data Maid
pub mod data_maid;

// Phase 11 — Providers & Backends
pub mod openai;
pub mod google;
pub mod anthropic;
pub mod openrouter;
pub mod novelai;
pub mod azure;
pub mod volcengine;
pub mod minimax;
pub mod horde;
pub mod stable_diffusion;
pub mod backends_text_completions;
pub mod backends_kobold;
pub mod backends_chat_completions;
pub mod speech;
