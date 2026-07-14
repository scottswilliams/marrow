//! Conformance laws every [`ByteEngine`] implementor satisfies. One suite, run
//! against both the in-memory and the redb-backed engine, pins the narrowed
//! ordered-byte contract: point get/put/remove, the one bounded forward
//! `scan_after` (proved at the boundary minus/at/plus a key), consuming
//! transactions, batch limits, and the integrity audit.

use crate::engine::{ByteEngine, CommitOutcome, ReadView, WriteTxn, limits};
use crate::error::StoreError;

pub(crate) fn run_all<E: ByteEngine>(
    mut make: impl FnMut() -> Result<E, StoreError>,
) -> Result<(), StoreError> {
    values_round_trip(&mut make()?)?;
    a_transaction_reads_its_own_writes(&mut make()?)?;
    a_committed_transaction_persists(&mut make()?)?;
    a_dropped_transaction_aborts(&mut make()?)?;
    remove_is_exact_not_prefix(&mut make()?)?;
    remove_of_an_absent_key_is_a_no_op(&mut make()?)?;
    scan_after_is_forward_half_open_at_the_boundary(&mut make()?)?;
    scan_after_stops_at_the_prefix_edge(&mut make()?)?;
    scan_after_is_bounded_by_the_record_limit(&mut make()?)?;
    oversized_cells_are_refused(&mut make()?)?;
    a_populated_store_passes_its_integrity_audit(&mut make()?)?;
    a_model_sequence_matches_a_reference_map(&mut make()?)?;
    Ok(())
}

/// Commit `cells` into `engine`, asserting the commit confirms.
fn seed<E: ByteEngine>(engine: &mut E, cells: &[(&[u8], &[u8])]) -> Result<(), StoreError> {
    let mut txn = engine.begin()?;
    for (key, value) in cells {
        txn.put(key, value.to_vec())?;
    }
    assert_eq!(txn.commit(), CommitOutcome::Confirmed, "seed commit");
    Ok(())
}

fn keys(cells: &[(Vec<u8>, Vec<u8>)]) -> Vec<Vec<u8>> {
    cells.iter().map(|(key, _)| key.clone()).collect()
}

fn values_round_trip<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    assert_eq!(engine.read_view()?.get(b"\x10key")?, None);
    seed(engine, &[(b"\x10key", b"draft")])?;
    assert_eq!(
        engine.read_view()?.get(b"\x10key")?,
        Some(b"draft".to_vec())
    );
    seed(engine, &[(b"\x10key", b"final")])?;
    assert_eq!(
        engine.read_view()?.get(b"\x10key")?,
        Some(b"final".to_vec())
    );
    Ok(())
}

fn a_transaction_reads_its_own_writes<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    let mut txn = engine.begin()?;
    assert_eq!(txn.get(b"\x60\x01")?, None);
    txn.put(b"\x60\x01", b"staged".to_vec())?;
    assert_eq!(txn.get(b"\x60\x01")?, Some(b"staged".to_vec()));
    assert_eq!(
        keys(&txn.scan_after(b"\x60", b"\x60")?),
        vec![b"\x60\x01".to_vec()],
        "a scan inside the transaction sees its staged write"
    );
    // The write is not visible outside until commit: dropping aborts it.
    drop(txn);
    assert_eq!(engine.read_view()?.get(b"\x60\x01")?, None);
    Ok(())
}

fn a_committed_transaction_persists<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    seed(engine, &[(b"k", b"v")])?;
    assert_eq!(engine.read_view()?.get(b"k")?, Some(b"v".to_vec()));
    Ok(())
}

fn a_dropped_transaction_aborts<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    seed(engine, &[(b"k", b"old")])?;
    {
        let mut txn = engine.begin()?;
        txn.put(b"k", b"new".to_vec())?;
        txn.put(b"temp", b"gone".to_vec())?;
        // No commit: the transaction aborts on drop.
    }
    let view = engine.read_view()?;
    assert_eq!(view.get(b"k")?, Some(b"old".to_vec()));
    assert_eq!(view.get(b"temp")?, None);
    Ok(())
}

/// `remove` deletes exactly one key. A key that is a byte-prefix of the removed
/// key survives — the engine has no prefix-delete, and the kernel removes an
/// entry's cells one exact key at a time.
fn remove_is_exact_not_prefix<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    seed(
        engine,
        &[
            (b"\x20a", b"node"),
            (b"\x20a\x01", b"child"),
            (b"\x20b", b"sibling"),
        ],
    )?;
    {
        let mut txn = engine.begin()?;
        txn.remove(b"\x20a")?;
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let view = engine.read_view()?;
    assert_eq!(view.get(b"\x20a")?, None);
    assert_eq!(view.get(b"\x20a\x01")?, Some(b"child".to_vec()));
    assert_eq!(view.get(b"\x20b")?, Some(b"sibling".to_vec()));
    Ok(())
}

fn remove_of_an_absent_key_is_a_no_op<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    seed(engine, &[(b"\x20b", b"kept")])?;
    {
        let mut txn = engine.begin()?;
        txn.remove(b"\x20a")?;
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    assert_eq!(engine.read_view()?.get(b"\x20b")?, Some(b"kept".to_vec()));
    Ok(())
}

