//! Stored and supplied keys cross the real session boundary at their declared domain.

use super::engine_call_support::{Counters, CountingEngine};
use super::*;
use crate::durable::{AuditFault, AuditSite, ContentDigest};
use marrow_store::Cell;
use marrow_temporal::{
    SUPPORTED_DATE_MAX_DAYS, SUPPORTED_DATE_MIN_DAYS, SUPPORTED_INSTANT_MAX_NANOS,
    SUPPORTED_INSTANT_MIN_NANOS,
};
use std::fmt::Debug;

#[derive(Clone, Copy, Debug)]
enum Level {
    Root,
    Branch,
}

impl Level {
    fn site(self) -> u16 {
        match self {
            Self::Root => 0,
            Self::Branch => 1,
        }
    }

    fn ancestors(self, parent: &KeyScalar) -> Vec<KeyScalar> {
        match self {
            Self::Root => Vec::new(),
            Self::Branch => vec![parent.clone()],
        }
    }
}

fn rejected<T: Debug>(result: Result<T, KernelFault>, label: &str, failures: &mut Vec<String>) {
    if !matches!(result, Err(KernelFault::Corruption)) {
        failures.push(format!("{label}: {result:?}"));
    }
}

fn idle_refusal<T: Debug>(
    counters: &Counters,
    failures: &mut Vec<String>,
    label: &str,
    run: impl FnOnce() -> Result<T, KernelFault>,
) {
    let before = (counters.gets(), counters.scans(), counters.writes());
    rejected(run(), label, failures);
    let after = (counters.gets(), counters.scans(), counters.writes());
    if before != after {
        failures.push(format!("{label}: engine work {before:?} -> {after:?}"));
    }
}

struct Discard;

impl ContentDigest for Discard {
    fn absorb(&mut self, _key: &[u8], _value: &[u8]) {}
}

// Each fixture holds fewer than 64 cells. Reuse one engine per replay instead of
// opening a native database for each scalar, layer and boundary position.
fn seeded<E: ByteEngine>(
    mut engine: E,
    projection: StoreProjection,
    cells: Vec<Cell>,
) -> DurableStore<E> {
    let old = engine
        .read_view()
        .expect("seed view")
        .scan_after(&[], &[])
        .expect("seed cells");
    assert!(old.len() < 64);
    let mut txn = engine.begin().expect("seed transaction");
    for (key, _) in old {
        txn.remove(&key).expect("clear prior fixture");
    }
    for (key, value) in cells {
        txn.put(&key, value).expect("put exact fixture cell");
    }
    assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    DurableStore::from_engine(engine, projection)
}

fn invalid_keys() -> [(ScalarKind, KeyScalar, KeyScalar); 3] {
    [
        (
            ScalarKind::Int,
            KeyScalar::Int(0),
            KeyScalar::Str("wrong".into()),
        ),
        (
            ScalarKind::Date,
            KeyScalar::Date(0),
            KeyScalar::Date(i32::MAX),
        ),
        (
            ScalarKind::Instant,
            KeyScalar::Instant(0),
            KeyScalar::Instant(i128::MAX),
        ),
    ]
}

fn valid_keys() -> Vec<Vec<KeyScalar>> {
    vec![
        vec![
            KeyScalar::Int(i64::MIN),
            KeyScalar::Int(0),
            KeyScalar::Int(i64::MAX),
        ],
        vec![KeyScalar::Bool(false), KeyScalar::Bool(true)],
        vec![ks(""), ks("a\0b"), ks("b")],
        vec![
            KeyScalar::Bytes(vec![]),
            KeyScalar::Bytes(vec![0]),
            KeyScalar::Bytes(vec![255]),
        ],
        vec![
            KeyScalar::Duration(i128::MIN),
            KeyScalar::Duration(0),
            KeyScalar::Duration(i128::MAX),
        ],
        vec![
            KeyScalar::Date(SUPPORTED_DATE_MIN_DAYS),
            KeyScalar::Date(0),
            KeyScalar::Date(SUPPORTED_DATE_MAX_DAYS),
        ],
        vec![
            KeyScalar::Instant(SUPPORTED_INSTANT_MIN_NANOS),
            KeyScalar::Instant(0),
            KeyScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS),
        ],
    ]
}

