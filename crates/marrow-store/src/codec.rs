//! Shared byte-framing helpers for private store codecs.

pub(crate) struct BoundedReader<'a, E> {
    bytes: &'a [u8],
    malformed: fn(&[u8]) -> E,
}

impl<'a, E> BoundedReader<'a, E> {
    pub(crate) fn new(bytes: &'a [u8], malformed: fn(&[u8]) -> E) -> Self {
        Self { bytes, malformed }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], E> {
        let Some((head, tail)) = self.bytes.split_at_checked(len) else {
            return Err((self.malformed)(self.bytes));
        };
        self.bytes = tail;
        Ok(head)
    }

    pub(crate) fn take_u8(&mut self) -> Result<u8, E> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn take_array<const N: usize>(&mut self) -> Result<[u8; N], E> {
        let bytes = self.take(N)?;
        let mut array = [0; N];
        array.copy_from_slice(bytes);
        Ok(array)
    }

    pub(crate) fn take_u32(&mut self) -> Result<u32, E> {
        Ok(u32::from_be_bytes(self.take_array()?))
    }

    pub(crate) fn take_u64(&mut self) -> Result<u64, E> {
        Ok(u64::from_be_bytes(self.take_array()?))
    }

    pub(crate) fn take_prefixed_bytes(&mut self) -> Result<&'a [u8], E> {
        let len = self.take_u32()? as usize;
        self.take(len)
    }

    pub(crate) fn take_bounded_count(&mut self, min_element_bytes: usize) -> Result<usize, E> {
        self.take_bounded_count_with(min_element_bytes, self.malformed)
    }

    pub(crate) fn take_bounded_count_with(
        &mut self,
        min_element_bytes: usize,
        malformed: fn(&[u8]) -> E,
    ) -> Result<usize, E> {
        let len = self.take_u32()? as usize;
        if min_element_bytes == 0 || len > self.bytes.len() / min_element_bytes {
            return Err(malformed(self.bytes));
        }
        Ok(len)
    }
}
