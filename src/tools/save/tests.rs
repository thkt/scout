use std::fs;
use std::io;
use std::path::Path;

use super::{ensure_new_file_can_be_created, save_new_file, save_new_file_with};
use crate::envelope::ErrorCode;

/// [T-TS043] A write that fails after the file was created removes the file
/// and surfaces as an IO error.
#[test]
fn failed_write_removes_the_file_it_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.png");

    let err = save_new_file_with(&path, |file| {
        io::Write::write_all(file, b"\x89PNG")?;
        Err(io::Error::from(io::ErrorKind::StorageFull))
    })
    .unwrap_err();

    assert_eq!(err.error_kind(), ErrorCode::IoError, "{err}");
    assert!(!path.exists(), "partial file must be removed");
}

/// [T-TS044] The write itself refuses an existing file and keeps its bytes,
/// for a file that appears after the pre-download check.
#[test]
fn existing_file_is_not_replaced_at_write_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.png");
    fs::write(&path, b"earlier content").unwrap();

    let err = save_new_file(&path, b"\x89PNG").unwrap_err();

    assert_eq!(err.error_kind(), ErrorCode::IoError, "{err}");
    assert_eq!(fs::read(&path).unwrap(), b"earlier content");
}

/// [T-TS045] The pre-download check rejects an existing path and a missing
/// parent directory, and accepts a new name in an existing directory.
#[test]
fn pre_download_check_rejects_paths_that_cannot_be_created() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("existing.png");
    fs::write(&existing, b"earlier content").unwrap();

    for rejected in [existing, dir.path().join("absent").join("saved.png")] {
        let err = ensure_new_file_can_be_created(&rejected).unwrap_err();
        assert_eq!(err.error_kind(), ErrorCode::IoError, "{err}");
    }
    ensure_new_file_can_be_created(&dir.path().join("new.png")).unwrap();
    ensure_new_file_can_be_created(Path::new("bare-name.png")).unwrap();
}
