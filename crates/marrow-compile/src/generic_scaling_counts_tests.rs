//! Deterministic operation-count KATs for generic instantiation scaling, driven
//! through the production `compile` path. These record exact operation counts (no
//! wall time, no noise) for each mapped owner and pin the decisive structural fact:
//! the reachable number of distinct instantiations is capped by the image bounds
//! (`MAX_TYPES` / `MAX_ENUMS` / `MAX_FUNCTIONS` = 64 each), far below the compiler's
//! `MAX_INSTANTIATIONS = 4096`. The per-mint directory rebuild and the
//! `(template, args)` primary-key scan are quadratic in the number of distinct
//! instantiations, but that number is bounded by ~64, so the absolute cost is a few
//! thousand primitive operations per compile — the packet's 512/1024/2048/4096
//! scaling points are unreachable on the beta image format.
//!
//! Counts are observed through the private `super::capture_scaling_counts` window;
//! they are neither a public hook nor a canonical fact. Freeze semantics: a
//! measurement-only close may not add an equivalent per-mint scan or rebuild site.
//! The reachable-ceiling and quadratic-shape assertions are the recurrence gate; if
//! the image type bounds are ever raised, this domain grows and the owners named
//! here must be revisited before that lane lands.

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

/// Type-only axis: `v` distinct `Held[Nk]` type instantiations, one per seed. Each
/// seed and each `Held` instance consumes an image record slot, so this axis is
/// bounded by `MAX_TYPES = 64` at roughly `v = 30`.
fn type_axis_fixture(v: usize) -> String {
    let mut source = String::from("module main\n\nstruct Held<T> { value: T }\n\n");
    seed_structs(&mut source, v);
    source.push_str("\npub fn driver(): int {\n");
    for seed in 0..v {
        writeln!(source, "    const h{seed} = Held(value: N{seed}(value: 0))")
            .expect("write held construction");
    }
    source.push_str("    return 0\n}\n");
    source
}

/// Function-only axis: `v` distinct `leaf[Nk]` function instantiations, one per seed.
/// Bounded by `MAX_FUNCTIONS = 64` (and by `MAX_TYPES` via the seeds).
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
/// with the scaling counts observed at that ceiling. Panics if even `v = 1` fails.
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

/// The compiler's shared instantiation budget, far above the reachable ceiling.
const MAX_INSTANTIATIONS: usize = super::MAX_INSTANTIATIONS;

