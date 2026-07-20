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

/// The reachable number of distinct type instantiations is bounded by the image
/// record cap (~30, well below `MAX_INSTANTIATIONS = 4096`), and the per-mint
/// directory rebuild plus `(template, args)` scan are quadratic on that capped
/// domain. The absolute operation count at the ceiling stays in the low thousands.
#[test]
fn type_axis_is_image_capped_and_quadratic_on_that_domain() {
    let (ceiling, top) = reachable_ceiling(type_axis_fixture, 64);

    assert!(
        ceiling < MAX_INSTANTIATIONS / 8,
        "distinct type instantiations are image-capped ({ceiling}) far below \
         MAX_INSTANTIATIONS ({MAX_INSTANTIATIONS}); the 512..4096 axis is unreachable"
    );

    // Quadratic shape on the reachable domain: half vs. full ceiling.
    let half = counts_for(type_axis_fixture(ceiling / 2));
    let row_ratio = ratio(top.directory_row_visits, half.directory_row_visits);
    let scan_ratio = ratio(top.type_inst_scan_steps, half.type_inst_scan_steps);
    assert!(
        row_ratio >= 2.6,
        "per-mint directory rebuild is super-linear; ceiling={ceiling} \
         row visits half={} full={} ratio={row_ratio:.2}",
        half.directory_row_visits,
        top.directory_row_visits
    );
    assert!(
        scan_ratio >= 2.6,
        "(template,args) primary-key scan is super-linear; ceiling={ceiling} \
         scan steps half={} full={} ratio={scan_ratio:.2}",
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
    // row_visits 2048, ty_scan 496 at ceiling 32). Each cap is below twice the
    // observed value, so a 2x constant-factor rebuild/scan regression fails here
    // even when the half→full ratio is unchanged.
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
        top.type_inst_scan_steps < 800,
        "type-inst scan work regressed past the frozen level: {} (ceiling={ceiling})",
        top.type_inst_scan_steps
    );
}

/// The function axis is bounded by `MAX_FUNCTIONS = 64` and is quadratic on that
/// capped domain through the `reserve_fn_instance` scan and the per-mint directory
/// rebuild; absolute cost at the ceiling stays small.
#[test]
fn fn_axis_is_image_capped_and_quadratic_on_that_domain() {
    let (ceiling, top) = reachable_ceiling(fn_axis_fixture, 64);

    assert!(
        ceiling < MAX_INSTANTIATIONS / 8,
        "distinct function instantiations are image-capped ({ceiling}) far below \
         MAX_INSTANTIATIONS ({MAX_INSTANTIATIONS})"
    );

    let half = counts_for(fn_axis_fixture(ceiling / 2));
    let scan_ratio = ratio(top.fn_inst_scan_steps, half.fn_inst_scan_steps);
    assert!(
        scan_ratio >= 2.6,
        "reserve_fn_instance scan is super-linear; ceiling={ceiling} \
         scan steps half={} full={} ratio={scan_ratio:.2}",
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

    // Tight absolute ceilings at the image-capped domain (observed: builds 191,
    // fn_scan 1953 at ceiling 63). Each cap is below twice the observed value, so a
    // 2x constant-factor rebuild/scan regression fails here even with an unchanged
    // half→full ratio.
    assert!(
        top.directory_builds < 260,
        "directory build count regressed past the frozen level: {} (ceiling={ceiling})",
        top.directory_builds
    );
    assert!(
        top.fn_inst_scan_steps < 3_000,
        "fn-inst scan work regressed past the frozen level: {} (ceiling={ceiling})",
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
