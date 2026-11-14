//! A tiny HTTP file server for the audio the device is asked to fetch (spec 0004, REQ-ESP-010).
//!
//! A device plays an announcement or a spoken reply by downloading a URL, so the core must serve the audio.
//! This is deliberately not the main web server: it binds its own address, serves only
//! `GET`/`HEAD /esphome/<token>.wav` for files it holds, and keeps at most four small files for two minutes.
//! The token is 128 random bits, so a URL cannot be guessed.
//!
//! A reply can also be offered while it is still being made (a *live* file): the response is a WAV whose header
//! declares no length, sent chunked (HTTP/1.1) or until the connection closes (HTTP/1.0), and ends when the
//! writer finishes, when it is dropped, when the response has been open too long, or when the reader goes away.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};

/// Most files held at once.
const MAX_FILES: usize = 4;
/// Largest file held (4 MiB, about two minutes of 16 kHz speech).
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
/// How long a file stays available.
const TTL: Duration = Duration::from_secs(120);
/// Request head limit.
const MAX_HEAD: usize = 4096;
/// Simultaneous downloads.
const MAX_CONNECTIONS: usize = 4;
/// Longest a live response stays open (a little over the longest reply the store can hold).
const LIVE_MAX_OPEN: Duration = Duration::from_secs(180);
/// A live response with no new audio for this long is ended.
const LIVE_IDLE: Duration = Duration::from_secs(30);
/// Most bytes written to a reader in one piece.
const LIVE_PIECE: usize = 16 * 1024;
/// Longest a single write to a reader may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// A reply that is still being made. The writer appends; a reader follows.
struct Live {
    state: Mutex<LiveState>,
    changed: watch::Sender<u64>,
}

struct LiveState {
    /// The streaming WAV header, then the audio so far.
    data: Vec<u8>,
    closed: bool,
}

impl Live {
    fn touch(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LiveState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Up to `LIVE_PIECE` bytes from `pos` on, and whether the writer has finished.
    fn piece(&self, pos: usize) -> (Vec<u8>, bool) {
        let s = self.lock();
        let end = s.data.len().min(pos.saturating_add(LIVE_PIECE));
        (s.data[pos.min(end)..end].to_vec(), s.closed)
    }
}

/// The writing end of a live file. Dropping it ends the file, so a reply whose producer vanishes still ends
/// cleanly for the reader.
pub struct LiveWriter {
    live: Arc<Live>,
}

impl std::fmt::Debug for LiveWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveWriter")
    }
}

impl LiveWriter {
    /// Append PCM. Returns false (and adds nothing) when the file would pass the size limit.
    pub fn push(&self, pcm: &[u8]) -> bool {
        let mut s = self.live.lock();
        if s.closed || s.data.len() - WAV_HEADER + pcm.len() > MAX_BYTES {
            return false;
        }
        s.data.extend_from_slice(pcm);
        drop(s);
        self.live.touch();
        true
    }

    /// No more audio will follow.
    pub fn finish(&self) {
        self.live.lock().closed = true;
        self.live.touch();
    }
}

impl Drop for LiveWriter {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Clone)]
enum Body {
    Whole(Arc<Vec<u8>>),
    Live(Arc<Live>),
}

struct Entry {
    token: String,
    body: Body,
    at: Instant,
}

/// The files on offer and the URL prefix they are reachable at.
#[derive(Clone)]
pub struct MediaStore {
    base_url: Arc<str>,
    files: Arc<Mutex<Vec<Entry>>>,
    live_open: Duration,
    live_idle: Duration,
}

impl std::fmt::Debug for MediaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MediaStore({})", self.base_url)
    }
}

/// Bytes in the WAV header written by this module.
const WAV_HEADER: usize = 44;

/// The 44-byte header of a 16 kHz, 16-bit, mono PCM WAV with the given RIFF and data lengths.
fn wav_header(riff_len: u32, data_len: u32) -> Vec<u8> {
    let mut w = Vec::with_capacity(WAV_HEADER);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&riff_len.to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&16_000u32.to_le_bytes());
    w.extend_from_slice(&32_000u32.to_le_bytes()); // bytes per second
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w
}

