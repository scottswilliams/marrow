//! Navigation work on both engine implementations. Counts include actual scan
//! page copies; they do not measure native cache, allocation peaks or seek time.

use super::engine_call_support::{Counters, CountingEngine};
use super::*;
use crate::durable::{ContentDigest, CreateOutcome};
use marrow_store::StoreError;

#[derive(Debug)]
struct Work {
    gets: usize,
    scans: usize,
    writes: usize,
    commits: usize,
    cells: usize,
    bytes: usize,
}

impl Work {
    fn snapshot(c: &Counters) -> Self {
        Self {
            gets: c.gets(),
            scans: c.scans(),
            writes: c.writes(),
            commits: c.commits(),
            cells: c.returned_cells(),
            bytes: c.returned_bytes(),
        }
    }

    fn since(self, c: &Counters) -> Self {
        Self {
            gets: c.gets() - self.gets,
            scans: c.scans() - self.scans,
            writes: c.writes() - self.writes,
            commits: c.commits() - self.commits,
            cells: c.returned_cells() - self.cells,
            bytes: c.returned_bytes() - self.bytes,
        }
    }

    fn scan_only(&self, expected: usize) {
        assert_eq!(self.scans, expected, "{self:?}");
        assert_eq!(
            (self.gets, self.writes, self.commits),
            (0, 0, 0),
            "{self:?}"
        );
        assert!(self.cells <= expected * 64, "{self:?}");
        // These valid fixtures' individual cells fit below the soft page ceiling.
        assert!(self.bytes <= expected * (1 << 20), "{self:?}");
    }
}

#[derive(Clone, Copy, Debug)]
struct Case {
    depth: usize,
    width: usize,
    children: usize,
    key_bytes: usize,
    value_bytes: usize,
}

fn entry(width: usize, bytes: usize) -> EntryValue {
    EntryValue {
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("v".repeat(bytes)))); width],
        groups: Vec::new(),
    }
}

fn projection(case: Case) -> StoreProjection {
    let mut builder = StoreSchemaBuilder::root("items", vec![ScalarKind::Str]);
    for level in 0..=case.depth + 1 {
        if level != 0 {
            builder.open_branch(format!("b{level}"), vec![ScalarKind::Str]);
        }
        let width = if level == case.depth { case.width } else { 1 };
        for field in 0..width {
            builder.scalar_field(format!("f{field}"), ScalarKind::Str, true);
        }
    }
    for _ in 0..=case.depth {
        builder.close_branch();
    }
    let target = if case.depth == 0 {
        SiteTarget::whole_payload()
    } else {
        SiteTarget::branch_entry(vec![0; case.depth])
    };
    project(
        &builder.finish().expect("bounded schema"),
        vec![target, SiteTarget::branch_entry(vec![0; case.depth + 1])],
    )
}

fn queries(
    session: &mut impl Durable,
    counters: &Counters,
    ancestors: &[KeyScalar],
    first: &KeyScalar,
    last: &KeyScalar,
) -> (Work, Work) {
    let site = session.site(0);
    let before = Work::snapshot(counters);
    let frozen = session
        .iterate_bounded(
            &site,
            ancestors,
            Some(first.clone()),
            BoundedLimit::new(1).expect("positive"),
        )
        .expect("bounded acquisition");
    assert_eq!(frozen.keys, vec![first.clone()]);
    assert!(frozen.more);
    let walk = before.since(counters);
    walk.scan_only(2);
    let before = Work::snapshot(counters);
    assert_eq!(
        session.family_populated(&site, ancestors),
        Ok(Presence::Present)
    );
    let presence = before.since(counters);
    presence.scan_only(1);

    let before = Work::snapshot(counters);
    let tail = session
        .iterate_bounded(
            &site,
            ancestors,
            Some(last.clone()),
            BoundedLimit::new(1).expect("positive"),
        )
        .expect("inclusive last");
    assert_eq!(tail.keys, vec![last.clone()]);
    assert!(!tail.more);
    before.since(counters).scan_only(2);
    (walk, presence)
}

struct Discard;
impl ContentDigest for Discard {
    fn absorb(&mut self, _key: &[u8], _value: &[u8]) {}
}

