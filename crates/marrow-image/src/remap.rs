//! Opaque canonical-order remap tokens.
//!
//! The encoder sorts the string and constant pools into canonical order and rewrites
//! every reference through a sort map. A section writer must not be able to *read* those
//! maps: a writer that can inspect a remapped index can branch on it, and a section whose
//! bytes depend on the sort order cannot be reasoned about from its rows. Writers
//! therefore receive tokens: each is minted only by its remap provider, carries its value
//! privately, and has exactly one operation, a consuming `emit` that appends the value's
//! two big-endian bytes to a sink. There is no accessor, comparison, or conversion, and
//! the string and constant domains are distinct types, so remap-dependent behavior other
//! than writing exactly two bytes is unrepresentable in a writer.

use crate::draft::{ConstId, StrId};
use crate::value_dag::{ImageByteSink, push_u16};

/// An opaque remapped string-pool reference: two wire bytes a writer can append and
/// nothing else.
pub(crate) struct StringToken(u16);

impl StringToken {
    /// Append this token's two big-endian bytes — the one operation a token has. The
    /// nine non-DURABLE section writers spend tokens only into the sealed
    /// [`SectionSink`] their drivers hand them, so a writer holds no sink it could
    /// read a token's bytes back out of.
    pub(crate) fn emit<S: ImageByteSink>(self, sink: &mut SectionSink<'_, S>) {
        self.emit_durable(sink);
    }

    /// The DURABLE writer's spending path: `write_durable_body` keeps its pinned
    /// public-sink signature, so its tokens append through the bound the gate pins. The
    /// writer is in a sibling module, so no visibility narrower than `pub(crate)` reaches
    /// it; the spend gate pins every spelling of this call to the writer file instead.
    pub(crate) fn emit_durable(self, sink: &mut impl ImageByteSink) {
        push_u16(sink, self.0);
    }
}

/// An opaque remapped constant-pool reference: two wire bytes a writer can append and
/// nothing else.
pub(crate) struct ConstToken(u16);

impl ConstToken {
    /// Append this token's two big-endian bytes — the one operation a token has,
    /// sealed to the [`SectionSink`] like [`StringToken::emit`]. The constant remap
    /// serves only section writers, so it has no DURABLE spending path.
    pub(crate) fn emit<S: ImageByteSink>(self, sink: &mut SectionSink<'_, S>) {
        push_u16(sink, self.0);
    }
}

/// The string remap: the one owner of reads from the string sort map. Writers receive
/// this provider and obtain per-reference [`StringToken`]s; the map's values never
/// leave it in readable form.
pub(crate) struct StringRemap<'a>(&'a [u16]);

impl<'a> StringRemap<'a> {
    pub(crate) fn new(map: &'a [u16]) -> Self {
        Self(map)
    }

    /// The token for one drafted string reference. An id outside the pool panics
    /// exactly as the raw map indexing it replaces did.
    pub(crate) fn token(&self, id: StrId) -> StringToken {
        StringToken(self.0[id.raw() as usize])
    }
}

/// The constant remap: the one owner of reads from the constant sort map, looked up
/// by the typed wide [`ConstId`] an instruction operand carries.
pub(crate) struct ConstRemap<'a>(&'a [u16]);

impl<'a> ConstRemap<'a> {
    pub(crate) fn new(map: &'a [u16]) -> Self {
        Self(map)
    }

    /// The token for one drafted constant reference. An id outside the pool panics
    /// exactly as the raw map indexing it replaces did.
    pub(crate) fn token(&self, id: ConstId) -> ConstToken {
        ConstToken(self.0[id.index() as usize])
    }
}

/// The sealed sink the nine non-DURABLE section writers receive.
///
/// A writer generic over the public [`ImageByteSink`] could conjure a probe
/// `Vec<u8>`, spend a token into it, and branch on the bytes — exactly the
/// remap-dependent behavior the token seal exists to forbid. This newtype closes
/// that route: the writers' signatures demand it, tokens spend only into it, its
/// field is private, and its one constructor is pinned by the carrier gate to the
/// measure core's counting and emission drivers (and the test tier), so no writer
/// can seal a sink of its own.
pub(crate) struct SectionSink<'a, S: ImageByteSink>(&'a mut S);

impl<'a, S: ImageByteSink> SectionSink<'a, S> {
    /// Seal one driver-owned sink for a section writer's run.
    pub(crate) fn over(sink: &'a mut S) -> Self {
        Self(sink)
    }
}

impl<S: ImageByteSink> ImageByteSink for SectionSink<'_, S> {
    fn push(&mut self, byte: u8) {
        self.0.push(byte);
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        self.0.extend_bytes(bytes);
    }

    fn is_full(&self) -> bool {
        self.0.is_full()
    }
}
