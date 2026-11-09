//! A bounds-checked reader for the FlatBuffers wire format, just enough for the TFLite schema.
//!
//! Every access goes through [`Buf`], which checks that the bytes it is asked for are inside the slice. No
//! offset is trusted: a table whose vtable lies outside the buffer, a vector longer than the data left, or an
//! offset that wraps all return [`Error::Malformed`].

use crate::error::{Error, Result};

const BAD: Error = Error::Malformed("offset outside the buffer");

/// The whole buffer.
#[derive(Debug, Clone, Copy)]
pub struct Buf<'a> {
    bytes: &'a [u8],
}

impl<'a> Buf<'a> {
    /// Wrap a slice.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// The bytes.
    #[must_use]
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    fn slice(&self, pos: usize, len: usize) -> Result<&'a [u8]> {
        let end = pos.checked_add(len).ok_or(BAD)?;
        self.bytes.get(pos..end).ok_or(BAD)
    }

    fn u8_at(&self, pos: usize) -> Result<u8> {
        self.bytes.get(pos).copied().ok_or(BAD)
    }

    fn u16_at(&self, pos: usize) -> Result<u16> {
        let s = self.slice(pos, 2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn u32_at(&self, pos: usize) -> Result<u32> {
        let s = self.slice(pos, 4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn i32_at(&self, pos: usize) -> Result<i32> {
        self.u32_at(pos).map(|v| v as i32)
    }

    fn u64_at(&self, pos: usize) -> Result<u64> {
        let s = self.slice(pos, 8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_le_bytes(a))
    }

    /// Follow a forward offset stored at `pos` (relative to `pos`).
    fn follow(&self, pos: usize) -> Result<usize> {
        let off = self.u32_at(pos)? as usize;
        pos.checked_add(off).ok_or(BAD)
    }

    /// The root table.
    pub fn root(&self) -> Result<Table<'a>> {
        let pos = self.follow(0)?;
        Table::at(*self, pos)
    }
}

/// A table: fields are found through its vtable, so a field can be absent.
#[derive(Debug, Clone, Copy)]
pub struct Table<'a> {
    buf: Buf<'a>,
    pos: usize,
    vtable: usize,
    vtable_len: usize,
}

impl<'a> Table<'a> {
    fn at(buf: Buf<'a>, pos: usize) -> Result<Self> {
        let back = buf.i32_at(pos)?;
        let vtable = if back >= 0 {
            pos.checked_sub(back as usize)
        } else {
            pos.checked_add(back.unsigned_abs() as usize)
        }
        .ok_or(BAD)?;
        let vtable_len = buf.u16_at(vtable)? as usize;
        if vtable_len < 4 || vtable_len % 2 != 0 {
            return Err(Error::Malformed("bad vtable size"));
        }
        // The whole vtable must be inside the buffer.
        buf.slice(vtable, vtable_len)?;
        Ok(Self {
            buf,
            pos,
            vtable,
            vtable_len,
        })
    }

    /// Absolute position of field `index`, or `None` when the field is absent.
    fn field(&self, index: usize) -> Result<Option<usize>> {
        let slot = 4 + 2 * index;
        if slot + 2 > self.vtable_len {
            return Ok(None);
        }
        let off = self.buf.u16_at(self.vtable + slot)? as usize;
        if off == 0 {
            return Ok(None);
        }
        Ok(Some(self.pos.checked_add(off).ok_or(BAD)?))
    }

    /// A signed 8-bit field.
    pub fn i8(&self, index: usize, default: i8) -> Result<i8> {
        match self.field(index)? {
            Some(p) => Ok(self.buf.u8_at(p)? as i8),
            None => Ok(default),
        }
    }

    /// An unsigned 8-bit field.
    pub fn u8(&self, index: usize, default: u8) -> Result<u8> {
        match self.field(index)? {
            Some(p) => self.buf.u8_at(p),
            None => Ok(default),
        }
    }

    /// A boolean field.
    pub fn bool(&self, index: usize, default: bool) -> Result<bool> {
        Ok(self.u8(index, u8::from(default))? != 0)
    }

    /// A signed 32-bit field.
    pub fn i32(&self, index: usize, default: i32) -> Result<i32> {
        match self.field(index)? {
            Some(p) => self.buf.i32_at(p),
            None => Ok(default),
        }
    }

    /// A nested table.
    pub fn table(&self, index: usize) -> Result<Option<Table<'a>>> {
        match self.field(index)? {
            Some(p) => {
                let t = self.buf.follow(p)?;
                Ok(Some(Table::at(self.buf, t)?))
            }
            None => Ok(None),
        }
    }

    /// A vector field.
    pub fn vector(&self, index: usize) -> Result<Option<Vector<'a>>> {
        match self.field(index)? {
            Some(p) => {
                let v = self.buf.follow(p)?;
                let len = self.buf.u32_at(v)? as usize;
                let start = v.checked_add(4).ok_or(BAD)?;
                Ok(Some(Vector {
                    buf: self.buf,
                    start,
                    len,
                }))
            }
            None => Ok(None),
        }
    }

    /// A string field, as raw bytes.
    pub fn string(&self, index: usize) -> Result<Option<&'a [u8]>> {
        match self.vector(index)? {
            Some(v) => Ok(Some(v.bytes(1)?)),
            None => Ok(None),
        }
    }
}

