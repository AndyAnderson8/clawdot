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
        let parse_port = |p: &str| p.parse::<u16>().map_err(|_| format!("bad port `{p}`"));
        let (host, port) = if let Some(inner) = hostport.strip_prefix('[') {
            // A bracketed IPv6 address: `[fe80::1]` or `[fe80::1]:3335`.
            let (addr, after) = inner
                .split_once(']')
                .ok_or("the URL has an unclosed `[` in its host")?;
            if addr.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(format!("`[{addr}]` is not an IPv6 address"));
            }
            match after {
                "" => (addr, 80),
                _ => (
                    addr,
                    parse_port(
                        after
                            .strip_prefix(':')
                            .ok_or("bad text after `]` in the URL")?,
                    )?,
                ),
            }
        } else if hostport.matches(':').count() > 1 {
            return Err(
                "write an IPv6 address in square brackets, for example http://[fe80::1]:3335"
                    .into(),
            );
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) => (h, parse_port(p)?),
                None => (hostport, 80),
            }
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

impl Url {
    /// The value of the `Host` header: the host (in square brackets when it is an IPv6 address) and the port.
    #[must_use]
    pub fn host_header(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Longest line of an HTTP status line, header or chunk-size line that is accepted, in bytes.
const MAX_HEAD_LINE: usize = 8 * 1024;
/// Longest status line plus headers that is accepted, in bytes.
const MAX_HEAD_BLOCK: usize = 8 * 1024;
/// Longest event-stream line, and longest event, that is accepted, in bytes. The largest reply is 4,000
/// characters of text plus JSON around it, so this is generous.
const MAX_EVENT: usize = 64 * 1024;

fn too_long(what: &str, max: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{what} is longer than {max} bytes"),
    )
}

/// Like `BufRead::read_line`, but a line longer than `max` bytes (the line ending included) is an
/// `InvalidData` error instead of memory growing without limit. Returns the bytes read; 0 at end of input.
fn read_line_bounded<R: BufRead>(r: &mut R, out: &mut String, max: usize) -> io::Result<usize> {
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let available = match r.fill_buf() {
            Ok(a) => a,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            break;
        }
        let (used, found) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if bytes.len() + used > max {
            return Err(too_long("a line from the speaker", max));
        }
        bytes.extend_from_slice(&available[..used]);
        r.consume(used);
        if found {
            break;
        }
    }
    let n = bytes.len();
    out.push_str(
        &String::from_utf8(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a line is not valid UTF-8"))?,
    );
    Ok(n)
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
        if read_line_bounded(&mut self.inner, &mut line, MAX_HEAD_LINE)? == 0 {
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
        "{method} {base}{path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nUser-Agent: clawdot-bridge\r\nContent-Length: 0\r\n{extra}\r\n",
        base = url.base,
        host = url.host_header(),
    );
    stream.write_all(req.as_bytes())
}

/// Read the status line and headers; returns the status and whether the body is chunked. A line, or the
/// whole block, over 8 KiB is an `InvalidData` error.
fn read_head<R: BufRead>(r: &mut R) -> io::Result<(u16, bool)> {
    let mut status_line = String::new();
    let mut block = read_line_bounded(r, &mut status_line, MAX_HEAD_LINE)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad HTTP status line"))?;
    let mut chunked = false;
    loop {
        let mut h = String::new();
        let n = read_line_bounded(r, &mut h, MAX_HEAD_LINE)?;
        if n == 0 || h.trim().is_empty() {
            break;
        }
        block += n;
        if block > MAX_HEAD_BLOCK {
            return Err(too_long("the response headers", MAX_HEAD_BLOCK));
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
    read_events(&mut BufReader::new(body), on_reply)
}

/// Read the event stream until the server closes it or `on_reply` asks to stop. A line, or one event, over
/// 64 KiB is an `InvalidData` error: the connection is dropped and the caller reconnects.
fn read_events<R: BufRead>(
    lines: &mut R,
    on_reply: &mut dyn FnMut(Reply) -> Flow,
) -> io::Result<()> {
    let mut parser = SseParser::default();
    let mut line = String::new();
    let mut event_bytes = 0_usize;
    loop {
        line.clear();
        let n = read_line_bounded(lines, &mut line, MAX_EVENT)?;
        if n == 0 {
            return Ok(()); // server closed the stream
        }
        event_bytes += n;
        if event_bytes > MAX_EVENT {
            return Err(too_long("one event", MAX_EVENT));
        }
        let Some(event) = parser.feed_line(line.trim_end_matches('\n')) else {
            if line.trim_end_matches(['\r', '\n']).is_empty() {
                event_bytes = 0; // a blank line ends the event even when it carried no data
            }
            continue;
        };
        event_bytes = 0;
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

    // REQ-INB-015: the bridge reaches the speaker by an IPv6 address too.
    #[test]
    fn bracketed_ipv6_hosts_parse_and_are_written_back_in_brackets() {
        let u = Url::parse("http://[fe80::1]:3335").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("fe80::1", 3335));
        assert_eq!(u.host_header(), "[fe80::1]:3335");
        let u = Url::parse("http://[::1]/api/").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.base.as_str()),
            ("::1", 80, "/api")
        );
        assert_eq!(u.host_header(), "[::1]:80");
        assert_eq!(
            Url::parse("http://clawdot.local:8080")
                .unwrap()
                .host_header(),
            "clawdot.local:8080"
        );
        for bad in [
            "http://[fe80::1",
            "http://[fe80::1]x",
            "http://[fe80::1]:notaport",
            "http://[not-an-address]:1",
            "http://[]:1",
            "http://fe80::1:3335",
        ] {
            assert!(Url::parse(bad).is_err(), "{bad}");
        }
    }

    // REQ-INB-015: a header line, the header block and an event line are bounded.
    #[test]
    fn an_over_long_header_line_or_block_is_a_protocol_error() {
        let mut long_line = b"HTTP/1.1 200 OK\r\nX-Pad: ".to_vec();
        long_line.extend(std::iter::repeat_n(b'a', MAX_HEAD_LINE));
        long_line.extend_from_slice(b"\r\n\r\n");
        let e = read_head(&mut Cursor::new(long_line)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);

        // Many short lines that together pass the limit.
        let mut block = b"HTTP/1.1 200 OK\r\n".to_vec();
        while block.len() < MAX_HEAD_BLOCK + 200 {
            block.extend_from_slice(b"X-A: 0123456789012345678901234567890123456789\r\n");
        }
        block.extend_from_slice(b"\r\n");
        let e = read_head(&mut Cursor::new(block)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);

        // A status line with no end in sight.
        let e = read_head(&mut Cursor::new(vec![b'x'; MAX_HEAD_LINE * 2])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);

        // A header block just under the limit still parses.
        let mut ok = b"HTTP/1.1 200 OK\r\n".to_vec();
        while ok.len() < MAX_HEAD_BLOCK - 100 {
            ok.extend_from_slice(b"X-A: 0123456789\r\n");
        }
        ok.extend_from_slice(b"\r\n");
        assert_eq!(read_head(&mut Cursor::new(ok)).unwrap(), (200, false));
    }

    #[test]
    fn an_over_long_chunk_size_line_is_an_error() {
        let mut wire = vec![b'a'; MAX_HEAD_LINE + 1];
        wire.extend_from_slice(b"\r\nx");
        let mut out = String::new();
        let e = ChunkedReader::new(Cursor::new(wire))
            .read_to_string(&mut out)
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    fn event(text: &str) -> String {
        format!(
            "event: reply\ndata: {{\"id\":1,\"endpoint\":\"e\",\"event_id\":1,\"session_ref\":null,\"kind\":\"instruct\",\"text\":\"{text}\",\"created_at\":0}}\n\n"
        )
    }

    #[test]
    fn an_over_long_event_line_drops_the_connection_and_normal_events_still_arrive() {
        let seen = std::cell::RefCell::new(Vec::new());
        let mut collect = |r: Reply| {
            seen.borrow_mut().push(r);
            Flow::Continue
        };
        // A 4,000 character reply is normal.
        let ok = event(&"a".repeat(4_000));
        read_events(&mut Cursor::new(ok.into_bytes()), &mut collect).unwrap();
        assert_eq!(seen.borrow().len(), 1, "a normal reply is delivered");
        // A line over the limit is a protocol error, not a delivered reply.
        let huge = format!("data: {}\n\n", "a".repeat(MAX_EVENT + 1));
        let e = read_events(&mut Cursor::new(huge.into_bytes()), &mut collect).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert_eq!(seen.borrow().len(), 1);
    }

    #[test]
    fn an_event_made_of_many_lines_is_bounded_too() {
        let mut collect = |_: Reply| Flow::Continue;
        let line = format!("data: {}\n", "a".repeat(10_000));
        let many = line.repeat(8);
        let e = read_events(&mut Cursor::new(many.into_bytes()), &mut collect).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        // Keep-alives between events do not add up.
        let keepalives = ": ping\n\n".repeat(20_000);
        read_events(&mut Cursor::new(keepalives.into_bytes()), &mut collect).unwrap();
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
