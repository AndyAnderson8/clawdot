# R-12: The hive: several ClawDot speakers sharing one knowledge base, with no server

Date: 2026-11-11 · Feeds: roadmap Beyond v1.0 item 20, [future scope](../future-scope.md) (multi-room and where-to-speak rows), a future spec (area code `HIV`, proposed below), a future ADR for the sync design · Builds on: [spec 0002](../specs/0002-resource-budget.md), [spec 0003](../specs/0003-satellite-protocol.md), [ADR-0015](../adr/0015-own-the-device-layer.md), [ADR-0019](../adr/0019-updates-and-remote-access.md), [R-07](07-echo-dot-gen2-hardware.md), [R-11](11-open-source-echo-feature-landscape.md)

Method: read-only. The repository was read first (specs, ADRs, the stores in `crates/core/src`). External claims come from primary sources fetched on 2026-11-11 and are listed in section 14 with a confidence level. Nothing was run on a device, nothing was installed, no code was copied. This is a research note: it proposes, it does not decide. Decisions become an ADR and spec requirements after the maintainer answers section 12.

## 0. Summary

A household with several ClawDot speakers wants them to behave like one assistant: a memory written in the kitchen is known in the bedroom, a skill edited once is edited everywhere, the language model and the spend cap are set once, and any speaker may be unplugged without the rest noticing. There must be no server, no leader and no cloud.

Recommendation in brief (full reasoning in sections 8 and 9):

1. Build a **small purpose-made replicated record store**, not a general CRDT library. Every shared fact is a record with a key, a value, a hybrid logical clock stamp, the id of the device that wrote it and that device's sequence number. The newest stamp wins per field; deletes are tombstones. This is the simplest convergent design, it matches the data (a few hundred small records), and it fits inside the budget.
2. **Gossip deltas pairwise**, Syncthing and Scuttlebutt style: each device keeps "the highest sequence I have seen from each origin", and two devices that meet exchange what the other is missing. Catching up after being offline needs no coordinator.
3. **Identity is a keypair per speaker**, the device id is the hash of its public key, membership is a signed roster, links are mutually authenticated TLS with pinned keys (rustls is already optional in the build). A new speaker joins with a short code shown on a speaker that is already in the hive, checked with a password-authenticated key exchange so a sniffer learns nothing.
4. **Secrets are shared only when the owner ticks them**, move only over the authenticated links, and a lost speaker means "remove it, then rotate the provider key at the provider". That last step cannot be done by software.
5. **The spend cap is split into shares** when speakers cannot see each other (cap divided by members, with a fresh-contact rule for borrowing), so the worst case stays near the cap instead of N times it.
6. **Who answers** is decided by the devices themselves in a 300 ms window from a signed per-wake score, as TECHO5 does. It fails open: if the exchange fails, behaviour is today's.

The first milestone (H1) is "pair two speakers and share memories and skills, with remove", usable alone, in about the size of the existing notes and skills code.

One correction to the brief: **ClawDot does not announce itself by mDNS today.** REQ-CFG-009 ("SHOULD be discoverable as `clawdot.local`") is not built (roadmap A6: "not started"). Discovery is therefore new work and is milestone H0.

## 1. What ClawDot already has (the starting point)

Read from the repository on `dev` (commit `7cd74ae`).

| Thing | Where | Shape today | What it means for a hive |
|---|---|---|---|
| Notes (memories) | `notes.rs`, `memory.json` | up to 50 notes, 200 characters each, 8 KiB in all; `Note { id: u64, text, ts }`; the id comes from a per-device counter `next_id`; written only by the host from the user's own words | Ids collide across devices (two devices both make id 7). A hive needs ids that are unique by construction. The caps (50, 8 KiB) apply to the merged set, not per device |
| Skills | `extensions.rs`, skills file | up to 16, name up to 40 characters, text up to 2000, up to 16 tools each; `enabled`; `deny_unknown_fields`; imports arrive turned off | Natural key is the name. `enabled` and the text change independently, so they should merge per field |
| HTTP tools, MCP servers, tiers, disabled sets | `extensions.rs` (`ToolsFile`, `ConnFile`) | up to 32 portal tools, 8 servers; risk-tier overrides by tool name; credentials are named by environment variable, never stored in the definition | Definitions can be shared. The credential an environment variable names cannot be, unless it goes through the secrets path |
| Operations vocabulary | `ExtOp` | `SkillAdd/Edit/Delete/Enable`, `ToolAdd/Delete/Enable/Tier`, `McpAdd/Delete/Enable`, `SkillImport` | Already the right set of change events. A hive can replicate these ops through the same validation the portal uses, so a remote change is checked like a local one |
| Settings | `settings.rs` | 25 fields in layers: built-in default, configuration file, `settings.json` (what the portal saved), command-line flag; the portal shows which layer won (`Source`) | A "hive" layer fits naturally between the file and the saved layer. A flag still beats everything, so an operator can pin a device |
| Secrets | `secrets.rs` | the provider key, kept in one 0600 file, never sent back whole, never in an export, "not encryption at rest" | The model for what a shared secret may and may not do |
| Spend and counts | `usage.rs` | per local day; request count and spend; `daily_spend_cap_usd`; 30 days kept | A cap that is read and written by one process. Needs a per-device share when several processes spend (section 5.7) |
| Audit | `audit.rs` | append-only JSON lines, 256 KiB per file, two files | Stays per device. Replicating a log is not needed; reading a peer's log through the hive is optional |
| Admin | `admin.rs` | one password, PBKDF2-HMAC-SHA256 100,000 iterations, `pbkdf2-sha256$iter$salt$hash`; up to 8 sessions, 12 h; reset window opened by a device button | The hash can be shared; sessions and the reset window cannot and should not be |
| Clock | `sched.rs` (`wall_clock_valid`: after 1,700,000,000), R-07 section 7 | The Dot's RTC reads 2010; after a reboot the clock is restored from a saved value that was **about 3.5 hours behind**: "plausible but wrong" until the first sync | Wall clocks cannot order edits on these devices. A logical clock is required |
| Crypto in the build | `Cargo.toml` | `sha2` always; `ring` and `rustls` behind the `tls` feature; Ed25519 verify already used by the updater (`update.rs`, ADR-0019); a Noise NNpsk0 client on `ring` behind `esphome-noise` | Ed25519, X25519, ChaCha20-Poly1305, HKDF and TLS 1.3 are all available without a new family of dependency |
| Network listeners | `REQ-CFG-014` and architecture/configuration | one HTTP listener, default `127.0.0.1:3335` (loopback) | A hive needs a **new** listener on the LAN. It must not exist until a hive is joined |
| Budget | spec 0002 | idle RSS 24 MB, peak 64 MB (design target 48), binary 8 MB, persistent storage 16 MB, idle network 1 KB/s average, 24-hour heap growth under 1 MB, optional features off by default (REQ-NFR-113) | Hive code is an optional feature; a single-speaker default configuration pays nothing |
| Multi-room in the docs | `future-scope.md` | "Multi-room / announcements between satellites: Out of scope, not planned"; "where-to-speak for inbox events: needs a second satellite" | This note does **not** propose synchronised audio. It covers shared state and who answers. The future-scope rows need a clarifying edit (done with this note) |
| Satellites | spec 0003, spec 0004 | A satellite (a Pi, a PC, a Dot running a base image over ESPHome) attaches to **one core** and the core runs the brain | A hive member means a **full ClawDot core** (the all-in-one build). Satellites hanging off one core are that core's business. See question 1 |

## 2. What the state of the art says

Details, versions and confidence are in section 14. This section keeps only what changes the design.

