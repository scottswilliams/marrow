//! Deterministic operation-count KATs for generic instantiation scaling, driven
//! through the production `compile` path. These record exact operation counts (no
//! wall time, no noise) for each mapped owner. The evidence-widened scale floor raised
//! the image type/function bounds (`MAX_TYPES` / `MAX_ENUMS` / `MAX_FUNCTIONS` = 4096),
//! so the 512/1024/2048 doubling points are now reachable and the scaling law is
//! exercised directly rather than argued from a ~64 cap.
//!
//! The `(template, args)` mint-dedup reuse probe is now a keyed lookup, so its per-mint
//! work is one probe: the scan-step count equals the mint-attempt count and doubles ~2x
//! across a doubled axis. Two owners are deliberately left unchanged by the scan repair
//! and remain super-linear on this domain — the per-mint metadata directory rebuild
//! (its build count and row work are byte-for-byte identical to before the repair) and
//! the per-template proof clone — so their frozen shapes are asserted here as the
//! recurrence gate for the follow-on lane that narrows them.
//!
//! Counts are observed through the private `super::capture_scaling_counts` window;
//! they are neither a public hook nor a canonical fact.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use super::{ScalingCounts, capture_scaling_counts};
use crate::compile::compile;

fn project(source: String) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// Compile a generated source through the production path, returning the exact
/// scaling counts on success or `None` if the program is refused (e.g. it exceeds an
/// image representational bound).
fn try_counts(source: String) -> Option<ScalingCounts> {
    let (result, counts) = capture_scaling_counts(|| compile(&project(source)));
    result.ok().map(|_| counts)
}

fn counts_for(source: String) -> ScalingCounts {
    try_counts(source).expect("fixture must compile cleanly")
}

/// `v` distinct seed structs `N0..Nv`, each a fresh non-generic argument type.
fn seed_structs(source: &mut String, v: usize) {
    for seed in 0..v {
        writeln!(source, "struct N{seed} {{ value: int }}").expect("write seed struct");
    }
}

/// Type-only axis: `v` distinct `Held[Nk]` type instantiations, one per seed, each
/// accumulated into a single reused `int` local (so the axis is bounded by the type
/// population, not by `MAX_LOCALS`). Each seed and each `Held` instance consumes an
/// image record slot, so the reachable ceiling is roughly `MAX_TYPES / 2`.
fn type_axis_fixture(v: usize) -> String {
    let mut source = String::from("module main\n\nstruct Held<T> { value: T }\n\n");
    seed_structs(&mut source, v);
    source.push_str("\npub fn driver(): int {\n    var sink: int = 0\n");
    for seed in 0..v {
        writeln!(
            source,
            "    sink = sink + Held(value: N{seed}(value: 0)).value.value"
        )
        .expect("write held accumulation");
    }
    source.push_str("    return sink\n}\n");
    source
}

/// Function-only axis: `v` distinct `leaf[Nk]` function instantiations, one per seed.
/// Bounded by `MAX_FUNCTIONS = 4096` (and by `MAX_TYPES` via the seeds).
fn fn_axis_fixture(v: usize) -> String {
    let mut source = String::from("module main\n\nfn leaf<T>(x: T): int { return 0 }\n\n");
    seed_structs(&mut source, v);
    source.push_str("\npub fn driver(): int {\n    var sink: int = 0\n");
    for seed in 0..v {
        writeln!(source, "    sink = leaf(N{seed}(value: 0))").expect("write leaf call");
    }
    source.push_str("    return sink\n}\n");
    source
}

/// A chain of `n + 1` structs `C0..Cn`, each holding the next by value and ending in
/// a scalar leaf. The value-containment graph is an acyclic path of `n + 1` nodes:
/// the former per-start reachability walk cost Σ(n − i) = O(n²) edge steps across the
/// struct start nodes, while the shared build-time cycle scan visits each edge once,
/// O(n).
fn struct_chain_fixture(n: usize) -> String {
    let mut source = String::from("module main\n\n");
    for link in 0..n {
        writeln!(source, "struct C{link} {{ next: C{} }}", link + 1).expect("write chain link");
    }
    writeln!(source, "struct C{n} {{ leaf: int }}").expect("write chain leaf");
    source.push_str("\npub fn driver(): int {\n    return 0\n}\n");
    source
}

/// The largest `v` (searching `1..=limit`) for which `fixture(v)` still compiles,
/// with the scaling counts observed at that `v`. Panics if even `v = 1` fails. With
/// the widened image bounds the axis fixtures compile well past a modest `limit`, so a
/// caller passing a small `limit` uses it as a sampling window (the returned `v` is
/// then `limit` itself) rather than a true compile ceiling.
fn reachable_ceiling(fixture: impl Fn(usize) -> String, limit: usize) -> (usize, ScalingCounts) {
    let mut best = None;
    for v in 1..=limit {
        match try_counts(fixture(v)) {
            Some(counts) => best = Some((v, counts)),
            None => break,
        }
    }
    best.expect("at least v=1 must compile")
}

