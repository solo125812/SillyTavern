//! Application configuration for the Rust sidecar.
//!
//! Configuration is loaded from environment variables or a config file.
//! The sidecar shares the same `DATA_ROOT` as the Node server.

use std::path::PathBuf;

use serde::Deserialize;

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// Root directory for user data storage (same as Node's `dataRoot`).
    #[serde(default = "default_data_root")]
    pub data_root: PathBuf,

    /// Address to listen on (e.g. "127.0.0.1:5050").
    #[serde(default = "default_listen_address")]
    pub listen_address: String,

    /// Path to the SillyTavern server directory (project root).
    #[serde(default = "default_server_directory")]
    pub server_directory: PathBuf,

    /// Log level filter (e.g. "info", "debug", "trace").
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Whether to attach `x-st-backend: rust` to Phase 1 responses.
    #[serde(default = "default_backend_header")]
    pub backend_header: bool,

    /// Whether thumbnails are enabled (mirrors `thumbnails.enabled`).
    #[serde(default = "default_thumbnails_enabled")]
    pub thumbnails_enabled: bool,

    /// Thumbnail dimensions loaded from config (`thumbnails.dimensions`).
    #[serde(default = "default_thumbnail_dimensions")]
    pub thumbnail_dimensions: ThumbnailDimensions,

    /// Whether cache buster is enabled (mirrors `cacheBuster.enabled`).
    #[serde(default = "default_cache_buster_enabled")]
    pub cache_buster_enabled: bool,

    /// Optional user agent regex for cache buster (mirrors `cacheBuster.userAgentPattern`).
    #[serde(default = "default_cache_buster_user_agent_pattern")]
    pub cache_buster_user_agent_pattern: String,

    /// Whether to return shallow character entries (mirrors `performance.lazyLoadCharacters`).
    #[serde(default = "default_lazy_load_characters")]
    pub lazy_load_characters: bool,

    /// Thumbnail JPEG quality (1-100, mirrors `thumbnails.quality`, default 95).
    #[serde(default = "default_thumbnail_quality")]
    pub thumbnail_quality: u8,

    /// Whether thumbnails use PNG format (mirrors `thumbnails.format` == 'png').
    #[serde(default = "default_thumbnail_pngformat")]
    pub thumbnail_pngformat: bool,

    // -- Chat backup configuration (Phase 6) --

    /// Whether chat backups are enabled (mirrors `backups.chat.enabled`).
    #[serde(default = "default_chat_backup_enabled")]
    pub chat_backup_enabled: bool,

    /// Maximum total chat backups across all chats. -1 = unlimited.
    /// Mirrors `backups.chat.maxTotalBackups`.
    #[serde(default = "default_chat_backup_max_total")]
    pub chat_backup_max_total: i32,

    /// Throttle interval for chat backups in milliseconds.
    /// Mirrors `backups.chat.throttleInterval`.
    #[serde(default = "default_chat_backup_throttle_ms")]
    pub chat_backup_throttle_ms: u64,

    /// Whether to check chat integrity before saving.
    /// Mirrors `backups.chat.checkIntegrity`.
    #[serde(default = "default_chat_backup_check_integrity")]
    pub chat_backup_check_integrity: bool,

    /// Number of backups to keep per chat.
    /// Mirrors `backups.common.numberOfBackups`.
    #[serde(default = "default_chat_backup_num_per_chat")]
    pub chat_backup_num_per_chat: usize,

    // -- Phase 8 configuration --

    /// Whether extensions are enabled (mirrors `enableExtensions`).
    #[serde(default = "default_enable_extensions")]
    pub enable_extensions: bool,

    /// Whether extension auto-update is enabled (mirrors `enableExtensionsAutoUpdate`).
    #[serde(default = "default_enable_extensions_auto_update")]
    pub enable_extensions_auto_update: bool,

    /// Whether user accounts are enabled (mirrors `enableUserAccounts`).
    #[serde(default = "default_enable_accounts")]
    pub enable_accounts: bool,

    /// Whether secret key values may be exposed to the client (mirrors `allowKeysExposure`).
    #[serde(default = "default_allow_keys_exposure")]
    pub allow_keys_exposure: bool,

    /// Whether discreet login mode is active (mirrors `enableDiscreetLogin`).
    #[serde(default = "default_enable_discreet_login")]
    pub enable_discreet_login: bool,

    /// Whitelisted domains for generic content import (mirrors `whitelistImportDomains`).
    #[serde(default = "default_whitelist_import_domains")]
    pub whitelist_import_domains: Vec<String>,

    /// Prefer `x-real-ip` header for rate limiting (mirrors `rateLimiting.preferRealIpHeader`).
    #[serde(default = "default_prefer_real_ip_header")]
    pub prefer_real_ip_header: bool,
}

