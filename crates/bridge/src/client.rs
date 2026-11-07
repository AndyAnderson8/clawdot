//! A minimal HTTP/1.1 client over `std::net` for talking to the device.
//!
//! The device speaks plain HTTP on the local network (use a VPN or a TLS-terminating proxy for anything
//! wider; see the threat model). That keeps the bridge dependency-free and trivially auditable. It
//! supports exactly what the bridge needs: a streaming GET with chunked decoding, and a POST.

use crate::sse::SseParser;
use clawdot_protocol::bridge::Reply;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A parsed `http://host:port[/base]` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// Host name or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Path prefix (no trailing slash), usually empty.
    pub base: String,
}

impl Url {
    /// Parse a device URL.
    ///
    /// # Errors
    /// A message for `https://` (unsupported here) or a malformed URL.
    pub fn parse(s: &str) -> Result<Self, String> {
        if s.starts_with("https://") {
            return Err("https:// is not supported by the bridge; use http:// on a trusted network (for example over Tailscale), or a TLS-terminating proxy".into());
        }
        let rest = s
            .strip_prefix("http://")
            .ok_or("the URL must start with http://")?;
        let (hostport, base) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| format!("bad port `{p}`"))?),
            None => (hostport, 80),
        };
        if host.is_empty() {
            return Err("the URL has no host".into());
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            base: format!("/{}", base.trim_matches('/'))
                .trim_end_matches('/')
                .to_owned(),
        })
    }
}

/// Decodes an HTTP/1.1 chunked body.
#[derive(Debug)]
pub struct ChunkedReader<R: BufRead> {
    inner: R,
    remaining: usize,
    done: bool,
}

impl<R: BufRead> ChunkedReader<R> {
    /// Wrap a reader positioned at the first chunk header.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            remaining: 0,
            done: false,
        }
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let mut line = String::new();
        if self.inner.read_line(&mut line)? == 0 {
            self.done = true;
            return Ok(());
        }
        let size_text = line.trim().split(';').next().unwrap_or("");
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
        if size == 0 {
            self.done = true;
        }
        self.remaining = size;
        Ok(())
    }
}

impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            self.next_chunk()?;
            if self.done {
                return Ok(0);
            }
        }
        let take = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..take])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.remaining -= n;
        if self.remaining == 0 {
            let mut crlf = [0_u8; 2];
            self.inner.read_exact(&mut crlf)?;
        }
        Ok(n)
    }
}

fn connect(url: &Url, timeout: Duration) -> io::Result<TcpStream> {
    let addr = (url.host.as_str(), url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host did not resolve"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(stream)
}

fn send_request(
    stream: &mut TcpStream,
    url: &Url,
    method: &str,
    path: &str,
    token: &str,
    extra: &str,
) -> io::Result<()> {
    let req = format!(
        "{method} {base}{path} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {token}\r\nUser-Agent: clawdot-bridge\r\nContent-Length: 0\r\n{extra}\r\n",
        base = url.base,
        host = url.host,
        port = url.port,
    );
    stream.write_all(req.as_bytes())
}

/// Read the status line and headers; returns the status and whether the body is chunked.
fn read_head<R: BufRead>(r: &mut R) -> io::Result<(u16, bool)> {
    let mut status_line = String::new();
    r.read_line(&mut status_line)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad HTTP status line"))?;
    let mut chunked = false;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    Ok((status, chunked))
}

/// What the stream handler wants next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep reading.
    Continue,
    /// Close the stream and return.
    Stop,
}

