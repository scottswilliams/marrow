//! The fixture ledger-id namespace: the fixed ids a durable fixture names, and the two
//! minters for every id spelled beside them.
//!
//! Shared because a ledger id is an identity: two files that spell the same fixture
//! identity differently describe two different durable graphs, and a reader comparing
//! them cannot tell which difference is load-bearing.

use marrow_image::LedgerIdBytes;

/// The sixteen-byte ledger id every byte of which is `byte`: the fixed fixture id a test
/// spells inline.
pub fn id(byte: u8) -> LedgerIdBytes {
    LedgerIdBytes::from_bytes([byte; 16])
}

/// The application identity a non-empty durable graph is anchored by.
pub const APPLICATION_ID: [u8; 16] = [0x0a; 16];
/// The placement a fixture's one root occurrence sits at.
pub const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
/// A second placement, for a fixture that admits two occurrences of one Product.
pub const SECOND_PLACEMENT_ID: [u8; 16] = [0x1b; 16];
/// The first key column of a keyed root occurrence.
pub const KEY_ID: [u8; 16] = [0x0c; 16];
/// The Product a fixture declares.
pub const PRODUCT_ID: [u8; 16] = [0x0d; 16];
/// The one field member a fixture Product declares.
pub const FIELD_ID: [u8; 16] = [0x0e; 16];
/// The managed index a fixture occurrence carries.
pub const INDEX_ID: [u8; 16] = [0x3b; 16];

/// A distinct 16-byte ledger id from a `tag` byte and a three-byte `seed`.
///
/// The fixed ids above are the reserved namespace: a seeded id fills every byte it does
/// not use with `0x40`, which no fixed id repeats, so the two spaces cannot meet. `tag`
/// partitions the seeded space by role within one fixture, and three seed bytes carry
/// the distinctness this minter promises — past them the ids silently repeat and a width
/// test would measure a smaller set than it named.
pub fn seeded_id(tag: u8, seed: usize) -> LedgerIdBytes {
    assert!(seed <= 0x00ff_ffff, "an id seed exceeds its three bytes");
    let mut bytes = [0x40u8; 16];
    bytes[0] = tag;
    bytes[1] = seed as u8;
    bytes[2] = (seed >> 8) as u8;
    bytes[3] = (seed >> 16) as u8;
    LedgerIdBytes::from_bytes(bytes)
}