**Local-first and CRDTs.** The local-first essay (Kleppmann, Wiggins, van Hardenberg, McGranaghan, 2019) lists "the network is optional", "not trapped on one device" and "ultimate ownership and control" as ideals; the hive is those three ideals for a household appliance. CRDT libraries exist in Rust: Automerge (crate 0.12.0, 2026-09), Loro (1.17.1, 2026-10), Yrs, the Yjs port (0.28.1, 2026-11). They give rich merging of text and lists, which ClawDot does not need: its shared data is a handful of small records. Their cost is a history that grows (or must be compacted), an opaque binary document format that makes mixed-version hives awkward, and code size that has not been measured on armv7. cr-sqlite does per-column last-writer-wins over SQLite, which is the model we want, but ClawDot has no SQLite and cr-sqlite is a C-and-Rust loadable extension with no stable tagged release visible on its page. The idea transfers; the dependency does not.

**Syncthing** is the closest working system to "devices that find each other, trust by key, and converge without a server". Its device id is the SHA-256 of the certificate shown as grouped base32 with check digits; connections are TLS 1.3 with the device id pinned; each side sends an index and later deltas keyed by `(index id, max sequence)`; each file carries a version vector; local discovery is a UDP announcement every 30 to 60 seconds on port 21027 carrying the device id, addresses and a random instance id that changes on restart. ClawDot copies the identity, the pinning and the "highest sequence per origin" delta idea. It does not need version vectors per record, because the hive's conflict rule is last writer wins, not "keep both files".

**Gossip membership.** SWIM (Das et al., 2002) and its descendants (HashiCorp memberlist, which adds the Lifeguard extensions; Serf; the Rust crates `foca` 2.0.0 and `chitchat` 0.14.0) exist to keep failure detection cheap for hundreds of nodes. chitchat is the instructive one: each node owns a key-value namespace that only it writes, peers exchange deltas by version, liveness is "a local judgment" from a phi-accrual detector, deletes are tombstones removed after a grace period (its docs recommend 15 minutes), and its docs say plainly that deleted state can resurface if a node reconnects after the grace period. A household has two to eight speakers. At that size every device can talk to every other device directly; SWIM's indirect probing and dissemination tricks buy nothing. Take the lessons (liveness is local, tombstones need a horizon, "too stale" must be handled on purpose), not the libraries.

**Matter fabrics.** A Matter fabric has a root certificate; every node holds a node operational certificate under it; a new node is commissioned with a short passcode over a password-authenticated session (PASE), is then given its certificate, and afterwards nodes authenticate each other with certificates (CASE); an existing administrator reopens a "commissioning window" to add another ecosystem. This is the right shape for pairing without a server: a short code, valid briefly, used once to bootstrap keys, then keys do the work. We do not adopt Matter's certificate hierarchy (no controller, no vendor attestation); a signed roster of public keys is enough for eight speakers.

**Home Assistant.** Its Assist pipeline reference notes that a satellite sends `wake_word_phrase` so that "Home Assistant uses it to avoid several devices answering the same wake word", and defines the error `duplicate_wake_up_detected`: "another device already answered the same wake word". It does not say how a winner is chosen. A community reply says the satellite that **began streaming first** wins, and other users report it working badly with several Voice PE units. Home Assistant has a hub (the server) doing that arbitration, which a hive lacks. I found no peer-to-peer multi-instance sync in Home Assistant itself; it solves the household problem with one server (not verified beyond that, section 14).

**Who answers: three precedents.**
- Amazon's Echo Spatial Perception: devices that hear the wake word compare how loud and clear it was and only the nearest answers; Amazon later moved this into its cloud. Exact signalling is not public. Users report it failing ("the furthest Echo sometimes responds").
- Academic and vendor descriptions (a Diehl paper surfaced in search): each device estimates distance, usually from wake word level in dB, they share estimates, and any device that is not closest returns to idle.
- **TECHO5**, the Dot base image the maintainer already studies (R-08, R-11), documents the design the maintainer referred to. From `docs/setup.md` in `HuskerMinion/techo5` (read via the GitHub API, 2026-11-11): a device that hears its wake word tells the others "how clearly it heard it" and waits a short window (default 300 ms, adjustable 100 to 500) to hear from them; only the best answers and the others "go back to listening without a sound". "How clearly" is **how far the wake word stood out from that device's own room**, so a device with hot microphones or in a noisy room does not win by that alone. Each device has a per-device offset (-30 to +30 dB). Messages are signed with a shared "house word" (16 characters or more advised) so nothing else on the network can silence the devices. A device that already answered keeps the turn; a device that cannot answer (the server is unreachable) does not take part, so it never silences one that could; and **if no other device answers in time the device answers as if the feature were off**, so "the worst a fault can do is what happened before". The same document has a "Sharing settings with the other devices" section that is also directly relevant: opt-in per device, per setting links, "there is no main device: every change is numbered one past the last, and the latest number wins, whatever any device's clock says", retries up to once a day for a week, a limit of 16 devices per link, "Keep mine" to pin a value, and an explicit list of what is never shared (anything that opens the device to the network, keys, passwords, the device's name, microphone behaviour). That is a field-tested, deliberately modest design, and it is the best single reference for ClawDot's choices.

**Clock trust.** The standard answer for devices whose wall clocks lie is a hybrid logical clock (Kulkarni, Demirbas, Madappa, Avva, Leone, "Logical Physical Clocks", OPODIS 2014): a pair `(l, c)` where `l` follows the highest physical time seen from the local clock or from any message and `c` is a counter that breaks ties, so stamps never go backwards and stay close to real time when clocks are good. TECHO5's "one past the last number" is the same idea reduced to a counter.

## 3. What is shared and what is not

The rule: **share what describes the household and what the assistant knows; keep what describes this box and this room.** A shared item is shown with a "Hive" badge in the portal; a per-speaker item with "This speaker". A setting can be shared by default and still carry a "Keep mine" pin, as in TECHO5.

### 3.1 Shared (hive scope)

| Data | Why shared | Merge unit | Notes |
|---|---|---|---|
| **Memories** (notes) | "remember that the printer is called Pixel" should hold in every room | one record per note, id = `origin:seq` | Still written only by the host from the user's words. Privacy: a note said in one room is now known in all. See question 3 |
| **Skills** | edit once | record per skill name; fields `instructions`, `tools`, `enabled` merged separately | A skill that names a tool this speaker does not have stays inactive here and says why |
| **HTTP tool and MCP server definitions**, and their enabled flags and **risk-tier overrides** | the same tools everywhere | record per name | Credentials are not in definitions (they are environment variable names). The variable's value is per device unless shared as a secret (3.3) |
| **Rules and routines** (the standing "when X, tell me" rules of roadmap A5b, and later routines) | the household rule, not the room's | record per rule id | Which speaker acts on a rule is a separate decision (section 5.8) |
| **Household settings**: `zone` (time zone), `speech_language`, `persona`, `llm_url`, `llm_model`, `llm_key_env`, `daily_spend_usd`, `quiet_start`, `quiet_end`, `notes` switch, agent limits (`agent_max_*`, `plan_speech`, `progress_speech`), `max_actions_per_day`, `max_costly_per_day`, `dry_run`, `update_check` | one decision per household | one record per setting key | `dry_run` and the action caps are safety settings: see the safety rule in 5.6 |
| **The provider key** (and any other named secret the owner ticks) | otherwise every speaker has to be given the key by hand | one record per secret name, value only moves over authenticated links | Section 4 |
| **The admin password hash** | one password for the household | one record | Sessions stay per device. See 4.4 |
| **Inbox endpoint definitions** (name, filter, priority mapping, token **hash**) | the agent can post to any speaker | record per endpoint | Later milestone; one event must still be spoken by one speaker (5.5) |
| **Spend counters** | the cap is a household cap | per-device per-day counters | Section 5.7 |
| **Roster** (members and removals) | who is in the hive | signed membership records | Section 4 |

### 3.2 Per speaker (never shared by the hive)

`device_name`, `voice` (the voice that speaks from this box), `listen` and `public_url` (addresses), `board` and everything hardware (LED colours and effects, microphone gain and wake sensitivity, wake word choice on this box, mute state, volume, Bluetooth), the **nearest-device offset** (section 7), timers and stopwatches (a timer set in the kitchen rings in the kitchen; alarms and reminders default to this speaker and may later gain a "ring everywhere" option), the audit log, the usage history charts, sessions, the reset window, the device's identity key, the update channel and installed version, and the logs. These match TECHO5's own list of what is never shared (names, network exposure, microphone behaviour) with the addition of the audit log, which should remain a record of what **this** box did.