/// Open the reply stream and call `on_reply` for each reply until it asks to stop, the stream ends, or
/// an error occurs. `read_timeout` must exceed the device's heartbeat interval (15 s by default).
///
/// # Errors
/// I/O errors, a non-200 status (as `PermissionDenied` for 401, `Other` otherwise), or a read timeout.
pub fn stream_replies(
    url: &Url,
    endpoint: &str,
    token: &str,
    read_timeout: Duration,
    on_reply: &mut dyn FnMut(Reply) -> Flow,
) -> io::Result<()> {
    let mut stream = connect(url, read_timeout)?;
    let path = format!("/bridge/{endpoint}/replies");
    send_request(
        &mut stream,
        url,
        "GET",
        &path,
        token,
        "Accept: text/event-stream\r\nCache-Control: no-cache\r\n",
    )?;
    let mut reader = BufReader::new(stream);
    let (status, chunked) = read_head(&mut reader)?;
    match status {
        200 => {}
        401 => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the device rejected the token (401)",
            ));
        }
        other => return Err(io::Error::other(format!("unexpected HTTP status {other}"))),
    }
    let body: Box<dyn Read> = if chunked {
        Box::new(ChunkedReader::new(reader))
    } else {
        Box::new(reader)
    };
    let mut lines = BufReader::new(body);
    let mut parser = SseParser::default();
    let mut line = String::new();
    loop {
        line.clear();
        if lines.read_line(&mut line)? == 0 {
            return Ok(()); // server closed the stream
        }
        let Some(event) = parser.feed_line(line.trim_end_matches('\n')) else {
            continue;
        };
        if event.event != "reply" {
            continue;
        }
        match serde_json::from_str::<Reply>(&event.data) {
            Ok(reply) => {
                if on_reply(reply) == Flow::Stop {
                    return Ok(());
                }
            }
            Err(e) => eprintln!("clawdot-bridge: ignoring a reply it could not parse: {e}"),
        }
    }
}

/// Acknowledge a reply.
///
/// # Errors
/// I/O errors; the HTTP status is returned so the caller can tell `204` from `404`.
pub fn ack(url: &Url, endpoint: &str, token: &str, reply_id: u64) -> io::Result<u16> {
    let mut stream = connect(url, Duration::from_secs(10))?;
    let path = format!("/bridge/{endpoint}/replies/{reply_id}/ack");
    send_request(
        &mut stream,
        url,
        "POST",
        &path,
        token,
        "Connection: close\r\n",
    )?;
    let mut reader = BufReader::new(stream);
    read_head(&mut reader).map(|(status, _)| status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn urls_parse() {
        assert_eq!(
            Url::parse("http://clawdot.local:8080").unwrap(),
            Url {
                host: "clawdot.local".into(),
                port: 8080,
                base: String::new()
            }
        );
        assert_eq!(Url::parse("http://10.0.0.5").unwrap().port, 80);
        assert_eq!(Url::parse("http://h:1/api/").unwrap().base, "/api");
        assert!(
            Url::parse("https://h")
                .unwrap_err()
                .contains("not supported")
        );
        assert!(Url::parse("ftp://h").is_err());
        assert!(Url::parse("http://h:notaport").is_err());
        assert!(Url::parse("http://:80").is_err());
    }

    #[test]
    fn chunked_bodies_decode_across_chunk_boundaries() {
        let wire = b"5\r\nhello\r\n7\r\n, world\r\n0\r\n\r\n";
        let mut out = String::new();
        ChunkedReader::new(Cursor::new(&wire[..]))
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out, "hello, world");
    }

    #[test]
    fn chunked_reading_works_with_tiny_reads_and_chunk_extensions() {
        let wire = b"3;ext=1\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n";
        let mut r = ChunkedReader::new(Cursor::new(&wire[..]));
        let mut collected = Vec::new();
        let mut one = [0_u8; 1];
        while r.read(&mut one).unwrap() == 1 {
            collected.push(one[0]);
        }
        assert_eq!(collected, b"abcdef");
    }

    #[test]
    fn malformed_chunk_sizes_are_errors_not_panics() {
        let mut out = String::new();
        let err = ChunkedReader::new(Cursor::new(&b"zz\r\nabc\r\n"[..]))
            .read_to_string(&mut out)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn truncated_chunks_are_errors() {
        let mut out = String::new();
        assert!(
            ChunkedReader::new(Cursor::new(&b"a\r\nshort"[..]))
                .read_to_string(&mut out)
                .is_err()
        );
    }

    #[test]
    fn response_heads_parse() {
        let mut r = Cursor::new(&b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\nbody"[..]);
        assert_eq!(read_head(&mut r).unwrap(), (200, true));
        let mut r2 = Cursor::new(&b"HTTP/1.1 401 Unauthorized\r\n\r\n"[..]);
        assert_eq!(read_head(&mut r2).unwrap(), (401, false));
        assert!(read_head(&mut Cursor::new(&b"garbage\r\n\r\n"[..])).is_err());
    }
}
