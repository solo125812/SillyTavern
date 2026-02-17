//! API router builder.
//!
//! Merges all route trees in a stable order and exposes [`build_router`]
//! for use by the main binary and integration tests.

use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    extract::Request,
    http::{header::CONTENT_TYPE, HeaderValue},
    middleware,
    response::Response,
    routing::{get, post},
    Router,
};
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

use crate::config::AppConfig;
use crate::http::middleware::require_user_context;

// Phase 1 handlers
use crate::api::global;

// Phase 2 handlers
use crate::api::avatars;
use crate::api::backgrounds;
use crate::api::images;
use crate::api::thumbnails;

// Phase 3 handlers
use crate::api::assets;
use crate::api::files;
use crate::api::moving_ui;
use crate::api::quick_replies;
use crate::api::sprites;
use crate::api::themes;

// Phase 4 handlers
use crate::api::characters;

// Phase 6 handlers
use crate::api::chats;

// Phase 7 handlers
use crate::api::groups;
use crate::api::worldinfo;

// Phase 8 handlers
use crate::api::content;
use crate::api::extensions;
use crate::api::presets;
use crate::api::secrets;
use crate::api::settings;
use crate::api::tokenizers;
use crate::api::users_admin;
use crate::api::users_private;
use crate::api::users_public;

// Phase 9 handlers
use crate::api::backups;
use crate::api::caption;
use crate::api::classify;
use crate::api::image_metadata;
use crate::api::search;
use crate::api::stats;
use crate::api::translate;
use crate::api::vector;

// Phase 10 handlers
use crate::api::data_maid;

// Phase 11 handlers
use crate::api::anthropic;
use crate::api::azure;
use crate::api::backends_chat_completions;
use crate::api::backends_kobold;
use crate::api::backends_text_completions;
use crate::api::google;
use crate::api::horde;
use crate::api::minimax;
use crate::api::novelai;
use crate::api::openai;
use crate::api::openrouter;
use crate::api::speech;
use crate::api::stable_diffusion;
use crate::api::volcengine;

/// Shared application state available to all handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Application configuration.
    pub config: AppConfig,
}

