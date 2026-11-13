use super::*;

pub(in crate::http::tests) const STOP: &str = r#"{"session_id":"s1","cwd":"/home/a/clawdot","hook_event_name":"Stop","last_assistant_message":"Done."}"#;

pub(in crate::http::tests) struct Harness {
    pub(in crate::http::tests) core: Arc<Core>,
    pub(in crate::http::tests) addr: SocketAddr,
    pub(in crate::http::tests) token: String,
    pub(in crate::http::tests) _shutdown: oneshot::Sender<()>,
}

pub(in crate::http::tests) async fn start(limits: Limits, opts: ServeOptions) -> Harness {
    start_with_clock(Arc::new(SystemClock), limits, opts).await
}

pub(in crate::http::tests) async fn start_with_clock(
    clock: Arc<dyn crate::sched::Clock + Send + Sync>,
    limits: Limits,
    opts: ServeOptions,
) -> Harness {
    let core = Arc::new(Core::in_memory(clock, Adapters::builtin(), limits));
    start_core(core, opts).await
}

pub(in crate::http::tests) async fn start_core(core: Arc<Core>, opts: ServeOptions) -> Harness {
    let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let served = Arc::clone(&core);
    tokio::spawn(async move {
        let _ = serve(listener, served, opts, async {
            let _ = rx.await;
        })
        .await;
    });
    Harness {
        core,
        addr,
        token,
        _shutdown: tx,
    }
}

/// One request, one response, then the connection closes.
pub(in crate::http::tests) async fn call(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    call_for(addr, "127.0.0.1", method, path, headers, body).await
}

/// Like [`call`], with the `Host` header the request is addressed to.
pub(in crate::http::tests) async fn call_for(
    addr: SocketAddr,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    call_from("127.0.0.1", addr, host, method, path, headers, body).await
}

/// Like [`call_for`], sent from the local address `source` (any 127.x.y.z address is this machine), so a test can
/// be several different clients.
pub(in crate::http::tests) async fn call_from(
    source: &str,
    addr: SocketAddr,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(format!("{source}:0").parse().unwrap()).unwrap();
    let mut s = socket.connect(addr).await.unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, head.to_owned(), body.to_owned())
}

pub(in crate::http::tests) fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Read from an SSE connection until `needle` appears or time runs out.
pub(in crate::http::tests) async fn read_until(s: &mut TcpStream, buf: &mut String, needle: &str) {
    let mut chunk = [0_u8; 1024];
    timeout(Duration::from_secs(5), async {
        while !buf.contains(needle) {
            let n = s.read(&mut chunk).await.unwrap();
            assert!(n > 0, "stream closed early; got {buf:?}");
            buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got {buf:?}"));
}

pub(in crate::http::tests) async fn open_stream(addr: SocketAddr, token: &str) -> TcpStream {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /bridge/claude-code/replies HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nAccept: text/event-stream\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    s
}

/// Complete setup with a password and return the signed-in session as an Authorization value.
pub(in crate::http::tests) fn admin(h: &Harness) -> String {
    let hash = crate::admin::hash_password("adm-secret").unwrap();
    let token = h
        .core
        .admin_setup_password([127, 0, 0, 1].into(), hash)
        .unwrap();
    bearer(&token)
}

pub(in crate::http::tests) fn json_of(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap()
}

pub(in crate::http::tests) async fn post(
    h: &Harness,
    path: &str,
    auth: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    let hdrs: Vec<(&str, &str)> = if auth.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization", auth)]
    };
    let (status, _, b) = call(h.addr, "POST", path, &hdrs, body).await;
    (status, serde_json::from_str(&b).unwrap_or_default())
}