fn navigation<E: ByteEngine>(engine: E, backend: &str, case: Case) {
    let counters = Counters::new();
    let mut store = DurableStore::from_projection_with_ceiling(
        CountingEngine::from_engine(engine, counters.clone()),
        projection(case),
        DemandCoverage {
            read: true,
            write: true,
        },
    );
    let ancestors = vec![ks("ancestor\0escaped"); case.depth];
    let first = ks(&format!("a\0{}", "x".repeat(case.key_bytes)));
    let last = ks(&format!("z\0{}", "x".repeat(case.key_bytes)));
    {
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("setup transaction");
        let target = txn.site(0);
        let child = txn.site(1);
        for key in [&first, &last] {
            let mut keys = ancestors.clone();
            keys.push(key.clone());
            assert_eq!(
                txn.create_entry(&target, &keys, entry(case.width, case.value_bytes)),
                Ok(CreateOutcome::Created)
            );
        }
        for child_number in 0..case.children {
            let mut keys = ancestors.clone();
            keys.push(ks(&format!("m{child_number:04}")));
            keys.push(ks("child"));
            assert_eq!(
                txn.create_entry(&child, &keys, entry(1, 1)),
                Ok(CreateOutcome::Created)
            );
        }
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let setup = Work::snapshot(&counters);
    assert_eq!(
        (setup.gets, setup.scans),
        (case.children + 3, case.children + 2)
    );
    // Session opening reads the commit witness; commit writes its next value.
    assert_eq!(setup.writes, 2 * (case.width + 1) + 2 * case.children + 1);
    assert_eq!(setup.commits, 1);
    assert_eq!((setup.cells, setup.bytes), (0, 0));

    let (walk, presence) = {
        let mut read = store
            .read_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: false,
                },
            )
            .expect("read session");
        queries(&mut read, &counters, &ancestors, &first, &last)
    };
    {
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("transaction session");
        let (txn_walk, txn_presence) = queries(&mut txn, &counters, &ancestors, &first, &last);
        assert_eq!((txn_walk.cells, txn_walk.bytes), (walk.cells, walk.bytes));
        assert_eq!(
            (txn_presence.cells, txn_presence.bytes),
            (presence.cells, presence.bytes)
        );
        let before = Work::snapshot(&counters);
        let mut keys = ancestors.clone();
        keys.push(first);
        assert_eq!(
            txn.erase_entry(&txn.site(0), &keys),
            Ok(EraseOutcome::Erased)
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
        let erase = before.since(&counters);
        assert_eq!(erase.writes, case.width + 2);
        assert_eq!(erase.commits, 1);
    }
    let before = Work::snapshot(&counters);
    let audit = store.logical_audit(&mut Discard).expect("complete audit");
    assert_eq!(audit.summary.findings, 0);
    assert_eq!(audit.summary.entries, (case.children + 1) as u64);
    assert_eq!(
        before.since(&counters).gets,
        1,
        "only the empty-key audit probe; no ancestor reads"
    );
    println!("{backend} {case:?}: setup={setup:?}; walk={walk:?}; presence={presence:?}");
}

#[test]
fn root_and_branch_navigation_work_is_independent_of_child_population_and_payload_width() {
    let base = Case {
        depth: 0,
        width: 1,
        children: 0,
        key_bytes: 1,
        value_bytes: 1,
    };
    let cases = [
        base,
        Case {
            children: 1,
            ..base
        },
        Case {
            children: 70,
            ..base
        },
        Case { width: 65, ..base },
        Case {
            key_bytes: 1000,
            ..base
        },
        Case {
            value_bytes: 700_000,
            ..base
        },
        Case {
            depth: 1,
            children: 70,
            ..base
        },
        Case {
            depth: 14,
            children: 70,
            ..base
        },
    ];
    for case in cases {
        navigation(MemoryEngine::new(), "memory", case);
        let temp = TempDir::new("navigation-work");
        navigation(native_fixture(&temp), "native", case);
    }
}