/// `scan_after` is forward and half-open: it excludes the cursor key and returns
/// the rest in ascending order. Proved at the boundary a cursor just below a key,
/// exactly at it, and just above the last key.
fn scan_after_is_forward_half_open_at_the_boundary<E: ByteEngine>(
    engine: &mut E,
) -> Result<(), StoreError> {
    seed(
        engine,
        &[
            (b"\x30\x01", b"a"),
            (b"\x30\x02", b"b"),
            (b"\x30\x03", b"c"),
        ],
    )?;
    let view = engine.read_view()?;
    // minus: a cursor just below the first key yields every cell.
    assert_eq!(
        keys(&view.scan_after(b"\x30", b"\x30\x00")?),
        vec![
            b"\x30\x01".to_vec(),
            b"\x30\x02".to_vec(),
            b"\x30\x03".to_vec()
        ],
    );
    // at: a cursor exactly at the first key excludes it (half-open).
    assert_eq!(
        keys(&view.scan_after(b"\x30", b"\x30\x01")?),
        vec![b"\x30\x02".to_vec(), b"\x30\x03".to_vec()],
    );
    // plus: a cursor at the last key yields nothing further.
    assert!(view.scan_after(b"\x30", b"\x30\x03")?.is_empty());
    Ok(())
}

fn scan_after_stops_at_the_prefix_edge<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    seed(
        engine,
        &[
            (b"\x30\x01", b"in"),
            (b"\x30\x02", b"in"),
            (b"\x31\x00", b"out"),
        ],
    )?;
    assert_eq!(
        keys(&engine.read_view()?.scan_after(b"\x30", b"\x30")?),
        vec![b"\x30\x01".to_vec(), b"\x30\x02".to_vec()],
        "the scan stops at the first key outside the prefix",
    );
    Ok(())
}

fn scan_after_is_bounded_by_the_record_limit<E: ByteEngine>(
    engine: &mut E,
) -> Result<(), StoreError> {
    let over = limits::SCAN_MAX_RECORDS + 8;
    {
        let mut txn = engine.begin()?;
        for n in 0..over {
            txn.put(format!("\x40{n:04}").as_bytes(), b"v".to_vec())?;
        }
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let page = engine.read_view()?.scan_after(b"\x40", b"\x40")?;
    assert_eq!(
        page.len(),
        limits::SCAN_MAX_RECORDS,
        "a single scan page never exceeds the record limit"
    );
    Ok(())
}

fn oversized_cells_are_refused<E: ByteEngine>(engine: &mut E) -> Result<(), StoreError> {
    let mut txn = engine.begin()?;
    let big_key = vec![0u8; limits::MAX_KEY_LEN + 1];
    assert!(matches!(
        txn.put(&big_key, b"v".to_vec()),
        Err(StoreError::LimitExceeded { .. })
    ));
    let big_value = vec![0u8; limits::MAX_VALUE_LEN + 1];
    assert!(matches!(
        txn.put(b"k", big_value),
        Err(StoreError::LimitExceeded { .. })
    ));
    Ok(())
}

fn a_populated_store_passes_its_integrity_audit<E: ByteEngine>(
    engine: &mut E,
) -> Result<(), StoreError> {
    seed(engine, &[(b"a", b"1"), (b"b", b"2"), (b"c", b"3")])?;
    engine.audit_integrity()
}

/// A deterministic sequence of puts and removes leaves the engine agreeing with a
/// plain `BTreeMap` reference model, read back both by point `get` and by paging
/// the whole key range with `scan_after`.
fn a_model_sequence_matches_a_reference_map<E: ByteEngine>(
    engine: &mut E,
) -> Result<(), StoreError> {
    use std::collections::BTreeMap;

    let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut step = 0u32;
    for round in 0..6u32 {
        let mut txn = engine.begin()?;
        for n in 0..20u32 {
            let key = format!("\x50{:03}", (step * 7 + n) % 50).into_bytes();
            if (round + n) % 5 == 0 {
                txn.remove(&key)?;
                model.remove(&key);
            } else {
                let value = format!("v{step}-{n}").into_bytes();
                txn.put(&key, value.clone())?;
                model.insert(key, value);
            }
            step += 1;
        }
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }

    let view = engine.read_view()?;
    // Point reads agree with the model.
    for (key, value) in &model {
        assert_eq!(view.get(key)?.as_ref(), Some(value));
    }
    // Paging the whole \x50 range with scan_after reconstructs the model in order.
    let mut paged: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut cursor = b"\x50".to_vec();
    loop {
        let page = view.scan_after(b"\x50", &cursor)?;
        let Some((last, _)) = page.last().cloned() else {
            break;
        };
        cursor = last;
        paged.extend(page);
    }
    let expected: Vec<(Vec<u8>, Vec<u8>)> = model.into_iter().collect();
    assert_eq!(
        paged, expected,
        "paged scan reconstructs the reference model"
    );
    Ok(())
}
