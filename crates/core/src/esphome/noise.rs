//! Noise encryption for the ESPHome native API (spec 0004, REQ-ESP-014), feature `esphome-noise`.
//!
//! The protocol is `Noise_NNpsk0_25519_ChaChaPoly_SHA256` with the client as the initiator and the prologue
//! `NoiseAPIInit\0\0`, written from the Noise specification (revision 34) on `ring` primitives (X25519,
//! ChaCha20-Poly1305, SHA-256, HMAC). There is no `snow` crate. The two-message handshake is:
//!
//! ```text
//! -> psk, e            (mix the pre-shared key, send our ephemeral key, empty encrypted payload)
//! <- e, ee             (the device's ephemeral key, empty encrypted payload)
//! ```
//!
//! ESPHome wraps it in frames: marker `0x01` and a 16-bit big-endian length. The client first sends an empty
//! hello frame, the device answers with a hello frame (`0x01`, its name, `0x00`, its MAC address, `0x00`), then
//! the handshake messages follow, each prefixed with `0x00` (success) or `0x01` plus an error text.

use super::frame::Link;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::agreement::{EphemeralPrivateKey, UnparsedPublicKey, X25519, agree_ephemeral};
use ring::digest::{SHA256, digest};
use ring::hmac;
use ring::rand::SystemRandom;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::{Zeroize, Zeroizing};

const PROTOCOL_NAME: &[u8] = b"Noise_NNpsk0_25519_ChaChaPoly_SHA256";
const PROLOGUE: &[u8] = b"NoiseAPIInit\x00\x00";
const TAG: usize = 16;
const KEY_LEN: usize = 32;
/// The device's ephemeral key plus the tag of the empty payload.
const MSG2_LEN: usize = KEY_LEN + TAG;
/// A frame carries a 16-bit length.
const MAX_RECORD: usize = 65_535;

/// Decode a base64 (standard alphabet) Noise key into its 32 bytes.
///
/// # Errors
/// A message when the text is not 32 bytes of base64.
pub fn decode_psk(text: &str) -> Result<[u8; 32], String> {
    // The decoded key passes through this buffer, so it is wiped when the function returns.
    let mut out = Zeroizing::new(Vec::with_capacity(33));
    let (mut acc, mut bits) = (0u32, 0u32);
    let body = text.trim_end_matches('=');
    if text.len() - body.len() > 2 {
        return Err("the Noise key is not valid base64".into());
    }
    for c in body.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err("the Noise key is not valid base64".into()),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    <[u8; 32]>::try_from(out.as_slice())
        .map_err(|_| "the Noise key must decode to exactly 32 bytes".to_owned())
}

fn hmac256(key: &[u8; 32], parts: &[&[u8]]) -> Secret {
    let k = hmac::Key::new(hmac::HMAC_SHA256, key);
    let mut ctx = hmac::Context::with_key(&k);
    for p in parts {
        ctx.update(p);
    }
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(ctx.sign().as_ref());
    out
}

/// A 32 byte secret that is wiped when it goes out of scope.
type Secret = Zeroizing<[u8; 32]>;

/// Noise's HKDF with three outputs (callers use the first two or three).
fn hkdf(ck: &[u8; 32], ikm: &[u8]) -> (Secret, Secret, Secret) {
    let temp = hmac256(ck, &[ikm]);
    let o1 = hmac256(&temp, &[&[1]]);
    let o2 = hmac256(&temp, &[&o1[..], &[2]]);
    let o3 = hmac256(&temp, &[&o2[..], &[3]]);
    (o1, o2, o3)
}

fn nonce(n: u64) -> Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_le_bytes());
    Nonce::assume_unique_for_key(b)
}

fn seal(key: &[u8; 32], n: u64, ad: &[u8], pt: &[u8]) -> Result<Vec<u8>, String> {
    let k = UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| "cipher setup failed")?;
    let mut buf = pt.to_vec();
    LessSafeKey::new(k)
        .seal_in_place_append_tag(nonce(n), Aad::from(ad), &mut buf)
        .map_err(|_| "encryption failed")?;
    Ok(buf)
}