/// A 16 kHz, 16-bit, mono WAV file around raw little-endian PCM.
#[must_use]
pub fn wav_from_pcm(pcm: &[u8]) -> Vec<u8> {
    let data = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let mut w = wav_header(data.saturating_add(36), data);
    w.reserve(pcm.len());
    w.extend_from_slice(pcm);
    w
}

impl MediaStore {
    /// A store whose files are advertised under `base_url` (for example `http://192.168.1.10:3337`).
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').into(),
            files: Arc::default(),
            live_open: LIVE_MAX_OPEN,
            live_idle: LIVE_IDLE,
        }
    }

    /// Change how long a live response may stay open and how long it may wait for new audio.
    #[must_use]
    pub fn with_live_limits(mut self, open: Duration, idle: Duration) -> Self {
        self.live_open = open;
        self.live_idle = idle;
        self
    }

    fn add(&self, body: Body) -> Option<String> {
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw).ok()?;
        let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        files.retain(|e| e.at.elapsed() < TTL);
        while files.len() >= MAX_FILES {
            files.remove(0);
        }
        let url = format!("{}/esphome/{token}.wav", self.base_url);
        files.push(Entry {
            token,
            body,
            at: Instant::now(),
        });
        Some(url)
    }

    /// Add a WAV file and return the URL it can be fetched from, or `None` if it is too large.
    #[must_use]
    pub fn put(&self, wav: Vec<u8>) -> Option<String> {
        if wav.len() > MAX_BYTES + WAV_HEADER {
            return None;
        }
        self.add(Body::Whole(Arc::new(wav)))
    }

    /// Offer a WAV file that is still being written. Returns the URL, usable at once, and the writer. The
    /// response a reader gets declares no length (the data length is the maximum a WAV header can hold) and
    /// ends when the writer finishes or is dropped.
    #[must_use]
    pub fn begin_live(&self) -> Option<(String, LiveWriter)> {
        let live = Arc::new(Live {
            state: Mutex::new(LiveState {
                data: wav_header(u32::MAX, u32::MAX),
                closed: false,
            }),
            changed: watch::channel(0).0,
        });
        let url = self.add(Body::Live(Arc::clone(&live)))?;
        Some((url, LiveWriter { live }))
    }

    fn get(&self, token: &str) -> Option<Body> {
        let files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        files
            .iter()
            .find(|e| e.token == token && e.at.elapsed() < TTL)
            .map(|e| e.body.clone())
    }
}

/// The token in `GET /esphome/<token>.wav HTTP/1.1`, and whether the method was `HEAD`.
fn parse_request(head: &str) -> Option<(&str, bool)> {
    let line = head.lines().next()?;
    let mut parts = line.split(' ');
    let method = parts.next()?;
    let path = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return None;
    }
    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return None,
    };
    let token = path.strip_prefix("/esphome/")?.strip_suffix(".wav")?;
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some((token, head_only))
}

/// Read the request head (up to the blank line) within five seconds. `None` when it is too long, never ends,
/// or the reader goes away first.
async fn read_head(stream: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(512);
    let mut tmp = [0u8; 512];
    let fill = async {
        loop {
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                return true;
            }
            if buf.len() >= MAX_HEAD {
                return false;
            }
            match stream.read(&mut tmp).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
    };
    let complete = tokio::time::timeout(Duration::from_secs(5), fill)
        .await
        .unwrap_or(false);
    complete.then_some(buf)
}

async fn handle(mut stream: tokio::net::TcpStream, store: MediaStore) {
    let head = read_head(&mut stream).await;
    let parsed = head.as_deref().and_then(|b| {
        std::str::from_utf8(b)
            .ok()
            .and_then(parse_request)
            .map(|(t, h)| (t.to_owned(), h))
    });
    let found = parsed.as_ref().and_then(|(t, _)| store.get(t));
    if let Some(Body::Live(live)) = &found {
        let head_only = parsed.as_ref().is_some_and(|(_, h)| *h);
        let http11 = head
            .as_deref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|h| h.lines().next())
            .is_some_and(|l| l.ends_with(" HTTP/1.1"));
        stream_live(stream, live, &store, http11, head_only).await;
        return;
    }
    let whole = match found {
        Some(Body::Whole(w)) => Some(w),
        _ => None,
    };
    let reply: Vec<u8> = match whole {
        Some(wav) => {
            let head_only = parsed.as_ref().is_some_and(|(_, h)| *h);
            let mut r = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                wav.len()
            )
            .into_bytes();
            if !head_only {
                r.extend_from_slice(&wav);
            }
            r
        }
        None if parsed.is_some() => {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        }
        None => {
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(30), async {
        let _ = stream.write_all(&reply).await;
        let _ = stream.shutdown().await;
    })
    .await;
}

