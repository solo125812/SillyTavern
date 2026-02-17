use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::Json;
use regex::Regex;
use serde_json::{Map, Value};
use tracing::warn;
use zip::ZipArchive;

use crate::api::router::AppState;
use crate::storage::atomic::atomic_write_file;
use crate::storage::paths::UserDirectories;
use crate::storage::png;
use crate::storage::sanitize::{
    sanitize_character_name, sanitize_filename, sanitize_filename_strip, SAFE_CHARACTER_REPLACEMENT,
};

use super::helpers::*;

const CHARX_EMBEDDED_URI_PREFIXES: [&str; 3] = ["embeded://", "embedded://", "__asset:"];
const CHARX_IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "webp", "gif", "apng", "avif", "bmp", "jfif",
];

const ZIP_SIGNATURE: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];

#[derive(Clone)]
struct CharXAssetRaw {
    type_name: String,
    name: String,
    ext: String,
    zip_path: String,
    order: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharXStorageCategory {
    Sprite,
    Background,
    Misc,
}

#[derive(Clone)]
struct CharXAsset {
    zip_path: String,
    ext: String,
    storage: CharXStorageCategory,
    base_name: String,
}

struct CharXParseResult {
    card: Value,
    avatar: Option<Vec<u8>>,
    assets: Vec<CharXAsset>,
    buffers: HashMap<String, Vec<u8>>,
}

struct ByafImage {
    filename: String,
    image: Vec<u8>,
    label: String,
}

struct ByafChatBackground {
    name: String,
    data: Vec<u8>,
    paths: Vec<String>,
}

struct ByafParseResult {
    card: Value,
    images: Vec<ByafImage>,
    scenarios: Vec<Value>,
    chat_backgrounds: Vec<ByafChatBackground>,
    character: Value,
}

fn zip_payload(data: &[u8]) -> Vec<u8> {
    if let Some(idx) = data.windows(4).position(|w| w == ZIP_SIGNATURE) {
        if idx > 0 {
            return data[idx..].to_vec();
        }
    }
    data.to_vec()
}

fn normalize_zip_entry_path(entry_name: &str) -> Option<String> {
    let mut normalized = entry_name.replace('\\', "/").trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    while normalized.starts_with("./") {
        normalized = normalized[2..].to_string();
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.is_empty() {
                return None;
            }
            parts.pop();
            continue;
        }
        parts.push(part);
    }

    if parts.is_empty() {
        return None;
    }

    let mut result = parts.join("/");
    if result.starts_with('/') {
        result = result.trim_start_matches('/').to_string();
    }

    Some(result)
}

fn extract_file_from_zip_buffer(archive_data: &[u8], suffix: &str) -> Option<Vec<u8>> {
    let zip_bytes = zip_payload(archive_data);
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = ZipArchive::new(cursor).ok()?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).ok()?;
        let name = entry.name().to_string();
        if name.starts_with("__MACOSX") {
            continue;
        }
        if name.ends_with(suffix) {
            let mut data = Vec::new();
            if entry.read_to_end(&mut data).is_ok() {
                return Some(data);
            }
        }
    }
    None
}

fn extract_files_from_zip_buffer(archive_data: &[u8], file_names: &[String]) -> HashMap<String, Vec<u8>> {
    let mut targets = HashSet::new();
    for name in file_names {
        if let Some(normalized) = normalize_zip_entry_path(name) {
            targets.insert(normalized);
        }
    }

    let mut results = HashMap::new();
    if targets.is_empty() {
        return results;
    }

    let zip_bytes = zip_payload(archive_data);
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = match ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(_) => return results,
    };

    for i in 0..archive.len() {
        if targets.is_empty() {
            break;
        }
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().to_string();
        let normalized = match normalize_zip_entry_path(&name) {
            Some(n) => n,
            None => continue,
        };
        if !targets.contains(&normalized) {
            continue;
        }
        let mut data = Vec::new();
        if entry.read_to_end(&mut data).is_ok() {
            results.insert(normalized.clone(), data);
            targets.remove(&normalized);
        }
    }

    results
}

fn ensure_directory(dir_path: &Path) -> bool {
    if !dir_path.exists() {
        if fs::create_dir_all(dir_path).is_err() {
            return false;
        }
    } else if !dir_path.is_dir() {
        warn!("ensure_directory: Path {:?} exists and is not a directory.", dir_path);
        return false;
    }
    true
}

fn delete_existing_by_base_name(dir_path: &Path, base_name: &str) {
    if let Ok(entries) = fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if !file_type.is_file() {
                    continue;
                }
            }
            let entry_path = entry.path();
            let stem = entry_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if stem == base_name {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn is_charx_image_ext(ext: &str) -> bool {
    let ext = ext.to_lowercase();
    CHARX_IMAGE_EXTENSIONS.iter().any(|e| *e == ext)
}

fn is_charx_sprite_type(ty: &str) -> bool {
    matches!(ty, "emotion" | "expression")
}

fn is_charx_background_type(ty: &str) -> bool {
    ty == "background"
}

fn get_embedded_zip_path_from_uri(uri: &str) -> Option<String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    for prefix in CHARX_EMBEDDED_URI_PREFIXES {
        if lower.starts_with(prefix) {
            let raw_path = trimmed[prefix.len()..].to_string();
            return normalize_zip_entry_path(&raw_path);
        }
    }
    None
}