fn layer_schema(kind: ScalarKind) -> StoreSchema {
    let mut schema = StoreSchemaBuilder::root("entries", vec![kind]);
    schema.open_branch("children", vec![kind]);
    schema.open_branch("leaves", vec![kind]);
    schema.close_branch().close_branch();
    schema.finish().expect("three empty-record levels")
}

fn layer_projection(schema: &StoreSchema) -> StoreProjection {
    project(
        schema,
        vec![
            SiteTarget::whole_payload(),
            SiteTarget::branch_entry(vec![0]),
        ],
    )
}

fn stem(schema: &StoreSchema, level: Level, parent: &KeyScalar, key: &KeyScalar) -> Vec<u8> {
    let root = root_numbering(schema)[0].root();
    match level {
        Level::Root => physical::marker_key(root, std::slice::from_ref(key)),
        Level::Branch => {
            physical::marker_key(branch_num(schema, &[0]), &[parent.clone(), key.clone()])
        }
    }
}

fn descendant(
    schema: &StoreSchema,
    level: Level,
    parent: &KeyScalar,
    key: &KeyScalar,
    child: &KeyScalar,
) -> Vec<u8> {
    let path: &[usize] = match level {
        Level::Root => &[0],
        Level::Branch => &[0, 0],
    };
    let mut keys = level.ancestors(parent);
    keys.extend([key.clone(), child.clone()]);
    physical::marker_key(branch_num(schema, path), &keys)
}

fn marker(key: Vec<u8>) -> Cell {
    (key, physical::MARKER_VALUE.to_vec())
}

#[derive(Clone, Copy, Debug)]
enum Position {
    First,
    Descendant,
    More,
}

fn check_bad_layer(
    session: &mut dyn Durable,
    level: Level,
    parent: &KeyScalar,
    position: Position,
    label: &str,
    failures: &mut Vec<String>,
) {
    let site = session.site(level.site());
    let ancestors = level.ancestors(parent);
    if matches!(position, Position::Descendant) {
        assert_eq!(
            session.iterate_bounded(&site, &ancestors, None, bound(1)),
            Ok(BoundedKeys {
                keys: Vec::new(),
                more: false
            }),
        );
        assert_eq!(
            session.family_populated(&site, &ancestors),
            Ok(Presence::Absent)
        );
        return;
    }
    rejected(
        session.iterate_bounded(&site, &ancestors, None, bound(1)),
        label,
        failures,
    );
    if matches!(position, Position::More) {
        assert_eq!(
            session.family_populated(&site, &ancestors),
            Ok(Presence::Present)
        );
    } else {
        rejected(session.family_populated(&site, &ancestors), label, failures);
    }
}

fn bad_layers<E: ByteEngine>(mut engine: E, failures: &mut Vec<String>) {
    let engine_type = std::any::type_name::<E>();
    for (kind, valid, invalid) in invalid_keys() {
        assert!(valid < invalid, "the invalid marker follows the frozen key");
        let schema = layer_schema(kind);
        for level in [Level::Root, Level::Branch] {
            for position in [Position::First, Position::Descendant, Position::More] {
                let bad = stem(&schema, level, &valid, &invalid);
                let cells = match position {
                    Position::First => vec![marker(bad)],
                    Position::Descendant => {
                        vec![marker(descendant(&schema, level, &valid, &invalid, &valid))]
                    }
                    Position::More => {
                        vec![marker(stem(&schema, level, &valid, &valid)), marker(bad)]
                    }
                };
                let malformed_ancestor =
                    matches!(position, Position::Descendant).then(|| cells[0].0.clone());
                let mut store = seeded(engine, layer_projection(&schema), cells);
                let label = format!("{engine_type}/{kind:?}/{level:?}/{position:?}");
                {
                    let mut read = store
                        .read_session(InvocationGrant::full_store(), read_demand())
                        .expect("read");
                    check_bad_layer(&mut read, level, &valid, position, &label, failures);
                }
                {
                    let mut txn = store
                        .txn_session(InvocationGrant::full_store(), write_demand())
                        .expect("transaction");
                    check_bad_layer(&mut txn, level, &valid, position, &label, failures);
                }
                if let Some(key) = malformed_ancestor {
                    let report = store
                        .logical_audit(&mut Discard)
                        .expect("audit stored ancestor");
                    assert_eq!(report.findings.len(), 1, "{label}: {:?}", report.findings);
                    assert_eq!(report.findings[0].fault, AuditFault::Undecodable);
                    assert_eq!(report.findings[0].site, AuditSite::Cell { key });
                }
                engine = store.into_engine();
            }
        }
    }
}

