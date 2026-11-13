//! Finding other speakers: announce this one and browse for the rest over mDNS/DNS-SD, in memory only.
//!
//! This is a deliberately small subset of RFC 6762/6763. What it does: answers queries for `_clawdot._tcp`, sends
//! the announcement twice at start (one second apart) and a goodbye (TTL 0) on stop, browses with the RFC's
//! exponential back-off, honours known-answer suppression and sends at most one response a second. What it does
//! not do: probing for name conflicts (the instance name comes from the device id, so a clash means the same key),
//! random response delays, unicast responses, IPv6, or more than one network interface. Nothing it hears is
//! trusted; peers are advisory until a later milestone pins their keys.
//!
//! What is checked on what arrives:
//!
//! * A packet whose source address is not loopback, private (RFC 1918) or link-local is dropped before it is
//!   parsed, so an answer from the internet is never looked at.
//! * A peer's address is the packet's source address, never a value taken from the packet. An address record in
//!   the response is ignored, so a machine with several addresses is listed under the one it sent from.
//! * The first address to announce an id keeps it until its record runs out, and only that address can say
//!   goodbye for it.
//! * The table holds at most [`MAX_PEERS`] peers and TTLs are capped.
//!
//! What is **not** checked: which network interface a packet arrived on, whether the sender used the
//! multicast group or the unicast address, and the IP time-to-live of 255 that RFC 6762 asks a receiver to
//! require (which shows that the packet did not cross a router). All three need socket options that the workspace's ban on `unsafe` code
//! excludes, so a host with several interfaces (a wired and a wireless network, a VPN, a bridge to virtual
//! machines) accepts answers from any private network it is attached to, and a device on a guest network that
//! is routed to this one can answer too. The source-address checks above do not stop a device that has a
//! private address on the network and lies about its id or name; peers stay advisory for that reason.

use super::dns::{self, Advert, Rdata, SERVICE};
use super::identity::is_device_id;
use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

/// The mDNS group and port (RFC 6762).
pub const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
/// The mDNS port.
pub const MDNS_PORT: u16 = 5353;

/// At most this many peers are remembered; a flood of fake announcements cannot grow the table past it.
pub const MAX_PEERS: usize = 32;
/// TTL on our own records (RFC 6762: 75 minutes for non-host records).
pub const RECORD_TTL: u32 = 4500;
/// Longest TTL accepted from a peer.
const MAX_TTL: u32 = 4500;
/// Re-asking never waits longer than this.
const QUERY_CAP: Duration = Duration::from_secs(15 * 60);

/// A speaker found on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// Device id (hex SHA-256 of its public key), as claimed in its announcement. Unverified in H0.
    pub id: String,
    /// Name from the announcement, if it chose to publish one; otherwise the instance label.
    pub name: String,
    /// Where the packet came from.
    pub address: IpAddr,
    /// Its portal port.
    pub port: u16,
    /// When it last announced or answered.
    pub last_seen: Instant,
    expires: Instant,
    instance: String,
}

/// The discovered peers.
#[derive(Debug, Default)]
pub struct Peers {
    map: BTreeMap<String, Peer>,
    conflicts: u64,
}

impl Peers {
    /// How many times a packet from another address claimed (or said goodbye for) an id that is already known.
    /// Anyone on the network can send any id; the address that announced first keeps it until its record runs out.
    #[cfg(test)]
    fn conflicts(&self) -> u64 {
        self.conflicts
    }

    fn conflict(&mut self, id: &str, known: IpAddr, from: IpAddr) {
        self.conflicts += 1;
        if self.conflicts == 1 {
            // Once, so a flood of forged packets cannot flood the log.
            crate::diag::warn(&format!(
                "hive: {from} claimed the speaker id {} that {known} announced first; keeping {known}. Further claims are only counted.",
                id.get(..12).unwrap_or(id)
            ));
        }
    }

