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

// ---- Named source-precheck: a member tree nested past its depth bound.

/// A resource whose durable member tree nests `branch` placements `branches` deep,
/// with one stored field in the innermost body, and the identity ledger that shape
/// declares.
///
/// Branches are the only source path to member depth: a nested `group` is refused as
/// `check.unsupported` on this line, so a depth corpus built from groups would pass
/// for the wrong reason. The innermost field sits one level below the innermost
/// branch, so `branches` nested branches place a member at depth `branches + 1`.
///
/// Returns the project alongside the innermost field's one-based line: the refusal must
/// land on that member, not on the resource, the store, or nowhere. A member's span runs
/// from the start of its line, as every other member diagnostic's does, so the line is
/// the whole of what identifies it.
fn nested_branch_project(branches: usize, ids: bool) -> (ProjectInput, u32) {
    let mut source = String::from("module main\n\nresource R {\n    required t: string\n\n");
    let mut anchors = vec![
        "application .".into(),
        "product R".into(),
        "field R.t".into(),
    ];
    let mut path = String::from("R");
    for level in 1..=branches {
        let indent = "    ".repeat(level);
        source.push_str(&format!("{indent}b{level}[k{level}: int] {{\n"));
        path.push_str(&format!(".b{level}"));
        anchors.push(format!("root {path}"));
        anchors.push(format!("key {path}.k{level}"));
    }
    let indent = "    ".repeat(branches + 1);
    let line = source.lines().count() as u32 + 1;
    source.push_str(&format!("{indent}required v: int\n"));
    anchors.push(format!("field {path}.v"));
    for level in (1..=branches).rev() {
        source.push_str(&format!("{}}}\n", "    ".repeat(level)));
    }
    source.push_str("}\n\nstore ^r[id: int]: R\n\npub fn noop(): int {\n    return 0\n}\n");
    anchors.push("root r".into());
    anchors.push("key r.id".into());
    let ledger = ids.then(|| ledger(&anchors));
    (project(&source, ledger.as_deref()), line)
}

/// The one `check.resource_limit` in a diagnostic set, with its span.
fn only_resource_limit(result: Result<impl std::fmt::Debug, CompileFailure>) -> SourceDiagnostic {
    match result {
        Ok(compiled) => panic!("expected a resource-limit diagnostic, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            let mut limits = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code() == "check.resource_limit");
            let limit = limits
                .next()
                .unwrap_or_else(|| {
                    panic!(
                        "expected a check.resource_limit, got {:#?}",
                        diagnostics.as_slice()
                    )
                })
                .clone();
            assert!(
                limits.next().is_none(),
                "one over-deep body is one fact: {:#?}",
                diagnostics.as_slice()
            );
            limit
        }
        Err(other) => panic!("expected a source diagnostic, got {other:?}"),
    }
}

/// The deepest member tree the bound admits: `MAX_DURABLE_DEPTH - 1` nested branches
/// place their innermost field at exactly `MAX_DURABLE_DEPTH`.
#[test]
fn a_member_tree_at_the_depth_bound_still_compiles() {
    let at_bound = marrow_image::bounds::MAX_DURABLE_DEPTH - 1;
    let (input, _) = nested_branch_project(at_bound, true);
    let result = compile(&input);
    assert!(
        result.is_ok(),
        "a member at exactly the depth bound compiles, got {:?}",
        result.err()
    );
}

/// One member past `MAX_DURABLE_DEPTH` (16) is refused at the offending member: the
/// source precheck reports one `check.resource_limit` located at that member's own span,
/// naming its file and line. The bound is not left to the encoder's later recheck, whose
/// verdict can name no source position at all.
#[test]
fn one_member_past_the_depth_bound_reports_a_located_resource_limit() {
    let past_bound = marrow_image::bounds::MAX_DURABLE_DEPTH;
    let (input, line) = nested_branch_project(past_bound, true);
    let limit = only_resource_limit(compile(&input));
    assert_eq!(limit.file().as_str(), "src/main.mw");
    assert_eq!(
        limit.line(),
        line,
        "the refusal names the offending member, got {limit:#?}"
    );
}