fn default_data_root() -> PathBuf {
    PathBuf::from("./data")
}

fn default_listen_address() -> String {
    "127.0.0.1:5050".to_string()
}

fn default_server_directory() -> PathBuf {
    PathBuf::from(".")
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_backend_header() -> bool {
    false
}

fn default_thumbnails_enabled() -> bool {
    true
}

fn default_thumbnail_dimensions() -> ThumbnailDimensions {
    ThumbnailDimensions {
        bg: (160, 90),
        avatar: (96, 144),
        persona: (96, 144),
    }
}

fn default_cache_buster_enabled() -> bool {
    false
}

fn default_cache_buster_user_agent_pattern() -> String {
    String::new()
}

fn default_lazy_load_characters() -> bool {
    false
}

fn default_thumbnail_quality() -> u8 {
    95
}

fn default_thumbnail_pngformat() -> bool {
    false
}

fn default_chat_backup_enabled() -> bool {
    true
}

fn default_chat_backup_max_total() -> i32 {
    -1
}

fn default_chat_backup_throttle_ms() -> u64 {
    10_000
}

fn default_chat_backup_check_integrity() -> bool {
    true
}

fn default_chat_backup_num_per_chat() -> usize {
    50
}

fn default_enable_extensions() -> bool {
    true
}

fn default_enable_extensions_auto_update() -> bool {
    true
}

fn default_enable_accounts() -> bool {
    false
}

fn default_allow_keys_exposure() -> bool {
    false
}

fn default_enable_discreet_login() -> bool {
    false
}

fn default_whitelist_import_domains() -> Vec<String> {
    Vec::new()
}

fn default_prefer_real_ip_header() -> bool {
    false
}

/// Thumbnail dimension configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ThumbnailDimensions {
    pub bg: (u32, u32),
    pub avatar: (u32, u32),
    pub persona: (u32, u32),
}