#[test]
fn stored_layer_keys_refuse_wrong_kinds_and_domains_on_both_engines() {
    let mut failures = Vec::new();
    bad_layers(MemoryEngine::new(), &mut failures);
    let temp = TempDir::new("key-domains-layer");
    bad_layers(native_fixture(&temp), &mut failures);
    assert!(
        failures.is_empty(),
        "invalid stored layer keys: {failures:#?}"
    );
}

fn check_order(session: &mut dyn Durable, level: Level, parent: &KeyScalar, keys: &[KeyScalar]) {
    let site = session.site(level.site());
    let ancestors = level.ancestors(parent);
    assert_eq!(
        session.family_populated(&site, &ancestors),
        Ok(Presence::Present)
    );
    assert_eq!(
        session.iterate_bounded(&site, &ancestors, None, bound(8)),
        Ok(BoundedKeys {
            keys: keys.to_vec(),
            more: false
        })
    );
    assert_eq!(
        session.iterate_bounded(&site, &ancestors, None, bound(1)),
        Ok(BoundedKeys {
            keys: vec![keys[0].clone()],
            more: true
        })
    );
    assert_eq!(
        session.iterate_bounded(&site, &ancestors, Some(keys[1].clone()), bound(1)),
        Ok(BoundedKeys {
            keys: vec![keys[1].clone()],
            more: keys.len() > 2
        })
    );
    for key in keys {
        let mut address = ancestors.clone();
        address.push(key.clone());
        assert_eq!(
            session.read_entry(&site, &address),
            Ok(Some(EntryValue {
                fields: vec![],
                groups: vec![]
            }))
        );
    }
}

fn good_layers<E: ByteEngine>(mut engine: E) {
    for keys in valid_keys() {
        let schema = layer_schema(keys[0].scalar_kind());
        for level in [Level::Root, Level::Branch] {
            let cells = keys
                .iter()
                .map(|key| marker(stem(&schema, level, &keys[0], key)))
                .collect();
            let mut store = seeded(engine, layer_projection(&schema), cells);
            {
                let mut read = store
                    .read_session(InvocationGrant::full_store(), read_demand())
                    .expect("read");
                check_order(&mut read, level, &keys[0], &keys);
            }
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("transaction");
                check_order(&mut txn, level, &keys[0], &keys);
            }
            engine = store.into_engine();
        }
    }
    let schema = layer_schema(ScalarKind::Int);
    for level in [Level::Root, Level::Branch] {
        let skipped = descendant(&schema, level, &ki(0), &ki(1), &ki(0));
        for present in [false, true] {
            let mut cells = vec![marker(skipped.clone())];
            if present {
                cells.extend([
                    marker(stem(&schema, level, &ki(0), &ki(0))),
                    marker(stem(&schema, level, &ki(0), &ki(2))),
                ]);
            }
            let mut store = seeded(engine, layer_projection(&schema), cells);
            {
                let mut read = store
                    .read_session(InvocationGrant::full_store(), read_demand())
                    .expect("read");
                if present {
                    check_order(&mut read, level, &ki(0), &[ki(0), ki(2)]);
                } else {
                    let site = read.site(level.site());
                    let ancestors = level.ancestors(&ki(0));
                    assert_eq!(
                        read.family_populated(&site, &ancestors),
                        Ok(Presence::Absent)
                    );
                    assert_eq!(
                        read.iterate_bounded(&site, &ancestors, None, bound(1)),
                        Ok(BoundedKeys {
                            keys: vec![],
                            more: false
                        })
                    );
                }
            }
            engine = store.into_engine();
        }
    }
}

#[test]
fn valid_layer_key_domains_keep_order_boundaries_and_descendant_independence() {
    good_layers(MemoryEngine::new());
    let temp = TempDir::new("key-domains-valid");
    good_layers(native_fixture(&temp));
}

const SCAN: [u8; 16] = [0xA1; 16];
const UNIQUE: [u8; 16] = [0xA2; 16];

