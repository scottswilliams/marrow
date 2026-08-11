//! Durable value-shape accounting (VALDAG01): the corpora that fix the depth
//! decision and the expansion cost of a durable field's stored value.
//!
//! A durable field's value is a reference into the program's acyclic value-shape
//! graph, not an occurrence tree. Three properties are pinned here, each against the
//! production `compile()` path:
//!
//! 1. **Cost.** Nesting is a shared subgraph, so admitting or refusing a value costs
//!    work in the unique value types and their declared edges. A project whose
//!    *expanded* occurrence tree is exponential in its nesting depth must still be
//!    decided promptly, with the same typed outcome. [red R28]
//! 2. **Depth.** A value type reached at two different depths is one node with one
//!    depth: the longest path from a top-level field value down to it. The decision
//!    may not depend on which occurrence the walk visits first, so the refuse/admit
//!    boundary is pinned in both field orders. [red R29]
//! 3. **Location.** The over-deep report keeps the exact code, message, and span it
//!    has today, for a struct leaf, an enum payload leaf, and a terminal scalar leaf,
//!    at both the admitting and the refusing level. [red R32]
//! 4. **Bytes.** A value graph that *fits* encodes to the exact bytes and the exact
//!    durable-contract identity it encoded to before the graph replaced the occurrence
//!    tree — a diamond, where the shared shape is the thing an interned graph
//!    represents differently. [red R31]

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use marrow_compile::{CompileFailure, ResourceLimitKind, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

#[path = "common/ids.rs"]
mod ids;

use ids::ledger;

fn project(source: &str, ids: Option<&[u8]>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, ids, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// The ledger anchors a single keyed store `^a` over resource `R` with one durable
/// field `R.f` needs, plus any extra anchors the corpus declares.
fn store_ledger(extra: &[&str]) -> Vec<u8> {
    let mut anchors: Vec<&str> = vec![
        "application .",
        "product R",
        "field R.f",
        "root a",
        "key a.id",
    ];
    anchors.extend_from_slice(extra);
    ledger(&anchors)
}

/// The source diagnostics of a failed compile, or a panic naming the arm reached
/// instead.
fn diagnostics(result: Result<impl std::fmt::Debug, CompileFailure>) -> Vec<SourceDiagnostic> {
    match result {
        Ok(compiled) => panic!("expected diagnostics, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

/// Every `check.resource_limit` row of a failed compile, rendered exactly as the CLI
/// spells it: `<file>:<line>:<column>: <code>: <message>`. This is the frozen form
/// red R32 compares, so a moved span or a reworded message fails the comparison.
fn located_resource_limits(result: Result<impl std::fmt::Debug, CompileFailure>) -> Vec<String> {
    diagnostics(result)
        .iter()
        .filter(|diagnostic| diagnostic.code() == "check.resource_limit")
        .map(|diagnostic| {
            format!(
                "{}:{}:{}: {}: {}",
                diagnostic.file().as_str(),
                diagnostic.line(),
                diagnostic.column(),
                diagnostic.code(),
                diagnostic.message(),
            )
        })
        .collect()
}

/// The exact located row IMGPROJ01 froze for an over-deep durable field value: the
/// store declaration's own line, at column 1.
fn over_deep_row(line: u32) -> String {
    format!(
        "src/main.mw:{line}:1: check.resource_limit: a durable field value nests structs \
         or enums deeper than the fixed limit of 32 levels"
    )
}

// ---- Red R28: expansion cost is the unique value graph, not the occurrence tree.

/// A single-root project whose one durable field nests `levels` distinct structs,
/// each with `fanout` fields referencing the level below, terminating in one scalar.
///
/// Every declared bound holds: the value is `levels + 2` levels deep (well inside
/// `MAX_DURABLE_VALUE_DEPTH` = 32) and each struct carries `fanout` leaves (well
/// inside `MAX_STRUCT_LEAVES` = 64). The *expanded* occurrence tree, however, has
/// `fanout ^ levels` leaves, so any representation that materializes one — in the
/// compiler, in the contract preimage, or in the DURABLE section — costs time
/// exponential in `levels`.
fn nested_struct_fanout(levels: usize, fanout: usize) -> ProjectInput {
    let mut source = String::from("module main\n\nstruct S0 {\n    v: int\n}\n");
    for level in 1..=levels {
        let _ = writeln!(source, "struct S{level} {{");
        for field in 1..=fanout {
            let _ = writeln!(source, "    f{field}: S{}", level - 1);
        }
        source.push_str("}\n");
    }
    let _ = write!(
        source,
        "\nresource R {{\n    required f: S{levels}\n}}\n\nstore ^a[id: int]: R\n\n\
         pub fn plain(n: int): int {{\n    return n + 1\n}}\n"
    );
    project(&source, Some(&store_ledger(&[])))
}

/// The wall-clock budget for one whole-project compile of a corpus whose expanded
/// occurrence tree is exponential. The work the invariant permits is linear in the
/// unique value nodes and declared edges (a few dozen here) plus the bytes the
/// bounded sink actually emits, so the true cost is milliseconds; the budget is two
/// orders of magnitude above that, and two orders of magnitude below the base
/// tree-materializing cost, so it distinguishes the two without timing precision.
const EXPANSION_BUDGET: Duration = Duration::from_secs(20);

/// 14 levels of 4 fields is 268,435,456 expanded leaves over 16 declared struct
/// types. Expanding that tree is minutes of CPU; deciding it from the value graph is
/// immediate. The typed outcome is unchanged either way: the DURABLE body this value
/// would occupy is far past `MAX_IMAGE_BYTES`, so the compile reaches the aggregate
/// `ImageBytes` resource limit.
#[test]
fn an_exponentially_expanded_value_is_decided_in_its_unique_nodes() {
    let input = nested_struct_fanout(14, 4);
    let started = Instant::now();
    let result = compile(&input);
    let elapsed = started.elapsed();
    match result {
        Err(CompileFailure::ResourceLimit(limit))
            if limit.kind() == ResourceLimitKind::ImageBytes => {}
        other => panic!("expected the ImageBytes aggregate limit, got {other:?}"),
    }
    assert!(
        elapsed < EXPANSION_BUDGET,
        "deciding a value with 4^14 expanded leaves took {elapsed:?}, over the \
         {EXPANSION_BUDGET:?} budget: the expanded occurrence tree is being materialized",
    );
}

// ---- Red R29: one type reached at two depths has one longest-path depth.

/// A diamond: struct `D` is both a top-level durable field value (depth 1) and the
/// base of a `chain`-long nesting chain under a second field. `D`'s scalar leaf
/// therefore sits at depth 2 through one field and at depth `chain + 2` through the
/// other, so the two occurrences of one type disagree about depth and only the
/// longest path decides the bound.
///
/// `field_order` is the declaration order of the two fields. The decision must not
/// depend on it: a walk that visits the shallow occurrence first and dedupes by
/// first visit would admit an over-deep value, and one that visits the deep
/// occurrence first and dedupes would refuse a fitting one.
fn depth_diamond(chain: usize, shallow_first: bool) -> ProjectInput {
    let mut source =
        String::from("module main\n\nstruct D {\n    v: int\n}\nstruct C1 {\n    inner: D\n}\n");
    for level in 2..=chain {
        let _ = writeln!(source, "struct C{level} {{\n    inner: C{}\n}}", level - 1);
    }
    source.push_str("\nresource R {\n");
    let shallow = "    required f: D\n";
    let deep = format!("    required g: C{chain}\n");
    match shallow_first {
        true => {
            source.push_str(shallow);
            source.push_str(&deep);
        }
        false => {
            source.push_str(&deep);
            source.push_str(shallow);
        }
    }
    source.push_str(
        "}\n\nstore ^a[id: int]: R\n\npub fn plain(n: int): int {\n    return n + 1\n}\n",
    );
    project(&source, Some(&store_ledger(&["field R.g"])))
}

/// `D`'s leaf sits at depth 32 through the deep field — exactly
/// `MAX_DURABLE_VALUE_DEPTH` — so the whole value admits, in either field order.
#[test]
fn a_diamond_at_the_depth_bound_admits_in_both_field_orders() {
    for shallow_first in [true, false] {
        compile(&depth_diamond(30, shallow_first)).unwrap_or_else(|failure| {
            panic!("a diamond whose deepest path ends at depth 32 admits (shallow_first={shallow_first}): {failure:?}")
        });
    }
}

/// One level deeper the same shared type's longest path reaches 33 and the value
/// refuses — again in either field order, and with the located report at the store
/// declaration. The shallow occurrence of the very same type stays within the bound,
/// so a per-node metric that kept only the first depth it saw would decide this
/// wrongly in one direction or the other.
#[test]
fn a_diamond_one_level_past_the_bound_refuses_in_both_field_orders() {
    for shallow_first in [true, false] {
        // The store declaration follows `D`, `C1..=C31`, the resource, and the blank
        // lines the corpus writes between them.
        let rows = located_resource_limits(compile(&depth_diamond(31, shallow_first)));
        assert_eq!(
            rows.len(),
            1,
            "an over-deep diamond reports the depth bound once (shallow_first={shallow_first})",
        );
        assert!(
            rows[0].ends_with(
                ": check.resource_limit: a durable field value nests structs or enums \
                 deeper than the fixed limit of 32 levels"
            ),
            "unexpected report: {rows:?}",
        );
    }
}

// ---- Red R32: the located depth report is preserved exactly.

/// A durable field whose value nests `chain` structs over `leaf`, so the leaf sits at
/// depth `chain + 1`. `store_line` is the 1-based line the `store ^a` declaration
/// lands on, which is where the located report is anchored.
fn leaf_chain(
    chain: usize,
    leaf: &str,
    extra_anchors: &[&str],
    prelude: &str,
) -> (ProjectInput, u32) {
    let mut source = String::from("module main\n\n");
    source.push_str(prelude);
    let _ = writeln!(source, "struct S0 {{\n    x: {leaf}\n}}");
    for level in 1..=chain.saturating_sub(1) {
        let _ = writeln!(source, "struct S{level} {{\n    s: S{}\n}}", level - 1);
    }
    let _ = write!(
        source,
        "\nresource R {{\n    required f: S{}\n}}\n\nstore ^a[id: int]: R\n\n\
         pub fn plain(n: int): int {{\n    return n + 1\n}}\n",
        chain - 1
    );
    let store_line = source
        .lines()
        .position(|line| line.starts_with("store ^a"))
        .expect("the corpus declares a store") as u32
        + 1;
    (
        project(&source, Some(&store_ledger(extra_anchors))),
        store_line,
    )
}

/// A terminating scalar occupies a level of its own. At 31 enclosing structs the
/// scalar sits at depth 32 and admits; at 32 it sits at depth 33 and draws exactly
/// the frozen located row.
#[test]
fn a_terminal_scalar_leaf_reports_at_the_frozen_span() {
    let (fitting, _) = leaf_chain(31, "int", &[], "");
    compile(&fitting).expect("a scalar leaf at depth 32 admits");

    let (over_deep, store_line) = leaf_chain(32, "int", &[], "");
    assert_eq!(
        located_resource_limits(compile(&over_deep)),
        vec![over_deep_row(store_line)],
    );
}

/// A struct leaf one level past the bound reports the same row: the value that
/// terminates the chain is a nested product, so the enclosing structs all fit while
/// the product's own leaf does not.
#[test]
fn a_struct_leaf_reports_at_the_frozen_span() {
    let prelude = "struct Leaf {\n    v: int\n}\n\n";
    let (fitting, _) = leaf_chain(30, "Leaf", &[], prelude);
    compile(&fitting).expect("a struct leaf whose scalar lands at depth 32 admits");

    let (over_deep, store_line) = leaf_chain(31, "Leaf", &[], prelude);
    assert_eq!(
        located_resource_limits(compile(&over_deep)),
        vec![over_deep_row(store_line)],
    );
}

/// An enum payload leaf is measured the same way: the enum sits one level above its
/// payload values, so a payload one level past the bound is over-deep even though the
/// enum that carries it fits.
#[test]
fn an_enum_payload_leaf_reports_at_the_frozen_span() {
    let prelude = "enum Leaf {\n    none\n    some(v: int)\n}\n\n";
    let anchors = ["sum Leaf", "member Leaf.none", "member Leaf.some"];
    let (fitting, _) = leaf_chain(30, "Leaf", &anchors, prelude);
    compile(&fitting).expect("an enum payload leaf at depth 32 admits");

    let (over_deep, store_line) = leaf_chain(31, "Leaf", &anchors, prelude);
    assert_eq!(
        located_resource_limits(compile(&over_deep)),
        vec![over_deep_row(store_line)],
    );
}

// ---- The one-row-per-over-deep-field law (B11) and its sibling.

/// Every located row of a failed compile carrying `code`, in report order, rendered
/// exactly as the CLI spells it.
fn located_rows(result: Result<impl std::fmt::Debug, CompileFailure>, code: &str) -> Vec<String> {
    diagnostics(result)
        .iter()
        .filter(|diagnostic| diagnostic.code() == code)
        .map(|diagnostic| {
            format!(
                "{}:{}:{}: {}: {}",
                diagnostic.file().as_str(),
                diagnostic.line(),
                diagnostic.column(),
                diagnostic.code(),
                diagnostic.message(),
            )
        })
        .collect()
}

/// The recorded B11 probe: a 32-level chain `C0..=C31`, a struct `W` holding that
/// chain twice, and a resource storing `W` in one field and the chain directly in
/// another, projected by two stores.
///
/// Both durable fields are over-deep. The expanded occurrence tree has *three*
/// over-deep leaves per store — `f.a`, `f.b`, and `g` — so an accounting that decides
/// at the leaf reports three identical rows per store. Deciding at the field root,
/// which is what makes the value graph a graph, reports one row per over-deep field.
fn two_over_deep_fields_over_two_stores() -> (ProjectInput, u32) {
    let mut source = String::from("module main\n\nstruct C0 {\n    v: int\n}\n");
    for level in 1..=31 {
        let _ = writeln!(source, "struct C{level} {{\n    inner: C{}\n}}", level - 1);
    }
    source.push_str(
        "\nstruct W {\n    a: C31\n    b: C31\n}\n\n\
         resource R {\n    required f: W\n    required g: C31\n}\n\n\
         store ^a[id: int]: R\nstore ^b[id: int]: R\n\n\
         pub fn plain(n: int): int {\n    return n + 1\n}\n",
    );
    let store_line = source_line(&source, "store ^a");
    (
        project(
            &source,
            Some(&store_ledger(&["field R.g", "root b", "key b.id"])),
        ),
        store_line,
    )
}

/// One over-deep durable field draws one located refusal, whatever the multiplicity
/// of over-deep leaves its expansion would have had. The message and span are
/// unchanged, and the per-store ordering is unchanged: this is a strict subset of the
/// rows the leaf-counting accounting emitted, and preserving the multiplicity would
/// require building the expansion red R28 deletes.
#[test]
fn one_over_deep_field_draws_one_row_however_many_leaves_its_expansion_would_have() {
    let (input, store_a) = two_over_deep_fields_over_two_stores();
    let rows = located_resource_limits(compile(&input));
    let store_b = store_a + 1;
    assert_eq!(
        rows,
        vec![
            over_deep_row(store_a),
            over_deep_row(store_a),
            over_deep_row(store_b),
            over_deep_row(store_b),
        ],
        "two over-deep durable fields draw two rows per store, not one per over-deep \
         leaf of the expansion",
    );
}

/// The 1-based line of the first line of `source` starting with `prefix`.
fn source_line(source: &str, prefix: &str) -> u32 {
    source
        .lines()
        .position(|line| line.starts_with(prefix))
        .expect("the corpus declares the line") as u32
        + 1
}

/// The same law for the value types the durable set excludes: a struct holding a
/// collection is refused once per store, not once per durable field that stores it.
/// The report is the same located `check.unsupported` row either way, so this is the
/// same strict subset the depth row takes.
#[test]
fn an_unsupported_value_type_draws_one_row_however_many_fields_store_it() {
    let source = "module main\n\n\
         struct Bad {\n    c: List<int>\n}\n\n\
         resource R {\n    required f: Bad\n    required g: Bad\n}\n\n\
         store ^a[id: int]: R\n\n\
         pub fn plain(n: int): int {\n    return n + 1\n}\n";
    let input = project(source, Some(&store_ledger(&["field R.g"])));
    let store_line = source_line(source, "store ^a");
    assert_eq!(
        located_rows(compile(&input), "check.unsupported"),
        vec![format!(
            "src/main.mw:{store_line}:1: check.unsupported: a collection stored directly \
             in a durable field (a large collection belongs under a keyed branch) is not \
             yet supported on the beta line"
        )],
    );
}

// ---- Red R30: a hostile compact expansion costs the bytes the ceiling admits.

/// The frozen hostile corpus: one scalar base under exactly 31 enclosing struct
/// levels, each carrying `MAX_STRUCT_LEAVES` = 64 fields referencing the level below.
///
/// Nothing here is over any declared bound. The outer value is depth 32 — exactly
/// `MAX_DURABLE_VALUE_DEPTH` — and every level is exactly at the leaf bound. What the
/// v0 wire would have to spell is `64^31` leaves, some 55 orders of magnitude past
/// `MAX_IMAGE_BYTES`, while the value graph that describes it is 32 nodes and 1,984
/// declared edges. This is the corpus that separates the two costs: a representation
/// that expands is not slow here, it does not terminate.
fn hostile_compact_expansion() -> ProjectInput {
    let mut source = String::from("module main\n\nstruct S0 {\n    v: int\n}\n");
    for level in 1..=30 {
        let _ = writeln!(source, "struct S{level} {{");
        for field in 1..=64 {
            let _ = writeln!(source, "    f{field}: S{}", level - 1);
        }
        source.push_str("}\n");
    }
    let _ = write!(
        source,
        "\nresource R {{\n    required f: S30\n}}\n\nstore ^a[id: int]: R\n\n\
         pub fn plain(n: int): int {{\n    return n + 1\n}}\n"
    );
    project(&source, Some(&store_ledger(&[])))
}

/// The repeated enum-payload analogue, at the same total depth. An enum payload leaf
/// is a scalar on the beta line, so the repetition is carried by the enclosing struct
/// levels: `Opt` sits at depth 2 and 30 enclosing levels of 64 fields each reference
/// it, so the outer value is depth 32 and the expansion writes the enum's sum and
/// member identities `64^30` times over. The enum arm of the expansion is what this
/// corpus drives — a header, two ledger identities, and a member count per occurrence
/// — rather than the scalar arm the struct corpus repeats.
fn hostile_enum_payload_expansion() -> ProjectInput {
    let mut source = String::from("module main\n\nenum Opt {\n    none\n    some(v: int)\n}\n");
    for level in 1..=30 {
        let _ = writeln!(source, "struct S{level} {{");
        for field in 1..=64 {
            match level {
                1 => {
                    let _ = writeln!(source, "    f{field}: Opt");
                }
                _ => {
                    let _ = writeln!(source, "    f{field}: S{}", level - 1);
                }
            }
        }
        source.push_str("}\n");
    }
    let _ = write!(
        source,
        "\nresource R {{\n    required f: S30\n}}\n\nstore ^a[id: int]: R\n\n\
         pub fn plain(n: int): int {{\n    return n + 1\n}}\n"
    );
    project(
        &source,
        Some(&store_ledger(&[
            "sum Opt",
            "member Opt.none",
            "member Opt.some",
        ])),
    )
}

/// The hostile corpus is decided, promptly, in the aggregate byte ceiling.
///
/// No stage may expand it: the compiler holds 32 value nodes, the producer's
/// durable-body lower bound stops at the first byte past `MAX_IMAGE_BYTES`, and the
/// contract identity is never computed over bytes no image can carry. The reported
/// outcome is the aggregate `ImageBytes` limit, which is what a body larger than the
/// whole-image ceiling has always meant.
#[test]
fn a_hostile_compact_expansion_is_decided_without_being_expanded() {
    for (name, input) in [
        ("31 struct levels x 64 fields", hostile_compact_expansion()),
        (
            "30 struct levels x 64 fields over a repeated enum",
            hostile_enum_payload_expansion(),
        ),
    ] {
        let started = Instant::now();
        let result = compile(&input);
        let elapsed = started.elapsed();
        match result {
            Err(CompileFailure::ResourceLimit(limit))
                if limit.kind() == ResourceLimitKind::ImageBytes => {}
            other => panic!("{name}: expected the ImageBytes aggregate limit, got {other:?}"),
        }
        assert!(
            elapsed < EXPANSION_BUDGET,
            "{name}: took {elapsed:?}, over the {EXPANSION_BUDGET:?} budget — an \
             expansion is being materialized",
        );
    }
}

// ---- Red R31: a fitting diamond keeps its exact v0 bytes and contract identity.

/// The fitting small-diamond corpus: struct `Leaf` is the value of one durable field
/// *and* both leaves of `Mid`, which is the value of another. One shape, two depths,
/// three occurrences — the case where an interned graph and an occurrence tree differ
/// in representation and must not differ in a single byte.
fn small_diamond() -> ProjectInput {
    let source = "module main\n\n\
         struct Leaf {\n    v: int\n    w: string\n}\n\
         struct Mid {\n    a: Leaf\n    b: Leaf\n}\n\n\
         resource R {\n    required f: Leaf\n    required g: Mid\n}\n\n\
         store ^a[id: int]: R\n\n\
         pub fn plain(n: int): int {\n    return n + 1\n}\n";
    project(source, Some(&store_ledger(&["field R.g"])))
}

/// The DURABLE section (0x03) body of an encoded image. The container is magic (4),
/// version (1), and image id (32), then a section count and one
/// `u8(id) ‖ u32_be(len) ‖ body` frame per section in emission order.
fn durable_section(image: &[u8]) -> &[u8] {
    let mut cursor = 4 + 1 + 32;
    let sections = image[cursor];
    cursor += 1;
    for _ in 0..sections {
        let id = image[cursor];
        let len = u32::from_be_bytes(
            image[cursor + 1..cursor + 5]
                .try_into()
                .expect("a four-byte section length"),
        ) as usize;
        cursor += 5;
        if id == 0x03 {
            return &image[cursor..cursor + len];
        }
        cursor += len;
    }
    panic!("the image carries a DURABLE section");
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The diamond's DURABLE section and whole-image identity are byte-exact against the
/// values recomputed at this lane's base `98b1665d`, before the value-shape graph
/// replaced the recursively cloned occurrence tree. The section ends in the 32-byte
/// durable-contract identity, so this pins the contract preimage too: the shared shape
/// is spelled once per occurrence on the wire, as v0 has always spelled it, and only
/// the retained representation changed.
#[test]
fn a_fitting_diamond_keeps_its_exact_bytes_and_contract_identity() {
    let compiled = compile(&small_diamond()).expect("the small diamond fits every bound");
    let section = durable_section(&compiled.image.bytes);
    assert_eq!(hex(section), DIAMOND_DURABLE_SECTION);
    assert_eq!(
        hex(&section[section.len() - 32..]),
        DIAMOND_CONTRACT_ID,
        "the contract identity closes the DURABLE section",
    );
    assert_eq!(compiled.image.image_id.to_hex(), DIAMOND_IMAGE_ID);
}

/// The exact DURABLE section bytes of [`small_diamond`] at base `98b1665d`.
const DIAMOND_DURABLE_SECTION: &str = concat!(
    "0001000000000000000000000000000000010003000101000000000000000000000000000000050000000000",
    "0000000000000000000000000400000000000000000000000000000002000200000000000000000000000000",
    "0000000301010002000100030000000000000000000000000000000006010100020100020001000301000200",
    "01000300000001020000000000000000000000000000000001030000000000000000000000000000000400ff",
    "c27d314dbabc963ab14259d0a2da35e46c8e7537840a97a971cb2683a8094b",
);

/// The exact durable-contract identity of [`small_diamond`] at base `98b1665d`.
const DIAMOND_CONTRACT_ID: &str =
    "ffc27d314dbabc963ab14259d0a2da35e46c8e7537840a97a971cb2683a8094b";

/// The exact whole-image identity of [`small_diamond`] at base `98b1665d`.
const DIAMOND_IMAGE_ID: &str = "58eaf4f478da12063519113e058998453a1e5e414c2a2e48855826dd961b8b3d";