fn derive_charx_asset_extension(asset_ext: &str, zip_path: &str) -> String {
    let meta_ext = asset_ext.trim().trim_start_matches('.').to_lowercase();
    if !meta_ext.is_empty() {
        return meta_ext;
    }
    Path::new(zip_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn strip_trailing_image_extension(name: &str, expected_ext: &str) -> String {
    if name.is_empty() {
        return name.to_string();
    }
    let lower = name.to_lowercase();
    if !expected_ext.is_empty() && lower.ends_with(&format!(".{}", expected_ext)) {
        return name[..name.len().saturating_sub(expected_ext.len() + 1)].to_string();
    }
    for ext in CHARX_IMAGE_EXTENSIONS {
        if lower.ends_with(&format!(".{}", ext)) {
            return name[..name.len().saturating_sub(ext.len() + 1)].to_string();
        }
    }
    name.to_string()
}

fn get_charx_asset_base_name(name: &str, fallback: &str, use_hyphens: bool) -> String {
    let cleaned = name.trim();
    if cleaned.is_empty() {
        return fallback.to_lowercase();
    }
    let sep = if use_hyphens { '-' } else { '_' };
    let mut base = String::new();
    let mut prev_sep = false;
    for ch in cleaned.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            base.push(lower);
            prev_sep = false;
        } else if !prev_sep {
            base.push(sep);
            prev_sep = true;
        }
    }
    while base.starts_with(sep) {
        base.remove(0);
    }
    while base.ends_with(sep) {
        base.pop();
    }
    if base.is_empty() {
        base = fallback.to_lowercase();
    }
    let sanitized = sanitize_filename_strip(&base);
    if sanitized.is_empty() {
        fallback.to_lowercase()
    } else {
        sanitized.to_lowercase()
    }
}