/// The same corpus before its identity ledger is minted. A fresh project's located
/// `check.durable_identity` rows once masked this bound entirely — the depth refusal
/// arrived only through the encoder, which a project without ids never reaches — so a
/// corpus checked on a fresh project alone proves nothing about it. The precheck runs
/// in the same walk that resolves anchors, so both facts are reported together.
#[test]
fn the_depth_refusal_is_located_before_the_ledger_is_minted() {
    let past_bound = marrow_image::bounds::MAX_DURABLE_DEPTH;
    let (input, line) = nested_branch_project(past_bound, false);
    let limit = only_resource_limit(compile(&input));
    assert_eq!(limit.file().as_str(), "src/main.mw");
    assert_eq!(limit.line(), line);
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

    const EVERY_KIND: [Kind; 19] = [
        Kind::Strings,
        Kind::Consts,
        Kind::Types,
        Kind::Enums,
        Kind::Collections,
        Kind::Roots,
        Kind::Sites,
        Kind::Functions,
        Kind::Exports,
        Kind::TestEntries,
        Kind::ImageBytes,
        Kind::StringBytes,
        Kind::CodeBytes,
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
            | Kind::Sites
            | Kind::Functions
            | Kind::Exports
            | Kind::TestEntries
            | Kind::ImageBytes
            | Kind::StringBytes
            | Kind::CodeBytes
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

// ---- The identity-ledger equation: a per-Product member overrun is unreachable.

/// Every admitted Product member anchors one live identity-ledger row, and any admitted
/// Product carries at least an application row, a Product row, and one outer root
/// placement row beside its members. So a project whose ledger admits declares at most
/// `MAX_IDS_ROWS - 3` members in one Product — below the member bound the image rechecks,
/// which is why an over-count Product is a producer contradiction rather than a bound a
/// program can meet. Every other ledger term (key columns, enum member rows, managed
/// index rows, tombstones, unrelated live rows) only tightens it.
///
/// The two constants live in different crates and neither reads the other, so the
/// relation is asserted here, where both are in scope. `marrow-image` may not depend on
/// `marrow-project`.
const _: () = assert!(
    marrow_project::MAX_IDS_ROWS - 3 <= marrow_image::bounds::MAX_DURABLE_MEMBERS,
    "a ledger-admitted Product must be narrower than the image's durable member bound",
);

/// A resource of `top` stored fields holding one keyed branch of `branch` stored fields,
/// projected by `roots` store roots. The branch's fields reuse the top-level spellings:
/// their anchors differ by path, so the shape is as wide as it reads while the string
/// pool carries one copy of each name.
fn wide_product_source(top: usize, branch: usize, roots: usize) -> String {
    let mut source = String::from("module main\n\nresource R {\n");
    for index in 0..top {
        source.push_str(&format!("    f{index}: int\n"));
    }
    source.push_str("\n    b[k: int] {\n");
    for index in 0..branch {
        source.push_str(&format!("        f{index}: int\n"));
    }
    source.push_str("    }\n}\n\n");
    for root in 0..roots {
        source.push_str(&format!("store ^r{root}[id: int]: R\n"));
    }
    source.push_str("\npub fn noop(): int {\n    return 0\n}\n");
    source
}

/// Exactly the anchors [`wide_product_source`] declares, in walk order.
fn wide_product_anchors(top: usize, branch: usize, roots: usize) -> Vec<String> {
    let mut anchors = vec!["application .".to_string(), "product R".to_string()];
    for index in 0..top {
        anchors.push(format!("field R.f{index}"));
    }
    anchors.push("root R.b".to_string());
    anchors.push("key R.b.k".to_string());
    for index in 0..branch {
        anchors.push(format!("field R.b.f{index}"));
    }
    for root in 0..roots {
        anchors.push(format!("root r{root}"));
        anchors.push(format!("key r{root}.id"));
    }
    anchors
}

/// The widest one-Product shape the ledger admits: its rows land exactly on
/// `MAX_IDS_ROWS`, and the member count that shape declares stays under the image's
/// member bound with room to spare. It compiles, so the equation is not vacuous — the
/// binder is the ledger, and it binds first.
#[test]
fn the_widest_ledger_admitted_product_compiles_below_the_member_bound() {
    let (top, branch, roots) = (4089, 4095, 1);
    let anchors = wide_product_anchors(top, branch, roots);
    assert!(
        anchors.len() <= marrow_project::MAX_IDS_ROWS,
        "the shape is sized against the ledger's row cap, not a remembered number"
    );
    // Members: every stored field plus the branch placement itself. The application,
    // Product, outer root, and key rows are the equation's fixed overhead and are not
    // members.
    let members = top + 1 + branch;
    assert!(
        members <= marrow_image::bounds::MAX_DURABLE_MEMBERS,
        "the ledger admits {members} members; the image bound is {}",
        marrow_image::bounds::MAX_DURABLE_MEMBERS
    );
    let source = wide_product_source(top, branch, roots);
    let result = compile(&project(&source, Some(&ledger(&anchors))));
    assert!(
        result.is_ok(),
        "the widest ledger-admitted Product compiles: {:?}",
        result.err()
    );
}

/// A second root over the same Product adds two ledger rows and no members: the Product
/// graph is built once, at its first root, and the later root references it. Were the
/// graph copied per root this shape would carry twice the member bound.
#[test]
fn a_shared_product_is_counted_once_however_many_roots_project_it() {
    let (top, branch, roots) = (4089, 4095, 2);
    let anchors = wide_product_anchors(top, branch, roots);
    assert_eq!(
        anchors.len(),
        marrow_project::MAX_IDS_ROWS,
        "the second root spends the last two rows the ledger has"
    );
    let members = top + 1 + branch;
    assert!(
        members * roots > marrow_image::bounds::MAX_DURABLE_MEMBERS,
        "a per-root copy of this Product would cross the member bound"
    );
    let source = wide_product_source(top, branch, roots);
    let result = compile(&project(&source, Some(&ledger(&anchors))));
    assert!(
        result.is_ok(),
        "one Product graph serves every root that projects it: {:?}",
        result.err()
    );
}

/// One row past the ledger's cap. The project never reaches compilation: identity
/// admission refuses the ledger itself, so the member bound is not what a wider shape
/// meets.
#[test]
fn one_ledger_row_past_the_cap_is_refused_before_compilation() {
    let (top, branch, roots) = (4090, 4095, 2);
    let anchors = wide_product_anchors(top, branch, roots);
    assert!(anchors.len() > marrow_project::MAX_IDS_ROWS);
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        wide_product_source(top, branch, roots).into_bytes(),
    )];
    let captured = marrow_project::capture(
        &manifest,
        files,
        Some(&ledger(&anchors)),
        &CaptureLimits::DEFAULT,
    );
    assert!(
        captured.is_err(),
        "a ledger past its row cap is refused at admission"
    );
}