/// Configuration file structure (subset of config.yaml relevant to sidecar).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    #[serde(default = "default_data_root_str")]
    data_root: String,

    #[serde(default)]
    rust_proxy: Option<RustProxyConfig>,

    #[serde(default)]
    thumbnails: Option<ThumbnailsConfig>,

    #[serde(default)]
    cache_buster: Option<CacheBusterConfig>,

    #[serde(default)]
    performance: Option<PerformanceConfig>,

    #[serde(default)]
    backups: Option<BackupsConfig>,

    #[serde(default)]
    enable_extensions: Option<bool>,

    #[serde(default)]
    enable_extensions_auto_update: Option<bool>,

    #[serde(default)]
    enable_user_accounts: Option<bool>,

    #[serde(default)]
    allow_keys_exposure: Option<bool>,

    #[serde(default)]
    enable_discreet_login: Option<bool>,

    #[serde(default)]
    whitelist_import_domains: Option<Vec<String>>,

    #[serde(default)]
    rate_limiting: Option<RateLimitingConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RustProxyConfig {
    #[serde(default)]
    backend_header: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailsConfig {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    dimensions: Option<ThumbnailDimensionsConfig>,
    #[serde(default)]
    quality: Option<u8>,
    #[serde(default)]
    format: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheBusterConfig {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    user_agent_pattern: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PerformanceConfig {
    #[serde(default)]
    lazy_load_characters: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupsConfig {
    #[serde(default)]
    common: Option<BackupsCommonConfig>,
    #[serde(default)]
    chat: Option<BackupsChatConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupsCommonConfig {
    #[serde(default)]
    number_of_backups: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupsChatConfig {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    check_integrity: Option<bool>,
    #[serde(default)]
    max_total_backups: Option<i32>,
    #[serde(default)]
    throttle_interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitingConfig {
    #[serde(default)]
    prefer_real_ip_header: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ThumbnailDimensionsConfig {
    #[serde(default)]
    bg: Option<Vec<u32>>,
    #[serde(default)]
    avatar: Option<Vec<u32>>,
    #[serde(default)]
    persona: Option<Vec<u32>>,
}

fn default_data_root_str() -> String {
    "./data".to_string()
}

impl AppConfig {
    /// Load configuration from environment variables, falling back to
    /// the SillyTavern config.yaml file if available.
    pub fn load() -> anyhow::Result<Self> {
        // Priority: environment variables > config file > defaults
        let config_file = Self::read_config_file();

        let data_root = std::env::var("ST_DATA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                // Try to read from config.yaml
                config_file
                    .as_ref()
                    .map(|cfg| PathBuf::from(&cfg.data_root))
                    .unwrap_or_else(default_data_root)
            });

        let listen_address = std::env::var("ST_LISTEN_ADDRESS")
            .unwrap_or_else(|_| default_listen_address());

        let server_directory = std::env::var("ST_SERVER_DIRECTORY")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_server_directory());

        let log_level = std::env::var("ST_LOG_LEVEL")
            .or_else(|_| std::env::var("RUST_LOG"))
            .unwrap_or_else(|_| default_log_level());

        let backend_header = std::env::var("ST_BACKEND_HEADER")
            .ok()
            .as_deref()
            .and_then(parse_env_bool)
            .unwrap_or_else(|| {
                config_file
                    .as_ref()
                    .and_then(|cfg| cfg.rust_proxy.as_ref())
                    .map(|proxy| proxy.backend_header)
                    .unwrap_or_else(default_backend_header)
            });

        let thumbnails_enabled = config_file
            .as_ref()
            .and_then(|cfg| cfg.thumbnails.as_ref())
            .and_then(|thumb| thumb.enabled)
            .unwrap_or_else(default_thumbnails_enabled);

        let thumbnail_dimensions = ThumbnailDimensions::from_config(
            config_file.as_ref().and_then(|cfg| cfg.thumbnails.as_ref()),
        );

        let cache_buster_enabled = config_file
            .as_ref()
            .and_then(|cfg| cfg.cache_buster.as_ref())
            .and_then(|cb| cb.enabled)
            .unwrap_or_else(default_cache_buster_enabled);

        let cache_buster_user_agent_pattern = config_file
            .as_ref()
            .and_then(|cfg| cfg.cache_buster.as_ref())
            .and_then(|cb| cb.user_agent_pattern.clone())
            .unwrap_or_else(default_cache_buster_user_agent_pattern);

        let lazy_load_characters = config_file
            .as_ref()
            .and_then(|cfg| cfg.performance.as_ref())
            .and_then(|perf| perf.lazy_load_characters)
            .unwrap_or_else(default_lazy_load_characters);

        let thumbnail_quality = config_file
            .as_ref()
            .and_then(|cfg| cfg.thumbnails.as_ref())
            .and_then(|t| t.quality)
            .map(|q| q.clamp(1, 100))
            .unwrap_or_else(default_thumbnail_quality);

        let thumbnail_pngformat = config_file
            .as_ref()
            .and_then(|cfg| cfg.thumbnails.as_ref())
            .and_then(|t| t.format.as_deref())
            .map(|f| f.trim().eq_ignore_ascii_case("png"))
            .unwrap_or_else(default_thumbnail_pngformat);

        // Chat backup config
        let chat_backup_enabled = config_file
            .as_ref()
            .and_then(|cfg| cfg.backups.as_ref())
            .and_then(|b| b.chat.as_ref())
            .and_then(|c| c.enabled)
            .unwrap_or_else(default_chat_backup_enabled);

        let chat_backup_max_total = config_file
            .as_ref()
            .and_then(|cfg| cfg.backups.as_ref())
            .and_then(|b| b.chat.as_ref())
            .and_then(|c| c.max_total_backups)
            .unwrap_or_else(default_chat_backup_max_total);

        let chat_backup_throttle_ms = config_file
            .as_ref()
            .and_then(|cfg| cfg.backups.as_ref())
            .and_then(|b| b.chat.as_ref())
            .and_then(|c| c.throttle_interval)
            .unwrap_or_else(default_chat_backup_throttle_ms);

        let chat_backup_check_integrity = config_file
            .as_ref()
            .and_then(|cfg| cfg.backups.as_ref())
            .and_then(|b| b.chat.as_ref())
            .and_then(|c| c.check_integrity)
            .unwrap_or_else(default_chat_backup_check_integrity);

        let chat_backup_num_per_chat = config_file
            .as_ref()
            .and_then(|cfg| cfg.backups.as_ref())
            .and_then(|b| b.common.as_ref())
            .and_then(|c| c.number_of_backups)
            .unwrap_or_else(default_chat_backup_num_per_chat);

        // Phase 8 config
        let enable_extensions = config_file
            .as_ref()
            .and_then(|cfg| cfg.enable_extensions)
            .unwrap_or_else(default_enable_extensions);

        let enable_extensions_auto_update = config_file
            .as_ref()
            .and_then(|cfg| cfg.enable_extensions_auto_update)
            .unwrap_or_else(default_enable_extensions_auto_update);

        let enable_accounts = config_file
            .as_ref()
            .and_then(|cfg| cfg.enable_user_accounts)
            .unwrap_or_else(default_enable_accounts);

        let allow_keys_exposure = config_file
            .as_ref()
            .and_then(|cfg| cfg.allow_keys_exposure)
            .unwrap_or_else(default_allow_keys_exposure);

        let enable_discreet_login = config_file
            .as_ref()
            .and_then(|cfg| cfg.enable_discreet_login)
            .unwrap_or_else(default_enable_discreet_login);

        let whitelist_import_domains = config_file
            .as_ref()
            .and_then(|cfg| cfg.whitelist_import_domains.clone())
            .unwrap_or_else(default_whitelist_import_domains);

        let prefer_real_ip_header = config_file
            .as_ref()
            .and_then(|cfg| cfg.rate_limiting.as_ref())
            .and_then(|rl| rl.prefer_real_ip_header)
            .unwrap_or_else(default_prefer_real_ip_header);

        let config = AppConfig {
            data_root,
            listen_address,
            server_directory,
            log_level,
            backend_header,
            thumbnails_enabled,
            thumbnail_dimensions,
            cache_buster_enabled,
            cache_buster_user_agent_pattern,
            lazy_load_characters,
            thumbnail_quality,
            thumbnail_pngformat,
            chat_backup_enabled,
            chat_backup_max_total,
            chat_backup_throttle_ms,
            chat_backup_check_integrity,
            chat_backup_num_per_chat,
            enable_extensions,
            enable_extensions_auto_update,
            enable_accounts,
            allow_keys_exposure,
            enable_discreet_login,
            whitelist_import_domains,
            prefer_real_ip_header,
        };

        // Validate that data_root exists
        if !config.data_root.exists() {
            tracing::warn!(
                data_root = %config.data_root.display(),
                "Data root directory does not exist — it will be created when needed"
            );
        }

        Ok(config)
    }

    /// Attempt to read the SillyTavern config.yaml.
    fn read_config_file() -> Option<ConfigFile> {
        let config_path = std::env::var("ST_CONFIG_PATH")
            .unwrap_or_else(|_| "./config.yaml".to_string());

        let contents = std::fs::read_to_string(&config_path).ok()?;
        let parsed: ConfigFile = serde_yaml::from_str(&contents).ok()?;
        Some(parsed)
    }
}

impl ThumbnailDimensions {
    fn from_config(config: Option<&ThumbnailsConfig>) -> Self {
        let defaults = default_thumbnail_dimensions();
        let dims = config.and_then(|cfg| cfg.dimensions.as_ref());

        let bg = parse_dimensions(dims.and_then(|d| d.bg.as_ref()), defaults.bg);
        let avatar = parse_dimensions(dims.and_then(|d| d.avatar.as_ref()), defaults.avatar);
        let persona = parse_dimensions(dims.and_then(|d| d.persona.as_ref()), defaults.persona);

        ThumbnailDimensions { bg, avatar, persona }
    }
}

fn parse_dimensions(values: Option<&Vec<u32>>, fallback: (u32, u32)) -> (u32, u32) {
    match values {
        Some(dim) if dim.len() >= 2 => (dim[0], dim[1]),
        _ => fallback,
    }
}

fn parse_env_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = AppConfig {
            data_root: default_data_root(),
            listen_address: default_listen_address(),
            server_directory: default_server_directory(),
            log_level: default_log_level(),
            backend_header: default_backend_header(),
            thumbnails_enabled: default_thumbnails_enabled(),
            thumbnail_dimensions: default_thumbnail_dimensions(),
            cache_buster_enabled: default_cache_buster_enabled(),
            cache_buster_user_agent_pattern: default_cache_buster_user_agent_pattern(),
            lazy_load_characters: default_lazy_load_characters(),
            thumbnail_quality: default_thumbnail_quality(),
            thumbnail_pngformat: default_thumbnail_pngformat(),
            chat_backup_enabled: default_chat_backup_enabled(),
            chat_backup_max_total: default_chat_backup_max_total(),
            chat_backup_throttle_ms: default_chat_backup_throttle_ms(),
            chat_backup_check_integrity: default_chat_backup_check_integrity(),
            chat_backup_num_per_chat: default_chat_backup_num_per_chat(),
            enable_extensions: default_enable_extensions(),
            enable_extensions_auto_update: default_enable_extensions_auto_update(),
            enable_accounts: default_enable_accounts(),
            allow_keys_exposure: default_allow_keys_exposure(),
            enable_discreet_login: default_enable_discreet_login(),
            whitelist_import_domains: default_whitelist_import_domains(),
            prefer_real_ip_header: default_prefer_real_ip_header(),
        };

        assert_eq!(config.listen_address, "127.0.0.1:5050");
        assert_eq!(config.data_root, PathBuf::from("./data"));
        assert!(config.thumbnails_enabled);
        assert_eq!(config.thumbnail_dimensions.bg, (160, 90));
        assert_eq!(config.thumbnail_quality, 95);
        assert!(!config.thumbnail_pngformat);
        assert!(config.chat_backup_enabled);
        assert_eq!(config.chat_backup_max_total, -1);
        assert_eq!(config.chat_backup_throttle_ms, 10_000);
        assert!(config.chat_backup_check_integrity);
        assert_eq!(config.chat_backup_num_per_chat, 50);
    }
}
