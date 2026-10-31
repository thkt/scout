use super::*;
use crate::fetch::ssrf::StaticDnsResolver;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::accept_async;

type Peer = WebSocketStream<TcpStream>;

async fn pair() -> (Connection, Peer) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "ws://{}/devtools/browser/test",
        listener.local_addr().unwrap()
    );
    let resolver = Arc::new(StaticDnsResolver::public());
    let client = Connection::connect(&endpoint, resolver);
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        accept_async(stream).await.unwrap()
    };
    let (client, server) = tokio::join!(client, server);
    (client.unwrap(), server)
}

async fn receive(peer: &mut Peer, method: &str) -> Value {
    let message = peer.next().await.unwrap().unwrap();
    let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert_eq!(value["method"], method);
    value
}

async fn send(peer: &mut Peer, value: Value) {
    peer.send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

async fn reply(peer: &mut Peer, request: &Value, result: Value) {
    let mut response = json!({"id": request["id"], "result": result});
    if let Some(session) = request.get("sessionId") {
        response["sessionId"] = session.clone();
    }
    send(peer, response).await;
}

async fn exchange(peer: &mut Peer, method: &str, result: Value) {
    let request = receive(peer, method).await;
    reply(peer, &request, result).await;
}

/// [T-CDPW001] CDP replies need no event; known event payloads are typed at receive.
#[test]
fn decodes_replies_and_events() {
    let reply: Envelope = serde_json::from_value(json!({"id": 1, "result": {}})).unwrap();
    assert!(matches!(reply, Envelope::Reply(Reply { id: 1, .. })));
    let event: Envelope = serde_json::from_value(json!({"method": "Fetch.requestPaused", "sessionId": "s", "params": {"requestId": "r", "request": {"url": "https://example.com"}}})).unwrap();
    assert!(matches!(
        event,
        Envelope::Event(EventEnvelope {
            event: Event::Paused(_),
            ..
        })
    ));
    assert!(serde_json::from_value::<Envelope>(json!({"sessionId": "page", "method": "Page.frameStartedLoading", "params": {"frameId": "frame"}})).is_ok());
    assert!(serde_json::from_value::<Envelope>(json!({"method": "DOM.documentUpdated"})).is_ok());
    assert!(
        serde_json::from_value::<Envelope>(
            json!({"method": "Fetch.requestPaused", "params": {"requestId": "r"}})
        )
        .is_err()
    );
}

/// [T-CDPW002] Request gates and load matching work in both event/reply orders.
#[tokio::test]
async fn navigation_services_public_and_private_requests_and_extracts_html() {
    for load_before_reply in [false, true] {
        let (mut client, mut peer) = pair().await;
        let url = ValidatedUrl::for_test("https://example.com");
        let navigate = client.navigate(&url);
        let server = async {
            exchange(
                &mut peer,
                "Target.createTarget",
                json!({"targetId": "target"}),
            )
            .await;
            let attach = receive(&mut peer, "Target.attachToTarget").await;
            assert_eq!(attach["params"]["flatten"], true);
            reply(&mut peer, &attach, json!({"sessionId": "page"})).await;
            exchange(&mut peer, "Page.enable", json!({})).await;
            exchange(&mut peer, "Page.setLifecycleEventsEnabled", json!({})).await;
            let enable = receive(&mut peer, "Fetch.enable").await;
            assert_eq!(enable["params"]["patterns"][0]["urlPattern"], "*");
            reply(&mut peer, &enable, json!({})).await;
            let navigation = receive(&mut peer, "Page.navigate").await;
            for (url, expected) in [
                ("https://example.com/script.js", "Fetch.continueRequest"),
                ("http://127.0.0.1/private", "Fetch.failRequest"),
                ("file:///etc/passwd", "Fetch.failRequest"),
            ] {
                send(&mut peer, json!({"sessionId": "page", "method": "Fetch.requestPaused", "params": {"requestId": "request", "request": {"url": url}}})).await;
                let intercepted = receive(&mut peer, expected).await;
                assert_eq!(intercepted["params"]["requestId"], "request");
                if expected == "Fetch.failRequest" {
                    assert_eq!(intercepted["params"]["errorReason"], "BlockedByClient");
                }
                reply(&mut peer, &intercepted, json!({})).await;
            }
            let load = json!({"sessionId": "page", "method": "Page.lifecycleEvent", "params": {"name": "load", "frameId": "main", "loaderId": "navigation"}});
            if load_before_reply {
                send(&mut peer, load.clone()).await;
            }
            reply(
                &mut peer,
                &navigation,
                json!({"frameId": "main", "loaderId": "navigation"}),
            )
            .await;
            if !load_before_reply {
                send(&mut peer, json!({"sessionId": "page", "method": "Page.lifecycleEvent", "params": {"name": "load", "frameId": "iframe", "loaderId": "navigation"}})).await;
                send(&mut peer, json!({"sessionId": "page", "method": "Page.lifecycleEvent", "params": {"name": "load", "frameId": "main", "loaderId": "old-loader"}})).await;
                assert!(
                    timeout(Duration::from_millis(25), peer.next())
                        .await
                        .is_err(),
                    "unrelated loads must not start DOM extraction"
                );
                send(&mut peer, load).await;
            }
            exchange(&mut peer, "DOM.getDocument", json!({"root": {"nodeId": 1}})).await;
            exchange(
                &mut peer,
                "DOM.getOuterHTML",
                json!({"outerHTML": "<html>rendered</html>"}),
            )
            .await;
        };
        let (html, ()) = timeout(Duration::from_secs(5), async {
            tokio::join!(navigate, server)
        })
        .await
        .unwrap();
        assert_eq!(html.unwrap(), "<html>rendered</html>");
    }
}

/// [T-CDPW003] An interceptor error wins even if the command reply arrived first.
#[tokio::test]
async fn interception_failure_is_not_hidden_by_a_ready_command_reply() {
    let (mut client, mut peer) = pair().await;
    client.session = Some("page".into());
    let command = client.command::<Value>("Page.navigate", json!({}), true);
    let server = async {
        let navigation = receive(&mut peer, "Page.navigate").await;
        send(&mut peer, json!({"sessionId": "page", "method": "Fetch.requestPaused", "params": {"requestId": "r", "request": {"url": "https://example.com"}}})).await;
        let intercept = receive(&mut peer, "Fetch.continueRequest").await;
        reply(&mut peer, &navigation, json!({})).await;
        send(&mut peer, json!({"sessionId": "page", "id": intercept["id"], "error": {"code": -32000, "message": "interception failed"}})).await;
    };
    let (result, ()) = timeout(Duration::from_secs(5), async {
        tokio::join!(command, server)
    })
    .await
    .unwrap();
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("intercept execute failed")
    );
}

