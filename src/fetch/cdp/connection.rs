//! A single CDP session; paused requests are serviced while awaiting command replies.

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::warn;

use super::BrowserError;
use super::launch::check_browser_request;
use crate::fetch::ssrf::{DnsResolver, RedactedLogUrl, ValidatedUrl};

#[derive(Deserialize)]
#[serde(untagged)]
enum Envelope {
    Reply(Reply),
    Event(EventEnvelope),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    id: u64,
    session_id: Option<String>,
    result: Option<Value>,
    error: Option<Value>,
}

#[derive(Deserialize)]
#[serde(try_from = "WireEvent")]
struct EventEnvelope {
    session_id: Option<String>,
    event: Event,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEvent {
    session_id: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

enum Event {
    Paused(PausedRequest),
    Lifecycle(Lifecycle),
    Detached,
    Other,
}

impl TryFrom<WireEvent> for EventEnvelope {
    type Error = serde_json::Error;

    fn try_from(wire: WireEvent) -> Result<Self, Self::Error> {
        let event = match wire.method.as_str() {
            "Fetch.requestPaused" => Event::Paused(serde_json::from_value(wire.params)?),
            "Page.lifecycleEvent" => Event::Lifecycle(serde_json::from_value(wire.params)?),
            "Inspector.detached" | "Inspector.targetCrashed" => Event::Detached,
            _ => Event::Other,
        };
        Ok(Self {
            session_id: wire.session_id,
            event,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PausedRequest {
    request_id: String,
    request: RequestUrl,
}

#[derive(Deserialize)]
struct RequestUrl {
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Lifecycle {
    name: String,
    frame_id: String,
    loader_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    target_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Navigation {
    frame_id: String,
    loader_id: Option<String>,
    error_text: Option<String>,
    #[serde(default)]
    is_download: bool,
}

#[derive(Deserialize)]
struct Document {
    root: DocumentNode,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentNode {
    node_id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Html {
    #[serde(rename = "outerHTML")]
    outer_html: String,
}

pub(super) struct Connection {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    session: Option<String>,
    resolver: Arc<dyn DnsResolver>,
    interceptions: HashSet<u64>,
    loads: HashSet<(String, String)>,
}

impl Connection {
    pub(super) async fn connect(
        url: &str,
        resolver: Arc<dyn DnsResolver>,
    ) -> Result<Self, BrowserError> {
        let (socket, _) = connect_async(url)
            .await
            .map_err(|error| failed(format!("browser connect: {error}")))?;
        Ok(Self {
            socket,
            next_id: 0,
            session: None,
            resolver,
            interceptions: HashSet::new(),
            loads: HashSet::new(),
        })
    }

    pub(super) async fn navigate(&mut self, url: &ValidatedUrl) -> Result<String, BrowserError> {
        let target: Target = self
            .command("Target.createTarget", json!({"url": "about:blank"}), false)
            .await?;
        let attached: Session = self
            .command(
                "Target.attachToTarget",
                json!({"targetId": target.target_id, "flatten": true}),
                false,
            )
            .await?;
        self.session = Some(attached.session_id);
        self.command::<Value>("Page.enable", json!({}), true)
            .await?;
        self.command::<Value>(
            "Page.setLifecycleEventsEnabled",
            json!({"enabled": true}),
            true,
        )
        .await?;
        // Install the existing request gate before the first untrusted navigation.
        self.command::<Value>(
            "Fetch.enable",
            json!({"patterns": [{"urlPattern": "*", "requestStage": "Request"}]}),
            true,
        )
        .await?;
        self.loads.clear();
        let navigation: Navigation = self
            .command("Page.navigate", json!({"url": url.as_str()}), true)
            .await?;
        if let Some(error) = navigation.error_text {
            return Err(failed(format!("navigation: {error}")));
        }
        if navigation.is_download {
            return Err(failed("navigation started a download"));
        }
        let loader = navigation
            .loader_id
            .ok_or_else(|| failed("CDP navigation reply missing loaderId"))?;
        let loaded = (navigation.frame_id, loader);
        // Reply/event order varies. Only this navigation's frame and loader can
        // finish the wait; iframe and initial about:blank events do not qualify.
        while !self.loads.contains(&loaded) {
            let message = self.receive().await?;
            self.process(&message).await?;
        }
        let document: Document = self
            .command("DOM.getDocument", json!({"depth": 0}), true)
            .await?;
        let html: Html = self
            .command(
                "DOM.getOuterHTML",
                json!({"nodeId": document.root.node_id}),
                true,
            )
            .await?;
        Ok(html.outer_html)
    }

    pub(super) async fn close(&mut self) -> Result<(), BrowserError> {
        self.command::<Value>("Browser.close", json!({}), false)
            .await
            .map(|_| ())
    }

    async fn send(&mut self, method: &str, params: Value, page: bool) -> Result<u64, BrowserError> {
        self.next_id += 1;
        let mut message = json!({"id": self.next_id, "method": method, "params": params});
        if page {
            message["sessionId"] = json!(self.session);
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .map_err(|error| failed(format!("CDP send: {error}")))?;
        Ok(self.next_id)
    }

    async fn command<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
        page: bool,
    ) -> Result<T, BrowserError> {
        let id = self.send(method, params, page).await?;
        let expected_session = if page { self.session.clone() } else { None };
        let mut reply = None;
        loop {
            if self.interceptions.is_empty()
                && let Some(reply) = reply.take()
            {
                return serde_json::from_value(reply)
                    .map_err(|error| failed(format!("CDP {method} reply: {error}")));
            }
            let message = self.receive().await?;
            self.process(&message).await?;
            let Envelope::Reply(message) = message else {
                continue;
            };
            if message.id != id || message.session_id != expected_session {
                continue;
            }
            if let Some(error) = message.error {
                return Err(failed(format!("CDP {method}: {error}")));
            }
            reply = Some(
                message
                    .result
                    .ok_or_else(|| failed(format!("CDP {method}: missing result")))?,
            );
        }
    }

    async fn receive(&mut self) -> Result<Envelope, BrowserError> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(&text)
                        .map_err(|error| failed(format!("CDP invalid message: {error}")));
                }
                Some(Ok(Message::Ping(payload))) => {
                    self.socket
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| failed(format!("CDP pong: {error}")))?;
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None => return Err(failed("CDP connection closed")),
                Some(Ok(_)) => return Err(failed("CDP expected a text message")),
                Some(Err(error)) => return Err(failed(format!("CDP receive: {error}"))),
            }
        }
    }

    async fn process(&mut self, message: &Envelope) -> Result<(), BrowserError> {
        let message = match message {
            Envelope::Reply(reply) => {
                if reply.session_id == self.session && self.interceptions.remove(&reply.id) {
                    if let Some(error) = &reply.error {
                        return Err(failed(format!("CDP intercept execute failed: {error}")));
                    }
                    if reply.result.is_none() {
                        return Err(failed("CDP intercept reply missing result"));
                    }
                }
                return Ok(());
            }
            Envelope::Event(event) => event,
        };
        if message.session_id != self.session || self.session.is_none() {
            return Ok(());
        }
        match &message.event {
            Event::Paused(event) => {
                let allowed =
                    check_browser_request(&event.request.url, self.resolver.as_ref()).await;
                let (method, params) = if allowed {
                    (
                        "Fetch.continueRequest",
                        json!({"requestId": event.request_id}),
                    )
                } else {
                    warn!(blocked_url = %RedactedLogUrl(&event.request.url), "SSRF: blocked browser subrequest");
                    (
                        "Fetch.failRequest",
                        json!({"requestId": event.request_id, "errorReason": "BlockedByClient"}),
                    )
                };
                let id = self.send(method, params, true).await?;
                self.interceptions.insert(id);
            }
            Event::Lifecycle(event) if event.name == "load" => {
                self.loads
                    .insert((event.frame_id.clone(), event.loader_id.clone()));
            }
            Event::Detached => {
                return Err(failed("CDP page disconnected"));
            }
            _ => {}
        }
        Ok(())
    }
}

fn failed(message: impl Into<String>) -> BrowserError {
    BrowserError::ProcessFailed(message.into())
}
