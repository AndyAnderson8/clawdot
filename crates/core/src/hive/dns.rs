//! The small slice of DNS that mDNS/DNS-SD (RFC 6762, RFC 6763) needs: PTR, SRV, TXT and A records.
//!
//! Everything read from the network is untrusted: lengths are checked, name compression is followed at most
//! [`MAX_JUMPS`] times, and counts are capped, so a hostile packet can fail to parse but cannot loop, allocate
//! without bound, or panic.

use std::net::Ipv4Addr;

/// DNS-SD service type for ClawDot speakers.
pub const SERVICE: &str = "_clawdot._tcp.local";

/// Record types used.
pub const T_A: u16 = 1;
/// PTR record type.
pub const T_PTR: u16 = 12;
/// TXT record type.
pub const T_TXT: u16 = 16;
/// SRV record type.
pub const T_SRV: u16 = 33;

/// The class IN, with the mDNS "cache flush" bit stripped.
const CLASS_IN: u16 = 1;
const CACHE_FLUSH: u16 = 0x8000;

const MAX_JUMPS: usize = 8;
const MAX_NAME: usize = 255;
/// Records read per message section: a ClawDot message has a handful; anything more is not for us.
const MAX_RECORDS: usize = 32;

/// Decoded record data. Types this code does not use are skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rdata {
    /// A pointer to an instance name.
    Ptr(String),
    /// Port and target host.
    Srv {
        /// TCP port.
        port: u16,
        /// Host name.
        target: String,
    },
    /// Key and value strings. A string without `=` has an empty value.
    Txt(Vec<(String, String)>),
    /// An IPv4 address.
    A(Ipv4Addr),
}

/// One resource record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Owner name, lower-case, without a trailing dot.
    pub name: String,
    /// Seconds to live; 0 is a goodbye.
    pub ttl: u32,
    /// The data.
    pub data: Rdata,
}

/// A decoded message: the questions (name, type) and the answer-section records (answers, authority and
/// additional together).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// True for a response (QR bit).
    pub response: bool,
    /// Questions as (lower-case name, type).
    pub questions: Vec<(String, u16)>,
    /// All records.
    pub records: Vec<Record>,
}

fn put_name(out: &mut Vec<u8>, name: &str) -> Option<()> {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        let b = label.as_bytes();
        if b.len() > 63 {
            return None;
        }
        out.push(u8::try_from(b.len()).ok()?);
        out.extend_from_slice(b);
    }
    out.push(0);
    Some(())
}

fn put_record(
    out: &mut Vec<u8>,
    name: &str,
    rtype: u16,
    flush: bool,
    ttl: u32,
    rdata: &[u8],
) -> Option<()> {
    put_name(out, name)?;
    out.extend_from_slice(&rtype.to_be_bytes());
    let class = if flush {
        CLASS_IN | CACHE_FLUSH
    } else {
        CLASS_IN
    };
    out.extend_from_slice(&class.to_be_bytes());
    out.extend_from_slice(&ttl.to_be_bytes());
    out.extend_from_slice(&u16::try_from(rdata.len()).ok()?.to_be_bytes());
    out.extend_from_slice(rdata);
    Some(())
}

/// Encode a TXT record's strings. Each `key=value` must fit in 255 bytes.
fn txt_rdata(txt: &[(String, String)]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for (k, v) in txt {
        let s = format!("{k}={v}");
        out.push(u8::try_from(s.len()).ok()?);
        out.extend_from_slice(s.as_bytes());
    }
    if out.is_empty() {
        out.push(0);
    }
    Some(out)
}

/// What a ClawDot instance publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advert {
    /// Instance label, e.g. `clawdot-0123456789ab`.
    pub instance: String,
    /// Host label (no `.local`).
    pub host: String,
    /// TCP port (the portal).
    pub port: u16,
    /// TXT key/value pairs.
    pub txt: Vec<(String, String)>,
    /// Address for the A record, when known.
    pub addr: Option<Ipv4Addr>,
}

impl Advert {
    /// The full instance name.
    #[must_use]
    pub fn instance_fqdn(&self) -> String {
        format!("{}.{SERVICE}", self.instance)
    }

