//! The reusable kernel operation-trace differential (design §I, finding 14).
//!
//! One typed trace of durable operations plus their expected outcomes is replayed
//! over *both* engines — the in-memory engine (the differential proving ground) and
//! the redb-backed engine (the production oracle). Each op's outcome and the final
//! logical state (a full iteration dump) must agree across the two backends, and the
//! transcript must match the expected outcomes. The real cross-process restart test
//! lives with the CLI; this proves the two engine stacks compute the same algebra.

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    CommitResult, CreateOutcome, DemandCoverage, Durable, DurableStore, EntryValue, FieldSchema,
    InvocationGrant, NextKey, Presence, Reopen, ReplaceOutcome, SiteSpec, SiteTarget, StoreSchema,
};
use marrow_store::{ByteEngine, MemoryEngine, NativeEngine};

// --- test scaffolding ---

struct TempDir {
    root: std::path::PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-kernel-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
    fn store(&self) -> std::path::PathBuf {
        self.root.join("store")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn schema() -> StoreSchema {
    StoreSchema {
        root_name: "counters".into(),
        key: ScalarKind::Str,
        fields: vec![
            FieldSchema {
                name: "value".into(),
                kind: ScalarKind::Int,
                required: true,
            },
            FieldSchema {
                name: "label".into(),
                kind: ScalarKind::Str,
                required: false,
            },
        ],
        branches: Vec::new(),
    }
}

fn sites() -> Vec<SiteSpec> {
    vec![
        SiteSpec {
            target: SiteTarget::WholePayload,
        },
        SiteSpec {
            target: SiteTarget::FieldLeaf(0),
        },
        SiteSpec {
            target: SiteTarget::FieldLeaf(1),
        },
    ]
}

const ENTRY: u16 = 0;
const VALUE: u16 = 1;
const LABEL: u16 = 2;

fn key(name: &str) -> KeyScalar {
    KeyScalar::Str(name.into())
}

fn entry(value: i64, label: Option<&str>) -> EntryValue {
    EntryValue {
        fields: vec![
            Some(RuntimeScalar::Int(value)),
            label.map(|text| RuntimeScalar::Str(text.into())),
        ],
    }
}

fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

fn read() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: false,
    }
}

/// A full logical dump: every key in iteration order with its value and label.
type Dump = Vec<(String, Option<i64>, Option<String>)>;

/// Replay the shared trace on `store`, returning the per-op transcript and the final
/// logical dump. The trace exercises create/replace/erase outcomes, sparse set and
/// clear, and cursor-edge iteration keys.
fn replay<E: ByteEngine>(mut store: DurableStore<E>) -> (Vec<String>, Dump) {
    let mut transcript = Vec::new();
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn session");
        let e = txn.site(ENTRY);
        let value = txn.site(VALUE);
        let label = txn.site(LABEL);

        // create outcomes: fresh vs already-present.
        transcript.push(format!(
            "create a = {:?}",
            txn.create_entry(&e, &[key("a")], entry(1, None)).unwrap()
        ));
        transcript.push(format!(
            "create a again = {:?}",
            txn.create_entry(&e, &[key("a")], entry(9, None)).unwrap()
        ));
        // prefix-related and edge keys.
        for name in ["", "ab", "a\u{0}"] {
            let outcome = txn.create_entry(&e, &[key(name)], entry(2, None)).unwrap();
            assert_eq!(outcome, CreateOutcome::Created);
        }
        // replace outcomes: present vs missing.
        transcript.push(format!(
            "replace a = {:?}",
            txn.replace_entry(&e, &[key("a")], entry(5, Some("first")))
                .unwrap()
        ));
        transcript.push(format!(
            "replace ghost = {:?}",
            txn.replace_entry(&e, &[key("ghost")], entry(0, None))
                .unwrap()
        ));
        // sparse set then clear.
        txn.set_sparse(
            &label,
            &[key("ab")],
            Some(RuntimeScalar::Str("mark".into())),
        )
        .unwrap();
        txn.set_sparse(&label, &[key("ab")], None).unwrap();
        // required field update.
        txn.set_required(&value, &[key("ab")], RuntimeScalar::Int(20))
            .unwrap();
        // erase outcomes.
        transcript.push(format!(
            "erase entry empty-key = {:?}",
            txn.erase_entry(&e, &[key("")]).unwrap()
        ));
        transcript.push(format!(
            "erase entry ghost = {:?}",
            txn.erase_entry(&e, &[key("ghost")]).unwrap()
        ));
        transcript.push(format!("commit = {:?}", txn.commit()));
    }

    // Read phase: presence and reads observed on a pinned snapshot.
    let dump = {
        let mut reader = store
            .read_session(InvocationGrant::full_store(), read())
            .expect("read session");
        let e = reader.site(ENTRY);
        let value = reader.site(VALUE);
        let label = reader.site(LABEL);
        transcript.push(format!(
            "presence a = {:?}",
            reader.presence(&e, &[key("a")]).unwrap()
        ));
        transcript.push(format!(
            "presence empty = {:?}",
            reader.presence(&e, &[key("")]).unwrap()
        ));

        let mut dump: Dump = Vec::new();
        let mut cursor = None;
        while let NextKey::Next(k) = reader.next_key(&e, cursor.clone()).unwrap() {
            let name = match &k {
                KeyScalar::Str(name) => name.clone(),
                other => panic!("unexpected key kind {other:?}"),
            };
            let v = reader
                .read_field(&value, std::slice::from_ref(&k))
                .unwrap()
                .map(|s| match s {
                    RuntimeScalar::Int(v) => v,
                    other => panic!("unexpected value {other:?}"),
                });
            let l = reader
                .read_field(&label, std::slice::from_ref(&k))
                .unwrap()
                .map(|s| match s {
                    RuntimeScalar::Str(s) => s,
                    other => panic!("unexpected label {other:?}"),
                });
            dump.push((name, v, l));
            cursor = Some(k);
        }
        dump
    };
    (transcript, dump)
}

