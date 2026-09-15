//! Managed-index read runtime: nonunique progressive-prefix scan and unique
//! complete-key lookup over the maintained `byLabel`/`byValue` index cells, driven
//! through the real maintenance write path, plus the forged-image hostiles the
//! verified image is the sole trust boundary against.

use super::*;
use crate::durable::AuthorizedSite;
use crate::durable::store::index_ops::{op_index_lookup, op_index_scan};
use crate::durable::store::resolve::resolve_site;

/// One resolved index site, reached the way the store reaches it: the target is checked
/// against the completed root by the projection before the resolver ever sees it.
fn resolved(target: SiteTarget) -> AuthorizedSite {
    let schema = indexed_schema();
    let projection = super::project(&schema, vec![target]);
    let numbering = number_store(&projection);
    let crate::durable::SiteSlot::Resolved(site) = &projection.sites()[0] else {
        panic!("a named site resolves rather than parking");
    };
    resolve_site(&schema, &numbering[0], 0, site.target())
}

fn scan_site() -> AuthorizedSite {
    resolved(SiteTarget::index_scan(0))
}

fn lookup_site() -> AuthorizedSite {
    resolved(SiteTarget::index_lookup(1))
}

/// A store seeded through the real maintenance path: three entries whose `byLabel`
/// rows share label `"x"` for `a` and `b`, giving distinct labels `{x, y}` and,
/// under `"x"`, distinct names `{a, b}`.
fn seeded() -> DurableStore<MemoryEngine> {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write_demand())
        .unwrap();
    let e = txn.site(0);
    txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
    txn.create_entry(&e, &[ks("b")], ent(2, Some("x"))).unwrap();
    txn.create_entry(&e, &[ks("c")], ent(3, Some("y"))).unwrap();
    assert!(matches!(txn.commit(), CommitResult::Committed));
    store
}

/// A store whose engine is seeded with raw index cells, bypassing maintenance — the
/// forged-image shape a hostile reference-valid image can carry.
fn forged(cells: &[(Vec<u8>, Vec<u8>)]) -> DurableStore<MemoryEngine> {
    let mut engine = MemoryEngine::new();
    let mut txn = engine.begin().unwrap();
    for (key, value) in cells {
        txn.put(key, value.clone()).unwrap();
    }
    assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    DurableStore::from_engine(engine, project(&indexed_schema(), sites()))
}

fn scan(
    store: &DurableStore<MemoryEngine>,
    prefix: &[KeyScalar],
    from: Option<KeyScalar>,
    limit: u32,
) -> Result<BoundedKeys, KernelFault> {
    let view = store.engine.read_view().unwrap();
    op_index_scan(&view, &scan_site(), prefix, from, bound(limit))
}

fn lookup(
    store: &DurableStore<MemoryEngine>,
    key: &[KeyScalar],
) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
    let view = store.engine.read_view().unwrap();
    op_index_lookup(&view, &lookup_site(), key)
}

#[test]
fn scan_yields_distinct_next_component_bounded() {
    let store = seeded();
    // The empty prefix enumerates the first projected component: the distinct
    // labels, in ascending order, with no further value beyond them.
    assert_eq!(
        scan(&store, &[], None, 10),
        Ok(BoundedKeys {
            keys: vec![ks("x"), ks("y")],
            more: false,
        })
    );
    // A bound below the population freezes the first `N` and flags the rest.
    assert_eq!(
        scan(&store, &[], None, 1),
        Ok(BoundedKeys {
            keys: vec![ks("x")],
            more: true,
        })
    );
}

#[test]
fn scan_refines_under_a_held_prefix_to_the_source_keys() {
    let store = seeded();
    // Holding label `"x"` enumerates its distinct source names — the terminal
    // (complete-projection) component, where each cell equals its component row key.
    assert_eq!(
        scan(&store, &[ks("x")], None, 10),
        Ok(BoundedKeys {
            keys: vec![ks("a"), ks("b")],
            more: false,
        })
    );
    // A label with a single row yields exactly that source name.
    assert_eq!(
        scan(&store, &[ks("y")], None, 10),
        Ok(BoundedKeys {
            keys: vec![ks("c")],
            more: false,
        })
    );
}

