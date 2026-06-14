const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn push_lower_hex(out: &mut String, bytes: &[u8]) {
    for byte in bytes {
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn push_lower_hex_uses_canonical_lowercase_pairs() {
        let mut out = String::from("prefix:");

        super::push_lower_hex(
            &mut out,
            &[
                0x00, 0x01, 0x02, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0xff,
            ],
        );

        assert_eq!(out, "prefix:000102090a0b0c0d0e0f10ff");
    }
}