#[test]
fn memory_and_redb_agree_on_the_operation_trace() {
    let (mem_transcript, mem_dump) = replay(DurableStore::from_engine(
        MemoryEngine::new(),
        schema(),
        sites(),
    ));

    let temp = TempDir::new("optrace");
    let native = NativeEngine::open(&temp.store()).expect("open native");
    let (redb_transcript, redb_dump) = replay(DurableStore::from_engine(native, schema(), sites()));

    assert_eq!(
        mem_transcript, redb_transcript,
        "the two backends disagree on outcomes"
    );
    assert_eq!(
        mem_dump, redb_dump,
        "the two backends disagree on final state"
    );

    // The transcript itself is frozen: outcomes and the algebra, not paths.
    assert_eq!(
        mem_transcript,
        vec![
            "create a = Created".to_string(),
            "create a again = AlreadyPresent".to_string(),
            "replace a = Replaced".to_string(),
            "replace ghost = Missing".to_string(),
            "erase entry empty-key = Erased".to_string(),
            "erase entry ghost = Missing".to_string(),
            "commit = Committed".to_string(),
            "presence a = Present".to_string(),
            "presence empty = Absent".to_string(),
        ]
    );
    // Final state: "a" replaced (value 5, label first); "ab" value updated to 20,
    // label cleared; "a\0" left at 2. Iteration order is ascending key order.
    assert_eq!(
        mem_dump,
        vec![
            ("a".to_string(), Some(5), Some("first".to_string())),
            ("a\u{0}".to_string(), Some(2), None),
            ("ab".to_string(), Some(20), None),
        ]
    );
}

/// The strict present-entry sparse set (`set_sparse_present`) sets a leaf of an
/// entry the caller proved present. Over both engines it must set the leaf exactly
/// like `set_sparse` and leave the same final state; on an absent marker it faults
/// `Corruption` (the marker law: a leaf never implies an absent entry into being).
#[test]
fn set_sparse_present_agrees_across_engines() {
    fn probe<E: ByteEngine>(mut store: DurableStore<E>) -> (Presence, Dump) {
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn");
            let e = txn.site(ENTRY);
            let label = txn.site(LABEL);
            txn.create_entry(&e, &[key("p")], entry(7, None)).unwrap();
            // The entry is present in the staged view, so the strict set assumes the
            // marker and writes the leaf.
            txn.set_sparse_present(
                &label,
                &[key("p")],
                Some(RuntimeScalar::Str("strict".into())),
            )
            .unwrap();
            // A strict clear of a present entry removes the leaf without touching the
            // marker.
            txn.create_entry(&e, &[key("q")], entry(8, Some("x")))
                .unwrap();
            txn.set_sparse_present(&label, &[key("q")], None).unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut reader = store
            .read_session(InvocationGrant::full_store(), read())
            .expect("read");
        let e = reader.site(ENTRY);
        let value = reader.site(VALUE);
        let label = reader.site(LABEL);
        let presence = reader.presence(&e, &[key("p")]).unwrap();
        let mut dump: Dump = Vec::new();
        let mut cursor = None;
        while let NextKey::Next(k) = reader.next_key(&e, cursor.clone()).unwrap() {
            let name = match &k {
                KeyScalar::Str(name) => name.clone(),
                other => panic!("unexpected key kind {other:?}"),
            };
            let v = reader
                .read_field(&value, std::slice::from_ref(&k))
                .unwrap()
                .map(|s| match s {
                    RuntimeScalar::Int(v) => v,
                    other => panic!("unexpected value {other:?}"),
                });
            let l = reader
                .read_field(&label, std::slice::from_ref(&k))
                .unwrap()
                .map(|s| match s {
                    RuntimeScalar::Str(s) => s,
                    other => panic!("unexpected label {other:?}"),
                });
            dump.push((name, v, l));
            cursor = Some(k);
        }
        (presence, dump)
    }

    let (mem_presence, mem_dump) = probe(DurableStore::from_engine(
        MemoryEngine::new(),
        schema(),
        sites(),
    ));
    let temp = TempDir::new("strict");
    let native = NativeEngine::open(&temp.store()).expect("open native");
    let (redb_presence, redb_dump) = probe(DurableStore::from_engine(native, schema(), sites()));

    assert_eq!(mem_presence, redb_presence);
    assert_eq!(mem_dump, redb_dump, "backends disagree on strict-set state");
    assert_eq!(
        mem_dump,
        vec![
            ("p".to_string(), Some(7), Some("strict".to_string())),
            ("q".to_string(), Some(8), None),
        ]
    );
}

