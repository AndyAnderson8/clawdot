//! MCP connections made beside the main loop (ADR-0021).
//!
//! Adding an MCP server from the portal means a handshake and a tool listing over the network, which can take
//! as long as the server's timeout. The loop validates the request and hands the network half to a task
//! ([`PendingServer::load`]); the task sends its result back as a [`Done`]; the loop alone applies it to the
//! extensions and answers the portal. This module keeps the book of connections in flight so the races are
//! decided in one place:
//!
//! * a name can be connecting once; a second request for it is refused;
//! * removing a name that is still connecting cancels it: the task is aborted and the request that started it
//!   is answered;
//! * every connection has a generation, and a result is applied only if it carries the generation now on the
//!   book. A result that was already queued when its connection was cancelled (and perhaps started again) is
//!   dropped, so it can neither resurrect a removed source nor overwrite a newer one;
//! * dropping the book (shutdown) aborts the tasks and tells each waiting request that ClawDot is stopping.

use crate::app::Fail;
use crate::mcp::{McpTool, PendingServer};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

/// The longest a connection may take altogether (a handshake and a listing, each bounded by the server's own
/// timeout of at most 60 s, would otherwise allow three times that).
pub const CONNECT_LIMIT: Duration = Duration::from_secs(65);

/// Where the answer to the portal request that started a connection goes.
pub type Reply = oneshot::Sender<Result<Value, Fail>>;

/// What a connection task reports to the loop.
#[derive(Debug)]
pub struct Done {
    name: String,
    generation: u64,
    /// The tools the server listed, or why it cannot be used.
    pub tools: Result<Vec<McpTool>, String>,
    /// The server, ready to be added.
    pub pending: PendingServer,
}

struct Flight {
    generation: u64,
    task: AbortHandle,
    reply: Reply,
}

/// The connections in flight and the channel their results come back on. Owned by the loop.
pub struct Connects {
    tx: mpsc::Sender<Done>,
    rx: Option<mpsc::Receiver<Done>>,
    next: u64,
    open: BTreeMap<String, Flight>,
}

impl Default for Connects {
    fn default() -> Self {
        Self::new()
    }
}

impl Connects {
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(8);
        Self {
            tx,
            rx: Some(rx),
            next: 0,
            open: BTreeMap::new(),
        }
    }

    /// Take the receiving end, for the loop to wait on beside its other arms. Only the first call gets
    /// results; a later one gets a receiver that is closed.
    pub fn attach(&mut self) -> mpsc::Receiver<Done> {
        self.rx.take().unwrap_or_else(|| mpsc::channel(1).1)
    }

    /// How many connections are being made.
    #[must_use]
    pub fn len(&self) -> usize {
        self.open.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Whether this name is being connected now.
    #[must_use]
    pub fn is_connecting(&self, name: &str) -> bool {
        self.open.contains_key(name)
    }

    /// Start the network half in a task. The caller has checked that the name is not connecting already; a
    /// request for a name that is gets its answer at once instead (and the first connection carries on).
    pub fn start(&mut self, pending: PendingServer, reply: Reply) {
        let name = pending.name().to_owned();
        if self.open.contains_key(&name) {
            let _ = reply.send(Err(Fail::Invalid(format!(
                "A connection named {name} is already being made. Wait for it, or remove it first."
            ))));
            return;
        }
        self.next += 1;
        let generation = self.next;
        let tx = self.tx.clone();
        let task_name = name.clone();
        let task = tokio::spawn(async move {
            let tools = match tokio::time::timeout(CONNECT_LIMIT, pending.load()).await {
                Ok(r) => r,
                Err(_) => Err(format!("no answer within {} s", CONNECT_LIMIT.as_secs())),
            };
            let _ = tx
                .send(Done {
                    name: task_name,
                    generation,
                    tools,
                    pending,
                })
                .await;
        })
        .abort_handle();
        self.open.insert(
            name,
            Flight {
                generation,
                task,
                reply,
            },
        );
    }

    /// A task reported. Returns the request to answer when this is the connection now on the book, and `None`
    /// for a result whose connection was cancelled or replaced meanwhile (it is dropped).
    pub fn finish(&mut self, done: &Done) -> Option<Reply> {
        match self.open.get(&done.name) {
            Some(f) if f.generation == done.generation => {
                self.open.remove(&done.name).map(|f| f.reply)
            }
            _ => None,
        }
    }

    /// Stop connecting this name and answer the request that started it. False if it was not connecting.
    pub fn cancel(&mut self, name: &str) -> bool {
        let Some(f) = self.open.remove(name) else {
            return false;
        };
        f.task.abort();
        let _ = f.reply.send(Err(Fail::Invalid(format!(
            "The connection {name} was removed before it finished connecting."
        ))));
        true
    }
}

