//! Opaque canonical-order remap tokens (design §C token law).
//!
//! The encoder sorts the string and constant pools into canonical order and rewrites
//! every reference through a sort map. The section writers must not be able to *read*
//! those maps: a writer that can inspect a remapped index can branch on it, and a
//! branch on a remap value is exactly how a counted section and an emitted section
//! drift apart. This module therefore hands writers tokens instead of indices.
//!
//! A token is minted only by its remap provider, carries its value privately, and has
//! exactly one operation: a consuming [`StringToken::emit`]/[`ConstToken::emit`] that
//! appends the value's two big-endian bytes to an [`ImageByteSink`]. There is no
//! accessor, no comparison, no formatting, and no conversion, so remap-dependent
//! behavior other than writing exactly two bytes is unrepresentable in a writer. The
//! two domains are distinct types, so a string remap cannot answer for a constant
//! reference or the reverse.
//!
//! The token types are publicly *nameable* — like the durable member views, whose
//! unconstructibility is proved the same way — solely so the seal is pinned by
//! compile-fail probes from outside the crate; no operation on them is public.

use crate::draft::StrId;
use crate::value_dag::{ImageByteSink, push_u16};

/// An opaque remapped string-pool reference: two wire bytes a writer can append and
/// nothing else.
///
/// ```compile_fail,E0369
/// // Two string tokens cannot be compared, so a writer cannot branch on a remap value.
/// fn same(a: marrow_image::StringToken, b: marrow_image::StringToken) -> bool {
///     a == b
/// }
/// ```
///
/// ```compile_fail,E0616
/// // A token's value cannot be read.
/// fn value(token: marrow_image::StringToken) -> u16 {
///     token.0
/// }
/// ```
///
/// ```compile_fail,E0603
/// // Nor can a token be forged from a raw index.
/// fn forge() -> marrow_image::StringToken {
///     marrow_image::StringToken(0)
/// }
/// ```
///
/// ```compile_fail,E0308
/// // A string token cannot stand where a constant token is expected.
/// fn takes_const(_: marrow_image::ConstToken) {}
/// fn cross(token: marrow_image::StringToken) {
///     takes_const(token)
/// }
/// ```
pub struct StringToken(u16);

impl StringToken {
    /// Append this token's two big-endian bytes — the one operation a token has. The
    /// nine non-DURABLE section writers spend tokens only into the sealed
    /// [`SectionSink`] their drivers hand them, so a writer holds no sink it could
    /// read a token's bytes back out of.
    pub(crate) fn emit<S: ImageByteSink>(self, sink: &mut SectionSink<'_, S>) {
        self.emit_durable(sink);
    }

    /// The DURABLE writer's spending path: `write_durable_body` keeps its pinned
    /// public-sink signature, so its tokens append through the bound the gate pins;
    /// the writer-region gate forbids this spelling anywhere else.
    pub(crate) fn emit_durable(self, sink: &mut impl ImageByteSink) {
        push_u16(sink, self.0);
    }
}

/// An opaque remapped constant-pool reference: two wire bytes a writer can append and
/// nothing else.
///
/// ```compile_fail,E0369
/// // Two constant tokens cannot be compared.
/// fn same(a: marrow_image::ConstToken, b: marrow_image::ConstToken) -> bool {
///     a == b
/// }
/// ```
///
/// ```compile_fail,E0616
/// // A token's value cannot be read.
/// fn value(token: marrow_image::ConstToken) -> u16 {
///     token.0
/// }
/// ```
///
/// ```compile_fail,E0308
/// // A constant token cannot stand where a string token is expected.
/// fn takes_string(_: marrow_image::StringToken) {}
/// fn cross(token: marrow_image::ConstToken) {
///     takes_string(token)
/// }
/// ```
pub struct ConstToken(u16);

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
///
/// The counting instantiation carries no map and yields the constant token: a token is
/// two bytes whatever its value, so a counted section's length cannot depend on the
/// sort — which is what lets the measure core count every section before sorting
/// anything, with the counted==emitted KATs as the executable proof.
pub(crate) struct StringRemap<'a>(Option<&'a [u16]>);

impl<'a> StringRemap<'a> {
    pub(crate) fn new(map: &'a [u16]) -> Self {
        Self(Some(map))
    }

    /// The allocation-free counting instantiation: every token is the constant token.
    /// Its callers run strictly after the coherence walk has proved every reference in
    /// range, so no lookup is skipped that could have refused one.
    pub(crate) fn counting() -> Self {
        Self(None)
    }

    /// The token for one drafted string reference. Under a sorted map, an id outside
    /// the pool panics exactly as the raw map indexing it replaces did.
    pub(crate) fn token(&self, id: StrId) -> StringToken {
        StringToken(match self.0 {
            Some(map) => map[id.raw() as usize],
            None => 0,
        })
    }
}

/// The constant remap: the one owner of reads from the constant sort map. Instruction
/// operands carry raw drafted indices, so the lookup takes the raw index rather than a
/// typed id. It has no counting instantiation: the measure core counts the FUNCTIONS
/// section arithmetically from the shared per-item widths, so only emission resolves
/// constant references.
pub(crate) struct ConstRemap<'a>(&'a [u16]);

impl<'a> ConstRemap<'a> {
    pub(crate) fn new(map: &'a [u16]) -> Self {
        Self(map)
    }

    /// The token for one drafted constant reference. An index outside the pool panics
    /// exactly as the raw map indexing it replaces did.
    pub(crate) fn token(&self, raw: u16) -> ConstToken {
        ConstToken(self.0[raw as usize])
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