### 3.3 Shared secrets are a separate class

A secret is shared only if the owner ticks "share with the hive" for that secret, in the portal, with the admin signed in. The default for the provider key is **ask once during pairing** ("Use the same language-model key on the new speaker?"). Never in settings exports (today's rule stays), never in diagnostics, never in the roster, never shown whole.

## 4. Network, identity, trust

### 4.1 Discovery (H0)

- **mDNS / DNS-SD** (RFC 6762 and 6763): a service `_clawdot._tcp` on UDP 5353, multicast 224.0.0.251 (IPv6 ff02::fb). RFC 6762 rules the implementation must obey: startup probing (three queries 250 ms apart) and at least two announcements one second apart; no periodic announcements as a matter of course; host record TTL 120 s, other records 75 minutes; a goodbye with TTL 0; known-answer suppression; no more than ten updates a minute. So the idle cost of discovery is a few packets at startup and on change, well inside REQ-NFR-109.
- TXT record: `v=1` (hive protocol version), `id=<first 16 characters of the device id>`, `h=<first 8 hex of a hash of the hive id>` (so a speaker can tell "my hive" from a neighbour's without a connection; the hive id itself is not published), `p=<port>`, and `s=unpaired|member`. **No name, no address list, no version string beyond the protocol number**: whatever the TXT says, the entire Wi-Fi sees.
- **Implementation:** the `mdns-sd` crate (0.21.7, 2026-11-02, Apache-2.0 or MIT, no async runtime, its own thread, "beta"; it does not implement unicast responses, the 250 ms probe spacing, or some delay windows exactly as the RFC says, and was only checked against Avahi, dns-sd and Bonjour at a basic level) or a small hand-written responder. A Dot already runs `avahi-daemon` (R-07 section 8: `linux.local`), so on that device the better route may be to register a second service with the existing daemon than to open port 5353 a second time (two responders on one host conflict). This is an open item for the device spike (question 11).
- **Fallbacks, because mDNS fails on real networks:** every device gossips the last known addresses of members, so one rediscovery re-finds the rest; a hive page lets the owner add a member by address; and a device that was reached once tries its last address first on boot. Guest networks and client isolation are in 4.7.
- **H0 is useful alone:** it also closes REQ-CFG-009 (`clawdot.local` for the portal).

### 4.2 Identity

- At first hive use, a speaker makes an **Ed25519 keypair** (`ring` can generate and verify; the updater already verifies Ed25519 with `ring`). The private key sits in a 0600 file like the secrets file. Not encrypted at rest (same honesty as `secrets.rs`).
- The **device id** is the SHA-256 of the public key, shown as grouped base32 with a short check group, like Syncthing's. The portal shows the first group everywhere so the owner can compare two screens.
- Links use **TLS 1.3 with mutual authentication and pinned keys** (rustls with a custom verifier that accepts exactly the roster's keys, as Syncthing does with certificate fingerprints). Needs a self-signed certificate wrapper around the Ed25519 key: `rcgen`-like code or a small hand-built DER. This is the one piece whose dependency cost I could not measure (question 10). A Noise XX/IK handshake on `ring` is the alternative; the repository already has a Noise NNpsk0 client of about 200 lines (R-08), so Noise is not foreign, but it would be a second hand-written secure channel to maintain, where rustls is a reviewed one.
- Every replicated record is **signed by its origin** (Ed25519, 64 bytes). Relaying through a third speaker cannot forge a record; a compromised speaker can only forge its own.

### 4.3 The roster and pairing without a server

- **Starting a hive:** the first speaker makes a random 16-byte hive id and a roster record that admits itself. A hive of one is legal and costs nothing.
- **Adding a speaker (a commissioning window, as in Matter):**
  1. The owner, signed in to the portal of a speaker already in the hive, presses **Add a speaker**. That speaker opens a pairing window: five minutes, one pairing at a time, and shows an **eight-digit code**.
  2. The new speaker advertises `s=unpaired`. It appears in a list on the first speaker's page. On a new speaker with no portal session, the owner opens the new speaker's setup page and types the code. (Speakers have no screens; the portal is the interface, as today.) Physical confirmation by the new speaker's action button within the window is an optional second factor, mirroring the existing button-opened reset window (REQ-CFG-069), and I recommend it: it proves the person is next to the speaker they are adding.
  3. The two run **SPAKE2** (RFC 9382; Rust crate `spake2` 0.4.0) with the code as the password and exchange their public keys inside the authenticated channel, with the key-confirmation MAC. An observer of the exchange cannot recover the code offline; an active attacker gets **one** guess per window (three failed tries close the window).
  4. The existing speaker signs a roster record "admit `<device id>`" and the new speaker receives the roster, the hive id, its first sync, and, if ticked, the provider key.
  5. The new speaker's own pre-existing notes and skills are **merged in** by default (name clashes keep the hive's version and import the other as a disabled skill with the speaker's name appended); "join and replace" is the other choice.
- **Any member may admit**, but only with the admin signed in on that member *and* a pairing window opened by hand. There is no hive-wide admin key to guard, and no single member whose loss ends the hive.
- **Fallback if SPAKE2 is rejected for size:** compare a short authentication string (the first group of both device ids, as a 4-digit number derived from both public keys) on the two portal pages. That is human-checked key agreement and needs no PAKE, at the cost of one more look at one more screen.

### 4.4 Removing a speaker (revoking a lost or sold one)

- Any member, with the admin signed in, writes a signed **removal** record for a device id. Removal **wins over everything** and is permanent for that key (a returning speaker gets a new key and pairs again). Members drop its links and refuse its handshakes.
- The removing speaker then **rotates the hive's election MAC key** (section 7) and sends the new value to each remaining member individually. Nothing else is encrypted under a hive-wide data key (records ride only inside the pairwise TLS links), so there is no bulk re-encryption to do.
- **What removal cannot do:** a speaker that was lost *offline* still holds a copy of all shared data and any shared secret, and nothing can reach into it. Therefore removal always ends with a plain instruction in the portal: "This speaker held your language-model key. Create a new key at the provider and paste it here; delete the old one there." Software cannot replace that step.
- A removed speaker that comes back online and reaches a member is told it was removed and (if still running ClawDot normally) **wipes hive data and shared secrets and leaves the hive**. This is a courtesy for the honest case.

### 4.5 Ports and traffic

Proposal, to be settled in the spec: one TCP port for the hive links (a fixed default such as 3336 next to 3335, configurable), one UDP port for the election datagrams (the same number), and UDP 5353 for mDNS. Nothing binds until the speaker is in a hive. Steady state for N members: a heartbeat to each peer every 15 s on the existing link (about 40 bytes plus TLS overhead), so for eight members roughly 7 × ~100 B / 15 s ≈ 50 B/s, against the 1,024 B/s idle limit. Chatty phases (initial sync, a large skills import) are bursts of tens of KB.

### 4.6 What an attacker on the Wi-Fi can and cannot do

| Attacker | Can | Cannot |
|---|---|---|
| Passive listener | see mDNS (a ClawDot hive exists, device-id prefixes, its port), see TLS handshakes and traffic volume, see election datagram timing | read any record (TLS 1.3), learn the pairing code (PAKE), learn scores' meaning without the MAC key (they are authenticated, not encrypted: scores reveal "someone woke the kitchen speaker now"; accept, or encrypt the datagram with the same key) |
| Active, no key, no code | connect to the hive port and be dropped at the handshake; flood the port (bounded pre-auth resources and per-source limits, the same pattern as REQ-SEC-008); spoof mDNS to point peers at itself (it fails the pinned-key check); try the pairing code during an open window (one guess per window with SPAKE2, three failures close it) | join, read, write, or silence the election (datagrams are MAC-authenticated) |
| Active, can open the portal over plain HTTP | try the admin password (existing rate limits), read portal traffic **in clear text** on the LAN (existing weakness: HTTPS for the portal is parked in future scope, and a hive makes it matter more because the same password now guards several speakers and the shared key) | skip the password |
| Wi-Fi password holder with a laptop | everything above | pair, because pairing needs the code **and** a signed-in admin on a member |

The portal's lack of HTTPS is the weakest link in this story. The hive adds a reason to promote the parked "HTTPS option" row in `future-scope.md` (question 9).

### 4.7 Guest networks, VLANs, client isolation

mDNS is link-local multicast; routers do not forward it between a guest network and the main one, and many access points isolate Wi-Fi clients from each other. A speaker on a guest SSID will not discover or be discovered. The design says so rather than hiding it: members must share one broadcast domain, or the owner adds the address by hand and the router must allow the TCP and UDP ports. Crossing homes or sites would need a private overlay network the household already runs; the hive does not provide one and does not depend on one. A speaker that cannot reach anyone simply behaves as a hive of one (section 6).

## 5. Propagation, offline behaviour and the rest

### 5.1 Data model

One logical table of **records**:

```
record = {
  collection : "note" | "skill" | "tool" | "mcp" | "setting" | "secret" | "rule" | "endpoint" | "spend" | "roster" | ...
  key        : string            e.g. note id "k3f2:41", skill name, setting key
  field      : string            e.g. "text", "enabled", "instructions"; "" for the whole value
  value      : json or tombstone
  hlc        : (l: u64 ms, c: u16)
  origin     : device id (short)
  seq        : u64               per-origin, increases by one for every record the origin writes
  sig        : 64 bytes          Ed25519 over the above
}
```

The state is the **map from (collection, key, field) to the record with the greatest (hlc, origin)**. That is a last-writer-wins register per field, the same family as cr-sqlite's column rule and as the setting-by-setting links in TECHO5. It is convergent: merging is commutative, associative and idempotent, so order, duplication and delay do not matter.

Unknown collections and fields are **kept and relayed unchanged** even when this build does not understand them. That is what lets a hive of speakers on different versions (updates are per-device and by confirmation, ADR-0019) keep working: an old speaker neither crashes nor blocks the new field from reaching a third speaker.

### 5.2 Ordering, clocks and the no-RTC problem

- Stamps are **hybrid logical clocks**. On a local write: `l = max(l_prev, wall_now)`, and `c` increments if `l` did not move. On receipt: `l = max(l_local, l_remote, wall_now)`. The persisted high-water mark is written with the store, so a Dot that boots with a clock **3.5 hours behind** (R-07) cannot stamp anything earlier than what it had already issued.
- A speaker whose clock is not trusted (board `ClockStatus`, `wall_clock_valid` false) first takes `l` from the highest stamp it has seen from peers and does not issue its own `l` ahead of `max seen + 1`. A freshly flashed speaker with no history and no peers is the only case that stamps from its raw clock; it is a hive of one, so nothing can lose to it.
- **Future poison:** a speaker whose clock is a year ahead would win every conflict for a year. Defence: a receiver compares each incoming `l` against the best time it trusts; if it is more than a day ahead it **quarantines** the record (kept, not applied, and listed in the portal with the sender). Local human edits always get `l = max(everything seen) + 1`, so a person's own edit on a speaker beats anything that speaker has already seen.
- Equal `(l, c)` ties break by device id, so every device picks the same winner.

### 5.3 Sync

- Each device keeps, per origin, `highest_seq_seen` (a vector of at most 8 numbers) and the log of records. Two devices on a link swap their vectors and each sends the records the other lacks (Syncthing's delta index by max sequence, Scuttlebutt's reconciliation). A new write is also pushed at once to connected peers.
- A record is relayed through any member, so two speakers that can see a third but not each other still converge.
- **Delivery to a device that was off:** nothing is queued for it; it asks for the difference when it returns. That removes TECHO5's retry schedule and the "no answer after a week" state.
- **Unreachable member:** it appears as offline with a last-seen time (section 5.4). No change waits for it.

### 5.4 Membership and failure detection

Full mesh: with at most 8 members every device keeps a TLS connection to each other device it can reach, with a 15-second heartbeat and a miss count (three misses mark a peer offline). Liveness is a **local opinion** (chitchat's phrase), used only for the portal list and for the election pool, and **no operation ever blocks on it**. No SWIM, no indirect probes; revisit if the member limit rises above 16 (TECHO5 also uses 16).

### 5.5 Deletes and the "too stale" case

- A delete is a tombstone with its own stamp; it competes by last writer wins like any value. Tombstones are kept for **30 days** (the same order as `usage.rs` `KEEP_DAYS`) and then collected.
- A speaker unplugged for **longer than the tombstone horizon** could resurrect a deleted skill, the exact hazard the chitchat documentation warns about. Rule: on rejoining, a member whose newest seen sequence for some origin is older than the horizon enters **catch-up as a replica**: it takes the hive's state wholesale for shared collections, and its own unsynced shared changes are not applied silently but listed ("Changes made on this speaker while it was away") for the owner to re-apply or drop. A short absence, the normal case, is a plain merge.
- Deleting a note and editing it concurrently on another speaker: the later stamp wins. If the edit is later, the note comes back, which is the outcome a person who edited it would expect.

### 5.6 Conflict rules by kind of data

| Data | Rule | Reason |
|---|---|---|
| Notes | Add: always keeps both (distinct ids). Same normalized text twice: shown once, newer kept (the existing duplicate rule). Edit: last writer wins on `text`. Delete: tombstone | A person rarely edits; mostly they add and forget |
| **The notes cap** (50 notes, 8 KiB) | The merged set may exceed the cap after a partition (two speakers each added 30). **Nothing is dropped.** The oldest by stamp fill the cap and are shown to the assistant; the rest are listed in the portal as "over the limit" until the owner deletes some. The hive record count is capped at twice the limit, beyond which a new note is refused on the speaker with the existing "memory is full" message | Dropping a memory silently is the worst outcome; refusing it is today's behaviour |
| Skills | Per field. Edit of `instructions` and toggle of `enabled` on different speakers both survive. Name is the key; a rename is delete plus add. Two different skills created with one name on two speakers merge as one skill (last writer wins); the loser's text is not lost: it is kept in a "previous versions" list of at most 3 per skill (bounded). Cap 16 skills: same over-the-limit handling as notes | Matches the per-field LWW in cr-sqlite and TECHO5's per-setting links |
| Tools and MCP servers | Per field; `ToolTier` changes follow the safety rule below | |
| Settings | Last writer wins per key; "Keep mine" pin makes a key local | |
| Secrets | Last writer wins per name; a clear is a tombstone | |
| Admin password hash | Last writer wins; a change also invalidates all sessions on all members (each member drops its sessions when the record arrives) | |
| Spend counters | Not LWW: see 5.7 | |
| Roster | Removal beats admission regardless of stamp | Security wins over availability here |
| Rules | Per field; firing is a separate record (5.8) | |

**Safety rule for changes that lower safety.** A remote change that **lowers** a safety setting (a tool's risk tier moved to a lower tier, `dry_run` turned off, an action cap raised, a new tool turned on that can act, a quiet-hours window removed) is applied, but the receiving speaker shows "arrived from `<name>`, review" in the portal, and the confirmation machinery of action safety (`action-safety.md`) still runs on every device, because it is host-held and local. A compromised member can therefore not make another speaker act without asking, unless the *owner* lowers the tier. Whether to require an extra confirmation on the receiving side is question 6.

### 5.7 The spend cap when speakers cannot see each other

The cap protects the owner's money. The risk is N speakers each believing the whole cap is still available. The design below is a guardrail with a stated bound, not a bank.

- Each device counts its **own** spend per local day in its own record `(spend, day, device)`; only that device writes it, so counters never conflict. The hive's spend today is the **sum** over members. Everyone sees the sum as soon as they sync (this is a grow-only counter per device, the simplest CRDT).
- The day boundary uses the shared `zone` and a trusted time; a speaker with an untrusted clock takes the day from its peers' stamps and is limited to its share until it has one.
- **Share rule.** Let `cap` be the household cap, `N` the number of members in the roster, and for each other member `j` let `assumed_j` be its reported spend if the last report is fresh (heard within 10 minutes), else `max(reported_j, cap / N)`. A device may spend while `own + Σ assumed_j < cap`. When the device is alone (all others stale), this is `own < cap / N`.
- **Borrowing rule.** A device may exceed `cap / N` only while **every** other member's report is fresh, and it must publish its new total to all members before the model call that crosses its share. That limits the overshoot to what one device spends in the last ten minutes of staleness plus one in-flight turn.
- **Result:** if all speakers are isolated, the household spends at most `cap` (each limited to its share) plus in-flight turns. If speakers are connected, the cap is shared exactly. The worst case is bounded by `cap + N × (spend in one staleness window + one turn)`, not unlimited. With a cap of a few dollars and turns of a few cents that is small, but it is **not** a hard guarantee, and the portal should say "about" when it shows a partition. A provider-side limit on the key (some providers offer key limits; not verified here) is the real hard stop and the portal should recommend it.
- The shared `daily_spend_usd` only has effect on a device that knows it; a device that has never synced uses its own configured value (today's behaviour).

### 5.8 Who runs a rule, who speaks an inbox event

Without a leader, "exactly one speaker does it" uses **rendezvous hashing** over live members: score = hash(rule id, day, device id); the live member with the highest score fires. If two speakers disagree about who is alive (a partition), both fire. At-least-once is the guarantee; the `fired(rule, instant)` record is gossiped, so reunited speakers see the duplicate. For "tell me" rules a duplicate announcement is an annoyance, not damage. Rules that **act** (send, buy, switch) must carry an idempotence note, or stay per-speaker, until the owner chooses. An inbox event posted to a webhook is handled by the speaker that received it; a shared endpoint (3.1) means an agent can post to any speaker, and routing it to the nearest or last-used speaker is the "where-to-speak" row in `future-scope.md` and belongs to a later milestone.

## 6. A speaker on its own, and catching up

| Situation | What the speaker does |
|---|---|
| Alone from the start (not in a hive) | Exactly today's behaviour; no hive code runs |
| In a hive, all peers unreachable | Everything works from its last copy: wake, voice, timers, memories, skills, tools. Local writes are stamped and logged. The spend share rule limits the model. The portal says "alone since 14:02; 2 of 3 speakers not seen" |
| Some peers reachable | Syncs with those; relays what they tell it |
| A peer returns | Heartbeat resumes; the two swap vectors and deltas; typical catch-up is a few KB |
| Away longer than 30 days | Replica catch-up (5.5) |
| Different versions | Unknown fields relayed; a protocol major mismatch means that link is refused and the portal says "needs update"; the other links work |
| Clock wrong | Section 5.2; a record stamped far in the future is quarantined, not applied |
| Disk full / store write fails | The write is refused on the speaker with today's "could not be saved" message; the hive store is bounded (section 8.3) |
| The shared provider key is wrong or revoked | Every speaker fails the same way and says so; a new key pasted on one speaker flows to the rest when they next meet |

Idle behaviour is the same as today when no peers exist: the mDNS responder and a listener, no work.

## 7. Who answers when two speakers hear the wake word

Adopt TECHO5's design, with four ClawDot adjustments.

1. **Score.** Each speaker computes, at the wake word, how much the detected wake word stood out from that speaker's own recent noise floor (a dB-like margin), plus a per-speaker offset (-30 to +30 dB, a per-speaker setting, default 0). Raw loudness is not used, for TECHO5's stated reason. Whether the wake engine exposes a usable margin (microWakeWord/openWakeWord score is a probability, not a level) is an open technical question for the wake-word runtime (spec 0007) (question 8). If the engine exposes only a probability, use probability and the frame energy ratio together, and measure.
2. **Exchange.** Within a few milliseconds of the wake word, the speaker sends a small datagram `{wake_id, score, device_id, hlc}` to every live member (unicast fan-out, not multicast: Wi-Fi multicast is slow and unreliable; with at most 7 peers, unicast is cheap), authenticated with a MAC under a hive key that is rotated on every removal. It then waits a window (default 300 ms, 100 to 500, a hive setting) for the others.
3. **Decision.** Highest score answers; ties go to the lowest device id; a speaker that **already answered** keeps the turn; a speaker that **cannot answer** (muted, in an error state, no provider key, over its share of the cap) does not take part, so it never silences one that could. Losers return to listening, send no audio anywhere, and make no sound.
4. **Fail open.** If no datagram from another speaker arrives in time, the speaker answers as it would alone. The worst a fault can do is two speakers answering, which is today's behaviour in a house with two speakers.

Extras: a follow-up turn ("yes", "no, the other one") stays with the speaker that has the open conversation; a speaker that is currently speaking or playing music is a poor choice for a new question only if the owner says so (not default); a button press on one speaker always answers on that speaker. A duplicate **action** is the real danger (two speakers both sending the same message): the host-held confirmation machinery is per turn on the speaker that holds the turn, and the loser's turn is aborted before the model is called, so a duplicate needs a failure of both the election and the abort. Satellites attached to one core are the core's problem (spec 0003), not the hive's.

Compared with the precedents: this is Echo Spatial Perception without the cloud and without a leader; Home Assistant's "first to stream wins" has a hub to be first at, which a hive lacks, and users report it unreliable. Measurement of real-room behaviour is a milestone exit test, not a paper claim (section 10, H4).

## 8. Options compared

### 8.1 Options

| | A. Do nothing: manual export and import | B. A leader (one speaker or a NAS holds the truth) | C. A CRDT library (Automerge, Loro or Yrs) over our transport | D. cr-sqlite | E. A gossip library (chitchat or foca) for state and membership | **F. Purpose-made signed LWW record store (recommended)** |
|---|---|---|---|---|---|---|
| Meets "no central server, any device may be off" | Yes, trivially, because nothing syncs | **No**: leader off means no propagation and a single point of failure; explicitly rejected | Yes | Yes | Yes | Yes |
| Convergent by construction | n/a | Yes | Yes (best on rich data) | Yes (per column) | Per node namespace; shared collections need an op layer on top | Yes (LWW per field is the simplest CRDT) |
| New code on the device (estimate) | 0 | 1,000 to 2,000 lines (client, election of the leader, failover, which defeats the point) | 500 to 1,500 lines of glue plus the library | needs SQLite in a project that has none; large | 800 to 1,500 lines glue plus library | 2,000 to 3,000 lines (store, sync, HLC, roster) plus tests |
| Binary size cost | 0 | small | **unmeasured, likely large** (full CRDT engine; Automerge has WASM and C heritage; the Rust crate is documented as low level) | SQLite plus an extension, many hundred KB | unmeasured, moderate (async runtime, UDP) | small, we write only what we use |
| Memory | 0 | small | history grows unless compacted; the Automerge 3 release notes claim a large memory reduction (not verified here) | SQLite page cache | moderate | **under 1 MB** (hundreds of small records) |
| Mixed versions (devices update one by one) | n/a | simple | opaque document format; a schema change touches every device | schema migrations over a replicated database | KV strings are version-neutral | **unknown fields are relayed**; versioned records |
| Complexity we own | none | high operationally | medium (library bugs are theirs) | high | medium | **medium-high (we own the correctness)**; mitigated by a randomised convergence test (below) |
| Fit for 8 devices, ~300 records | n/a | fine | far more than needed | far more than needed | built for hundreds of nodes | matches |
| Fit for secrets and signatures | n/a | easy | secrets inside a CRDT document are replicated forever with history | same | custom | per-record signatures and a secrets collection that can be cleared; history of a secret is just the latest record |

### 8.2 Why not a CRDT library

The data is a small map of independent records with small values. The strong points of Automerge, Loro and Yrs (merging concurrent text edits, ordered lists, move operations) are not used. Their costs are real: a binary format that cannot easily be audited for secrets, history that must be compacted, and a measured-on-armv7 size we do not have. The decision can be revisited if the hive ever needs collaborative editing of long text (no such feature is planned). The simple design is also easy to **test exhaustively**: a deterministic simulation that applies random operations on three to five replicas with random partitions, duplicates and reorderings, then asserts all replicas have identical state after a final full sync, runs in milliseconds and catches the bugs hand-written sync code usually has.

### 8.3 Costs of the recommendation against spec 0002

Estimates; none has been built or measured. Each becomes a measured gate when the milestone lands (REQ-NFR-114 style ratchet).

| Limit (spec 0002) | Single speaker, hive off | Hive on, 8 members (estimate) |
|---|---|---|
| Idle RSS ≤ 24 MB | unchanged | +1 to 3 MB: record store under 1 MB, TLS state per peer link about 20 to 40 KB each (7 links under 0.3 MB; rustls figure unverified), mDNS responder tens of KB |
| Peak RSS ≤ 64 MB (target 48) | unchanged | initial sync of a full skills import adds a few hundred KB transiently |
| Binary ≤ 8 MB | unchanged (feature off by default, REQ-NFR-113) | +100 to 300 KB for the hive, mDNS, `spake2` and the certificate wrapper (rustls and `ring` are already there under `tls`) |
| Persistent storage ≤ 16 MB | unchanged | hive store ≤ 256 KB hard cap including a 30-day tombstone log; keys a few hundred bytes |
| Idle CPU ≤ 15% | unchanged | heartbeat and timers: negligible; signature verification only on receipt of new records (Ed25519 verify is on the order of tens of microseconds on a desktop CPU, not measured on a Cortex-A53) |
| Idle network ≤ 1 KB/s | unchanged | about 50 B/s heartbeats at 8 members |
| Heap growth over 24 h < 1 MB | unchanged | all collections bounded; tombstone log capped; peer table capped at 16 |
| Cold start ≤ 5 s | unchanged | roster and store loaded from disk; links dialled in the background, never blocking wake-ready |

Record sizing: a note ~250 B, a skill ~2.2 KB, a setting ~150 B, plus ~120 B per record for stamp, origin, sequence and signature. A full hive holds at most ~50 notes + 16 skills + 32 tools + 8 servers + ~25 settings + rules and spend entries, so on the order of 100 to 150 KB. Under the cap with room.

### 8.4 Recommendation

**Option F**, staged so that the first stage needs only the pieces for notes and skills. The reasons, in order: it meets the requirement with no leader; it is the smallest design whose correctness can be tested to exhaustion; its cost against the 480 MB-class budget is small and bounded; the failure modes are boring (a lost update to a rarely edited record, never a corrupt store); and it borrows only ideas with years of field use (Syncthing identity and deltas, Scuttlebutt/chitchat tombstones and liveness, hybrid logical clocks, TECHO5 sharing and election rules, Matter-style commissioning).

## 9. Portal story (in words)

- A **Hive** tab on every speaker's portal. It lists the members: name, a short device id, online or offline with "last seen", software version, and whether its clock is trusted. A speaker that is out of date or on a different protocol is flagged. A banner says "alone since ..." if the speaker sees no one.
- Settings, Skills, Tools and Memory show the **same hive content from any speaker**, because each speaker has the whole copy. A small "Hive" or "This speaker" badge sits on each setting. The line "Last changed by Kitchen, 3 minutes ago" replaces guessing. Editing a hive item on any speaker propagates; "Keep mine on this speaker" pins a value locally.
- There is **no hub page**. The member list links to each other member's own portal address so the owner can open a speaker to change its per-speaker items (voice, wake sensitivity, lights, volume). Each speaker has its own sign-in session; the password is the same.
- **Add a speaker**: a button that opens the five-minute window and shows the code; the new speaker appears in a list once it advertises. **Remove a speaker**: a button with a confirmation that spells out the key-rotation step. Items that arrived and lower a safety setting are shown in a "to review" list.
- If a speaker is offline, its page is simply unreachable; everything shared is editable elsewhere. A later, optional step lets one portal read another speaker's logs and status through the hive link (read-through, not replication) so the owner does not hunt for addresses.

## 10. Milestones

Every exit test uses the existing test layers (unit, the e2e crate, `scripts/check.sh`) plus the stated manual check. Milestones are small and ordered; H0 and H1 are each useful without the rest.

| # | Milestone | Scope | Exit test |
|---|---|---|---|
| **H0** | **Be found**: mDNS/DNS-SD announce of the portal (`clawdot.local`, `_clawdot._tcp`), and a device identity keypair with the device id shown in the portal | closes REQ-CFG-009; decides responder versus Avahi on the Dot; no hive behaviour yet | (a) a unit test that the service records and TXT contents are exactly the specified set (no name or address in TXT); (b) two processes on a LAN find each other by the service name and the portal opens at `clawdot.local`; (c) on the adb-root Dot the announcement works beside `avahi-daemon` without two responders on 5353; (d) idle network and memory deltas recorded against REQ-NFR-109 and REQ-NFR-101 |
| **H1** | **Two speakers, one memory**: start a hive, pair a second speaker with the code, replicate **memories and skills**, remove a speaker | roster, signed records, HLC, delta sync over the TLS link, the Hive tab (members, add, remove), conflict rules for notes and skills, caps with over-the-limit handling, tombstones with the 30-day horizon, replica catch-up. **No secrets, no settings yet** | (a) the randomised convergence test: 3 to 5 replicas, 10,000 random note and skill operations with random partitions, duplicates, reordering and clock jumps of ±1 day, all replicas end identical, caps honoured, no deleted record resurrected within the horizon; (b) an e2e test over real sockets: pair with the code, add a note on A while B is off, start B, B has it in under 30 s; delete on B while A is off; concurrent skill edit and toggle both survive; (c) a wrong code three times closes the window; a removed speaker's handshake is refused; (d) a deliberately corrupt or oversized record is rejected without a panic (fuzz the parser); (e) measured on two real devices or a Dot plus a PC: RSS delta ≤ 3 MB, store ≤ 256 KB, idle network ≤ 200 B/s; (f) the single-speaker default build is unchanged in size and RSS (feature off) |
| **H2** | **One household setting**: settings with hive scope (zone, language, model, persona, quiet hours, ...), the hive settings layer and "Keep mine", HTTP tool and MCP definitions and their tiers with the safety review list, the shared admin password hash, the shared provider key (opt-in at pairing) and removal with election-key rotation and the provider-key warning | `Source::Hive`; scope flag on each field of `FIELDS`; the secrets path | (a) a setting changed on one speaker takes effect on the other within 30 s and shows "Hive" and the author; (b) a flag still beats a hive value; (c) the provider key never appears in a settings export, diagnostics or the roster; (d) a removed speaker is refused and told to wipe; (e) a change that lowers a risk tier appears under "to review" on the receiver and does not weaken its confirmation behaviour (test against the action-safety suite); (f) sessions: changing the password on one speaker ends sessions on all |
| **H3** | **One spend cap**: per-device counters, the share rule, the portal showing hive spend today and "about" in a partition | `usage.rs` gains a hive input | (a) a simulation: 5 speakers, random partitions, each spending to its limit: total never exceeds `cap + N × (one staleness window + one turn)`, and equals exactly the cap when connected; (b) with all peers stale a speaker stops at `cap / N`; (c) a speaker with an untrusted clock uses its share only |
| **H4** | **Who answers**: the election window | the wake-score exposure in the wake runtime, the MAC datagram, per-speaker offset, the fail-open rule | (a) a simulated-microphone test with two speakers hearing the same recorded wake word at different levels: exactly one answers, the nearer, in under the window; (b) with the peer unplugged or its datagrams dropped the speaker answers alone with no extra delay beyond the window; (c) a forged datagram (no or wrong MAC) is ignored; (d) a manual room test with two real speakers and 20 utterances from several positions, recorded with the failure counts (wrong speaker, both, neither). The note does not predict the rate; the test measures it |
| **H5** | **Rules, inbox endpoints, and read-through** | shared rules with rendezvous firing, shared endpoint definitions, read-through of status and logs from another speaker | (a) a rule fires once when the hive is connected and at most once per speaker partition; (b) an endpoint token works on any member; (c) a peer's logs open from the Hive tab and respect the same redaction rules |

Spec work to do alongside H1: new area code `HIV` in the spec index (see 13), an ADR for the sync design and its rejected alternatives, and an amendment to REQ-CFG-009.

## 11. Risks

| Risk | Why it matters | Mitigation |
|---|---|---|
| We own a sync algorithm | Subtle bugs lose or duplicate data | Keep the data model to LWW registers and grow-only counters; the randomised convergence test is a required gate; a read-only "state digest" in the portal shows when members disagree |
| A compromised or stolen member has everything shared | Section 11.1 below; it is the cost of "no central server" | Shared secrets are opt-in; removal plus provider-key rotation; skills and tool changes that lower safety are marked for review; the host-held confirmation machinery runs on every device |
| Prompt-injection persistence through skills | A malicious skill text replicated everywhere | Skills arrive from a signed member only; imports arrive turned off; a skill's tool list narrows, never widens, the tools in its turn (REQ-SEC-013) |
| Clock failure | The Dot restores a clock hours wrong | HLC with a persisted high-water mark; quarantine of far-future stamps; share rule limits the spend of a clock-untrusted speaker |
| mDNS fails on the network | Guest networks, isolated Wi-Fi, mDNS-blocking APs | Manual address entry, last-known addresses gossiped, clear portal message; no silent failure |
| The Dot cannot afford TLS links to 7 peers | Memory and CPU on a shared 512 MB box | Cap members at 8 on small devices; links open lazily; measure at H1; the Dot can run with fewer links and relay |
| Portal over plain HTTP | The password guards all speakers and the shared key | Promote the parked HTTPS option for the portal (future scope) at or before H2 |
| Election misfires (two answer, or none) | The most visible failure to the person | Fail open; tunable window and offset; a device that cannot answer abstains; a hardware test before H4 ships |
| Wake runtime does not expose a usable score | Blocks H4 | H4 begins with a spike on the engine's outputs (spec 0007) |
| Update skew across speakers | Updates are per device by confirmation | Unknown-field relay; protocol major check; "needs update" shown |
| Scope creep into multi-room audio | Not planned (future scope) | This note stops at shared state and who-answers; audio sync stays parked |
| Privacy | A memory spoken in one room appears in another | Question 3; per-note scope is an extension if wanted |
| Personal data in the repository | AGENTS.md and the personal-data guard | Examples use generic names ("kitchen", "bedroom"); no addresses or ids from a real home |

### 11.1 The blast radius of one compromised speaker, in one table

| A compromised speaker can | A compromised speaker cannot |
|---|---|
| read every shared memory, skill, tool definition, setting and any shared secret, including the provider key and the admin password hash (the hash is guessable offline at 100,000 PBKDF2 iterations if the password is weak; consider raising the iteration count for new hashes and for the shared case) | read another speaker's per-speaker data, its identity key, its sessions, its audit log, or any audio (audio is never shared; election datagrams carry only a score) |
| spend the provider key until the key is rotated, bounded by the cap only while it respects the cap | forge a record in another member's name (records are signed by their origin) |
| write shared records that change what every speaker says and does, and plant a skill | make another speaker skip its own confirmation prompts, which are host-held per speaker |
| silence or confuse the election (denial of service) | pair a new speaker without the owner signed in on a member and a pairing window opened by hand (the roster record is signed by the admitting member, so a rogue admission is visible and attributable, and removable) |
| keep a copy of all of it after removal | undo its own removal |

The honest conclusion: **a hive is one trust domain.** Treat every speaker as holding the household's keys. This is the same position as a household sharing one router password. Do not put a speaker you do not control (a guest's, a rented house's) into the hive.

## 12. Questions for the maintainer

1. **What is a member?** Is a hive member always a full ClawDot core (a Dot, a Pi or a PC running the all-in-one build), with satellites and ESPHome speakers hanging off one core staying that core's business? I assumed yes.
2. **Which data to share first.** Is "memories and skills, no secrets, no settings" the right size for the first release (H1)?
3. **Privacy of memories.** Should a memory spoken in one room be known in all rooms by default? Or per-note scope ("this room only") from the start? I assumed shared.
4. **Provider key.** Share by default at pairing with one question, or never shared and pasted on each speaker? I proposed an explicit tick, defaulting to ask once.
5. **Admin password.** One shared password for the hive (my default), or one per speaker? A shared one means a stolen speaker's hash lets someone guess it offline.
6. **Lowering safety remotely.** When a change from another speaker lowers a safety setting, is "apply and mark for review" enough, or should it wait for the owner to accept it on the receiving speaker?
7. **Timers, alarms, reminders.** Per speaker (my default) or "ring wherever the person is"? The second needs presence and is a different feature.
8. **Wake score.** Is it acceptable to add a wake-score output (margin over the room's noise) to the wake word runtime, and to measure election behaviour on real speakers before H4 ships?
9. **HTTPS for the portal.** Promote the parked HTTPS option so it ships before shared secrets (H2)?
10. **Dependencies.** Accept `spake2` (or the fallback of comparing a short code on two pages), a certificate wrapper for TLS pinning (or Noise on `ring`), and `mdns-sd` (or a hand-written responder or the Dot's Avahi)? Each must be sized by building it for armv7; I could not measure them from here.
11. **Dot mDNS.** On the Dot the stock Avahi already holds port 5353. Is it acceptable to register ClawDot's services with it (when running beside the stock stack) and run its own responder only elsewhere?
12. **Member limit.** Is 8 members enough for a household (TECHO5 allows 16 in a link)?
13. **Tombstone horizon.** Is 30 days for "away too long to merge silently" right?
14. **Hive across sites.** Out of scope (a holiday house, a second home)? I assumed yes, apart from entering an address by hand if a private network already joins the sites.
15. **Name.** "Hive" is the working name in the portal and docs. Keep it?

## 13. Proposed requirements (not applied)

Spec 0001 is at version 0.40; the repository convention is that new requirements enter the spec in a spec-change branch with new `REQ-` ids, a version bump and a changelog row ([specs/README.md](../specs/README.md), SDLC section 5). This note leaves them as a proposal. When the maintainer accepts the direction, add a new area code `HIV` to the table in `specs/README.md`, a new section in spec 0001 (or a new spec 0008 if it grows), and these as a starting set:

| ID (proposed) | Requirement | Pri | Ver |
|---|---|---|---|
| REQ-HIV-001 | The hive code MUST be an optional feature that is off in the default build and opens no listener and sends no packet until the speaker is in a hive (REQ-NFR-113). | Must | T |
| REQ-HIV-002 | A speaker MUST have a persistent Ed25519 identity; its device id MUST be the hash of the public key. | Must | T |
| REQ-HIV-003 | A speaker MUST join a hive only through a pairing window opened by an administrator on an existing member, valid at most five minutes, one pairing at a time, with a code of at least eight digits checked by a password-authenticated key exchange (or an equivalent human-compared check), and MUST close the window after three failed attempts. | Must | T |
| REQ-HIV-004 | Links between members MUST be mutually authenticated and encrypted, accepting only keys in the signed roster; any other peer MUST be dropped before it can read or write. | Must | T |
| REQ-HIV-005 | Every shared record MUST be signed by its origin and verified on receipt; an invalid or oversized record MUST be rejected without affecting the store. | Must | T |
| REQ-HIV-006 | Replicas MUST converge: for any set of records delivered in any order, with duplicates, to any member, all members MUST end with identical shared state. | Must | T |
| REQ-HIV-007 | Records MUST be ordered by a hybrid logical clock whose high-water mark is persisted; a record stamped more than 24 hours ahead of the best trusted time MUST be quarantined, not applied. | Must | T |
| REQ-HIV-008 | A deleted record MUST NOT reappear within the tombstone horizon; a member away for longer MUST catch up as a replica and MUST NOT silently reapply its own shared changes. | Must | T |
| REQ-HIV-009 | A member that is unreachable MUST NOT delay, block or fail any operation on another member, and each speaker MUST keep working from its last copy when alone. | Must | T |
| REQ-HIV-010 | Per-speaker items (name, voice, hardware, wake settings, light settings, audit log, sessions, identity key) MUST NOT be shared. A secret MUST be shared only on the administrator's explicit choice and MUST NOT appear in an export, diagnostics or the roster. | Must | T |
| REQ-HIV-011 | A removal MUST override admission, MUST take effect on each member as it learns of it, and the portal MUST tell the administrator to rotate any secret the removed speaker held. | Must | T |
| REQ-HIV-012 | The merged notes and skills MUST respect the existing caps; records beyond a cap MUST be kept and listed, not dropped. | Must | T |
| REQ-HIV-013 | The hive spend total MUST be the sum of per-speaker counters; a speaker that cannot see all members MUST limit itself to its share of the cap. | Must | T |
| REQ-HIV-014 | The election MUST fail open: with no timely message from another member, a speaker MUST answer as if no hive existed. A speaker that cannot answer MUST NOT take part. | Must | T |
| REQ-HIV-015 | The hive MUST stay within its stated bounds: store ≤ 256 KB, at most 8 members by default, idle network ≤ 200 B/s, RSS delta ≤ 3 MB, measured and gated at H1. | Must | T |

## 14. Sources, versions, confidence

Fetched 2026-11-11 unless noted. "Primary" means the project's own documentation, specification or source repository. **I could not read two PDFs** (the SWIM paper and Lamport's "Time, Clocks, and the Ordering of Events" came back as undecodable binary), so statements about SWIM come from the memberlist README and the chitchat README, and statements about Lamport clocks are from general knowledge, not from the file.

| Claim | Source | Confidence |
|---|---|---|
| Local-first ideals (seven), authors, April 2019 | <https://www.inkandswitch.com/essay/local-first/> | high (read) |
| Syncthing BEP v1: device id from the certificate hash, TLS 1.3, index and index-update by `(index id, max sequence)`, version vectors | <https://docs.syncthing.net/specs/bep-v1.html> | high for stated points; the page does not describe conflict handling in detail |
| Syncthing device id: SHA-256 of the DER certificate, base32, grouped with check digits | <https://docs.syncthing.net/dev/device-ids.html> | high (check-digit algorithm not given on the page) |
| Syncthing local discovery: UDP 21027, broadcast/multicast, 30 to 60 s, magic number, device id, addresses, instance id | <https://docs.syncthing.net/specs/localdisco-v4.html> | high |
| memberlist is SWIM plus extensions plus Lifeguard, eventual consistency, partial partition tolerance | <https://github.com/hashicorp/memberlist> (README) | high for the README; version not stated there |
| chitchat: scuttlebutt anti-entropy, per-node namespace, phi-accrual detector, tombstones with a recommended 15-minute grace, reset of stale nodes, resurrection caveat, MIT; crate 0.14.0 dated 2026-11-05 | <https://github.com/quickwit-oss/chitchat>, crates.io API | high |
| foca 2.0.0 (SWIM), memberlist crate 0.8.5, automerge 0.12.0, loro 1.17.1, yrs 0.28.1, snow 0.10.0, spake2 0.4.0, mdns-sd 0.21.7, ed25519-dalek 3.0.0, ring 0.17.14 | crates.io API (versions and dates only) | high for the numbers; sizes on armv7 **not measured** |
| Automerge: CRDTs, compact format, sync protocol, Rust core with WASM/JS/C bindings, Rust API low level, Automerge 3 reported ~10x memory reduction | <https://github.com/automerge/automerge> README | medium (the memory claim is the README's, not checked) |
| cr-sqlite: SQLite loadable extension, per-column CRDTs (LWW, counter, fractional index), causal-length-set tables, MIT, "main may not be stable", no release version visible on the page | <https://github.com/vlcn-io/cr-sqlite> | medium |
| RFC 6762 mDNS rules (probing, announcements, TTLs, goodbye, known-answer suppression, rate limits) | <https://www.rfc-editor.org/rfc/rfc6762> (first half read; remainder not read) | high for the cited rules |
| `mdns-sd` crate: client and responder, no async runtime, own thread, Apache-2.0 or MIT, "beta", unicast responses not implemented, 250 ms probe spacing not implemented | <https://github.com/keepsimple1/mdns-sd> | high |
| SPAKE2 (RFC 9382): PAKE, key confirmation MACs, security reduction to Gap DH, no augmentation | <https://www.rfc-editor.org/rfc/rfc9382> | high; the offline-guess property is **my inference** from the standard PAKE goal, the page did not state it |
| Matter: PASE with a passcode for commissioning, NOC and root certificate per fabric, CASE for operation, multi-admin by reopening a commissioning window | several secondary sources via search (Silicon Labs, Nordic, Espressif, Google Home developer primer, buildwithmatter handbook); the CSA specification itself was **not** read | medium |
| Home Assistant: `wake_word_phrase` "avoid several devices answering the same wake word"; error `duplicate_wake_up_detected` | <https://contributors.home-assistant.io/core/internals/assist-pipelines/> | high for the two quoted lines; the tie-break rule is **community-reported only** ("first to stream wins") from forum threads and not documented |
| Home Assistant has no native peer-to-peer multi-instance sync | **not verified**: I found no counter-evidence but did not read its whole documentation | low |
| Amazon Echo Spatial Perception: nearest device answers, based on wake-word level/proximity; later moved to the cloud; fails for some users | search summaries (VentureBeat, Digital Trends, forum threads); Amazon's own specification of the signalling not found | low to medium |
| Sonos and Google Nest multi-device behaviour | forum threads only; **no vendor documentation** was found, so nothing in the design depends on them | low |
| TECHO5 nearest device answers, house word, window, offset, fail-open; settings sharing without a main device; never-shared list; limit of 16 | `docs/setup.md` in <https://github.com/HuskerMinion/techo5> via the GitHub API, repository head `31ab368` (2026-11-11); the Dot image repo `HuskerMinion/techo5-dot` is MIT per its README | high (read directly). This is the "other Echo project" in the brief |
| Hybrid logical clocks | Kulkarni, Demirbas, Madappa, Avva, Leone, "Logical Physical Clocks", OPODIS 2014, LNCS 8878, DOI 10.1007/978-3-319-14472-6_2 (metadata and summaries; the PDF's bounded-drift theorem was **not** read) | medium |
| Echo Dot Gen 2 clock behaviour (RTC 2010, restored clock about 3.5 h behind) | this repository, [R-07](07-echo-dot-gen2-hardware.md) section 7 | high (hands-on findings of the project) |
| ClawDot does not announce by mDNS today | this repository: no mDNS code in `crates`, `docs/roadmap.md` A6 "mDNS: not started" | high |

### Not verified

- The size and memory cost of any new dependency on armv7. All figures in 8.3 are estimates.
- Whether rustls can be driven with a hand-built Ed25519 self-signed certificate without adding a certificate-generation crate.
- Whether the Dot's Avahi can host a second service registration from ClawDot.
- Whether Wi-Fi power-save on the Dot delays UDP datagrams enough to break a 300 ms window.
- Whether common providers offer a per-key spend limit usable as a hard stop (I believe some do; I did not check).
- Home Assistant's, Sonos's and Google's exact arbitration algorithms.
- Matter specification text (the cited points are from secondary sources).

## 15. Spec impact and confidence

Spec impact (proposals, not applied): a new area `HIV` and the requirements in section 13; REQ-CFG-009 moves from "SHOULD" to a scheduled milestone (H0); the settings `FIELDS` table gains a scope attribute and `Source` gains a hive value (H2); notes need globally unique ids (H1) while keeping the displayed number local; `usage.rs` gains a hive input (H3); the wake word runtime gains a score output (H4); `future-scope.md` rows for multi-room and where-to-speak are clarified (applied with this note); a roadmap item 20 is added (applied with this note).

Confidence: **medium-high** on the architecture (each element has a documented precedent), **medium** on the cost estimates (unmeasured), **low** on election quality in real rooms (needs H4 measurement).
