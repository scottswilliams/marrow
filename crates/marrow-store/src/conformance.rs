//! Private conformance checks for ordered-byte backend implementors.

use crate::backend::{Backend, StoreError};

pub(crate) fn run_all<B: Backend>(
    mut make: impl FnMut() -> Result<B, StoreError>,
) -> Result<(), StoreError> {
    values_round_trip(&mut make()?)?;
    delete_removes_the_prefix_subtree(&mut make()?)?;
    delete_of_an_absent_prefix_is_a_no_op(&mut make()?)?;
    scan_returns_only_the_prefix_in_order(&mut make()?)?;
    scan_is_bounded_by_the_limit(&mut make()?)?;
    scan_after_resumes_inside_the_prefix(&mut make()?)?;
    scan_before_resumes_inside_the_prefix_in_reverse(&mut make()?)?;
    bounded_scan_returns_only_entries_between_prefix_bounds(&mut make()?)?;
    bounded_reverse_scan_returns_only_entries_between_prefix_bounds(&mut make()?)?;
    a_committed_transaction_keeps_its_writes(&mut make()?)?;
    a_rolled_back_transaction_discards_its_writes(&mut make()?)?;
    an_unbalanced_commit_or_rollback_is_a_no_op(&mut make()?)?;
    a_joined_transaction_commit_waits_for_the_outer_commit(&mut make()?)?;
    rollback_of_a_joined_transaction_aborts_the_whole_transaction(&mut make()?)?;
    a_transaction_sees_its_writes_in_scans(&mut make()?)?;
    a_snapshot_pins_one_consistent_view(&mut make()?)?;
    a_snapshot_and_write_transaction_cannot_overlap(&mut make()?)?;
    read_snapshots_are_not_reentrant(&mut make()?)?;
    writes_are_rejected_while_a_read_snapshot_is_pinned(&mut make()?)?;
    Ok(())
}

fn values_round_trip(store: &mut dyn Backend) -> Result<(), StoreError> {
    assert_eq!(store.read(b"\x10key")?, None);
    store.write(b"\x10key", b"draft".to_vec())?;
    assert_eq!(store.read(b"\x10key")?, Some(b"draft".to_vec()));
    store.write(b"\x10key", b"final".to_vec())?;
    assert_eq!(store.read(b"\x10key")?, Some(b"final".to_vec()));
    Ok(())
}

fn delete_removes_the_prefix_subtree(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"\x20a", b"node".to_vec())?;
    store.write(b"\x20a\x01", b"left".to_vec())?;
    store.write(b"\x20b\x01", b"right".to_vec())?;
    store.delete(b"\x20a")?;
    assert_eq!(store.read(b"\x20a")?, None);
    assert_eq!(store.read(b"\x20a\x01")?, None);
    assert_eq!(store.read(b"\x20b\x01")?, Some(b"right".to_vec()));
    Ok(())
}

fn delete_of_an_absent_prefix_is_a_no_op(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"\x20b\x01", b"right".to_vec())?;
    store.delete(b"\x20a")?;
    assert_eq!(store.read(b"\x20b\x01")?, Some(b"right".to_vec()));
    Ok(())
}

fn scan_returns_only_the_prefix_in_order(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"\x30\x02", b"second".to_vec())?;
    store.write(b"\x30\x01", b"first".to_vec())?;
    store.write(b"\x31\x01", b"outside".to_vec())?;
    let page = store.scan(b"\x30", 10)?;
    assert!(!page.truncated);
    assert_eq!(
        page.entries,
        vec![
            (b"\x30\x01".to_vec(), b"first".to_vec()),
            (b"\x30\x02".to_vec(), b"second".to_vec()),
        ]
    );
    Ok(())
}

fn scan_is_bounded_by_the_limit(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"\x40\x01", b"first".to_vec())?;
    store.write(b"\x40\x02", b"second".to_vec())?;
    let page = store.scan(b"\x40", 1)?;
    assert_eq!(page.entries.len(), 1);
    assert!(page.truncated);
    Ok(())
}

