//! Generic instantiation over a widening type and function population, driven
//! through the production `compile` path.

use std::fmt::Write as _;
use std::time::Instant;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

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

/// Wall time to compile `source` cleanly.
fn compile_time(source: String) -> f64 {
    let input = project(source);
    let start = Instant::now();
    compile(&input).expect("the scaling fixture compiles cleanly");
    start.elapsed().as_secs_f64()
}

/// Quadratic instantiation work would make a 4x population cost about 16x. The bound
/// is 8x, well clear of that and well clear of scheduling noise on a loaded machine;
/// this catches a regression to a per-instantiation rescan, not a constant factor.
#[test]
fn instantiation_work_stays_far_below_quadratic_on_both_axes() {
    for (axis, fixture) in [
        ("type", type_axis_fixture as fn(usize) -> String),
        ("function", fn_axis_fixture),
    ] {
        let base = compile_time(fixture(128));
        let wide = compile_time(fixture(512));
        // A base compile faster than a millisecond is dominated by fixed setup, and
        // its ratio measures nothing; only compare once the signal is above that.
        if base < 0.001 {
            continue;
        }
        let growth = wide / base;
        assert!(
            growth < 8.0,
            "{axis} axis: 4x the instantiations cost {growth:.1}x the time \
             ({base:.4}s -> {wide:.4}s); quadratic work would cost about 16x",
        );
    }
}

/// A generic type whose own field re-applies it grows without bound. The refusal
/// carries the instantiating expression's span, not the declaration's.
#[test]
fn recursive_struct_field_refusal_preserves_its_source_span() {
    use marrow_codes::Code;
    use marrow_syntax::SourceSpan;

    use crate::compile::CompileFailure;

    compile(&project(type_axis_fixture(1))).expect("Held<N0> must compile");

    let source = "module main\n\n\
struct Grow<T> {\n    leaf: T\n    next: Grow<List<T>>\n}\n\n\
fn deepen<T>(x: T): Grow<T> {\n    return deepen(x)\n}\n\n\
pub fn driver(): int {\n    const ignored = deepen(1)\n    return 0\n}\n";
    let Err(CompileFailure::Diagnostics(rows)) = compile(&project(source.to_string())) else {
        panic!("Grow must retain its typed source refusal");
    };
    assert_eq!(rows.as_slice().len(), 1);
    let row = &rows.as_slice()[0];
    assert_eq!(row.code(), Code::CheckInstantiationLimit.as_str());
    assert_eq!(row.file().as_str(), "src/main.mw");
    assert_eq!(
        row.span(),
        SourceSpan {
            start_byte: 89,
            end_byte: 96,
            line: 8,
            column: 21,
        }
    );
    assert_eq!(
        row.message(),
        "generic type instantiation reached the nesting limit of 256"
    );
}

#[test]
fn generic_struct_fields_keep_declared_order_and_types() {
    let source = r#"module main

struct Pair<A, B> {
    zeta: A
    alpha: B
}

pub fn driver(): int {
    const pair = Pair(alpha: true, zeta: 7)
    if pair.alpha {
        return pair.zeta
    }
    return 0
}
"#;
    compile(&project(source.to_string()))
        .expect("both generic field reads must retain their types");
}
