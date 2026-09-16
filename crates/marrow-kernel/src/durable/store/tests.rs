use marrow_store::{
    ByteEngine, CommitOutcome, MemoryEngine, NativeEngineOwner, ReadView, WriteTxn,
};

use super::super::physical;
use super::super::{
    BoundedKeys, BoundedLimit, CommitResult, CreateOutcome, DemandCoverage, EntryValue,
    EraseOutcome, IndexComponent, InvocationGrant, KernelFault, Presence, RootNumbering,
    SessionError, SiteTarget, StoreProjection, StoreSchema, StoreSchemaBuilder, number_store,
};
use super::{Durable, DurableStore};
use crate::codec::key::KeyScalar;
use crate::codec::value::{RuntimeScalar, ScalarKind};
use crate::equality::ValueDomain;
use crate::test_common::Scratch;

mod bounded_acquisition;
mod branch_fields;
mod corruption_corpus;
mod entry_values;
mod field_token_work;
mod index_lifetime;
mod index_read;
mod key_domains;
mod navigation_work;
mod nested_branches;

fn native_fixture(temp: &Scratch) -> NativeEngineOwner {
    NativeEngineOwner::provision(&temp.store()).expect("provision native fixture");
    NativeEngineOwner::acquire_existing(&temp.store())
        .expect("hold native fixture")
        .bind_and_open_existing(
            crate::durable::NativeOpenAccess::ReadWrite,
            [0x4B; 16],
            || Ok::<_, std::convert::Infallible>(()),
        )
        .expect("open native fixture")
}

/// The single-root projection a case opens under: the root, plus its sites resolved against
/// it. Every site in this module names root 0 — the store's only root.
fn project(schema: &StoreSchema, sites: Vec<SiteTarget>) -> StoreProjection {
    let mut projection = StoreProjection::builder();
    projection.root(schema.clone());
    for target in sites {
        projection.site(0, target);
    }
    projection
        .finish()
        .expect("every site names the one declared root")
}

/// The numbering of a single-root schema, through the same minter the store uses.
fn root_numbering(schema: &StoreSchema) -> Vec<RootNumbering> {
    number_store(&project(schema, Vec::new()))
}

/// The store-wide cell-key number of top-level field `index` of a single-root `schema`,
/// from the same numbering the store computes — so a test's hand-built cells key by the
/// exact numbers the ops write (correct by construction, not by hardcoded literals).
fn field_num(schema: &StoreSchema, index: usize) -> physical::NodeNumber {
    root_numbering(schema)[0].fields()[index]
}

/// The cell-key number of the branch reached by `path` (per-level branch indices from the
/// root down) in a single-root `schema`.
fn branch_num(schema: &StoreSchema, path: &[usize]) -> physical::NodeNumber {
    let numbering = root_numbering(schema);
    let mut level = numbering[0].branches();
    let mut number = 0;
    for &p in path {
        number = level[p].number();
        level = level[p].branches();
    }
    number
}

/// The cell-key number of field `field` of the branch reached by `path` in a single-root
/// `schema`.
fn branch_field_num(schema: &StoreSchema, path: &[usize], field: usize) -> physical::NodeNumber {
    let numbering = root_numbering(schema);
    let mut node = &numbering[0].branches()[path[0]];
    for &p in &path[1..] {
        node = &node.branches()[p];
    }
    node.fields()[field]
}

/// The cell-key number of group `index` of a single-root `schema`.
fn group_num(schema: &StoreSchema, index: usize) -> physical::NodeNumber {
    root_numbering(schema)[0].groups()[index].number()
}

/// The cell-key number of field `field` of group `index` of a single-root `schema`.
fn group_field_num(schema: &StoreSchema, index: usize, field: usize) -> physical::NodeNumber {
    root_numbering(schema)[0].groups()[index].fields()[field]
}

/// The flat `counters` root — keyed by string, with a required `value` and a sparse
/// `label` — as an unfinished builder, so the indexed variant can add its managed indexes
/// to the same root without a second spelling of the fields.
fn counters_builder() -> StoreSchemaBuilder {
    let mut builder = StoreSchemaBuilder::root("counters", vec![ScalarKind::Str]);
    builder.scalar_field("value", ScalarKind::Int, true);
    builder.scalar_field("label", ScalarKind::Str, false);
    builder
}

fn schema() -> StoreSchema {
    counters_builder()
        .finish()
        .expect("the counters schema builds")
}

