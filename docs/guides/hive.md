# Hive, milestone H0: identity and discovery

This is the first step of the hive described in [research/14-hive.md](../research/14-hive.md). It lets a speaker
have a stable identity and see other ClawDot speakers on the same home network. It does **not** pair speakers,
share any data, or add anything to the portal. The research's open questions are still unanswered, so only what is
safe under any answer was built.

## What it does

- **Identity.** On first start the speaker makes an Ed25519 key pair and keeps it in
  `hive-identity.json` in the data directory (mode 0600 in a 0700 directory, like `secrets.json`). The **device id**
  is the SHA-256 of the public key, 64 hex characters. The key never leaves the device and is in no API, log,
  export or diagnostic. A damaged identity file is an error, never a silent new identity.
- **Announce.** The speaker announces `_clawdot._tcp` over mDNS/DNS-SD (RFC 6762, 6763): two announcements a second
  apart at start, answers to queries (at most one a second, with known-answer suppression), and a goodbye
  (TTL 0) on an orderly stop. Its TXT record is exactly `v=1`, `id=<device id>`, `p=<portal port>`. The device
  name is **not** published unless you pass `--hive-publish-name`; without it other speakers show the instance
  label `clawdot-<first 12 hex of the id>`.
- **Browse.** It asks for other speakers at start and again at 1, 2, 4... seconds up to every 15 minutes, and lists
  what it hears in memory only. A peer expires when its record's TTL runs out (75 minutes) or at once on its
  goodbye. At most 32 peers are kept. The address shown is where the packet came from, never a value read from the
  packet.
- **What is checked on what arrives.** A packet from a source address that is not loopback, private (RFC 1918) or
  link-local (IPv4 or IPv6) is dropped before it is read. A peer's address is the packet's source address. An A
  record in the response is ignored, so a machine with several addresses is listed under the one it sent from. The first address to announce an
  id keeps it until its record runs out, and only that address can say goodbye for it.
- **API.** `GET /api/hive/peers` (administrator only, [openapi](../api/openapi.yaml)) returns this speaker's id,
  name, public key and portal port, the discovery mode, and the peers with `last_seen_secs`.

## Turning it on

Build with the cargo feature (off by default; the default build, binary and idle behaviour are unchanged):

```
scripts/cargo.sh build --release -p clawdot-core --features hive
clawdot --hive-discovery native            # own responder on UDP 5353
clawdot --hive-discovery avahi             # the system's Avahi announces; ClawDot only browses
clawdot --hive-discovery avahi:/path/x.service
clawdot --hive-discovery off               # identity only, nothing on the network
```

Without `--hive-discovery` nothing starts and `/api/hive/peers` answers 404.

## The Dot and Avahi

**Tried on a Dot on 2026-11-11** ([proving pass](../reports/2026-11-11-proving-pass.md)): the TECHO5 image runs no
`avahi-daemon`. Its own voice service holds UDP 5353 for its own announcements, with address reuse. So on that image
use `native`: ClawDot's responder shared the port, an independent mDNS client on another computer got the service,
port, id and address back, and two speakers started on the Dot listed each other through `/api/hive/peers`.

The `avahi` mode is for systems that do run the daemon: ClawDot writes `/etc/avahi/services/clawdot.service` (type,
port and the same TXT set), which the daemon watches and publishes itself, and removes nothing on a crash (see below).
Browsing in this mode opens a second socket on 5353 with address and port reuse and joins the group. That mode has
not been tried on a real system. The
announcer sits behind the `Announcer` trait so another route (for example D-Bus) can replace the file later.

## What it does not do

- **No pairing, no trust.** Everything heard on the network is unauthenticated and advisory. The id in a TXT record
  is a claim; nothing checks it against a key until the pairing milestone. Do not act on the list.
- **No `clawdot.local`.** The host name is not answered (REQ-CFG-009 is not closed). An A record is sent only when
  `--listen` names a specific IPv4 address.
- **The network interface and the multicast group are not checked.** A packet is not tied to the interface it
  arrived on, is not checked to have been sent to the multicast group rather than to this host's own address, and
  its IP time-to-live (RFC 6762 asks for 255, which shows it did not cross a router) is not read. Those need socket
  options that the workspace's ban on `unsafe` code excludes. So a host attached to several private networks (wired
  and wireless, a VPN, a bridge to virtual machines) accepts answers from all of them, and a device on a routed
  guest network can answer. The checks above do not stop a device with a private address that lies about its id
  or name.
- **No probing or conflict handling**, no random response delay, no IPv6, no unicast responses, and only the
  default network interface. mDNS does not cross a router, a guest network or an isolated Wi-Fi.
- **No goodbye on a crash or kill.** The process has no signal handler, so a stopped speaker lingers in others'
  lists until its TTL runs out, and in `avahi` mode the service file stays and Avahi keeps announcing it.
- **No portal page** and no persistence of the peer list.

## Cost

Release binary with the feature on x86_64 Linux: 3,669,608 bytes without, 3,726,912 with, about 56 KB more. No new
crate is linked: `ring` is already built for TLS and `socket2` is already compiled as a dependency of tokio. Not
yet measured: the same figure on armv7, idle RSS, and idle network bytes on a Dot (REQ-NFR-101, REQ-NFR-109).

## Tests

All on loopback, with no multicast: identity stability, private file modes and tamper handling (Unix), the TXT round
trip and exact TXT set, hostile packets, two in-process announcers finding each other and forgetting on goodbye, a
browse-only node, a stale peer expiring, and the API. Real multicast on a LAN is not covered by an automated test.
