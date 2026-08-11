//! Compiler resource-limit totality (CRES01): every user-reachable construction
//! bound classifies before mutation as a truthful `check.resource_limit` source
//! diagnostic (one offending construct), a payload-free `CompileFailure::ResourceLimit`
//! (an aggregate exhaustion), or a private invariant (a producer contradiction).
//! These reds drive the production `compile()` path over over-bound projects and
//! assert the classified outcome, including the three defects the rescope named:
//! a finite acyclic over-deep durable value silently dropping its root, a root
//! key-column bound reported as `check.unsupported`, and an unprechecked branch
//! key tuple reaching the synthetic image-bound diagnostic.

use std::panic::{AssertUnwindSafe, catch_unwind};

use marrow_compile::{CompileFailure, ResourceLimitKind, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};
use marrow_syntax::SourceSpan;

#[path = "common/ids.rs"]
mod ids;

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
                    .any(|diagnostic| diagnostic.code() == "check.resource_limit"
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

/// A durable identity ledger over an ordered anchor list. The caller lists exactly
/// the anchors its shape declares; the format is written in one place.
fn ledger(anchors: &[String]) -> Vec<u8> {
    let borrowed: Vec<&str> = anchors.iter().map(String::as_str).collect();
    ids::ledger(&borrowed)
}

// ---- Per-function source precheck: total local-slot allocation.

/// One unit function containing `binding_count` explicit bindings. Reusing the
/// spelling is intentional: shadowing still consumes a fresh monotone frame slot,
/// and the short line keeps the 65,537-binding totality case below the 1 MiB source
/// capture bound. The returned span is the 257th initializer when present.
/// A function of `binding_count` `const` bindings, each line prefixed by `indent`.
///
/// The prefix is a parameter because the widest fixture here has to fit one admitted
/// file: 65,537 bindings at the canonical four-space indent do not, and what a file may
/// be is a consequence of the heap ceiling rather than a round number to spend freely.
fn local_binding_program(binding_count: usize, indent: &str) -> (String, Option<SourceSpan>) {
    let mut source = String::from("module main\n\npub fn locals() {\n");
    let mut first_rejected = None;
    for index in 0..binding_count {
        source.push_str(indent);
        source.push_str("const x=");
        let start_byte = source.len();
        if index == marrow_image::bounds::MAX_LOCALS {
            first_rejected = Some(SourceSpan {
                start_byte,
                end_byte: start_byte + 1,
                line: source.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1,
                column: indent.len() as u32 + 9,
            });
        }
        source.push_str("0\n");
    }
    source.push_str("}\n");
    (source, first_rejected)
}

/// A function whose sixteen parameters and `binding_count` explicit bindings share
/// the same frame allocator. An optional empty range loop asks for the next
/// compiler-generated counter slot; the returned span covers that whole loop.
fn parameter_local_program(binding_count: usize, with_loop: bool) -> (String, Option<SourceSpan>) {
    let params: Vec<String> = (0..16).map(|index| format!("p{index}: int")).collect();
    let mut source = format!("module main\n\npub fn locals({}) {{\n", params.join(", "));
    for _ in 0..binding_count {
        source.push_str("    const x = 0\n");
    }
    let loop_span = with_loop.then(|| {
        source.push_str("    ");
        let start_byte = source.len();
        let text = "for i in 0..1 {}";
        let span = SourceSpan {
            start_byte,
            end_byte: start_byte + text.len(),
            line: source.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1,
            column: 5,
        };
        source.push_str(text);
        source.push('\n');
        span
    });
    source.push_str("}\n");
    (source, loop_span)
}

fn assert_exact_local_limit(
    result: Result<marrow_compile::Compiled, CompileFailure>,
    span: SourceSpan,
) {
    let diagnostics = match result {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics,
        Ok(compiled) => panic!("expected the local limit, compiled: {compiled:?}"),
        Err(other) => panic!("expected one source-located local limit, got {other:?}"),
    };
    let diagnostics = diagnostics.as_slice();
    assert_eq!(
        diagnostics.len(),
        1,
        "the first rejected slot request emits exactly one diagnostic: {diagnostics:#?}",
    );
    let diagnostic: &SourceDiagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code(), "check.resource_limit");
    assert_eq!(diagnostic.file().as_str(), "src/main.mw");
    assert_eq!(diagnostic.span(), span);
}