fn page_limits<E: ByteEngine>(engine: E) {
    let counters = Counters::new();
    let mut engine = CountingEngine::from_engine(engine, counters.clone());
    {
        let mut txn = engine.begin().expect("raw setup");
        for key in 0..65_u8 {
            txn.put(&[1, key], vec![1]).expect("small cell");
        }
        // The engine admits this maximum-size cell. Its bytes exceed the soft
        // aggregate page ceiling, so the first-cell exception must ensure progress.
        let key = vec![2; 4096];
        txn.put(&key, vec![1; 1 << 20])
            .expect("largest admitted cell");
        assert!(matches!(
            txn.put(&vec![2; 4097], vec![]),
            Err(StoreError::LimitExceeded { .. })
        ));
        assert!(matches!(txn.commit(), CommitOutcome::Confirmed));
    }
    let view = engine.read_view().expect("view");
    let before = Work::snapshot(&counters);
    let page = view.scan_after(&[1], &[1]).expect("small page");
    assert_eq!(page.len(), 64);
    let work = before.since(&counters);
    assert_eq!((work.scans, work.cells, work.bytes), (1, 64, 192));
    let before = Work::snapshot(&counters);
    let page = view.scan_after(&[2], &[2]).expect("oversized first cell");
    assert_eq!(page.len(), 1);
    let work = before.since(&counters);
    assert_eq!(
        (work.scans, work.cells, work.bytes),
        (1, 1, (1 << 20) + 4096)
    );
    assert!(view.scan_after(&[2], &page[0].0).expect("end").is_empty());
}

#[test]
fn actual_scan_pages_observe_the_cell_cap_and_oversized_first_cell_exception() {
    page_limits(MemoryEngine::new());
    let temp = TempDir::new("navigation-pages");
    page_limits(native_fixture(&temp));
}

fn wide_path<E: ByteEngine>(engine: E, columns: usize, depth: usize) {
    let mut kinds = vec![ScalarKind::Int; columns];
    kinds[columns - 1] = ScalarKind::Str;
    let mut builder = StoreSchemaBuilder::root("wide", kinds.clone());
    for level in 1..=depth {
        builder.open_branch(format!("b{level}"), kinds.clone());
    }
    // A field is itself a member: the depth-16 terminal branch can carry only
    // an empty payload, while depth 15 permits a field at the member-depth cap.
    let width = usize::from(depth < 16);
    if width != 0 {
        builder.scalar_field("value", ScalarKind::Str, true);
    }
    for _ in 0..depth {
        builder.close_branch();
    }
    let projection = project(
        &builder.finish().expect("full depth with composite keys"),
        vec![SiteTarget::branch_entry(vec![0; depth])],
    );
    let mut store = DurableStore::from_engine(engine, projection);
    let mut keys = Vec::new();
    for level in 0..=depth {
        for column in 0..columns {
            keys.push(if column + 1 == columns {
                ks("\0")
            } else {
                ki(level as i64)
            });
        }
    }
    assert_eq!(keys.len(), (depth + 1) * columns);
    *keys.last_mut().expect("last key") = ks("");
    let mut too_long;
    {
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("wide transaction");
        let site = txn.site(0);
        let base_len = super::super::address::node_stem(&site, &keys)
            .expect("full tuple")
            .len();
        let marker_limit = 4096 - 5 * width;
        let padding = "x".repeat(marker_limit - base_len);
        *keys.last_mut().expect("last key") = ks(&padding);
        let stem = super::super::address::node_stem(&site, &keys).expect("boundary tuple");
        assert_eq!(stem.len(), marker_limit);
        assert_eq!(
            stem.len(),
            7 + crate::codec::key::encode_key_tuple(&keys).len()
        );
        assert_eq!(
            txn.create_entry(&site, &keys, entry(width, 1)),
            Ok(CreateOutcome::Created)
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
        too_long = keys.clone();
        *too_long.last_mut().expect("last key") = ks(&(padding + "x"));
    }
    {
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("over-limit transaction");
        let site = txn.site(0);
        assert_eq!(
            txn.create_entry(&site, &too_long, entry(width, 1)),
            Err(KernelFault::Engine(StoreError::LimitExceeded {
                limit: "key length"
            })),
        );
        // The runtime abandons a faulted invocation. Drop also proves any staged
        // marker cannot survive a rejected field write in this fixture.
    }
    {
        let mut read = store
            .read_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: false,
                },
            )
            .expect("read committed bytes");
        let site = read.site(0);
        assert_eq!(read.read_entry(&site, &keys), Ok(Some(entry(width, 1))));
        assert_eq!(read.read_entry(&site, &too_long), Ok(None));
    }
    assert!(
        store
            .logical_audit(&mut Discard)
            .expect("all composite ancestors checked")
            .is_clean()
    );
}