/// A strict set whose entry marker is absent is corruption, never implicit
/// creation. The compiler's presence proof makes this unreachable; the kernel
/// asserts it as defense in depth.
#[test]
fn set_sparse_present_on_an_absent_marker_is_corruption() {
    let mut store = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn");
    let label = txn.site(LABEL);
    assert_eq!(
        txn.set_sparse_present(
            &label,
            &[key("missing")],
            Some(RuntimeScalar::Str("x".into()))
        ),
        Err(marrow_kernel::durable::KernelFault::Corruption)
    );
}

#[test]
fn rollback_discards_staged_writes_on_both_backends() {
    fn probe<E: ByteEngine>(mut store: DurableStore<E>) -> Presence {
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn");
            let e = txn.site(ENTRY);
            txn.create_entry(&e, &[key("z")], entry(1, None)).unwrap();
            // Drop without commit: the transaction rolls back.
        }
        let mut reader = store
            .read_session(InvocationGrant::full_store(), read())
            .expect("read");
        let e = reader.site(ENTRY);
        reader.presence(&e, &[key("z")]).unwrap()
    }
    assert_eq!(
        probe(DurableStore::from_engine(
            MemoryEngine::new(),
            schema(),
            sites()
        )),
        Presence::Absent
    );
    let temp = TempDir::new("rollback");
    let native = NativeEngine::open(&temp.store()).expect("open native");
    assert_eq!(
        probe(DurableStore::from_engine(native, schema(), sites())),
        Presence::Absent
    );
}

#[test]
fn required_missing_commit_agrees_on_both_backends() {
    fn probe<E: ByteEngine>(mut store: DurableStore<E>) -> bool {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn");
        let label = txn.site(LABEL);
        // Stage only the sparse label on a fresh entry; the required value is unset.
        txn.set_sparse(&label, &[key("x")], Some(RuntimeScalar::Str("hi".into())))
            .unwrap();
        matches!(txn.commit(), CommitResult::RequiredMissing { .. })
    }
    assert!(probe(DurableStore::from_engine(
        MemoryEngine::new(),
        schema(),
        sites()
    )));
    let temp = TempDir::new("required-missing");
    let native = NativeEngine::open(&temp.store()).expect("open native");
    assert!(probe(DurableStore::from_engine(native, schema(), sites())));
}

#[test]
fn witness_classifies_a_reopen_as_complete_new() {
    // A committed transaction records its witness; reopening with that token
    // classifies the store as complete-new, and any other token as complete-old.
    let temp = TempDir::new("witness");
    let token = {
        let native = NativeEngine::open(&temp.store()).expect("open native");
        let mut store = DurableStore::from_engine(native, schema(), sites());
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn");
        let e = txn.site(ENTRY);
        txn.create_entry(&e, &[key("k")], entry(1, None)).unwrap();
        let token = txn.token();
        assert_eq!(txn.commit(), CommitResult::Committed);
        token
    };
    // Reopen in a fresh handle and classify.
    let native = NativeEngine::open(&temp.store()).expect("reopen native");
    let store = DurableStore::from_engine(native, schema(), sites());
    assert_eq!(store.classify(token).unwrap(), Reopen::CompleteNew);
    assert_eq!(store.classify([0u8; 16]).unwrap(), Reopen::CompleteOld);
}

#[test]
fn a_replaced_entry_drops_unlisted_sparse_leaves() {
    // Exact replacement: a replace with no label removes a previously set label.
    fn probe<E: ByteEngine>(mut store: DurableStore<E>) -> Option<String> {
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn");
            let e = txn.site(ENTRY);
            txn.create_entry(&e, &[key("k")], entry(1, Some("keep")))
                .unwrap();
            assert_eq!(
                txn.replace_entry(&e, &[key("k")], entry(2, None)).unwrap(),
                ReplaceOutcome::Replaced
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut reader = store
            .read_session(InvocationGrant::full_store(), read())
            .expect("read");
        let label = reader.site(LABEL);
        reader
            .read_field(&label, &[key("k")])
            .unwrap()
            .map(|s| match s {
                RuntimeScalar::Str(s) => s,
                other => panic!("unexpected {other:?}"),
            })
    }
    assert_eq!(
        probe(DurableStore::from_engine(
            MemoryEngine::new(),
            schema(),
            sites()
        )),
        None
    );
    let temp = TempDir::new("replace-drops");
    let native = NativeEngine::open(&temp.store()).expect("open native");
    assert_eq!(
        probe(DurableStore::from_engine(native, schema(), sites())),
        None
    );
}
