//! Length-prefixed framing with an explicit maximum size.
//!
//! A frame is `u32_be(body_len) ‖ body`, where `body` is `u8(version) ‖ json`. The
//! length prefix is validated against [`crate::MAX_FRAME`] before the body is read,
//! so the body read and allocation stay within that bound. The reader is expected
//! to consume exactly four header bytes, call
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

/// A complete frame produced by the wire owner. The header, version, message
/// envelope and canonical payload have all been encoded within the frame limit.
#[derive(Debug, PartialEq, Eq)]
pub struct EncodedFrame(Vec<u8>);

impl EncodedFrame {
    /// Borrow the complete length-prefixed frame for transport.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consume the frame without copying its bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

pub(crate) fn encode(
    write: impl FnOnce(crate::json::ValueWriter<'_>) -> Result<(), WireError>,
) -> Result<EncodedFrame, WireError> {
    let mut out = crate::json::Encoder::new("\0\0\0\0\0".to_string(), Some(4 + MAX_FRAME));
    out.value(write)?;
    // Consume the allocation before filling the binary header and version.
    let mut bytes = out.finish()?.into_bytes();
    bytes[4] = PROTOCOL_VERSION;
    let body_len = u32::try_from(bytes.len() - 4).expect("MAX_FRAME fits u32");
    bytes[..4].copy_from_slice(&body_len.to_be_bytes());
    Ok(EncodedFrame(bytes))
}

#[cfg(test)]
mod tests {
    use super::{body_json, encode, frame_body_len};
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
        let frame =
            encode(|slot| slot.object(|object| object.field("kind", |slot| slot.string("value"))))
                .expect("assemble")
                .into_bytes();
        assert_eq!(&frame[5..], json);
        let len = frame_body_len([frame[0], frame[1], frame[2], frame[3]]).expect("len");
        let body = &frame[4..4 + len];
        assert_eq!(body[0], PROTOCOL_VERSION);
        assert_eq!(body_json(body), Ok(&json[..]));
    }

    #[test]
    fn streamed_frame_counts_envelope_version_and_maximum_turn() {
        use crate::EncodedFrame;
        for (turn, overhead) in [(0, 36), (u32::MAX, 45)] {
            // Empty string plus fixed envelope, decimal turn and version; header excluded.
            let text = "x".repeat(MAX_FRAME - overhead);
            let frame =
                EncodedFrame::value(turn, |slot| slot.string(&text)).expect("exact body fit");
            assert_eq!(frame.as_bytes().len(), 4 + MAX_FRAME);
            assert_eq!(
                frame_body_len(frame.as_bytes()[..4].try_into().expect("header")),
                Ok(MAX_FRAME)
            );
            assert_eq!(frame.as_bytes()[4], PROTOCOL_VERSION);
            assert!(
                frame
                    .as_bytes()
                    .ends_with(format!(",\"kind\":\"value\",\"turn\":{turn}}}").as_bytes())
            );
            assert_eq!(
                EncodedFrame::value(turn, |slot| slot.string(&(text + "x"))),
                Err(WireError::FrameTooLarge),
            );
        }
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
