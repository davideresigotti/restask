//! Crash-safe file writes shared by every adapter (vault notes, `.restask/` state, config).
//!
//! The markdown, vtodo and planner modules stay pure; anything that touches the disk goes
//! through here or through an adapter module that calls into here.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Suffix of the hidden sibling file a write goes through (git-ignored as `*.restask-tmp`).
const TMP_SUFFIX: &str = "restask-tmp";

/// The temp sibling for `path`: `<dir>/.<name>.restask-tmp` (same directory, so the final
/// rename is atomic on POSIX; hidden, so vault scanners and Syncthing ignore patterns skip it).
fn tmp_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: path has no file name", path.display()),
        )
    })?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(".");
    tmp_name.push(TMP_SUFFIX);
    Ok(dir.join(tmp_name))
}

/// Writes `contents` to `path` atomically: temp sibling, write, fsync, rename.
///
/// A leftover temp file from an interrupted earlier write is overwritten, so a crash can
/// never wedge later writes. `unix_mode` forces the file mode; when `None` the mode of the
/// file being replaced is kept (a fresh file gets the process default).
pub fn write_atomic_mode(path: &Path, contents: &str, unix_mode: Option<u32>) -> io::Result<()> {
    let tmp = tmp_path(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(mode) = unix_mode {
            options.mode(mode);
        }
    }
    let mut file = options.open(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = unix_mode.or_else(|| {
            fs::metadata(path)
                .ok()
                .map(|meta| meta.permissions().mode() & 0o7777)
        });
        if let Some(mode) = mode {
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
    }
    #[cfg(not(unix))]
    let _ = unix_mode;
    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written.and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

/// [`write_atomic_mode`] keeping the replaced file's mode.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    write_atomic_mode(path, contents, None)
}

/// Writes only when the file's current contents differ, so an idempotent pass never bumps
/// an mtime (no Syncthing churn, no watcher echo). Returns whether a write happened.
pub fn write_if_changed(path: &Path, contents: &str) -> io::Result<bool> {
    match fs::read_to_string(path) {
        Ok(current) if current == contents => Ok(false),
        _ => write_atomic(path, contents).map(|()| true),
    }
}
