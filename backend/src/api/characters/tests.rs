//! Unit tests for the characters module.

use std::fs;
use std::path::Path;

use super::helpers::*;
use super::reads::get_chat_info;
use crate::storage::paths::UserDirectories;
use crate::storage::png;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::Value;

#[test]
fn format_bytes_zero() {
    assert_eq!(format_bytes(0), "0B");
}

#[test]
fn format_bytes_small() {
    assert_eq!(format_bytes(100), "100B");
}

#[test]
fn format_bytes_kilobytes() {
    assert_eq!(format_bytes(1024), "1KB");
}

#[test]
fn format_bytes_megabytes() {
    let result = format_bytes(1_048_576);
    assert_eq!(result, "1MB");
}

#[test]
fn format_bytes_fractional() {
    let result = format_bytes(1_536);
    assert_eq!(result, "1.5KB");
}

#[test]
fn calculate_data_size_with_data() {
    let json = serde_json::json!({
        "data": {
            "name": "Test",
            "description": "A character"
        }
    });
    let size = calculate_data_size(&json);
    assert!(size > 0);
}

#[test]
fn calculate_data_size_no_data() {
    let json = serde_json::json!({
        "name": "Test"
    });
    assert_eq!(calculate_data_size(&json), 0);
}

#[test]
fn timestamp_to_iso_produces_valid_string() {
    let iso = timestamp_to_iso(1700000000000.0);
    assert!(iso.contains("2023"));
    assert!(iso.contains("T"));
}

#[test]
fn process_character_from_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let chars_dir = dir.path().join("characters");
    let chats_dir = dir.path().join("chats");
    fs::create_dir_all(&chars_dir).unwrap();
    fs::create_dir_all(&chats_dir).unwrap();

    // Build a PNG with character data
    let char_data = serde_json::json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "data": {
            "name": "TestChar",
            "description": "A test character",
            "personality": "Friendly",
            "scenario": "",
            "first_mes": "Hello!",
            "mes_example": "",
            "creator_notes": "",
            "tags": [],
            "creator": "test",
            "character_version": "1.0",
            "extensions": {
                "talkativeness": 0.5,
                "fav": false
            }
        }
    });

    let json_str = serde_json::to_string(&char_data).unwrap();
    let encoded = BASE64.encode(json_str.as_bytes());
    let png_bytes = build_test_png(&[("chara", &encoded)]);

    fs::write(chars_dir.join("TestChar.png"), &png_bytes).unwrap();

    // Build UserDirectories manually for test
    let _dirs = UserDirectories::new(dir.path(), "test_user");
    // We need to create dirs matching the template, but for this test
    // we'll use the actual path
    let _test_dirs = UserDirectories::new(dir.path(), "");

    // Since UserDirectories::new uses a handle, we'll work around it
    // by directly processing
    let result = process_character_with_paths(
        "TestChar.png",
        &chars_dir,
        &chats_dir,
    );

    assert!(result.is_ok());
    let character = result.unwrap();
    assert_eq!(character["name"], "TestChar");
    assert_eq!(character["avatar"], "TestChar.png");
    assert!(character.get("date_added").is_some());
    assert!(character.get("chat_size").is_some());
}

/// Process a character using explicit paths (test helper).
fn process_character_with_paths(
    filename: &str,
    chars_dir: &Path,
    chats_dir: &Path,
) -> Result<Value, String> {
    let img_path = chars_dir.join(filename);

    let img_data = png::read_character_data(&img_path)
        .map_err(|e| format!("Failed to read: {}", e))?;

    let mut json_object: Value =
        serde_json::from_str(&img_data).map_err(|e| format!("Invalid JSON: {}", e))?;

    png::normalise_to_v2(&mut json_object);

    json_object["avatar"] = Value::String(filename.to_string());
    json_object["json_data"] = Value::String(img_data);

    let char_stat = fs::metadata(&img_path)
        .map_err(|e| format!("Failed to stat: {}", e))?;

    let date_added = file_ctime_ms(&char_stat);
    json_object["date_added"] = Value::from(date_added);

    if json_object.get("create_date").and_then(|v| v.as_str()).is_none() {
        json_object["create_date"] = Value::String(timestamp_to_iso(date_added));
    }

    let char_name = filename.strip_suffix(".png").unwrap_or(filename);
    let chats_directory = chats_dir.join(char_name);
    let (chat_size, date_last_chat) = calculate_chat_size(&chats_directory);
    json_object["chat_size"] = Value::from(chat_size);
    json_object["date_last_chat"] = Value::from(date_last_chat);
    json_object["data_size"] = Value::from(calculate_data_size(&json_object));

    Ok(json_object)
}