fn collect_charx_assets(card: &Value) -> Vec<CharXAssetRaw> {
    let assets = card
        .get("data")
        .and_then(|d| d.get("assets"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut results = Vec::new();
    for (index, asset) in assets.iter().enumerate() {
        let uri = asset.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        let zip_path = match get_embedded_zip_path_from_uri(uri) {
            Some(p) => p,
            None => continue,
        };
        let ext = asset.get("ext").and_then(|v| v.as_str()).unwrap_or("");
        let ty = asset
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let name = asset
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let derived_ext = derive_charx_asset_extension(ext, &zip_path);

        results.push(CharXAssetRaw {
            type_name: ty,
            name,
            ext: derived_ext,
            zip_path,
            order: index,
        });
    }
    results
}

fn pick_charx_icon_asset(assets: &[CharXAssetRaw]) -> Option<CharXAssetRaw> {
    let mut icons: Vec<CharXAssetRaw> = assets
        .iter()
        .filter(|asset| asset.type_name == "icon" && is_charx_image_ext(&asset.ext))
        .cloned()
        .collect();
    if icons.is_empty() {
        return None;
    }
    if let Some(main) = icons
        .iter()
        .find(|asset| asset.name.to_lowercase() == "main")
    {
        return Some(main.clone());
    }
    Some(icons.remove(0))
}

fn map_charx_assets_for_storage(assets: &[CharXAssetRaw]) -> Vec<CharXAsset> {
    let mut mapped = Vec::new();
    for asset in assets {
        if !is_charx_image_ext(&asset.ext) {
            continue;
        }
        if asset.type_name == "icon" || asset.type_name == "user_icon" {
            continue;
        }
        let storage = if is_charx_sprite_type(&asset.type_name) {
            CharXStorageCategory::Sprite
        } else if is_charx_background_type(&asset.type_name) {
            CharXStorageCategory::Background
        } else {
            CharXStorageCategory::Misc
        };
        let use_hyphens = storage == CharXStorageCategory::Sprite;
        let name_without_ext = strip_trailing_image_extension(&asset.name, &asset.ext);
        let base_name = get_charx_asset_base_name(
            &name_without_ext,
            &format!(
                "{}-{}",
                match storage {
                    CharXStorageCategory::Sprite => "sprite",
                    CharXStorageCategory::Background => "background",
                    CharXStorageCategory::Misc => "misc",
                },
                asset.order
            ),
            use_hyphens,
        );
        mapped.push(CharXAsset {
            zip_path: asset.zip_path.clone(),
            ext: asset.ext.clone(),
            storage,
            base_name,
        });
    }
    mapped
}

fn parse_charx(data: &[u8]) -> Result<CharXParseResult, String> {
    let card_buffer = extract_file_from_zip_buffer(data, "card.json")
        .ok_or_else(|| "Failed to extract card.json from CharX file".to_string())?;
    let card: Value = serde_json::from_slice(&card_buffer)
        .map_err(|_| "Invalid CharX card file".to_string())?;
    if card.get("spec").is_none() {
        return Err("Invalid CharX card file: missing spec field".to_string());
    }

    let assets = collect_charx_assets(&card);
    let icon_asset = pick_charx_icon_asset(&assets);
    let auxiliary_assets = map_charx_assets_for_storage(&assets);

    let mut archive_paths = HashSet::new();
    if let Some(icon) = &icon_asset {
        archive_paths.insert(icon.zip_path.clone());
    }
    for asset in &auxiliary_assets {
        archive_paths.insert(asset.zip_path.clone());
    }

    let buffers = extract_files_from_zip_buffer(data, &archive_paths.into_iter().collect::<Vec<_>>());
    let avatar = icon_asset
        .and_then(|icon| buffers.get(&icon.zip_path).cloned());

    Ok(CharXParseResult {
        card,
        avatar,
        assets: auxiliary_assets,
        buffers,
    })
}

fn persist_charx_assets(
    assets: &[CharXAsset],
    buffer_map: &HashMap<String, Vec<u8>>,
    dirs: &UserDirectories,
    character_folder: &str,
) {
    if assets.is_empty() {
        return;
    }

    let mut sprites_path: Option<PathBuf> = None;
    let mut misc_path: Option<PathBuf> = None;

    let mut ensure_sprites_path = || -> Option<PathBuf> {
        if let Some(path) = &sprites_path {
            return Some(path.clone());
        }
        let candidate = dirs.characters.join(character_folder);
        if !ensure_directory(&candidate) {
            return None;
        }
        sprites_path = Some(candidate.clone());
        Some(candidate)
    };

    let mut ensure_misc_path = || -> Option<PathBuf> {
        if let Some(path) = &misc_path {
            return Some(path.clone());
        }
        let candidate = dirs.user_images.join(character_folder);
        if !ensure_directory(&candidate) {
            return None;
        }
        misc_path = Some(candidate.clone());
        Some(candidate)
    };

    for asset in assets {
        let buffer = match buffer_map.get(&asset.zip_path) {
            Some(data) => data,
            None => {
                warn!("CharX: Asset {} missing or unsupported, skipping.", asset.zip_path);
                continue;
            }
        };

        match asset.storage {
            CharXStorageCategory::Sprite => {
                let target_dir = match ensure_sprites_path() {
                    Some(d) => d,
                    None => continue,
                };
                delete_existing_by_base_name(&target_dir, &asset.base_name);
                let file_path = target_dir.join(format!(
                    "{}.{}",
                    asset.base_name,
                    if asset.ext.is_empty() { "png" } else { &asset.ext }
                ));
                if let Err(err) = atomic_write_file(&file_path, buffer) {
                    warn!("CharX: Failed to save sprite {:?}: {}", file_path, err);
                }
            }
            CharXStorageCategory::Background => {
                let background_dir = dirs
                    .characters
                    .join(character_folder)
                    .join("backgrounds");
                if !ensure_directory(&background_dir) {
                    continue;
                }
                delete_existing_by_base_name(&background_dir, &asset.base_name);
                let file_path = background_dir.join(format!(
                    "{}.{}",
                    asset.base_name,
                    if asset.ext.is_empty() { "png" } else { &asset.ext }
                ));
                if let Err(err) = atomic_write_file(&file_path, buffer) {
                    warn!("CharX: Failed to save background {:?}: {}", file_path, err);
                }
            }
            CharXStorageCategory::Misc => {
                let misc_dir = match ensure_misc_path() {
                    Some(d) => d,
                    None => continue,
                };
                let file_path = misc_dir.join(format!(
                    "{}.{}",
                    asset.base_name,
                    if asset.ext.is_empty() { "png" } else { &asset.ext }
                ));
                if let Err(err) = atomic_write_file(&file_path, buffer) {
                    warn!("CharX: Failed to save asset {:?}: {}", file_path, err);
                }
            }
        }
    }
}

fn url_join(base: &str, relative: &str) -> String {
    let base = base.replace('\\', "/").trim_end_matches('/').to_string();
    let rel = relative.replace('\\', "/").trim_start_matches('/').to_string();
    if base.is_empty() {
        rel
    } else if rel.is_empty() {
        base
    } else {
        format!("{}/{}", base, rel)
    }
}

fn replace_macros(input: &str) -> String {
    let mut out = input.to_string();
    let re_user = Regex::new(r"(?i)#\{user\}:").unwrap();
    out = re_user.replace_all(&out, "{{user}}:").to_string();
    let re_char = Regex::new(r"(?i)#\{character\}:").unwrap();
    out = re_char.replace_all(&out, "{{char}}:").to_string();
    let re_char2 = Regex::new(r"(?i)\{character\}(?!})").unwrap();
    out = re_char2.replace_all(&out, "{{char}}").to_string();
    let re_user2 = Regex::new(r"(?i)\{user\}(?!})").unwrap();
    out = re_user2.replace_all(&out, "{{user}}").to_string();
    out
}

fn format_example_messages(examples: Option<&Vec<Value>>) -> String {
    let mut output = String::new();
    if let Some(arr) = examples {
        for example in arr {
            let text = example.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                continue;
            }
            output.push_str("<START>\n");
            output.push_str(&replace_macros(text));
            output.push('\n');
        }
    }
    output.trim_end().to_string()
}

fn get_first_message_text(scenario: &Value) -> Option<String> {
    scenario
        .get("firstMessages")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.get(0))
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn format_alternate_greetings(scenarios: &[Value]) -> Vec<String> {
    if scenarios.len() <= 1 {
        return Vec::new();
    }
    let first_text = get_first_message_text(&scenarios[0]);
    let mut greetings = HashSet::new();
    for scenario in scenarios.iter().skip(1) {
        if let Some(text) = get_first_message_text(scenario) {
            if first_text.as_deref() == Some(&text) {
                continue;
            }
            greetings.insert(replace_macros(&text));
        }
    }
    greetings.into_iter().collect()
}

fn convert_character_book(items: Option<&Vec<Value>>) -> Option<Value> {
    let items = items?;
    if items.is_empty() {
        return None;
    }
    let mut entries = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let key = item.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
        if key.is_empty() && value.is_empty() {
            continue;
        }
        let keys = replace_macros(key)
            .split(',')
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .map(|k| Value::String(k.to_string()))
            .collect::<Vec<_>>();
        let entry = serde_json::json!({
            "keys": keys,
            "content": replace_macros(value),
            "extensions": {},
            "enabled": true,
            "insertion_order": index,
        });
        entries.push(entry);
    }
    if entries.is_empty() {
        return None;
    }
    Some(serde_json::json!({
        "entries": entries,
        "extensions": {},
    }))
}

