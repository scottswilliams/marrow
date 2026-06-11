//! Private conformance checks for ordered-byte backend implementors.

use crate::backend::Backend;

pub(crate) fn run_all<B: Backend>(mut make: impl FnMut() -> B) {
    values_round_trip(&mut make());
    delete_removes_the_prefix_subtree(&mut make());
    delete_of_an_absent_prefix_is_a_no_op(&mut make());
    scan_returns_only_the_prefix_in_order(&mut make());
    scan_is_bounded_by_the_limit(&mut make());
    scan_after_resumes_inside_the_prefix(&mut make());
    scan_before_resumes_inside_the_prefix_in_reverse(&mut make());
    a_committed_transaction_keeps_its_writes(&mut make());
    a_rolled_back_transaction_discards_its_writes(&mut make());
    an_unbalanced_commit_or_rollback_is_a_no_op(&mut make());
    nested_transactions_are_savepoints(&mut make());
    an_inner_commit_then_outer_rollback_discards_everything(&mut make());
    three_level_nesting_with_a_middle_commit_and_outer_rollback(&mut make());
    a_transaction_sees_its_writes_in_scans(&mut make());
    a_snapshot_pins_one_consistent_view(&mut make());
    a_snapshot_and_write_transaction_cannot_overlap(&mut make());
    read_snapshots_are_not_reentrant(&mut make());
    writes_are_rejected_while_a_read_snapshot_is_pinned(&mut make());
}

fn values_round_trip(store: &mut dyn Backend) {
    assert_eq!(store.read(b"\x10key").unwrap(), None);
    store.write(b"\x10key", b"draft".to_vec()).unwrap();
    assert_eq!(store.read(b"\x10key").unwrap(), Some(b"draft".to_vec()));
    store.write(b"\x10key", b"final".to_vec()).unwrap();
    assert_eq!(store.read(b"\x10key").unwrap(), Some(b"final".to_vec()));
}

fn delete_removes_the_prefix_subtree(store: &mut dyn Backend) {
    store.write(b"\x20a", b"node".to_vec()).unwrap();
    store.write(b"\x20a\x01", b"left".to_vec()).unwrap();
    store.write(b"\x20b\x01", b"right".to_vec()).unwrap();
    store.delete(b"\x20a").unwrap();
    assert_eq!(store.read(b"\x20a").unwrap(), None);
    assert_eq!(store.read(b"\x20a\x01").unwrap(), None);
    assert_eq!(store.read(b"\x20b\x01").unwrap(), Some(b"right".to_vec()));
}

fn delete_of_an_absent_prefix_is_a_no_op(store: &mut dyn Backend) {
    store.write(b"\x20b\x01", b"right".to_vec()).unwrap();
    store.delete(b"\x20a").unwrap();
    assert_eq!(store.read(b"\x20b\x01").unwrap(), Some(b"right".to_vec()));
}

fn scan_returns_only_the_prefix_in_order(store: &mut dyn Backend) {
    store.write(b"\x30\x02", b"second".to_vec()).unwrap();
    store.write(b"\x30\x01", b"first".to_vec()).unwrap();
    store.write(b"\x31\x01", b"outside".to_vec()).unwrap();
    let page = store.scan(b"\x30", 10).unwrap();
    assert!(!page.truncated);
    assert_eq!(
        page.entries,
        vec![
            (b"\x30\x01".to_vec(), b"first".to_vec()),
            (b"\x30\x02".to_vec(), b"second".to_vec()),
        ]
    );
}

fn scan_is_bounded_by_the_limit(store: &mut dyn Backend) {
    store.write(b"\x40\x01", b"first".to_vec()).unwrap();
    store.write(b"\x40\x02", b"second".to_vec()).unwrap();
    let page = store.scan(b"\x40", 1).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert!(page.truncated);
}

