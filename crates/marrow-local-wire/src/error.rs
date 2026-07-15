//! The typed local-wire rejection.
//!
//! Every way a frame or message can be refused is one [`WireError`] variant, each
//! carrying the single [`marrow_codes::Code`] it renders as. The wire crate is the
//! one owner of these rejections: the runner and the generated client surface a
//! wire failure only through a code this enum produced.

use marrow_codes::Code;

/// Why the single wire owner refused a frame or message. A typed fact, never
/// rendered prose; the [`Self::code`] is the stable machine identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// A frame declared a payload longer than [`crate::MAX_FRAME`].
    FrameTooLarge,
    /// A value nests deeper than [`crate::MAX_DEPTH`].
    DepthLimit,
    /// A string is longer than [`crate::MAX_STRING_BYTES`].
    StringLimit,
    /// The frame's protocol version byte is not [`crate::PROTOCOL_VERSION`].
    UnsupportedVersion,
    /// The body is not a well-formed protocol message (bad JSON, a non-integer
    /// number, an unknown message kind, a missing or wrong-typed field, or trailing
    /// bytes).
    Malformed,
    /// The body is valid JSON but not in canonical form (insignificant whitespace,
    /// unsorted or duplicate object keys, a non-minimal number, or a non-canonical
    /// string escape).
    Noncanonical,
}

impl WireError {
    /// The diagnostic code this rejection renders as.
    pub const fn code(self) -> Code {
        match self {
            WireError::FrameTooLarge => Code::WireFrameTooLarge,
            WireError::DepthLimit => Code::WireDepthLimit,
            WireError::StringLimit => Code::WireStringLimit,
            WireError::UnsupportedVersion => Code::WireUnsupportedVersion,
            WireError::Malformed => Code::WireMalformed,
            WireError::Noncanonical => Code::WireNoncanonical,
        }
    }

    /// The canonical dotted code string.
    pub const fn code_str(self) -> &'static str {
        self.code().as_str()
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code_str())
    }
}

impl std::error::Error for WireError {}