#[test]
fn full_key_paths_preserve_arity_and_the_exact_engine_key_limit() {
    // Eight columns per node reaches the image's 136-column path. The trusted
    // kernel builder has no separate per-node arity cap: nine columns also work.
    for columns in [8, 9] {
        for depth in [15, 16] {
            wide_path(MemoryEngine::new(), columns, depth);
            let temp = TempDir::new("navigation-wide-path");
            wide_path(native_fixture(&temp), columns, depth);
        }
    }
}

fn orphan_queries(
    session: &mut impl Durable,
    counters: &Counters,
    ancestors: &[KeyScalar],
    present: usize,
) {
    let site = session.site(0);
    let before = Work::snapshot(counters);
    let presence = session.family_populated(&site, ancestors);
    assert_eq!(
        presence,
        if present == 0 {
            Err(KernelFault::Corruption)
        } else {
            Ok(Presence::Present)
        }
    );
    before.since(counters).scan_only(1);

    let before = Work::snapshot(counters);
    let walked = session.iterate_bounded(
        &site,
        ancestors,
        None,
        BoundedLimit::new(1).expect("positive"),
    );
    if present < 2 {
        assert_eq!(walked, Err(KernelFault::Corruption));
    } else {
        assert_eq!(
            walked,
            Ok(BoundedKeys {
                keys: vec![ks("a")],
                more: true
            })
        );
    }
    let work = before.since(counters);
    work.scan_only(if present == 0 { 1 } else { 2 });
    // The pages really contain the orphan. Only the first cell of each scan is
    // classified, so the second valid marker stops before that later bad entry.
    assert_eq!(work.cells, [1, 4, 8][present]);
}

fn orphan_boundary<E: ByteEngine>(engine: E, depth: usize, present: usize) {
    let counters = Counters::new();
    let case = Case {
        depth,
        width: 1,
        children: 0,
        key_bytes: 1,
        value_bytes: 1,
    };
    let projection = projection(case);
    let mut store = DurableStore::from_engine(
        CountingEngine::from_engine(engine, counters.clone()),
        projection.clone(),
    );
    let ancestors = vec![ks("absent ancestor"); depth];
    let orphan;
    {
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("setup");
        let site = txn.site(0);
        for key in ["a", "b"].into_iter().take(present) {
            let mut keys = ancestors.clone();
            keys.push(ks(key));
            assert_eq!(
                txn.create_entry(&site, &keys, entry(1, 1)),
                Ok(CreateOutcome::Created)
            );
        }
        let mut keys = ancestors.clone();
        keys.push(ks("z"));
        let stem = super::super::address::node_stem(&site, &keys).expect("orphan address");
        let crate::durable::AuthTarget::Entry { fields, .. } = &site.target else {
            panic!("entry")
        };
        orphan = physical::stem_field_leaf(&stem, fields[0].number);
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut engine = store.into_engine();
    {
        let mut txn = engine.begin().expect("raw orphan setup");
        let value = crate::codec::value::encode_domain(&ValueDomain::Scalar(RuntimeScalar::Str(
            "orphan".into(),
        )))
        .expect("scalar");
        txn.put(&orphan, value).expect("own field without marker");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let mut store = DurableStore::from_engine(engine, projection);
    {
        let mut read = store
            .read_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: false,
                },
            )
            .expect("read");
        orphan_queries(&mut read, &counters, &ancestors, present);
    }
    {
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("transaction");
        orphan_queries(&mut txn, &counters, &ancestors, present);
    }
    let audit = store.logical_audit(&mut Discard).expect("full inspection");
    assert_eq!(audit.summary.findings, 1);
    assert_eq!(
        audit.findings[0].fault,
        crate::durable::AuditFault::OrphanLeaf
    );
}

#[test]
fn encountered_orphans_fault_but_a_successful_more_marker_stops_inspection() {
    for depth in [0, 1] {
        for present in 0..=2 {
            orphan_boundary(MemoryEngine::new(), depth, present);
            let temp = TempDir::new("navigation-orphans");
            orphan_boundary(native_fixture(&temp), depth, present);
        }
    }
}
