//! Length-prefixed framing with an explicit maximum size.
//!
//! A frame is `u32_be(body_len) ‖ body`, where `body` is `u8(version) ‖ json`. The
//! length prefix is validated against [`crate::MAX_FRAME`] before the body is read,
//! so a hostile length cannot drive an unbounded read or allocation (campaign law
//! 9). The reader is expected to consume exactly four header bytes, call
//! [`frame_body_len`], read that many body bytes, and hand them to a message
//! decoder; this crate never touches a socket.

use crate::error::WireError;
use crate::{MAX_FRAME, PROTOCOL_VERSION};

/// The length of a frame's body, decoded and bounds-checked from its four-byte
/// big-endian length prefix. A zero length (no room for the version byte) is
/// malformed; a length past [`crate::MAX_FRAME`] is rejected before the body is
/// read.
pub fn frame_body_len(header: [u8; 4]) -> Result<usize, WireError> {
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 {
        return Err(WireError::Malformed);
    }
    if len > MAX_FRAME {
        return Err(WireError::FrameTooLarge);
    }
    Ok(len)
}

/// Split a frame body into its JSON bytes after checking the protocol version.
pub(crate) fn body_json(body: &[u8]) -> Result<&[u8], WireError> {
    let (&version, json) = body.split_first().ok_or(WireError::Malformed)?;
    if version != PROTOCOL_VERSION {
        return Err(WireError::UnsupportedVersion);
    }
    Ok(json)
}

/// Assemble a full frame (`length ‖ version ‖ json`) from canonical JSON bytes,
/// rejecting a body that would exceed [`crate::MAX_FRAME`].
pub(crate) fn assemble(json: &[u8]) -> Result<Vec<u8>, WireError> {
    let body_len = 1 + json.len();
    if body_len > MAX_FRAME {
        return Err(WireError::FrameTooLarge);
    }
    let mut out = Vec::with_capacity(4 + body_len);
    out.extend_from_slice(&(body_len as u32).to_be_bytes());
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(json);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{assemble, body_json, frame_body_len};
    use crate::error::WireError;
    use crate::{MAX_FRAME, PROTOCOL_VERSION};

    #[test]
    fn header_length_is_bounded() {
        assert_eq!(frame_body_len([0, 0, 0, 0]), Err(WireError::Malformed));
        assert_eq!(frame_body_len([0, 0, 0, 5]), Ok(5));
        let big = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert_eq!(frame_body_len(big), Err(WireError::FrameTooLarge));
        let ok = (MAX_FRAME as u32).to_be_bytes();
        assert_eq!(frame_body_len(ok), Ok(MAX_FRAME));
    }

    #[test]
    fn assemble_then_split_round_trips() {
        let json = br#"{"kind":"value"}"#;
        let frame = assemble(json).expect("assemble");
        let len = frame_body_len([frame[0], frame[1], frame[2], frame[3]]).expect("len");
        let body = &frame[4..4 + len];
        assert_eq!(body[0], PROTOCOL_VERSION);
        assert_eq!(body_json(body), Ok(&json[..]));
    }

    #[test]
    fn wrong_version_is_rejected() {
        assert_eq!(
            body_json(&[PROTOCOL_VERSION.wrapping_add(1), b'{', b'}']),
            Err(WireError::UnsupportedVersion)
        );
        assert_eq!(body_json(&[]), Err(WireError::Malformed));
    }
}