fn index_projection(kind: ScalarKind) -> StoreProjection {
    let mut schema = StoreSchemaBuilder::root("indexed", vec![ScalarKind::Str, kind]);
    schema.scalar_field("projected", kind, false);
    schema.index(
        SCAN,
        false,
        vec![
            IndexComponent::field(0),
            IndexComponent::key(0),
            IndexComponent::key(1),
        ],
    );
    schema.index(UNIQUE, true, vec![IndexComponent::field(0)]);
    project(
        &schema.finish().expect("indexed schema"),
        vec![
            SiteTarget::index_scan(0),
            SiteTarget::index_lookup(1),
            SiteTarget::whole_payload(),
        ],
    )
}

fn scan_cell(value: KeyScalar, source: &[KeyScalar]) -> Cell {
    let mut projection = vec![value];
    projection.extend_from_slice(source);
    (
        physical::index_cell_key(0, &SCAN, &projection),
        physical::index_cell_value(source),
    )
}

fn unique_cell(value: &KeyScalar, source: &[KeyScalar]) -> Cell {
    (
        physical::index_cell_key(0, &UNIQUE, std::slice::from_ref(value)),
        physical::index_cell_value(source),
    )
}

fn bad_indexes<E: ByteEngine>(mut engine: E, failures: &mut Vec<String>) {
    let engine_type = std::any::type_name::<E>();
    for (kind, valid, invalid) in invalid_keys() {
        let source = [ks("owner"), valid.clone()];
        for more in [false, true] {
            let mut cells = vec![scan_cell(invalid.clone(), &source)];
            if more {
                cells.push(scan_cell(valid.clone(), &source));
            }
            let mut store = seeded(engine, index_projection(kind), cells);
            let label = format!("{engine_type}/scan {kind:?}, more={more}");
            {
                let mut read = store
                    .read_session(InvocationGrant::full_store(), read_demand())
                    .expect("read");
                let site = read.site(0);
                rejected(
                    read.index_scan(&site, &[], None, bound(1)),
                    &label,
                    failures,
                );
            }
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("transaction");
                let site = txn.site(0);
                rejected(txn.index_scan(&site, &[], None, bound(1)), &label, failures);
            }
            engine = store.into_engine();
        }
        for source in [[ki(7), valid.clone()], [ks("owner"), invalid]] {
            let mut store = seeded(
                engine,
                index_projection(kind),
                vec![unique_cell(&valid, &source)],
            );
            let label = format!("{engine_type}/lookup {kind:?}, source={source:?}");
            {
                let mut read = store
                    .read_session(InvocationGrant::full_store(), read_demand())
                    .expect("read");
                let site = read.site(1);
                rejected(
                    read.index_lookup(&site, std::slice::from_ref(&valid)),
                    &label,
                    failures,
                );
            }
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("transaction");
                let site = txn.site(1);
                rejected(
                    txn.index_lookup(&site, std::slice::from_ref(&valid)),
                    &label,
                    failures,
                );
            }
            engine = store.into_engine();
        }
    }
}

#[test]
fn stored_index_components_and_composite_sources_refuse_invalid_domains() {
    let mut failures = Vec::new();
    bad_indexes(MemoryEngine::new(), &mut failures);
    let temp = TempDir::new("key-domains-index");
    bad_indexes(native_fixture(&temp), &mut failures);
    assert!(
        failures.is_empty(),
        "invalid stored index keys: {failures:#?}"
    );
}

fn good_indexes<E: ByteEngine>(mut engine: E) {
    for keys in valid_keys() {
        let cells = keys
            .iter()
            .flat_map(|key| {
                let source = [ks("owner"), key.clone()];
                [scan_cell(key.clone(), &source), unique_cell(key, &source)]
            })
            .collect();
        let mut store = seeded(engine, index_projection(keys[0].scalar_kind()), cells);
        {
            let mut read = store
                .read_session(InvocationGrant::full_store(), read_demand())
                .expect("read");
            let scan = read.site(0);
            let lookup = read.site(1);
            let root = read.site(2);
            assert_eq!(
                read.index_scan(&scan, &[], None, bound(8)),
                Ok(BoundedKeys {
                    keys: keys.clone(),
                    more: false
                })
            );
            assert_eq!(
                read.index_scan(&scan, &[], None, bound(1)),
                Ok(BoundedKeys {
                    keys: vec![keys[0].clone()],
                    more: true
                })
            );
            for key in &keys {
                let source = vec![ks("owner"), key.clone()];
                assert_eq!(
                    read.index_lookup(&lookup, std::slice::from_ref(key)),
                    Ok(Some(source.clone()))
                );
                assert_eq!(
                    read.presence(&root, &source),
                    Ok(Presence::Absent),
                    "a derived key does not establish entry presence"
                );
                assert_eq!(
                    read.index_scan(
                        &scan,
                        &[key.clone(), ks("owner")],
                        Some(key.clone()),
                        bound(1)
                    ),
                    Ok(BoundedKeys {
                        keys: vec![key.clone()],
                        more: false
                    })
                );
            }
        }
        engine = store.into_engine();
    }
}

