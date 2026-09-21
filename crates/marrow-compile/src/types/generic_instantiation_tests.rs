//! Generic instantiation over a widening type and function population, driven
//! through the production `compile` path.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use crate::compile::compile;

pub(super) fn project(source: String) -> ProjectInput {
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

/// Collection axis: `v` seeds, each applied through eight distinct `List`/`Map`/
/// `Option` shapes as the parameters of one function, so the program mints `8 * v`
/// distinct collection and reserved-enum instantiations without constructing a value
/// of any. Every mint validates its arguments through the registry's cached row
/// directory; the test pins that the population encodes deterministically as it
/// widens. That the directory is reused rather than rebuilt per mint is enforced by
/// construction, not by a clock: `MetadataScratch::try_new` is private to the
/// directory module with `RowDirectory::build_full` as its sole caller, and a
/// directory leaves the registry only through `RowDirectoryGuard::seat`.
fn collection_axis_fixture(v: usize) -> String {
    let mut source = String::from("module main\n\n");
    seed_structs(&mut source, v);
    source.push('\n');
    for seed in 0..v {
        writeln!(
            source,
            "fn take{seed}(a: List<N{seed}>, b: Map<int, N{seed}>, c: Option<N{seed}>, \
             d: List<Option<N{seed}>>, e: Map<string, List<N{seed}>>, f: Map<string, N{seed}>, \
             g: List<Map<int, N{seed}>>, h: Map<int, Option<N{seed}>>): int {{ return 0 }}"
        )
        .expect("write collection parameters");
    }
    source.push_str("\npub fn driver(): int { return 0 }\n");
    source
}

/// Every axis is deterministic across a widening instantiation population: the same
/// program compiles to the same image bytes however many instances it mints.
#[test]
fn every_instantiation_axis_compiles_reproducibly_as_it_widens() {
    for (axis, fixture) in [
        ("type", type_axis_fixture as fn(usize) -> String),
        ("function", fn_axis_fixture),
        ("collection", collection_axis_fixture),
    ] {
        for width in [128usize, 512] {
            let input = project(fixture(width));
            let first = compile(&input).expect("the widening fixture compiles cleanly");
            let second = compile(&input).expect("the widening fixture compiles cleanly");
            assert_eq!(
                first.image.bytes, second.image.bytes,
                "{axis} axis at {width} instantiations encodes identically",
            );
        }
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
    assert_eq!(row.code(), Code::CheckInstantiationLimit);
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

/// A finite chain of `links` generic structs, `Link0<T>` through `Link{links-1}<T>`,
/// each holding the next. The chain is resolved by a concrete function's parameter
/// annotation rather than by a call, so nothing constructs a value of it and the mint
/// nesting bound is the only bound the corpus can reach.
fn nested_chain_fixture(links: usize) -> String {
    let mut source = String::from("module main\n\n");
    for link in 0..links - 1 {
        writeln!(
            source,
            "struct Link{link}<T> {{ next: Link{}<T> }}",
            link + 1
        )
        .expect("write a chain link");
    }
    writeln!(source, "struct Link{}<T> {{ leaf: T }}", links - 1).expect("write the chain leaf");
    source.push_str(
        "\nfn sink(x: Link0<int>): int { return 0 }\n\npub fn driver(): int { return 0 }\n",
    );
    source
}

/// `MINT_DEPTH_LIMIT` bounds the nesting depth of a mint and nothing else: a chain
/// exactly that many links deep resolves, and one link more is refused. Filling is
/// iterative, so the depth a row is reserved at travels with its queued fill; a bound
/// read off the queue's length instead would refuse a wide shallow program and admit a
/// narrow deep one.
#[test]
fn the_mint_bound_admits_its_full_nesting_depth_and_refuses_one_link_more() {
    use marrow_codes::Code;

    use crate::compile::CompileFailure;
    use crate::types::MINT_DEPTH_LIMIT;

    compile(&project(nested_chain_fixture(MINT_DEPTH_LIMIT)))
        .expect("a chain as deep as the mint bound resolves");

    let Err(CompileFailure::Diagnostics(rows)) =
        compile(&project(nested_chain_fixture(MINT_DEPTH_LIMIT + 1)))
    else {
        panic!("one link past the mint bound must be refused");
    };
    assert_eq!(rows.as_slice().len(), 1);
    assert_eq!(rows.as_slice()[0].code(), Code::CheckInstantiationLimit);
    assert_eq!(
        rows.as_slice()[0].message(),
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