fn open(key: &[u8; 32], n: u64, ad: &[u8], ct: &[u8]) -> Result<Vec<u8>, String> {
    let k = UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| "cipher setup failed")?;
    let mut buf = ct.to_vec();
    let len = LessSafeKey::new(k)
        .open_in_place(nonce(n), Aad::from(ad), &mut buf)
        .map_err(|_| "decryption failed")?
        .len();
    buf.truncate(len);
    Ok(buf)
}

/// The handshake's running state (the Noise `SymmetricState`).
struct Symmetric {
    h: [u8; 32],
    ck: [u8; 32],
    k: Option<[u8; 32]>,
    n: u64,
}

impl Drop for Symmetric {
    fn drop(&mut self) {
        self.wipe();
    }
}

impl Symmetric {
    /// Overwrite the hash, the chaining key and the cipher key with zeros.
    fn wipe(&mut self) {
        self.h.zeroize();
        self.ck.zeroize();
        self.k.zeroize();
    }

    fn new() -> Self {
        let mut h = Zeroizing::new([0u8; 32]);
        // The protocol name is longer than 32 bytes, so it is hashed.
        h.copy_from_slice(digest(&SHA256, PROTOCOL_NAME).as_ref());
        let mut s = Self {
            h: *h,
            ck: *h,
            k: None,
            n: 0,
        };
        s.mix_hash(PROLOGUE);
        s
    }

    fn mix_hash(&mut self, data: &[u8]) {
        let mut ctx = ring::digest::Context::new(&SHA256);
        ctx.update(&self.h);
        ctx.update(data);
        self.h.copy_from_slice(ctx.finish().as_ref());
    }

    fn mix_key(&mut self, ikm: &[u8]) {
        let (ck, k, _) = hkdf(&self.ck, ikm);
        self.ck = *ck;
        self.k = Some(*k);
        self.n = 0;
    }

    fn mix_key_and_psk(&mut self, psk: &[u8; 32]) {
        let (ck, temp_h, k) = hkdf(&self.ck, psk);
        self.ck = *ck;
        self.mix_hash(&*temp_h);
        self.k = Some(*k);
        self.n = 0;
    }

    fn encrypt_and_hash(&mut self, pt: &[u8]) -> Result<Vec<u8>, String> {
        let Some(k) = self.k.as_ref() else {
            return Err("handshake out of order".into());
        };
        let ct = seal(k, self.n, &self.h, pt)?;
        self.n += 1;
        self.mix_hash(&ct);
        Ok(ct)
    }

    fn decrypt_and_hash(&mut self, ct: &[u8]) -> Result<Vec<u8>, String> {
        let Some(k) = self.k.as_ref() else {
            return Err("handshake out of order".into());
        };
        let pt = open(k, self.n, &self.h, ct)?;
        self.n += 1;
        self.mix_hash(ct);
        Ok(pt)
    }

    /// The two transport keys: first for initiator-to-responder, second for the other direction.
    fn split(&self) -> ([u8; 32], [u8; 32]) {
        let (a, b, _) = hkdf(&self.ck, &[]);
        (*a, *b)
    }
}

fn new_ephemeral() -> Result<(EphemeralPrivateKey, [u8; 32]), String> {
    let rng = SystemRandom::new();
    let key = EphemeralPrivateKey::generate(&X25519, &rng)
        .map_err(|_| "cannot make a Noise key".to_owned())?;
    let public = key
        .compute_public_key()
        .map_err(|_| "cannot make a Noise key".to_owned())?;
    let mut out = [0u8; 32];
    out.copy_from_slice(public.as_ref());
    Ok((key, out))
}

fn dh(private: EphemeralPrivateKey, peer: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    agree_ephemeral(private, &UnparsedPublicKey::new(&X25519, peer), |s| {
        Zeroizing::new(s.to_vec())
    })
    .map_err(|_| "the device sent an unusable Noise key".to_owned())
}

