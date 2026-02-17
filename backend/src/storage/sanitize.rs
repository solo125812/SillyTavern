//! Filename sanitization and path traversal checks.
//!
//! Provides Rust equivalents of the Node `sanitize-filename` npm package
//! and the [`isPathUnderParent()`](../../../src/util.js:1376) utility.
//!
//! These must match the Node behavior exactly to preserve filename parity
//! across the migration.

use std::path::Path;

// ---------------------------------------------------------------------------
// Characters forbidden by sanitize-filename
// ---------------------------------------------------------------------------

/// Characters replaced by `sanitize-filename`.
/// Mirrors: /[\/\?<>\\:\*\|"]/g
const ILLEGAL_CHARS: &[char] = &['/', '\\', '<', '>', ':', '"', '|', '?', '*'];

/// Windows reserved device names (case-insensitive).
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Maximum filename length (matches `sanitize-filename` default).
const MAX_FILENAME_LENGTH: usize = 255;

// ---------------------------------------------------------------------------
// sanitize_filename — mirrors npm `sanitize-filename`
// ---------------------------------------------------------------------------

/// Sanitize a filename to be safe for filesystem use.
///
/// Mirrors the behavior of the `sanitize-filename` npm package:
/// 1. Replace illegal characters (`/ ? < > \ : * | "`) with `replacement`.
/// 2. Replace C0/C1 control chars (0x00–0x1F, 0x80–0x9F) with `replacement`.
/// 3. Replace reserved filenames (`.` or `..` or any all-dot name) with `replacement`.
/// 4. Replace Windows reserved device names (CON, PRN, AUX, NUL, COMx, LPTx) with `replacement`.
/// 5. Strip trailing dots/spaces (Windows trailing rule).
/// 6. Truncate to 255 bytes (UTF-8 safe).
///
/// # Arguments
/// - `input` — The raw filename string.
/// - `replacement` — Character to substitute for illegal chars (typically `""`
///   or `"_"`). Pass an empty string to strip illegal characters.
pub fn sanitize_filename(input: &str, replacement: &str) -> String {
    let output = sanitize_internal(input, replacement);
    if replacement.is_empty() {
        output
    } else {
        sanitize_internal(&output, "")
    }
}

/// Convenience wrapper: sanitize with no replacement character (strip illegal chars).
pub fn sanitize_filename_strip(input: &str) -> String {
    sanitize_filename(input, "")
}

fn is_control_c0_c1(ch: char) -> bool {
    let code = ch as u32;
    (code <= 0x1F) || (code >= 0x80 && code <= 0x9F)
}

fn sanitize_internal(input: &str, replacement: &str) -> String {
    // Step 1-2: Replace illegal + control characters
    let mut sanitized = String::with_capacity(input.len());
    for ch in input.chars() {
        if ILLEGAL_CHARS.contains(&ch) || is_control_c0_c1(ch) {
            sanitized.push_str(replacement);
        } else {
            sanitized.push(ch);
        }
    }

    // Step 3: Replace all-dot names (., .., ...)
    if !sanitized.is_empty() && sanitized.chars().all(|c| c == '.') {
        sanitized = replacement.to_string();
    }

    // Step 4: Windows reserved device names (case-insensitive), with or without extension
    let lower = sanitized.to_lowercase();
    let base = lower.split('.').next().unwrap_or("");
    if WINDOWS_RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(base))
        && (lower == base || lower.starts_with(&format!("{}.", base)))
    {
        sanitized = replacement.to_string();
    }

    // Step 5: Strip trailing dots/spaces
    while sanitized.ends_with(['.', ' ']) {
        sanitized.pop();
    }

    // Step 6: Truncate to 255 bytes (UTF-8 safe)
    truncate_utf8_bytes(&sanitized, MAX_FILENAME_LENGTH)
}

fn truncate_utf8_bytes(input: &str, max_bytes: usize) -> String {
    if input.as_bytes().len() <= max_bytes {
        return input.to_string();
    }
    let mut end = 0;
    for (idx, ch) in input.char_indices() {
        let next = idx + ch.len_utf8();
        if next <= max_bytes {
            end = next;
        } else {
            break;
        }
    }
    input[..end].to_string()
}

// ---------------------------------------------------------------------------
// Safe character replacements — mirrors Node's custom sanitize replacement
// ---------------------------------------------------------------------------

/// The replacement string used by SillyTavern's character sanitization.
/// Matches `sanitizeSafeCharacterReplacements` from `src/endpoints/characters.js`.
pub const SAFE_CHARACTER_REPLACEMENT: &str = "_";