fn ratio(at_2v: usize, at_v: usize) -> f64 {
    assert!(at_v > 0, "1x count must be positive to form a ratio");
    at_2v as f64 / at_v as f64
}

/// The compiler's shared instantiation budget, reported by the manual scaling probe.
const MAX_INSTANTIATIONS: usize = super::MAX_INSTANTIATIONS;

/// The `(template, args)` mint-dedup reuse probe is a keyed lookup, so its scan-step
/// count equals the mint-attempt count and doubles ~2x across a doubled type axis —
/// proven directly at the now-reachable 512→1024 and 1024→2048 doubling points. Two
/// owners are deliberately unchanged by the scan repair and stay super-linear here: the
/// per-mint metadata directory rebuild (row work roughly quadruples per doubling) and
/// its build count stays ~linear. Their frozen shapes are the recurrence gate for the
/// follow-on lane. (The type axis reaches ~`MAX_TYPES / 2` because each instantiation
/// costs its seed plus its `Held` record; the near-4096 point is exercised on the fn
/// axis, which reaches ~`MAX_FUNCTIONS − 1`.)
#[test]
fn type_axis_scan_is_linear_directory_rebuild_still_quadratic() {
    let a = counts_for(type_axis_fixture(512));
    let b = counts_for(type_axis_fixture(1024));
    let c = counts_for(type_axis_fixture(2048));

    // Repaired: one keyed probe per mint attempt, so the scan count is the axis width
    // and each doubling is ~2x.
    for (lo, hi) in [(&a, &b), (&b, &c)] {
        let scan_ratio = ratio(hi.type_inst_scan_steps, lo.type_inst_scan_steps);
        assert!(
            (1.7..=2.3).contains(&scan_ratio),
            "type reuse probe is a keyed lookup, linear in mint attempts; \
             steps {} -> {} ratio {scan_ratio:.2}",
            lo.type_inst_scan_steps,
            hi.type_inst_scan_steps,
        );
    }
    assert_eq!(
        c.type_inst_scan_steps, 2048,
        "one keyed probe per mint attempt at v=2048"
    );

    // Unrepaired (frozen): the per-mint directory rebuild is super-linear — row work
    // roughly quadruples per doubling — awaiting the follow-on lane.
    for (lo, hi) in [(&a, &b), (&b, &c)] {
        let row_ratio = ratio(hi.directory_row_visits, lo.directory_row_visits);
        assert!(
            row_ratio >= 3.5,
            "per-mint directory rebuild is still super-linear; \
             row visits {} -> {} ratio {row_ratio:.2}",
            lo.directory_row_visits,
            hi.directory_row_visits,
        );
    }
    // The directory-build COUNT stays ~linear (one rebuild per mint attempt).
    let build_ratio = ratio(c.directory_builds, b.directory_builds);
    assert!(
        (1.7..=2.3).contains(&build_ratio),
        "directory build count is ~linear; builds {} -> {} ratio {build_ratio:.2}",
        b.directory_builds,
        c.directory_builds,
    );
}

/// The `reserve_fn_instance` reuse probe is a keyed lookup — one probe per reserve
/// attempt — so its scan-step count is the axis width and doubles ~2x, proven at the
/// now-reachable 512→1024 and 2048→4095 doubling points (the function axis reaches
/// ~`MAX_FUNCTIONS − 1` before its seed types exhaust `MAX_TYPES`). The function axis
/// drives no per-mint type directory rebuild, so it is linear after the repair.
#[test]
fn fn_axis_scan_is_linear() {
    let a = counts_for(fn_axis_fixture(512));
    let b = counts_for(fn_axis_fixture(1024));
    let c = counts_for(fn_axis_fixture(2048));
    let d = counts_for(fn_axis_fixture(4095));

    for (lo, hi) in [(&a, &b), (&c, &d)] {
        let scan_ratio = ratio(hi.fn_inst_scan_steps, lo.fn_inst_scan_steps);
        assert!(
            (1.7..=2.3).contains(&scan_ratio),
            "fn reuse probe is a keyed lookup, linear in reserve attempts; \
             steps {} -> {} ratio {scan_ratio:.2}",
            lo.fn_inst_scan_steps,
            hi.fn_inst_scan_steps,
        );
    }
    assert_eq!(
        d.fn_inst_scan_steps, 4095,
        "one keyed probe per reserve attempt at v=4095"
    );

    // The per-fn-mint directory rebuild count stays ~linear across a doubling.
    let build_ratio = ratio(c.directory_builds, b.directory_builds);
    assert!(
        (1.7..=2.3).contains(&build_ratio),
        "directory build count is ~linear; builds {} -> {} ratio {build_ratio:.2}",
        b.directory_builds,
        c.directory_builds,
    );
}