fn sites() -> Vec<SiteTarget> {
    vec![
        SiteTarget::whole_payload(),
        SiteTarget::field_leaf(0),
        SiteTarget::field_leaf(1),
    ]
}

/// A branch-entry target naming the branch node the `path` of per-level branch
/// indices descends to (`&[0]` a direct child branch, `&[0, 1]` a nested one).
fn branch_entry(path: &[u16]) -> SiteTarget {
    SiteTarget::branch_entry(Box::from(path))
}

/// A branch-field target: the branch node `path` descends to, field index `field`.
fn branch_field(path: &[u16], field: u16) -> SiteTarget {
    SiteTarget::branch_field(Box::from(path), field)
}

fn value_entry(v: i64) -> EntryValue {
    EntryValue {
        groups: Vec::new(),
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(v))), None],
    }
}

fn write_demand() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

fn read_demand() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: false,
    }
}

#[test]
fn the_authority_triple_admits_the_union_and_checks_the_named_record() {
    // The compiler-side demand reaches the triple as read/write coverage: a
    // whole-program union for admission, a named export's record for invocation.
    // Under a read-only grant, a read-only record is admitted while a writing
    // record — including the union of a program that writes — is denied. Demand
    // never grants; the grant is the intersecting term.
    let read_grant = InvocationGrant {
        read: true,
        write: false,
    };

    // Invocation of a read-only export: admitted under the read-only grant.
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    assert!(store.read_session(read_grant, read_demand()).is_ok());

    // Admission of a program whose union writes: denied under the read-only grant.
    assert!(matches!(
        store.txn_session(read_grant, write_demand()),
        Err(SessionError::Denied)
    ));

    // A full grant admits the writing union.
    assert!(
        store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .is_ok()
    );
}

#[test]
fn iterates_created_keys_in_forward_order() {
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let entry = txn.site(0);
        // Insert out of order; iteration must still be ascending.
        for name in ["b", "a", "c"] {
            txn.create_entry(&entry, &[KeyScalar::Str(name.into())], value_entry(1))
                .expect("create");
        }
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let entry = read.site(0);
    let frozen = read
        .iterate_bounded(&entry, &[], None, bound(16))
        .expect("iterate");
    assert!(!frozen.more);
    assert_eq!(
        frozen.keys,
        vec![
            KeyScalar::Str("a".into()),
            KeyScalar::Str("b".into()),
            KeyScalar::Str("c".into()),
        ]
    );
}

#[test]
fn a_field_leaf_without_a_marker_is_corruption() {
    // Write a field leaf directly, with no entry marker: an orphan leaf.
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        txn.put(
            &physical::stem_field_leaf(
                &physical::marker_key(0, &[KeyScalar::Str("x".into())]),
                field_num(&schema(), 0),
            ),
            b"5".to_vec(),
        )
        .expect("seed orphan leaf");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let mut store = DurableStore::from_engine(engine, project(&schema(), sites()));
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let entry = read.site(0);
    assert_eq!(
        read.iterate_bounded(&entry, &[], None, bound(4)),
        Err(KernelFault::Corruption)
    );
}

#[test]
fn a_branch_field_write_with_a_root_only_key_path_faults() {
    // A branch-field site addresses the two-element key-path [root_key, branch_key].
    // A forged image that drives the field set over it with a single-element
    // key path must fault at the trust boundary rather than drop the branch hop and
    // mis-address the write to the root node.
    let mut builder = StoreSchemaBuilder::root("counters", vec![ScalarKind::Str]);
    builder.scalar_field("value", ScalarKind::Int, true);
    builder.open_branch("notes", vec![ScalarKind::Str]);
    builder.scalar_field("body", ScalarKind::Str, false);
    builder.close_branch();
    let schema = builder.finish().expect("the branch schema builds");
    let sites = vec![branch_field(&[0], 0)];
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write_demand())
        .expect("txn session");
    let branch_field = txn.site(0);
    // One key where the branch-field node needs two ([root_key, branch_key]).
    assert_eq!(
        txn.set_field(
            &branch_field,
            &[KeyScalar::Str("root".into())],
            ValueDomain::Scalar(RuntimeScalar::Str("note".into())),
        ),
        Err(KernelFault::Corruption)
    );
}