fn scan_after_resumes_inside_the_prefix(store: &mut dyn Backend) {
    store.write(b"\x50\x01", b"first".to_vec()).unwrap();
    store.write(b"\x50\x02", b"second".to_vec()).unwrap();
    store.write(b"\x51\x01", b"outside".to_vec()).unwrap();

    let first = store.scan(b"\x50", 1).unwrap();
    assert!(first.truncated);
    let cursor = first.entries.last().unwrap().0.clone();
    let second = store.scan_after(b"\x50", &cursor, 10).unwrap();
    assert!(!second.truncated);
    assert_eq!(
        second.entries,
        vec![(b"\x50\x02".to_vec(), b"second".to_vec())]
    );
}

fn scan_before_resumes_inside_the_prefix_in_reverse(store: &mut dyn Backend) {
    store.write(b"\x50\x01", b"first".to_vec()).unwrap();
    store.write(b"\x50\x02", b"second".to_vec()).unwrap();
    store.write(b"\x51\x01", b"outside".to_vec()).unwrap();

    let first = store.scan_before(b"\x50", b"\x51", 1).unwrap();
    assert!(first.truncated);
    assert_eq!(
        first.entries,
        vec![(b"\x50\x02".to_vec(), b"second".to_vec())]
    );
    let cursor = first.entries.last().unwrap().0.clone();
    let second = store.scan_before(b"\x50", &cursor, 10).unwrap();
    assert!(!second.truncated);
    assert_eq!(
        second.entries,
        vec![(b"\x50\x01".to_vec(), b"first".to_vec())]
    );
}

fn a_committed_transaction_keeps_its_writes(store: &mut dyn Backend) {
    store.begin().unwrap();
    store.write(b"k", b"v".to_vec()).unwrap();
    store.commit().unwrap();
    assert_eq!(store.read(b"k").unwrap(), Some(b"v".to_vec()));
}

fn a_rolled_back_transaction_discards_its_writes(store: &mut dyn Backend) {
    store.write(b"k", b"old".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"k", b"new".to_vec()).unwrap();
    store.write(b"temp", b"gone".to_vec()).unwrap();
    store.rollback().unwrap();
    assert_eq!(store.read(b"k").unwrap(), Some(b"old".to_vec()));
    assert_eq!(store.read(b"temp").unwrap(), None);
}

fn an_unbalanced_commit_or_rollback_is_a_no_op(store: &mut dyn Backend) {
    store.write(b"k", b"v".to_vec()).unwrap();
    store.commit().unwrap();
    assert_eq!(store.read(b"k").unwrap(), Some(b"v".to_vec()));
    store.rollback().unwrap();
    assert_eq!(store.read(b"k").unwrap(), Some(b"v".to_vec()));
}

fn nested_transactions_are_savepoints(store: &mut dyn Backend) {
    store.write(b"k", b"base".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"k", b"outer".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"k", b"inner".to_vec()).unwrap();
    store.rollback().unwrap();
    assert_eq!(store.read(b"k").unwrap(), Some(b"outer".to_vec()));
    store.commit().unwrap();
    assert_eq!(store.read(b"k").unwrap(), Some(b"outer".to_vec()));
}

fn an_inner_commit_then_outer_rollback_discards_everything(store: &mut dyn Backend) {
    store.write(b"k", b"base".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"k", b"outer".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"inner", b"value".to_vec()).unwrap();
    store.commit().unwrap();
    assert_eq!(store.read(b"inner").unwrap(), Some(b"value".to_vec()));
    store.rollback().unwrap();
    assert_eq!(store.read(b"k").unwrap(), Some(b"base".to_vec()));
    assert_eq!(store.read(b"inner").unwrap(), None);
}

fn three_level_nesting_with_a_middle_commit_and_outer_rollback(store: &mut dyn Backend) {
    store.begin().unwrap();
    store.write(b"outer", b"1".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"middle", b"2".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"inner", b"3".to_vec()).unwrap();
    store.rollback().unwrap();
    assert_eq!(store.read(b"inner").unwrap(), None);
    store.commit().unwrap();
    assert_eq!(store.read(b"middle").unwrap(), Some(b"2".to_vec()));
    store.rollback().unwrap();
    assert_eq!(store.read(b"outer").unwrap(), None);
    assert_eq!(store.read(b"middle").unwrap(), None);
}

