use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use super::params::FetchParams;
use super::test_helpers::scout_reaching;
use crate::test_support::try_spawn_mock_server;

/// [T-TS046] An interrupt signalled while the image downloads is stored, and
/// the fetch then writes no file. `join!` polls the fetch up to its first
/// network wait before the signal, the order the interrupt handler produces.
#[tokio::test]
async fn interrupted_image_fetch_writes_no_file() {
    let Some(server) = try_spawn_mock_server("tools::image_fetch").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/picture.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"\x89PNG\r\n\x1a\n pixels".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let addr = *server.address();
    let scout = scout_reaching(addr);
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("saved.png");
    let mut params = FetchParams::for_test(&format!(
        "http://scout-test.example:{}/picture.png",
        addr.port()
    ));
    params.output = Some(saved.to_str().unwrap().to_owned());

    let (result, signalled) = tokio::join!(scout.fetch(params), async { scout.cancel.send(true) });

    assert!(
        signalled.is_ok(),
        "the fetch must hold a receiver, or `send` drops the interrupt"
    );
    assert!(
        result.is_err(),
        "an interrupted fetch must not report success"
    );
    assert!(
        !saved.exists(),
        "an interrupted fetch must not save the image"
    );
}
