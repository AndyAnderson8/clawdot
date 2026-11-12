//! A tiny HTTP file server for the audio the device is asked to fetch (spec 0004, REQ-ESP-010).
//!
//! A device plays an announcement or a spoken reply by downloading a URL, so the core must serve the audio.
//! This is deliberately not the main web server: it binds its own address, serves only
//! `GET`/`HEAD /esphome/<token>.wav` for files it holds, and keeps at most four small files for two minutes.
//! The token is 128 random bits, so a URL cannot be guessed.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

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

struct Entry {
    token: String,
    wav: Arc<Vec<u8>>,
    at: Instant,
}

/// The files on offer and the URL prefix they are reachable at.
#[derive(Clone)]
pub struct MediaStore {
    base_url: Arc<str>,
    files: Arc<Mutex<Vec<Entry>>>,
}

impl std::fmt::Debug for MediaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MediaStore({})", self.base_url)
    }
}

/// A 16 kHz, 16-bit, mono WAV file around raw little-endian PCM.
#[must_use]
pub fn wav_from_pcm(pcm: &[u8]) -> Vec<u8> {
    let data = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let mut w = Vec::with_capacity(pcm.len() + 44);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&data.saturating_add(36).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&16_000u32.to_le_bytes());
    w.extend_from_slice(&32_000u32.to_le_bytes()); // bytes per second
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
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
        }
    }

    /// Add a WAV file and return the URL it can be fetched from, or `None` if it is too large.
    #[must_use]
    pub fn put(&self, wav: Vec<u8>) -> Option<String> {
        if wav.len() > MAX_BYTES + 44 {
            return None;
        }
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
            wav: Arc::new(wav),
            at: Instant::now(),
        });
        Some(url)
    }

    fn get(&self, token: &str) -> Option<Arc<Vec<u8>>> {
        let files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        files
            .iter()
            .find(|e| e.token == token && e.at.elapsed() < TTL)
            .map(|e| Arc::clone(&e.wav))
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

async fn handle(mut stream: tokio::net::TcpStream, store: MediaStore) {
    let mut buf = Vec::with_capacity(512);
    let mut tmp = [0u8; 512];
    let read_head = async {
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
    let complete = tokio::time::timeout(Duration::from_secs(5), read_head)
        .await
        .unwrap_or(false);
    let parsed = if complete {
        std::str::from_utf8(&buf)
            .ok()
            .and_then(parse_request)
            .map(|(t, h)| (t.to_owned(), h))
    } else {
        None
    };
    let reply: Vec<u8> = match parsed.as_ref().and_then(|(t, _)| store.get(t)) {
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

    #[test]
    fn wav_header_is_well_formed() {
        let w = wav_from_pcm(&[0; 100]);
        assert_eq!(w.len(), 144);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([w[4], w[5], w[6], w[7]]), 136);
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 100);
    }
}