/// Write `data` to a reader, as one HTTP chunk when the response is chunked. False when the reader is gone or
/// too slow.
async fn write_piece(wr: &mut tokio::net::tcp::OwnedWriteHalf, data: &[u8], chunked: bool) -> bool {
    let mut out = Vec::with_capacity(data.len() + 16);
    if chunked {
        out.extend_from_slice(format!("{:x}\r\n", data.len()).as_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    } else {
        out.extend_from_slice(data);
    }
    tokio::time::timeout(WRITE_TIMEOUT, wr.write_all(&out))
        .await
        .is_ok_and(|r| r.is_ok())
}

/// Answer a request for a live file: the head, then the audio as it is written, until the writer finishes, the
/// response has been open too long, nothing new has come for too long, or the reader goes away (which frees the
/// connection slot at once, even while no audio is arriving).
async fn stream_live(
    stream: tokio::net::TcpStream,
    live: &Arc<Live>,
    store: &MediaStore,
    http11: bool,
    head_only: bool,
) {
    let (mut rd, mut wr) = stream.into_split();
    let head: &[u8] = if http11 {
        b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nTransfer-Encoding: chunked\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    } else {
        b"HTTP/1.0 200 OK\r\nContent-Type: audio/wav\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    };
    if !write_piece(&mut wr, head, false).await {
        return;
    }
    if !head_only {
        if !pump_live(&mut rd, &mut wr, live, store, http11).await {
            return;
        }
        if http11 {
            // An empty chunk is the terminator: `0 CRLF CRLF`. The reader can tell a finished reply from a cut one.
            write_piece(&mut wr, b"", true).await;
        }
    }
    let _ = tokio::time::timeout(WRITE_TIMEOUT, wr.shutdown()).await;
}

/// Write the audio of a live file as it arrives. True when the writer finished or a time limit ran out (the
/// response is then ended properly); false when the reader went away or a write failed.
async fn pump_live(
    rd: &mut tokio::net::tcp::OwnedReadHalf,
    wr: &mut tokio::net::tcp::OwnedWriteHalf,
    live: &Arc<Live>,
    store: &MediaStore,
    http11: bool,
) -> bool {
    let started = Instant::now();
    let mut last = started;
    let mut pos = 0;
    let mut changes = live.changed.subscribe();
    let mut sink = [0u8; 64];
    loop {
        if started.elapsed() >= store.live_open {
            break;
        }
        changes.borrow_and_update();
        let (piece, closed) = live.piece(pos);
        if !piece.is_empty() {
            if !write_piece(wr, &piece, http11).await {
                return false;
            }
            pos += piece.len();
            last = Instant::now();
            continue;
        }
        if closed {
            break;
        }
        let until = (started + store.live_open).min(last + store.live_idle);
        tokio::select! {
            r = changes.changed() => {
                if r.is_err() {
                    break;
                }
            }
            r = rd.read(&mut sink) => {
                // Anything the reader sends now is ignored; its closing the connection ends the response.
                if matches!(r, Ok(0) | Err(_)) {
                    return false;
                }
            }
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(until)) => break,
        }
    }
    true
}

/// Serve files from `store` on `listener` until `shutdown` completes.
pub async fn serve(
    listener: TcpListener,
    store: MediaStore,
    shutdown: impl std::future::Future<Output = ()> + Send,
) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    tokio::pin!(shutdown);
    loop {
        let (stream, _) = tokio::select! {
            r = listener.accept() => match r {
                Ok(x) => x,
                Err(_) => {
                    // Out of file descriptors, for example: do not spin while it lasts.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = &mut shutdown => return,
        };
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let store = store.clone();
        tokio::spawn(async move {
            let _permit = permit;
            handle(stream, store).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fetch(addr: std::net::SocketAddr, request: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    // REQ-ESP-010
    #[test]
    fn only_a_get_or_head_for_a_32_digit_token_is_understood() {
        let token = "0123456789abcdef0123456789abcdef";
        let ok = format!("GET /esphome/{token}.wav HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(parse_request(&ok), Some((token, false)));
        let head = format!("HEAD /esphome/{token}.wav HTTP/1.0\r\n\r\n");
        assert_eq!(parse_request(&head), Some((token, true)));
        for bad in [
            String::new(),
            "GET".into(),
            "GET /".into(),
            "GET / HTTP/1.1".into(),
            format!("POST /esphome/{token}.wav HTTP/1.1"),
            format!("get /esphome/{token}.wav HTTP/1.1"),
            format!("GET /esphome/{token}.wav HTTP/2"),
            format!("GET /esphome/{token}.wav HTTP/1.1 extra"),
            format!("GET /esphome/{token}.wav  HTTP/1.1"),
            format!("GET /esphome/{}.wav HTTP/1.1", &token[..31]),
            format!("GET /esphome/{token}0.wav HTTP/1.1"),
            format!("GET /esphome/../{token}.wav HTTP/1.1"),
            format!("GET /esphome/{token}.wav?x=1 HTTP/1.1"),
            format!("GET /esphome/{}.wav HTTP/1.1", "g".repeat(32)),
            format!("GET /esphome/{}.wav HTTP/1.1", "é".repeat(16)),
            format!("GET /{} HTTP/1.1", "a".repeat(10_000)),
        ] {
            assert_eq!(parse_request(&bad), None, "{bad:?}");
        }
    }

    // REQ-ESP-010
    #[tokio::test]
    async fn req_esp_010_serves_only_known_tokens() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let store = MediaStore::new(&format!("http://{addr}/"));
        tokio::spawn(serve(l, store.clone(), std::future::pending()));
        let url = store.put(wav_from_pcm(&[1, 2, 3, 4])).unwrap();
        let path = url.strip_prefix(&format!("http://{addr}")).unwrap();

        let ok = fetch(addr, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n")).await;
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("Content-Length: 48"), "{ok}");
        let head = fetch(addr, &format!("HEAD {path} HTTP/1.1\r\n\r\n")).await;
        assert!(
            head.starts_with("HTTP/1.1 200") && head.ends_with("\r\n\r\n"),
            "{head}"
        );

        let missing = format!("/esphome/{}.wav", "0".repeat(32));
        for (req, status) in [
            (format!("GET {missing} HTTP/1.1\r\n\r\n"), "404"),
            ("GET /etc/passwd HTTP/1.1\r\n\r\n".to_owned(), "400"),
            (format!("POST {path} HTTP/1.1\r\n\r\n"), "400"),
            ("garbage\r\n\r\n".to_owned(), "400"),
            ("GET /esphome/../x.wav HTTP/1.1\r\n\r\n".to_owned(), "400"),
        ] {
            let r = fetch(addr, &req).await;
            assert!(r.contains(status), "{req:?} -> {r}");
        }
    }

    // REQ-ESP-010
    #[test]
    fn req_esp_010_keeps_few_small_files() {
        let store = MediaStore::new("http://h");
        assert!(store.put(vec![0; MAX_BYTES + 45]).is_none());
        let urls: Vec<_> = (0..6).map(|_| store.put(vec![0; 50]).unwrap()).collect();
        let token = |u: &str| {
            u.rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".wav")
                .to_owned()
        };
        assert!(store.get(&token(&urls[0])).is_none());
        assert!(store.get(&token(&urls[5])).is_some());
        assert_eq!(store.files.lock().unwrap().len(), MAX_FILES);
    }

    async fn live_store(open_ms: u64, idle_ms: u64) -> (std::net::SocketAddr, MediaStore) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let store = MediaStore::new(&format!("http://{addr}")).with_live_limits(
            Duration::from_millis(open_ms),
            Duration::from_millis(idle_ms),
        );
        tokio::spawn(serve(l, store.clone(), std::future::pending()));
        (addr, store)
    }

    fn path_of(url: &str) -> &str {
        &url[url.find("/esphome/").unwrap()..]
    }

    // REQ-ESP-010: a live file ends, with a proper terminator, when the writer finishes.
    #[tokio::test]
    async fn a_live_response_is_chunked_and_ends_when_the_writer_finishes() {
        let (addr, store) = live_store(60_000, 60_000).await;
        let (url, w) = store.begin_live().unwrap();
        assert!(w.push(&[1, 2, 3, 4]));
        let get = format!("GET {} HTTP/1.1\r\nHost: x\r\n\r\n", path_of(&url));
        let reader = tokio::spawn(async move { fetch(addr, &get).await });
        assert!(w.push(&[5, 6]));
        w.finish();
        assert!(!w.push(&[7]), "nothing is added after the end");
        let r = reader.await.unwrap();
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(r.contains("Transfer-Encoding: chunked"), "{r}");
        assert!(!r.contains("Content-Length"), "{r}");
        assert!(r.ends_with("\r\n0\r\n\r\n"), "{r:?}");
    }

    // REQ-ESP-010: dropping the writer (the producer vanished) ends the response the same way.
    #[tokio::test]
    async fn a_live_response_ends_when_the_writer_is_dropped() {
        let (addr, store) = live_store(60_000, 60_000).await;
        let (url, w) = store.begin_live().unwrap();
        let get = format!("GET {} HTTP/1.1\r\n\r\n", path_of(&url));
        let reader = tokio::spawn(async move { fetch(addr, &get).await });
        drop(w);
        assert!(reader.await.unwrap().ends_with("\r\n0\r\n\r\n"));
    }

    // REQ-ESP-010: a response is not held open for ever: by a writer that goes quiet, or by one that keeps going.
    #[tokio::test]
    async fn a_live_response_is_closed_after_the_idle_and_open_limits() {
        for (open_ms, idle_ms) in [(60_000, 150), (150, 60_000)] {
            let (addr, store) = live_store(open_ms, idle_ms).await;
            let (url, w) = store.begin_live().unwrap();
            assert!(w.push(&[0; 8]));
            let r = fetch(addr, &format!("GET {} HTTP/1.1\r\n\r\n", path_of(&url))).await;
            assert!(r.ends_with("\r\n0\r\n\r\n"), "{open_ms}/{idle_ms}: {r:?}");
            drop(w);
        }
    }

    // REQ-ESP-010: HTTP/1.0 gets no chunking; HEAD gets the head only.
    #[tokio::test]
    async fn a_live_response_for_http_1_0_is_closed_delimited_and_head_has_no_body() {
        let (addr, store) = live_store(60_000, 60_000).await;
        let (url, w) = store.begin_live().unwrap();
        assert!(w.push(&[9; 6]));
        w.finish();
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let get = format!("GET {} HTTP/1.0\r\n\r\n", path_of(&url));
        s.write_all(get.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.unwrap();
        let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let old = String::from_utf8_lossy(&raw[..at]).into_owned();
        assert!(old.starts_with("HTTP/1.0 200"), "{old}");
        assert!(!old.contains("chunked"), "{old}");
        assert_eq!(raw.len() - at - 4, 44 + 6, "header and audio, no framing");
        let head = fetch(addr, &format!("HEAD {} HTTP/1.1\r\n\r\n", path_of(&url))).await;
        assert!(
            head.starts_with("HTTP/1.1 200") && head.ends_with("\r\n\r\n"),
            "{head}"
        );
        assert!(!head.contains("\r\n0\r\n"), "{head}");
    }

    #[test]
    fn a_live_file_is_bounded_and_streams_a_header_with_no_length() {
        let store = MediaStore::new("http://h");
        let (_, w) = store.begin_live().unwrap();
        assert!(w.push(&vec![0; MAX_BYTES - 2]));
        assert!(w.push(&[0, 0]));
        assert!(!w.push(&[0]), "past the limit nothing is added");
        let h = wav_header(u32::MAX, u32::MAX);
        assert_eq!(h.len(), WAV_HEADER);
        assert_eq!(&h[..4], b"RIFF");
        assert_eq!(&h[4..8], &[0xff; 4]);
        assert_eq!(&h[40..], &[0xff; 4]);
    }

    #[test]
    fn wav_header_is_well_formed() {
        let w = wav_from_pcm(&[0; 100]);
        assert_eq!(w.len(), 144);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([w[4], w[5], w[6], w[7]]), 136);
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 100);
    }
}
