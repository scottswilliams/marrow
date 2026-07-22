//! Deferred C03/C04 obligation on the native path: durable temporal keys and values are
//! stable across a real store close and reopen, and the durable key order after a restart is
//! the language comparison order.
//!
//! The composition this closes: `temporal_order_agreement` (a marrow-vm KAT) proves the
//! language `<` order of each temporal scalar equals the kernel key-codec byte order; this
//! test proves the native ordered-byte engine persists those encoded keys and ranges over
//! them in ascending byte order across a genuine close/reopen. Together they establish that a
//! durable root keyed by a temporal scalar iterates in language order after a restart, for
//! every durable-key scalar including temporal — and that a stored value (an instant among
//! them) round-trips its bytes unchanged. The int/text/bool key and value round-trips over a
//! real restart are covered end to end by the Workshop companion journey
//! (`marrow-runner/tests/native_attach.rs`).

use marrow_kernel::codec::key::{KeyScalar, encode_key_value};
use marrow_store::{ByteEngine, NativeEngineOwner, ReadView, WriteTxn};

fn scratch(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "marrow-native-temporal-{tag}-{}-{nonce}-{counter}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Write each key scalar (scrambled from language order) as a cell whose value echoes the
/// encoded key, commit, close the store, reopen it, and return the persisted cells in the
/// engine's ascending scan order.
fn round_trip(tag: &str, scrambled: &[KeyScalar]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let path = scratch(tag);
    NativeEngineOwner::provision(&path).expect("provision native store");
    {
        let mut engine = NativeEngineOwner::open_existing_admitted(&path, [0x33; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("open native store");
        let mut txn = engine.begin().expect("begin");
        for key in scrambled {
            let encoded = encode_key_value(key);
            // The value echoes the key bytes so a byte-for-byte value round-trip is checked
            // alongside the key order (an instant/date/duration stored as a value).
            txn.put(&encoded, encoded.clone()).expect("put");
        }
        assert!(
            matches!(txn.commit(), marrow_store::CommitOutcome::Confirmed),
            "the commit must confirm durably",
        );
        // Dropping `engine` closes the file: the next open is a genuine restart.
    }

    let engine = NativeEngineOwner::open_existing_admitted(&path, [0x33; 16], || {
        Ok::<_, std::convert::Infallible>(())
    })
    .expect("reopen native store");
    let view = engine.read_view().expect("read view");
    let cells = view.scan_after(&[], &[]).expect("scan");
    let _ = std::fs::remove_dir_all(&path);
    cells
}

/// Assert that after a restart the persisted keys range in the language order of the scalars,
/// and every stored value round-trips its bytes unchanged.
fn assert_restart_order(tag: &str, scrambled: Vec<KeyScalar>) {
    let mut language_sorted = scrambled.clone();
    language_sorted.sort();
    let expected_keys: Vec<Vec<u8>> = language_sorted.iter().map(encode_key_value).collect();

    let cells = round_trip(tag, &scrambled);
    let scanned_keys: Vec<Vec<u8>> = cells.iter().map(|(key, _)| key.clone()).collect();
    assert_eq!(
        scanned_keys, expected_keys,
        "the native scan order after a restart must be the language temporal order",
    );
    for (key, value) in &cells {
        assert_eq!(
            key, value,
            "a stored value must round-trip unchanged across a restart"
        );
    }
}

#[test]
fn instant_keys_and_values_are_restart_stable_in_language_order() {
    assert_restart_order(
        "instant",
        [
            0i128,
            -1,
            1,
            1_500_000_000_000_000_000,
            -86_400_000_000_000,
            42,
        ]
        .into_iter()
        .map(KeyScalar::Instant)
        .collect(),
    );
}

#[test]
fn date_keys_are_restart_stable_in_language_order() {
    assert_restart_order(
        "date",
        [0i32, -1, 1, 20_650, -719_162, 3]
            .into_iter()
            .map(KeyScalar::Date)
            .collect(),
    );
}

#[test]
fn duration_keys_are_restart_stable_in_language_order() {
    assert_restart_order(
        "duration",
        [0i128, -1, 1, 90_000_000_000, -90_000_000_000, 7]
            .into_iter()
            .map(KeyScalar::Duration)
            .collect(),
    );
}