/// The complete frame is accepted and its image bytes stay frozen, so adding the
/// rejection path cannot perturb an already-admitted function.
#[test]
fn exactly_256_explicit_bindings_compile_with_stable_image_bytes() {
    let (source, rejected) = local_binding_program(marrow_image::bounds::MAX_LOCALS, "    ");
    assert!(rejected.is_none());
    let compiled = compile(&project(&source, None))
        .unwrap_or_else(|failure| panic!("the complete 256-slot frame must compile: {failure:?}"));
    let hex: String = marrow_image::image_id(&compiled.image.bytes)
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        hex, "5237454e742ecf77261a087622408dfa22d48d30e6388319ed53324fea8022bf",
        "the accepted 256-slot frame's encoded image changed"
    );
}

/// Request 257 is refused at its initializer before the image builder sees an
/// over-wide frame; the former locationless aggregate `Locals` outcome is forbidden.
#[test]
fn the_257th_explicit_binding_is_one_source_limited_diagnostic() {
    let (source, rejected) = local_binding_program(marrow_image::bounds::MAX_LOCALS + 1, "    ");
    assert_exact_local_limit(
        compile(&project(&source, None)),
        rejected.expect("the 257th initializer span"),
    );
}

/// Parameters, source locals, and generated loop temporaries consume one allocator:
/// 16 + 240 is accepted, then the range loop's counter request is refused at the
/// loop's exact real span.
#[test]
fn parameters_locals_and_generated_temporaries_share_the_bound() {
    let (accepted, no_loop) = parameter_local_program(240, false);
    assert!(no_loop.is_none());
    compile(&project(&accepted, None)).unwrap_or_else(|failure| {
        panic!("16 parameters plus 240 locals fill the frame exactly: {failure:?}")
    });

    let (rejected, loop_span) = parameter_local_program(240, true);
    assert_exact_local_limit(
        compile(&project(&rejected, None)),
        loop_span.expect("generated-counter request span"),
    );
}