#[test]
fn scan_from_is_an_inclusive_lower_bound_at_both_incomplete_and_complete_levels() {
    let store = seeded();
    // A non-terminal `from` (a label, which is not itself a whole cell): the walk
    // starts at or after it.
    assert_eq!(
        scan(&store, &[], Some(ks("y")), 10),
        Ok(BoundedKeys {
            keys: vec![ks("y")],
            more: false,
        })
    );
    // A terminal `from` (a source name whose cell equals its row key exactly): the
    // probe includes the equal row a bare forward scan would exclude.
    assert_eq!(
        scan(&store, &[ks("x")], Some(ks("b")), 10),
        Ok(BoundedKeys {
            keys: vec![ks("b")],
            more: false,
        })
    );
    // A `from` strictly above every source name under the prefix yields nothing.
    assert_eq!(
        scan(&store, &[ks("x")], Some(ks("z")), 10),
        Ok(BoundedKeys {
            keys: vec![],
            more: false,
        })
    );
}

#[test]
fn lookup_yields_the_one_source_key_or_absent() {
    let store = seeded();
    assert_eq!(lookup(&store, &[ki(2)]), Ok(Some(vec![ks("b")])));
    assert_eq!(lookup(&store, &[ki(1)]), Ok(Some(vec![ks("a")])));
    assert_eq!(lookup(&store, &[ki(99)]), Ok(None));
}

#[test]
fn a_scan_over_a_unique_index_is_rejected() {
    let store = seeded();
    let view = store.engine.read_view().unwrap();
    assert_eq!(
        op_index_scan(&view, &lookup_site(), &[], None, bound(10)),
        Err(KernelFault::Corruption),
    );
}

#[test]
fn a_lookup_over_a_nonunique_index_is_rejected() {
    let store = seeded();
    let view = store.engine.read_view().unwrap();
    assert_eq!(
        op_index_lookup(&view, &scan_site(), &[ks("x"), ks("a")]),
        Err(KernelFault::Corruption),
    );
}

#[test]
fn a_scan_operand_of_the_wrong_kind_is_rejected() {
    let store = seeded();
    // `byLabel`'s first component is the string label; an int prefix is a forged
    // operand.
    assert_eq!(
        scan(&store, &[ki(1)], None, 10),
        Err(KernelFault::Corruption)
    );
}

#[test]
fn a_scan_prefix_covering_the_whole_projection_is_rejected() {
    let store = seeded();
    // No component remains to enumerate: a complete projection is a lookup shape,
    // not a scan.
    assert_eq!(
        scan(&store, &[ks("x"), ks("a")], None, 10),
        Err(KernelFault::Corruption),
    );
}

#[test]
fn a_lookup_of_the_wrong_arity_is_rejected() {
    let store = seeded();
    let view = store.engine.read_view().unwrap();
    assert_eq!(
        op_index_lookup(&view, &lookup_site(), &[ki(1), ki(2)]),
        Err(KernelFault::Corruption),
    );
}

#[test]
fn a_forged_cell_whose_component_decodes_at_the_wrong_kind_is_corruption() {
    // A `byLabel` cell whose first projected column is an int, not the string
    // label the projection declares: a reference-valid image the runtime must not
    // read as a label.
    let store = forged(&[(
        physical::index_cell_key(0, &BY_LABEL, &[ki(5), ks("a")]),
        physical::index_cell_value(&[ks("a")]),
    )]);
    assert_eq!(scan(&store, &[], None, 10), Err(KernelFault::Corruption));
}

#[test]
fn a_forged_cell_whose_value_is_not_a_source_key_is_corruption() {
    // A unique `byValue` cell whose value does not decode as the root's key tuple
    // (an empty value cannot yield the one expected source key column).
    let store = forged(&[(physical::index_cell_key(0, &BY_VALUE, &[ki(7)]), Vec::new())]);
    assert_eq!(lookup(&store, &[ki(7)]), Err(KernelFault::Corruption));
}