    /// Add or refresh a peer. Returns false when the table is full and `p` is new, or when a live record for the
    /// same id came from a different address (the first one is kept until it expires).
    fn upsert(&mut self, p: Peer) -> bool {
        match self.map.get(&p.id) {
            Some(known) if known.address != p.address && known.expires > p.last_seen => {
                let known = known.address;
                self.conflict(&p.id.clone(), known, p.address);
                return false;
            }
            None if self.map.len() >= MAX_PEERS => return false,
            _ => {}
        }
        self.map.insert(p.id.clone(), p);
        true
    }

    /// A goodbye for `instance` from `from`. Only the address that announced it can withdraw it.
    fn remove_instance(&mut self, instance: &str, from: IpAddr) {
        let foreign = self
            .map
            .values()
            .find(|p| p.instance == instance && p.address != from)
            .map(|p| (p.id.clone(), p.address));
        if let Some((id, known)) = foreign {
            self.conflict(&id, known, from);
            return;
        }
        self.map.retain(|_, p| p.instance != instance);
    }

    /// Drop peers whose records ran out, and return the rest, newest first.
    pub fn live(&mut self, now: Instant) -> Vec<Peer> {
        self.map.retain(|_, p| p.expires > now);
        let mut v: Vec<Peer> = self.map.values().cloned().collect();
        v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then_with(|| a.id.cmp(&b.id)));
        v
    }
}

/// Whether a packet from `from` is looked at at all: only loopback, private and link-local senders are, since
/// mDNS never legitimately crosses the internet. This is a check on the sender's address only.
fn source_is_local(from: IpAddr) -> bool {
    crate::netpolicy::is_private_ip(from)
}

/// Turn one received response into peer updates. Pure so it can be tested without a socket.
fn apply_response(
    peers: &mut Peers,
    msg: &dns::Message,
    src: IpAddr,
    own_id: Option<&str>,
    now: Instant,
) {
    for rec in &msg.records {
        if rec.name != SERVICE {
            continue;
        }
        let Rdata::Ptr(inst) = &rec.data else {
            continue;
        };
        if rec.ttl == 0 {
            peers.remove_instance(inst, src);
            continue;
        }
        let find = |want: fn(&Rdata) -> bool| {
            msg.records
                .iter()
                .find(|r| &r.name == inst && want(&r.data))
                .map(|r| &r.data)
        };
        // An advertised address record is not read: the peer's address is the packet's source (REQ-CFG-009).
        let (Some(Rdata::Srv { port, .. }), Some(Rdata::Txt(txt))) = (
            find(|d| matches!(d, Rdata::Srv { .. })),
            find(|d| matches!(d, Rdata::Txt(_))),
        ) else {
            continue;
        };
        let get = |k: &str| {
            txt.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        if get("v") != Some("1") {
            continue;
        }
        let Some(id) = get("id").filter(|i| is_device_id(i)) else {
            continue;
        };
        if own_id == Some(id) {
            continue;
        }
        let announced_port = get("p")
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(*port);
        let name = get("n")
            .map(|n| {
                n.chars()
                    .filter(|c| !c.is_control())
                    .take(40)
                    .collect::<String>()
            })
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| inst.split('.').next().unwrap_or("").to_owned());
        peers.upsert(Peer {
            id: id.to_owned(),
            name,
            address: src,
            port: announced_port,
            last_seen: now,
            expires: now + Duration::from_secs(u64::from(rec.ttl.min(MAX_TTL))),
            instance: inst.clone(),
        });
    }
}

/// What the responder should do about a received query.
fn wants_answer(msg: &dns::Message, own: &Advert) -> bool {
    if msg.response {
        return false;
    }
    let asks = msg.questions.iter().any(|(n, t)| {
        (n == SERVICE && *t == dns::T_PTR)
            || (*n == own.instance_fqdn() && matches!(*t, dns::T_SRV | dns::T_TXT))
    });
    // Known-answer suppression: the asker already has our PTR with more than half its life left.
    let known = msg.records.iter().any(|r| {
        r.name == SERVICE && r.ttl >= RECORD_TTL / 2 && r.data == Rdata::Ptr(own.instance_fqdn())
    });
    asks && !known
}

