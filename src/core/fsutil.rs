//! Small filesystem helpers: atomic writes and private permissions.

use crate::core::error::{Error, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Write `content` to `path` atomically: write `<path>.tmp`, fsync, rename.
/// With `backup`, the previous file is copied to `<path>.bak` first.
pub fn write_atomic(path: &Path, content: &[u8], mode: u32, backup: bool) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(format!("create {}", dir.display()), e))?;
    }
    let tmp = path.with_extension(append_ext(path, "tmp"));
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)
            .map_err(|e| Error::io(format!("write {}", tmp.display()), e))?;
        f.write_all(content)
            .and_then(|_| f.sync_all())
            .map_err(|e| Error::io(format!("write {}", tmp.display()), e))?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
        .map_err(|e| Error::io(format!("chmod {}", tmp.display()), e))?;
    if backup && path.exists() {
        let bak = path.with_extension(append_ext(path, "bak"));
        fs::copy(path, &bak).map_err(|e| Error::io(format!("backup to {}", bak.display()), e))?;
    }
    fs::rename(&tmp, path).map_err(|e| Error::io(format!("rename to {}", path.display()), e))?;
    if let Some(dir) = path.parent() {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// `servers.toml` + `tmp` → `toml.tmp` (so the result is `servers.toml.tmp`).
fn append_ext(path: &Path, ext: &str) -> String {
    match path.extension() {
        Some(e) => format!("{}.{ext}", e.to_string_lossy()),
        None => ext.to_string(),
    }
}

/// Create `dir` (and parents) and set it to mode 0700.
pub fn private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| Error::io(format!("create {}", dir.display()), e))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| Error::io(format!("chmod {}", dir.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("servers.toml");
        write_atomic(&p, b"one", 0o600, true).unwrap();
        write_atomic(&p, b"two", 0o600, true).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two");
        assert_eq!(
            fs::read_to_string(dir.path().join("servers.toml.bak")).unwrap(),
            "one"
        );
        assert!(!dir.path().join("servers.toml.tmp").exists());
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