/// The keys of an established connection.
pub struct Transport {
    send_key: [u8; 32],
    recv_key: [u8; 32],
    send_n: u64,
    recv_n: u64,
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.wipe();
    }
}

impl Transport {
    /// Overwrite both keys with zeros.
    fn wipe(&mut self) {
        self.send_key.zeroize();
        self.recv_key.zeroize();
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keys are secrets.
        f.write_str("Transport")
    }
}

impl Transport {
    /// Encrypt one message into a complete frame.
    ///
    /// # Errors
    /// A message when the payload is too large for one frame.
    pub fn seal(&mut self, ty: u32, payload: &[u8]) -> Result<Vec<u8>, String> {
        let ty = u16::try_from(ty).map_err(|_| "message type too large".to_owned())?;
        let len = u16::try_from(payload.len()).map_err(|_| "message too large".to_owned())?;
        let mut plain = Vec::with_capacity(payload.len() + 4);
        plain.extend_from_slice(&ty.to_be_bytes());
        plain.extend_from_slice(&len.to_be_bytes());
        plain.extend_from_slice(payload);
        let ct = seal(&self.send_key, self.send_n, &[], &plain)?;
        self.send_n += 1;
        frame(&ct)
    }

    /// Decrypt one record.
    ///
    /// # Errors
    /// A message when authentication fails or the plaintext is too short.
    pub fn open(&mut self, record: &[u8]) -> Result<(u32, Vec<u8>), String> {
        let plain = open(&self.recv_key, self.recv_n, &[], record)
            .map_err(|_| "encryption error: a record failed authentication".to_owned())?;
        self.recv_n += 1;
        if plain.len() < 4 {
            return Err("decrypted message too short".into());
        }
        let ty = u32::from(u16::from_be_bytes([plain[0], plain[1]]));
        // The declared length (bytes 2 and 3) is not trusted; the record's own length is.
        Ok((ty, plain[4..].to_vec()))
    }
}

/// `0x01`, a 16-bit length, then `body`.
fn frame(body: &[u8]) -> Result<Vec<u8>, String> {
    let len = u16::try_from(body.len()).map_err(|_| "record too large".to_owned())?;
    let mut out = Vec::with_capacity(body.len() + 3);
    out.push(1);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(body);
    Ok(out)
}

/// Take one encrypted-mode record body from the front of `buf`, if a whole one has arrived.
///
/// # Errors
/// A message when the marker byte is not `0x01`.
pub fn take_record(buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>, String> {
    match buf.first() {
        None => return Ok(None),
        Some(1) => {}
        Some(0) => {
            return Err("the device is using the plaintext protocol; remove `psk_env`".into());
        }
        Some(b) => return Err(format!("bad frame marker {b:#04x}")),
    }
    if buf.len() < 3 {
        return Ok(None);
    }
    let len = usize::from(u16::from_be_bytes([buf[1], buf[2]]));
    debug_assert!(len <= MAX_RECORD);
    if buf.len() < 3 + len {
        return Ok(None);
    }
    let body = buf[3..3 + len].to_vec();
    buf.drain(..3 + len);
    Ok(Some(body))
}

async fn read_record<IO: AsyncRead + Unpin>(
    io: &mut IO,
    rx: &mut Vec<u8>,
) -> Result<Vec<u8>, String> {
    loop {
        if let Some(r) = take_record(rx)? {
            return Ok(r);
        }
        let mut tmp = [0u8; 512];
        let n = io.read(&mut tmp).await.map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Err("the device closed the connection during the Noise handshake".into());
        }
        rx.extend_from_slice(&tmp[..n]);
    }
}