/// Build the complete Axum router with all route trees merged.
///
/// Route trees are merged in a stable order matching the Node/Express
/// registration order from `server-startup.js`. New route modules are
/// added as they are implemented in subsequent migration phases.
pub fn build_router(config: AppConfig) -> Router {
    let state = Arc::new(AppState { config });

    // Health check / readiness probe (no auth required)
    let health_routes = Router::new().route("/health", get(health_check));

    // Authenticated API routes — require user context from Node proxy headers
    let api_routes = Router::new()
        // Phase 1 — Global routes
        .route("/api/ping", post(global::ping_handler))
        .route("/version", get(global::version_handler))
        // Phase 2 — Low-Risk Reads
        .route("/thumbnail", get(thumbnails::thumbnail_handler))
        .route("/api/avatars/get", post(avatars::get_avatars))
        // Phase 3 — Avatars writes
        .route("/api/avatars/upload", post(avatars::upload_avatar))
        .route("/api/avatars/delete", post(avatars::delete_avatar))
        // Phase 2+3 — Backgrounds
        .route("/api/backgrounds/all", post(backgrounds::get_all_backgrounds))
        .route("/api/backgrounds/upload", post(backgrounds::upload_background))
        .route("/api/backgrounds/delete", post(backgrounds::delete_background))
        .route("/api/backgrounds/rename", post(backgrounds::rename_background))
        // Phase 2+3 — Images
        .route("/api/images/list", post(images::list_images))
        .route("/api/images/list/{folder}", post(images::list_images_with_folder))
        .route("/api/images/folders", post(images::list_image_folders))
        .route("/api/images/upload", post(images::upload_image))
        .route("/api/images/delete", post(images::delete_image))
        // Phase 3 — Files
        .route("/api/files/sanitize-filename", post(files::sanitize_filename_handler))
        .route("/api/files/upload", post(files::upload_file))
        .route("/api/files/delete", post(files::delete_file))
        .route("/api/files/verify", post(files::verify_files))
        // Phase 3 — Sprites
        .route("/api/sprites/get", get(sprites::get_sprites))
        .route("/api/sprites/delete", post(sprites::delete_sprite))
        .route("/api/sprites/upload-zip", post(sprites::upload_sprite_zip))
        .route("/api/sprites/upload", post(sprites::upload_sprite))
        // Phase 3 — Assets
        .route("/api/assets/get", post(assets::get_assets))
        .route("/api/assets/download", post(assets::download_asset))
        .route("/api/assets/delete", post(assets::delete_asset))
        .route("/api/assets/character", post(assets::character_assets))
        // Phase 3 — Themes
        .route("/api/themes/save", post(themes::save_theme))
        .route("/api/themes/delete", post(themes::delete_theme))
        // Phase 3 — Moving UI
        .route("/api/moving-ui/save", post(moving_ui::save_moving_ui))
        // Phase 3 — Quick Replies
        .route("/api/quick-replies/save", post(quick_replies::save_quick_reply))
        .route("/api/quick-replies/delete", post(quick_replies::delete_quick_reply))
        // Phase 4 — Characters (reads)
        .route("/api/characters/all", post(characters::get_all_characters))
        .route("/api/characters/get", post(characters::get_character))
        .route("/api/characters/chats", post(characters::get_character_chats))
        // Phase 5 — Characters (writes)
        .route("/api/characters/create", post(characters::create_character))
        .route("/api/characters/rename", post(characters::rename_character))
        .route("/api/characters/edit", post(characters::edit_character))
        .route("/api/characters/edit-avatar", post(characters::edit_character_avatar))
        .route("/api/characters/edit-attribute", post(characters::edit_character_attribute))
        .route("/api/characters/merge-attributes", post(characters::merge_character_attributes))
        .route("/api/characters/delete", post(characters::delete_character))
        .route("/api/characters/import", post(characters::import_character))
        .route("/api/characters/duplicate", post(characters::duplicate_character))
        .route("/api/characters/export", post(characters::export_character))
        // Phase 6 — Chats
        .route("/api/chats/save", post(chats::save_chat))
        .route("/api/chats/get", post(chats::get_chat))
        .route("/api/chats/rename", post(chats::rename_chat))
        .route("/api/chats/delete", post(chats::delete_chat))
        .route("/api/chats/export", post(chats::export_chat))
        .route("/api/chats/import", post(chats::import_chat))
        .route("/api/chats/search", post(chats::search_chat))
        .route("/api/chats/recent", post(chats::recent_chats))
        .route("/api/chats/group/import", post(chats::group_import_chat))
        .route("/api/chats/group/get", post(chats::group_get_chat))
        .route("/api/chats/group/info", post(chats::group_info_chat))
        .route("/api/chats/group/delete", post(chats::group_delete_chat))
        .route("/api/chats/group/save", post(chats::group_save_chat))
        // Phase 7 — Groups
        .route("/api/groups/all", post(groups::get_all_groups))
        .route("/api/groups/create", post(groups::create_group))
        .route("/api/groups/edit", post(groups::edit_group))
        .route("/api/groups/delete", post(groups::delete_group))
        // Phase 7 — World Info
        .route("/api/worldinfo/list", post(worldinfo::list_world_info))
        .route("/api/worldinfo/get", post(worldinfo::get_world_info))
        .route("/api/worldinfo/delete", post(worldinfo::delete_world_info))
        .route("/api/worldinfo/import", post(worldinfo::import_world_info))
        .route("/api/worldinfo/edit", post(worldinfo::edit_world_info))
        // Phase 8 — Settings
        .route("/api/settings/get", post(settings::get_settings))
        .route("/api/settings/save", post(settings::save_settings))
        .route("/api/settings/get-snapshots", post(settings::get_snapshots))
        .route("/api/settings/load-snapshot", post(settings::load_snapshot))
        .route("/api/settings/make-snapshot", post(settings::make_snapshot))
        .route("/api/settings/restore-snapshot", post(settings::restore_snapshot))
        // Phase 8 — Presets
        .route("/api/presets/save", post(presets::save_preset))
        .route("/api/presets/delete", post(presets::delete_preset))
        .route("/api/presets/restore", post(presets::restore_preset))
        // Phase 8 — Secrets
        .route("/api/secrets/write", post(secrets::write_secret))
        .route("/api/secrets/read", post(secrets::read_secrets))
        .route("/api/secrets/view", post(secrets::view_secrets))
        .route("/api/secrets/find", post(secrets::find_secret))
        .route("/api/secrets/delete", post(secrets::delete_secret))
        .route("/api/secrets/rotate", post(secrets::rotate_secret))
        .route("/api/secrets/rename", post(secrets::rename_secret))
        // Phase 8 — Users (public)
        .route("/api/users/list", post(users_public::list_users))
        .route("/api/users/login", post(users_public::login_user))
        .route("/api/users/recover-step1", post(users_public::recover_step1))
        .route("/api/users/recover-step2", post(users_public::recover_step2))
        // Phase 8 — Users (private)
        .route("/api/users/logout", post(users_private::logout_user))
        .route("/api/users/me", get(users_private::get_me))
        .route("/api/users/change-avatar", post(users_private::change_avatar))
        .route("/api/users/change-password", post(users_private::change_password))
        .route("/api/users/backup", post(users_private::backup_user))
        .route("/api/users/reset-settings", post(users_private::reset_settings))
        .route("/api/users/change-name", post(users_private::change_name))
        .route("/api/users/reset-step1", post(users_private::reset_step1))
        .route("/api/users/reset-step2", post(users_private::reset_step2))
        // Phase 8 — Users (admin)
        .route("/api/users/get", post(users_admin::admin_get_users))
        .route("/api/users/disable", post(users_admin::disable_user))
        .route("/api/users/enable", post(users_admin::enable_user))
        .route("/api/users/promote", post(users_admin::promote_user))
        .route("/api/users/demote", post(users_admin::demote_user))
        .route("/api/users/create", post(users_admin::create_user))
        .route("/api/users/delete", post(users_admin::delete_user))
        .route("/api/users/slugify", post(users_admin::slugify_text))
        // Phase 8 — Tokenizers
        .route("/api/tokenizers/llama/encode", post(tokenizers::llama_encode))
        .route("/api/tokenizers/llama/decode", post(tokenizers::llama_decode))
        .route("/api/tokenizers/nerdstash/encode", post(tokenizers::nerdstash_encode))
        .route("/api/tokenizers/nerdstash/decode", post(tokenizers::nerdstash_decode))
        .route("/api/tokenizers/nerdstash_v2/encode", post(tokenizers::nerdstash_v2_encode))
        .route("/api/tokenizers/nerdstash_v2/decode", post(tokenizers::nerdstash_v2_decode))
        .route("/api/tokenizers/mistral/encode", post(tokenizers::mistral_encode))
        .route("/api/tokenizers/mistral/decode", post(tokenizers::mistral_decode))
        .route("/api/tokenizers/yi/encode", post(tokenizers::yi_encode))
        .route("/api/tokenizers/yi/decode", post(tokenizers::yi_decode))
        .route("/api/tokenizers/gemma/encode", post(tokenizers::gemma_encode))
        .route("/api/tokenizers/gemma/decode", post(tokenizers::gemma_decode))
        .route("/api/tokenizers/jamba/encode", post(tokenizers::jamba_encode))
        .route("/api/tokenizers/jamba/decode", post(tokenizers::jamba_decode))
        .route("/api/tokenizers/gpt2/encode", post(tokenizers::gpt2_encode))
        .route("/api/tokenizers/gpt2/decode", post(tokenizers::gpt2_decode))
        .route("/api/tokenizers/claude/encode", post(tokenizers::claude_encode))
        .route("/api/tokenizers/claude/decode", post(tokenizers::claude_decode))
        .route("/api/tokenizers/llama3/encode", post(tokenizers::llama3_encode))
        .route("/api/tokenizers/llama3/decode", post(tokenizers::llama3_decode))
        .route("/api/tokenizers/qwen2/encode", post(tokenizers::qwen2_encode))
        .route("/api/tokenizers/qwen2/decode", post(tokenizers::qwen2_decode))
        .route("/api/tokenizers/command-r/encode", post(tokenizers::command_r_encode))
        .route("/api/tokenizers/command-r/decode", post(tokenizers::command_r_decode))
        .route("/api/tokenizers/command-a/encode", post(tokenizers::command_a_encode))
        .route("/api/tokenizers/command-a/decode", post(tokenizers::command_a_decode))
        .route("/api/tokenizers/nemo/encode", post(tokenizers::nemo_encode))
        .route("/api/tokenizers/nemo/decode", post(tokenizers::nemo_decode))
        .route("/api/tokenizers/deepseek/encode", post(tokenizers::deepseek_encode))
        .route("/api/tokenizers/deepseek/decode", post(tokenizers::deepseek_decode))
        .route("/api/tokenizers/openai/encode", post(tokenizers::openai_encode))
        .route("/api/tokenizers/openai/decode", post(tokenizers::openai_decode))
        .route("/api/tokenizers/openai/count", post(tokenizers::openai_count))
        .route("/api/tokenizers/remote/kobold/count", post(tokenizers::remote_kobold_count))
        .route("/api/tokenizers/remote/textgenerationwebui/encode", post(tokenizers::remote_textgenwebui_encode))
        // Phase 8 — Extensions
        .route("/api/extensions/install", post(extensions::install_extension))
        .route("/api/extensions/update", post(extensions::update_extension))
        .route("/api/extensions/branches", post(extensions::list_branches))
        .route("/api/extensions/switch", post(extensions::switch_branch))
        .route("/api/extensions/move", post(extensions::move_extension))
        .route("/api/extensions/version", post(extensions::get_version))
        .route("/api/extensions/delete", post(extensions::delete_extension))
        .route("/api/extensions/discover", get(extensions::discover_extensions))
        // Phase 8 — Content
        .route("/api/content/importURL", post(content::import_url))
        .route("/api/content/importUUID", post(content::import_uuid))
        // Phase 9 — Image Metadata
        .route("/api/image-metadata", post(image_metadata::get_image_metadata))
        .route("/api/image-metadata/all", post(image_metadata::get_all_image_metadata))
        .route("/api/image-metadata/cleanup", post(image_metadata::cleanup_image_metadata))
        // Phase 9 — Backups
        .route("/api/backups/chat/get", post(backups::get_chat_backups))
        .route("/api/backups/chat/delete", post(backups::delete_chat_backup))
        .route("/api/backups/chat/download", post(backups::download_chat_backup))
        // Phase 9 — Stats
        .route("/api/stats/get", post(stats::get_stats))
        .route("/api/stats/recreate", post(stats::recreate_stats))
        .route("/api/stats/update", post(stats::update_stats))
        // Phase 9 — Vectors
        .route("/api/vector/query", post(vector::query_vector))
        .route("/api/vector/query-multi", post(vector::query_multi_vector))
        .route("/api/vector/insert", post(vector::insert_vector))
        .route("/api/vector/list", post(vector::list_vector))
        .route("/api/vector/delete", post(vector::delete_vector))
        .route("/api/vector/purge-all", post(vector::purge_all_vectors))
        .route("/api/vector/purge", post(vector::purge_vector))
        // Phase 9 — Search
        .route("/api/search/serpapi", post(search::search_serpapi))
        .route("/api/search/transcript", post(search::search_transcript))
        .route("/api/search/searxng", post(search::search_searxng))
        .route("/api/search/tavily", post(search::search_tavily))
        .route("/api/search/koboldcpp", post(search::search_koboldcpp))
        .route("/api/search/serper", post(search::search_serper))
        .route("/api/search/zai", post(search::search_zai))
        .route("/api/search/visit", post(search::search_visit))
        // Phase 9 — Translate
        .route("/api/translate/libre", post(translate::translate_libre))
        .route("/api/translate/google", post(translate::translate_google))
        .route("/api/translate/yandex", post(translate::translate_yandex))
        .route("/api/translate/lingva", post(translate::translate_lingva))
        .route("/api/translate/deepl", post(translate::translate_deepl))
        .route("/api/translate/onering", post(translate::translate_onering))
        .route("/api/translate/deeplx", post(translate::translate_deeplx))
        .route("/api/translate/bing", post(translate::translate_bing))
        // Phase 9 — Classify
        .route("/api/extra/classify/labels", post(classify::classify_labels))
        .route("/api/extra/classify", post(classify::classify_text))
        // Phase 9 — Caption
        .route("/api/extra/caption", post(caption::caption_image))
        // Phase 10 — Data Maid
        .route("/api/data-maid/report", post(data_maid::generate_report))
        .route("/api/data-maid/finalize", post(data_maid::finalize_token))
        .route("/api/data-maid/view", get(data_maid::view_file))
        .route("/api/data-maid/delete", post(data_maid::delete_files))
        // Phase 11 — OpenAI
        .route("/api/openai/caption-image", post(openai::caption_image))
        .route("/api/openai/generate-voice", post(openai::generate_voice))
        .route("/api/openai/electronhub/generate-voice", post(openai::electronhub_generate_voice))
        .route("/api/openai/electronhub/models", post(openai::electronhub_models))
        .route("/api/openai/chutes/generate-voice", post(openai::chutes_generate_voice))
        .route("/api/openai/chutes/models/embedding", post(openai::chutes_models_embedding))
        .route("/api/openai/nanogpt/models/embedding", post(openai::nanogpt_models_embedding))
        .route("/api/openai/generate-image", post(openai::generate_image))
        .route("/api/openai/generate-video", post(openai::generate_video))
        .route("/api/openai/custom/generate-voice", post(openai::custom_generate_voice))
        .route("/api/openai/transcribe-audio", post(openai::transcribe_audio))
        .route("/api/openai/groq/transcribe-audio", post(openai::groq_transcribe_audio))
        .route("/api/openai/mistral/transcribe-audio", post(openai::mistral_transcribe_audio))
        .route("/api/openai/zai/transcribe-audio", post(openai::zai_transcribe_audio))
        .route("/api/openai/chutes/transcribe-audio", post(openai::chutes_transcribe_audio))
        // Phase 11 — Google
        .route("/api/google/caption-image", post(google::caption_image))
        .route("/api/google/list-voices", post(google::list_voices))
        .route("/api/google/generate-voice", post(google::generate_voice))
        .route("/api/google/list-native-voices", post(google::list_native_voices))
        .route("/api/google/generate-native-tts", post(google::generate_native_tts))
        .route("/api/google/generate-image", post(google::generate_image))
        .route("/api/google/generate-video", post(google::generate_video))
        // Phase 11 — Anthropic
        .route("/api/anthropic/caption-image", post(anthropic::caption_image))
        // Phase 11 — OpenRouter
        .route("/api/openrouter/models/providers", post(openrouter::models_providers))
        .route("/api/openrouter/models/multimodal", post(openrouter::models_multimodal))
        .route("/api/openrouter/models/embedding", post(openrouter::models_embedding))
        .route("/api/openrouter/models/image", post(openrouter::models_image))
        .route("/api/openrouter/image/generate", post(openrouter::image_generate))
        // Phase 11 — NovelAI
        .route("/api/novelai/status", post(novelai::status))
        .route("/api/novelai/generate", post(novelai::generate))
        .route("/api/novelai/generate-image", post(novelai::generate_image))
        .route("/api/novelai/generate-voice", post(novelai::generate_voice))
        // Phase 11 — Azure TTS
        .route("/api/azure/list", post(azure::list_voices))
        .route("/api/azure/generate", post(azure::generate_voice))
        // Phase 11 — Volcengine TTS
        .route("/api/volcengine/generate-voice", post(volcengine::generate_voice))
        // Phase 11 — MiniMax TTS
        .route("/api/minimax/generate-voice", post(minimax::generate_voice))
        // Phase 11 — AI Horde
        .route("/api/horde/text-workers", post(horde::text_workers))
        .route("/api/horde/text-models", post(horde::text_models))
        .route("/api/horde/status", post(horde::horde_status))
        .route("/api/horde/cancel-task", post(horde::cancel_task))
        .route("/api/horde/task-status", post(horde::task_status))
        .route("/api/horde/generate-text", post(horde::generate_text))
        .route("/api/horde/sd-samplers", post(horde::sd_samplers))
        .route("/api/horde/sd-models", post(horde::sd_models))
        .route("/api/horde/caption-image", post(horde::caption_image))
        .route("/api/horde/user-info", post(horde::user_info))
        .route("/api/horde/generate-image", post(horde::generate_image))
        // Phase 11 — Stable Diffusion (core)
        .route("/api/sd/ping", post(stable_diffusion::ping))
        .route("/api/sd/upscalers", post(stable_diffusion::upscalers))
        .route("/api/sd/vaes", post(stable_diffusion::vaes))
        .route("/api/sd/samplers", post(stable_diffusion::samplers))
        .route("/api/sd/schedulers", post(stable_diffusion::schedulers))
        .route("/api/sd/models", post(stable_diffusion::models))
        .route("/api/sd/get-model", post(stable_diffusion::get_model))
        .route("/api/sd/set-model", post(stable_diffusion::set_model))
        .route("/api/sd/generate", post(stable_diffusion::generate))
        .route("/api/sd/sd-next/upscalers", post(stable_diffusion::sdnext_upscalers))
        // Phase 11 — Stable Diffusion (external providers)
        .route("/api/sd/comfy/ping", post(stable_diffusion::comfy_ping))
        .route("/api/sd/together/models", post(stable_diffusion::together_models))
        .route("/api/sd/together/generate", post(stable_diffusion::together_generate))
        .route("/api/sd/stability/generate", post(stable_diffusion::stability_generate))
        .route("/api/sd/pollinations/generate", post(stable_diffusion::pollinations_generate))
        .route("/api/sd/huggingface/generate", post(stable_diffusion::huggingface_generate))
        .route("/api/sd/electronhub/models", post(stable_diffusion::electronhub_models))
        .route("/api/sd/electronhub/generate", post(stable_diffusion::electronhub_generate))
        .route("/api/sd/chutes/models", post(stable_diffusion::chutes_models))
        .route("/api/sd/chutes/generate", post(stable_diffusion::chutes_generate))
        .route("/api/sd/generic/generate", post(stable_diffusion::generic_proxy_generate))
        // Phase 11 — Speech
        .route("/api/speech/recognize", post(speech::recognize))
        .route("/api/speech/synthesize", post(speech::synthesize))
        .route("/api/speech/pollinations/voices", post(speech::pollinations_voices))
        .route("/api/speech/pollinations/generate", post(speech::pollinations_generate))
        .route("/api/speech/elevenlabs/voices", post(speech::elevenlabs_voices))
        .route("/api/speech/elevenlabs/voice-settings", post(speech::elevenlabs_voice_settings))
        .route("/api/speech/elevenlabs/synthesize", post(speech::elevenlabs_synthesize))
        .route("/api/speech/elevenlabs/history", post(speech::elevenlabs_history))
        .route("/api/speech/elevenlabs/history-audio", post(speech::elevenlabs_history_audio))
        .route("/api/speech/elevenlabs/voices/add", post(speech::elevenlabs_voices_add))
        .route("/api/speech/elevenlabs/recognize", post(speech::elevenlabs_recognize))
        // Phase 11 — Backends: Text Completions
        .route("/api/backends/text-completions/status", post(backends_text_completions::tc_status))
        .route("/api/backends/text-completions/props", post(backends_text_completions::tc_props))
        .route("/api/backends/text-completions/generate", post(backends_text_completions::tc_generate))
        // Phase 11 — Backends: KoboldAI
        .route("/api/backends/kobold/generate", post(backends_kobold::generate))
        .route("/api/backends/kobold/status", post(backends_kobold::status))
        .route("/api/backends/kobold/transcribe-audio", post(backends_kobold::transcribe_audio))
        .route("/api/backends/kobold/embed", post(backends_kobold::embed))
        // Phase 11 — Backends: Chat Completions
        .route("/api/backends/chat-completions/status", post(backends_chat_completions::cc_status))
        .route("/api/backends/chat-completions/bias", post(backends_chat_completions::cc_bias))
        .route("/api/backends/chat-completions/generate", post(backends_chat_completions::cc_generate))
        .route("/api/backends/chat-completions/process", post(backends_chat_completions::cc_process))
        .route("/api/backends/chat-completions/multimodal-models/pollinations", post(backends_chat_completions::mm_pollinations))
        .route("/api/backends/chat-completions/multimodal-models/aimlapi", post(backends_chat_completions::mm_aimlapi))
        .route("/api/backends/chat-completions/multimodal-models/nanogpt", post(backends_chat_completions::mm_nanogpt))
        .route("/api/backends/chat-completions/multimodal-models/electronhub", post(backends_chat_completions::mm_electronhub))
        .route("/api/backends/chat-completions/multimodal-models/chutes", post(backends_chat_completions::mm_chutes))
        .route("/api/backends/chat-completions/multimodal-models/mistral", post(backends_chat_completions::mm_mistral))
        .route("/api/backends/chat-completions/multimodal-models/xai", post(backends_chat_completions::mm_xai))
        .route("/api/backends/chat-completions/multimodal-models/moonshot", post(backends_chat_completions::mm_moonshot))
        // Match Node's multer field size limit (500MB) for multipart uploads.
        .layer(DefaultBodyLimit::max(500 * 1024 * 1024))
        .layer(middleware::from_fn(require_user_context));

    // Merge all route trees
    Router::new()
        .merge(health_routes)
        .merge(api_routes)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(force_version_content_type))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            add_backend_header,
        ))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Internal handlers
