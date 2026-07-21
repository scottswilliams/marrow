//! Compiler resource-limit totality (CRES01): every user-reachable construction
//! bound classifies before mutation as a truthful `check.resource_limit` source
//! diagnostic (one offending construct), a payload-free `CompileFailure::ResourceLimit`
//! (an aggregate exhaustion), or a private invariant (a producer contradiction).
//! These reds drive the production `compile()` path over over-bound projects and
//! assert the classified outcome, including the three defects the rescope named:
//! a finite acyclic over-deep durable value silently dropping its root, a root
//! key-column bound reported as `check.unsupported`, and an unprechecked branch
//! key tuple reaching the synthetic image-bound diagnostic.

use marrow_compile::{CompileFailure, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

fn project(source: &str, ids: Option<&[u8]>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, ids, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// Assert the failure is a source-diagnostic result carrying exactly one
/// `check.resource_limit` at a real, non-empty source file, and that no diagnostic
/// in the set carries an empty (fabricated) filename.
fn assert_source_resource_limit(result: Result<impl std::fmt::Debug, CompileFailure>) {
    match result {
        Ok(compiled) => panic!("expected a resource-limit diagnostic, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            assert!(
                diagnostics
                    .iter()
                    .all(|diagnostic| !diagnostic.file().as_str().is_empty()),
                "no resource diagnostic may carry a fabricated empty filename: {:#?}",
                diagnostics.as_slice(),
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "check.resource_limit"
                        && diagnostic.file().as_str() == "src/main.mw"),
                "expected a check.resource_limit at src/main.mw, got {:#?}",
                diagnostics.as_slice(),
            );
        }
        Err(other) => panic!("expected a source diagnostic, got {other:?}"),
    }
}

/// Assert the failure is the payload-free aggregate `ResourceLimit` arm.
fn assert_aggregate_resource_limit(result: Result<impl std::fmt::Debug, CompileFailure>) {
    match result {
        Ok(compiled) => panic!("expected an aggregate resource limit, compiled: {compiled:?}"),
        Err(CompileFailure::ResourceLimit(_)) => {}
        Err(other) => panic!("expected CompileFailure::ResourceLimit, got {other:?}"),
    }
}

/// A durable identity ledger built from an ordered anchor list, each `"kind path"`
/// receiving a distinct seeded id. No hand-alignment: the caller lists exactly the
/// anchors its shape declares.
fn ledger(anchors: &[String]) -> Vec<u8> {
    let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    for (seed, anchor) in anchors.iter().enumerate() {
        out.push_str(&format!("id {anchor} {:032x}\n", seed as u128 + 1));
    }
    out.push_str("high-water 0\nend\n");
    out.into_bytes()
}

// ---- Defect 1: a finite acyclic over-deep durable value silently drops its root.

/// A durable field whose stored value nests structs past `MAX_DURABLE_VALUE_DEPTH`
/// (32) is finite and acyclic, so the value-cycle pass never fires. Today the
/// builder marks the graph incomplete and drops the root with no diagnostic, so the
/// program compiles with the durable graph silently absent. It must instead report a
/// `check.resource_limit` at the offending field.
#[test]
fn over_deep_durable_value_reports_resource_limit_not_a_silent_drop() {
    let mut source = String::from("module main\n\n");
    for level in 0..40 {
        source.push_str(&format!("struct S{level} {{ s: S{} }}\n", level + 1));
    }
    source.push_str("struct S40 { x: int }\n\n");
    source.push_str("resource Deep {\n    required d: S0\n}\n\n");
    source.push_str("store ^deep[id: int]: Deep\n\n");
    source.push_str("pub fn noop(): int {\n    return 0\n}\n");
    let ids = ledger(&[
        "application .".into(),
        "product Deep".into(),
        "field Deep.d".into(),
        "root deep".into(),
        "key deep.id".into(),
    ]);
    assert_source_resource_limit(compile(&project(&source, Some(&ids))));
}

// ---- Long-cycle double-report law (QP01): the depth bound and the value-cycle
// pass are distinct owners in separate compile stages. The over-deep depth report
// is emitted by the durable value-shape builder (before the value graph exists);
// the cycle report is emitted later by the independent `reject_value_cycles` graph
// pass. A value-containment cycle whose distinct prefix crosses
// `MAX_DURABLE_VALUE_DEPTH` therefore truthfully draws BOTH, and that pair is the
// pinned law — not a redundancy to suppress. Suppressing the depth report for such
// a cycle would require the durable-identity stage to consult the later type-cycle
// graph (a cross-stage coupling and a second cycle-membership owner), and the only
// stage-local signal — a global "any cycle exists" flag — would wrongly silence the
// finite acyclic over-deep case whenever an unrelated cycle sat elsewhere in the
// program. The three sibling cases below fix the law in place.

