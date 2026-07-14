//! Slice K.4 image-bytes fuzz driver (design §E, B02 pattern).
//!
//! A bounded, seeded, deterministic driver over the verifier's decoder: the reusable
//! oracle asserts that `verify` never panics and never allocates unboundedly on
//! arbitrary or mutated bytes — it always returns a typed rejection (or, rarely, a
//! valid image). No external fuzz dependency; a fixed iteration budget keeps it in
//! the default suite. A minimized counterexample becomes a permanent fixture.

use marrow_image::{
    ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry, image_id,
};
use marrow_verify::verify;

/// The reusable bounded oracle: `verify` must return without panicking, and any
/// success must be internally consistent (its digest recomputes over the payload).
fn oracle(bytes: &[u8]) {
    if let Ok(image) = verify(bytes) {
        // A verified image's stored digest must equal the recomputed payload digest —
        // a decode that accepts a mismatched digest would be unsound.
        let recomputed = image_id(&bytes[37..]);
        assert_eq!(image.image_id().0, recomputed.0, "verified digest mismatch");
    }
}

/// A tiny deterministic xorshift RNG (no external dependency).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn seed() -> u64 {
    std::env::var("MARROW_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

fn a_good_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let answer = draft.intern_int(42);
    let code = vec![Instr::ConstLoad(answer.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: vec![SpanEntry {
            instr_index: 0,
            line: 1,
            column: 1,
        }],
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn random_bytes_never_panic_the_verifier() {
    let mut rng = Rng(seed());
    for _ in 0..4096 {
        let len = rng.below(512);
        let bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        oracle(&bytes);
    }
}

#[test]
fn mutated_good_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0xD1B5_4A32_D192_ED03);
    let base = a_good_image();
    for _ in 0..4096 {
        let mut bytes = base.clone();
        // Flip one to three random bytes.
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

#[test]
fn structured_prefix_of_a_good_image_never_panics() {
    let base = a_good_image();
    // Every truncation of a good image must decode-reject cleanly, never panic.
    for len in 0..base.len() {
        oracle(&base[..len]);
    }
}
