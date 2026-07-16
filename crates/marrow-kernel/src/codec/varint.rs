//! Minimal (canonical) unsigned LEB128 length framing for the widened value codec.
//!
//! A composite durable value (a `struct`/record or a closed `enum`/`Option`/`Result`)
//! packs several scalar leaves into one field-leaf cell. Because the scalar value codec is
//! text-based and variable-length (`int` is decimal text), each scalar leaf carries a
//! byte-length prefix so the decoder can split leaves without a delimiter. That prefix is
//! an unsigned LEB128 integer in **canonical minimal form**: the shortest byte sequence for
//! the magnitude, low 7-bit group first, high bit as the continuation flag.
//!
//! Canonicality is load-bearing for the codec's identity law (one value, one encoding): a
//! non-minimal encoding — a redundant trailing `0x80` group, e.g. `0x80 0x00` for zero — is
//! **rejected, never normalized**, so two byte strings can never decode to the same value.

/// The largest LEB128 payload this framing admits: a value byte-length, bounded well within
/// `usize`. Ten 7-bit groups cover the full 64-bit range; a longer run is a malformed frame.
const MAX_GROUPS: usize = 10;

/// Append the canonical minimal LEB128 encoding of `value` to `out`.
pub(crate) fn encode_len(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let group = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(group);
            return;
        }
        out.push(group | 0x80);
    }
}

/// Decode a canonical minimal LEB128 integer from the front of `bytes`, returning the value
/// and the number of bytes consumed. Rejects (`None`):
/// - a truncated run (a continuation group with no successor);
/// - an overlong run (more than [`MAX_GROUPS`] groups, or a shift past 64 bits);
/// - a **non-minimal** encoding: a multi-byte run whose final group is `0x00` (a redundant
///   high group), so `0x80 0x00` (a non-minimal zero) and any padded form is refused.
pub(crate) fn decode_len(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for index in 0..MAX_GROUPS {
        let byte = *bytes.get(index)?;
        let group = u64::from(byte & 0x7f);
        // A shift of 63 leaves room for at most one more bit; guard the 64-bit ceiling so an
        // overlong or overflowing run is refused rather than silently truncated.
        if shift >= 64 || (shift == 63 && group > 1) {
            return None;
        }
        value |= group << shift;
        if byte & 0x80 == 0 {
            // Minimal form: a multi-byte run's last group must be non-zero, else a shorter
            // run encodes the same value. (A single `0x00` byte is the canonical zero.)
            if index > 0 && group == 0 {
                return None;
            }
            return Some((value, index + 1));
        }
        shift += 7;
    }
    // Ran off MAX_GROUPS without a terminating group: malformed.
    None
}

#[cfg(test)]
mod tests {
    use super::{decode_len, encode_len};

    fn enc(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        encode_len(value, &mut out);
        out
    }

    #[test]
    fn round_trips_representative_values() {
        for value in [
            0u64,
            1,
            63,
            64,
            127,
            128,
            129,
            16_383,
            16_384,
            1 << 20,
            u64::MAX,
        ] {
            let bytes = enc(value);
            let (decoded, used) = decode_len(&bytes).expect("decodes");
            assert_eq!(decoded, value);
            assert_eq!(used, bytes.len(), "consumes the exact frame");
        }
    }

    #[test]
    fn minimal_fingerprints_are_stable() {
        assert_eq!(enc(0), vec![0x00]);
        assert_eq!(enc(1), vec![0x01]);
        assert_eq!(enc(127), vec![0x7f]);
        assert_eq!(enc(128), vec![0x80, 0x01]);
        assert_eq!(enc(16_384), vec![0x80, 0x80, 0x01]);
    }

    #[test]
    fn decode_stops_at_the_frame_and_ignores_trailing_bytes() {
        // The frame is self-delimiting; a caller consumes `used` and handles the rest.
        let mut bytes = enc(300);
        let frame_len = bytes.len();
        bytes.extend_from_slice(b"tail");
        let (value, used) = decode_len(&bytes).expect("decodes the leading frame");
        assert_eq!(value, 300);
        assert_eq!(used, frame_len);
    }

    #[test]
    fn a_non_minimal_encoding_is_rejected_never_normalized() {
        // `0x80 0x00` is a two-byte encoding of zero; the canonical zero is `0x00`.
        assert_eq!(decode_len(&[0x80, 0x00]), None);
        // A redundant high `0x00` group on a nonzero value.
        assert_eq!(decode_len(&[0x81, 0x00]), None);
        // A minimal two-byte form still decodes (control: 128 is genuinely two bytes).
        assert_eq!(decode_len(&[0x80, 0x01]), Some((128, 2)));
    }

    #[test]
    fn a_truncated_run_is_rejected() {
        // A continuation group with no successor byte.
        assert_eq!(decode_len(&[0x80]), None);
        assert_eq!(decode_len(&[0x80, 0x80]), None);
        assert_eq!(decode_len(&[]), None);
    }

    #[test]
    fn an_overlong_or_overflowing_run_is_rejected() {
        // Eleven continuation groups exceed MAX_GROUPS.
        let overlong = [0x80u8; 11];
        assert_eq!(decode_len(&overlong), None);
        // Ten groups whose top group sets a bit past 64.
        let overflow = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02];
        assert_eq!(decode_len(&overflow), None);
    }
}