#[test]
fn valid_index_domains_keep_composite_identity_and_inclusive_exact_hits() {
    good_indexes(MemoryEngine::new());
    let temp = TempDir::new("key-domains-index-valid");
    good_indexes(native_fixture(&temp));
}

#[test]
fn supplied_layer_from_and_ancestor_keys_refuse_before_operation_io() {
    let mut failures = Vec::new();
    for (kind, valid, invalid) in invalid_keys() {
        let counters = Counters::new();
        let schema = layer_schema(kind);
        let mut store = DurableStore::from_engine(
            CountingEngine::new(counters.clone()),
            layer_projection(&schema),
        );
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("transaction");
        for level in [Level::Root, Level::Branch] {
            let site = txn.site(level.site());
            let ancestors = level.ancestors(&valid);
            let label = format!("from {kind:?}/{level:?}");
            idle_refusal(&counters, &mut failures, &label, || {
                txn.iterate_bounded(&site, &ancestors, Some(invalid.clone()), bound(1))
            });
        }
        let branch = txn.site(1);
        idle_refusal(&counters, &mut failures, "ancestor iteration", || {
            txn.iterate_bounded(&branch, std::slice::from_ref(&invalid), None, bound(1))
        });
        idle_refusal(&counters, &mut failures, "ancestor family presence", || {
            txn.family_populated(&branch, std::slice::from_ref(&invalid))
        });
    }
    assert!(
        failures.is_empty(),
        "supplied traversal operands: {failures:#?}"
    );
}

#[test]
fn supplied_index_prefix_from_and_lookup_keys_refuse_before_operation_io() {
    let mut failures = Vec::new();
    for (kind, valid, invalid) in invalid_keys() {
        for exact_cell in [false, true] {
            let counters = Counters::new();
            let source = [ks("owner"), invalid.clone()];
            let cells = if exact_cell {
                vec![scan_cell(valid.clone(), &source)]
            } else {
                Vec::new()
            };
            let mut store = seeded(
                CountingEngine::new(counters.clone()),
                index_projection(kind),
                cells,
            );
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("transaction");
            let scan = txn.site(0);
            let lookup = txn.site(1);
            idle_refusal(&counters, &mut failures, "index prefix", || {
                txn.index_scan(&scan, std::slice::from_ref(&invalid), None, bound(1))
            });
            idle_refusal(&counters, &mut failures, "index from", || {
                txn.index_scan(&scan, &[], Some(invalid.clone()), bound(1))
            });
            idle_refusal(&counters, &mut failures, "index exact from", || {
                txn.index_scan(
                    &scan,
                    &[valid.clone(), ks("owner")],
                    Some(invalid.clone()),
                    bound(1),
                )
            });
            idle_refusal(&counters, &mut failures, "index lookup operand", || {
                txn.index_lookup(&lookup, std::slice::from_ref(&invalid))
            });
        }
    }
    assert!(
        failures.is_empty(),
        "supplied index operands: {failures:#?}"
    );
}

fn addressed_projection(kind: ScalarKind) -> StoreProjection {
    let mut schema = StoreSchemaBuilder::root("addressed", vec![kind]);
    schema.scalar_field("value", ScalarKind::Int, false);
    schema.open_branch("children", vec![kind]);
    schema.scalar_field("value", ScalarKind::Int, false);
    schema.close_branch();
    project(
        &schema.finish().expect("addressed schema"),
        vec![
            SiteTarget::whole_payload(),
            SiteTarget::field_leaf(0),
            SiteTarget::branch_entry(vec![0]),
            SiteTarget::branch_field(vec![0], 0),
        ],
    )
}

fn sparse_entry() -> EntryValue {
    EntryValue {
        fields: vec![None],
        groups: vec![],
    }
}

