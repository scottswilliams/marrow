//! An owner-local bounded byte reader.
//!
//! The verifier decodes untrusted image bytes, so every read is length-checked and
//! every length/offset is validated against the remaining input *before* it is
//! used to slice or allocate. A short read is a typed envelope/table rejection, not
//! a panic. This reader is private to the verifier: no decode utility is shared
//! across the trust boundary (design §F deletes the prototype's shared reader).

/// A cursor over a byte slice that never reads past the end.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        let raw: [u8; 2] = self.take(2)?.try_into().ok()?;
        Some(u16::from_be_bytes(raw))
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        let raw: [u8; 4] = self.take(4)?.try_into().ok()?;
        Some(u32::from_be_bytes(raw))
    }

    pub(crate) fn i64(&mut self) -> Option<i64> {
        let raw: [u8; 8] = self.take(8)?.try_into().ok()?;
        Some(i64::from_be_bytes(raw))
    }
}
