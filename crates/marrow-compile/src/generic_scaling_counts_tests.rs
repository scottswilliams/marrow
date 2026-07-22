//! Deterministic operation-count KATs for generic instantiation scaling, driven
//! through the production `compile` path. These record exact operation counts (no
//! wall time, no noise) for each mapped owner. The evidence-widened scale floor raised
//! the image type/function bounds (`MAX_TYPES` / `MAX_ENUMS` / `MAX_FUNCTIONS` = 4096),
//! so the 512/1024/2048 doubling points are now reachable and the scaling law is
//! exercised directly rather than argued from a ~64 cap.
//!
//! The `(template, args)` mint-dedup reuse probe is a keyed lookup, so its per-mint work
//! is one probe: the scan-step count equals the mint-attempt count and doubles ~2x across
//! a doubled axis. The mint/resolution directory is reused across a pass and extended for
//! the newly appended rows, so classifying a growing instantiation population is linear
//! (`nested_type_mint_directory_is_linear_in_instantiation_count`). The per-template proof
//! pass likewise reads that shared directory inside a savepoint rather than replaying the
//! population into a clone, so its row cost is the template body's own mint count, constant
//! as the population grows (`proof_clone_cost_is_independent_of_instantiation_population`).
//! The per-field-projection presentation path (exercised by the `.value.value` accesses on
//! the type axis) now reuses that same pass directory, so its row work is linear in the
//! instantiation count as well (`type_axis_scan_and_field_projection_rebuild_are_linear`).
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

/// Proof-cost axis: `v` distinct `Held<Nk>` instantiations minted at declare/fill time (in
/// the fields of `Uses`, so the settled instantiation population is `v` before the
/// template-proof loop runs), plus one generic function template `leaf` whose once-checked
/// proof pass mints exactly one instantiation of its own (`Held<T>` over the abstract
/// parameter). The pre-existing population and the proof's own work are thus separable: the
/// proof's row cost is `1` at every `v`, while a per-template full clone replayed all `v`
/// settled rows.
fn proof_cost_fixture(v: usize) -> String {
    let mut source =
        String::from("module main\n\nfn leaf<T>(x: T): Held<T> { return Held(value: x) }\n\n");
    source.push_str("struct Held<T> { value: T }\n\n");
    seed_structs(&mut source, v);
    source.push_str("\nstruct Uses {\n");
    for seed in 0..v {
        writeln!(source, "    f{seed}: Held<N{seed}>").expect("write held field");
    }
    source.push_str("}\n\npub fn driver(): int {\n    return 0\n}\n");
    source
}

/// The once-checked template proof pass runs directly on the in-progress registry inside a
/// savepoint, reading the shared already-built metadata directory instead of replaying the
/// settled instantiation population into a per-template clone. So its row cost is the number
/// of rows the template body itself mints — constant as the pre-existing population doubles.
/// Before this repair the proof cloned the whole registry and replayed every settled row, so
/// `proof_clone_rows` was the population size (`16 -> 32 -> 64` on this fixture); after it the
/// count is the template's own mint (`1`) at every width. The entry count stays one proof per
/// template (`proof_clones`), independent of instantiation count.
#[test]
fn proof_clone_cost_is_independent_of_instantiation_population() {
    let a = counts_for(proof_cost_fixture(16));
    let b = counts_for(proof_cost_fixture(32));
    let c = counts_for(proof_cost_fixture(64));

    assert_eq!(
        (a.proof_clones, b.proof_clones, c.proof_clones),
        (1, 1, 1),
        "one generic template proves once, regardless of instantiation population"
    );
    assert_eq!(
        (a.proof_clone_rows, b.proof_clone_rows, c.proof_clone_rows),
        (1, 1, 1),
        "the proof classifies only the rows its own body mints, not the settled population; \
         a per-template replay would grow this to the population size (16 / 32 / 64)"
    );
}