/// A vector: a length and `len` elements.
#[derive(Debug, Clone, Copy)]
pub struct Vector<'a> {
    buf: Buf<'a>,
    start: usize,
    len: usize,
}

impl<'a> Vector<'a> {
    /// Number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the vector has no elements.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The raw bytes of the whole vector for elements of `size` bytes; checks they are inside the buffer.
    pub fn bytes(&self, size: usize) -> Result<&'a [u8]> {
        let total = self.len.checked_mul(size).ok_or(BAD)?;
        self.buf.slice(self.start, total)
    }

    /// Byte range `(start, len)` of the vector inside the buffer, for data that stays in place.
    pub fn byte_range(&self, size: usize) -> Result<(usize, usize)> {
        let total = self.len.checked_mul(size).ok_or(BAD)?;
        self.buf.slice(self.start, total)?;
        Ok((self.start, total))
    }

    fn pos(&self, i: usize, size: usize) -> Result<usize> {
        if i >= self.len {
            return Err(BAD);
        }
        self.start
            .checked_add(i.checked_mul(size).ok_or(BAD)?)
            .ok_or(BAD)
    }

    /// Element `i` of a vector of `i32`.
    pub fn i32(&self, i: usize) -> Result<i32> {
        self.buf.i32_at(self.pos(i, 4)?)
    }

    /// Element `i` of a vector of `i64`.
    pub fn i64(&self, i: usize) -> Result<i64> {
        Ok(self.buf.u64_at(self.pos(i, 8)?)? as i64)
    }

    /// Element `i` of a vector of `f32`.
    pub fn f32(&self, i: usize) -> Result<f32> {
        Ok(f32::from_bits(self.buf.u32_at(self.pos(i, 4)?)?))
    }

    /// Element `i` of a vector of tables.
    pub fn table(&self, i: usize) -> Result<Table<'a>> {
        let p = self.pos(i, 4)?;
        let t = self.buf.follow(p)?;
        Table::at(self.buf, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny buffer built by hand: root table with one i32 field (slot 0) and one vector of i32 (slot 1).
    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&12u32.to_le_bytes()); // 0: root -> 12
        b.extend_from_slice(&8u16.to_le_bytes()); // 4: vtable len
        b.extend_from_slice(&12u16.to_le_bytes()); // 6: table len
        b.extend_from_slice(&4u16.to_le_bytes()); // 8: field 0 at +4
        b.extend_from_slice(&8u16.to_le_bytes()); // 10: field 1 at +8
        b.extend_from_slice(&8i32.to_le_bytes()); // 12: table: vtable is at 12 - 8 = 4
        b.extend_from_slice(&(-7i32).to_le_bytes()); // 16: field 0 value
        b.extend_from_slice(&8u32.to_le_bytes()); // 20: field 1 -> vector at 28
        b.extend_from_slice(&2u32.to_le_bytes()); // 24: padding
        b.extend_from_slice(&2u32.to_le_bytes()); // 28: vector length 2
        b.extend_from_slice(&5i32.to_le_bytes()); // 32
        b.extend_from_slice(&9i32.to_le_bytes()); // 36
        b
    }

    #[test]
    fn reads_fields_vectors_and_defaults() {
        let data = sample();
        let t = Buf::new(&data).root().unwrap();
        assert_eq!(t.i32(0, 0).unwrap(), -7);
        let v = t.vector(1).unwrap().unwrap();
        assert_eq!((v.len(), v.i32(0).unwrap(), v.i32(1).unwrap()), (2, 5, 9));
        assert!(v.i32(2).is_err());
        assert_eq!(
            t.i32(5, 42).unwrap(),
            42,
            "an absent field takes its default"
        );
        assert!(t.table(5).unwrap().is_none());
    }

    #[test]
    fn req_wak_002_every_truncation_and_corruption_is_an_error_not_a_panic() {
        let data = sample();
        for cut in 0..data.len() {
            let _ = Buf::new(&data[..cut]).root().and_then(|t| {
                t.i32(0, 0)?;
                let v = t.vector(1)?;
                if let Some(v) = v {
                    for i in 0..v.len().min(8) {
                        v.i32(i)?;
                    }
                    v.bytes(4)?;
                }
                Ok(())
            });
        }
        // Flip every byte to every extreme value.
        for i in 0..data.len() {
            for v in [0u8, 1, 0x7f, 0x80, 0xff] {
                let mut d = data.clone();
                d[i] = v;
                let _ = Buf::new(&d).root().and_then(|t| {
                    t.i32(0, 0)?;
                    if let Some(v) = t.vector(1)? {
                        for i in 0..v.len().min(8) {
                            v.i32(i)?;
                        }
                        v.bytes(4)?;
                    }
                    t.string(1)?;
                    t.table(1)?;
                    Ok(())
                });
            }
        }
    }

    #[test]
    fn a_huge_vector_length_is_refused() {
        let mut d = sample();
        d[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        let t = Buf::new(&d).root().unwrap();
        let v = t.vector(1).unwrap().unwrap();
        assert!(v.bytes(4).is_err());
        assert!(
            v.i32(3).is_err(),
            "elements past the end of the data are refused"
        );
    }
}
