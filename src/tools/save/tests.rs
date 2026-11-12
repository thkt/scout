use std::io;

use super::save_new_file_with;
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
