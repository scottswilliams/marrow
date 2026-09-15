//! Logical-walk controls: every classification, fault site and finding the audit reports.

use marrow_store::{ByteEngine, CommitOutcome, MemoryEngine, WriteTxn};

use super::*;
use crate::codec::key::encode_key_tuple;
use crate::codec::value::RuntimeScalar;
use crate::durable::{
    CommitResult, CreateOutcome, DemandCoverage, Durable, DurableStore, EntryValue, IndexComponent,
    InvocationGrant, SiteTarget, StoreSchemaBuilder, number_store,
};

const BY_ISBN: [u8; 16] = [0xA1; 16];
const BY_TITLE: [u8; 16] = [0xB2; 16];

/// `^books[id: string]`: a required `title`, sparse `pages` and `isbn`, a `details` group
/// with a sparse `pages`, a `notes[Int]` branch with a required `text` and a nested
/// `tags[Str]` branch with a sparse `weight`, a unique index on `isbn`, and a nonunique
/// index on `(title, id)`.
fn projection() -> StoreProjection {
    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.scalar_field("pages", ScalarKind::Int, false);
    builder.scalar_field("isbn", ScalarKind::Str, false);
    builder.open_group("details");
    builder.scalar_field("pages", ScalarKind::Int, false);
    builder.close_group();
    builder.open_branch("notes", vec![ScalarKind::Int]);
    builder.scalar_field("text", ScalarKind::Str, true);
    builder.open_branch("tags", vec![ScalarKind::Str]);
    builder.scalar_field("weight", ScalarKind::Int, false);
    builder.close_branch();
    builder.close_branch();
    builder.index(BY_ISBN, true, vec![IndexComponent::field(2)]);
    builder.index(
        BY_TITLE,
        false,
        vec![IndexComponent::field(0), IndexComponent::key(0)],
    );
    let schema = builder.finish().expect("the books schema builds");
    let mut projection = StoreProjection::builder();
    projection.root(schema);
    projection.site(0, SiteTarget::whole_payload());
    projection.site(0, SiteTarget::branch_entry(vec![0]));
    projection.site(0, SiteTarget::branch_entry(vec![0, 0]));
    projection.finish().expect("the sites resolve")
}

fn numbers() -> RootNumbering {
    number_store(&projection()).remove(0)
}

fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

fn s(text: &str) -> KeyScalar {
    KeyScalar::Str(text.into())
}

fn vs(text: &str) -> Option<ValueDomain> {
    Some(ValueDomain::Scalar(RuntimeScalar::Str(text.into())))
}

fn vi(n: i64) -> Option<ValueDomain> {
    Some(ValueDomain::Scalar(RuntimeScalar::Int(n)))
}

fn book(
    title: &str,
    pages: Option<i64>,
    isbn: Option<&str>,
    group_pages: Option<i64>,
) -> EntryValue {
    EntryValue {
        fields: vec![vs(title), pages.and_then(vi), isbn.and_then(vs)],
        groups: vec![EntryValue {
            fields: vec![group_pages.and_then(vi)],
            groups: Vec::new(),
        }],
    }
}