/// A source inside the admitted length can ask for enough locals to overflow the backing
/// `u16`. Compilation must stop at request 257 with the typed source refusal, never
/// unwind, wrap, or continue through the remaining 65,280 requests.
#[test]
fn sixty_five_thousand_local_requests_are_total_and_fail_stop() {
    let (source, rejected) = local_binding_program(65_537, "");
    assert!(
        source.len() <= marrow_compile::MAX_PARSED_FILE_BYTES,
        "the fixture has to be a file the drive admits, or it never reaches the local \
         allocator: {} bytes against an admitted {}",
        source.len(),
        marrow_compile::MAX_PARSED_FILE_BYTES
    );
    let outcome = catch_unwind(AssertUnwindSafe(|| compile(&project(&source, None))));
    let result = outcome.expect("local allocation must not panic or overflow");
    assert_exact_local_limit(
        result,
        rejected.expect("the first rejected initializer span"),
    );
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
                .map(|diagnostic| diagnostic.code())
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

// ---- The leaf level counts: compiler, image, and verifier agree on value depth.

/// A durable field whose value is `struct_count` nested structs terminating in
/// `leaf`. The outermost struct sits at depth 1 and the terminating leaf at depth
/// `struct_count + 1`, so `struct_count = 31` places the leaf exactly at
/// `MAX_DURABLE_VALUE_DEPTH` (32) and `struct_count = 32` one level past it.
fn struct_chain_to_leaf(struct_count: usize, leaf: &str) -> ProjectInput {
    let mut source = String::from("module main\n\n");
    source.push_str(&format!("struct S0 {{ x: {leaf} }}\n"));
    for level in 1..struct_count {
        source.push_str(&format!("struct S{level} {{ s: S{} }}\n", level - 1));
    }
    source.push_str(&format!(
        "\nresource Deep {{\n    required d: S{}\n}}\n\n",
        struct_count - 1
    ));
    source.push_str("store ^deep[id: int]: Deep\n\n");
    source.push_str("pub fn noop(): int {\n    return 0\n}\n");
    let ids = ledger(&[
        "application .".into(),
        "product Deep".into(),
        "field Deep.d".into(),
        "root deep".into(),
        "key deep.id".into(),
    ]);
    project(&source, Some(&ids))
}

/// The terminating scalar occupies a level of its own, so a value whose enclosing
/// structs all fit the bound can still be over-deep at its leaf. The compiler
/// checked the depth only where it descended — at a struct or an enum — while the
/// image encoder measures the whole shape including its leaf, so the two disagreed:
/// a leaf one level past the bound left the source diagnostic unreported and the
/// program failing at the image invariant instead. Both leaf kinds are checked.
#[test]
fn a_scalar_leaf_one_level_past_the_bound_reports_the_source_limit() {
    assert_source_resource_limit(compile(&struct_chain_to_leaf(32, "int")));
}

/// An enum payload leaf is measured the same way: the enum itself sits one level
/// above its payload values, so a payload one level past the bound is over-deep even
/// though the enum that carries it fits.
fn enum_payload_chain(struct_count: usize) -> ProjectInput {
    let mut source = String::from("module main\n\n");
    source.push_str("enum Leaf {\n    none\n    some(v: int)\n}\n\n");
    source.push_str("struct S0 { x: Leaf }\n");
    for level in 1..struct_count {
        source.push_str(&format!("struct S{level} {{ s: S{} }}\n", level - 1));
    }
    source.push_str(&format!(
        "\nresource Deep {{\n    required d: S{}\n}}\n\n",
        struct_count - 1
    ));
    source.push_str("store ^deep[id: int]: Deep\n\n");
    source.push_str("pub fn noop(): int {\n    return 0\n}\n");
    let ids = ledger(&[
        "application .".into(),
        "product Deep".into(),
        "field Deep.d".into(),
        "root deep".into(),
        "key deep.id".into(),
        "sum Leaf".into(),
        "member Leaf.none".into(),
        "member Leaf.some".into(),
    ]);
    project(&source, Some(&ids))
}

#[test]
fn an_enum_payload_leaf_one_level_past_the_bound_reports_the_source_limit() {
    assert_source_resource_limit(compile(&enum_payload_chain(31)));
}

#[test]
fn an_enum_payload_leaf_at_the_bound_still_compiles() {
    compile(&enum_payload_chain(30)).expect("an enum payload leaf at the depth bound is admitted");
}

/// The bound itself still admits: a leaf at exactly `MAX_DURABLE_VALUE_DEPTH`
/// compiles, so the leaf check refuses one level and not the level below it.
#[test]
fn a_scalar_leaf_at_the_bound_still_compiles() {
    compile(&struct_chain_to_leaf(31, "int")).expect("a leaf at the depth bound is admitted");
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

/// More than `MAX_EXPORTS` (256) public functions is an aggregate export count.
#[test]
fn too_many_exports_is_an_aggregate_resource_limit() {
    let mut source = String::from("module main\n\n");
    for i in 0..257 {
        source.push_str(&format!("pub fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    assert_aggregate_resource_limit(compile(&project(&source, None)));
}

/// Exactly `MAX_EXPORTS` (256) public functions compiles: the M2 widen raised the export
/// bound from the T01 waypoint of 32 to admit a production application's multi-module
/// public surface (a measured 43-export ensemble) with headroom. Before the widen a
/// 33-export program refused as an aggregate resource limit.
#[test]
fn exports_within_the_widened_bound_compile() {
    let mut source = String::from("module main\n\n");
    for i in 0..256 {
        source.push_str(&format!("pub fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    compile(&project(&source, None)).unwrap_or_else(|failure| {
        panic!("256 exports must compile after the M2 widen: {failure:?}")
    });
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

/// Every fixed bound projects two ways from one typed kind: `detail` is the frozen
/// machine identifier a tool bisects on, `description` is the sentence fragment a
/// person reads. Rendering the identifier into terminal prose is the defect this
/// pins — no CLI surface may put a Rust variant name in front of a reader, such as
/// a silent `Functions`-shaped word on stderr. The match below is exhaustive over
/// the kind, so adding a bound is a compile error until its arm is written here.
#[test]
fn every_resource_limit_kind_describes_itself_without_its_variant_name() {
    use marrow_compile::ResourceLimitKind as Kind;

    const EVERY_KIND: [Kind; 22] = [
        Kind::Strings,
        Kind::Consts,
        Kind::Types,
        Kind::Enums,
        Kind::Collections,
        Kind::Roots,
        Kind::DurableMembers,
        Kind::Sites,
        Kind::Functions,
        Kind::Exports,
        Kind::TestEntries,
        Kind::ImageBytes,
        Kind::StringBytes,
        Kind::CodeBytes,
        Kind::IndexComponents,
        Kind::DurableDepth,
        Kind::DiagnosticCount,
        Kind::DiagnosticBytes,
        Kind::ProjectFiles,
        Kind::ProjectFileBytes,
        Kind::ProjectSourceBytes,
        Kind::DeclarationLedgerBytes,
    ];

    for kind in EVERY_KIND {
        // Exhaustiveness anchor: adding a bound makes this match non-exhaustive, so
        // a new kind cannot land without an arm here. The arm is where a maintainer
        // meets this test, not a proof that the list is complete — Rust cannot
        // enumerate variants, so `EVERY_KIND` is extended by the same hand.
        match kind {
            Kind::Strings
            | Kind::Consts
            | Kind::Types
            | Kind::Enums
            | Kind::Collections
            | Kind::Roots
            | Kind::DurableMembers
            | Kind::Sites
            | Kind::Functions
            | Kind::Exports
            | Kind::TestEntries
            | Kind::ImageBytes
            | Kind::StringBytes
            | Kind::CodeBytes
            | Kind::IndexComponents
            | Kind::DurableDepth
            | Kind::DiagnosticCount
            | Kind::DiagnosticBytes
            | Kind::DeclarationLedgerBytes
            | Kind::ProjectFiles
            | Kind::ProjectFileBytes
            | Kind::ProjectSourceBytes => {}
        }

        let identifier = kind.detail();
        let description = kind.description();
        assert!(
            description.contains(' '),
            "`{identifier}` describes itself as `{description}`, which is an identifier, \
             not prose"
        );
        assert!(
            description
                .chars()
                .next()
                .is_some_and(|first| first.is_lowercase()),
            "`{description}` must read as a sentence fragment, lowercase and unpunctuated"
        );
        assert!(
            !description.contains(identifier),
            "`{description}` still carries the Rust variant name `{identifier}`"
        );
    }

    let identifiers: Vec<&str> = EVERY_KIND.iter().map(|kind| kind.detail()).collect();
    let descriptions: Vec<&str> = EVERY_KIND.iter().map(|kind| kind.description()).collect();
    for projection in [&identifiers, &descriptions] {
        let mut sorted = projection.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            EVERY_KIND.len(),
            "each bound must be distinguishable on both surfaces"
        );
    }
}

// ---- Red R24: the operation-site policy pair, at the cap and one demand past it.

/// The declared width of the site-policy corpus's one shared Product. The last field is
/// deliberately left untouched by the corpus below, so it is the single unused demand the
/// paired program adds.
const POLICY_FIELDS: usize = 64;

/// The recomputed per-site byte floors of the pinned v0 layout, and the conclusion they
/// force. A root whole-payload site encodes a 2-step path (`1 + 2 * 17 + 1 = 36` bytes)
/// beside a root occurrence row costing at least its name, key count, entry record,
/// placement, product, member count and index count (>= 42 bytes). Every other site
/// encodes at least a 3-step path (`1 + 3 * 17 + 1 = 53` bytes) beside the distinct
/// member row it must address (a kind byte, a 16-byte ledger id, and its shape, so
/// 19 bytes or more). A full site table therefore implies at least
/// `8_192 * 72 = 589_824` bytes, above the 524,288-byte ceiling: **no image with a full
/// site table fits**.
///
/// These are compile-time assertions rather than a test body: they are arithmetic over
/// published constants, so a drift in either constant must fail the build, not a run.
/// The conclusion survives a floor a third of this one, so it does not rest on the exact
/// per-row terms.
const ROOT_SITE_FLOOR: usize = 36 + 42;
const OTHER_SITE_FLOOR: usize = 53 + 19;
const _: () = assert!(ROOT_SITE_FLOOR == 78);
const _: () = assert!(OTHER_SITE_FLOOR == 72);
const _: () = assert!(
    marrow_image::bounds::MAX_SITES * OTHER_SITE_FLOOR > marrow_image::bounds::MAX_IMAGE_BYTES
);

/// The corpus: `roots` keyed roots projecting **one shared Product** of
/// [`POLICY_FIELDS`] required `int` fields, each root writing the leading `touched` of
/// them. `extra_demand` adds one write of the last field, through the first root only.
///
/// The site plan holds one eager whole-payload site per root plus one lazily demanded
/// field leaf per distinct `(root, written field)` pair, so the corpus demands
/// `roots * (1 + touched)` sites, plus one more when `extra_demand` is set. A declared
/// but unwritten field mints nothing.
fn site_policy_source(roots: usize, touched: usize, extra_demand: bool) -> String {
    let mut source = String::from("module main\n\nresource R {\n");
    for field in 0..POLICY_FIELDS {
        source.push_str(&format!("    required f{field}: int\n"));
    }
    source.push_str("}\n\n");
    for root in 0..roots {
        source.push_str(&format!("store ^r{root}[id: int]: R\n"));
    }
    source.push('\n');
    for root in 0..roots {
        source.push_str(&format!(
            "pub fn w{root}(id: int, v: int) {{\n    transaction {{\n"
        ));
        for field in 0..touched {
            source.push_str(&format!("        ^r{root}[id].f{field} = v\n"));
        }
        if extra_demand && root == 0 {
            source.push_str(&format!("        ^r0[id].f{} = v\n", POLICY_FIELDS - 1));
        }
        source.push_str("    }\n}\n\n");
    }
    source
}

/// The corpus ledger: the application, the one shared Product with its Product-scoped
/// field anchors, then each root occurrence with its own key column. Member anchoring is
/// Product-scoped, so many occurrences of one Product add no member row.
fn site_policy_ids(roots: usize) -> Vec<u8> {
    let mut anchors = vec!["application .".to_string(), "product R".to_string()];
    anchors.extend((0..POLICY_FIELDS).map(|field| format!("field R.f{field}")));
    for root in 0..roots {
        anchors.push(format!("root r{root}"));
        anchors.push(format!("key r{root}.id"));
    }
    ledger(&anchors)
}

fn site_policy_compile(
    roots: usize,
    touched: usize,
    extra_demand: bool,
) -> Result<marrow_compile::Compiled, CompileFailure> {
    let source = site_policy_source(roots, touched, extra_demand);
    assert!(
        source.len() <= marrow_compile::MAX_PARSED_FILE_BYTES,
        "the corpus has to be a file the drive admits: {} bytes against an admitted {}",
        source.len(),
        marrow_compile::MAX_PARSED_FILE_BYTES,
    );
    compile(&project(&source, Some(&site_policy_ids(roots))))
}

/// Compile one full-scale corpus and return the aggregate bound it exhausted. A corpus
/// that compiles, or that reports a source diagnostic, fails the test: both endpoints of
/// the pair must be refused by the image projection alone.
fn site_policy_limit(extra_demand: bool) -> ResourceLimitKind {
    // 128 roots x (1 root site + 63 written fields) = 8,192 demands; `extra_demand` is
    // the 8,193rd.
    match site_policy_compile(128, 63, extra_demand) {
        Ok(compiled) => panic!(
            "the site-policy corpus must not produce an image ({} bytes)",
            compiled.image.bytes.len(),
        ),
        Err(CompileFailure::ResourceLimit(limit)) => limit.kind(),
        Err(other) => panic!("expected an aggregate resource limit, got {other:?}"),
    }
}

/// The corpus shape is a real compiling program that mints sites through the production
/// pipeline: at four roots it is far inside every bound and produces an image. Without
/// this control the pair below could agree for a reason that has nothing to do with the
/// site plan.
#[test]
fn the_site_policy_corpus_shape_compiles_far_inside_the_cap() {
    let compiled = site_policy_compile(4, 63, false)
        .unwrap_or_else(|failure| panic!("the corpus shape must compile: {failure:?}"));
    assert!(
        !compiled.image.bytes.is_empty(),
        "the corpus shape lowers to a non-empty image",
    );
}

/// At exactly `MAX_SITES` the site plan is full but not crossed, so the Sites bound does
/// not fire and the program is carried to the late whole-image byte ceiling — which it
/// cannot clear.
///
/// **It must not claim the 8,192-site image fits**, and it cannot: [`OTHER_SITE_FLOOR`]
/// carries that as a compile-time assertion over the published bounds.
#[test]
fn the_site_plan_at_capacity_selects_the_late_image_byte_ceiling() {
    assert_eq!(
        site_policy_limit(false),
        ResourceLimitKind::ImageBytes,
        "a full-but-uncrossed site plan is refused by the byte ceiling, not by Sites",
    );
}

/// One demand past the cap the plan crosses, and the Sites bound fires in the encoder's
/// precheck — **before** the image is assembled and before the byte ceiling is reached.
/// The pair freezes the candidate precedence: crossing the site cap is reported as the
/// site cap, and only an uncrossed plan can reach the later byte verdict.
///
/// The pair also pins the corpus arithmetic without decoding an image that is never
/// produced. The two programs differ by exactly one distinct `(root, field)` demand — a
/// field no other statement writes — so their demand counts are `d` and `d + 1`. The
/// first is uncrossed (`d <= MAX_SITES`) and the second is crossed (`d + 1 > MAX_SITES`),
/// which forces `d = MAX_SITES` exactly. The corpus is therefore the 8,192/8,193 pair it
/// claims to be, by the outcomes themselves rather than by counting site emissions.
#[test]
fn one_demand_past_the_site_cap_selects_sites_before_the_image_byte_ceiling() {
    assert_eq!(
        site_policy_limit(true),
        ResourceLimitKind::Sites,
        "the crossed site plan is reported as Sites, not as the later byte ceiling",
    );
}
