//! Entry absence and sparse payload absence have different index consequences.

use super::engine_call_support::{Counters, CountingEngine};
use super::*;
use crate::codec::value::encode_domain;
use crate::durable::ContentDigest;
use marrow_store::Cell as StoreCell;

const UNIQUE: [u8; 16] = [0x91; 16];
const ALL: [u8; 16] = [0x92; 16];
const MIXED: [u8; 16] = [0x93; 16];

struct Discard;

impl ContentDigest for Discard {
    fn absorb(&mut self, _key: &[u8], _value: &[u8]) {}
}

fn key_schema(kinds: &[ScalarKind], width: usize) -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("items", kinds.to_vec());
    for field in 0..width {
        builder.scalar_field(format!("f{field}"), ScalarKind::Int, false);
    }
    let keys: Vec<_> = (0..kinds.len())
        .map(|column| IndexComponent::key(column as u16))
        .collect();
    builder.index(UNIQUE, true, keys.clone());
    builder.index(ALL, false, keys.clone());
    if width != 0 {
        let mut mixed = vec![IndexComponent::field(0)];
        mixed.extend(keys);
        builder.index(MIXED, false, mixed);
    }
    builder.finish().expect("small indexed schema")
}

fn key_projection(schema: &StoreSchema) -> StoreProjection {
    let mut sites = vec![
        SiteTarget::whole_payload(),
        SiteTarget::index_lookup(0),
        SiteTarget::index_scan(1),
    ];
    if !schema.fields().is_empty() {
        sites.push(SiteTarget::field_leaf(0));
    }
    project(schema, sites)
}

fn vacant(width: usize) -> EntryValue {
    EntryValue {
        fields: vec![None; width],
        groups: Vec::new(),
    }
}

fn integers() -> EntryValue {
    EntryValue {
        fields: vec![
            Some(ValueDomain::Scalar(RuntimeScalar::Int(7))),
            Some(ValueDomain::Scalar(RuntimeScalar::Int(8))),
        ],
        groups: Vec::new(),
    }
}

fn key_cells(keys: &[KeyScalar]) -> Vec<StoreCell> {
    [UNIQUE, ALL]
        .into_iter()
        .map(|id| {
            (
                physical::index_cell_key(0, &id, keys),
                physical::index_cell_value(keys),
            )
        })
        .collect()
}

fn mixed_cell(keys: &[KeyScalar], value: i64) -> StoreCell {
    let mut projection = vec![ki(value)];
    projection.extend_from_slice(keys);
    (
        physical::index_cell_key(0, &MIXED, &projection),
        physical::index_cell_value(keys),
    )
}

fn cells<E: ByteEngine>(store: &DurableStore<E>) -> Vec<StoreCell> {
    let result = store
        .engine
        .read_view()
        .expect("raw fixture view")
        .scan_after(&[], &[])
        .expect("small fixture page");
    assert!(result.len() < 64, "the complete fixture fits in one page");
    result
}

// Reuse the held engine between small cases. All fixture cells fit in one page;
// raw seeding never relies on the entry/index maintenance under test.
fn seeded<E: ByteEngine>(
    mut engine: E,
    projection: StoreProjection,
    contents: Vec<StoreCell>,
) -> DurableStore<E> {
    let old = engine
        .read_view()
        .expect("seed view")
        .scan_after(&[], &[])
        .expect("old fixture cells");
    assert!(old.len() < 64);
    let mut txn = engine.begin().expect("seed transaction");
    for (key, _) in old {
        txn.remove(&key).expect("remove prior fixture cell");
    }
    for (key, value) in contents {
        txn.put(&key, value).expect("seed exact cell");
    }
    assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    DurableStore::from_engine(engine, projection)
}

fn clean<E: ByteEngine>(store: &DurableStore<E>, entries: u64, indexes: u64) {
    let report = store.logical_audit(&mut Discard).expect("logical audit");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.summary.entries, entries);
    assert_eq!(report.summary.index_cells, indexes);
}

