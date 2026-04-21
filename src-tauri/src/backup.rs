//! Backup file orchestration on the Rust side.
//!
//! The UI owns the encryption / decryption of the bundle (so the mnemonic
//! never leaves the renderer process); this module only handles filesystem
//! reads and writes of the ciphertext blob.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs;

/// Name of the current on-disk backup, relative to `$APPDATA/edet`.
pub const CURRENT_BACKUP_FILENAME: &str = "backup.edet-backup";

pub fn current_backup_path(backup_dir: &Path) -> PathBuf {
    backup_dir.join(CURRENT_BACKUP_FILENAME)
}

/// Read the current backup ciphertext, if it exists on disk.
pub async fn read_current(backup_dir: &Path) -> Result<Option<Vec<u8>>> {
    let path = current_backup_path(backup_dir);
    match fs::read(&path).await {
        Ok(bytes) if bytes.is_empty() => Ok(None),
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Atomically replace the current backup file (empty slice clears it).
///
/// Atomic here means "write to a sibling `.tmp` file then rename"; an
/// interrupted write never leaves us with a half-baked backup the restore
/// flow would refuse to decrypt.
pub async fn write_current(backup_dir: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(backup_dir).await.ok();
    let target = current_backup_path(backup_dir);
    if bytes.is_empty() {
        match fs::remove_file(&target).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("clearing {}", target.display())),
        }
    } else {
        let tmp = backup_dir.join(format!("{CURRENT_BACKUP_FILENAME}.tmp"));
        fs::write(&tmp, bytes)
            .await
            .with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &target)
            .await
            .with_context(|| format!("renaming {} → {}", tmp.display(), target.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns `None` when the target file does not exist.
    #[tokio::test]
    async fn read_current_missing_returns_none() {
        let tmp = tempdir();
        let got = read_current(tmp.path()).await.unwrap();
        assert!(got.is_none());
    }

    /// Round-trips non-empty payloads atomically.
    #[tokio::test]
    async fn write_current_round_trip() {
        let tmp = tempdir();
        let payload = b"edet-smoke-test-bundle-content";
        write_current(tmp.path(), payload).await.unwrap();
        let got = read_current(tmp.path()).await.unwrap();
        assert_eq!(got.as_deref(), Some(&payload[..]));
        // Ensure we actually committed a real file, not just a tmp.
        assert!(current_backup_path(tmp.path()).exists());
    }

    /// Empty payload clears the file and subsequent read returns None.
    #[tokio::test]
    async fn write_current_empty_clears_file() {
        let tmp = tempdir();
        write_current(tmp.path(), b"initial").await.unwrap();
        assert!(read_current(tmp.path()).await.unwrap().is_some());
        write_current(tmp.path(), &[]).await.unwrap();
        assert!(read_current(tmp.path()).await.unwrap().is_none());
    }

    /// Helper: create a unique temp directory inside the Cargo target dir.
    /// We avoid the `tempfile` crate to keep dev-deps small.
    fn tempdir() -> TempGuard {
        let dir = std::env::temp_dir().join(format!(
            "edet-backup-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempGuard(dir)
    }

    struct TempGuard(PathBuf);
    impl TempGuard {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