fn scan_after_resumes_inside_the_prefix(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"\x50\x01", b"first".to_vec())?;
    store.write(b"\x50\x02", b"second".to_vec())?;
    store.write(b"\x51\x01", b"outside".to_vec())?;

    let first = store.scan(b"\x50", 1)?;
    assert!(first.truncated);
    assert_eq!(
        first.entries,
        vec![(b"\x50\x01".to_vec(), b"first".to_vec())]
    );
    let cursor = first.entries[0].0.clone();
    let second = store.scan_after(b"\x50", &cursor, 10)?;
    assert!(!second.truncated);
    assert_eq!(
        second.entries,
        vec![(b"\x50\x02".to_vec(), b"second".to_vec())]
    );
    Ok(())
}

fn scan_before_resumes_inside_the_prefix_in_reverse(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.write(b"\x50\x01", b"first".to_vec())?;
    store.write(b"\x50\x02", b"second".to_vec())?;
    store.write(b"\x51\x01", b"outside".to_vec())?;

    let first = store.scan_before(b"\x50", b"\x51", 1)?;
    assert!(first.truncated);
    assert_eq!(
        first.entries,
        vec![(b"\x50\x02".to_vec(), b"second".to_vec())]
    );
    let cursor = first.entries[0].0.clone();
    let second = store.scan_before(b"\x50", &cursor, 10)?;
    assert!(!second.truncated);
    assert_eq!(
        second.entries,
        vec![(b"\x50\x01".to_vec(), b"first".to_vec())]
    );
    Ok(())
}

fn bounded_scan_returns_only_entries_between_prefix_bounds(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.write(b"\x58\x01", b"below".to_vec())?;
    store.write(b"\x58\x02", b"first".to_vec())?;
    store.write(b"\x58\x03", b"second".to_vec())?;
    store.write(b"\x58\x04", b"above".to_vec())?;
    store.write(b"\x59\x02", b"outside".to_vec())?;

    let page = store.scan_between(b"\x58", Some(b"\x58\x02"), Some(b"\x58\x04"), 10)?;
    assert!(!page.truncated);
    assert_eq!(
        page.entries,
        vec![
            (b"\x58\x02".to_vec(), b"first".to_vec()),
            (b"\x58\x03".to_vec(), b"second".to_vec()),
        ]
    );
    Ok(())
}

fn bounded_reverse_scan_returns_only_entries_between_prefix_bounds(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.write(b"\x59\x01", b"below".to_vec())?;
    store.write(b"\x59\x02", b"first".to_vec())?;
    store.write(b"\x59\x03", b"second".to_vec())?;
    store.write(b"\x59\x04", b"above".to_vec())?;
    store.write(b"\x5a\x02", b"outside".to_vec())?;

    let first = store.scan_between_before(
        b"\x59",
        Some(b"\x59\x02"),
        Some(b"\x59\x04"),
        b"\x59\x04",
        1,
    )?;
    assert!(first.truncated);
    assert_eq!(
        first.entries,
        vec![(b"\x59\x03".to_vec(), b"second".to_vec())]
    );

    let cursor = first.entries[0].0.clone();
    let second =
        store.scan_between_before(b"\x59", Some(b"\x59\x02"), Some(b"\x59\x04"), &cursor, 10)?;
    assert!(!second.truncated);
    assert_eq!(
        second.entries,
        vec![(b"\x59\x02".to_vec(), b"first".to_vec())]
    );
    Ok(())
}

fn a_committed_transaction_keeps_its_writes(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.begin()?;
    store.write(b"k", b"v".to_vec())?;
    store.commit()?;
    assert_eq!(store.read(b"k")?, Some(b"v".to_vec()));
    Ok(())
}

fn a_rolled_back_transaction_discards_its_writes(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.write(b"k", b"old".to_vec())?;
    store.begin()?;
    store.write(b"k", b"new".to_vec())?;
    store.write(b"temp", b"gone".to_vec())?;
    store.rollback()?;
    assert_eq!(store.read(b"k")?, Some(b"old".to_vec()));
    assert_eq!(store.read(b"temp")?, None);
    Ok(())
}

fn an_unbalanced_commit_or_rollback_is_a_no_op(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"k", b"v".to_vec())?;
    store.commit()?;
    assert_eq!(store.read(b"k")?, Some(b"v".to_vec()));
    store.rollback()?;
    assert_eq!(store.read(b"k")?, Some(b"v".to_vec()));
    Ok(())
}

