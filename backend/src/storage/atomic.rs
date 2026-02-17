//! Atomic file write utilities.
//!
//! Mirrors Node's `write-file-atomic` package behavior:
//! writes data to a temporary file in the same directory, then
//! renames atomically via `rename(2)`.  This prevents partial writes
//! or corruption on crash.

use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

/// Write `data` atomically to `path`.
///
/// 1. Creates a `NamedTempFile` in the **same directory** as `path`
///    (required for same-filesystem rename).
/// 2. Writes all bytes to the temp file.
/// 3. Calls `persist()` which does an atomic `rename(2)`.
///
/// If the rename fails the temp file is cleaned up automatically.
pub fn atomic_write_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(data)?;
    tmp.flush()?;

    // `persist` does an atomic rename on Unix.
    // On Windows it uses `MoveFileEx` with `REPLACE_EXISTING`.
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_file() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("hello.txt");

        atomic_write_file(&target, b"hello world").unwrap();

        assert!(target.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("data.bin");

        std::fs::write(&target, b"old content").unwrap();
        atomic_write_file(&target, b"new content").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new content");
    }

    #[test]
    fn atomic_write_preserves_on_nonexistent_parent() {
        // Writing to a path whose parent doesn't exist should fail
        // without leaving partial files.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("nonexistent_dir").join("file.txt");

        let result = atomic_write_file(&target, b"data");
        assert!(result.is_err());
        // No partial file should exist
        assert!(!target.exists());
    }

    #[test]
    fn atomic_write_empty_data() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("empty.bin");

        atomic_write_file(&target, b"").unwrap();

        assert!(target.exists());
        assert_eq!(std::fs::read(&target).unwrap().len(), 0);
    }

    #[test]
    fn atomic_write_large_data() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("large.bin");
        let data = vec![0xABu8; 1024 * 1024]; // 1 MB

        atomic_write_file(&target, &data).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), data);
    }
}