fn encode_uri(input: &str) -> String {
    fn is_allowed(byte: u8) -> bool {
        matches!(
            byte,
            b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'_'
                | b'.'
                | b'!'
                | b'~'
                | b'*'
                | b'\''
                | b'('
                | b')'
                | b';'
                | b'/'
                | b'?'
                | b':'
                | b'@'
                | b'&'
                | b'='
                | b'+'
                | b'$'
                | b','
                | b'#'
        )
    }

    let mut out = String::new();
    for &b in input.as_bytes() {
        if is_allowed(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn parse_byaf(data: &[u8], default_avatar: &[u8]) -> Result<ByafParseResult, String> {
    let manifest_buffer = extract_file_from_zip_buffer(data, "manifest.json")
        .ok_or_else(|| "Failed to extract manifest.json from BYAF file".to_string())?;
    let manifest: Value = serde_json::from_slice(&manifest_buffer)
        .map_err(|_| "Invalid BYAF manifest".to_string())?;

    let character_path = manifest
        .get("characters")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.get(0))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Invalid BYAF file: missing character path".to_string())?
        .to_string();

    let character_buffer = extract_file_from_zip_buffer(data, &character_path)
        .ok_or_else(|| "Invalid BYAF file: failed to extract character JSON".to_string())?;
    let character: Value = serde_json::from_slice(&character_buffer)
        .map_err(|_| "Invalid BYAF file: character is not valid JSON".to_string())?;

    let scenarios = {
        let scenario_paths = manifest
            .get("scenarios")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if scenario_paths.is_empty() {
            vec![serde_json::json!({})]
        } else {
            let mut out = Vec::new();
            for path_val in scenario_paths {
                let path = match path_val.as_str() {
                    Some(p) => p,
                    None => continue,
                };
                if let Some(buf) = extract_file_from_zip_buffer(data, path) {
                    if let Ok(parsed) = serde_json::from_slice::<Value>(&buf) {
                        out.push(parsed);
                    }
                }
            }
            if out.is_empty() {
                vec![serde_json::json!({})]
            } else {
                out
            }
        }
    };

    let images = {
        let character_images = character
            .get("images")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if character_images.is_empty() {
            vec![ByafImage {
                filename: String::new(),
                image: default_avatar.to_vec(),
                label: String::new(),
            }]
        } else {
            let mut out = Vec::new();
            let base_dir = Path::new(&character_path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            for image in character_images {
                let image_path = match image.get("path").and_then(|v| v.as_str()) {
                    Some(p) => p,
                    None => continue,
                };
                let full_path = url_join(base_dir, image_path);
                let buffer = match extract_file_from_zip_buffer(data, &full_path) {
                    Some(b) => b,
                    None => continue,
                };
                let filename = Path::new(image_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let label = image.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
                out.push(ByafImage {
                    filename,
                    image: buffer,
                    label,
                });
            }
            if out.is_empty() {
                vec![ByafImage {
                    filename: String::new(),
                    image: default_avatar.to_vec(),
                    label: String::new(),
                }]
            } else {
                out
            }
        }
    };

    let chat_backgrounds = {
        let mut backgrounds: Vec<ByafChatBackground> = Vec::new();
        let mut i = 1;
        for scenario in &scenarios {
            let bg_path = scenario
                .get("backgroundImage")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if bg_path.is_empty() {
                continue;
            }
            let data_buf = match extract_file_from_zip_buffer(data, bg_path) {
                Some(b) => b,
                None => continue,
            };
            if let Some(existing) = backgrounds
                .iter_mut()
                .find(|bg| bg.data == data_buf)
            {
                existing.paths.push(bg_path.to_string());
                continue;
            }
            let base_name = character.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let name = format!("{} bg {}", base_name, i);
            i += 1;
            backgrounds.push(ByafChatBackground {
                name,
                data: data_buf,
                paths: vec![bg_path.to_string()],
            });
        }
        backgrounds
    };

    let card = {
        let name = character
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| character.get("displayName").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let description = replace_macros(
            character.get("persona").and_then(|v| v.as_str()).unwrap_or(""),
        );
        let scenario_text = replace_macros(
            scenarios
                .get(0)
                .and_then(|s| s.get("narrative"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        let first_mes = replace_macros(
            scenarios
                .get(0)
                .and_then(|s| s.get("firstMessages"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(0))
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        let mes_example = format_example_messages(
            scenarios
                .get(0)
                .and_then(|s| s.get("exampleMessages"))
                .and_then(|v| v.as_array()),
        );
        let creator_notes = manifest
            .get("author")
            .and_then(|v| v.get("backyardURL"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let system_prompt = replace_macros(
            scenarios
                .get(0)
                .and_then(|s| s.get("formattingInstructions"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        let alternate_greetings = format_alternate_greetings(&scenarios);
        let character_book = convert_character_book(
            character
                .get("loreItems")
                .and_then(|v| v.as_array()),
        );
        let tags = if character
            .get("isNSFW")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            vec![Value::String("nsfw".to_string())]
        } else {
            Vec::new()
        };
        let creator = manifest
            .get("author")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut data_obj = Map::new();
        data_obj.insert("name".to_string(), Value::String(name));
        data_obj.insert("description".to_string(), Value::String(description));
        data_obj.insert("personality".to_string(), Value::String(String::new()));
        data_obj.insert("scenario".to_string(), Value::String(scenario_text));
        data_obj.insert("first_mes".to_string(), Value::String(first_mes));
        data_obj.insert("mes_example".to_string(), Value::String(mes_example));
        data_obj.insert("creator_notes".to_string(), Value::String(creator_notes));
        data_obj.insert("system_prompt".to_string(), Value::String(system_prompt));
        data_obj.insert(
            "post_history_instructions".to_string(),
            Value::String(String::new()),
        );
        data_obj.insert(
            "alternate_greetings".to_string(),
            Value::Array(alternate_greetings.into_iter().map(Value::String).collect()),
        );
        if let Some(book) = character_book {
            data_obj.insert("character_book".to_string(), book);
        }
        data_obj.insert("tags".to_string(), Value::Array(tags));
        data_obj.insert("creator".to_string(), Value::String(creator));
        data_obj.insert("character_version".to_string(), Value::String(String::new()));

        let mut extensions = Map::new();
        if let Some(display) = character.get("displayName").and_then(|v| v.as_str()) {
            extensions.insert("display_name".to_string(), Value::String(display.to_string()));
        }
        data_obj.insert("extensions".to_string(), Value::Object(extensions));

        serde_json::json!({
            "spec": "chara_card_v2",
            "spec_version": "2.0",
            "data": Value::Object(data_obj),
            "create_date": chrono::Utc::now().to_rfc3339(),
        })
    };

    Ok(ByafParseResult {
        card,
        images,
        scenarios,
        chat_backgrounds,
        character,
    })
}

fn value_to_number(val: Option<&Value>) -> Value {
    if let Some(v) = val {
        if let Some(n) = v.as_i64() {
            return Value::Number(n.into());
        }
        if let Some(n) = v.as_f64() {
            if let Some(num) = serde_json::Number::from_f64(n) {
                return Value::Number(num);
            }
        }
        if let Some(s) = v.as_str() {
            if let Ok(n) = s.parse::<f64>() {
                if let Some(num) = serde_json::Number::from_f64(n) {
                    return Value::Number(num);
                }
            }
        }
    }
    Value::Null
}

fn parse_active_timestamp(val: Option<&Value>) -> Option<i64> {
    let v = val?;
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(n) = v.as_f64() {
        return Some(n as i64);
    }
    if let Some(s) = v.as_str() {
        if let Ok(n) = s.parse::<i64>() {
            return Some(n);
        }
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(dt.timestamp_millis());
        }
    }
    None
}

fn get_newest_ai_message(message: &Value) -> Option<&Value> {
    let outputs = message.get("outputs").and_then(|v| v.as_array())?;
    let mut newest: Option<&Value> = None;
    let mut newest_ts: Option<i64> = None;
    for output in outputs {
        let ts = parse_active_timestamp(output.get("activeTimestamp"));
        if newest.is_none() || ts.unwrap_or(0) >= newest_ts.unwrap_or(0) {
            newest = Some(output);
            newest_ts = ts;
        }
    }
    newest.or_else(|| outputs.get(0))
}

fn get_swipes(message: &Value) -> Vec<String> {
    let outputs = match message.get("outputs").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };
    outputs
        .iter()
        .filter_map(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
        .collect()
}

fn get_chat_from_scenario(
    scenario: &Value,
    user_name: &str,
    character_name: &str,
    chat_backgrounds: &[ByafChatBackground],
) -> String {
    let messages_value = scenario.get("messages").and_then(|v| v.as_array());
    let messages = messages_value.cloned().unwrap_or_default();

    let chat_start_date = if let Some(arr) = messages_value {
        if arr.is_empty() {
            Value::String(chrono::Utc::now().to_rfc3339())
        } else {
            arr.iter()
                .find(|m| m.get("createdAt").is_some())
                .and_then(|m| m.get("createdAt"))
                .cloned()
                .unwrap_or(Value::Null)
        }
    } else {
        Value::Null
    };

    let scenario_bg = scenario
        .get("backgroundImage")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let chat_background = chat_backgrounds
        .iter()
        .find(|bg| bg.paths.iter().any(|p| p == scenario_bg))
        .map(|bg| bg.name.clone())
        .unwrap_or_default();

    let mut header_meta = Map::new();
    header_meta.insert(
        "scenario".to_string(),
        Value::String(
            scenario
                .get("narrative")
                .and_then(|v| v.as_str())
                .map(replace_macros)
                .unwrap_or_default(),
        ),
    );
    header_meta.insert(
        "mes_example".to_string(),
        Value::String(format_example_messages(
            scenario
                .get("exampleMessages")
                .and_then(|v| v.as_array()),
        )),
    );
    header_meta.insert(
        "system_prompt".to_string(),
        Value::String(
            scenario
                .get("formattingInstructions")
                .and_then(|v| v.as_str())
                .map(replace_macros)
                .unwrap_or_default(),
        ),
    );
    header_meta.insert(
        "mes_examples_optional".to_string(),
        Value::Bool(
            scenario
                .get("canDeleteExampleMessages")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ),
    );

    let mut model_settings = Map::new();
    model_settings.insert(
        "model".to_string(),
        Value::String(
            scenario
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ),
    );
    model_settings.insert(
        "temperature".to_string(),
        Value::Number(
            serde_json::Number::from_f64(
                scenario
                    .get("temperature")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.2),
            )
            .unwrap(),
        ),
    );
    model_settings.insert(
        "top_k".to_string(),
        Value::Number(
            serde_json::Number::from_f64(
                scenario.get("topK").and_then(|v| v.as_f64()).unwrap_or(40.0),
            )
            .unwrap(),
        ),
    );
    model_settings.insert(
        "top_p".to_string(),
        Value::Number(
            serde_json::Number::from_f64(
                scenario.get("topP").and_then(|v| v.as_f64()).unwrap_or(0.9),
            )
            .unwrap(),
        ),
    );
    model_settings.insert(
        "min_p".to_string(),
        Value::Number(
            serde_json::Number::from_f64(
                scenario.get("minP").and_then(|v| v.as_f64()).unwrap_or(0.1),
            )
            .unwrap(),
        ),
    );
    model_settings.insert(
        "min_p_enabled".to_string(),
        Value::Bool(
            scenario
                .get("minPEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
        ),
    );
    model_settings.insert(
        "repeat_penalty".to_string(),
        Value::Number(
            serde_json::Number::from_f64(
                scenario
                    .get("repeatPenalty")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.05),
            )
            .unwrap(),
        ),
    );
    model_settings.insert(
        "repeat_penalty_tokens".to_string(),
        Value::Number(
            serde_json::Number::from_f64(
                scenario
                    .get("repeatLastN")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(256.0),
            )
            .unwrap(),
        ),
    );
    model_settings.insert(
        "by_prompt_template".to_string(),
        Value::String(
            scenario
                .get("promptTemplate")
                .and_then(|v| v.as_str())
                .unwrap_or("general")
                .to_string(),
        ),
    );
    model_settings.insert(
        "grammar".to_string(),
        scenario.get("grammar").cloned().unwrap_or(Value::Null),
    );

    header_meta.insert("byaf_model_settings".to_string(), Value::Object(model_settings));
    if !chat_background.is_empty() {
        header_meta.insert(
            "chat_backgrounds".to_string(),
            Value::Array(vec![Value::String(chat_background.clone())]),
        );
        header_meta.insert(
            "custom_background".to_string(),
            Value::String(format!("url(\"{}\")", encode_uri(&chat_background))),
        );
    } else {
        header_meta.insert("chat_backgrounds".to_string(), Value::Array(vec![]));
        header_meta.insert("custom_background".to_string(), Value::String(String::new()));
    }

    let mut chat = Vec::new();
    let header = serde_json::json!({
        "user_name": "unused",
        "character_name": "unused",
        "chat_metadata": Value::Object(header_meta),
    });
    chat.push(header);

    if let Some(first) = get_first_message_text(scenario) {
        if !first.is_empty() {
            chat.push(serde_json::json!({
                "name": character_name,
                "is_user": false,
                "send_date": chat_start_date,
                "mes": first,
            }));
        }
    }

    let user_messages: Vec<Value> = messages
        .iter()
        .filter(|m| m.get("type").and_then(|v| v.as_str()) == Some("human"))
        .cloned()
        .collect();
    let character_messages: Vec<Value> = messages
        .iter()
        .filter(|m| m.get("type").and_then(|v| v.as_str()) == Some("ai"))
        .cloned()
        .collect();

    if !user_messages.is_empty()
        && user_messages.len() == character_messages.len()
        && !character_messages.is_empty()
    {
        for i in 0..user_messages.len() {
            let user_msg = &user_messages[i];
            chat.push(serde_json::json!({
                "name": user_name,
                "is_user": true,
                "send_date": value_to_number(user_msg.get("createdAt")),
                "mes": user_msg.get("text").and_then(|v| v.as_str()).unwrap_or(""),
            }));

            let ai_msg = &character_messages[i];
            let newest = get_newest_ai_message(ai_msg);
            let swipes = get_swipes(ai_msg);
            let newest_text = newest
                .and_then(|m| m.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let swipe_id = swipes.iter().position(|s| s == newest_text).unwrap_or(0);
            chat.push(serde_json::json!({
                "name": character_name,
                "is_user": false,
                "send_date": value_to_number(newest.and_then(|m| m.get("createdAt"))),
                "mes": newest_text,
                "swipes": swipes,
                "swipe_id": swipe_id,
            }));
        }
    } else if !messages.is_empty() {
        for msg in &messages {
            let is_user = msg.get("type").and_then(|v| v.as_str()) == Some("human");
            if is_user {
                chat.push(serde_json::json!({
                    "name": user_name,
                    "is_user": true,
                    "send_date": value_to_number(msg.get("createdAt")),
                    "mes": msg.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                }));
            } else {
                let newest = get_newest_ai_message(msg);
                let swipes = get_swipes(msg);
                let newest_text = newest
                    .and_then(|m| m.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let swipe_id = swipes.iter().position(|s| s == newest_text).unwrap_or(0);
                chat.push(serde_json::json!({
                    "name": character_name,
                    "is_user": false,
                    "send_date": value_to_number(newest.and_then(|m| m.get("createdAt"))),
                    "mes": newest_text,
                    "swipes": swipes,
                    "swipe_id": swipe_id,
                }));
            }
        }
    }

    chat.iter()
        .filter_map(|obj| serde_json::to_string(obj).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

fn client_relative_path(root: &Path, input_path: &Path) -> Result<String, String> {
    let root_str = root.to_string_lossy();
    let input_str = input_path.to_string_lossy();
    if !input_str.starts_with(root_str.as_ref()) {
        return Err("Input path does not start with root".to_string());
    }
    let mut rel = input_str[root_str.len()..].to_string();
    if std::path::MAIN_SEPARATOR != '/' {
        rel = rel.replace(std::path::MAIN_SEPARATOR, "/");
    }
    Ok(rel)
}

fn get_unique_name<F>(base_name: &str, exists: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    let max_tries = 1000;
    let mut i = 1;
    while i < max_tries + 1 {
        let name = if i == 0 {
            base_name.to_string()
        } else {
            format!("{} ({})", base_name, i)
        };
        if !exists(&name) {
            return Some(name);
        }
        i += 1;
    }
    None
}

pub(super) async fn import_from_charx(
    file_data: &[u8],
    dirs: &UserDirectories,
    preserved_name: Option<&str>,
    state: &Arc<AppState>,
) -> Result<String, Response> {
    let parsed = match parse_charx(file_data) {
        Ok(p) => p,
        Err(err) => {
            warn!("CharX import failed: {}", err);
            return Err(Json(serde_json::json!({ "error": true })).into_response());
        }
    };

    let mut card = parsed.card;
    png::normalise_to_v2(&mut card);
    unset_private_fields(&mut card);
    set_val(
        &mut card,
        "create_date",
        Value::String(chrono::Utc::now().to_rfc3339()),
    );

    let current_name = card.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let sanitized_name = sanitize_filename_strip(current_name);
    set_val(&mut card, "name", Value::String(sanitized_name.clone()));

    let file_name = preserved_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| get_png_name(&sanitized_name, &dirs.characters));

    if !parsed.assets.is_empty() {
        persist_charx_assets(&parsed.assets, &parsed.buffers, dirs, &sanitized_name);
    }

    let avatar = match parsed.avatar {
        Some(data) => data,
        None => match read_image_source(None, &state.config.server_directory) {
            Ok(data) => data,
            Err(status) => return Err(status.into_response()),
        },
    };

    let char_json = serde_json::to_string(&card).unwrap_or_default();
    if let Err(status) =
        write_and_save_character(&avatar, &char_json, &file_name, &dirs.characters, None)
    {
        return Err(status.into_response());
    }

    Ok(file_name)
}

pub(super) async fn import_from_byaf(
    file_data: &[u8],
    dirs: &UserDirectories,
    preserved_name: Option<&str>,
    state: &Arc<AppState>,
    body: &Value,
) -> Result<String, Response> {
    let default_avatar = match read_image_source(None, &state.config.server_directory) {
        Ok(data) => data,
        Err(status) => return Err(status.into_response()),
    };
    let mut parsed = match parse_byaf(file_data, &default_avatar) {
        Ok(p) => p,
        Err(err) => {
            warn!("BYAF import failed: {}", err);
            return Err(Json(serde_json::json!({ "error": true })).into_response());
        }
    };

    let mut card = parsed.card;
    png::normalise_to_v2(&mut card);

    let display_name = parsed
        .character
        .get("displayName")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let card_name = card.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let name_for_file = if display_name.is_empty() {
        card_name
    } else {
        display_name
    };
    let sanitized_name = sanitize_character_name(name_for_file);
    let file_name = preserved_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| get_png_name(&sanitized_name, &dirs.characters));

    if preserved_name.is_none() {
        let base_name = Path::new(&file_name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&file_name)
            .to_string();
        let user_name = body
            .get("user_name")
            .and_then(|v| v.as_str())
            .unwrap_or("undefined");

        // Upload backgrounds
        for bg in parsed.chat_backgrounds.iter_mut() {
            let ext = bg
                .paths
                .get(0)
                .and_then(|p| Path::new(p).extension().and_then(|s| s.to_str()))
                .map(|s| format!(".{}", s))
                .unwrap_or_else(|| ".png".to_string());
            let folder = dirs.user_images.join(&file_name);
            if !ensure_directory(&folder) {
                continue;
            }
            let base = format!("{}_bg", base_name);
            let unique = get_unique_name(&base, |name| folder.join(format!("{}{}", name, ext)).exists());
            let file_base = match unique {
                Some(u) => u,
                None => continue,
            };
            let new_file = format!("{}{}", file_base, ext);
            let path_to_file = folder.join(&new_file);
            if atomic_write_file(&path_to_file, &bg.data).is_ok() {
                if let Ok(rel) = client_relative_path(&dirs.root, &path_to_file) {
                    bg.name = rel;
                }
            }
        }

        // Create chats
        let mut chats = Vec::new();
        for scenario in &parsed.scenarios {
            let title = scenario.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let base = if !title.is_empty() { title } else { card_name };
            let chat_name = sanitize_filename(
                &format!("{} - {} imported.jsonl", base, png::humanized_date_time()),
                SAFE_CHARACTER_REPLACEMENT,
            );
            let chat_dir = dirs.chats.join(&base_name);
            if let Err(err) = fs::create_dir_all(&chat_dir) {
                warn!("Failed to create chat dir: {}", err);
                continue;
            }
            let chat_path = chat_dir.join(&chat_name);
            let jsonl = get_chat_from_scenario(
                scenario,
                user_name,
                card_name,
                &parsed.chat_backgrounds,
            );
            if atomic_write_file(&chat_path, jsonl.as_bytes()).is_ok() {
                chats.push(chat_name);
            }
        }

        if let Some(first) = chats.first() {
            let chat_id = Path::new(first)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            set_val(&mut card, "chat", Value::String(chat_id.to_string()));
        }

        // Alternate icons
        if let Some(card_name) = card.get("name").and_then(|v| v.as_str()) {
            let alt_folder = dirs.characters.join(sanitize_filename_strip(card_name));
            if ensure_directory(&alt_folder) {
                for icon in parsed.images.iter().skip(1) {
                    let ext = Path::new(&icon.filename)
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| format!(".{}", s))
                        .unwrap_or_else(|| ".png".to_string());
                    let mut base = sanitize_filename(&icon.label, SAFE_CHARACTER_REPLACEMENT);
                    if base.is_empty() {
                        base = "alt".to_string();
                    }
                    let unique = get_unique_name(&base, |name| alt_folder.join(format!("{}{}", name, ext)).exists());
                    let name = match unique {
                        Some(u) => u,
                        None => continue,
                    };
                    let target = alt_folder.join(format!("{}{}", name, ext));
                    let _ = atomic_write_file(&target, &icon.image);
                }
            }
        }
    }

    let avatar = parsed
        .images
        .first()
        .map(|img| img.image.clone())
        .unwrap_or_else(|| default_avatar.clone());

    let char_json = serde_json::to_string(&card).unwrap_or_default();
    if let Err(status) =
        write_and_save_character(&avatar, &char_json, &file_name, &dirs.characters, None)
    {
        return Err(status.into_response());
    }

    Ok(file_name)
}
