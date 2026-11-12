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
//! * a task that dies without a result (a panic) is reported as a failed connection, so the name is freed and
//!   the request that started it is answered;
//! * dropping the book (shutdown) aborts the tasks and tells each waiting request that ClawDot is stopping.

use crate::app::Fail;
use crate::extensions::MAX_PORTAL_SERVERS;
use crate::mcp::{McpTool, PendingServer};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::task::{AbortHandle, JoinHandle};

/// The longest a connection may take altogether (a handshake and a listing, each bounded by the server's own
/// timeout of at most 60 s, would otherwise allow three times that).
pub const CONNECT_LIMIT: Duration = Duration::from_secs(65);

/// Where the answer to the portal request that started a connection goes.
pub type Reply = oneshot::Sender<Result<Value, Fail>>;

/// What the loop is told when a connection task ended without a result (it panicked or was aborted).
const DIED: &str = "The connection could not be made.";

/// Aborts the task it holds when dropped, so cancelling a connection also stops the work under it.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What a connection task reports to the loop.
#[derive(Debug)]
pub struct Done {
    name: String,
    generation: u64,
    /// The tools the server listed, or why it cannot be used.
    pub tools: Result<Vec<McpTool>, String>,
    /// The server, ready to be added. Shared with the task that loaded it, which may have died.
    pub pending: Arc<PendingServer>,
}

/// Who is waiting for a connection.
#[derive(Debug)]
pub enum Source {
    /// A portal request that adds the server; the loop answers it when the result is applied.
    Portal(Reply),
    /// A source already saved (or defined in the configuration file), connected at start-up. Nobody waits; a
    /// failure leaves it listed and reported.
    Saved,
}

struct Flight {
    generation: u64,
    task: AbortHandle,
    source: Source,
}

/// The connections in flight and the channel their results come back on. Owned by the loop.
pub struct Connects {
    tx: mpsc::Sender<Done>,
    rx: Option<mpsc::Receiver<Done>>,
    next: u64,
    open: BTreeMap<String, Flight>,
    /// How many saved sources connect at the same time; the others wait their turn. Portal requests are
    /// counted against the same limit by the extensions, not here.
    slots: Arc<Semaphore>,
    /// Test hook: the next connection task panics, as a task that died would.
    #[cfg(test)]
    die_next: bool,
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
            slots: Arc::new(Semaphore::new(MAX_PORTAL_SERVERS)),
            #[cfg(test)]
            die_next: false,
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

    /// How many connections are being made for portal requests. Saved sources are already counted among the
    /// extensions' own, so the portal's limit adds only these.
    #[must_use]
    pub fn portal_len(&self) -> usize {
        self.open
            .values()
            .filter(|f| matches!(f.source, Source::Portal(_)))
            .count()
    }

    /// Test hook: the connection task started next panics, as a task that died would.
    #[cfg(test)]
    pub(crate) fn make_next_task_die(&mut self) {
        self.die_next = true;
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
        self.spawn(name, pending, Source::Portal(reply), None);
    }

    /// Start connecting a source that was saved or defined in the configuration, at start-up. At most
    /// [`MAX_PORTAL_SERVERS`] saved sources connect at once; the rest wait for a free place. The time a source
    /// waits for its turn does not count against its own time limit.
    pub fn start_saved(&mut self, pending: PendingServer) {
        let name = pending.name().to_owned();
        if !self.open.contains_key(&name) {
            let slots = Arc::clone(&self.slots);
            self.spawn(name, pending, Source::Saved, Some(slots));
        }
    }