/// Settings for [`Discovery::start`].
#[derive(Debug, Clone)]
pub struct Config {
    /// What to announce, or `None` to browse only (for example while the system's Avahi announces).
    pub advert: Option<Advert>,
    /// This speaker's device id, so its own announcement is not listed as a peer.
    pub own_id: Option<String>,
    /// Where datagrams are sent: the mDNS group in production, loopback sockets in tests.
    pub dests: Vec<SocketAddr>,
    /// Whether to look for other speakers.
    pub browse: bool,
    /// Longest wait between browse queries.
    pub query_cap: Duration,
}

impl Config {
    /// Production defaults: send to the mDNS group, browse, re-ask at most every 15 minutes.
    #[must_use]
    pub fn multicast(advert: Option<Advert>, own_id: Option<String>) -> Self {
        Self {
            advert,
            own_id,
            dests: vec![SocketAddr::from((MDNS_GROUP, MDNS_PORT))],
            browse: true,
            query_cap: QUERY_CAP,
        }
    }
}

/// A running announcer/browser.
#[derive(Debug)]
pub struct Discovery {
    peers: Arc<Mutex<Peers>>,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Discovery {
    /// Start on `socket` (already bound; multicast membership, if any, already joined).
    #[must_use]
    pub fn start(socket: UdpSocket, cfg: Config) -> Self {
        let peers = Arc::new(Mutex::new(Peers::default()));
        let (tx, rx) = oneshot::channel();
        let task = tokio::spawn(run(socket, cfg, peers.clone(), rx));
        Self {
            peers,
            stop: Some(tx),
            task,
        }
    }

    /// The peers whose records are still alive.
    #[must_use]
    pub fn peers(&self) -> Vec<Peer> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .live(Instant::now())
    }

    /// Send the goodbye and stop.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        let _ = (&mut self.task).await;
    }
}

async fn send_all(sock: &UdpSocket, dests: &[SocketAddr], data: &[u8]) {
    for d in dests {
        let _ = sock.send_to(data, d).await;
    }
}

async fn run(
    sock: UdpSocket,
    cfg: Config,
    peers: Arc<Mutex<Peers>>,
    mut stop: oneshot::Receiver<()>,
) {
    let announce = cfg.advert.as_ref().and_then(|a| a.response(RECORD_TTL));
    let query = dns::browse_query();
    let start = Instant::now();
    let mut announces_left: u8 = if announce.is_some() { 2 } else { 0 };
    let mut next_announce = start;
    let mut next_query = cfg.browse.then_some(start);
    let mut gap = Duration::from_secs(1);
    let mut last_response: Option<Instant> = None;
    let mut buf = vec![0u8; 2048];
    loop {
        let now = Instant::now();
        if announces_left > 0 && now >= next_announce {
            if let Some(a) = &announce {
                send_all(&sock, &cfg.dests, a).await;
            }
            announces_left -= 1;
            next_announce = now + Duration::from_secs(1);
        }
        if next_query.is_some_and(|t| now >= t) {
            send_all(&sock, &cfg.dests, &query).await;
            next_query = Some(now + gap);
            gap = (gap * 2).min(cfg.query_cap);
        }
        let wake = [(announces_left > 0).then_some(next_announce), next_query]
            .into_iter()
            .flatten()
            .min();
        let sleep = async {
            match wake {
                Some(t) => tokio::time::sleep_until(t.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = &mut stop => break,
            () = sleep => {}
            got = sock.recv_from(&mut buf) => {
                let Ok((n, from)) = got else { continue };
                if !source_is_local(from.ip()) {
                    continue;
                }
                let Some(msg) = buf.get(..n).and_then(dns::parse) else { continue };
                let now = Instant::now();
                if msg.response {
                    if cfg.browse {
                        let mut p = peers.lock().unwrap_or_else(PoisonError::into_inner);
                        apply_response(&mut p, &msg, from.ip(), cfg.own_id.as_deref(), now);
                    }
                } else if let (Some(ad), Some(reply)) = (&cfg.advert, &announce)
                    && wants_answer(&msg, ad)
                    && last_response.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1))
                {
                    last_response = Some(now);
                    send_all(&sock, &cfg.dests, reply).await;
                }
            }
        }
    }
    if let Some(bye) = cfg.advert.as_ref().and_then(|a| a.response(0)) {
        send_all(&sock, &cfg.dests, &bye).await;
    }
}