    /// An unsolicited or query-answering response carrying PTR, SRV, TXT and (if known) A. `ttl` 0 is a goodbye.
    #[must_use]
    pub fn response(&self, ttl: u32) -> Option<Vec<u8>> {
        let inst = self.instance_fqdn();
        let host = format!("{}.local", self.host);
        let mut body = Vec::new();
        let mut n: u16 = 0;
        let mut ptr = Vec::new();
        put_name(&mut ptr, &inst)?;
        put_record(&mut body, SERVICE, T_PTR, false, ttl, &ptr)?;
        let mut srv = Vec::new();
        srv.extend_from_slice(&[0, 0, 0, 0]);
        srv.extend_from_slice(&self.port.to_be_bytes());
        put_name(&mut srv, &host)?;
        put_record(&mut body, &inst, T_SRV, true, ttl, &srv)?;
        put_record(&mut body, &inst, T_TXT, true, ttl, &txt_rdata(&self.txt)?)?;
        n += 3;
        if let Some(a) = self.addr {
            put_record(&mut body, &host, T_A, true, ttl, &a.octets())?;
            n += 1;
        }
        let mut out = Vec::with_capacity(12 + body.len());
        out.extend_from_slice(&[0, 0, 0x84, 0]); // id 0, response, authoritative
        out.extend_from_slice(&[0, 0]); // no questions
        out.extend_from_slice(&n.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&body);
        Some(out)
    }
}

/// A query for the PTR records of [`SERVICE`].
#[must_use]
pub fn browse_query() -> Vec<u8> {
    let mut out = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    let _ = put_name(&mut out, SERVICE);
    out.extend_from_slice(&T_PTR.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from(self.u16()?) << 16 | u32::from(self.u16()?))
    }
}

/// Read a possibly compressed name starting at `*pos`; leaves `*pos` after the name as it appears in place.
fn read_name(buf: &[u8], pos: &mut usize) -> Option<String> {
    let mut out = String::new();
    let mut p = *pos;
    let mut jumps = 0;
    let mut end: Option<usize> = None;
    loop {
        let len = usize::from(*buf.get(p)?);
        if len == 0 {
            p += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            let lo = usize::from(*buf.get(p + 1)?);
            end.get_or_insert(p + 2);
            jumps += 1;
            if jumps > MAX_JUMPS {
                return None;
            }
            p = ((len & 0x3F) << 8) | lo;
            continue;
        }
        if len > 63 {
            return None;
        }
        let label = buf.get(p + 1..p + 1 + len)?;
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(&String::from_utf8_lossy(label).to_ascii_lowercase());
        if out.len() > MAX_NAME {
            return None;
        }
        p += 1 + len;
    }
    *pos = end.unwrap_or(p);
    Some(out)
}