fn indexed_entry<E: ByteEngine>(
    store: &mut DurableStore<E>,
    keys: &[KeyScalar],
    entry: Option<EntryValue>,
) {
    let present = entry.is_some();
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("fresh read session");
    assert_eq!(read.read_entry(&read.site(0), keys), Ok(entry));
    assert_eq!(
        read.index_lookup(&read.site(1), keys),
        Ok(present.then(|| keys.to_vec())),
    );
    let (last, prefix) = keys.split_last().expect("keyed entry");
    assert_eq!(
        read.index_scan(&read.site(2), prefix, None, bound(2)),
        Ok(BoundedKeys {
            keys: if present {
                vec![last.clone()]
            } else {
                Vec::new()
            },
            more: false,
        }),
    );
}

fn reopen(temp: &TempDir) -> NativeEngineOwner {
    NativeEngineOwner::acquire_existing(&temp.store())
        .expect("reacquire native owner")
        .bind_and_open_existing(
            crate::durable::NativeOpenAccess::ReadWrite,
            [0x4B; 16],
            || Ok::<_, std::convert::Infallible>(()),
        )
        .expect("reopen native engine")
}

fn lifetime<E: ByteEngine>(mut engine: E) -> E {
    for kinds in [
        vec![ScalarKind::Int],
        vec![ScalarKind::Str, ScalarKind::Int],
    ] {
        let keys = if kinds.len() == 1 {
            vec![ki(7)]
        } else {
            vec![ks("tenant"), ki(7)]
        };
        for width in [0, 2] {
            let schema = key_schema(&kinds, width);
            let mut store = seeded(engine, key_projection(&schema), Vec::new());
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("create");
                assert_eq!(
                    txn.create_entry(&txn.site(0), &keys, vacant(width)),
                    Ok(CreateOutcome::Created)
                );
                assert!(matches!(txn.commit(), CommitResult::Committed));
            }
            indexed_entry(&mut store, &keys, Some(vacant(width)));
            assert_eq!(index_cells(&store), sorted(key_cells(&keys)));
            clean(&store, 1, 2);
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("replace");
                let replacement = if width == 0 { vacant(0) } else { integers() };
                assert_eq!(
                    txn.create_entry(&txn.site(0), &keys, replacement),
                    Ok(CreateOutcome::AlreadyPresent)
                );
                assert_eq!(txn.read_entry(&txn.site(0), &keys), Ok(Some(vacant(width))));
                txn.replace_entry(&txn.site(0), &keys, vacant(width))
                    .expect("present replacement");
                assert!(matches!(txn.commit(), CommitResult::Committed));
            }
            indexed_entry(&mut store, &keys, Some(vacant(width)));
            assert_eq!(index_cells(&store), sorted(key_cells(&keys)));
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("erase");
                assert_eq!(
                    txn.erase_entry(&txn.site(0), &keys),
                    Ok(EraseOutcome::Erased)
                );
                assert!(matches!(txn.commit(), CommitResult::Committed));
            }
            indexed_entry(&mut store, &keys, None);
            assert!(index_cells(&store).is_empty());
            clean(&store, 0, 0);
            {
                let mut txn = store
                    .txn_session(InvocationGrant::full_store(), write_demand())
                    .expect("recreate");
                assert_eq!(
                    txn.create_entry(&txn.site(0), &keys, vacant(width)),
                    Ok(CreateOutcome::Created)
                );
                assert!(matches!(txn.commit(), CommitResult::Committed));
            }
            indexed_entry(&mut store, &keys, Some(vacant(width)));
            clean(&store, 1, 2);
            engine = store.into_engine();
        }
    }
    engine
}

#[test]
fn key_only_lifetime_covers_empty_and_sparse_scalar_and_composite_entries_in_memory() {
    lifetime(MemoryEngine::new());
}

#[test]
fn key_only_lifetime_covers_empty_and_sparse_scalar_and_composite_entries_natively() {
    let temp = TempDir::new("index-lifetime");
    drop(lifetime(native_fixture(&temp)));
    let schema = key_schema(&[ScalarKind::Str, ScalarKind::Int], 2);
    let mut store = DurableStore::from_engine(reopen(&temp), key_projection(&schema));
    indexed_entry(&mut store, &[ks("tenant"), ki(7)], Some(vacant(2)));
    clean(&store, 1, 2);
}