/// The handshake message in the device's reply, or why the device refused: a status byte of 0 is followed by
/// the message; anything else is followed by the device's reason.
fn reply_body(reply: &[u8]) -> Result<&[u8], String> {
    match reply.first() {
        Some(0) => {}
        Some(_) => {
            let why = String::from_utf8_lossy(&reply[1..]);
            return Err(if why == "Handshake MAC failure" {
                "the device rejected the Noise key (wrong `psk_env` value)".to_owned()
            } else {
                "the device refused the Noise handshake".to_owned()
            });
        }
        None => return Err("empty Noise handshake reply".into()),
    }
    let msg2 = &reply[1..];
    if msg2.len() != MSG2_LEN {
        return Err("the Noise handshake reply has the wrong size".into());
    }
    Ok(msg2)
}

/// Run the initiator handshake on `io` and return the encrypted link.
///
/// # Errors
/// A message for a wrong key, a device that is not using Noise, or any malformed handshake.
pub async fn connect<IO>(mut io: IO, psk: &[u8; 32]) -> Result<Link<IO>, String>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let mut st = Symmetric::new();
    // -> psk, e
    st.mix_key_and_psk(psk);
    let (eph, e_pub) = new_ephemeral()?;
    st.mix_hash(&e_pub);
    st.mix_key(&e_pub);
    let payload = st.encrypt_and_hash(&[])?;

    let mut msg1 = vec![0u8];
    msg1.extend_from_slice(&e_pub);
    msg1.extend_from_slice(&payload);
    let mut out = vec![1, 0, 0]; // the empty client hello
    out.extend_from_slice(&frame(&msg1)?);
    io.write_all(&out)
        .await
        .map_err(|e| format!("write: {e}"))?;

    // The device's hello: the chosen protocol (1), then its name and MAC address.
    let mut rx = Vec::new();
    let hello = read_record(&mut io, &mut rx).await?;
    if hello.first() != Some(&1) {
        return Err("the device chose an unknown Noise framing".into());
    }
    // <- e, ee
    let reply = read_record(&mut io, &mut rx).await?;
    let msg2 = reply_body(&reply)?;
    let (re, ct) = msg2.split_at(KEY_LEN);
    st.mix_hash(re);
    st.mix_key(re);
    st.mix_key(&dh(eph, re)?);
    let empty = st.decrypt_and_hash(ct).map_err(|_| {
        "the device's Noise reply failed authentication (wrong `psk_env` value?)".to_owned()
    })?;
    if !empty.is_empty() {
        return Err("unexpected data in the Noise handshake".into());
    }
    let (send_key, recv_key) = st.split();
    Ok(Link::noise(
        io,
        rx,
        Transport {
            send_key,
            recv_key,
            send_n: 0,
            recv_n: 0,
        },
    ))
}

