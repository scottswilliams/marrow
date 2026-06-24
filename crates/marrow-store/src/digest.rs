//! The per-root structural digest that anchors data-family completeness.
//!
//! A localized data-page corruption can drop or silently rewrite a committed cell
//! while every traversal still reads cleanly past the damage: the data family is its
//! own derivation, so any expectation drawn from the live cells shrinks or shifts with
//! them. The independent oracle is a digest the commit stamps over every committed
//! cell, summing one 128-bit hash per cell into a single per-root value. Each hash
//! covers the cell's full physical key — its root, identity, and field path — together
//! with its stored value bytes, so a dropped cell, a torn-but-decodable value, or a
//! moved field all change the digest. The sum combiner is wrapping `u128` addition,
//! which is commutative and associative, so the digest is order-independent and a write
//! maintains it in constant time: add the new cell's hash and, on overwrite or delete,
//! subtract the prior one. The same per-cell hash drives both the incremental update
//! and the full re-derivation integrity runs, so the two can never disagree by
//! construction.

/// The accumulated structural digest of a set of committed cells: the wrapping sum of
/// each cell's [`cell_hash`]. The zero digest is the empty set, so a root with no data
/// cells and a root whose cells all net out carry the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RootDigest(u128);

impl RootDigest {
    pub(crate) const ENCODED_LEN: usize = 16;

    pub(crate) fn zero() -> Self {
        Self(0)
    }

    /// Fold one committed cell into the digest.
    pub(crate) fn add_cell(&mut self, key: &[u8], value: &[u8]) {
        self.0 = self.0.wrapping_add(cell_hash(key, value));
    }

    /// Remove one cell that was previously folded in, the inverse of [`add_cell`].
    pub(crate) fn remove_cell(&mut self, key: &[u8], value: &[u8]) {
        self.0 = self.0.wrapping_sub(cell_hash(key, value));
    }

    /// Fold an accumulated delta into this digest. Because the combiner is wrapping
    /// addition, a transaction's net change can be applied to the prior stamp as a single
    /// delta rather than by replaying each cell.
    pub(crate) fn add(&mut self, delta: RootDigest) {
        self.0 = self.0.wrapping_add(delta.0);
    }

    pub(crate) fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn to_be_bytes(self) -> [u8; Self::ENCODED_LEN] {
        self.0.to_be_bytes()
    }

    pub(crate) fn from_be_bytes(bytes: [u8; Self::ENCODED_LEN]) -> Self {
        Self(u128::from_be_bytes(bytes))
    }
}

/// A 128-bit content hash over a cell's full physical key and stored value. The key
/// already encodes the cell's root, record identity, and field path unambiguously, so
/// hashing key-then-value over a length-framed stream makes the digest sensitive to the
/// cell's identity and its bytes alike: any flipped value byte, dropped cell, or moved
/// field changes the result. Two independent 64-bit FNV-1a streams over distinct bases
/// give the 128 bits, and framing each segment by length keeps a key/value boundary
/// shift from colliding with a different split of the same bytes.
fn cell_hash(key: &[u8], value: &[u8]) -> u128 {
    const BASIS_HI: u64 = 0xcbf2_9ce4_8422_2325;
    const BASIS_LO: u64 = 0x9e37_79b9_7f4a_7c15;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hi = BASIS_HI;
    let mut lo = BASIS_LO;
    let mut mix = |bytes: &[u8]| {
        for chunk in (bytes.len() as u64).to_le_bytes() {
            hi = (hi ^ u64::from(chunk)).wrapping_mul(PRIME);
            lo = (lo ^ u64::from(chunk)).wrapping_mul(PRIME);
        }
        for &byte in bytes {
            hi = (hi ^ u64::from(byte)).wrapping_mul(PRIME);
            lo = (lo ^ u64::from(byte)).wrapping_mul(PRIME);
        }
    };
    mix(key);
    mix(value);
    (u128::from(hi) << 64) | u128::from(lo)
}

#[cfg(test)]
mod tests {
    use super::RootDigest;

    #[test]
    fn empty_digest_is_zero_and_round_trips_through_bytes() {
        let digest = RootDigest::zero();
        assert!(digest.is_zero());
        assert_eq!(RootDigest::from_be_bytes(digest.to_be_bytes()), digest);
    }

    #[test]
    fn add_then_remove_a_cell_restores_the_prior_digest() {
        let mut digest = RootDigest::zero();
        digest.add_cell(b"k1", b"v1");
        let after_first = digest;
        digest.add_cell(b"k2", b"v2");
        digest.remove_cell(b"k2", b"v2");
        assert_eq!(digest, after_first);
        digest.remove_cell(b"k1", b"v1");
        assert!(digest.is_zero());
    }

    #[test]
    fn folding_is_order_independent() {
        let mut forward = RootDigest::zero();
        forward.add_cell(b"a", b"1");
        forward.add_cell(b"b", b"2");
        forward.add_cell(b"c", b"3");
        let mut reverse = RootDigest::zero();
        reverse.add_cell(b"c", b"3");
        reverse.add_cell(b"b", b"2");
        reverse.add_cell(b"a", b"1");
        assert_eq!(forward, reverse);
    }

    #[test]
    fn a_torn_value_changes_the_digest() {
        let mut intact = RootDigest::zero();
        intact.add_cell(b"key", b"the original body");
        let mut torn = RootDigest::zero();
        torn.add_cell(b"key", b"the corrupted body");
        assert_ne!(intact, torn);
    }

    #[test]
    fn dropping_a_cell_changes_the_digest() {
        let mut all = RootDigest::zero();
        all.add_cell(b"k1", b"v1");
        all.add_cell(b"k2", b"v2");
        let mut dropped = RootDigest::zero();
        dropped.add_cell(b"k1", b"v1");
        assert_ne!(all, dropped);
    }

    #[test]
    fn a_value_moved_to_a_different_field_changes_the_digest() {
        let mut here = RootDigest::zero();
        here.add_cell(b"field_a", b"value");
        let mut there = RootDigest::zero();
        there.add_cell(b"field_b", b"value");
        assert_ne!(here, there);
    }

    #[test]
    fn a_key_value_boundary_shift_does_not_collide() {
        let mut split_one = RootDigest::zero();
        split_one.add_cell(b"ab", b"cd");
        let mut split_two = RootDigest::zero();
        split_two.add_cell(b"a", b"bcd");
        assert_ne!(split_one, split_two);
    }
}