/// [T-CDPW004] Replies from another session cannot satisfy the active command.
#[tokio::test]
async fn ignores_other_sessions_and_handles_ping() {
    let (mut client, mut peer) = pair().await;
    client.session = Some("page".into());
    let command = client.command::<Value>("Page.enable", json!({}), true);
    let server = async {
        let request = receive(&mut peer, "Page.enable").await;
        send(
            &mut peer,
            json!({"id": request["id"], "sessionId": "other", "result": {"wrong": true}}),
        )
        .await;
        send(&mut peer, json!({"sessionId": "other", "method": "Fetch.requestPaused", "params": {"requestId": "r", "request": {"url": "https://example.com"}}})).await;
        peer.send(Message::Ping(vec![1, 2].into())).await.unwrap();
        assert!(matches!(
            peer.next().await.unwrap().unwrap(),
            Message::Pong(_)
        ));
        reply(&mut peer, &request, json!({"correct": true})).await;
    };
    let (result, ()) = timeout(Duration::from_secs(5), async {
        tokio::join!(command, server)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap(), json!({"correct": true}));
}

/// [T-CDPW005] Missing required result fields surface a protocol error.
#[tokio::test]
async fn rejects_malformed_command_results() {
    let (mut client, mut peer) = pair().await;
    let command = client.command::<Target>("Target.createTarget", json!({}), false);
    let server = exchange(&mut peer, "Target.createTarget", json!({}));
    let (result, ()) = timeout(Duration::from_secs(5), async {
        tokio::join!(command, server)
    })
    .await
    .unwrap();
    assert!(result.err().unwrap().to_string().contains("targetId"));
}
