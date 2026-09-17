//! The pure local-wire protocol: framing, limits, canonical JSON, and the closed
//! handshake/request/response/fault grammar between a Marrow runner and its client.
//!
//! This crate is the **single owner of the local wire**: the one framed byte
//! protocol a terminal driver and the generated TypeScript client both speak. It
//! depends on nothing but the diagnostic-code registry ([`marrow_codes`]), touches
//! no socket, and spawns no process; a caller reads and writes the bytes itself.
//!
//! The protocol is closed and small: one protocol version, one canonical JSON
//! encoding, a fixed set of message kinds, and no streaming, replay, cancellation,
//! or pagination. Every decoder input is bounded before it allocates, and an
//! over-limit or malformed input is rejected here with a typed [`WireError`].
//!
//! - [`frame`] — length-prefixed framing with [`MAX_FRAME`].
//! - [`json`] — the canonical JSON model and codec (the wire's canonical-JSON owner).
//! - [`message`] — the closed [`ClientMessage`]/[`ServerMessage`] grammar.

mod error;
mod frame;
mod id;
mod json;
mod message;
mod span;

pub use error::WireError;
pub use frame::{EncodedFrame, frame_body_len};
pub use id::Id32;
pub use json::{
    ArrayWriter, Json, ObjectWriter, ValueWriter, encode, parse_strict, write_json_string,
};
pub use message::{ClientMessage, ServerMessage};
pub use span::Span;

/// The protocol version byte carried by every frame. The runner and the generated
/// client are a matched release pair; a frame with any other version is rejected at
/// the frame boundary ([`WireError::UnsupportedVersion`]).
pub const PROTOCOL_VERSION: u8 = 3;

/// The maximum frame body size (version byte plus canonical JSON), in bytes. A
/// declared frame length past this is rejected before the body is read.
pub const MAX_FRAME: usize = 1 << 20;

/// The maximum nesting depth of a wire JSON value. A value that nests arrays or
/// objects deeper is rejected before it is fully materialized.
pub const MAX_DEPTH: usize = 64;

/// The maximum byte length of a single wire JSON string.
pub const MAX_STRING_BYTES: usize = 64 * 1024;