fn erase_seeded<E: ByteEngine>(mut engine: E) -> E {
    for width in [0, 2] {
        let schema = key_schema(&[ScalarKind::Str, ScalarKind::Int], width);
        let keys = [ks("tenant"), ki(7)];
        let mut initial = key_cells(&keys);
        initial.push((
            physical::marker_key(0, &keys),
            physical::MARKER_VALUE.to_vec(),
        ));
        let mut store = seeded(engine, key_projection(&schema), initial);
        indexed_entry(&mut store, &keys, Some(vacant(width)));
        assert_eq!(index_cells(&store), sorted(key_cells(&keys)));
        clean(&store, 1, 2);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("erase seeded entry");
            assert_eq!(
                txn.erase_entry(&txn.site(0), &keys),
                Ok(EraseOutcome::Erased)
            );
            assert!(matches!(txn.commit(), CommitResult::Committed));
        }
        indexed_entry(&mut store, &keys, None);
        assert!(index_cells(&store).is_empty());
        clean(&store, 0, 0);
        engine = store.into_engine();
    }
    engine
}

#[test]
fn independently_valid_key_only_indexes_are_erased_in_memory() {
    erase_seeded(MemoryEngine::new());
}

#[test]
fn independently_valid_key_only_indexes_are_erased_natively() {
    let temp = TempDir::new("index-seeded-erase");
    drop(erase_seeded(native_fixture(&temp)));
    let schema = key_schema(&[ScalarKind::Str, ScalarKind::Int], 2);
    let mut store = DurableStore::from_engine(reopen(&temp), key_projection(&schema));
    indexed_entry(&mut store, &[ks("tenant"), ki(7)], None);
    clean(&store, 0, 0);
}

fn mixed_fields<E: ByteEngine>(engine: E) {
    let schema = key_schema(&[ScalarKind::Int], 2);
    let keys = [ki(7)];
    let mut initial = key_cells(&keys);
    initial.push((
        physical::marker_key(0, &keys),
        physical::MARKER_VALUE.to_vec(),
    ));
    let mut store = seeded(engine, key_projection(&schema), initial);
    clean(&store, 1, 2);
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("set sparse field");
        txn.set_field(
            &txn.site(3),
            &keys,
            ValueDomain::Scalar(RuntimeScalar::Int(7)),
        )
        .expect("set");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut expected = key_cells(&keys);
    expected.push(mixed_cell(&keys, 7));
    assert_eq!(index_cells(&store), sorted(expected));
    clean(&store, 1, 3);
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("clear sparse field");
        assert_eq!(
            txn.erase_field(&txn.site(3), &keys),
            Ok(EraseOutcome::Erased)
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(index_cells(&store), sorted(key_cells(&keys)));
    clean(&store, 1, 2);
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("replace projected field");
        txn.replace_entry(&txn.site(0), &keys, integers())
            .expect("replace");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut expected = key_cells(&keys);
    expected.push(mixed_cell(&keys, 7));
    assert_eq!(index_cells(&store), sorted(expected));
    indexed_entry(&mut store, &keys, Some(integers()));
    clean(&store, 1, 3);
}

#[test]
fn mixed_sparse_membership_preserves_key_only_indexes_in_memory() {
    mixed_fields(MemoryEngine::new());
}

#[test]
fn mixed_sparse_membership_preserves_key_only_indexes_natively() {
    let temp = TempDir::new("index-mixed");
    mixed_fields(native_fixture(&temp));
    let schema = key_schema(&[ScalarKind::Int], 2);
    let mut store = DurableStore::from_engine(reopen(&temp), key_projection(&schema));
    indexed_entry(&mut store, &[ki(7)], Some(integers()));
    clean(&store, 1, 3);
}

fn subset_schema() -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("slots", vec![ScalarKind::Str, ScalarKind::Int]);
    builder.scalar_field("score", ScalarKind::Int, false);
    builder.index(UNIQUE, true, vec![IndexComponent::key(0)]);
    builder.index(
        MIXED,
        false,
        vec![
            IndexComponent::field(0),
            IndexComponent::key(0),
            IndexComponent::key(1),
        ],
    );
    builder.finish().expect("subset and field projection")
}