/// A store populated through the kernel's own ops: two books (one with a note and a
/// tag under it), so every node kind and both indexes carry cells.
fn populated() -> DurableStore<MemoryEngine> {
    let mut store = DurableStore::from_engine(MemoryEngine::new(), projection());
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn");
        let root = txn.site(0);
        let note = txn.site(1);
        let tag = txn.site(2);
        assert_eq!(
            txn.create_entry(
                &root,
                &[s("a")],
                book("Alpha", Some(10), Some("111"), Some(3))
            )
            .expect("create"),
            CreateOutcome::Created
        );
        assert_eq!(
            txn.create_entry(&root, &[s("b")], book("Beta", None, None, None))
                .expect("create"),
            CreateOutcome::Created
        );
        assert_eq!(
            txn.create_entry(
                &note,
                &[s("a"), KeyScalar::Int(1)],
                EntryValue {
                    fields: vec![vs("first note")],
                    groups: Vec::new(),
                },
            )
            .expect("create note"),
            CreateOutcome::Created
        );
        assert_eq!(
            txn.create_entry(
                &tag,
                &[s("a"), KeyScalar::Int(1), s("red")],
                EntryValue {
                    fields: vec![vi(7)],
                    groups: Vec::new(),
                },
            )
            .expect("create tag"),
            CreateOutcome::Created
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    store
}

/// A digest that records the cell stream, so a test can compare streams and count cells.
#[derive(Default)]
struct Recording {
    cells: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ContentDigest for Recording {
    fn absorb(&mut self, key: &[u8], value: &[u8]) {
        self.cells.push((key.to_vec(), value.to_vec()));
    }
}

impl ExportSink for Recording {
    fn cell(&mut self, key: &[u8], value: &[u8]) -> std::io::Result<()> {
        self.absorb(key, value);
        Ok(())
    }
}

#[test]
fn export_keeps_the_logical_digest_and_adds_indexes_without_witnesses() {
    let store = populated();
    let (expected, expected_digest) = audit(&store);
    let mut digest = Recording::default();
    let mut sink = Recording::default();
    let report = store.export_cells(&mut digest, &mut sink).expect("export");
    assert_eq!(report, expected);
    assert!(report.is_clean());
    assert_eq!(digest.cells, expected_digest.cells);
    assert_eq!(
        sink.cells.len(),
        digest.cells.len() + report.summary.index_cells as usize
    );
    assert_eq!(sink.cells[..digest.cells.len()], digest.cells);
    assert!(sink.cells.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert!(
        sink.cells
            .iter()
            .all(|(key, _)| *key != physical::meta_key(WITNESS))
    );
}

#[test]
fn export_stops_at_the_first_sink_failure_and_preserves_its_kind() {
    struct FailingSink(usize);
    impl ExportSink for FailingSink {
        fn cell(&mut self, _: &[u8], _: &[u8]) -> std::io::Result<()> {
            self.0 += 1;
            if self.0 == 2 {
                Err(std::io::Error::from(std::io::ErrorKind::StorageFull))
            } else {
                Ok(())
            }
        }
    }
    let mut sink = FailingSink(0);
    let result = populated().export_cells(&mut Recording::default(), &mut sink);
    assert!(matches!(result, Err(ExportError::Output(error))
        if error.kind() == std::io::ErrorKind::StorageFull));
    assert_eq!(sink.0, 2);
}

#[test]
fn export_preserves_a_store_read_failure_without_calling_the_sink() {
    struct Unreadable;
    impl ReadView for Unreadable {
        fn get(&self, _: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            Err(StoreError::RecoveryRequired)
        }
        fn scan_after(&self, _: &[u8], _: &[u8]) -> Result<Vec<marrow_store::Cell>, StoreError> {
            Err(StoreError::RecoveryRequired)
        }
    }
    impl ByteEngine for Unreadable {
        type View<'a> = Unreadable;
        type Txn<'a> = <MemoryEngine as ByteEngine>::Txn<'a>;
        fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
            Ok(Unreadable)
        }
        fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
            Err(StoreError::RecoveryRequired)
        }
        fn require_write_access(&self, _: &'static str) -> Result<(), StoreError> {
            Ok(())
        }
        fn audit_integrity(&mut self) -> Result<(), StoreError> {
            Err(StoreError::RecoveryRequired)
        }
    }
    let store = DurableStore::from_engine(Unreadable, projection());
    let mut sink = Recording::default();
    let result = store.export_cells(&mut Recording::default(), &mut sink);
    assert!(matches!(
        result,
        Err(ExportError::Read(super::super::SessionError::Engine(
            StoreError::RecoveryRequired
        )))
    ));
    assert!(sink.cells.is_empty());
}

#[test]
fn export_does_not_turn_emitted_bytes_into_a_clean_report() {
    let store = tamper(populated(), |txn| {
        txn.remove(&physical::stem_field_leaf(&a_stem(), numbers().fields()[0]))
            .expect("remove required title");
    });
    let mut sink = Recording::default();
    let report = store
        .export_cells(&mut Recording::default(), &mut sink)
        .expect("output succeeded");
    assert!(!sink.cells.is_empty());
    assert!(faults(&report).contains(&AuditFault::RequiredMissing));
}

#[test]
fn restore_exported_cells_preserves_content_and_indexes_without_witnesses() {
    let source = populated();
    let mut cells = Recording::default();
    let mut before = Recording::default();
    let report = source.export_cells(&mut before, &mut cells).unwrap();
    assert!(report.is_clean());
    let mut input = cells.cells.clone().into_iter();
    let mut after = Recording::default();
    let restored = DurableStore::from_engine(MemoryEngine::new(), projection())
        .restore(|| Ok::<_, ()>(input.next()), &mut after);
    let (restored, report) = match restored {
        Ok(result) => result,
        Err(error) => panic!("restore failed: {error:?}"),
    };
    assert!(report.is_clean());
    assert_eq!(before.cells, after.cells);
    let mut output = Recording::default();
    restored
        .export_cells(&mut Recording::default(), &mut output)
        .unwrap();
    assert_eq!(output.cells, cells.cells);
    assert_eq!(
        restored
            .into_engine()
            .read_view()
            .unwrap()
            .get(&physical::meta_key(WITNESS))
            .unwrap(),
        None
    );
}

#[test]
fn restore_rejects_logically_incomplete_export_even_when_input_completes() {
    let source = tamper(populated(), |txn| {
        txn.remove(&physical::stem_field_leaf(&a_stem(), numbers().fields()[0]))
            .unwrap();
    });
    let mut cells = Recording::default();
    source
        .export_cells(&mut Recording::default(), &mut cells)
        .unwrap();
    let mut input = cells.cells.into_iter();
    let result = DurableStore::from_engine(MemoryEngine::new(), projection())
        .restore(|| Ok::<_, ()>(input.next()), &mut Recording::default());
    assert!(
        matches!(result, Err(super::super::RestoreError::Invalid(report))
        if faults(&report).contains(&AuditFault::RequiredMissing))
    );
}

fn audit(store: &DurableStore<MemoryEngine>) -> (AuditReport, Recording) {
    let mut digest = Recording::default();
    let report = store.logical_audit(&mut digest).expect("the walk reads");
    (report, digest)
}

fn raw_store(
    projection: StoreProjection,
    cells: Vec<(Vec<u8>, Vec<u8>)>,
) -> DurableStore<MemoryEngine> {
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        for (key, value) in cells {
            txn.put(&key, value).expect("raw cell");
        }
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    DurableStore::from_engine(engine, projection)
}

/// Apply raw cell edits to a populated store and reopen it under the same projection.
fn tamper(
    store: DurableStore<MemoryEngine>,
    edit: impl FnOnce(&mut <MemoryEngine as ByteEngine>::Txn<'_>),
) -> DurableStore<MemoryEngine> {
    let mut engine = store.into_engine();
    {
        let mut txn = engine.begin().expect("begin");
        edit(&mut txn);
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    DurableStore::from_engine(engine, projection())
}

fn a_stem() -> Vec<u8> {
    physical::marker_key(numbers().root(), &[s("a")])
}

fn faults(report: &AuditReport) -> Vec<AuditFault> {
    report.findings.iter().map(|f| f.fault).collect()
}

/// Every layout constructor extends the stem it is given, so the suffix a cell carries
/// below a stem is the same cell built on an empty stem — the identity the walk's
/// suffix tables rest on.
#[test]
fn suffixes_extend_stems() {
    let stem = a_stem();
    for number in [1u32, 7, 65_535] {
        assert_eq!(
            physical::stem_field_leaf(&stem, number),
            [stem.clone(), physical::stem_field_leaf(&[], number)].concat()
        );
        assert_eq!(
            physical::group_stem(&stem, number),
            [stem.clone(), physical::group_stem(&[], number)].concat()
        );
    }
}

#[test]
fn a_populated_store_audits_clean_with_a_stable_content_stream() {
    let store = populated();
    let (report, first) = audit(&store);
    assert!(report.is_clean(), "{:?}", report.findings);
    assert_eq!(
        report.summary,
        AuditSummary {
            // markers 4 + leaves: a(title,pages,isbn,group pages)=4, b(title)=1, note
            // text 1, tag weight 1; the witness and the 3 index cells are not entry cells.
            cells: 4 + 4 + 1 + 1 + 1 + 3 + 1,
            entries: 4,
            index_cells: 3,
            findings: 0,
        }
    );
    let (_, second) = audit(&store);
    assert_eq!(first.cells, second.cells, "the stream is deterministic");
    assert_eq!(first.cells.len(), 11, "entry-family cells only");
    assert!(
        first.cells.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "the stream is in key order"
    );

    // One committed write changes the stream.
    let mut store = store;
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn");
        let root = txn.site(0);
        txn.create_entry(&root, &[s("c")], book("Gamma", None, None, None))
            .expect("create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let (report, third) = audit(&store);
    assert!(report.is_clean());
    assert_ne!(first.cells, third.cells);
}

#[test]
fn an_own_leaf_without_a_marker_is_an_orphan_named_by_its_node() {
    let numbers = numbers();
    let store = tamper(populated(), |txn| {
        let stem = physical::marker_key(0, &[s("z")]);
        txn.put(
            &physical::stem_field_leaf(&stem, numbers.fields()[0]),
            b"Zed".to_vec(),
        )
        .expect("put");
        txn.put(
            &physical::stem_field_leaf(&stem, numbers.fields()[1]),
            b"2".to_vec(),
        )
        .expect("second own leaf");
        let group = &numbers.groups()[0];
        txn.put(
            &physical::stem_field_leaf(
                &physical::group_stem(&stem, group.number()),
                group.fields()[0],
            ),
            b"3".to_vec(),
        )
        .expect("group leaf");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![AuditFinding {
            fault: AuditFault::OrphanLeaf,
            site: AuditSite::Node {
                root: 0,
                branch: Vec::new(),
                keys: vec![s("z")],
            },
        }]
    );
    // The orphan's leaf still counts as a cell, never as an entry.
    assert_eq!(report.summary.entries, 4);
}

#[test]
fn a_branch_leaf_without_its_marker_is_an_orphan_under_a_present_parent() {
    let numbers = numbers();
    let notes = &numbers.branches()[0];
    let store = tamper(populated(), |txn| {
        let note = physical::marker_key(notes.number(), &[s("a"), KeyScalar::Int(9)]);
        txn.put(
            &physical::stem_field_leaf(&note, notes.fields()[0]),
            b"x".to_vec(),
        )
        .expect("put");
    });
    let (report, _) = audit(&store);
    assert_eq!(faults(&report), vec![AuditFault::OrphanLeaf]);
    assert_eq!(
        report.findings[0].site,
        AuditSite::Node {
            root: 0,
            branch: vec![0],
            keys: vec![s("a"), KeyScalar::Int(9)],
        }
    );
}

#[test]
fn a_dangling_unique_index_cell_and_a_stale_cell_are_distinct_findings() {
    let root = numbers().root();
    let store = tamper(populated(), |txn| {
        // An index cell for isbn "999" naming a book that does not exist.
        txn.put(
            &physical::index_cell_key(root, &BY_ISBN, &[s("999")]),
            physical::index_cell_value(&[s("nobody")]),
        )
        .expect("put");
        // An index cell for isbn "222" naming book "a", whose isbn is "111".
        txn.put(
            &physical::index_cell_key(root, &BY_ISBN, &[s("222")]),
            physical::index_cell_value(&[s("a")]),
        )
        .expect("put");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![
            AuditFinding {
                fault: AuditFault::IndexStale,
                site: AuditSite::IndexCell {
                    root: 0,
                    index: 0,
                    values: vec![s("222")],
                },
            },
            AuditFinding {
                fault: AuditFault::IndexOrphan,
                site: AuditSite::IndexCell {
                    root: 0,
                    index: 0,
                    values: vec![s("999")],
                },
            },
        ]
    );
}

#[test]
fn an_empty_physical_key_is_reported_and_counted() {
    let baseline = audit(&populated()).0.summary.cells;
    let store = tamper(populated(), |txn| {
        txn.put(&[], b"foreign".to_vec()).expect("empty key");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![AuditFinding {
            fault: AuditFault::OutsideSchema,
            site: AuditSite::Cell { key: Vec::new() },
        }]
    );
    assert_eq!(report.summary.cells, baseline + 1);
}

#[test]
fn temporal_entry_keys_must_be_in_the_language_domain() {
    for key in [KeyScalar::Date(i32::MAX), KeyScalar::Instant(i128::MAX)] {
        let mut schema = StoreSchemaBuilder::root("dated", vec![key.scalar_kind()]);
        schema.scalar_field("title", ScalarKind::Str, true);
        let mut projection = StoreProjection::builder();
        projection.root(schema.finish().expect("schema"));
        let projection = projection.finish().expect("projection");
        let numbers = number_store(&projection).remove(0);
        let stem = physical::marker_key(numbers.root(), &[key]);
        let mut engine = MemoryEngine::new();
        {
            let mut txn = engine.begin().expect("begin");
            txn.put(&stem, physical::MARKER_VALUE.to_vec())
                .expect("marker");
            txn.put(
                &physical::stem_field_leaf(&stem, numbers.fields()[0]),
                b"valid title".to_vec(),
            )
            .expect("payload");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let store = DurableStore::from_engine(engine, projection);
        let (report, _) = audit(&store);
        assert!(faults(&report).contains(&AuditFault::Undecodable));
        assert_eq!(report.summary.cells, 2);
    }
}

#[test]
fn temporal_branch_and_index_keys_use_the_same_domain_check() {
    for (valid, invalid) in [
        (KeyScalar::Date(0), KeyScalar::Date(i32::MAX)),
        (KeyScalar::Instant(0), KeyScalar::Instant(i128::MAX)),
    ] {
        let mut schema = StoreSchemaBuilder::root("dated", vec![valid.scalar_kind()]);
        schema.scalar_field("title", ScalarKind::Str, true);
        schema.open_branch("edits", vec![valid.scalar_kind()]);
        schema.scalar_field("title", ScalarKind::Str, true);
        schema.close_branch();
        schema.index(BY_ISBN, true, vec![IndexComponent::key(0)]);
        let mut projection = StoreProjection::builder();
        projection.root(schema.finish().expect("schema"));
        let projection = projection.finish().expect("projection");
        let numbers = number_store(&projection).remove(0);
        let root = numbers.root();
        let branch = &numbers.branches()[0];
        let child = physical::marker_key(branch.number(), &[valid.clone(), invalid.clone()]);
        let cases = [
            (
                "branch",
                vec![
                    (child.clone(), physical::MARKER_VALUE.to_vec()),
                    (
                        physical::stem_field_leaf(&child, branch.fields()[0]),
                        b"valid title".to_vec(),
                    ),
                ],
            ),
            (
                "index key",
                vec![(
                    physical::index_cell_key(root, &BY_ISBN, std::slice::from_ref(&invalid)),
                    physical::index_cell_value(std::slice::from_ref(&valid)),
                )],
            ),
            (
                "index source",
                vec![(
                    physical::index_cell_key(root, &BY_ISBN, std::slice::from_ref(&valid)),
                    physical::index_cell_value(std::slice::from_ref(&invalid)),
                )],
            ),
        ];
        for (name, cells) in cases {
            let count = cells.len();
            let mut engine = MemoryEngine::new();
            {
                let mut txn = engine.begin().expect("begin");
                for (key, value) in cells {
                    txn.put(&key, value).expect("raw cell");
                }
                assert_eq!(txn.commit(), CommitOutcome::Confirmed);
            }
            let store = DurableStore::from_engine(engine, projection.clone());
            let (report, _) = audit(&store);
            assert_eq!(
                faults(&report),
                vec![AuditFault::Undecodable; count],
                "{name}"
            );
        }
    }
}

#[test]
fn a_unique_index_cell_cannot_represent_two_present_entries() {
    let numbers = numbers();
    let store = tamper(populated(), |txn| {
        let stem = physical::marker_key(numbers.root(), &[s("b")]);
        txn.put(
            &physical::stem_field_leaf(&stem, numbers.fields()[2]),
            b"111".to_vec(),
        )
        .expect("duplicate indexed value");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![AuditFinding {
            fault: AuditFault::IndexMissing,
            site: AuditSite::IndexCell {
                root: 0,
                index: 0,
                values: vec![s("111")],
            },
        }]
    );
}

#[test]
fn an_entry_whose_row_is_absent_is_an_index_missing_finding() {
    let root = numbers().root();
    let store = tamper(populated(), |txn| {
        txn.remove(&physical::index_cell_key(
            root,
            &BY_TITLE,
            &[s("Beta"), s("b")],
        ))
        .expect("remove");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![AuditFinding {
            fault: AuditFault::IndexMissing,
            site: AuditSite::IndexCell {
                root: 0,
                index: 1,
                values: vec![s("Beta"), s("b")],
            },
        }]
    );
    assert_eq!(report.summary.index_cells, 2);
}

#[test]
fn a_present_entry_missing_a_required_leaf_is_named_by_its_field() {
    let numbers = numbers();
    let notes = &numbers.branches()[0];
    let store = tamper(populated(), |txn| {
        txn.remove(&physical::stem_field_leaf(&a_stem(), numbers.fields()[0]))
            .expect("remove title");
        let note = physical::marker_key(notes.number(), &[s("a"), KeyScalar::Int(1)]);
        txn.remove(&physical::stem_field_leaf(&note, notes.fields()[0]))
            .expect("remove text");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![
            AuditFinding {
                fault: AuditFault::RequiredMissing,
                site: AuditSite::Field {
                    root: 0,
                    branch: Vec::new(),
                    keys: vec![s("a")],
                    group: None,
                    field: 0,
                },
            },
            AuditFinding {
                fault: AuditFault::RequiredMissing,
                site: AuditSite::Field {
                    root: 0,
                    branch: vec![0],
                    keys: vec![s("a"), KeyScalar::Int(1)],
                    group: None,
                    field: 0,
                },
            },
            // Book "a" no longer has a title, so its title index cell is stale.
            AuditFinding {
                fault: AuditFault::IndexStale,
                site: AuditSite::IndexCell {
                    root: 0,
                    index: 1,
                    values: vec![s("Alpha"), s("a")],
                },
            },
        ]
    );
}

#[test]
fn an_undecodable_leaf_is_named_by_its_field_including_a_group_field() {
    let numbers = numbers();
    let group = &numbers.groups()[0];
    let store = tamper(populated(), |txn| {
        txn.put(
            &physical::stem_field_leaf(&a_stem(), numbers.fields()[1]),
            b"1x".to_vec(),
        )
        .expect("put pages");
        let group_stem = physical::group_stem(&a_stem(), group.number());
        txn.put(
            &physical::stem_field_leaf(&group_stem, group.fields()[0]),
            b"".to_vec(),
        )
        .expect("put group pages");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![
            AuditFinding {
                fault: AuditFault::Undecodable,
                site: AuditSite::Field {
                    root: 0,
                    branch: Vec::new(),
                    keys: vec![s("a")],
                    group: None,
                    field: 1,
                },
            },
            AuditFinding {
                fault: AuditFault::Undecodable,
                site: AuditSite::Field {
                    root: 0,
                    branch: Vec::new(),
                    keys: vec![s("a")],
                    group: Some(0),
                    field: 0,
                },
            },
        ]
    );
}

#[test]
fn cells_outside_the_schema_are_named_by_their_raw_key() {
    let numbers = numbers();
    let foreign_field = physical::stem_field_leaf(&a_stem(), 4000);
    let foreign_branch = physical::marker_key(4001, &[s("a"), KeyScalar::Int(1)]);
    let foreign_root = physical::marker_key(4002, &[s("q")]);
    let foreign_group = physical::stem_field_leaf(&physical::group_stem(&a_stem(), 4003), 4004);
    let field_as_family = physical::marker_key(numbers.fields()[0], &[s("a")]);
    let group_as_family = physical::marker_key(numbers.groups()[0].number(), &[s("a")]);
    let branch_as_index_root =
        physical::index_cell_key(numbers.branches()[0].number(), &BY_ISBN, &[s("x")]);
    let foreign_index = physical::index_cell_key(numbers.root(), &[0xEE; 16], &[s("x")]);
    let foreign_meta = physical::meta_key("profile");
    let cells = [
        foreign_field.clone(),
        foreign_branch.clone(),
        foreign_root.clone(),
        foreign_group.clone(),
        field_as_family,
        group_as_family,
        branch_as_index_root,
        foreign_index.clone(),
        foreign_meta.clone(),
    ];
    let store = tamper(populated(), |txn| {
        for cell in &cells {
            txn.put(cell, vec![0x01]).expect("put");
        }
    });
    let (report, digest) = audit(&store);
    assert!(
        faults(&report)
            .iter()
            .all(|fault| *fault == AuditFault::OutsideSchema),
        "{:?}",
        report.findings
    );
    let mut named: Vec<Vec<u8>> = report
        .findings
        .iter()
        .filter_map(|finding| match &finding.site {
            AuditSite::Cell { key } => Some(key.clone()),
            AuditSite::UndeclaredIndex { root: 0, id } => {
                assert_eq!(*id, [0xEE; 16], "the undeclared index is named by its id");
                None
            }
            other => panic!("an outside-schema cell is named by its key: {other:?}"),
        })
        .collect();
    named.sort();
    let mut expected = cells.to_vec();
    expected.retain(|cell| *cell != foreign_index);
    expected.sort();
    assert_eq!(named, expected);
    // The commit witness is a declared meta cell and never a finding.
    assert_eq!(report.summary.findings, 9);
    let mut expected_digest = audit(&populated()).1.cells;
    expected_digest.push((foreign_field, vec![0x01]));
    expected_digest.push((foreign_group, vec![0x01]));
    expected_digest.sort();
    assert_eq!(
        digest.cells, expected_digest,
        "unknown families, indexes and metadata are excluded"
    );
}

/// The commit witness is the one meta cell the walk admits, and only in a shape the
/// next transaction would accept.
#[test]
fn a_witness_of_a_foreign_shape_is_a_typed_finding() {
    let witness = physical::meta_key(WITNESS);
    let store = tamper(populated(), |txn| {
        txn.put(&witness, vec![0x02; 17]).expect("put");
    });
    let (report, _) = audit(&store);
    assert_eq!(
        report.findings,
        vec![AuditFinding {
            fault: AuditFault::WitnessInvalid,
            site: AuditSite::Cell { key: witness },
        }]
    );
}

#[test]
fn a_marker_with_a_foreign_value_is_invalid() {
    let store = tamper(populated(), |txn| {
        txn.put(&a_stem(), vec![0x02]).expect("put");
    });
    let (report, _) = audit(&store);
    assert_eq!(faults(&report), vec![AuditFault::MarkerInvalid]);
    assert_eq!(
        report.findings[0].site,
        AuditSite::Node {
            root: 0,
            branch: Vec::new(),
            keys: vec![s("a")],
        }
    );
}

/// Only concrete entry markers count; absent ancestors neither add entries nor findings.
#[test]
fn a_child_with_two_absent_ancestors_is_audited_in_its_own_family() {
    let numbers = numbers();
    let notes = &numbers.branches()[0];
    let tags = &notes.branches()[0];
    let store = tamper(populated(), |txn| {
        let tag = physical::marker_key(tags.number(), &[s("z"), KeyScalar::Int(1), s("blue")]);
        txn.put(&tag, physical::MARKER_VALUE.to_vec())
            .expect("put tag");
        txn.put(
            &physical::stem_field_leaf(&tag, tags.fields()[0]),
            b"5".to_vec(),
        )
        .expect("put weight");
    });
    let (report, digest) = audit(&store);
    assert!(report.is_clean(), "{:?}", report.findings);
    assert_eq!(report.summary.entries, 5, "the tag is a present entry");
    let tag = physical::marker_key(tags.number(), &[s("z"), KeyScalar::Int(1), s("blue")]);
    let child_cells: Vec<_> = digest
        .cells
        .iter()
        .filter(|(key, _)| key.starts_with(&tag))
        .cloned()
        .collect();
    assert_eq!(
        child_cells,
        vec![
            (tag.clone(), physical::MARKER_VALUE.to_vec()),
            (
                physical::stem_field_leaf(&tag, tags.fields()[0]),
                b"5".to_vec()
            ),
        ]
    );
    assert_eq!(digest.cells.len(), 13);
    let mut exported_digest = Recording::default();
    let mut sink = Recording::default();
    let exported = store
        .export_cells(&mut exported_digest, &mut sink)
        .expect("export");
    assert_eq!(exported, report);
    assert_eq!(exported_digest.cells, digest.cells);
    for cell in child_cells {
        assert!(sink.cells.contains(&cell));
    }
}

#[test]
fn findings_past_the_cap_are_counted_but_not_retained() {
    let numbers = numbers();
    let branch = &numbers.branches()[0];
    let per_family = (MAX_REPORTED_FINDINGS + 40) / 2;
    let store = tamper(populated(), |txn| {
        for i in 0..per_family {
            let stem = physical::marker_key(0, &[s(&format!("orphan-{i:04}"))]);
            txn.put(
                &physical::stem_field_leaf(&stem, numbers.fields()[0]),
                b"x".to_vec(),
            )
            .expect("put");
            let child =
                physical::marker_key(branch.number(), &[s("absent"), KeyScalar::Int(i as i64)]);
            txn.put(
                &physical::stem_field_leaf(&child, branch.fields()[0]),
                b"x".to_vec(),
            )
            .expect("child leaf");
        }
    });
    let (report, digest) = audit(&store);
    assert_eq!(report.summary.findings, (MAX_REPORTED_FINDINGS + 40) as u64);
    assert_eq!(report.findings.len(), MAX_REPORTED_FINDINGS);
    assert!(
        faults(&report)
            .iter()
            .all(|fault| *fault == AuditFault::OrphanLeaf)
    );
    assert_eq!(
        report.findings[per_family - 1].site,
        AuditSite::Node {
            root: 0,
            branch: Vec::new(),
            keys: vec![s(&format!("orphan-{:04}", per_family - 1))],
        }
    );
    assert_eq!(
        report.findings[per_family].site,
        AuditSite::Node {
            root: 0,
            branch: vec![0],
            keys: vec![s("absent"), KeyScalar::Int(0)],
        }
    );
    assert_eq!(
        report.findings.last().expect("capped report").site,
        AuditSite::Node {
            root: 0,
            branch: vec![0],
            keys: vec![
                s("absent"),
                KeyScalar::Int((MAX_REPORTED_FINDINGS - per_family - 1) as i64)
            ],
        }
    );
    assert_eq!(
        digest.cells.len(),
        11 + MAX_REPORTED_FINDINGS + 40,
        "the finding cap does not cap the content stream"
    );
}

/// An index cell whose value is not a whole source key tuple is undecodable, not stale.
#[test]
fn an_index_cell_with_a_malformed_source_key_is_undecodable() {
    let root = numbers().root();
    let store = tamper(populated(), |txn| {
        let mut value = encode_key_tuple(&[s("a")]);
        value.push(0xAB);
        txn.put(
            &physical::index_cell_key(root, &BY_ISBN, &[s("333")]),
            value,
        )
        .expect("put");
    });
    let (report, _) = audit(&store);
    assert_eq!(faults(&report), vec![AuditFault::Undecodable]);
    assert_eq!(
        report.findings[0].site,
        AuditSite::IndexCell {
            root: 0,
            index: 0,
            values: vec![s("333")],
        }
    );
}

#[test]
fn family_key_segments_borrow_the_projection_through_the_admitted_depth() {
    let mut schema = StoreSchemaBuilder::root("root", vec![ScalarKind::Str, ScalarKind::Int]);
    for depth in 0..16 {
        schema.open_branch(
            format!("child{depth}"),
            vec![ScalarKind::Int, ScalarKind::Str],
        );
    }
    for _ in 0..16 {
        schema.close_branch();
    }
    let mut projection = StoreProjection::builder();
    projection.root(schema.finish().expect("sixteen branch hops"));
    let projection = projection.finish().expect("projection");
    let numbering = number_store(&projection);
    let tables = Tables::new(&projection, &numbering);
    assert_eq!(tables.families.len(), 17);
    assert_eq!(tables.roots[0].family, 0);
    assert!(
        tables
            .families
            .windows(2)
            .all(|pair| pair[0].prefix < pair[1].prefix)
    );
    let root = &projection.roots()[0];
    let mut expected = vec![root.key()];
    let mut branches = root.branches();
    for (depth, family) in tables.families.iter().enumerate() {
        assert_eq!(family.root, 0);
        assert_eq!(family.branch_path, vec![0; depth]);
        assert_eq!(family.key_segments.len(), depth + 1);
        for (borrowed, original) in family.key_segments.iter().zip(&expected) {
            assert!(
                std::ptr::eq(*borrowed, *original),
                "kind slices retain projection identity"
            );
        }
        if let Some(branch) = branches.first() {
            expected.push(branch.key());
            branches = branch.branches();
        }
    }
}

fn composite_child() -> (StoreProjection, Vec<KeyScalar>) {
    let mut schema = StoreSchemaBuilder::root("root", vec![ScalarKind::Str, ScalarKind::Int]);
    schema.open_branch("parent", vec![ScalarKind::Str, ScalarKind::Date]);
    schema.open_branch("child", vec![ScalarKind::Int, ScalarKind::Str]);
    schema.scalar_field("title", ScalarKind::Str, true);
    schema.scalar_field("count", ScalarKind::Int, false);
    schema.close_branch().close_branch();
    let mut projection = StoreProjection::builder();
    projection.root(schema.finish().expect("composite schema"));
    (
        projection.finish().expect("projection"),
        vec![
            s("root\0key"),
            KeyScalar::Int(11),
            s("ancestor\0key"),
            KeyScalar::Date(3),
            KeyScalar::Int(22),
            s("own\0key"),
        ],
    )
}

#[test]
fn the_open_entry_keeps_its_complete_decoded_tuple_across_own_leaves() {
    let (projection, keys) = composite_child();
    let numbering = number_store(&projection);
    let child = &numbering[0].branches()[0].branches()[0];
    let stem = physical::marker_key(child.number(), &keys);
    let cells = vec![
        (stem.clone(), physical::MARKER_VALUE.to_vec()),
        (
            physical::stem_field_leaf(&stem, child.fields()[0]),
            b"valid".to_vec(),
        ),
        (
            physical::stem_field_leaf(&stem, child.fields()[1]),
            b"7".to_vec(),
        ),
    ];
    let store = raw_store(projection.clone(), cells.clone());
    let (report, digest) = audit(&store);
    assert!(report.is_clean(), "{:?}", report.findings);
    assert_eq!(report.summary.entries, 1, "neither ancestor needs a marker");
    assert_eq!(digest.cells, cells);

    let mut exported = Recording::default();
    let exported_report = store
        .export_cells(&mut Recording::default(), &mut exported)
        .unwrap();
    assert_eq!(exported_report, report);
    let mut input = exported.cells.clone().into_iter();
    let mut restored_digest = Recording::default();
    let (restored, restored_report) =
        DurableStore::from_engine(MemoryEngine::new(), projection.clone())
            .restore(|| Ok::<_, ()>(input.next()), &mut restored_digest)
            .unwrap();
    assert_eq!(restored_report, report);
    assert_eq!(restored_digest.cells, digest.cells);
    let mut output = Recording::default();
    let final_report = restored
        .export_cells(&mut Recording::default(), &mut output)
        .unwrap();
    assert_eq!(final_report, report);
    assert_eq!(output.cells, exported.cells);

    let tables = Tables::new(&projection, &numbering);
    let engine = MemoryEngine::new();
    let view = engine.read_view().expect("view");
    let mut digest = Recording::default();
    let mut walker = Walker {
        view: &view,
        tables: &tables,
        digest: &mut digest,
        frame: None,
        family_cursor: 0,
        index_cursor: 0,
        index_family_cursor: 0,
        summary: AuditSummary::default(),
        findings: Vec::new(),
    };
    walker.cell(&cells[0].0, &cells[0].1).expect("marker");
    let frame = walker.frame.as_ref().expect("one open entry");
    assert_eq!(frame.family, 2);
    assert_eq!(frame.keys, keys);
    let decoded = frame.keys.as_ptr();
    for (key, value) in &cells[1..] {
        walker.cell(key, value).expect("own leaf");
        assert_eq!(
            walker.frame.as_ref().expect("same entry").keys.as_ptr(),
            decoded
        );
    }
    walker.close_entry().expect("close");
    assert!(walker.frame.is_none());
    assert!(walker.findings.is_empty());

    let orphan = raw_store(projection, cells[1..].to_vec());
    let (report, _) = audit(&orphan);
    assert_eq!(
        report.findings,
        vec![AuditFinding {
            fault: AuditFault::OrphanLeaf,
            site: AuditSite::Node {
                root: 0,
                branch: vec![0, 0],
                keys
            },
        }]
    );
    assert_eq!(report.summary.entries, 0);
}

#[test]
fn malformed_full_key_paths_are_reported_and_digested_in_the_child_family() {
    let (projection, keys) = composite_child();
    let numbering = number_store(&projection);
    let child = &numbering[0].branches()[0].branches()[0];
    let marker = physical::marker_key(child.number(), &keys);
    let mut wrong_root = keys.clone();
    wrong_root[1] = s("wrong root kind");
    let mut wrong_ancestor = keys.clone();
    wrong_ancestor[3] = s("wrong later ancestor kind");
    let mut invalid_ancestor = keys.clone();
    invalid_ancestor[3] = KeyScalar::Date(i32::MAX);
    let mut wrong_own = keys.clone();
    wrong_own[4] = s("wrong own kind");
    let mut extra = keys.clone();
    extra.push(KeyScalar::Int(99));
    let mut missing_terminator = marker.clone();
    missing_terminator.pop();
    let mut wrong_terminator = marker;
    *wrong_terminator.last_mut().expect("marker terminator") = 0x7F;
    let mut invalid_utf8 = physical::entry_family_prefix(child.number());
    invalid_utf8.extend(encode_key_tuple(&keys[..2]));
    invalid_utf8.extend([crate::codec::key::KEY_STR, 0xFE, 0, 0]);
    invalid_utf8.extend(encode_key_tuple(&keys[3..]));
    invalid_utf8.push(0);
    let cases = [
        (
            "root kind",
            physical::marker_key(child.number(), &wrong_root),
        ),
        (
            "later ancestor kind",
            physical::marker_key(child.number(), &wrong_ancestor),
        ),
        (
            "later ancestor domain",
            physical::marker_key(child.number(), &invalid_ancestor),
        ),
        ("own kind", physical::marker_key(child.number(), &wrong_own)),
        (
            "short tuple",
            physical::marker_key(child.number(), &keys[..5]),
        ),
        ("extra column", physical::marker_key(child.number(), &extra)),
        ("missing terminator", missing_terminator),
        ("wrong terminator", wrong_terminator),
        ("ancestor UTF-8", invalid_utf8),
    ];
    for (name, stem) in cases {
        let cells = vec![
            (stem.clone(), physical::MARKER_VALUE.to_vec()),
            (
                physical::stem_field_leaf(&stem, child.fields()[0]),
                b"valid".to_vec(),
            ),
        ];
        let store = raw_store(projection.clone(), cells.clone());
        let (report, digest) = audit(&store);
        let expected: Vec<_> = cells
            .iter()
            .map(|(key, _)| AuditFinding {
                fault: AuditFault::Undecodable,
                site: AuditSite::Cell { key: key.clone() },
            })
            .collect();
        assert_eq!(report.findings, expected, "{name}");
        assert_eq!(report.summary.entries, 0, "{name}");
        assert_eq!(report.summary.cells, 2, "{name}");
        assert_eq!(
            digest.cells, cells,
            "{name}: declared-family corruption remains in the digest"
        );
    }
}

#[test]
fn required_fields_close_in_entry_family_and_eof_order() {
    let mut schema = StoreSchemaBuilder::root("root", vec![ScalarKind::Str]);
    schema.scalar_field("title", ScalarKind::Str, true);
    schema.open_group("group");
    schema.scalar_field("code", ScalarKind::Int, true);
    schema.close_group();
    schema.open_branch("child", vec![ScalarKind::Int]);
    schema.scalar_field("title", ScalarKind::Str, true);
    schema.close_branch();
    let mut projection = StoreProjection::builder();
    projection.root(schema.finish().expect("schema"));
    let projection = projection.finish().expect("projection");
    let numbering = number_store(&projection);
    let root = &numbering[0];
    let child = &root.branches()[0];
    let cells = vec![
        (
            physical::marker_key(root.root(), &[s("a")]),
            physical::MARKER_VALUE.to_vec(),
        ),
        (
            physical::marker_key(root.root(), &[s("b")]),
            physical::MARKER_VALUE.to_vec(),
        ),
        (
            physical::marker_key(child.number(), &[s("absent"), KeyScalar::Int(7)]),
            physical::MARKER_VALUE.to_vec(),
        ),
    ];
    let store = raw_store(projection, cells.clone());
    let (report, digest) = audit(&store);
    let mut expected = Vec::new();
    for key in [s("a"), s("b")] {
        for group in [None, Some(0)] {
            expected.push(AuditFinding {
                fault: AuditFault::RequiredMissing,
                site: AuditSite::Field {
                    root: 0,
                    branch: Vec::new(),
                    keys: vec![key.clone()],
                    group,
                    field: 0,
                },
            });
        }
    }
    expected.push(AuditFinding {
        fault: AuditFault::RequiredMissing,
        site: AuditSite::Field {
            root: 0,
            branch: vec![0],
            keys: vec![s("absent"), KeyScalar::Int(7)],
            group: None,
            field: 0,
        },
    });
    assert_eq!(report.findings, expected);
    assert_eq!(report.summary.entries, 3);
    assert_eq!(digest.cells, cells);
}

#[test]
fn index_sorting_preserves_multiple_root_ownership_and_declaration_positions() {
    check_multiple_root_ownership(&[0, 1, 2, 3, 4, 5]);
}

#[test]
fn accepted_addresses_preserve_audit_and_restore_ownership() {
    check_multiple_root_ownership(&[90, 91, 5, 20, 21, 2]);
}

fn check_multiple_root_ownership(addresses: &[NodeNumber]) {
    use crate::durable::{CommitRecoveryScope, NumberedProjection};

    let mut projection = StoreProjection::builder();
    for name in ["first", "second"] {
        let mut schema = StoreSchemaBuilder::root(name, vec![ScalarKind::Str]);
        schema.scalar_field("title", ScalarKind::Str, true);
        schema.open_branch("child", vec![ScalarKind::Int]);
        schema.close_branch();
        schema.index(BY_TITLE, true, vec![IndexComponent::field(0)]);
        schema.index(
            BY_ISBN,
            true,
            vec![IndexComponent::field(0), IndexComponent::key(0)],
        );
        projection.root(schema.finish().expect("schema"));
    }
    projection.site(0, SiteTarget::whole_payload());
    projection.site(1, SiteTarget::whole_payload());
    projection.site(0, SiteTarget::branch_entry(vec![0]));
    projection.site(1, SiteTarget::branch_entry(vec![0]));
    let projection = projection.finish().expect("projection");
    let open = |engine| {
        DurableStore::from_numbered_with_ceiling_and_recovery_scope(
            engine,
            NumberedProjection::accepted(projection.clone(), addresses, 92)
                .expect("accepted addresses"),
            write(),
            CommitRecoveryScope::persistent([1; 16], "/audit-memory-fixture"),
        )
    };
    let mut store = open(MemoryEngine::new());
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn");
        for position in 0..2 {
            let site = txn.site(position);
            assert_eq!(
                txn.create_entry(
                    &site,
                    &[s("id")],
                    EntryValue {
                        fields: vec![vs("title")],
                        groups: Vec::new(),
                    }
                )
                .expect("create"),
                CreateOutcome::Created
            );
            let child = txn.site(position + 2);
            assert_eq!(
                txn.create_entry(
                    &child,
                    &[s("id"), KeyScalar::Int(7)],
                    EntryValue {
                        fields: Vec::new(),
                        groups: Vec::new()
                    }
                )
                .expect("child"),
                CreateOutcome::Created
            );
        }
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let (report, _) = audit(&store);
    assert!(report.is_clean(), "{:?}", report.findings);
    assert_eq!(report.summary.index_cells, 4);
    assert_eq!(report.summary.entries, 4);
    let mut exported = Recording::default();
    let mut digest = Recording::default();
    assert_eq!(
        store
            .export_cells(&mut digest, &mut exported)
            .expect("export"),
        report
    );
    let mut input = exported.cells.clone().into_iter();
    let mut restored_digest = Recording::default();
    let (restored, restored_report) = open(MemoryEngine::new())
        .restore(|| Ok::<_, ()>(input.next()), &mut restored_digest)
        .expect("restore accepted addresses");
    let mut transferred_report = report.clone();
    // The committed source witness is local metadata, excluded from transfer.
    transferred_report.summary.cells -= 1;
    assert_eq!(restored_report, transferred_report);
    assert_eq!(restored_digest.cells, digest.cells);
    let mut output = Recording::default();
    restored
        .export_cells(&mut Recording::default(), &mut output)
        .expect("restored export");
    assert_eq!(output.cells, exported.cells);
    let mut engine = store.into_engine();
    {
        let mut txn = engine.begin().expect("begin");
        txn.remove(&physical::index_cell_key(
            addresses[0],
            &BY_TITLE,
            &[s("title")],
        ))
        .expect("first root's declared index zero");
        txn.remove(&physical::index_cell_key(
            addresses[3],
            &BY_ISBN,
            &[s("title"), s("id")],
        ))
        .expect("second root's declared index one");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let store = open(engine);
    let (report, _) = audit(&store);
    let mut expected = vec![
        AuditFinding {
            fault: AuditFault::IndexMissing,
            site: AuditSite::IndexCell {
                root: 0,
                index: 0,
                values: vec![s("title")],
            },
        },
        AuditFinding {
            fault: AuditFault::IndexMissing,
            site: AuditSite::IndexCell {
                root: 1,
                index: 1,
                values: vec![s("title"), s("id")],
            },
        },
    ];
    if addresses[0] > addresses[3] {
        expected.reverse();
    }
    assert_eq!(report.findings, expected);
    assert_eq!(report.summary.index_cells, 2);
}