/// Sanitize a character name for use as a filename.
/// Uses `"_"` as the replacement character, matching SillyTavern's convention.
pub fn sanitize_character_name(name: &str) -> String {
    sanitize_filename(name, SAFE_CHARACTER_REPLACEMENT)
}

// ---------------------------------------------------------------------------
// Path traversal checks
// ---------------------------------------------------------------------------

/// Check if `child_path` is under `parent_path` after normalization.
///
/// Mirrors [`isPathUnderParent()`](../../../src/util.js:1376):
/// ```js
/// const relativePath = path.relative(normalizedParent, normalizedChild);
/// return !relativePath.startsWith('..') && !path.isAbsolute(relativePath);
/// ```
///
/// Returns `true` if the child path is safely contained within the parent.
pub fn is_path_under_parent(parent_path: &Path, child_path: &Path) -> bool {
    // Use canonical paths when possible, fall back to lexical normalization
    let normalized_parent = normalize_path(parent_path);
    let normalized_child = normalize_path(child_path);

    match normalized_child.strip_prefix(&normalized_parent) {
        Ok(relative) => {
            // Ensure the relative path doesn't escape upward
            !relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        }
        Err(_) => false,
    }
}

/// Lexically normalize a path (resolve `.` and `..` components without
/// touching the filesystem). This is a pure string operation.
///
/// Unlike `std::fs::canonicalize()`, this does not require the path to exist.
fn normalize_path(path: &Path) -> std::path::PathBuf {
    use std::path::Component;

    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Pop the last component if possible
                if !normalized.pop() {
                    // If we can't pop, keep the `..` (for relative paths)
                    normalized.push("..");
                }
            }
            Component::CurDir => {
                // Skip `.` components
            }
            _ => {
                normalized.push(component);
            }
        }
    }
    normalized
}

// ---------------------------------------------------------------------------
// Asset filename validation
// ---------------------------------------------------------------------------

/// Unsafe file extensions that should be rejected for uploaded assets.
/// Mirrors `UNSAFE_EXTENSIONS` from [`src/constants.js:64`](../../../src/constants.js:64).
pub const UNSAFE_EXTENSIONS: &[&str] = &[
    ".php",
    ".exe",
    ".com",
    ".dll",
    ".pif",
    ".application",
    ".gadget",
    ".msi",
    ".jar",
    ".cmd",
    ".bat",
    ".reg",
    ".sh",
    ".py",
    ".js",
    ".jse",
    ".jsp",
    ".pdf",
    ".html",
    ".htm",
    ".hta",
    ".vb",
    ".vbs",
    ".vbe",
    ".cpl",
    ".msc",
    ".scr",
    ".sql",
    ".iso",
    ".img",
    ".dmg",
    ".ps1",
    ".ps1xml",
    ".ps2",
    ".ps2xml",
    ".psc1",
    ".psc2",
    ".msh",
    ".msh1",
    ".msh2",
    ".mshxml",
    ".msh1xml",
    ".msh2xml",
    ".scf",
    ".lnk",
    ".inf",
    ".doc",
    ".docm",
    ".docx",
    ".dot",
    ".dotm",
    ".dotx",
    ".xls",
    ".xlsm",
    ".xlsx",
    ".xlt",
    ".xltm",
    ".xltx",
    ".xlam",
    ".ppt",
    ".pptm",
    ".pptx",
    ".pot",
    ".potm",
    ".potx",
    ".ppam",
    ".ppsx",
    ".ppsm",
    ".pps",
    ".sldx",
    ".sldm",
    ".ws",
];

/// Result of asset filename validation.
#[derive(Debug)]
pub struct FilenameValidation {
    /// Whether the filename is valid.
    pub valid: bool,
    /// Human-readable error message if invalid.
    pub message: Option<String>,
}