fn subset_projection(schema: &StoreSchema) -> StoreProjection {
    project(
        schema,
        vec![SiteTarget::whole_payload(), SiteTarget::index_lookup(0)],
    )
}

fn subset_owner(schema: &StoreSchema) -> Vec<StoreCell> {
    let keys = [ks("tenant"), ki(1)];
    let marker = physical::marker_key(0, &keys);
    vec![
        (marker.clone(), physical::MARKER_VALUE.to_vec()),
        (
            physical::stem_field_leaf(&marker, field_num(schema, 0)),
            encode_domain(&ValueDomain::Scalar(RuntimeScalar::Int(7))).expect("valid scalar"),
        ),
        (
            physical::index_cell_key(0, &UNIQUE, &[ks("tenant")]),
            physical::index_cell_value(&keys),
        ),
        mixed_cell(&keys, 7),
    ]
}

fn subset_lookup<E: ByteEngine>(store: &mut DurableStore<E>) {
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("fresh subset lookup");
    assert_eq!(
        read.index_lookup(&read.site(1), &[ks("tenant")]),
        Ok(Some(vec![ks("tenant"), ki(1)]))
    );
}

fn absent_orphans<E: ByteEngine>(engine: E) {
    let counters = Counters::new();
    let mut engine = CountingEngine::from_engine(engine, counters.clone());
    let schema = subset_schema();
    let absent = [ks("tenant"), ki(2)];
    let orphan =
        physical::stem_field_leaf(&physical::marker_key(0, &absent), field_num(&schema, 0));
    let valid = encode_domain(&ValueDomain::Scalar(RuntimeScalar::Int(7))).expect("valid scalar");
    for payload in [Some(vec![255]), None, Some(valid)] {
        let malformed = payload.as_deref() == Some(&[255][..]);
        let mut initial = subset_owner(&schema);
        if let Some(value) = payload {
            initial.push((orphan.clone(), value));
        }
        let mut store = seeded(engine, subset_projection(&schema), initial);
        subset_lookup(&mut store);
        let before = cells(&store);
        let indexes = index_cells(&store);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("absent erase");
            let writes = counters.writes();
            let result = txn.erase_entry(&txn.site(0), &absent);
            if malformed {
                assert_eq!(result, Err(KernelFault::Corruption));
                assert_eq!(
                    counters.writes() - writes,
                    0,
                    "projected corruption refuses before cleanup"
                );
            } else {
                assert_eq!(result, Ok(EraseOutcome::Missing));
                assert_eq!(
                    counters.writes() - writes,
                    2,
                    "only the absent marker and own field are removed"
                );
                assert!(matches!(txn.commit(), CommitResult::Committed));
            }
        }
        assert_eq!(
            index_cells(&store),
            indexes,
            "the other identity owns the unique subset cell"
        );
        subset_lookup(&mut store);
        if malformed {
            assert_eq!(cells(&store), before);
        } else {
            assert!(
                store
                    .engine
                    .read_view()
                    .expect("read cleanup")
                    .get(&orphan)
                    .expect("orphan cell")
                    .is_none()
            );
            clean(&store, 1, 2);
        }
        engine = store.into_engine();
    }
}

#[test]
fn absent_and_orphan_erasure_preserve_the_unique_owner_in_memory() {
    absent_orphans(MemoryEngine::new());
}

#[test]
fn absent_and_orphan_erasure_preserve_the_unique_owner_natively() {
    let temp = TempDir::new("index-orphan");
    absent_orphans(native_fixture(&temp));
    let schema = subset_schema();
    let mut store = DurableStore::from_engine(reopen(&temp), subset_projection(&schema));
    subset_lookup(&mut store);
    clean(&store, 1, 2);
}