/// The stable diagnostic codes of a failed compile, in report order. Panics if the
/// project compiled or failed at the aggregate/invariant arm. Also asserts no
/// diagnostic carries a fabricated empty filename, so every pinned report lands at a
/// real source span.
fn diagnostic_codes(result: Result<impl std::fmt::Debug, CompileFailure>) -> Vec<&'static str> {
    match result {
        Ok(compiled) => panic!("expected diagnostics, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            assert!(
                diagnostics
                    .iter()
                    .all(|diagnostic| !diagnostic.file().as_str().is_empty()),
                "no diagnostic may carry a fabricated empty filename: {:#?}",
                diagnostics.as_slice(),
            );
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect()
        }
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

/// A durable-reachable value-containment cycle through `struct_count` distinct
/// structs (`S0 -> S1 -> ... -> S{n-1} -> S0`). At `struct_count > 33` the distinct
/// prefix crosses `MAX_DURABLE_VALUE_DEPTH` (32) before the cycle closes; at a small
/// count the cycle closes within the bound.
fn cyclic_struct_chain(struct_count: usize) -> ProjectInput {
    let mut source = String::from("module main\n\n");
    for level in 0..struct_count {
        let next = (level + 1) % struct_count;
        source.push_str(&format!("struct S{level} {{ s: S{next} }}\n"));
    }
    source.push_str("\nresource R {\n    required d: S0\n}\n\n");
    source.push_str("store ^r[id: int]: R\n\n");
    source.push_str("pub fn noop(): int {\n    return 0\n}\n");
    let ids = ledger(&[
        "application .".into(),
        "product R".into(),
        "field R.d".into(),
        "root r".into(),
        "key r.id".into(),
    ]);
    project(&source, Some(&ids))
}

/// A cycle whose distinct prefix crosses `MAX_DURABLE_VALUE_DEPTH` draws BOTH the
/// depth `check.resource_limit` (exactly once, at the durable declaration) and the
/// value-cycle `check.recursion` (once per struct on the cycle). The pair is the
/// pinned law: dropping either report would fail this test.
#[test]
fn long_value_cycle_reports_both_resource_limit_and_recursion() {
    let struct_count = 34;
    let codes = diagnostic_codes(compile(&cyclic_struct_chain(struct_count)));
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "check.resource_limit")
            .count(),
        1,
        "a cycle crossing the depth bound draws exactly one depth report: {codes:?}",
    );
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "check.recursion")
            .count(),
        struct_count,
        "the value-cycle pass reports every struct on the cycle: {codes:?}",
    );
}

/// A cycle whose repeat falls within `MAX_DURABLE_VALUE_DEPTH` is pre-empted at the
/// value-shape builder's on-path check before any depth report, so only the
/// value-cycle pass fires.
#[test]
fn short_value_cycle_reports_only_recursion() {
    let codes = diagnostic_codes(compile(&cyclic_struct_chain(2)));
    assert!(
        codes.contains(&"check.recursion"),
        "a cycle within the depth bound reports the value-cycle pass: {codes:?}",
    );
    assert!(
        !codes.contains(&"check.resource_limit"),
        "a cycle within the depth bound draws no depth report: {codes:?}",
    );
}

/// A finite acyclic value that reaches the depth bound draws only the depth
/// `check.resource_limit`; the value-cycle pass never fires, so no `check.recursion`
/// accompanies it. This is the sibling that a global "any cycle exists" suppression
/// signal would wrongly silence, and the reason the depth report stays stage-local.
#[test]
fn acyclic_over_deep_value_reports_only_resource_limit() {
    let mut source = String::from("module main\n\n");
    for level in 0..40 {
        source.push_str(&format!("struct S{level} {{ s: S{} }}\n", level + 1));
    }
    source.push_str("struct S40 { x: int }\n\n");
    source.push_str("resource Deep {\n    required d: S0\n}\n\n");
    source.push_str("store ^deep[id: int]: Deep\n\n");
    source.push_str("pub fn noop(): int {\n    return 0\n}\n");
    let ids = ledger(&[
        "application .".into(),
        "product Deep".into(),
        "field Deep.d".into(),
        "root deep".into(),
        "key deep.id".into(),
    ]);
    let codes = diagnostic_codes(compile(&project(&source, Some(&ids))));
    assert!(
        codes.contains(&"check.resource_limit"),
        "an acyclic over-deep value reports the depth bound: {codes:?}",
    );
    assert!(
        !codes.contains(&"check.recursion"),
        "an acyclic over-deep value draws no value-cycle report: {codes:?}",
    );
}