/// Validate a filename for asset upload.
///
/// Mirrors [`validateAssetFileName()`](../../../src/endpoints/assets.js:20)
/// in Node:
/// 1. Only alphanumeric, `_`, `-`, `.` are accepted.
/// 2. Unsafe extensions are rejected.
/// 3. Filenames starting with `.` are rejected.
/// 4. Reserved or long filenames (per `sanitize-filename`) are rejected.
pub fn validate_asset_filename(filename: &str) -> FilenameValidation {
    // Check for illegal characters (only alphanumeric, _, -, . are allowed)
    if !filename
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return FilenameValidation {
            valid: false,
            message: Some(
                "Illegal character in filename; only alphanumeric, '_', '-' are accepted."
                    .to_string(),
            ),
        };
    }

    // Check for unsafe extensions
    let extension = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()));
    if let Some(ref ext) = extension {
        if UNSAFE_EXTENSIONS.contains(&ext.as_str()) {
            return FilenameValidation {
                valid: false,
                message: Some("Forbidden file extension.".to_string()),
            };
        }
    }

    // Reject filenames starting with '.'
    if filename.starts_with('.') {
        return FilenameValidation {
            valid: false,
            message: Some("Filename cannot start with '.'.".to_string()),
        };
    }

    // Reject reserved or too-long filenames
    let sanitized = sanitize_filename_strip(filename);
    if sanitized != filename {
        return FilenameValidation {
            valid: false,
            message: Some("Reserved or long filename.".to_string()),
        };
    }

    FilenameValidation {
        valid: true,
        message: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- sanitize_filename tests --

    #[test]
    fn strips_control_characters() {
        let input = "hello\x00world\x1F!";
        assert_eq!(sanitize_filename_strip(input), "helloworld!");
    }

    #[test]
    fn strips_path_separators() {
        let input = "path/to\\file";
        assert_eq!(sanitize_filename_strip(input), "pathtofile");
    }

    #[test]
    fn strips_reserved_characters() {
        let input = "file<>:\"|?*name";
        assert_eq!(sanitize_filename_strip(input), "filename");
    }

    #[test]
    fn replaces_with_custom_replacement() {
        let input = "hello/world";
        assert_eq!(sanitize_filename(input, "_"), "hello_world");
    }

    #[test]
    fn handles_windows_reserved_names() {
        assert_eq!(sanitize_filename_strip("CON"), "");
        assert_eq!(sanitize_filename_strip("con.txt"), "");
        assert_eq!(sanitize_filename("CON", "_"), "_");
        assert_eq!(sanitize_filename("con.txt", "_"), "_");
    }

    #[test]
    fn strips_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename_strip("...hello..."), "...hello");
        assert_eq!(sanitize_filename_strip("  hello  "), "  hello");
        assert_eq!(sanitize_filename_strip("..hello..world.."), "..hello..world");
        assert_eq!(sanitize_filename_strip("..."), "");
    }

    #[test]
    fn truncates_long_filenames() {
        let input = "a".repeat(300);
        let result = sanitize_filename_strip(&input);
        assert_eq!(result.len(), MAX_FILENAME_LENGTH);
    }

    #[test]
    fn preserves_normal_filenames() {
        assert_eq!(sanitize_filename_strip("hello.txt"), "hello.txt");
        assert_eq!(
            sanitize_filename_strip("My Character (v2).png"),
            "My Character (v2).png"
        );
    }

    #[test]
    fn character_name_sanitization() {
        assert_eq!(sanitize_character_name("Normal Name"), "Normal Name");
        assert_eq!(sanitize_character_name("Name/With\\Slashes"), "Name_With_Slashes");
        assert_eq!(sanitize_character_name("Name<>Brackets"), "Name__Brackets");
    }

    // -- is_path_under_parent tests --

    #[test]
    fn child_under_parent_returns_true() {
        assert!(is_path_under_parent(
            Path::new("/data"),
            Path::new("/data/users/file.txt")
        ));
    }

    #[test]
    fn child_equals_parent_returns_true() {
        assert!(is_path_under_parent(
            Path::new("/data"),
            Path::new("/data")
        ));
    }

    #[test]
    fn child_escapes_parent_returns_false() {
        assert!(!is_path_under_parent(
            Path::new("/data/users"),
            Path::new("/data/other/file.txt")
        ));
    }

    #[test]
    fn dotdot_traversal_returns_false() {
        assert!(!is_path_under_parent(
            Path::new("/data/users"),
            Path::new("/data/users/../../etc/passwd")
        ));
    }

    #[test]
    fn relative_paths_work() {
        assert!(is_path_under_parent(
            Path::new("data"),
            Path::new("data/subdir/file.txt")
        ));
        assert!(!is_path_under_parent(
            Path::new("data"),
            Path::new("other/file.txt")
        ));
    }

    // -- validate_asset_filename tests --

    #[test]
    fn valid_asset_filenames() {
        assert!(validate_asset_filename("image.png").valid);
        assert!(validate_asset_filename("my-file_v2.webp").valid);
        assert!(validate_asset_filename("audio.mp3").valid);
    }

    #[test]
    fn rejects_illegal_characters() {
        assert!(!validate_asset_filename("hello world.png").valid);
        assert!(!validate_asset_filename("file@name.png").valid);
    }

    #[test]
    fn rejects_unsafe_extensions() {
        assert!(!validate_asset_filename("script.js").valid);
        assert!(!validate_asset_filename("payload.exe").valid);
        assert!(!validate_asset_filename("page.html").valid);
    }

    #[test]
    fn rejects_dot_prefix() {
        assert!(!validate_asset_filename(".hidden").valid);
    }
}