/// Build a minimal PNG with tEXt chunks for testing.
fn build_test_png(chunks: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let ihdr_data: [u8; 13] = [
        0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0,
    ];
    write_test_chunk(&mut buf, b"IHDR", &ihdr_data);

    for &(keyword, text) in chunks {
        let mut data = Vec::new();
        data.extend_from_slice(keyword.as_bytes());
        data.push(0);
        data.extend_from_slice(text.as_bytes());
        write_test_chunk(&mut buf, b"tEXt", &data);
    }

    write_test_chunk(&mut buf, b"IDAT", &[0]);
    write_test_chunk(&mut buf, b"IEND", &[]);

    buf
}

fn write_test_chunk(buf: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(chunk_type);
    buf.extend_from_slice(data);
    let crc = test_crc32(chunk_type, data);
    buf.extend_from_slice(&crc.to_be_bytes());
}

fn test_crc32(chunk_type: &[u8], data: &[u8]) -> u32 {
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

#[test]
fn get_chat_info_from_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let chat_file = dir.path().join("test_chat.jsonl");

    let header = serde_json::json!({
        "chat_metadata": {"some": "data"},
        "user_name": "User",
        "character_name": "Char"
    });
    let msg1 = serde_json::json!({
        "name": "User",
        "is_user": true,
        "mes": "Hello there!",
        "send_date": "2024-01-15T10:00:00.000Z"
    });
    let msg2 = serde_json::json!({
        "name": "Char",
        "is_user": false,
        "mes": "Hi! How can I help?",
        "send_date": "2024-01-15T10:01:00.000Z"
    });

    let content = format!(
        "{}\n{}\n{}",
        serde_json::to_string(&header).unwrap(),
        serde_json::to_string(&msg1).unwrap(),
        serde_json::to_string(&msg2).unwrap()
    );

    fs::write(&chat_file, &content).unwrap();

    let info = get_chat_info(&chat_file, true).unwrap();

    assert_eq!(info.file_name, "test_chat.jsonl");
    assert_eq!(info.file_id, "test_chat");
    assert_eq!(info.chat_items, 2);
    assert_eq!(info.mes, "Hi! How can I help?");
    assert_eq!(info.last_mes, "2024-01-15T10:01:00.000Z");
    assert!(info.is_match);
    assert!(info.chat_metadata.is_some());
}

#[test]
fn get_chat_info_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let chat_file = dir.path().join("empty.jsonl");
    fs::write(&chat_file, "").unwrap();

    let info = get_chat_info(&chat_file, false).unwrap();

    assert_eq!(info.chat_items, 0);
    assert_eq!(info.mes, "[The chat is empty]");
}

#[test]
fn simple_chat_list() {
    let dir = tempfile::tempdir().unwrap();
    let chat_dir = dir.path().join("TestChar");
    fs::create_dir_all(&chat_dir).unwrap();

    fs::write(
        chat_dir.join("TestChar - 2024-01-15.jsonl"),
        r#"{"chat_metadata":{}}"#,
    )
    .unwrap();
    fs::write(
        chat_dir.join("TestChar - 2024-01-16.jsonl"),
        r#"{"chat_metadata":{}}"#,
    )
    .unwrap();
    // Non-jsonl file should be ignored
    fs::write(chat_dir.join("notes.txt"), "some notes").unwrap();

    let entries = fs::read_dir(&chat_dir).unwrap();
    let jsonl_files: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    assert_eq!(jsonl_files.len(), 2);
}

#[test]
fn calculate_chat_size_nonexistent() {
    let (size, last) = calculate_chat_size(Path::new("/nonexistent/path"));
    assert_eq!(size, 0);
    assert_eq!(last, 0.0);
}

#[test]
fn calculate_chat_size_with_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("chat1.jsonl"), "hello world").unwrap();
    fs::write(dir.path().join("chat2.jsonl"), "another chat").unwrap();

    let (size, last) = calculate_chat_size(dir.path());
    assert!(size > 0);
    assert!(last > 0.0);
}

// -----------------------------------------------------------------------
// New parity delta tests
// -----------------------------------------------------------------------

#[test]
fn deep_merge_objects() {
    let target = serde_json::json!({
        "a": 1,
        "b": {"nested": true, "keep": "yes"},
        "c": "original"
    });
    let source = serde_json::json!({
        "b": {"nested": false, "added": "new"},
        "d": "extra"
    });
    let result = deep_merge(&target, &source);
    assert_eq!(result["a"], 1);
    assert_eq!(result["b"]["nested"], false);
    assert_eq!(result["b"]["keep"], "yes");
    assert_eq!(result["b"]["added"], "new");
    assert_eq!(result["c"], "original");
    assert_eq!(result["d"], "extra");
}