// ---------------------------------------------------------------------------

/// Health check endpoint — returns 200 OK.
/// Not proxied from Node; used for sidecar readiness probes.
async fn health_check() -> &'static str {
    "ok"
}

/// Middleware to force `Content-Type: application/json; charset=utf-8` on
/// the `/version` endpoint. This ensures the compression layer doesn't
/// strip or alter the content-type header.
async fn force_version_content_type(req: Request, next: middleware::Next) -> Response {
    let is_version = req.uri().path() == "/version";
    let mut response = next.run(req).await;
    if is_version {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
    }
    response
}

/// Middleware to attach `x-st-backend: rust` header when enabled.
async fn add_backend_header(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: Request,
    next: middleware::Next,
) -> Response {
    let mut response = next.run(req).await;
    if state.config.backend_header {
        response.headers_mut().insert(
            "x-st-backend",
            HeaderValue::from_static("rust"),
        );
    }
    response
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThumbnailDimensions;
    use axum::{body::Body, http::{Request, StatusCode}};
    use std::path::PathBuf;
    use tower::ServiceExt;

    fn test_config(backend_header: bool) -> AppConfig {
        AppConfig {
            data_root: PathBuf::from("./data"),
            listen_address: "127.0.0.1:0".to_string(),
            server_directory: PathBuf::from("."),
            log_level: "info".to_string(),
            backend_header,
            thumbnails_enabled: true,
            thumbnail_dimensions: ThumbnailDimensions {
                bg: (160, 90),
                avatar: (96, 144),
                persona: (96, 144),
            },
            cache_buster_enabled: false,
            cache_buster_user_agent_pattern: String::new(),
            lazy_load_characters: false,
            thumbnail_quality: 95,
            thumbnail_pngformat: false,
            chat_backup_enabled: true,
            chat_backup_max_total: -1,
            chat_backup_throttle_ms: 10_000,
            chat_backup_check_integrity: true,
            chat_backup_num_per_chat: 50,
            enable_extensions: true,
            enable_extensions_auto_update: true,
            enable_accounts: false,
            allow_keys_exposure: false,
            enable_discreet_login: false,
            whitelist_import_domains: Vec::new(),
            prefer_real_ip_header: false,
        }
    }

    #[tokio::test]
    async fn adds_backend_header_when_enabled() {
        let app = build_router(test_config(true));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let header = response
            .headers()
            .get("x-st-backend")
            .and_then(|v| v.to_str().ok());
        assert_eq!(header, Some("rust"));
    }

    #[tokio::test]
    async fn skips_backend_header_when_disabled() {
        let app = build_router(test_config(false));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-st-backend").is_none());
    }
}