// ---- The compiler-side maximum-live equation for the durable contract graph.

/// The identity-ledger anchors one project may declare beside its application and its
/// Product. Every member row, root occurrence, key column, enum member, and managed-index
/// declaration the compiler admits anchors one of them, so this is the census that bounds
/// how much durable graph a compiler drive can be holding at once.
const LEDGER_ADMITTED_ANCHORS: u64 = (marrow_project::MAX_IDS_ROWS - 2) as u64;

/// The compiler-side maximum-live durable contract graph, in bytes.
///
/// The equation is `marrow-image`'s — it owns what its representation costs — and the
/// extrema are the compiler's, because the identity ledger is what bounds a source-admitted
/// graph. Each population is charged at the *whole* remaining anchor budget: no ledger can
/// satisfy every term at once (they share one budget of
/// `marrow_project::MAX_IDS_ROWS` rows), so this envelope sits deliberately above the true
/// simultaneous maximum. It is stated that way because the alternative — solving for the
/// costliest admissible split — would have to be re-solved whenever any per-element charge
/// moved, and an envelope that is provably above every split is the stronger claim.
///
/// The value arena is charged separately by
/// [`MAX_LIVE_DURABLE_VALUE_ARENA_BYTES`]: its populations are bounded by the *type*
/// population, not by the identity ledger, and its ceiling belongs to the durable-value
/// owner rather than to the durable-graph owner.
const MAX_LIVE_DURABLE_GRAPH_BYTES: u64 =
    marrow_image::bounds::max_live_durable_graph_bytes(marrow_image::bounds::DurableGraphCounts {
        products: LEDGER_ADMITTED_ANCHORS,
        occurrences: LEDGER_ADMITTED_ANCHORS,
        indexes: LEDGER_ADMITTED_ANCHORS,
        members: LEDGER_ADMITTED_ANCHORS,
        value_nodes: 0,
        value_references: 0,
    });