fn a_joined_transaction_commit_waits_for_the_outer_commit(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.write(b"k", b"base".to_vec())?;
    store.begin()?;
    store.write(b"k", b"outer".to_vec())?;
    store.begin()?;
    store.write(b"inner", b"value".to_vec())?;
    store.commit()?;
    assert_eq!(store.read(b"inner")?, Some(b"value".to_vec()));
    store.rollback()?;
    assert_eq!(store.read(b"k")?, Some(b"base".to_vec()));
    assert_eq!(store.read(b"inner")?, None);
    Ok(())
}

fn rollback_of_a_joined_transaction_aborts_the_whole_transaction(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.begin()?;
    store.write(b"outer", b"1".to_vec())?;
    store.begin()?;
    store.write(b"inner", b"3".to_vec())?;
    store.rollback()?;
    assert_eq!(store.read(b"outer")?, None);
    assert_eq!(store.read(b"inner")?, None);
    store.commit()?;
    assert_eq!(store.read(b"outer")?, None);
    assert_eq!(store.read(b"inner")?, None);
    Ok(())
}

fn a_transaction_sees_its_writes_in_scans(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.begin()?;
    store.write(b"\x60\x01", b"inside".to_vec())?;
    assert_eq!(
        store.scan(b"\x60", 10)?.entries,
        vec![(b"\x60\x01".to_vec(), b"inside".to_vec())]
    );
    store.rollback()?;
    Ok(())
}

fn a_snapshot_pins_one_consistent_view(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"\x70\x01", b"before".to_vec())?;
    store.begin_snapshot()?;
    assert_eq!(store.read(b"\x70\x01")?, Some(b"before".to_vec()));
    assert_eq!(store.read(b"\x70\x02")?, None);
    assert_eq!(
        store.scan(b"\x70", 10)?.entries,
        vec![(b"\x70\x01".to_vec(), b"before".to_vec())]
    );
    store.end_snapshot();

    store.write(b"\x70\x01", b"after".to_vec())?;
    store.write(b"\x70\x02", b"added".to_vec())?;
    assert_eq!(store.read(b"\x70\x01")?, Some(b"after".to_vec()));
    assert_eq!(
        store.scan(b"\x70", 10)?.entries,
        vec![
            (b"\x70\x01".to_vec(), b"after".to_vec()),
            (b"\x70\x02".to_vec(), b"added".to_vec()),
        ]
    );
    Ok(())
}

fn a_snapshot_and_write_transaction_cannot_overlap(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.begin_snapshot()?;
    let begin = store
        .begin()
        .expect_err("begin must reject an already pinned snapshot");
    assert_eq!(begin.code(), "store.transaction");
    store.end_snapshot();

    store.begin()?;
    let snapshot = store
        .begin_snapshot()
        .expect_err("begin_snapshot must reject an open write transaction");
    assert_eq!(snapshot.code(), "store.transaction");
    store.rollback()?;
    Ok(())
}

fn read_snapshots_are_not_reentrant(store: &mut dyn Backend) -> Result<(), StoreError> {
    store.write(b"\x80\x01", b"before".to_vec())?;
    store.begin_snapshot()?;
    let nested = store
        .begin_snapshot()
        .expect_err("a second pinned snapshot on the same handle must be rejected");
    assert_eq!(nested.code(), "store.transaction");
    let begin = store
        .begin()
        .expect_err("the original snapshot still blocks a write transaction");
    assert_eq!(begin.code(), "store.transaction");
    store.end_snapshot();

    store.begin()?;
    store.write(b"\x80\x01", b"after".to_vec())?;
    store.commit()?;
    assert_eq!(store.read(b"\x80\x01")?, Some(b"after".to_vec()));
    Ok(())
}

fn writes_are_rejected_while_a_read_snapshot_is_pinned(
    store: &mut dyn Backend,
) -> Result<(), StoreError> {
    store.write(b"\x90\x01", b"before".to_vec())?;
    store.begin_snapshot()?;
    let write = store
        .write(b"\x90\x01", b"after".to_vec())
        .expect_err("autocommit writes must reject a pinned read snapshot");
    assert_eq!(write.code(), "store.transaction");
    let delete = store
        .delete(b"\x90")
        .expect_err("autocommit deletes must reject a pinned read snapshot");
    assert_eq!(delete.code(), "store.transaction");
    assert_eq!(store.read(b"\x90\x01")?, Some(b"before".to_vec()));
    store.end_snapshot();

    store.write(b"\x90\x01", b"after".to_vec())?;
    assert_eq!(store.read(b"\x90\x01")?, Some(b"after".to_vec()));
    Ok(())
}
