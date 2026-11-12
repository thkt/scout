//! Writing a fetched image to the path given by `fetch --output`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use super::ScoutError;

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
                ScoutError::io_error(format!(
                    "--output file already exists: {}; choose a path that does not exist",
                    path.display()
                ))
            } else {
                write_failure(path, &e)
            }
        })?;
    write(&mut file).map_err(|e| {
        drop(file);
        // `create_new` succeeded, so this run created the file being removed.
        let _ = fs::remove_file(path);
        write_failure(path, &e)
    })
}

fn write_failure(path: &Path, e: &io::Error) -> ScoutError {
    ScoutError::io_error(format!(
        "cannot write --output file {}: {e}",
        path.display()
    ))
}

#[cfg(test)]
mod tests;
