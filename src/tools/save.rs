//! Writing a fetched image to the path given by `fetch --output`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use super::ScoutError;

/// Fail before the download when `path` exists or its directory does not, so
/// an unusable path costs no request. [`save_new_file`] still decides at write
/// time: the filesystem can change while the image downloads.
pub(super) fn ensure_new_file_can_be_created(path: &Path) -> Result<(), ScoutError> {
    // Not `exists()`: that follows a dangling symlink, which `create_new` rejects.
    if path.symlink_metadata().is_ok() {
        return Err(already_exists(path));
    }
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if !directory.is_dir() {
        return Err(ScoutError::io_error(format!(
            "cannot write --output file {}: {} is not an existing directory",
            path.display(),
            directory.display()
        )));
    }
    Ok(())
}

/// Create `path` and write `bytes` to it. An existing file is never replaced.
pub(super) fn save_new_file(path: &Path, bytes: &[u8]) -> Result<(), ScoutError> {
    save_new_file_with(path, |file| file.write_all(bytes))
}

/// `write` is the seam for a failure after the file exists, which a real
/// filesystem only produces when it is full.
fn save_new_file_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), ScoutError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == ErrorKind::AlreadyExists {
                already_exists(path)
            } else {
                write_failure(path, &e)
            }
        })?;
    // Sync before reporting success: a full or remote filesystem can accept
    // the write into its cache and report the failure only on flush.
    let written = write(&mut file).and_then(|()| file.sync_all());
    drop(file);
    written.map_err(|e| {
        // `create_new` succeeded, so this run created the file being removed.
        let _ = fs::remove_file(path);
        write_failure(path, &e)
    })
}

fn already_exists(path: &Path) -> ScoutError {
    ScoutError::io_error(format!(
        "--output file already exists: {}; choose a path that does not exist",
        path.display()
    ))
}

fn write_failure(path: &Path, e: &io::Error) -> ScoutError {
    ScoutError::io_error(format!(
        "cannot write --output file {}: {e}",
        path.display()
    ))
}

#[cfg(test)]
mod tests;