/// Manual reporting probe: prints the reachable ceiling and the full operation-count
/// table for each axis at half and full ceiling. Ignored by default (a reproducible
/// artifact, never a default assertion). Run with
/// `cargo test -p marrow-compile --lib generic_scaling_report -- --ignored --nocapture`.
#[test]
#[ignore = "manual reproducible reporting artifact, not a default assertion"]
fn generic_scaling_report() {
    for (name, fixture) in [
        ("type-axis", type_axis_fixture as fn(usize) -> String),
        ("fn-axis", fn_axis_fixture as fn(usize) -> String),
    ] {
        let (ceiling, top) = reachable_ceiling(fixture, 64);
        let half = counts_for(fixture(ceiling / 2));
        println!(
            "\n[{name}] reachable ceiling = {ceiling} distinct instantiations \
             (MAX_INSTANTIATIONS = {MAX_INSTANTIATIONS})"
        );
        println!(
            "  half (v={:>2}): builds={:>4} row_visits={:>6} ty_scan={:>6} fn_scan={:>6} \
             cycle_steps={:>6} proof_clones={} proof_rows={}",
            ceiling / 2,
            half.directory_builds,
            half.directory_row_visits,
            half.type_inst_scan_steps,
            half.fn_inst_scan_steps,
            half.cycle_walk_steps,
            half.proof_clones,
            half.proof_clone_rows,
        );
        println!(
            "  full (v={:>2}): builds={:>4} row_visits={:>6} ty_scan={:>6} fn_scan={:>6} \
             cycle_steps={:>6} proof_clones={} proof_rows={}",
            ceiling,
            top.directory_builds,
            top.directory_row_visits,
            top.type_inst_scan_steps,
            top.fn_inst_scan_steps,
            top.cycle_walk_steps,
            top.proof_clones,
            top.proof_clone_rows,
        );
    }
}

/// The value-cycle audit walks the value-containment graph once at build time, so its
/// per-edge work is linear in the graph's edges — not the former sum over start nodes
/// of each start's reachable subgraph, which was quadratic on a chain. Measured on an
/// acyclic struct chain the shared scan visits each edge once (n steps at chain n), so
/// half→full doubles ~2x and the absolute count stays at the edge count. The
/// pre-repair per-start walk cost n(n+1)/2 steps (136 at chain 16, 528 at chain 32),
/// which fails both the linear ratio and the absolute ceiling below.
#[test]
fn value_cycle_walk_is_a_shared_linear_scan() {
    let half = counts_for(struct_chain_fixture(16));
    let full = counts_for(struct_chain_fixture(32));
    let walk_ratio = ratio(full.cycle_walk_steps, half.cycle_walk_steps);
    assert!(
        (1.7..=2.3).contains(&walk_ratio),
        "value-cycle walk is a shared linear scan; chain 16->32 steps {} -> {} \
         ratio {walk_ratio:.2}",
        half.cycle_walk_steps,
        full.cycle_walk_steps,
    );
    assert!(
        full.cycle_walk_steps < 100,
        "value-cycle walk regressed to a per-start quadratic: {} steps at chain 32",
        full.cycle_walk_steps,
    );
}

/// The proof fork is entered once per generic template, not per instantiation. With
/// exactly one generic function template the entry count is constant as
/// instantiations grow; the type-only fixture (no generic function template) never
/// enters it. This pins the axis-4 entry cardinality.
#[test]
fn proof_fork_entry_is_per_template_not_per_instantiation() {
    let (ty_ceiling, ty_top) = reachable_ceiling(type_axis_fixture, 64);
    let ty_half = counts_for(type_axis_fixture(ty_ceiling / 2));
    assert_eq!(
        ty_half.proof_clones, ty_top.proof_clones,
        "type-only fixture proof-fork entry is constant (0) across instantiation count"
    );

    let (fn_ceiling, fn_top) = reachable_ceiling(fn_axis_fixture, 64);
    let fn_half = counts_for(fn_axis_fixture(fn_ceiling / 2));
    assert_eq!(
        fn_top.proof_clones, 1,
        "one generic function template proves once"
    );
    assert_eq!(
        fn_half.proof_clones, fn_top.proof_clones,
        "proof-fork entry is constant per template across instantiation counts"
    );
}
