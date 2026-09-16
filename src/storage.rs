use anyhow::{Context, Result, ensure};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Replace a sensitive file atomically without making its contents publicly readable.
pub fn write_private_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut directories = fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directories.mode(0o700);
    }
    directories
        .create(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;

    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_file(),
            "{} must be a regular file, not a directory or symlink",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = path
        .file_name()
        .context("missing output filename")?
        .to_os_string();
    name.push(format!(
        ".{}-{}-{}.tmp",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary_path = parent.join(name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary_path)
        .with_context(|| format!("failed to create private checkpoint for {}", path.display()))?;
    let temporary = TemporaryFile(temporary_path);
    let written = file.write_all(data).and_then(|_| file.sync_all());
    drop(file);
    written?;
    fs::rename(&temporary.0, path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "cli-tools-storage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn token_files_are_private_on_creation_and_replacement() {
        use std::os::unix::fs::PermissionsExt;
        let directory = Directory::new();
        let path = directory.0.join("config/tokens.json");
        let mut tokens = crate::auth::TokenSet {
            access_token: "test-access".into(),
            id_token: "test-id".into(),
            refresh_token: "test-refresh".into(),
            expires_in: None,
            token_type: None,
        };
        crate::auth::persist_tokens(&tokens, Some(&path)).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        tokens.refresh_token = "new-refresh".into();
        crate::auth::persist_tokens(&tokens, Some(&path)).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            crate::auth::load_tokens(&path).unwrap().refresh_token,
            "new-refresh"
        );
    }

    #[test]
    fn readers_only_observe_complete_replacements() {
        let directory = Directory::new();
        let path = directory.0.join("session.json");
        let first = vec![b'a'; 64 * 1024];
        let second = vec![b'b'; 64 * 1024];
        write_private_atomic(&path, &first).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..50 {
                    let data = fs::read(&path).unwrap();
                    assert!(data == first || data == second);
                }
            });
            for _ in 0..5 {
                write_private_atomic(&path, &second).unwrap();
                write_private_atomic(&path, &first).unwrap();
            }
        });
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_without_changing_their_targets() {
        let directory = Directory::new();
        let target = directory.0.join("original.json");
        let link = directory.0.join("tokens.json");
        fs::write(&target, b"original").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_private_atomic(&link, b"replacement").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(fs::symlink_metadata(link).unwrap().is_symlink());
    }
}