#[test]
fn a_committed_orphan_reads_as_corruption() {
    // A committed store with a field leaf but no entry marker is corrupt. A
    // whole-entry read through a coherent read session reports corruption via the
    // bounded prefix probe rather than silently reading the slot as absent.
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        txn.put(
            &physical::stem_field_leaf(
                &physical::marker_key(0, &[KeyScalar::Str("x".into())]),
                field_num(&schema(), 0),
            ),
            b"5".to_vec(),
        )
        .expect("seed orphan leaf");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let mut store = DurableStore::from_engine(engine, project(&schema(), sites()));
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let entry = read.site(0);
    assert_eq!(
        read.read_entry(&entry, &[KeyScalar::Str("x".into())]),
        Err(KernelFault::Corruption),
    );
}

/// A schema with one keyed branch: root `books` keyed by string with a required
/// `title`, plus a keyed branch `notes` keyed by int with a required `text`. The
/// site table addresses the root entry (0) and the branch entry (1).
fn branch_schema() -> (StoreSchema, Vec<SiteTarget>) {
    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_branch("notes", vec![ScalarKind::Int]);
    builder.scalar_field("text", ScalarKind::Str, true);
    builder.close_branch();
    let schema = builder.finish().expect("the books schema builds");
    let sites = vec![SiteTarget::whole_payload(), branch_entry(&[0])];
    (schema, sites)
}

/// The whole-entry branch vertical end to end: creating a branch entry under an
/// absent root leaves the root descendant-only (no payload marker, children below
/// it), so a whole read of the root is payload-absent; a create over that
/// descendant-only slot gives the root a payload without disturbing the branch
/// descendant, and a replace over the branch keeps the branch's own record while a
/// replace over the descendant-only root reports `KernelFault::Corruption`.
#[test]
fn a_branch_entry_makes_its_root_descendant_only_and_root_create_preserves_it() {
    let (schema, sites) = branch_schema();
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    let book = KeyScalar::Str("a".into());
    let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];

    // Create a branch entry under the absent root `a`: this writes the branch
    // child's marker and its `text` leaf, and never the root `a` marker.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let branch = txn.site(1);
        let entry = EntryValue {
            groups: Vec::new(),
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))],
        };
        assert_eq!(
            txn.create_entry(&branch, &note, entry)
                .expect("branch create"),
            CreateOutcome::Created,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    // The root `a` is descendant-only: no payload marker, so a whole read is
    // payload-absent and presence is absent, while a replace reports Corruption
    // without touching the descendant. The branch entry itself is present.
    {
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        assert_eq!(
            read.read_entry(&root, std::slice::from_ref(&book)),
            Ok(None),
            "a descendant-only root reads payload-absent",
        );
        assert_eq!(
            read.presence(&root, std::slice::from_ref(&book)),
            Ok(Presence::Absent),
            "a descendant-only root has no payload marker",
        );
        let branch = read.site(1);
        assert_eq!(
            read.presence(&branch, &note),
            Ok(Presence::Present),
            "the branch entry is present",
        );
    }

    // A replace over the descendant-only root is a marker/payload mismatch (a replace
    // runs only on a present edge) and leaves the branch untouched.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let root = txn.site(0);
        let entry = EntryValue {
            groups: Vec::new(),
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("late".into())))],
        };
        assert_eq!(
            txn.replace_entry(&root, std::slice::from_ref(&book), entry),
            Err(KernelFault::Corruption),
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    // Create the root `a` payload over the descendant-only slot: this writes the
    // root marker and `title` without touching the branch descendant.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let root = txn.site(0);
        let entry = EntryValue {
            groups: Vec::new(),
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str(
                "Book A".into(),
            )))],
        };
        assert_eq!(
            txn.create_entry(&root, std::slice::from_ref(&book), entry)
                .expect("root create"),
            CreateOutcome::Created,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    // The root now has a payload and the branch descendant survived the create.
    {
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        assert_eq!(
            read.read_entry(&root, std::slice::from_ref(&book)),
            Ok(Some(EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str(
                    "Book A".into()
                )))],
            })),
            "the root create gave the descendant-only node a payload",
        );
        let branch = read.site(1);
        assert_eq!(
            read.read_entry(&branch, &note),
            Ok(Some(EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))],
            })),
            "the branch descendant survived the root create",
        );
    }
}

/// Every raw cell of a store, as an owned key→value map (the test stores are small
/// enough that one page holds them all).
fn all_cells(store: &DurableStore<MemoryEngine>) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
    let view = store.engine.read_view().expect("read view");
    view.scan_after(&[], &[])
        .expect("scan")
        .into_iter()
        .map(|(key, value)| (key.to_vec(), value.to_vec()))
        .collect()
}