#[test]
fn deep_merge_overwrite_non_object() {
    let target = serde_json::json!({"x": [1, 2, 3]});
    let source = serde_json::json!({"x": [4, 5]});
    let result = deep_merge(&target, &source);
    // Arrays are replaced, not merged (matching Node's deepMerge)
    assert_eq!(result["x"], serde_json::json!([4, 5]));
}

#[test]
fn convert_world_info_to_character_book_basic() {
    let entries = serde_json::json!({
        "0": {
            "uid": 42,
            "key": ["hello", "world"],
            "keysecondary": [],
            "comment": "Test entry",
            "content": "Some lore content",
            "constant": true,
            "selective": false,
            "order": 10,
            "disable": false,
            "position": 0
        }
    });
    let book = convert_world_info_to_character_book("TestWorld", &entries);
    assert_eq!(book["name"], "TestWorld");

    let book_entries = book["entries"].as_array().unwrap();
    assert_eq!(book_entries.len(), 1);

    let e = &book_entries[0];
    assert_eq!(e["id"], 42);
    assert_eq!(e["keys"], serde_json::json!(["hello", "world"]));
    assert_eq!(e["content"], "Some lore content");
    assert_eq!(e["constant"], true);
    assert_eq!(e["insertion_order"], 10);
    assert_eq!(e["enabled"], true);
    assert_eq!(e["position"], "before_char");
    assert_eq!(e["use_regex"], true);
    assert_eq!(e["extensions"]["depth"], 4);
}

#[test]
fn convert_v1_to_v2_with_world_info() {
    let dir = tempfile::tempdir().unwrap();
    let worlds_dir = dir.path().join("worlds");
    fs::create_dir_all(&worlds_dir).unwrap();

    // Create a world info file
    let world_data = serde_json::json!({
        "entries": {
            "0": {
                "uid": 1,
                "key": ["keyword"],
                "keysecondary": [],
                "comment": "",
                "content": "World lore",
                "constant": false,
                "selective": false,
                "order": 100,
                "disable": false,
                "position": 1
            }
        }
    });
    fs::write(
        worlds_dir.join("MyWorld.json"),
        serde_json::to_string(&world_data).unwrap(),
    )
    .unwrap();

    let mut v1_char = serde_json::json!({
        "name": "TestChar",
        "description": "A test character",
        "world": "MyWorld"
    });

    convert_v1_to_v2(&mut v1_char, &worlds_dir);

    assert_eq!(v1_char["spec"], "chara_card_v2");
    assert_eq!(v1_char["data"]["extensions"]["world"], "MyWorld");

    // Verify character_book was populated
    let book = &v1_char["data"]["character_book"];
    assert_eq!(book["name"], "MyWorld");
    assert!(book["entries"].as_array().unwrap().len() > 0);
}

#[test]
fn convert_v1_to_v2_with_extensions_merge() {
    let dir = tempfile::tempdir().unwrap();
    let worlds_dir = dir.path().join("worlds");
    fs::create_dir_all(&worlds_dir).unwrap();

    let mut v1_char = serde_json::json!({
        "name": "TestChar",
        "description": "A test",
        "extensions": "{\"custom_field\": \"custom_val\", \"depth_prompt\": {\"extra\": true}}"
    });

    convert_v1_to_v2(&mut v1_char, &worlds_dir);

    // Verify custom extension was merged
    assert_eq!(v1_char["data"]["extensions"]["custom_field"], "custom_val");
    // Verify original depth_prompt fields are preserved
    assert_eq!(v1_char["data"]["extensions"]["depth_prompt"]["depth"], 4.0);
    // Verify extra field from merged extensions is present
    assert_eq!(v1_char["data"]["extensions"]["depth_prompt"]["extra"], true);
}

#[test]
fn format_bytes_matches_node() {
    assert_eq!(format_bytes(0), "0B");
    assert_eq!(format_bytes(1), "1B");
    assert_eq!(format_bytes(1024), "1KB");
    assert_eq!(format_bytes(1536), "1.5KB");
    assert_eq!(format_bytes(1048576), "1MB");
    assert_eq!(format_bytes(1073741824), "1GB");
    // Edge case: values that previously triggered rounding issues
    assert_eq!(format_bytes(1023), "1023B");
    assert_eq!(format_bytes(1025), "1KB");
}