#[test]
fn supplied_direct_and_ancestor_addresses_refuse_before_reads_or_writes() {
    let mut failures = Vec::new();
    for (kind, valid, invalid) in invalid_keys() {
        let counters = Counters::new();
        let mut store = DurableStore::from_engine(
            CountingEngine::new(counters.clone()),
            addressed_projection(kind),
        );
        for (entry_id, keys) in [
            (0, vec![invalid.clone()]),
            (2, vec![invalid.clone(), valid.clone()]),
            (2, vec![valid, invalid]),
        ] {
            let label = format!("address {kind:?}/{keys:?}");
            {
                let mut read = store
                    .read_session(InvocationGrant::full_store(), read_demand())
                    .expect("read");
                let entry = read.site(entry_id);
                let field = read.site(entry_id + 1);
                idle_refusal(&counters, &mut failures, &label, || {
                    read.presence(&entry, &keys)
                });
                idle_refusal(&counters, &mut failures, &label, || {
                    read.read_entry(&entry, &keys)
                });
                idle_refusal(&counters, &mut failures, &label, || {
                    read.read_field(&field, &keys)
                });
            }
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("transaction");
                let entry = txn.site(entry_id);
                let field = txn.site(entry_id + 1);
                idle_refusal(&counters, &mut failures, &label, || {
                    txn.create_entry(&entry, &keys, sparse_entry())
                });
                idle_refusal(&counters, &mut failures, &label, || {
                    txn.replace_entry(&entry, &keys, sparse_entry())
                });
                idle_refusal(&counters, &mut failures, &label, || {
                    txn.set_field(&field, &keys, ValueDomain::Scalar(RuntimeScalar::Int(1)))
                });
                idle_refusal(&counters, &mut failures, &label, || {
                    txn.erase_field(&field, &keys)
                });
                idle_refusal(&counters, &mut failures, &label, || {
                    txn.erase_entry(&entry, &keys)
                });
            }
        }
        assert!(
            store
                .engine
                .read_view()
                .expect("post-drop view")
                .scan_after(&[], &[])
                .expect("post-drop cells")
                .is_empty()
        );
    }
    assert!(
        failures.is_empty(),
        "supplied direct addresses: {failures:#?}"
    );
}

#[test]
fn supplied_mixed_composite_addresses_validate_later_columns_without_io() {
    let mut schema = StoreSchemaBuilder::root("mixed", vec![ScalarKind::Str, ScalarKind::Date]);
    schema.open_branch("children", vec![ScalarKind::Int, ScalarKind::Instant]);
    schema.close_branch();
    let projection = project(
        &schema.finish().expect("mixed composite addresses"),
        vec![
            SiteTarget::whole_payload(),
            SiteTarget::branch_entry(vec![0]),
        ],
    );
    let counters = Counters::new();
    let mut store = DurableStore::from_engine(CountingEngine::new(counters.clone()), projection);
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write_demand())
        .expect("transaction");
    let root = txn.site(0);
    let branch = txn.site(1);
    let valid_root = [ks("owner"), KeyScalar::Date(0)];
    let valid_branch = [
        ks("owner"),
        KeyScalar::Date(0),
        ki(7),
        KeyScalar::Instant(0),
    ];
    assert_eq!(txn.read_entry(&root, &valid_root), Ok(None));
    assert_eq!(txn.read_entry(&branch, &valid_branch), Ok(None));
    assert_eq!(txn.presence(&branch, &valid_branch), Ok(Presence::Absent));

    let mut failures = Vec::new();
    for (site, keys, label) in [
        (
            &root,
            vec![ks("owner"), KeyScalar::Date(i32::MAX)],
            "second root column",
        ),
        (
            &branch,
            vec![
                ks("owner"),
                KeyScalar::Date(i32::MAX),
                ki(7),
                KeyScalar::Instant(0),
            ],
            "second ancestor column",
        ),
        (
            &branch,
            vec![
                ks("owner"),
                KeyScalar::Date(0),
                ki(7),
                KeyScalar::Instant(i128::MAX),
            ],
            "second branch column",
        ),
    ] {
        idle_refusal(&counters, &mut failures, label, || {
            txn.read_entry(site, &keys)
        });
        idle_refusal(&counters, &mut failures, label, || {
            txn.create_entry(
                site,
                &keys,
                EntryValue {
                    fields: vec![],
                    groups: vec![],
                },
            )
        });
    }
    assert!(
        failures.is_empty(),
        "mixed composite operands: {failures:#?}"
    );
}