#[derive(Clone, Copy)]
enum ExistingOwner {
    Committed,
    Staged,
}

fn collision<E: ByteEngine>(engine: E, owner: ExistingOwner) {
    let schema = subset_schema();
    let initial = match owner {
        ExistingOwner::Committed => subset_owner(&schema),
        ExistingOwner::Staged => Vec::new(),
    };
    let mut store = seeded(engine, subset_projection(&schema), initial);
    match owner {
        ExistingOwner::Committed => {
            subset_lookup(&mut store);
            clean(&store, 1, 2);
        }
        ExistingOwner::Staged => clean(&store, 0, 0),
    }
    let before = cells(&store);
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("collision transaction");
        let entry = txn.site(0);
        txn.create_entry(&entry, &[ks("unrelated"), ki(99)], vacant(1))
            .expect("unrelated staged write");
        if matches!(owner, ExistingOwner::Staged) {
            txn.create_entry(&entry, &[ks("tenant"), ki(1)], vacant(1))
                .expect("first subset owner");
        }
        assert_eq!(
            txn.create_entry(&entry, &[ks("tenant"), ki(2)], vacant(1)),
            Err(KernelFault::UniqueIndexViolation)
        );
        // The source VM has the rollback obligation; this kernel fixture drops
        // the failed transaction and never calls commit after the fault.
    }
    assert_eq!(
        cells(&store),
        before,
        "no staged write survives the dropped transaction"
    );
    match owner {
        ExistingOwner::Committed => {
            subset_lookup(&mut store);
            clean(&store, 1, 2);
        }
        ExistingOwner::Staged => clean(&store, 0, 0),
    }
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("usable owner after rollback");
        txn.create_entry(&txn.site(0), &[ks("later"), ki(3)], vacant(1))
            .expect("later valid write");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let (entries, indexes) = match owner {
        ExistingOwner::Committed => (2, 3),
        ExistingOwner::Staged => (1, 1),
    };
    clean(&store, entries, indexes);
}

#[test]
fn a_committed_subset_owner_rejects_collision_and_rolls_back_in_memory() {
    collision(MemoryEngine::new(), ExistingOwner::Committed);
}

#[test]
fn a_committed_subset_owner_rejects_collision_and_rolls_back_natively() {
    let temp = TempDir::new("index-committed-collision");
    collision(native_fixture(&temp), ExistingOwner::Committed);
    let schema = subset_schema();
    let mut store = DurableStore::from_engine(reopen(&temp), subset_projection(&schema));
    subset_lookup(&mut store);
    clean(&store, 2, 3);
}

#[test]
fn a_staged_subset_owner_rejects_collision_and_rolls_back_in_memory() {
    collision(MemoryEngine::new(), ExistingOwner::Staged);
}

#[test]
fn a_staged_subset_owner_rejects_collision_and_rolls_back_natively() {
    let temp = TempDir::new("index-staged-collision");
    collision(native_fixture(&temp), ExistingOwner::Staged);
    let schema = subset_schema();
    let mut store = DurableStore::from_engine(reopen(&temp), subset_projection(&schema));
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("fresh owner after collision");
    assert_eq!(read.index_lookup(&read.site(1), &[ks("tenant")]), Ok(None));
    assert_eq!(
        read.index_lookup(&read.site(1), &[ks("later")]),
        Ok(Some(vec![ks("later"), ki(3)]))
    );
    drop(read);
    clean(&store, 1, 1);
}

#[derive(Clone, Copy)]
enum Mutation {
    Create,
    Replace,
    Erase,
}

fn work(c: &Counters) -> [usize; 5] {
    [c.opens(), c.gets(), c.scans(), c.writes(), c.commits()]
}

fn work_since(c: &Counters, before: [usize; 5]) -> [usize; 5] {
    let after = work(c);
    std::array::from_fn(|i| after[i] - before[i])
}

