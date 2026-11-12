//! Image saving through the real fetch CLI and a local HTTP proxy.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;
use std::sync::atomic::Ordering;

const URL: &str = "http://scout-audit.example/picture";

/// Each body pairs a valid signature with bytes that are not UTF-8, so a text
/// decode on the way to disk would change them.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\xff\xfe\x80 pixels";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\xff\xfe\x80";
const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\xff\xfe\x00";
const WEBP: &[u8] = b"RIFF\x1a\x00\x00\x00WEBPVP8 \xff\xfe\x80\x00";

/// [T-C061] `--output` writes the response bytes unchanged and reports the
/// path, final URL, canonical media type and size as a frontmatter block.
#[test]
fn image_response_is_saved_verbatim_and_reported_as_frontmatter() {
    for (content_type, mime, body) in [
        ("image/png", "image/png", PNG),
        ("IMAGE/JPEG", "image/jpeg", JPEG),
        ("image/gif; q=1", "image/gif", GIF),
        (" image/webp ", "image/webp", WEBP),
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("saved.bin");
        let Some(output) = fetch(content_type, body, &["--output", path_str(&path)]) else {
            return;
        };

        assert_eq!(output.status.code(), Some(0), "{content_type}: {output:?}");
        assert_eq!(fs::read(&path).expect("saved file"), body, "{content_type}");
        assert_eq!(
            String::from_utf8(output.stdout).expect("UTF-8 output"),
            format!(
                "---\npath: \"{}\"\nurl: \"{URL}\"\ncontent_type: \"{mime}\"\nbytes: {}\n---\n",
                path_str(&path),
                body.len()
            ),
            "{content_type}"
        );
    }
}

/// [T-C062] `--json` carries the same four fields in `data` and no image bytes.
#[test]
fn image_response_json_data_has_path_url_content_type_and_bytes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("saved.png");
    let Some(output) = fetch("image/png", PNG, &["--output", path_str(&path), "--json"]) else {
        return;
    };

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(
        envelope["data"],
        serde_json::json!({
            "path": path_str(&path),
            "url": URL,
            "content_type": "image/png",
            "bytes": PNG.len(),
        })
    );
    assert_eq!(fs::read(&path).expect("saved file"), PNG);
}

/// [T-C063] An image without `--output` fails with exit 65 and names the flag.
#[test]
fn image_response_without_output_is_rejected_with_an_output_hint() {
    let Some(output) = fetch("image/png", PNG, &[]) else {
        return;
    };

    assert_eq!(output.status.code(), Some(65), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--output"), "{stderr}");
}

/// [T-C064] A body whose leading bytes do not match the declared image type
/// fails with exit 65 and leaves no file.
#[test]
fn image_body_not_matching_its_content_type_is_rejected_without_a_file() {
    for (content_type, body) in [
        (
            "image/png",
            b"<html><body>not a picture</body></html>".as_slice(),
        ),
        ("image/png", JPEG),
        ("image/jpeg", PNG),
        ("image/gif", b"GIF90a"),
        ("image/webp", b"RIFF\x1a\x00\x00\x00WAVEfmt "),
        ("image/png", b"\x89PNG"),
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("saved.bin");
        let Some(output) = fetch(content_type, body, &["--output", path_str(&path)]) else {
            return;
        };

        assert_eq!(output.status.code(), Some(65), "{content_type}: {output:?}");
        assert!(output.stdout.is_empty(), "{content_type}: {output:?}");
        assert!(!path.exists(), "{content_type}: file must not be created");
    }
}

/// [T-C065] An existing file at `--output` fails with exit 74 and keeps its bytes.
#[test]
fn existing_output_file_is_not_overwritten() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("saved.png");
    fs::write(&path, b"earlier content").expect("seed file");
    let Some(output) = fetch("image/png", PNG, &["--output", path_str(&path)]) else {
        return;
    };

    assert_eq!(output.status.code(), Some(74), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(fs::read(&path).expect("seed file"), b"earlier content");
}

/// [T-C066] A missing parent directory fails with exit 74 and creates nothing.
#[test]
fn output_path_in_a_missing_directory_fails_without_creating_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent = dir.path().join("absent");
    let path = parent.join("saved.png");
    let Some(output) = fetch("image/png", PNG, &["--output", path_str(&path)]) else {
        return;
    };

    assert_eq!(output.status.code(), Some(74), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(!parent.exists(), "parent directory must not be created");
}

/// [T-C067] `--output` with a response outside the four raster types fails
/// with exit 65, prints no body and leaves no file.
#[test]
fn non_image_response_with_output_is_rejected_without_a_file() {
    for content_type in [
        "text/html",
        "text/plain",
        "application/xml",
        "image/svg+xml",
        "image/bmp",
        "application/json",
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("saved.bin");
        let Some(output) = fetch(
            content_type,
            b"<svg><p>body text</p></svg>",
            &["--output", path_str(&path)],
        ) else {
            return;
        };

        assert_eq!(output.status.code(), Some(65), "{content_type}: {output:?}");
        assert!(output.stdout.is_empty(), "{content_type}: {output:?}");
        assert!(!path.exists(), "{content_type}: file must not be created");
    }
}

/// [T-C068] `--js` with an image fails with exit 65, names the flag and
/// leaves no file. The second body is not PNG data: the flag conflict is
/// reported from the headers, ahead of any failure the body would cause.
#[test]
fn image_response_with_js_is_rejected_without_a_file() {
    for body in [PNG, b"not png data".as_slice()] {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("saved.png");
        let Some(output) = fetch("image/png", body, &["--js", "--output", path_str(&path)]) else {
            return;
        };

        assert_eq!(output.status.code(), Some(65), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("remove --js"), "{stderr}");
        assert!(!path.exists(), "file must not be created");
    }
}

/// [T-C069] `--js --output` on a page reports that the response is not an
/// image, so the hint names the flag whose removal lets the fetch succeed.
#[test]
fn page_response_with_js_and_output_reports_the_media_type_mismatch() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("saved.png");
    let Some(output) = fetch(
        "text/html",
        b"<p>page</p>",
        &["--js", "--output", path_str(&path)],
    ) else {
        return;
    };

    assert_eq!(output.status.code(), Some(65), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("remove --output"), "{stderr}");
    assert!(!stderr.contains("remove --js"), "{stderr}");
    assert!(!path.exists(), "file must not be created");
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("UTF-8 temp path")
}

/// Serve one response through the mock proxy and run `scout fetch` with the
/// extra arguments. Returns None under the shared bind-skip policy.
fn fetch(content_type: &str, body: &[u8], args: &[&str]) -> Option<Output> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let (proxy, connections, handle) = common::spawn_mock_proxy_raw_response(&response)?;
    let output = common::scout_with_clean_env()
        .env("HTTP_PROXY", &proxy)
        .args(["fetch", URL])
        .args(args)
        .output()
        .expect("scout should launch");
    assert_eq!(connections.load(Ordering::SeqCst), 1, "{output:?}");
    handle.join().expect("proxy should finish");
    Some(output)
}