/// The device side of the handshake, for tests and the fake device. Not used by the client.
///
/// # Errors
/// A message for any malformed or unauthenticated handshake. A wrong key is also reported to the peer the way
/// ESPHome does, with the text `Handshake MAC failure`.
#[doc(hidden)]
pub async fn accept<IO>(mut io: IO, psk: &[u8; 32], name: &str) -> Result<Link<IO>, String>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let mut rx = Vec::new();
    let hello = read_record(&mut io, &mut rx).await?;
    if !hello.is_empty() {
        return Err("expected an empty client hello".into());
    }
    let mut server_hello = vec![1];
    server_hello.extend_from_slice(name.as_bytes());
    server_hello.push(0);
    io.write_all(&frame(&server_hello)?)
        .await
        .map_err(|e| format!("write: {e}"))?;

    let msg1 = read_record(&mut io, &mut rx).await?;
    if msg1.first() != Some(&0) || msg1.len() != 1 + MSG2_LEN {
        return Err("bad Noise handshake message".into());
    }
    let (re, ct) = msg1[1..].split_at(KEY_LEN);
    let mut st = Symmetric::new();
    st.mix_key_and_psk(psk);
    st.mix_hash(re);
    st.mix_key(re);
    if st.decrypt_and_hash(ct).is_err() {
        let mut body = vec![1];
        body.extend_from_slice(b"Handshake MAC failure");
        let _ = io.write_all(&frame(&body)?).await;
        return Err("Handshake MAC failure".into());
    }
    // -> e, ee
    let (eph, e_pub) = new_ephemeral()?;
    st.mix_hash(&e_pub);
    st.mix_key(&e_pub);
    st.mix_key(&dh(eph, re)?);
    let payload = st.encrypt_and_hash(&[])?;
    let mut body = vec![0];
    body.extend_from_slice(&e_pub);
    body.extend_from_slice(&payload);
    io.write_all(&frame(&body)?)
        .await
        .map_err(|e| format!("write: {e}"))?;
    let (initiator_to_responder, responder_to_initiator) = st.split();
    Ok(Link::noise(
        io,
        rx,
        Transport {
            send_key: responder_to_initiator,
            recv_key: initiator_to_responder,
            send_n: 0,
            recv_n: 0,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::esphome::proto::Msg;

    const KEY: [u8; 32] = [7; 32];

    async fn pair(
        client_key: [u8; 32],
        device_key: [u8; 32],
    ) -> (
        Result<Link<tokio::io::DuplexStream>, String>,
        Result<Link<tokio::io::DuplexStream>, String>,
    ) {
        let (a, b) = tokio::io::duplex(4096);
        tokio::join!(connect(a, &client_key), accept(b, &device_key, "dot"))
    }

    // REQ-ESP-014
    #[tokio::test]
    async fn req_esp_014_the_handshake_and_records_work_both_ways() {
        let (c, d) = pair(KEY, KEY).await;
        let (mut c, mut d) = (c.unwrap(), d.unwrap());
        c.send(&Msg::PingRequest).await.unwrap();
        assert_eq!(d.recv().await.unwrap(), Some(Msg::PingRequest));
        for i in 0..5u8 {
            d.send(&Msg::VoiceAudio {
                data: vec![i; 1000],
                end: false,
            })
            .await
            .unwrap();
            assert!(matches!(
                c.recv().await.unwrap(),
                Some(Msg::VoiceAudio { .. })
            ));
        }
    }

    // REQ-ESP-014: the wipe functions leave no key bytes behind (this tests the functions, not freed memory).
    #[test]
    fn req_esp_014_wiping_clears_every_key_field() {
        let mut t = Transport {
            send_key: KEY,
            recv_key: KEY,
            send_n: 0,
            recv_n: 0,
        };
        t.wipe();
        assert_eq!((t.send_key, t.recv_key), ([0; 32], [0; 32]));
        let mut s = Symmetric::new();
        s.mix_key_and_psk(&KEY);
        assert!(s.h != [0; 32] && s.ck != [0; 32] && s.k.is_some());
        s.wipe();
        assert_eq!((s.h, s.ck), ([0; 32], [0; 32]));
        assert_eq!(s.k, None);
    }

    // REQ-ESP-014
    #[tokio::test]
    async fn req_esp_014_a_wrong_key_is_reported_and_nothing_panics() {
        let (c, d) = pair(KEY, [8; 32]).await;
        let e = c.unwrap_err();
        assert!(e.contains("wrong"), "{e}");
        assert!(d.unwrap_err().contains("MAC"));
    }

    // REQ-ESP-014
    #[test]
    fn req_esp_014_a_tampered_or_replayed_record_is_refused() {
        let mut a = Transport {
            send_key: KEY,
            recv_key: KEY,
            send_n: 0,
            recv_n: 0,
        };
        let mut b = Transport {
            send_key: KEY,
            recv_key: KEY,
            send_n: 0,
            recv_n: 0,
        };
        let f1 = a.seal(7, b"hello").unwrap();
        let f2 = a.seal(7, b"again").unwrap();
        let body = |f: &[u8]| f[3..].to_vec();
        // Out of order (a replayed counter) fails.
        assert!(b.open(&body(&f2)).is_err());
        assert_eq!(b.open(&body(&f1)).unwrap(), (7, b"hello".to_vec()));
        let mut bad = body(&f2);
        bad[2] ^= 1;
        assert!(b.open(&bad).is_err());
        assert!(b.open(&[1, 2, 3]).is_err());
        assert!(b.open(&[]).is_err());
    }

    // REQ-ESP-014: transport nonces are 4 zero bytes then the little-endian counter (Noise spec, ChaChaPoly).
    // The vector is RFC 8439 section 2.8.2, which pins the cipher itself.
    #[test]
    fn the_cipher_matches_rfc_8439() {
        let key: Vec<u8> = (0x80..=0x9f).collect();
        let key: [u8; 32] = key.try_into().unwrap();
        let nonce12 = [
            0x07, 0, 0, 0, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
        ];
        let aad = [
            0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,
        ];
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let k = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &key).unwrap());
        let mut buf = pt.to_vec();
        k.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce12),
            Aad::from(aad),
            &mut buf,
        )
        .unwrap();
        assert_eq!(buf[..8], [0xd3, 0x1a, 0x8d, 0x34, 0x64, 0x8e, 0x60, 0xdb]);
        assert_eq!(
            buf[buf.len() - 16..],
            [
                0x1a, 0xe1, 0x0b, 0x59, 0x4f, 0x09, 0xe2, 0x6a, 0x7e, 0x90, 0x2e, 0xcb, 0xd0, 0x60,
                0x06, 0x91
            ]
        );
        // And our counter layout: counter 1 is 00 00 00 00 01 00 00 00 00 00 00 00.
        let n = nonce(1);
        assert_eq!(n.as_ref(), &[0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn keys_decode_from_base64() {
        // 32 bytes 0x00..0x1f
        let raw: Vec<u8> = (0u8..32).collect();
        let b64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
        assert_eq!(decode_psk(b64).unwrap().to_vec(), raw);
        for bad in [
            "",
            "AAEC",
            "!!!!",
            &b64.replace('=', "==="),
            &format!("{b64}A"),
        ] {
            assert!(decode_psk(bad).is_err(), "{bad}");
        }
    }

    /// Manual interop check against an independent Noise implementation (the `noiseprotocol` Python package
    /// that aioesphomeapi uses). Run with `CLAWDOT_NOISE_PEER=127.0.0.1:PORT CLAWDOT_NOISE_PSK=<base64>` and
    /// `--ignored`; the peer is started by hand (see docs/architecture/esphome-satellite.md).
    #[tokio::test]
    #[ignore = "needs an external peer"]
    async fn interop_with_an_independent_responder() {
        let peer = std::env::var("CLAWDOT_NOISE_PEER").unwrap();
        let psk = decode_psk(&std::env::var("CLAWDOT_NOISE_PSK").unwrap()).unwrap();
        let s = tokio::net::TcpStream::connect(peer).await.unwrap();
        let mut link = connect(s, &psk).await.unwrap();
        link.send(&Msg::PingRequest).await.unwrap();
        assert_eq!(link.recv().await.unwrap(), Some(Msg::PingResponse));
        link.send(&Msg::HelloRequest {
            client_info: "t".into(),
            major: 1,
            minor: 10,
        })
        .await
        .unwrap();
        assert!(matches!(
            link.recv().await.unwrap(),
            Some(Msg::HelloResponse { major: 1, .. })
        ));
    }

    // REQ-ESP-014: random bytes in the handshake never panic.
    #[tokio::test]
    async fn req_esp_014_garbage_handshakes_are_errors() {
        let mut x: u64 = 0xfeed_beef_1234_5678;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..200 {
            let len = (next() % 200) as usize;
            let mut junk: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if next() % 2 == 0 && !junk.is_empty() {
                junk[0] = 1;
            }
            let (a, mut b) = tokio::io::duplex(4096);
            let client = tokio::spawn(async move { connect(a, &KEY).await.map(|_| ()) });
            let _ = b.write_all(&junk).await;
            drop(b);
            assert!(client.await.unwrap().is_err());
        }
    }
}