fn a_transaction_sees_its_writes_in_scans(store: &mut dyn Backend) {
    store.begin().unwrap();
    store.write(b"\x60\x01", b"inside".to_vec()).unwrap();
    assert_eq!(
        store.scan(b"\x60", 10).unwrap().entries,
        vec![(b"\x60\x01".to_vec(), b"inside".to_vec())]
    );
    store.rollback().unwrap();
}

fn a_snapshot_pins_one_consistent_view(store: &mut dyn Backend) {
    store.write(b"\x70\x01", b"before".to_vec()).unwrap();
    store.begin_snapshot().unwrap();
    assert_eq!(store.read(b"\x70\x01").unwrap(), Some(b"before".to_vec()));
    assert_eq!(store.read(b"\x70\x02").unwrap(), None);
    assert_eq!(
        store.scan(b"\x70", 10).unwrap().entries,
        vec![(b"\x70\x01".to_vec(), b"before".to_vec())]
    );
    store.end_snapshot();

    store.write(b"\x70\x01", b"after".to_vec()).unwrap();
    store.write(b"\x70\x02", b"added".to_vec()).unwrap();
    assert_eq!(store.read(b"\x70\x01").unwrap(), Some(b"after".to_vec()));
    assert_eq!(
        store.scan(b"\x70", 10).unwrap().entries,
        vec![
            (b"\x70\x01".to_vec(), b"after".to_vec()),
            (b"\x70\x02".to_vec(), b"added".to_vec()),
        ]
    );
}

fn a_snapshot_and_write_transaction_cannot_overlap(store: &mut dyn Backend) {
    store.begin_snapshot().unwrap();
    let begin = store
        .begin()
        .expect_err("begin must reject an already pinned snapshot");
    assert_eq!(begin.code(), "store.transaction");
    store.end_snapshot();

    store.begin().unwrap();
    let snapshot = store
        .begin_snapshot()
        .expect_err("begin_snapshot must reject an open write transaction");
    assert_eq!(snapshot.code(), "store.transaction");
    store.rollback().unwrap();
}

fn read_snapshots_are_not_reentrant(store: &mut dyn Backend) {
    store.write(b"\x80\x01", b"before".to_vec()).unwrap();
    store.begin_snapshot().unwrap();
    let nested = store
        .begin_snapshot()
        .expect_err("a second pinned snapshot on the same handle must be rejected");
    assert_eq!(nested.code(), "store.transaction");
    let begin = store
        .begin()
        .expect_err("the original snapshot still blocks a write transaction");
    assert_eq!(begin.code(), "store.transaction");
    store.end_snapshot();

    store.begin().unwrap();
    store.write(b"\x80\x01", b"after".to_vec()).unwrap();
    store.commit().unwrap();
    assert_eq!(store.read(b"\x80\x01").unwrap(), Some(b"after".to_vec()));
}

fn writes_are_rejected_while_a_read_snapshot_is_pinned(store: &mut dyn Backend) {
    store.write(b"\x90\x01", b"before".to_vec()).unwrap();
    store.begin_snapshot().unwrap();
    let write = store
        .write(b"\x90\x01", b"after".to_vec())
        .expect_err("autocommit writes must reject a pinned read snapshot");
    assert_eq!(write.code(), "store.transaction");
    let delete = store
        .delete(b"\x90")
        .expect_err("autocommit deletes must reject a pinned read snapshot");
    assert_eq!(delete.code(), "store.transaction");
    assert_eq!(store.read(b"\x90\x01").unwrap(), Some(b"before".to_vec()));
    store.end_snapshot();

    store.write(b"\x90\x01", b"after".to_vec()).unwrap();
    assert_eq!(store.read(b"\x90\x01").unwrap(), Some(b"after".to_vec()));
}