// ---- Defect 2: a root key tuple over the bound must not be `check.unsupported`.

/// A store root with more than `MAX_KEY_COLUMNS` (8) key columns is prechecked
/// today, but under the displaced `check.unsupported` code. The migration reports it
/// as `check.resource_limit` at the store root.
#[test]
fn over_wide_root_key_reports_resource_limit_not_unsupported() {
    let cols: Vec<String> = (0..9).map(|i| format!("k{i}: int")).collect();
    let source = format!(
        "module main\n\nresource R {{\n    required v: int\n}}\n\nstore ^r[{}]: R\n\npub fn noop(): int {{\n    return 0\n}}\n",
        cols.join(", ")
    );
    let mut anchors = vec![
        "application .".into(),
        "product R".into(),
        "field R.v".into(),
        "root r".into(),
    ];
    for i in 0..9 {
        anchors.push(format!("key r.k{i}"));
    }
    assert_source_resource_limit(compile(&project(&source, Some(&ledger(&anchors)))));
}

// ---- Defect 3: an unprechecked branch key tuple reaches the synthetic diagnostic.

/// A keyed `branch` with more than `MAX_KEY_COLUMNS` (8) key columns is caught only
/// at encode today, producing the synthetic empty-filename image-bound diagnostic.
/// It must be prechecked at the branch, reporting `check.resource_limit` at a real
/// span.
#[test]
fn over_wide_branch_key_reports_resource_limit() {
    let cols: Vec<String> = (0..9).map(|i| format!("k{i}: int")).collect();
    let source = format!(
        "module main\n\nresource R {{\n    required title: string\n\n    b[{}] {{\n        required v: int\n    }}\n}}\n\nstore ^r[id: int]: R\n\npub fn noop(): int {{\n    return 0\n}}\n",
        cols.join(", ")
    );
    let mut anchors = vec![
        "application .".into(),
        "product R".into(),
        "field R.title".into(),
        "root r".into(),
        "key r.id".into(),
        "root R.b".into(),
    ];
    for i in 0..9 {
        anchors.push(format!("key R.b.k{i}"));
    }
    anchors.push("field R.b.v".into());
    assert_source_resource_limit(compile(&project(&source, Some(&ledger(&anchors)))));
}

// ---- Named source-precheck: an index projection past its component bound.

/// A `unique` managed index projecting more than `MAX_INDEX_COMPONENTS` (72) leaves
/// crosses the projection bound. It must report `check.resource_limit` at the index.
#[test]
fn over_wide_index_projection_reports_resource_limit() {
    let field_count = 73;
    let mut source = String::from("module main\n\nresource R {\n");
    for i in 0..field_count {
        source.push_str(&format!("    f{i}: int\n"));
    }
    source.push_str("}\n\n");
    let projection: Vec<String> = (0..field_count).map(|i| format!("f{i}")).collect();
    source.push_str(&format!(
        "store ^r[id: int]: R {{\n    index wide[{}] unique\n}}\n\npub fn noop(): int {{\n    return 0\n}}\n",
        projection.join(", ")
    ));
    let mut anchors = vec!["application .".into(), "product R".into()];
    for i in 0..field_count {
        anchors.push(format!("field R.f{i}"));
    }
    anchors.push("root r".into());
    anchors.push("key r.id".into());
    anchors.push("index r.wide".into());
    assert_source_resource_limit(compile(&project(&source, Some(&ledger(&anchors)))));
}

// ---- Named source-precheck: an overlong interned source string.

/// A string literal longer than `MAX_STRING_BYTES` (4 KiB) is a single source
/// construct crossing the interned-string bound, so it reports `check.resource_limit`
/// at that literal rather than the synthetic image-bound diagnostic.
#[test]
fn over_long_string_literal_reports_resource_limit() {
    let literal = "a".repeat(5000);
    let source =
        format!("module main\n\npub fn label(): string {{\n    return \"{literal}\"\n}}\n");
    assert_source_resource_limit(compile(&project(&source, None)));
}