fn mutation_work<E: ByteEngine>(engine: E, mutation: Mutation) {
    let schema = key_schema(&[ScalarKind::Int], 2);
    let keys = [ki(7)];
    let initial = if matches!(mutation, Mutation::Create) {
        Vec::new()
    } else {
        let marker = physical::marker_key(0, &keys);
        let mut initial = key_cells(&keys);
        initial.push(mixed_cell(&keys, 7));
        initial.push((marker.clone(), physical::MARKER_VALUE.to_vec()));
        for (position, value) in [(0, 7), (1, 8)] {
            initial.push((
                physical::stem_field_leaf(&marker, field_num(&schema, position)),
                encode_domain(&ValueDomain::Scalar(RuntimeScalar::Int(value)))
                    .expect("valid scalar"),
            ));
        }
        initial
    };
    let counters = Counters::new();
    let mut store = seeded(
        CountingEngine::from_engine(engine, counters.clone()),
        key_projection(&schema),
        initial,
    );
    if matches!(mutation, Mutation::Create) {
        clean(&store, 0, 0);
    } else {
        indexed_entry(&mut store, &keys, Some(integers()));
        clean(&store, 1, 3);
    }
    let before = work(&counters);
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write_demand())
        .expect("measured session");
    assert_eq!(
        work_since(&counters, before),
        [2, 1, 0, 0, 0],
        "session setup reads the witness then opens its transaction"
    );
    let entry = txn.site(0);
    let before = work(&counters);
    let expected = match mutation {
        Mutation::Create => {
            assert_eq!(
                txn.create_entry(&entry, &keys, integers()),
                Ok(CreateOutcome::Created)
            );
            // Marker get + one own-payload scan; one unique ownership get;
            // marker/two fields and three index puts. No old projected read.
            [0, 2, 1, 6, 0]
        }
        Mutation::Replace => {
            txn.replace_entry(&entry, &keys, integers())
                .expect("unchanged replacement");
            // Marker and one projected field get; three own removals and puts.
            // Equal index projections must not rewrite or recheck index cells.
            [0, 2, 0, 6, 0]
        }
        Mutation::Erase => {
            assert_eq!(txn.erase_entry(&entry, &keys), Ok(EraseOutcome::Erased));
            // Marker and projected field get; three own and three index removals.
            [0, 2, 0, 6, 0]
        }
    };
    assert_eq!(
        work_since(&counters, before),
        expected,
        "mutation work excludes session setup and commit"
    );
    if matches!(mutation, Mutation::Create) {
        let before = work(&counters);
        assert_eq!(
            txn.create_entry(&entry, &keys, integers()),
            Ok(CreateOutcome::AlreadyPresent)
        );
        assert_eq!(
            work_since(&counters, before),
            [0, 1, 0, 0, 0],
            "create over a present entry only probes its marker"
        );
    }
    let before = work(&counters);
    assert!(matches!(txn.commit(), CommitResult::Committed));
    assert_eq!(
        work_since(&counters, before),
        [0, 0, 0, 1, 1],
        "commit writes one witness and consumes one engine transaction"
    );
    drop(txn);
    if matches!(mutation, Mutation::Erase) {
        indexed_entry(&mut store, &keys, None);
        clean(&store, 0, 0);
    } else {
        indexed_entry(&mut store, &keys, Some(integers()));
        clean(&store, 1, 3);
    }
}

#[test]
fn create_work_uses_presence_and_unique_ownership_without_old_projected_reads() {
    mutation_work(MemoryEngine::new(), Mutation::Create);
    let temp = TempDir::new("index-create-work");
    mutation_work(native_fixture(&temp), Mutation::Create);
}

#[test]
fn unchanged_replace_work_does_not_rewrite_indexes() {
    mutation_work(MemoryEngine::new(), Mutation::Replace);
    let temp = TempDir::new("index-replace-work");
    mutation_work(native_fixture(&temp), Mutation::Replace);
}

#[test]
fn erase_work_removes_key_only_and_mixed_indexes() {
    mutation_work(MemoryEngine::new(), Mutation::Erase);
    let temp = TempDir::new("index-erase-work");
    mutation_work(native_fixture(&temp), Mutation::Erase);
}
