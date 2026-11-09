//! Real HTTP responses with declarative request dependencies, without timing assertions.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

type Route = (&'static str, Value, &'static [&'static str]);

pub(crate) struct HttpFixture {
    pub(crate) uri: String,
    task: JoinHandle<()>,
}

impl HttpFixture {
    /// A response waits until every path in its third tuple field has arrived.
    /// Dropping the fixture stops the listener and every unfinished connection.
    pub(crate) async fn start(routes: &[Route]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        let routes: HashMap<_, _> = routes
            .iter()
            .map(|(path, body, dependencies)| (*path, (body.to_string(), *dependencies)))
            .collect();
        let routes = Arc::new(routes);
        let (requests, _) = watch::channel(HashSet::<String>::new());
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.unwrap();
                        let routes = routes.clone();
                        let requests = requests.clone();
                        connections.spawn(async move {
                            let mut request = Vec::new();
                            let mut chunk = [0; 1024];
                            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                                let n = stream.read(&mut chunk).await.unwrap();
                                if n == 0 {
                                    return;
                                }
                                request.extend_from_slice(&chunk[..n]);
                            }
                            let request = String::from_utf8(request).unwrap();
                            let path = request.split_whitespace().nth(1).unwrap();
                            let path = path.split('?').next().unwrap();
                            requests.send_modify(|seen| { seen.insert(path.to_owned()); });
                            let (body, dependencies) = routes.get(path).expect("unexpected fixture request");
                            requests.subscribe().wait_for(|seen| {
                                dependencies.iter().all(|path| seen.contains(*path))
                            }).await.unwrap();
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            stream.write_all(response.as_bytes()).await.unwrap();
                        });
                    }
                    Some(result) = connections.join_next() => { result.unwrap(); }
                }
            }
        });
        Self { uri, task }
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