/// Collection axis: `v` distinct `List<Nk>` instantiations, one per seed struct, each in a
/// distinct function signature (so the axis is bounded by the type/function population, not
/// by `MAX_LOCALS`). Resolving each `use{k}` param type mints its `List<Nk>`, one
/// collection-mint attempt, so the keyed dedup index is probed once per attempt.
fn collection_axis_fixture(v: usize) -> String {
    let mut source = String::from("module main\n\n");
    seed_structs(&mut source, v);
    source.push('\n');
    for seed in 0..v {
        writeln!(
            source,
            "fn use{seed}(xs: List<N{seed}>): int {{ return 0 }}"
        )
        .expect("write list-param fn");
    }
    source.push_str("\npub fn driver(): int {\n    return 0\n}\n");
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

/// A function whose one parameter has a type nested `d` levels deep,
/// `Wrap<Wrap<...<int>>>`. Resolving the annotation mints `d` distinct type
/// instantiations (one per nesting level), each a separate top-level mint, exercising
/// the mint/dedup directory across a growing instantiation population without any value
/// construction or field projection.
fn nested_type_fixture(d: usize) -> String {
    let mut source = String::from("module main\n\nstruct Wrap<T> { inner: T }\n\n");
    source.push_str("pub fn driver(x: ");
    for _ in 0..d {
        source.push_str("Wrap<");
    }
    source.push_str("int");
    for _ in 0..d {
        source.push('>');
    }
    source.push_str("): int {\n    return 0\n}\n");
    source
}

/// A divergent-monomorphization shape whose instances deepen, terminating at a chosen
/// depth. A chain of `depth + 1` distinct generic functions `w0..w{depth}`: each `wk`
/// (k > 0) binds its parameter — a use site that renders the resolved type — and tail-
/// calls `w{k-1}(some(x))`, deepening the argument one `Option` level per hop; `w0` is the
/// leaf. The single driver call `w{depth}(0)` instantiates `wk<Option^(depth-k)<int>>` for
/// each k, so instance `k`'s one use-site type spelling is `O(depth - k)` characters and
/// the total instance spelling is `Σ = O(depth²)`. The deepening lives entirely in
/// instance bodies (each `some(x)` nests one level, well under the expression-nesting
/// limit), and the driver stays a single constant call — so a monomorphic-only render
/// budget is constant in `depth`. The instantiation population (`~2·depth`) stays well
/// under `MAX_INSTANTIATIONS`, so the program compiles cleanly rather than hitting the
/// bound.
fn divergent_hover_fixture(depth: usize) -> String {
    let mut source = String::from("module main\n\n");
    writeln!(
        source,
        "fn w0<T>(x: T): int {{\n    const y = x\n    return 0\n}}\n"
    )
    .expect("write leaf");
    for level in 1..=depth {
        writeln!(
            source,
            "fn w{level}<T>(x: T): int {{\n    const y = x\n    return w{}(some(x))\n}}\n",
            level - 1
        )
        .expect("write chain link");
    }
    writeln!(
        source,
        "pub fn driver(): int {{\n    return w{depth}(0)\n}}"
    )
    .expect("write driver");
    source
}

fn ratio(at_2v: usize, at_v: usize) -> f64 {
    assert!(at_v > 0, "1x count must be positive to form a ratio");
    at_2v as f64 / at_v as f64
}

/// The compiler's shared instantiation budget, reported by the manual scaling probe.
const MAX_INSTANTIATIONS: usize = super::MAX_INSTANTIATIONS;

/// The `(template, args)` mint-dedup reuse probe is a keyed lookup, so its scan-step
/// count equals the mint-attempt count and doubles ~2x across a doubled type axis —
/// proven directly at the now-reachable 512→1024 and 1024→2048 doubling points. The
/// mint/resolution directory is reused and extended (see
/// `nested_type_mint_directory_is_linear_in_instantiation_count`), and this fixture also
/// projects two struct fields per seed (`.value.value`). Each field-projection session
/// now reuses that same pass directory and classifies only the rows appended since the
/// previous probe, so classifying a growing instantiation population through the
/// projection path is linear: row work doubles ~2x per doubling rather than quadrupling.
/// (The type axis reaches ~`MAX_TYPES / 2` because each instantiation costs its seed plus
/// its `Held` record; the near-4096 point is exercised on the fn axis, which reaches
/// ~`MAX_FUNCTIONS − 1`.)
#[test]
fn type_axis_scan_and_field_projection_rebuild_are_linear() {
    let a = counts_for(type_axis_fixture(512));
    let b = counts_for(type_axis_fixture(1024));
    let c = counts_for(type_axis_fixture(2048));

    // One keyed probe per mint attempt, so the scan count is the axis width and each
    // doubling is ~2x.
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

    // Repaired: the field-projection path reuses the pass directory and classifies each
    // appended row once, so total row work is linear in the instantiation count — each
    // doubling is ~2x rather than the former ~4x rebuild-over-every-prior-row.
    for (lo, hi) in [(&a, &b), (&b, &c)] {
        let row_ratio = ratio(hi.directory_row_visits, lo.directory_row_visits);
        assert!(
            (1.7..=2.3).contains(&row_ratio),
            "field-projection directory reuse classifies each row once, linear in the \
             instantiation count; row visits {} -> {} ratio {row_ratio:.2}",
            lo.directory_row_visits,
            hi.directory_row_visits,
        );
    }
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

/// Editor hover facts are rendered only for the monomorphic bodies the editor queries;
/// a generic instance's use-site spans duplicate its template's, so its facts are
/// discarded and its use-site type spellings are never rendered. On a divergent
/// monomorphization (`wrap` called at `Optionᵏ<int>` for growing `k`), the pre-repair
/// per-instance render made the total spelling work `Σ O(k) = O(instances²)`; the repair
/// holds `hover_spelling_chars` to the driver's monomorphic baseline — one constant-width
/// signature display per generic call, so it grows ~linearly (2x per axis doubling) and
/// stays under a linear ceiling. Before the repair the same measurements quadruple per
/// doubling and blow past the ceiling (this is the recurrence gate: a future eager
/// per-instance render fails here instead of silently reinflating the warm suite).
#[test]
fn instance_hover_spelling_is_not_rendered_so_the_axis_is_linear() {
    let a = counts_for(divergent_hover_fixture(64));
    let b = counts_for(divergent_hover_fixture(128));
    let c = counts_for(divergent_hover_fixture(256));

    for (lo, hi) in [(&a, &b), (&b, &c)] {
        let spelling_ratio = ratio(hi.hover_spelling_chars, lo.hover_spelling_chars);
        assert!(
            spelling_ratio <= 2.3,
            "hover spelling work is the monomorphic driver baseline, not super-linear in \
             the instantiation depth; chars {} -> {} ratio {spelling_ratio:.2} \
             (a per-instance render quadruples per doubling)",
            lo.hover_spelling_chars,
            hi.hover_spelling_chars,
        );
    }

    // Absolute ceiling: the retained work is the driver's single generic call, so it is a
    // small constant independent of the chain depth. A per-instance deep-type render
    // (`Σ = O(depth²)`, ~3·256² ≈ 200k chars at depth=256) blows past this.
    assert!(
        c.hover_spelling_chars < 256,
        "hover spelling work is the constant monomorphic baseline, not quadratic in the \
         instantiation depth: {} chars at depth=256",
        c.hover_spelling_chars,
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

/// The `instantiate_collection` reuse probe is a keyed lookup into the collection dedup
/// index (CF3) — one keyed probe per collection-mint attempt — so its probe-step count is
/// the number of distinct collection instantiations and doubles ~2x across a doubled axis.
/// The former linear spec scan made per-attempt work O(collections), i.e. O(collections²)
/// over a compile; this pins the repaired keyed-lookup linearity so a regression is
/// conspicuous.
#[test]
fn collection_axis_dedup_probe_is_linear() {
    let a = counts_for(collection_axis_fixture(256));
    let b = counts_for(collection_axis_fixture(512));
    let c = counts_for(collection_axis_fixture(1024));
    for (lo, hi) in [(&a, &b), (&b, &c)] {
        let ratio = ratio(hi.coll_inst_probe_steps, lo.coll_inst_probe_steps);
        assert!(
            (1.7..=2.3).contains(&ratio),
            "collection dedup probe is a keyed lookup, linear in mint attempts; \
             steps {} -> {} ratio {ratio:.2}",
            lo.coll_inst_probe_steps,
            hi.coll_inst_probe_steps,
        );
    }
    // A constant number of keyed probes per distinct collection (the param type is
    // resolved in both the check and lower passes), so the total is proportional to the
    // collection population — linear, not the former O(collections²) scan.
    assert_eq!(
        c.coll_inst_probe_steps,
        2 * 1024,
        "keyed probes are a small constant per distinct collection mint at v=1024",
    );
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

/// Minting a type nested `d` levels deep classifies each instantiation row once, not
/// once per level: the mint/resolution directory is reused across the pass's probes and
/// extended for the newly appended rows, so its row-classification work is linear in the
/// instantiation count. Before this repair each nested level rebuilt the directory over
/// every prior row, so `directory_row_visits` quadrupled (roughly `2 * d^2`) per depth
/// doubling and `directory_builds` grew ~linearly; both are pinned linear here at the
/// 32→64 and 64→128 doublings. The remaining directory rebuild on the width + field
/// projection axis (the `type_axis` KAT above) is a distinct owner — presentation and
/// field-projection sessions each build a fresh directory — and stays super-linear.
#[test]
fn nested_type_mint_directory_is_linear_in_instantiation_count() {
    let a = counts_for(nested_type_fixture(32));
    let b = counts_for(nested_type_fixture(64));
    let c = counts_for(nested_type_fixture(128));

    // One classification per level, not one rescan of every prior row per level: the
    // row-visit count is proportional to the depth and doubles ~2x per doubling.
    for (lo, hi) in [(&a, &b), (&b, &c)] {
        let visit_ratio = ratio(hi.directory_row_visits, lo.directory_row_visits);
        assert!(
            (1.7..=2.3).contains(&visit_ratio),
            "mint directory row visits are linear in depth; visits {} -> {} ratio {visit_ratio:.2}",
            lo.directory_row_visits,
            hi.directory_row_visits,
        );
    }

    // A bounded, depth-independent number of full directory builds seeds the reuse; the
    // remaining classification is incremental extension, which is not a full build.
    assert!(
        a.directory_builds == b.directory_builds && b.directory_builds == c.directory_builds,
        "full directory builds are constant across depth: {} / {} / {}",
        a.directory_builds,
        b.directory_builds,
        c.directory_builds,
    );
    assert!(
        c.directory_builds <= 4,
        "the reused directory is seeded by a small constant number of full builds, not one \
         per level: {} at depth 128",
        c.directory_builds,
    );
}