/// Open the mDNS socket: UDP 5353 with address and port reuse (so it can sit beside another responder that also
/// shares the port), joined to 224.0.0.251 on the default interface, with multicast loop-back on so speakers on
/// the same host see each other.
///
/// # Errors
/// When the socket cannot be created, bound or joined.
pub fn bind_multicast() -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    s.set_reuse_port(true)?;
    s.set_nonblocking(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, MDNS_PORT)).into())?;
    s.join_multicast_v4(&MDNS_GROUP, &Ipv4Addr::UNSPECIFIED)?;
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(255)?;
    UdpSocket::from_std(s.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn advert(n: u8, port: u16, name: Option<&str>) -> (Advert, String) {
        let id = format!("{n:02x}").repeat(32);
        let mut txt = vec![
            ("v".to_owned(), "1".to_owned()),
            ("id".to_owned(), id.clone()),
            ("p".to_owned(), port.to_string()),
        ];
        if let Some(n) = name {
            txt.push(("n".to_owned(), n.to_owned()));
        }
        let label = format!("clawdot-{}", &id[..12]);
        (
            Advert {
                instance: label.clone(),
                host: label,
                port,
                txt,
                addr: None,
            },
            id,
        )
    }

    #[test]
    fn a_response_becomes_a_peer_and_a_goodbye_removes_it() {
        let (ad, id) = advert(1, 3335, Some("Kitchen"));
        let t0 = Instant::now();
        let mut peers = Peers::default();
        let src = IpAddr::from([192, 0, 2, 7]);
        let msg = dns::parse(&ad.response(100).unwrap()).unwrap();
        apply_response(&mut peers, &msg, src, None, t0);
        let live = peers.live(t0);
        assert_eq!(live.len(), 1);
        assert_eq!(
            (live[0].id.as_str(), live[0].name.as_str()),
            (id.as_str(), "Kitchen")
        );
        assert_eq!((live[0].address, live[0].port), (src, 3335));
        let bye = dns::parse(&ad.response(0).unwrap()).unwrap();
        apply_response(&mut peers, &bye, src, None, t0);
        assert!(peers.live(t0).is_empty());
    }

    #[test]
    fn a_known_id_claimed_from_another_address_keeps_the_first_until_it_expires() {
        let (ad, _) = advert(5, 3335, Some("Kitchen"));
        let t0 = Instant::now();
        let mut peers = Peers::default();
        let (first, other) = (IpAddr::from([192, 0, 2, 20]), IpAddr::from([192, 0, 2, 66]));
        let msg = dns::parse(&ad.response(60).unwrap()).unwrap();
        apply_response(&mut peers, &msg, first, None, t0);
        // Someone else claims the same id: the address stays, the attempt is counted.
        apply_response(&mut peers, &msg, other, None, t0 + Duration::from_secs(10));
        assert_eq!(peers.live(t0 + Duration::from_secs(10))[0].address, first);
        assert_eq!(peers.conflicts(), 1);
        // Nor can a goodbye from there remove it.
        let bye = dns::parse(&ad.response(0).unwrap()).unwrap();
        apply_response(&mut peers, &bye, other, None, t0 + Duration::from_secs(11));
        assert_eq!(peers.live(t0 + Duration::from_secs(11)).len(), 1);
        assert_eq!(peers.conflicts(), 2);
        // The real one refreshing from the same address is not a conflict.
        apply_response(&mut peers, &msg, first, None, t0 + Duration::from_secs(20));
        assert_eq!(peers.conflicts(), 2);
        // Once the record has run out the id is free to move.
        apply_response(&mut peers, &msg, other, None, t0 + Duration::from_secs(200));
        assert_eq!(peers.live(t0 + Duration::from_secs(200))[0].address, other);
        assert_eq!(peers.conflicts(), 2);
    }

    // REQ-CFG-009: only senders on this machine or the private network are listened to.
    #[test]
    fn only_loopback_private_and_link_local_senders_are_listened_to() {
        for ok in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.7",
            "169.254.3.4",
            "::1",
            "fd00::5",
            "fe80::1",
        ] {
            assert!(source_is_local(ok.parse().unwrap()), "{ok}");
        }
        for bad in [
            "8.8.8.8",
            "203.0.113.9",
            "172.32.0.1",
            "100.64.0.1",
            "2001:db8::1",
        ] {
            assert!(!source_is_local(bad.parse().unwrap()), "{bad}");
        }
    }

    // REQ-CFG-009: an address record is ignored; the peer is listed under the address the packet came from.
    #[test]
    fn a_peer_is_listed_under_the_packet_source_whatever_address_it_advertises() {
        let (mut ad, id) = advert(7, 3335, Some("Den"));
        ad.addr = Some(Ipv4Addr::new(192, 168, 1, 50));
        let msg = dns::parse(&ad.response(100).unwrap()).unwrap();
        let t0 = Instant::now();
        let mut peers = Peers::default();
        // Sent from another private address than the one it names: listed, under the source.
        apply_response(&mut peers, &msg, IpAddr::from([192, 168, 1, 66]), None, t0);
        let live = peers.live(t0);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, id);
        assert_eq!(live[0].address, IpAddr::from([192, 168, 1, 66]));
        // Sent from the address it names: the same peer, the same address.
        let mut peers = Peers::default();
        apply_response(&mut peers, &msg, IpAddr::from([192, 168, 1, 50]), None, t0);
        assert_eq!(peers.live(t0)[0].address, IpAddr::from([192, 168, 1, 50]));
        // A link-local IPv6 source is accepted by the source filter and kept as the address.
        let ll: IpAddr = "fe80::7".parse().unwrap();
        assert!(source_is_local(ll));
        let mut peers = Peers::default();
        apply_response(&mut peers, &msg, ll, None, t0);
        assert_eq!(peers.live(t0)[0].address, ll);
        // A response with no address record is accepted, with the source as the address.
        let (bare, _) = advert(8, 1, None);
        let m = dns::parse(&bare.response(100).unwrap()).unwrap();
        apply_response(&mut peers, &m, IpAddr::from([192, 168, 1, 51]), None, t0);
        assert_eq!(peers.live(t0).len(), 2);
    }

    #[test]
    fn a_stale_peer_expires_and_a_refresh_extends_it() {
        let (ad, _) = advert(2, 3335, None);
        let t0 = Instant::now();
        let mut peers = Peers::default();
        let src = IpAddr::from([192, 0, 2, 8]);
        let msg = dns::parse(&ad.response(60).unwrap()).unwrap();
        apply_response(&mut peers, &msg, src, None, t0);
        assert_eq!(peers.live(t0 + Duration::from_secs(59)).len(), 1);
        apply_response(&mut peers, &msg, src, None, t0 + Duration::from_secs(50));
        assert_eq!(peers.live(t0 + Duration::from_secs(100)).len(), 1);
        assert!(peers.live(t0 + Duration::from_secs(111)).is_empty());
        // No name published: the instance label stands in.
        apply_response(&mut peers, &msg, src, None, t0);
        assert!(peers.live(t0).first().unwrap().name.starts_with("clawdot-"));
    }

    #[test]
    fn junk_and_oversized_claims_are_ignored() {
        let t0 = Instant::now();
        let mut peers = Peers::default();
        let src = IpAddr::from([192, 0, 2, 9]);
        // Wrong version, bad id, own id.
        for (v, id) in [
            ("2", "ab".repeat(32)),
            ("1", "short".to_owned()),
            ("1", "AB".repeat(32)),
        ] {
            let (mut ad, _) = advert(3, 1, None);
            ad.txt = vec![("v".into(), v.into()), ("id".into(), id)];
            apply_response(
                &mut peers,
                &dns::parse(&ad.response(100).unwrap()).unwrap(),
                src,
                None,
                t0,
            );
        }
        let (ad, id) = advert(4, 1, None);
        let msg = dns::parse(&ad.response(100).unwrap()).unwrap();
        apply_response(&mut peers, &msg, src, Some(&id), t0);
        assert!(peers.live(t0).is_empty());
        // The table is capped.
        for n in 0..=u8::try_from(MAX_PEERS + 5).unwrap() {
            let (ad, _) = advert(n, 1, None);
            apply_response(
                &mut peers,
                &dns::parse(&ad.response(100).unwrap()).unwrap(),
                src,
                None,
                t0,
            );
        }
        assert_eq!(peers.live(t0).len(), MAX_PEERS);
    }

    #[test]
    fn txt_carries_exactly_the_published_set() {
        let (ad, id) = advert(5, 3335, None);
        let m = dns::parse(&ad.response(100).unwrap()).unwrap();
        let txt = m
            .records
            .iter()
            .find_map(|r| match &r.data {
                Rdata::Txt(t) => Some(t.clone()),
                _ => None,
            })
            .unwrap();
        let keys: Vec<&str> = txt.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["v", "id", "p"]);
        assert_eq!(txt[1].1, id);
    }

    #[test]
    fn answer_rules() {
        let (ad, _) = advert(6, 3335, None);
        let q = dns::parse(&dns::browse_query()).unwrap();
        assert!(wants_answer(&q, &ad));
        // Responses are never answered.
        assert!(!wants_answer(
            &dns::parse(&ad.response(100).unwrap()).unwrap(),
            &ad
        ));
        // A query that already lists our PTR with a fresh TTL is suppressed.
        let mut known = dns::parse(&dns::browse_query()).unwrap();
        known.records = dns::parse(&ad.response(RECORD_TTL).unwrap())
            .unwrap()
            .records;
        assert!(!wants_answer(&known, &ad));
    }

    async fn socket() -> (UdpSocket, SocketAddr) {
        let s = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let a = s.local_addr().unwrap();
        (s, a)
    }

    fn cfg(ad: Advert, id: String, to: SocketAddr) -> Config {
        Config {
            advert: Some(ad),
            own_id: Some(id),
            dests: vec![to],
            browse: true,
            query_cap: Duration::from_secs(1),
        }
    }

    async fn until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn two_announcers_on_loopback_find_each_other_and_forget_on_goodbye() {
        let (sa, addr_a) = socket().await;
        let (sb, addr_b) = socket().await;
        let (ad_a, id_a) = advert(0xa1, 3335, Some("Kitchen"));
        let (ad_b, id_b) = advert(0xb2, 4444, Some("Den"));
        // Each sends to the other's socket, standing in for the multicast group.
        let a = Discovery::start(sa, cfg(ad_a, id_a.clone(), addr_b));
        let b = Discovery::start(sb, cfg(ad_b, id_b.clone(), addr_a));
        assert!(until(|| !a.peers().is_empty() && !b.peers().is_empty()).await);
        let seen_by_a = a.peers();
        assert_eq!(seen_by_a.len(), 1, "a never lists itself");
        assert_eq!(seen_by_a[0].id, id_b);
        assert_eq!(seen_by_a[0].name, "Den");
        assert_eq!(seen_by_a[0].port, 4444);
        assert!(seen_by_a[0].address.is_loopback());
        assert_eq!(b.peers()[0].id, id_a);
        // b stops: its goodbye removes it from a at once, not after the TTL.
        b.shutdown().await;
        assert!(until(|| a.peers().is_empty()).await);
        a.shutdown().await;
    }

    #[tokio::test]
    async fn a_browse_only_node_sends_queries_and_learns_from_answers() {
        let (sa, addr_a) = socket().await;
        let (sb, addr_b) = socket().await;
        let (ad_b, id_b) = advert(0xc3, 5555, None);
        let mut ca = cfg(advert(0xa1, 1, None).0, "x".into(), addr_b);
        ca.advert = None;
        let a = Discovery::start(sa, ca);
        let mut cb = cfg(ad_b, id_b.clone(), addr_a);
        cb.browse = false;
        let b = Discovery::start(sb, cb);
        assert!(until(|| a.peers().iter().any(|p| p.id == id_b)).await);
        a.shutdown().await;
        b.shutdown().await;
    }
}