/// The byte prefix (marker stem) of a `books` root entry. `books` is the sole root, so it
/// is cell-key number 0.
fn book_stem(key: &str) -> Vec<u8> {
    physical::marker_key(0, &[KeyScalar::Str(key.into())])
}

/// Seed `cells` (key, value pairs) into a fresh engine and wrap it in a branch-schema
/// store, so a read session observes exactly the injected bytes.
fn injected_branch_store(cells: &[(Vec<u8>, Vec<u8>)]) -> DurableStore<MemoryEngine> {
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        for (key, value) in cells {
            txn.put(key, value.clone()).expect("seed cell");
        }
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let (schema, sites) = branch_schema();
    DurableStore::from_engine(engine, project(&schema, sites))
}

fn bound(n: u32) -> BoundedLimit {
    BoundedLimit::new(n).expect("a positive traversal bound")
}

const BY_LABEL: [u8; 16] = [0x70; 16];

const BY_VALUE: [u8; 16] = [0x71; 16];

fn ent(value: i64, label: Option<&str>) -> EntryValue {
    EntryValue {
        groups: Vec::new(),
        fields: vec![
            Some(ValueDomain::Scalar(RuntimeScalar::Int(value))),
            label.map(|l| ValueDomain::Scalar(RuntimeScalar::Str(l.into()))),
        ],
    }
}

/// Every managed-index cell (family `0x02`) of a store, in ascending key order — the
/// raw index state a maintained write leaves behind.
fn index_cells<E: ByteEngine>(store: &DurableStore<E>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let view = store.engine.read_view().expect("read view");
    let mut cells = view
        .scan_after(&[0x02], &[0x02])
        .expect("scan index family");
    cells.sort();
    cells
}

/// The `counters` root with a non-unique `byLabel(label, name)` index and a unique
/// `byValue(value)` index — the maintenance the write path keeps coherent.
fn indexed_schema() -> StoreSchema {
    let mut builder = counters_builder();
    builder.index(
        BY_LABEL,
        false,
        vec![IndexComponent::field(1), IndexComponent::key(0)],
    );
    builder.index(BY_VALUE, true, vec![IndexComponent::field(0)]);
    builder
        .finish()
        .expect("the indexed counters schema builds")
}

/// The expected `byLabel` cell for entry `name` with label `label`: keyed by the
/// projected `[label, name]` tuple, valued by the source key `[name]`.
fn label_cell(name: &str, label: &str) -> (Vec<u8>, Vec<u8>) {
    (
        physical::index_cell_key(
            0,
            &BY_LABEL,
            &[KeyScalar::Str(label.into()), KeyScalar::Str(name.into())],
        ),
        physical::index_cell_value(&[KeyScalar::Str(name.into())]),
    )
}

/// A fresh redb-backed store over the indexed schema, in a temp dir kept alive by the
/// returned guard.
fn native_indexed() -> (DurableStore<NativeEngineOwner>, Scratch) {
    let temp = Scratch::new("index-maint");
    NativeEngineOwner::provision(&temp.store()).expect("provision native");
    let engine = NativeEngineOwner::acquire_existing(&temp.store())
        .expect("acquire the owner lock")
        .bind_and_open_existing(
            crate::durable::NativeOpenAccess::ReadWrite,
            [0x31; 16],
            || Ok::<_, std::convert::Infallible>(()),
        )
        .expect("open native");
    (
        DurableStore::from_engine(engine, project(&indexed_schema(), sites())),
        temp,
    )
}

fn sorted(mut cells: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<(Vec<u8>, Vec<u8>)> {
    cells.sort();
    cells
}

/// The expected unique `byValue` cell for entry `name` with value `value`: keyed by
/// the projected `[value]`, valued by the source key `[name]`.
fn value_cell(name: &str, value: i64) -> (Vec<u8>, Vec<u8>) {
    (
        physical::index_cell_key(0, &BY_VALUE, &[KeyScalar::Int(value)]),
        physical::index_cell_value(&[KeyScalar::Str(name.into())]),
    )
}

fn ki(n: i64) -> KeyScalar {
    KeyScalar::Int(n)
}

fn ks(s: &str) -> KeyScalar {
    KeyScalar::Str(s.into())
}

fn vi(n: i64) -> Option<ValueDomain> {
    Some(ValueDomain::Scalar(RuntimeScalar::Int(n)))
}

fn vs(s: &str) -> Option<ValueDomain> {
    Some(ValueDomain::Scalar(RuntimeScalar::Str(s.into())))
}