    fn spawn(
        &mut self,
        name: String,
        pending: PendingServer,
        source: Source,
        slots: Option<Arc<Semaphore>>,
    ) {
        self.next += 1;
        let generation = self.next;
        let tx = self.tx.clone();
        let task_name = name.clone();
        let pending = Arc::new(pending);
        #[cfg(test)]
        let die = std::mem::take(&mut self.die_next);
        // Two tasks: one does the work, and this one waits for it and reports. If the work dies, the report
        // is still sent (as a failure), so the flight never stays on the book with nobody to settle it.
        let task = tokio::spawn(async move {
            let load = Arc::clone(&pending);
            let mut work = AbortOnDrop(tokio::spawn(async move {
                // Held while connecting, and given back before the result waits for the loop.
                let slot = match slots {
                    Some(s) => s.acquire_owned().await.ok(),
                    None => None,
                };
                #[cfg(test)]
                #[allow(clippy::panic)]
                if die {
                    panic!("test: the connection task died");
                }
                let tools = match tokio::time::timeout(CONNECT_LIMIT, load.load()).await {
                    Ok(r) => r,
                    Err(_) => Err(format!("no answer within {} s", CONNECT_LIMIT.as_secs())),
                };
                drop(slot);
                tools
            }));
            let tools = (&mut work.0).await.unwrap_or_else(|_| Err(DIED.to_owned()));
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
                source,
            },
        );
    }

    /// A task reported. Returns who was waiting when this is the connection now on the book, and `None` for a
    /// result whose connection was cancelled or replaced meanwhile (it is dropped).
    pub fn finish(&mut self, done: &Done) -> Option<Source> {
        match self.open.get(&done.name) {
            Some(f) if f.generation == done.generation => {
                self.open.remove(&done.name).map(|f| f.source)
            }
            _ => None,
        }
    }

    /// Stop connecting this name and answer the portal request that started it. False if it was not connecting
    /// for a portal request (a saved source is left to [`Connects::forget_saved`]).
    pub fn cancel(&mut self, name: &str) -> bool {
        match self.open.remove(name) {
            Some(Flight {
                task,
                source: Source::Portal(reply),
                ..
            }) => {
                task.abort();
                let _ = reply.send(Err(Fail::Invalid(format!(
                    "The connection {name} was removed before it finished connecting."
                ))));
                true
            }
            // A saved source is left to `forget_saved`.
            Some(saved) => {
                self.open.insert(name.to_owned(), saved);
                false
            }
            None => false,
        }
    }

    /// The saved source of this name was removed: stop connecting it and drop its result.
    pub fn forget_saved(&mut self, name: &str) {
        if matches!(
            self.open.get(name),
            Some(Flight {
                source: Source::Saved,
                ..
            })
        ) && let Some(f) = self.open.remove(name)
        {
            f.task.abort();
        }
    }
}