/// The declared ceiling for the durable contract graph a compiler drive holds live.
///
/// The `H_` prefix is the maximum-live accounting's *ceiling* term — the `H` of `M <= H` —
/// paired with the `MAX_LIVE_` accounted maximum above it, never an abbreviated
/// measurement. (`bound_reachability.rs` spells `H_SITES`/`H_BYTES` with the unrelated
/// sense *highest-fitting*; each site names which it means.)
///
/// Declared, not derived from the sum: a ceiling defined as whatever the current
/// representation costs proves nothing, because every widening would raise both sides
/// equally. 64 MiB is a generous fraction of the compiler's owned heap for one of its
/// several live structures, and it still fails a representation that charged per
/// (root x member) — a member tree per occurrence rather than one shared declaration — as
/// the negative control below shows.
const H_DURABLE_GRAPH_BYTES: u64 = 64 * 1024 * 1024;

/// Prove the compiler-side equation closes at compile time. A representation change that
/// breached the ceiling would fail the build here rather than at some later measurement.
const _: () = {
    assert!(
        MAX_LIVE_DURABLE_GRAPH_BYTES <= H_DURABLE_GRAPH_BYTES,
        "the compiler-side maximum-live durable contract graph exceeds its declared ceiling",
    );
    // And not trivially under: an accounting that had stopped charging its populations
    // would satisfy the ceiling for the wrong reason.
    assert!(
        MAX_LIVE_DURABLE_GRAPH_BYTES > H_DURABLE_GRAPH_BYTES / 4,
        "the compiler-side accounting no longer charges a meaningful fraction of its ceiling",
    );
};

/// The maximum-live value arena the same drive holds, in bytes — **an exported term, not a
/// bound this suite asserts.**
///
/// The arena's populations are bounded by the type population (`MAX_TYPES` distinct
/// composite shapes, `MAX_ENUMS` distinct enum shapes, each at its own admitted width), not
/// by the identity ledger, so this figure is dominated by enum payload leaves and is far
/// looser than anything the durable graph contributes. Tightening it is the durable-value
/// owner's work; it is derived and published here so that owner and the capacity join
/// consume a stated number rather than rediscovering it.
const MAX_LIVE_DURABLE_VALUE_ARENA_BYTES: u64 =
    marrow_image::bounds::max_live_durable_graph_bytes(marrow_image::bounds::DurableGraphCounts {
        products: 0,
        occurrences: 0,
        indexes: 0,
        members: 0,
        value_nodes: (marrow_image::bounds::MAX_TYPES + marrow_image::bounds::MAX_ENUMS) as u64,
        value_references: (marrow_image::bounds::MAX_TYPES
            * marrow_image::bounds::MAX_STRUCT_LEAVES
            + marrow_image::bounds::MAX_ENUMS
                * marrow_image::bounds::MAX_VARIANTS
                * (1 + marrow_image::bounds::MAX_PAYLOAD_FIELDS)) as u64,
    }) - marrow_image::bounds::DURABLE_GRAPH_FIXED_BYTES;

/// The exact accounted figures, published in the implementation map.
///
/// They are asserted rather than only bounded because they are the terms a later capacity
/// join consumes: a change to either is an observable-contract change, and the map and this
/// pin move together.
#[test]
fn the_compiler_side_maximum_live_graph_holds_its_accounted_figures() {
    assert_eq!(
        MAX_LIVE_DURABLE_GRAPH_BYTES, 42_850_312,
        "the accounted compiler-side durable contract graph moved; re-derive the exported \
         term and update the implementation map with this pin"
    );
    assert_eq!(
        MAX_LIVE_DURABLE_VALUE_ARENA_BYTES, 8_212_611_072,
        "the accounted durable value arena moved; it is the durable-value owner's term and \
         the implementation map publishes it"
    );
}