/// Parse a datagram. `None` means malformed; unknown record types are skipped, not errors.
#[must_use]
pub fn parse(buf: &[u8]) -> Option<Message> {
    let mut r = Reader { buf, pos: 0 };
    r.u16()?;
    let flags = r.u16()?;
    let (qd, an, ns, ar) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
    let total = usize::from(an) + usize::from(ns) + usize::from(ar);
    if usize::from(qd) > MAX_RECORDS || total > MAX_RECORDS {
        return None;
    }
    let mut m = Message {
        response: flags & 0x8000 != 0,
        ..Message::default()
    };
    for _ in 0..qd {
        let name = read_name(buf, &mut r.pos)?;
        let t = r.u16()?;
        r.u16()?;
        m.questions.push((name, t));
    }
    for _ in 0..total {
        let name = read_name(buf, &mut r.pos)?;
        let t = r.u16()?;
        r.u16()?;
        let ttl = r.u32()?;
        let len = usize::from(r.u16()?);
        let start = r.pos;
        let end = start.checked_add(len)?;
        buf.get(start..end)?;
        let data = match t {
            T_PTR => {
                let mut p = start;
                Some(Rdata::Ptr(read_name(buf, &mut p)?))
            }
            T_SRV if len >= 7 => {
                let port = u16::from_be_bytes([buf[start + 4], buf[start + 5]]);
                let mut p = start + 6;
                Some(Rdata::Srv {
                    port,
                    target: read_name(buf, &mut p)?,
                })
            }
            T_TXT => {
                let mut txt = Vec::new();
                let mut p = start;
                while p < end {
                    let l = usize::from(buf[p]);
                    let s = buf.get(p + 1..p + 1 + l).filter(|_| p + 1 + l <= end)?;
                    p += 1 + l;
                    let s = String::from_utf8_lossy(s);
                    if s.is_empty() {
                        continue;
                    }
                    let (k, v) = s.split_once('=').unwrap_or((&s, ""));
                    if txt.len() < 16 {
                        txt.push((k.to_ascii_lowercase(), v.to_owned()));
                    }
                }
                Some(Rdata::Txt(txt))
            }
            T_A if len == 4 => Some(Rdata::A(Ipv4Addr::new(
                buf[start],
                buf[start + 1],
                buf[start + 2],
                buf[start + 3],
            ))),
            _ => None,
        };
        r.pos = end;
        if let Some(data) = data {
            m.records.push(Record { name, ttl, data });
        }
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert() -> Advert {
        Advert {
            instance: "clawdot-0123456789ab".into(),
            host: "clawdot-0123456789ab".into(),
            port: 3335,
            txt: vec![("v".into(), "1".into()), ("id".into(), "ab".repeat(32))],
            addr: Some(Ipv4Addr::new(127, 0, 0, 1)),
        }
    }

    #[test]
    fn a_response_round_trips() {
        let a = advert();
        let m = parse(&a.response(4500).unwrap()).unwrap();
        assert!(m.response);
        assert_eq!(m.records.len(), 4);
        assert_eq!(
            m.records[0],
            Record {
                name: SERVICE.into(),
                ttl: 4500,
                data: Rdata::Ptr(a.instance_fqdn())
            }
        );
        assert!(m.records.iter().any(|r| r.data
            == Rdata::Srv {
                port: 3335,
                target: "clawdot-0123456789ab.local".into()
            }));
        assert!(
            m.records
                .iter()
                .any(|r| r.data == Rdata::Txt(a.txt.clone()))
        );
        assert!(
            m.records
                .iter()
                .any(|r| r.data == Rdata::A(Ipv4Addr::LOCALHOST))
        );
    }

    #[test]
    fn a_goodbye_has_ttl_zero_and_a_query_asks_for_the_service() {
        let m = parse(&advert().response(0).unwrap()).unwrap();
        assert!(m.records.iter().all(|r| r.ttl == 0));
        let q = parse(&browse_query()).unwrap();
        assert!(!q.response);
        assert_eq!(q.questions, vec![(SERVICE.to_owned(), T_PTR)]);
    }

    /// A query whose one question name is a chain of `hops` compression pointers ending at the root.
    fn pointer_chain(hops: usize) -> Vec<u8> {
        let mut p = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for i in 0..hops {
            let next = 12 + 2 * (i + 1);
            p.extend([0xC0, u8::try_from(next).unwrap()]);
        }
        p.push(0); // the root label the last pointer lands on
        p.extend([0, 12, 0, 1]); // type and class (after the first pointer, but inside the buffer)
        p
    }

    #[test]
    fn a_chain_of_compression_pointers_is_followed_only_so_far() {
        assert!(parse(&pointer_chain(MAX_JUMPS)).is_some());
        assert!(parse(&pointer_chain(MAX_JUMPS + 1)).is_none());
        assert!(parse(&pointer_chain(100)).is_none());
    }

    #[test]
    fn labels_and_names_that_are_too_long_or_cut_short_are_refused() {
        // A label that claims 63 bytes but ends the packet.
        let mut p = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 63];
        p.extend([b'a'; 10]);
        assert!(parse(&p).is_none());
        // A length byte of 64..=191 is neither a label nor a pointer.
        let q = [0u8, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 100, 0, 0, 12, 0, 1];
        assert!(parse(&q).is_none());
        // A name that adds up to more than 255 characters.
        let mut long = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for _ in 0..6 {
            long.push(60);
            long.extend([b'x'; 60]);
        }
        long.extend([0, 0, 12, 0, 1]);
        assert!(parse(&long).is_none());
    }

    #[test]
    fn a_record_that_claims_more_data_than_the_packet_holds_is_refused() {
        // One answer: root name, type TXT, class IN, ttl 0, rdlength 0xFFFF, no data.
        let p = [
            0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 16, 0, 1, 0, 0, 0, 0, 0xFF, 0xFF,
        ];
        assert!(parse(&p).is_none());
        // A TXT string that runs past the end of its record.
        let q = [
            0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 16, 0, 1, 0, 0, 0, 0, 0, 3, 9, b'a', b'b',
        ];
        assert!(parse(&q).is_none());
        // An SRV record too short to hold a port and a name is skipped, not an error.
        let r = [
            0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 33, 0, 1, 0, 0, 0, 0, 0, 2, 0, 0,
        ];
        assert_eq!(parse(&r).map(|m| m.records.len()), Some(0));
    }

    #[test]
    fn hostile_packets_fail_cleanly() {
        // A compression pointer that points at itself.
        let mut looped = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0, 12, 0, 12, 0, 1];
        assert!(parse(&looped).is_none());
        // Every truncation of a valid message is rejected or parsed without panicking.
        let good = advert().response(10).unwrap();
        for n in 0..good.len() {
            let _ = parse(&good[..n]);
        }
        // Absurd counts.
        looped[4..6].copy_from_slice(&0xFFFFu16.to_be_bytes());
        assert!(parse(&looped).is_none());
        assert!(parse(&[]).is_none());
    }
}