/// The `(template, args)` mint-dedup reuse probe is a keyed lookup, so its work is
/// linear in the number of mint attempts (one probe each), not quadratic in the
/// number of distinct instantiations. The per-mint directory rebuild is deliberately
/// unchanged by the index repair (its build count and row work stay identical), so it
/// remains quadratic on the reachable domain; this test pins the repaired scan as
/// linear while holding the directory rebuild at its frozen shape.
#[test]
fn type_axis_scan_is_keyed_lookup_directory_rebuild_unchanged() {
    let (ceiling, top) = reachable_ceiling(type_axis_fixture, 64);

    assert!(
        ceiling < MAX_INSTANTIATIONS / 8,
        "distinct type instantiations are image-capped ({ceiling}) far below \
         MAX_INSTANTIATIONS ({MAX_INSTANTIATIONS}); the 512..4096 axis is unreachable"
    );

    let half = counts_for(type_axis_fixture(ceiling / 2));
    let row_ratio = ratio(top.directory_row_visits, half.directory_row_visits);
    let scan_ratio = ratio(top.type_inst_scan_steps, half.type_inst_scan_steps);
    // The unrepaired per-mint directory rebuild is still super-linear on the domain.
    assert!(
        row_ratio >= 2.6,
        "per-mint directory rebuild is super-linear; ceiling={ceiling} \
         row visits half={} full={} ratio={row_ratio:.2}",
        half.directory_row_visits,
        top.directory_row_visits
    );
    // The repaired keyed reuse probe is linear in mint attempts (one probe each).
    assert!(
        (1.7..=2.3).contains(&scan_ratio),
        "(template,args) reuse probe is a keyed lookup, linear in mint attempts; \
         ceiling={ceiling} probe steps half={} full={} ratio={scan_ratio:.2}",
        half.type_inst_scan_steps,
        top.type_inst_scan_steps
    );

    // The directory-build COUNT is ~linear (one rebuild per mint attempt): the
    // half→full ratio sits near 2x. A ratio band alone would still pass if every
    // count doubled at a constant factor, so the tight absolute ceilings below pin
    // the level and fail a doubled-rebuild regression that keeps the ratio.
    let build_ratio = ratio(top.directory_builds, half.directory_builds);
    assert!(
        (1.7..=2.3).contains(&build_ratio),
        "directory build count is ~linear in V; ceiling={ceiling} \
         builds half={} full={} ratio={build_ratio:.2}",
        half.directory_builds,
        top.directory_builds
    );

    // Tight absolute ceilings at the image-capped domain (observed: builds 129,
    // row_visits 2048 unchanged by the index repair; ty_scan 32 = one keyed probe per
    // mint attempt at ceiling 32, down from the pre-repair 496 linear scan). Each cap
    // is below twice the observed value, so a regression that reintroduces a linear
    // scan (or doubles the rebuild) fails here even with an unchanged half→full ratio.
    assert!(
        top.directory_builds < 170,
        "directory build count regressed past the frozen level: {} (ceiling={ceiling})",
        top.directory_builds
    );
    assert!(
        top.directory_row_visits < 3_000,
        "directory row work regressed past the frozen level: {} (ceiling={ceiling})",
        top.directory_row_visits
    );
    assert!(
        top.type_inst_scan_steps < 64,
        "reuse probe regressed to a linear scan: {} (ceiling={ceiling})",
        top.type_inst_scan_steps
    );
}

/// The function axis is bounded by `MAX_FUNCTIONS = 64`. The `reserve_fn_instance`
/// reuse probe is now a keyed lookup — linear in reserve attempts — and the function
/// axis drives no per-mint type directory rebuild, so the whole axis is linear after
/// the index repair.
#[test]
fn fn_axis_scan_is_keyed_lookup() {
    let (ceiling, top) = reachable_ceiling(fn_axis_fixture, 64);

    assert!(
        ceiling < MAX_INSTANTIATIONS / 8,
        "distinct function instantiations are image-capped ({ceiling}) far below \
         MAX_INSTANTIATIONS ({MAX_INSTANTIATIONS})"
    );

    let half = counts_for(fn_axis_fixture(ceiling / 2));
    let scan_ratio = ratio(top.fn_inst_scan_steps, half.fn_inst_scan_steps);
    assert!(
        (1.7..=2.3).contains(&scan_ratio),
        "reserve_fn_instance reuse probe is a keyed lookup, linear in reserve \
         attempts; ceiling={ceiling} probe steps half={} full={} ratio={scan_ratio:.2}",
        half.fn_inst_scan_steps,
        top.fn_inst_scan_steps
    );

    // The per-fn-mint directory rebuild count is ~linear; its ratio sits near 2x.
    let build_ratio = ratio(top.directory_builds, half.directory_builds);
    assert!(
        (1.7..=2.3).contains(&build_ratio),
        "directory build count is ~linear in V; ceiling={ceiling} \
         builds half={} full={} ratio={build_ratio:.2}",
        half.directory_builds,
        top.directory_builds
    );

    // Tight absolute ceilings at the image-capped domain (observed: builds 191;
    // fn_scan 63 = one keyed probe per reserve attempt at ceiling 63, down from the
    // pre-repair 1953 linear scan). The scan cap is below twice the observed value, so
    // a regression that reintroduces a linear scan fails here.
    assert!(
        top.directory_builds < 260,
        "directory build count regressed past the frozen level: {} (ceiling={ceiling})",
        top.directory_builds
    );
    assert!(
        top.fn_inst_scan_steps < 128,
        "reuse probe regressed to a linear scan: {} (ceiling={ceiling})",
        top.fn_inst_scan_steps
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