// ---- Per-declaration source-precheck: enum variant count.

/// An enum declaring more than `MAX_VARIANTS` (256) members crosses the per-enum
/// variant bound at its declaration.
#[test]
fn over_wide_enum_reports_resource_limit() {
    let variants: Vec<String> = (0..257).map(|i| format!("    V{i}")).collect();
    let source = format!(
        "module main\n\nenum E {{\n{}\n}}\n\npub fn noop(): int {{\n    return 0\n}}\n",
        variants.join("\n")
    );
    assert_source_resource_limit(compile(&project(&source, None)));
}

// ---- Per-declaration source-precheck: variant payload width.

/// An enum variant carrying more than `MAX_PAYLOAD_FIELDS` (64) payload leaves
/// crosses the per-variant payload bound.
#[test]
fn over_wide_variant_payload_reports_resource_limit() {
    let payload: Vec<String> = (0..65).map(|i| format!("a{i}: int")).collect();
    let source = format!(
        "module main\n\nenum E {{\n    Small\n    Big({})\n}}\n\npub fn noop(): int {{\n    return 0\n}}\n",
        payload.join(", "),
    );
    assert_source_resource_limit(compile(&project(&source, None)));
}

// ---- Per-declaration source-precheck: record field width and function arity.

/// A record type (here a storeless `resource`) declaring more than
/// `MAX_RECORD_FIELDS` (4096) top-level fields crosses the per-record width at its
/// declaration.
#[test]
fn over_wide_record_reports_resource_limit() {
    let mut source = String::from("module main\n\nresource Wide {\n");
    for i in 0..4097 {
        source.push_str(&format!("    f{i}: int\n"));
    }
    source.push_str("}\n\npub fn noop(): int {\n    return 0\n}\n");
    assert_source_resource_limit(compile(&project(&source, None)));
}

/// A function declaring more than `MAX_PARAMS` (16) parameters crosses the per-frame
/// arity bound at its declaration.
#[test]
fn over_wide_function_arity_reports_resource_limit() {
    let params: Vec<String> = (0..17).map(|i| format!("p{i}: int")).collect();
    let source = format!(
        "module main\n\npub fn many({}): int {{\n    return 0\n}}\n",
        params.join(", ")
    );
    assert_source_resource_limit(compile(&project(&source, None)));
}

// ---- Aggregate: whole-program function and export counts.

/// More than `MAX_FUNCTIONS` (4096) functions is an aggregate count with no single
/// offending declaration, so it is the payload-free `ResourceLimit` arm.
#[test]
fn too_many_functions_is_an_aggregate_resource_limit() {
    let mut source = String::from("module main\n\n");
    for i in 0..4097 {
        source.push_str(&format!("fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    source.push_str("pub fn main(): int {\n    return 0\n}\n");
    assert_aggregate_resource_limit(compile(&project(&source, None)));
}

/// More than `MAX_EXPORTS` (32) public functions is an aggregate export count.
#[test]
fn too_many_exports_is_an_aggregate_resource_limit() {
    let mut source = String::from("module main\n\n");
    for i in 0..33 {
        source.push_str(&format!("pub fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    assert_aggregate_resource_limit(compile(&project(&source, None)));
}

/// A program whose emitted image exceeds the whole-image byte ceiling
/// (`MAX_IMAGE_BYTES`, 512 KiB) is an aggregate exhaustion with no single offender.
/// The string pool is the bulk here: a wide durable resource is bounded first by the
/// durable identity ledger (~4091 fields, ~343 KB) well under the ceiling, so the
/// ceiling is driven instead by many distinct near-maximal string literals — each a
/// live return value, so none is dead-stripped.
#[test]
fn image_too_large_is_an_aggregate_resource_limit() {
    // 150 distinct ~4000-byte strings ≈ 600 KB of string pool, past the 512 KiB image
    // ceiling while staying under MAX_STRINGS and MAX_STRING_BYTES.
    let mut source = String::from("module main\n\n");
    for i in 0..150 {
        let literal = format!("{i:04}{}", "a".repeat(3996));
        source.push_str(&format!(
            "fn f{i}(): string {{\n    return \"{literal}\"\n}}\n\n"
        ));
    }
    source.push_str("pub fn main(): int {\n    return 0\n}\n");
    assert_aggregate_resource_limit(compile(&project(&source, None)));
}