/// **The negative control.** The superseded representation — one fully expanded member
/// tree materialized per root occurrence, rather than one declaration shared by every
/// occurrence over it — does not close under the same ceiling.
///
/// Without this, the equation above would be satisfied by any representation at all,
/// including the one this row deleted, and closing it would not be evidence that the
/// deletion was load-bearing.
#[test]
fn the_superseded_per_occurrence_member_tree_does_not_close() {
    // Every occurrence carried its own copy of the Product's member rows, so the member
    // term was multiplied by the occurrence count instead of shared across it. Charged at
    // the widest split a ledger admits — half its anchors occurrences, half members — which
    // is the *most favourable* reading of the superseded shape.
    let half = LEDGER_ADMITTED_ANCHORS / 2;
    let superseded = marrow_image::bounds::max_live_durable_graph_bytes(
        marrow_image::bounds::DurableGraphCounts {
            products: 1,
            occurrences: half,
            indexes: 0,
            members: half * half,
            value_nodes: 0,
            value_references: 0,
        },
    );
    assert!(
        superseded > H_DURABLE_GRAPH_BYTES,
        "the superseded per-occurrence member tree must not close, or sharing one member \
         graph across occurrences is not load-bearing: {superseded}"
    );
}

/// A one-member Product and the ledger rows it declares, so an identity-state variation
/// is the only difference between the cases below.
fn one_member_source() -> String {
    String::from(
        "module main\n\nresource R {\n    required v: int\n}\n\nstore ^r[id: int]: R\n\n\
         pub fn noop(): int {\n    return 0\n}\n",
    )
}

fn one_member_anchors() -> Vec<String> {
    vec![
        "application .".into(),
        "product R".into(),
        "field R.v".into(),
        "root r".into(),
        "key r.id".into(),
    ]
}

/// The complete ledger compiles: the identity-state cases below differ from this by one
/// row each.
#[test]
fn a_complete_ledger_admits_its_product() {
    let anchors = one_member_anchors();
    let result = compile(&project(&one_member_source(), Some(&ledger(&anchors))));
    assert!(result.is_ok(), "{:?}", result.err());
}

/// A member whose anchor is absent is a located `check.durable_identity` — an earlier
/// typed declaration cause, not a member-count refusal.
#[test]
fn a_missing_member_anchor_is_a_located_identity_diagnostic() {
    let anchors: Vec<String> = one_member_anchors()
        .into_iter()
        .filter(|anchor| anchor != "field R.v")
        .collect();
    match compile(&project(&one_member_source(), Some(&ledger(&anchors)))) {
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            assert!(
                diagnostics
                    .iter()
                    .any(|row| row.code() == "check.durable_identity"
                        && !row.file().as_str().is_empty()),
                "{:#?}",
                diagnostics.as_slice()
            );
        }
        other => panic!("expected a located identity diagnostic, got {other:?}"),
    }
}

/// A retired anchor can never be reused, so its member is refused by the same located
/// identity cause rather than by any bound.
#[test]
fn a_retired_member_anchor_is_a_located_identity_diagnostic() {
    let mut text = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    for (seed, anchor) in one_member_anchors()
        .iter()
        .filter(|anchor| *anchor != "field R.v")
        .enumerate()
    {
        text.push_str(&format!("id {anchor} {:032x}\n", seed as u128 + 1));
    }
    text.push_str(&format!("retired field R.v {:032x} 1\n", 99u128));
    text.push_str("high-water 4\nend\n");
    match compile(&project(&one_member_source(), Some(text.as_bytes()))) {
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            assert!(
                diagnostics
                    .iter()
                    .any(|row| row.code() == "check.durable_identity"
                        && !row.file().as_str().is_empty()),
                "{:#?}",
                diagnostics.as_slice()
            );
        }
        other => panic!("expected a located identity diagnostic, got {other:?}"),
    }
}

/// Two live rows for one anchor is a malformed ledger: identity admission refuses it
/// before any compilation, so a duplicated member never reaches a count.
#[test]
fn a_duplicated_member_anchor_is_refused_before_compilation() {
    let mut text = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    for (seed, anchor) in one_member_anchors().iter().enumerate() {
        text.push_str(&format!("id {anchor} {:032x}\n", seed as u128 + 1));
    }
    text.push_str(&format!("id field R.v {:032x}\n", 99u128));
    text.push_str("high-water 6\nend\n");
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        one_member_source().into_bytes(),
    )];
    assert!(
        marrow_project::capture(
            &manifest,
            files,
            Some(text.as_bytes()),
            &CaptureLimits::DEFAULT
        )
        .is_err(),
        "a ledger with two rows for one anchor is refused at admission"
    );
}