impl Drop for Connects {
    fn drop(&mut self) {
        for (_, f) in std::mem::take(&mut self.open) {
            f.task.abort();
            let _ = f.reply.send(Err(Fail::Unavailable(
                "ClawDot is stopping, so the connection was not made.".into(),
            )));
        }
    }
}

impl std::fmt::Debug for Connects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connects")
            .field("in_flight", &self.open.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// A loopback MCP server that answers only once the test lets it, for tests of work in flight.
#[cfg(test)]
pub(crate) mod testutil {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::watch;

    /// Starts the server. Requests are read at once, but no reply is sent until `open` holds true. Returns
    /// the port. The one tool it lists is `ping`.
    pub(crate) async fn gated_server(open: watch::Receiver<bool>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    return;
                };
                let mut open = open.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 8192];
                    let body = loop {
                        let Ok(n) = c.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&raw[..p]).to_ascii_lowercase();
                            let want: usize = head
                                .lines()
                                .find_map(|l| {
                                    l.strip_prefix("content-length:")?.trim().parse().ok()
                                })
                                .unwrap_or(0);
                            if raw.len() >= p + 4 + want {
                                break raw[p + 4..p + 4 + want].to_vec();
                            }
                        }
                    };
                    let msg: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    if open.wait_for(|v| *v).await.is_err() {
                        return;
                    }
                    let id = msg["id"].clone();
                    let (status, extra, payload) = match msg["method"].as_str().unwrap_or("") {
                        "initialize" => (
                            "200 OK",
                            "Mcp-Session-Id: s\r\n",
                            json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":crate::mcp::PROTOCOL_REVISION,"capabilities":{"tools":{}}}}).to_string(),
                        ),
                        "tools/list" => (
                            "200 OK",
                            "",
                            json!({"jsonrpc":"2.0","id":id,"result":{"tools":[{"name":"ping","description":"p","inputSchema":{"type":"object"}}]}}).to_string(),
                        ),
                        _ => ("202 Accepted", "", String::new()),
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = c.write_all(resp.as_bytes()).await;
                });
            }
        });
        port
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::gated_server;
    use super::*;
    use crate::mcp::McpClient;
    use serde_json::json;
    use tokio::sync::watch;

    async fn pending(port: u16, name: &str) -> PendingServer {
        let (client, _) = McpClient::connect(Vec::new(), &[], |_| None).await.unwrap();
        client
            .prepare_server(
                serde_json::from_value(
                    json!({"name": name, "url": format!("http://127.0.0.1:{port}/mcp")}),
                )
                .unwrap(),
                |_| None,
            )
            .unwrap()
    }

    // A result that was already queued when its connection was cancelled, and the name started again, must
    // not be applied: it is neither the connection on the book nor allowed to answer its request.
    #[tokio::test(flavor = "current_thread")]
    async fn a_result_queued_before_a_cancel_is_dropped_even_if_the_name_connects_again() {
        let (open_tx, open) = watch::channel(true);
        let port = gated_server(open).await;
        let mut c = Connects::new();
        let mut rx = c.attach();
        let (r1, a1) = oneshot::channel();
        c.start(pending(port, "home").await, r1);
        let stale = rx.recv().await.unwrap();
        assert!(c.cancel("home"), "it was still on the book");
        assert!(matches!(a1.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("removed")));

        open_tx.send_replace(false);
        let (r2, mut a2) = oneshot::channel();
        c.start(pending(port, "home").await, r2);
        assert!(c.finish(&stale).is_none(), "the old result is dropped");
        assert!(c.is_connecting("home"), "the new connection is untouched");
        assert!(a2.try_recv().is_err(), "and its request is still waiting");
        assert!(!c.cancel("nope"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_name_connects_once_at_a_time() {
        let (_hold, open) = watch::channel(false);
        let port = gated_server(open).await;
        let mut c = Connects::new();
        let (r1, mut a1) = oneshot::channel();
        c.start(pending(port, "home").await, r1);
        let (r2, a2) = oneshot::channel();
        c.start(pending(port, "home").await, r2);
        assert!(matches!(a2.await.unwrap(), Err(Fail::Invalid(m)) if m.contains("already")));
        assert_eq!(c.len(), 1);
        assert!(a1.try_recv().is_err(), "the first is still waiting");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_with_a_connection_in_flight_stops_it_and_answers_the_request() {
        let (_hold, open) = watch::channel(false);
        let port = gated_server(open).await;
        let mut c = Connects::new();
        let (r, a) = oneshot::channel();
        c.start(pending(port, "home").await, r);
        drop(c);
        assert!(matches!(a.await.unwrap(), Err(Fail::Unavailable(m)) if m.contains("stopping")));
    }
}
