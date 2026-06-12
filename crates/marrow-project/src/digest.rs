//! SHA-256 for catalog and analyzed-source integrity digests.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

pub fn sha256_digest(bytes: &[u8]) -> String {
    let mut digest = Sha256Digest::new();
    digest.update(bytes);
    digest.finish()
}

pub struct Sha256Digest {
    hash: Sha256,
}

impl Sha256Digest {
    pub fn new() -> Self {
        Self {
            hash: Sha256::new(),
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.hash.update(bytes);
    }

    pub fn finish(self) -> String {
        let digest = self.hash.finalize();
        sha256_hex(digest.as_slice())
    }
}

impl Default for Sha256Digest {
    fn default() -> Self {
        Self::new()
    }
}

fn sha256_hex(digest: &[u8]) -> String {
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn sha256_matches_known_vectors() {
        let million_a = vec![b'a'; 1_000_000];
        let vectors: &[(&[u8], &str)] = &[
            (
                b"",
                "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"message digest",
                "sha256:f7846f55cf23e14eebeab5b4e1550cad5b509e3348fbc4efa3a1413d393cb650",
            ),
            (
                b"abcdefghijklmnopqrstuvwxyz",
                "sha256:71c480df93d6ae2f1efad1447c66c9525e316218cf51fc8d9ed832f2daf18b73",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                &million_a,
                "sha256:cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ];
        for (input, expected) in vectors {
            assert_eq!(super::sha256_digest(input), *expected);
        }
    }

    #[test]
    fn sha256_matches_padding_boundary_vectors() {
        let a55 = vec![b'a'; 55];
        let a56 = vec![b'a'; 56];
        let a57 = vec![b'a'; 57];
        let vectors: &[(&[u8], &str)] = &[
            (
                &a55,
                "sha256:9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                &a56,
                "sha256:b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            (
                &a57,
                "sha256:f13b2d724659eb3bf47f2dd6af1accc87b81f09f59f2b75e5c0bed6589dfe8c6",
            ),
        ];
        for (input, expected) in vectors {
            assert_eq!(super::sha256_digest(input), *expected);
        }
    }
}
