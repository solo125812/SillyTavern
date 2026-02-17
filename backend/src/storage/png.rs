//! PNG tEXt chunk reader/writer for character card metadata.
//!
//! SillyTavern embeds character JSON in PNG tEXt chunks:
//! - `chara`: base64-encoded V2 spec JSON
//! - `ccv3`: base64-encoded V3 spec JSON (takes precedence when present)
//!
//! This module reads/writes those chunks and normalises the parsed JSON to
//! V2/V3 structures matching the Node `character-card-parser.js` behaviour.

use std::io::{Cursor, Read};
use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use tracing::warn;

/// Errors that can occur when reading character data from a PNG file.
#[derive(Debug, thiserror::Error)]
pub enum PngCharaError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("PNG decoding error: {0}")]
    PngDecode(String),

    #[error("No character metadata found in PNG")]
    NoMetadata,

    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("UTF-8 decode error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Read character card JSON from a PNG file.
///
/// Mirrors the Node `character-card-parser.js` `read()` function:
/// 1. Extract all tEXt chunks from the PNG.
/// 2. If a `ccv3` chunk exists, decode and return it (V3 takes precedence).
/// 3. Otherwise decode the `chara` chunk (V2).
/// 4. If neither exists, return [`PngCharaError::NoMetadata`].
///
/// Returns the decoded JSON string (not yet parsed into a struct, since
/// character cards contain many optional/dynamic fields).
pub fn read_character_data(png_path: &Path) -> Result<String, PngCharaError> {
    let data = std::fs::read(png_path)?;
    read_character_data_from_bytes(&data)
}

/// Read character card JSON from an in-memory PNG buffer.
pub fn read_character_data_from_bytes(data: &[u8]) -> Result<String, PngCharaError> {
    let text_chunks = extract_text_chunks(data)?;

    if text_chunks.is_empty() {
        return Err(PngCharaError::NoMetadata);
    }

    // ccv3 takes precedence (V3 spec)
    if let Some(ccv3) = text_chunks
        .iter()
        .find(|(kw, _)| kw.eq_ignore_ascii_case("ccv3"))
    {
        let decoded = decode_base64_loose(&ccv3.1)?;
        return Ok(String::from_utf8(decoded)?);
    }

    // Fall back to chara (V2 spec)
    if let Some(chara) = text_chunks
        .iter()
        .find(|(kw, _)| kw.eq_ignore_ascii_case("chara"))
    {
        let decoded = decode_base64_loose(&chara.1)?;
        return Ok(String::from_utf8(decoded)?);
    }

    Err(PngCharaError::NoMetadata)
}

/// Normalise a parsed character JSON value to V2 format.
///
/// Mirrors the Node `readFromV2` / `getCharaCardV2` logic:
/// - If `spec` is present, hoist `data.*` fields to top level.
/// - If `spec` is absent, it's a V1 card (we pass through as-is for now,
///   since the `/all` and `/get` endpoints do the full conversion in Node).
///
/// For Phase 4 (read-only), we perform the minimal normalisation needed
/// to match Node output: copy `data.extensions.fav` → `fav`,
/// `data.extensions.talkativeness` → `talkativeness`, etc.
pub fn normalise_to_v2(char_json: &mut serde_json::Value) {
    let obj = match char_json.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // If spec exists, it's already V2/V3 format — apply readFromV2 field mappings
    if obj.contains_key("spec") {
        if let Some(data) = obj.get("data").cloned() {
            // Field mappings: top-level field ← data.path
            let field_mappings: &[(&str, &[&str])] = &[
                ("name", &["name"]),
                ("description", &["description"]),
                ("personality", &["personality"]),
                ("scenario", &["scenario"]),
                ("first_mes", &["first_mes"]),
                ("mes_example", &["mes_example"]),
                ("talkativeness", &["extensions", "talkativeness"]),
                ("fav", &["extensions", "fav"]),
                ("tags", &["tags"]),
            ];

            for &(char_field, path) in field_mappings {
                let v2_value = get_nested(&data, path);
                match v2_value {
                    Some(v) => {
                        obj.insert(char_field.to_string(), v.clone());
                    }
                    None => {
                        // Backfill defaults for missing ST extension fields
                        match char_field {
                            "talkativeness" => {
                                obj.insert(
                                    char_field.to_string(),
                                    serde_json::Value::from(0.5),
                                );
                            }
                            "fav" => {
                                obj.insert(
                                    char_field.to_string(),
                                    serde_json::Value::Bool(false),
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Ensure chat field exists
            if !obj.contains_key("chat") {
                let name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                let chat_name = format!("{} - {}", name, humanized_date_time());
                obj.insert("chat".to_string(), serde_json::Value::String(chat_name));
            }
        }

        // Remove json_data if present (prevent recursive saving)
        obj.remove("json_data");
    }
    // V1 cards without spec: would need convertToV2, but for read-only
    // Phase 4 we pass through since the frontend handles this.
}

/// Get a nested value from a JSON object using a path of keys.
fn get_nested<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for &key in path {
        current = current.get(key)?;
    }
    Some(current)
}

/// Generate a humanized date-time string matching the Node `humanizedDateTime()`.
/// Format: `YYYY-MM-DD @HHhMMmSSs`
pub fn humanized_date_time() -> String {
    let now = chrono::Local::now();
    now.format("%Y-%m-%d @%Hh%Mm%Ss").to_string()
}

// ---------------------------------------------------------------------------
// PNG tEXt chunk extraction & writing
// ---------------------------------------------------------------------------

/// A raw PNG chunk: type (4 bytes) + data bytes.
struct RawChunk {
    chunk_type: [u8; 4],
    data: Vec<u8>,
}

/// Extract all raw chunks from a PNG byte buffer.
///
/// Returns every chunk (IHDR, tEXt, IDAT, IEND, etc.) as a `RawChunk`.
fn extract_raw_chunks(data: &[u8]) -> Result<Vec<RawChunk>, PngCharaError> {
    if data.len() < 8 {
        return Err(PngCharaError::PngDecode("File too small for PNG".into()));
    }
    let png_signature: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if &data[..8] != &png_signature {
        return Err(PngCharaError::PngDecode("Invalid PNG signature".into()));
    }

    let mut cursor = Cursor::new(data);
    cursor.set_position(8);
    let mut chunks = Vec::new();

    loop {
        let mut len_buf = [0u8; 4];
        if cursor.read_exact(&mut len_buf).is_err() {
            break;
        }
        let chunk_len = u32::from_be_bytes(len_buf) as usize;

        let mut type_buf = [0u8; 4];
        if cursor.read_exact(&mut type_buf).is_err() {
            break;
        }

        let mut chunk_data = vec![0u8; chunk_len];
        if cursor.read_exact(&mut chunk_data).is_err() {
            break;
        }

        // Skip CRC (4 bytes)
        let mut crc_buf = [0u8; 4];
        if cursor.read_exact(&mut crc_buf).is_err() {
            break;
        }

        let is_iend = &type_buf == b"IEND";
        chunks.push(RawChunk {
            chunk_type: type_buf,
            data: chunk_data,
        });

        if is_iend {
            break;
        }
    }

    Ok(chunks)
}

/// Build a tEXt chunk data payload: keyword (null-terminated) + text.
fn encode_text_chunk_data(keyword: &str, text: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(keyword.len() + 1 + text.len());
    data.extend_from_slice(keyword.as_bytes());
    data.push(0);
    data.extend_from_slice(text.as_bytes());
    data
}

/// Write a single PNG chunk (length + type + data + CRC) into a buffer.
fn write_raw_chunk(buf: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(chunk_type);
    buf.extend_from_slice(data);
    let crc = crc32(chunk_type, data);
    buf.extend_from_slice(&crc.to_be_bytes());
}

/// CRC32 for PNG chunk validation.
fn crc32(chunk_type: &[u8], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in chunk_type.iter().chain(data.iter()) {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFFFFFF
}

/// Write character card data into a PNG image buffer.
///
/// Mirrors Node's `character-card-parser.js` `write()` function:
/// 1. Parse all existing chunks from the PNG buffer.
/// 2. Remove any existing `chara` or `ccv3` tEXt chunks.
/// 3. Insert a new `chara` tEXt chunk (V2, base64-encoded) before IEND.
/// 4. Try to insert a `ccv3` tEXt chunk (V3 variant) before IEND.
/// 5. Rebuild the PNG buffer.
pub fn write_character_data(image: &[u8], data: &str) -> Result<Vec<u8>, PngCharaError> {
    let mut chunks = extract_raw_chunks(image)?;

    // Remove existing chara/ccv3 tEXt chunks
    chunks.retain(|chunk| {
        if &chunk.chunk_type != b"tEXt" {
            return true;
        }
        if let Some((keyword, _)) = parse_text_chunk(&chunk.data) {
            let kw_lower = keyword.to_ascii_lowercase();
            kw_lower != "chara" && kw_lower != "ccv3"
        } else {
            true
        }
    });

    // Build V2 chara tEXt chunk
    let base64_data = BASE64.encode(data.as_bytes());
    let chara_chunk_data = encode_text_chunk_data("chara", &base64_data);
    let chara_chunk = RawChunk {
        chunk_type: *b"tEXt",
        data: chara_chunk_data,
    };

    // Build V3 ccv3 tEXt chunk (best-effort)
    let ccv3_chunk = match serde_json::from_str::<serde_json::Value>(data) {
        Ok(mut v3_data) => {
            v3_data["spec"] = serde_json::Value::String("chara_card_v3".into());
            v3_data["spec_version"] = serde_json::Value::String("3.0".into());
            let v3_json = serde_json::to_string(&v3_data).unwrap_or_default();
            let v3_base64 = BASE64.encode(v3_json.as_bytes());
            let ccv3_data = encode_text_chunk_data("ccv3", &v3_base64);
            Some(RawChunk {
                chunk_type: *b"tEXt",
                data: ccv3_data,
            })
        }
        Err(e) => {
            warn!("Failed to create ccv3 chunk: {}", e);
            None
        }
    };

    // Insert chara (and optionally ccv3) before IEND
    let iend_pos = chunks
        .iter()
        .position(|c| &c.chunk_type == b"IEND")
        .unwrap_or(chunks.len());
    chunks.insert(iend_pos, chara_chunk);
    if let Some(ccv3) = ccv3_chunk {
        // Insert after chara, still before IEND
        chunks.insert(iend_pos + 1, ccv3);
    }

    // Rebuild PNG
    let mut output = Vec::new();
    output.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]); // PNG signature
    for chunk in &chunks {
        write_raw_chunk(&mut output, &chunk.chunk_type, &chunk.data);
    }

    Ok(output)
}

/// Extract all tEXt chunks from a PNG byte buffer.
///
/// PNG structure: 8-byte signature, then chunks.
/// Each chunk: 4-byte length (big-endian), 4-byte type, `length` bytes data, 4-byte CRC.
/// tEXt chunks contain: keyword (null-terminated Latin-1), then text data.
fn extract_text_chunks(data: &[u8]) -> Result<Vec<(String, String)>, PngCharaError> {
    // Validate PNG signature
    if data.len() < 8 {
        return Err(PngCharaError::PngDecode("File too small for PNG".into()));
    }
    let png_signature: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if &data[..8] != &png_signature {
        return Err(PngCharaError::PngDecode("Invalid PNG signature".into()));
    }

    let mut cursor = Cursor::new(data);
    cursor.set_position(8); // Skip signature

    let mut text_chunks = Vec::new();

    loop {
        // Read chunk length (4 bytes, big-endian)
        let mut len_buf = [0u8; 4];
        if cursor.read_exact(&mut len_buf).is_err() {
            break; // End of file
        }
        let chunk_len = u32::from_be_bytes(len_buf) as usize;

        // Read chunk type (4 bytes)
        let mut type_buf = [0u8; 4];
        if cursor.read_exact(&mut type_buf).is_err() {
            break;
        }
        let chunk_type = String::from_utf8_lossy(&type_buf).to_string();

        // Read chunk data
        let mut chunk_data = vec![0u8; chunk_len];
        if cursor.read_exact(&mut chunk_data).is_err() {
            break;
        }

        // Skip CRC (4 bytes)
        let mut crc_buf = [0u8; 4];
        if cursor.read_exact(&mut crc_buf).is_err() {
            break;
        }

        // Process tEXt chunks
        if chunk_type == "tEXt" {
            if let Some((keyword, text)) = parse_text_chunk(&chunk_data) {
                text_chunks.push((keyword, text));
            }
        }

        // Stop at IEND
        if chunk_type == "IEND" {
            break;
        }
    }

    Ok(text_chunks)
}

/// Parse a tEXt chunk's data into (keyword, text).
///
/// Format: keyword (null-terminated), text (rest of data).
fn parse_text_chunk(data: &[u8]) -> Option<(String, String)> {
    let null_pos = data.iter().position(|&b| b == 0)?;
    let keyword = String::from_utf8_lossy(&data[..null_pos]).to_string();
    let text = String::from_utf8_lossy(&data[null_pos + 1..]).to_string();
    Some((keyword, text))
}

fn decode_base64_loose(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let mut cleaned: String = input
        .chars()
        .filter(|c| {
            matches!(
                c,
                'A'..='Z'
                    | 'a'..='z'
                    | '0'..='9'
                    | '+'
                    | '/'
                    | '-'
                    | '_'
                    | '='
            )
        })
        .collect();

    if cleaned.contains('-') || cleaned.contains('_') {
        cleaned = cleaned.replace('-', "+").replace('_', "/");
    }

    match cleaned.len() % 4 {
        2 => cleaned.push_str("=="),
        3 => cleaned.push('='),
        _ => {}
    }

    BASE64.decode(cleaned.as_bytes())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid PNG with a tEXt chunk.
    fn build_png_with_text_chunks(chunks: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();

        // PNG signature
        buf.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

        // IHDR chunk (minimal 13-byte header)
        let ihdr_data: [u8; 13] = [
            0, 0, 0, 1, // width: 1
            0, 0, 0, 1, // height: 1
            8,  // bit depth
            2,  // color type (RGB)
            0,  // compression
            0,  // filter
            0,  // interlace
        ];
        write_chunk(&mut buf, b"IHDR", &ihdr_data);

        // tEXt chunks
        for &(keyword, text) in chunks {
            let mut data = Vec::new();
            data.extend_from_slice(keyword.as_bytes());
            data.push(0); // null separator
            data.extend_from_slice(text.as_bytes());
            write_chunk(&mut buf, b"tEXt", &data);
        }

        // IDAT chunk (minimal)
        write_chunk(&mut buf, b"IDAT", &[0]);

        // IEND chunk
        write_chunk(&mut buf, b"IEND", &[]);

        buf
    }

    fn write_chunk(buf: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        write_raw_chunk(buf, chunk_type, data);
    }

    #[test]
    fn reads_chara_text_chunk() {
        let json_str = r#"{"name":"TestChar","spec":"chara_card_v2"}"#;
        let encoded = BASE64.encode(json_str.as_bytes());
        let png = build_png_with_text_chunks(&[("chara", &encoded)]);

        let result = read_character_data_from_bytes(&png).unwrap();
        assert_eq!(result, json_str);
    }

    #[test]
    fn ccv3_takes_precedence_over_chara() {
        let v2_json = r#"{"name":"V2Char","spec":"chara_card_v2"}"#;
        let v3_json = r#"{"name":"V3Char","spec":"chara_card_v3","spec_version":"3.0"}"#;
        let v2_encoded = BASE64.encode(v2_json.as_bytes());
        let v3_encoded = BASE64.encode(v3_json.as_bytes());
        let png = build_png_with_text_chunks(&[("chara", &v2_encoded), ("ccv3", &v3_encoded)]);

        let result = read_character_data_from_bytes(&png).unwrap();
        assert_eq!(result, v3_json);
    }

    #[test]
    fn no_metadata_returns_error() {
        let png = build_png_with_text_chunks(&[]);
        let result = read_character_data_from_bytes(&png);
        assert!(result.is_err());
        assert!(matches!(result, Err(PngCharaError::NoMetadata)));
    }

    #[test]
    fn case_insensitive_keyword_matching() {
        let json_str = r#"{"name":"CaseTest"}"#;
        let encoded = BASE64.encode(json_str.as_bytes());
        let png = build_png_with_text_chunks(&[("Chara", &encoded)]);

        let result = read_character_data_from_bytes(&png).unwrap();
        assert_eq!(result, json_str);
    }

    #[test]
    fn invalid_png_returns_error() {
        let result = read_character_data_from_bytes(b"not a png");
        assert!(result.is_err());
    }

    #[test]
    fn normalise_v2_hoists_data_fields() {
        let mut json: serde_json::Value = serde_json::json!({
            "spec": "chara_card_v2",
            "spec_version": "2.0",
            "data": {
                "name": "TestChar",
                "description": "A test character",
                "personality": "Friendly",
                "scenario": "Testing",
                "first_mes": "Hello!",
                "mes_example": "Example",
                "extensions": {
                    "talkativeness": 0.7,
                    "fav": true
                },
                "tags": ["test", "demo"]
            }
        });

        normalise_to_v2(&mut json);

        assert_eq!(json["name"], "TestChar");
        assert_eq!(json["description"], "A test character");
        assert_eq!(json["talkativeness"], 0.7);
        assert_eq!(json["fav"], true);
        assert_eq!(json["tags"], serde_json::json!(["test", "demo"]));
    }

    #[test]
    fn normalise_v2_backfills_defaults() {
        let mut json: serde_json::Value = serde_json::json!({
            "spec": "chara_card_v2",
            "data": {
                "name": "NoExtensions",
                "extensions": {}
            }
        });

        normalise_to_v2(&mut json);

        assert_eq!(json["talkativeness"], 0.5);
        assert_eq!(json["fav"], false);
    }

    #[test]
    fn normalise_removes_json_data() {
        let mut json: serde_json::Value = serde_json::json!({
            "spec": "chara_card_v2",
            "json_data": "should be removed",
            "data": {
                "name": "Test"
            }
        });

        normalise_to_v2(&mut json);

        assert!(json.get("json_data").is_none());
    }

    #[test]
    fn humanized_date_time_format() {
        let result = humanized_date_time();
        // Should match pattern: YYYY-MM-DD @HHhMMmSSs
        assert!(result.contains('@'));
        assert!(result.contains('h'));
        assert!(result.contains('m'));
        assert!(result.ends_with('s'));
    }

    #[test]
    fn parse_text_chunk_basic() {
        let mut data = Vec::new();
        data.extend_from_slice(b"chara");
        data.push(0);
        data.extend_from_slice(b"some text data");

        let result = parse_text_chunk(&data).unwrap();
        assert_eq!(result.0, "chara");
        assert_eq!(result.1, "some text data");
    }

    #[test]
    fn write_then_read_round_trip() {
        let json_str = r#"{"name":"WriteTest","spec":"chara_card_v2","spec_version":"2.0","data":{"name":"WriteTest"}}"#;
        let orig_png = build_png_with_text_chunks(&[]);
        let written = write_character_data(&orig_png, json_str).unwrap();
        let read_back = read_character_data_from_bytes(&written).unwrap();
        // ccv3 takes precedence on read — it will have spec=chara_card_v3
        let parsed: serde_json::Value = serde_json::from_str(&read_back).unwrap();
        assert_eq!(parsed["name"], "WriteTest");
        assert_eq!(parsed["spec"], "chara_card_v3");
    }

    #[test]
    fn write_replaces_existing_chunks() {
        let old_json = r#"{"name":"Old"}"#;
        let old_encoded = BASE64.encode(old_json.as_bytes());
        let orig_png = build_png_with_text_chunks(&[("chara", &old_encoded)]);

        let new_json = r#"{"name":"New","spec":"chara_card_v2"}"#;
        let written = write_character_data(&orig_png, new_json).unwrap();
        let read_back = read_character_data_from_bytes(&written).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&read_back).unwrap();
        assert_eq!(parsed["name"], "New");
    }

    #[test]
    fn write_preserves_non_text_chunks() {
        let json_str = r#"{"name":"Preserve"}"#;
        let orig_png = build_png_with_text_chunks(&[]);
        let written = write_character_data(&orig_png, json_str).unwrap();
        // Should still be a valid PNG with IHDR, IDAT, our tEXt chunks, and IEND
        let chunks = extract_raw_chunks(&written).unwrap();
        let types: Vec<String> = chunks.iter().map(|c| String::from_utf8_lossy(&c.chunk_type).to_string()).collect();
        assert!(types.contains(&"IHDR".to_string()));
        assert!(types.contains(&"IDAT".to_string()));
        assert!(types.contains(&"tEXt".to_string()));
        assert!(types.contains(&"IEND".to_string()));
    }
}