impl Drop for Connects {
    fn drop(&mut self) {
        for (_, f) in std::mem::take(&mut self.open) {
            f.task.abort();
            if let Source::Portal(reply) = f.source {
                let _ = reply.send(Err(Fail::Unavailable(
                    "ClawDot is stopping, so the connection was not made.".into(),
                )));
            }
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

    // Saved sources connect together but never more than the connection limit at once; the rest wait their
    // turn and all of them finish.
    #[tokio::test(flavor = "current_thread")]
    async fn saved_sources_connect_concurrently_up_to_the_limit() {
        let (open_tx, open) = watch::channel(false);
        let port = gated_server(open).await;
        let mut c = Connects::new();
        let mut rx = c.attach();
        let total = MAX_PORTAL_SERVERS + 3;
        for i in 0..total {
            c.start_saved(pending(port, &format!("s{i}")).await);
        }
        assert_eq!((c.len(), c.portal_len()), (total, 0));
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(c.slots.available_permits(), 0, "the limit is in use");
        assert!(rx.try_recv().is_err(), "nobody has answered yet");

        open_tx.send_replace(true);
        for _ in 0..total {
            let done = rx.recv().await.unwrap();
            assert!(done.tools.is_ok());
            assert!(matches!(c.finish(&done), Some(Source::Saved)));
        }
        assert!(c.is_empty());
        assert_eq!(c.slots.available_permits(), MAX_PORTAL_SERVERS);
    }

    // `declare` lists every server as connecting, without touching the network; `settle` records each result
    // and keeps a failed server listed with its reason.
    #[tokio::test(flavor = "current_thread")]
    async fn declared_servers_are_connecting_until_settled_and_a_failed_one_stays_listed() {
        let (_open_tx, open) = watch::channel(true);
        let port = gated_server(open).await;
        let def = |name: &str, port: u16| -> crate::mcp::ServerDef {
            serde_json::from_value(
                json!({"name": name, "url": format!("http://127.0.0.1:{port}/mcp"), "timeout_secs": 1}),
            )
            .unwrap()
        };
        let (mut client, pending) =
            McpClient::declare(vec![def("up", port), def("down", 9)], |_| None).unwrap();
        assert_eq!(pending.len(), 2);
        assert!(client.is_connecting("up") && client.is_connecting("down"));
        assert_eq!(client.report()[0]["connecting"], true);
        assert_eq!(client.report()[0]["connected"], false);
        assert!(client.tools().is_empty());

        let up = pending[0].load().await;
        assert!(client.settle("up", up, &[]).is_empty());
        let down = pending[1].load().await;
        let warnings = client.settle("down", down, &[]);
        assert!(
            warnings[0].contains("`down` is unavailable"),
            "{warnings:?}"
        );
        assert!(!client.is_connecting("up") && !client.is_connecting("down"));
        let r = client.report();
        assert_eq!(
            (r[0]["connected"].clone(), r[0]["connecting"].clone()),
            (true.into(), false.into())
        );
        assert_eq!(
            (r[1]["connected"].clone(), r[1]["connecting"].clone()),
            (false.into(), false.into())
        );
        assert!(r[1]["error"].is_string());
        assert!(client.owns("up__ping") && !client.owns("down__ping"));
        // A name that is gone is ignored; a clash with a reserved name is skipped with a warning.
        assert!(client.settle("gone", Ok(Vec::new()), &[]).is_empty());
        let (mut other, p) = McpClient::declare(vec![def("up", port)], |_| None).unwrap();
        let warnings = other.settle("up", p[0].load().await, &["up__ping".to_owned()]);
        assert!(warnings[0].contains("clashes"), "{warnings:?}");
        assert!(other.tools().is_empty());
    }

    // A connection task that dies without a result must not leave its flight on the book: the loop is told
    // it failed, the name is free again, and a portal request would be answered.
    #[tokio::test(flavor = "current_thread")]
    async fn a_task_that_dies_is_reported_as_a_failed_connection_and_frees_the_name() {
        let (_open_tx, open) = watch::channel(true);
        let port = gated_server(open).await;
        let mut c = Connects::new();
        let mut rx = c.attach();

        let (r, _answer) = oneshot::channel();
        c.make_next_task_die();
        c.start(pending(port, "home").await, r);
        let done = rx.recv().await.unwrap();
        assert_eq!(done.tools.as_ref().err().map(String::as_str), Some(DIED));
        assert!(matches!(c.finish(&done), Some(Source::Portal(_))));
        assert!(!c.is_connecting("home") && c.is_empty());

        c.make_next_task_die();
        c.start_saved(pending(port, "saved").await);
        let done = rx.recv().await.unwrap();
        assert!(done.tools.is_err());
        assert!(matches!(c.finish(&done), Some(Source::Saved)));
        assert!(c.is_empty());

        // The name connects again, and this time the task lives.
        let (r, _answer) = oneshot::channel();
        c.start(pending(port, "home").await, r);
        let done = rx.recv().await.unwrap();
        assert!(done.tools.is_ok());
        assert!(c.finish(&done).is_some());
    }

    // The cancel of a portal connection leaves a saved one alone.
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_leaves_a_saved_connection_on_the_book() {
        let (_hold, open) = watch::channel(false);
        let port = gated_server(open).await;
        let mut c = Connects::new();
        c.start_saved(pending(port, "saved").await);
        assert!(!c.cancel("saved"));
        assert!(c.is_connecting("saved"));
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
