//! The container header layout a forged-image test must know: where the image digest
//! sits, and which bytes it covers.
//!
//! A hostile artifact must carry a *valid* digest, or every one of them stops at the
//! envelope gate and proves nothing about the phase that owns the invariant under test.
//! Recomputing that digest means naming the digest slot and the payload it covers by
//! offset, which is the container format itself — so it is shared rather than copied.
//! A hand copy that drifts from the real header does not fail loudly: it produces
//! artifacts that reject at the envelope, and a test asserting a *rejection* still
//! passes, at the wrong phase, for the wrong reason.

use marrow_image::image_id;

/// The image digest slot: 32 bytes at offset 5 of the container header.
const DIGEST_SLOT: std::ops::Range<usize> = 5..37;

/// Recompute and rewrite the digest over the payload — every byte after the slot — so a
/// forged artifact reaches the phase that owns the invariant it violates rather than
/// stopping at the envelope.
pub fn rehash(bytes: &mut [u8]) {
    let id = image_id(&bytes[DIGEST_SLOT.end..]);
    bytes[DIGEST_SLOT].copy_from_slice(&id.0);
}

/// Overwrite the `nth` (0-based) occurrence of `needle` with `replacement` and rehash.
pub fn forge(bytes: &mut [u8], needle: &[u8], nth: usize, replacement: &[u8]) {
    assert_eq!(needle.len(), replacement.len());
    let at = bytes
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(offset, _)| offset)
        .nth(nth)
        .expect("the pattern occurs often enough to forge");
    bytes[at..at + needle.len()].copy_from_slice(replacement);
    rehash(bytes);
}
