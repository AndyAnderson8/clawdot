//! Varint, field and value helpers shared by the encoder and the decoder.

use super::{Error, MAX_TEXT_CHARS, MAX_VARINT_BYTES};

/// Read a varint. `Ok(None)` means the input ended before the varint did (more bytes may complete it).
///
/// # Errors
/// [`Error::VarintTooLong`] when it runs past ten bytes.
pub fn read_varint(b: &[u8]) -> Result<Option<(u64, usize)>, Error> {
    let mut v: u64 = 0;
    for (i, &byte) in b.iter().enumerate() {
        if i >= MAX_VARINT_BYTES {
            return Err(Error::VarintTooLong);
        }
        v |= u64::from(byte & 0x7f)
            .checked_shl(7 * u32::try_from(i).unwrap_or(63))
            .unwrap_or(0);
        if byte & 0x80 == 0 {
            return Ok(Some((v, i + 1)));
        }
    }
    if b.len() >= MAX_VARINT_BYTES {
        return Err(Error::VarintTooLong);
    }
    Ok(None)
}

/// Append a varint.
pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// One decoded field value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Value<'a> {
    Varint(u64),
    Fixed32(u32),
    Bytes(&'a [u8]),
    /// A fixed 64-bit value; no message here uses one, so it is read and dropped.
    Other,
}

/// Iterator over the fields of one message.
pub(super) struct Fields<'a> {
    pub(super) b: &'a [u8],
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<(u32, Value<'a>), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.b.is_empty() {
            return None;
        }
        let r = self.read();
        if r.is_err() {
            self.b = &[];
        }
        Some(r)
    }
}

impl<'a> Fields<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.b.len() {
            return Err(Error::Truncated);
        }
        let (head, tail) = self.b.split_at(n);
        self.b = tail;
        Ok(head)
    }

    fn varint(&mut self) -> Result<u64, Error> {
        match read_varint(self.b)? {
            Some((v, n)) => {
                self.b = &self.b[n..];
                Ok(v)
            }
            None => Err(Error::Truncated),
        }
    }

    fn read(&mut self) -> Result<(u32, Value<'a>), Error> {
        let tag = self.varint()?;
        let field = u32::try_from(tag >> 3).map_err(|_| Error::FieldNumber)?;
        if field == 0 {
            return Err(Error::FieldNumber);
        }
        let value = match tag & 7 {
            0 => Value::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Value::Other
            }
            2 => {
                let len = usize::try_from(self.varint()?).map_err(|_| Error::Truncated)?;
                Value::Bytes(self.take(len)?)
            }
            5 => {
                let raw = self.take(4)?;
                Value::Fixed32(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
            }
            w => return Err(Error::WireType(w as u8)),
        };
        Ok((field, value))
    }
}

pub(super) fn text(b: &[u8]) -> Result<String, Error> {
    let s = std::str::from_utf8(b).map_err(|_| Error::Utf8)?;
    Ok(s.chars().take(MAX_TEXT_CHARS).collect())
}

pub(super) fn as_u32(v: Value<'_>) -> Result<u32, Error> {
    match v {
        // A wider varint is truncated to 32 bits, as protobuf does for uint32 fields.
        Value::Varint(n) => Ok(n as u32),
        _ => Err(Error::Mismatch),
    }
}

pub(super) fn as_bool(v: Value<'_>) -> Result<bool, Error> {
    match v {
        Value::Varint(n) => Ok(n != 0),
        _ => Err(Error::Mismatch),
    }
}

pub(super) fn as_text(v: Value<'_>) -> Result<String, Error> {
    match v {
        Value::Bytes(b) => text(b),
        _ => Err(Error::Mismatch),
    }
}

pub(super) fn as_fixed32(v: Value<'_>) -> Result<u32, Error> {
    match v {
        Value::Fixed32(n) => Ok(n),
        _ => Err(Error::Mismatch),
    }
}

pub(super) fn put_tag(out: &mut Vec<u8>, field: u32, wire: u8) {
    put_varint(out, (u64::from(field) << 3) | u64::from(wire));
}

pub(super) fn put_uint(out: &mut Vec<u8>, field: u32, v: u32) {
    if v != 0 {
        put_tag(out, field, 0);
        put_varint(out, u64::from(v));
    }
}

pub(super) fn put_bool(out: &mut Vec<u8>, field: u32, v: bool) {
    if v {
        put_tag(out, field, 0);
        out.push(1);
    }
}

pub(super) fn put_bytes(out: &mut Vec<u8>, field: u32, v: &[u8]) {
    if !v.is_empty() {
        put_tag(out, field, 2);
        put_varint(out, v.len() as u64);
        out.extend_from_slice(v);
    }
}

pub(super) fn put_fixed32(out: &mut Vec<u8>, field: u32, v: u32) {
    put_tag(out, field, 5);
    out.extend_from_slice(&v.to_le_bytes());
}

/// Text that must not be clipped (URLs): same checks, no character limit beyond the frame.
pub(super) fn as_text_full(v: Value<'_>) -> Result<String, Error> {
    match v {
        Value::Bytes(b) => std::str::from_utf8(b)
            .map(str::to_owned)
            .map_err(|_| Error::Utf8),
        _ => Err(Error::Mismatch),
    }
}
