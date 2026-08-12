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
    /// Append this token's two big-endian bytes — the one operation a token has.
    pub(crate) fn emit(self, sink: &mut impl ImageByteSink) {
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
    /// Append this token's two big-endian bytes — the one operation a token has.
    pub(crate) fn emit(self, sink: &mut impl ImageByteSink) {
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

/// The constant remap: the one owner of reads from the constant sort map. Instruction
/// operands carry raw drafted indices, so the lookup takes the raw index rather than a
/// typed id.
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
