//! Media file listing utilities.
//!
//! Mirrors Node's [`getImages()`](../../../src/util.js:669) function
//! which lists files from a directory filtered by MIME type.

use std::cmp::Ordering;
use std::ffi::CString;
use std::fs;
use std::path::Path;
use std::sync::Once;

// ---------------------------------------------------------------------------
// Media request types (bitwise flags, matching Node's MEDIA_REQUEST_TYPE)
// ---------------------------------------------------------------------------

/// Bitwise flag for image files.
pub const MEDIA_IMAGE: u32 = 0b001;
/// Bitwise flag for video files.
pub const MEDIA_VIDEO: u32 = 0b010;
/// Bitwise flag for audio files.
pub const MEDIA_AUDIO: u32 = 0b100;

/// Sort field for media listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    /// Sort by name using locale-aware comparison.
    Name,
    /// Sort by file modification time.
    Date,
    /// No sorting.
    None,
}

impl SortField {
    /// Parse from a string, defaulting to `Name`.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "date" => SortField::Date,
            "name" => SortField::Name,
            _ => SortField::None,
        }
    }
}

/// Sort order for media listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    /// Parse from a string, defaulting to `Asc`.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "desc" => SortOrder::Desc,
            _ => SortOrder::Asc,
        }
    }
}

/// List media files in a directory, filtered by MIME type and sorted.
///
/// Mirrors Node's `getImages()` from `src/util.js:669-702`:
/// - Reads directory entries
/// - Filters to files only
/// - Filters by MIME type (image/video/audio) based on `media_type` flags
/// - Sorts by name (locale-aware) or date
pub fn list_media_files(
    directory: &Path,
    sort_by: SortField,
    media_type: u32,
) -> Vec<String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
        })
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| {
            let mime = mime_guess::from_path(name).first();
            match mime {
                Some(m) => {
                    let type_str = m.type_().as_str();
                    if (media_type & MEDIA_IMAGE) != 0 && type_str == "image" {
                        return true;
                    }
                    if (media_type & MEDIA_VIDEO) != 0 && type_str == "video" {
                        return true;
                    }
                    if (media_type & MEDIA_AUDIO) != 0 && type_str == "audio" {
                        return true;
                    }
                    false
                }
                None => false,
            }
        })
        .collect();

    match sort_by {
        SortField::Name => {
            // Locale-aware sort — mirrors Intl.Collator().compare.
            files.sort_by(|a, b| locale_compare(a, b));
        }
        SortField::Date => {
            files.sort_by(|a, b| {
                let mtime_a = fs::metadata(directory.join(a))
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let mtime_b = fs::metadata(directory.join(b))
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                mtime_a.cmp(&mtime_b)
            });
        }
        SortField::None => {}
    }

    files
}

fn locale_compare(a: &str, b: &str) -> Ordering {
    static INIT: Once = Once::new();

    INIT.call_once(|| unsafe {
        let locale = CString::new("").unwrap_or_else(|_| CString::new("C").unwrap());
        libc::setlocale(libc::LC_COLLATE, locale.as_ptr());
    });

    let a_c = match CString::new(a) {
        Ok(value) => value,
        Err(_) => return a.cmp(b),
    };
    let b_c = match CString::new(b) {
        Ok(value) => value,
        Err(_) => return a.cmp(b),
    };

    let result = unsafe { libc::strcoll(a_c.as_ptr(), b_c.as_ptr()) };
    if result < 0 {
        Ordering::Less
    } else if result > 0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// List media files filtered to images only, sorted by name.
/// Convenience wrapper for the common case.
pub fn list_image_files(directory: &Path) -> Vec<String> {
    list_media_files(directory, SortField::Name, MEDIA_IMAGE)
}

// ---------------------------------------------------------------------------
// Media extensions list (mirrors Node's MEDIA_EXTENSIONS)
// ---------------------------------------------------------------------------

/// Valid media file extensions, matching Node's `MEDIA_EXTENSIONS` from
/// [`src/constants.js:504-528`](../../../src/constants.js:504).
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "bmp", "png", "jpg", "webp", "jpeg", "jfif", "gif", "mp4", "avi", "mov",
    "wmv", "flv", "webm", "3gp", "mkv", "mpg", "mp3", "wav", "ogg", "flac",
    "aac", "m4a", "aiff",
];

/// Check if a format/extension string is a valid media extension.
pub fn is_valid_media_extension(ext: &str) -> bool {
    MEDIA_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn list_image_files_returns_images_only() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("photo.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        File::create(dir.path().join("video.mp4"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        File::create(dir.path().join("readme.txt"))
            .unwrap()
            .write_all(b"text")
            .unwrap();
        File::create(dir.path().join("avatar.jpg"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();

        let images = list_image_files(dir.path());
        assert_eq!(images, vec!["avatar.jpg", "photo.png"]);
    }

    #[test]
    fn list_media_with_video_flag() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("clip.mp4"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        File::create(dir.path().join("photo.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();

        let result = list_media_files(dir.path(), SortField::Name, MEDIA_VIDEO);
        assert_eq!(result, vec!["clip.mp4"]);
    }

    #[test]
    fn list_media_with_combined_flags() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("clip.mp4"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        File::create(dir.path().join("photo.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();

        let result =
            list_media_files(dir.path(), SortField::Name, MEDIA_IMAGE | MEDIA_VIDEO);
        assert_eq!(result, vec!["clip.mp4", "photo.png"]);
    }

    #[test]
    fn list_media_empty_dir() {
        let dir = TempDir::new().unwrap();
        let images = list_image_files(dir.path());
        assert!(images.is_empty());
    }

    #[test]
    fn list_media_nonexistent_dir() {
        let images = list_image_files(Path::new("/nonexistent/path/for/test"));
        assert!(images.is_empty());
    }

    #[test]
    fn is_valid_media_extension_works() {
        assert!(is_valid_media_extension("png"));
        assert!(is_valid_media_extension("PNG"));
        assert!(is_valid_media_extension("mp4"));
        assert!(!is_valid_media_extension("exe"));
        assert!(!is_valid_media_extension("rs"));
    }
}
