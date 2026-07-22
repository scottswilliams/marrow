//! The analysis snapshot answers active-call at a source position with the innermost
//! enclosing call's callee signature, its typed parameter pieces, and the active argument
//! index — derived purely positionally over the retained parse tree. The query runs on a
//! broken file over recovered incomplete-call nodes (the just-opened and just-typed-comma
//! trigger moments), presents a generic callee's template signature, refuses an over-cap
//! rendered display as a query-local resource limit rather than truncating, and
//! distinguishes an invalid coordinate from a legitimate absence.

use std::sync::Arc;

use marrow_compile::{
    ActiveCall, ActiveCallOutcome, AnalysisResourceLimit, AnalysisSnapshot, Fact, InputRevision,
    MAX_ACTIVE_CALL_RENDER_BYTES, QueryError, Unavailability, analyze,
};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};

fn project_bytes(files: &[(&str, Vec<u8>)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.clone()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn snap(source: &str) -> Arc<AnalysisSnapshot> {
    let files = [("src/app.mw", source.as_bytes().to_vec())];
    let Ok(snapshot) = analyze(Arc::new(project_bytes(&files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot");
    };
    snapshot
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path).expect("canonical identity").0
}

/// The byte offset of `needle` in `source`, advanced by `extra` bytes.
fn at(source: &str, needle: &str, extra: usize) -> usize {
    source.find(needle).expect("needle present") + extra
}

/// The present active-call fact at an offset, or a panic describing the outcome.
fn present(snapshot: &AnalysisSnapshot, offset: usize) -> ActiveCall {
    match snapshot.active_call(&identity("src/app.mw"), offset) {
        Ok(ActiveCallOutcome::Ready(Fact::Present(active))) => active,
        other => panic!("expected a present active call, got {}", describe(&other)),
    }
}

fn describe(outcome: &Result<ActiveCallOutcome, QueryError>) -> &'static str {
    match outcome {
        Ok(ActiveCallOutcome::Ready(Fact::Present(_))) => "Present",
        Ok(ActiveCallOutcome::Ready(Fact::Absent)) => "Absent",
        Ok(ActiveCallOutcome::Ready(Fact::Unavailable(_))) => "Unavailable",
        Ok(ActiveCallOutcome::Refused(_)) => "Refused",
        Err(_) => "QueryError",
    }
}

fn labels(active: &ActiveCall) -> Vec<String> {
    active
        .params()
        .iter()
        .map(|piece| piece.label().to_string())
        .collect()
}

#[test]
fn active_call_inside_a_complete_call_marks_the_argument() {
    let source = "module app\n\n\
        fn getOr(m: int, key: int, fallback: int): int {\n    return fallback\n}\n\n\
        fn caller(): int {\n    return getOr(1, 2, 3)\n}\n";
    let snapshot = snap(source);
    // The cursor sits inside the second argument `2`.
    let offset = at(source, "getOr(1, 2, 3)", "getOr(1, ".len());
    let active = present(&snapshot, offset);
    assert_eq!(
        active.signature(),
        "fn getOr(m: int, key: int, fallback: int): int"
    );
    assert_eq!(
        labels(&active),
        vec![
            "m: int".to_string(),
            "key: int".to_string(),
            "fallback: int".to_string()
        ]
    );
    assert_eq!(active.active(), Some(1));
}

#[test]
fn active_call_at_the_open_paren_marks_the_first_parameter() {
    // The just-opened `f(` moment: a recovered incomplete call with no arguments yet still
    // resolves and marks the first parameter.
    let source = "module app\n\n\
        fn getOr(m: int, key: int): int {\n    return m\n}\n\n\
        fn caller(): int {\n    return getOr(\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return getOr(\n}", "return getOr(".len());
    let active = present(&snapshot, offset);
    assert_eq!(active.signature(), "fn getOr(m: int, key: int): int");
    assert_eq!(active.active(), Some(0));
}

#[test]
fn active_call_after_a_trailing_comma_marks_the_next_parameter() {
    // The just-typed-comma moment `getOr(reached, ` — the production-red position. A
    // recovered incomplete call marks the second parameter even across the trailing space.
    let source = "module app\n\n\
        fn getOr(m: int, key: int, fallback: int): int {\n    return fallback\n}\n\n\
        fn caller(): int {\n    const reached = 1\n    return getOr(reached, \n}\n";
    let snapshot = snap(source);
    let offset = at(source, "getOr(reached, \n}", "getOr(reached, ".len());
    let active = present(&snapshot, offset);
    assert_eq!(
        active.signature(),
        "fn getOr(m: int, key: int, fallback: int): int"
    );
    assert_eq!(active.active(), Some(1));
}

#[test]
fn active_call_broken_file_still_resolves() {
    // A recovered incomplete call (a second argument typed, no closing paren) makes the
    // file broken (a parse.syntax error), yet the active-call fact resolves — the point of
    // parser-owned recovery for signature help.
    let source = "module app\n\n\
        fn getOr(m: int, key: int): int {\n    return m\n}\n\n\
        fn caller(): int {\n    return getOr(1, key\n}\n";
    let snapshot = snap(source);
    let file = identity("src/app.mw");
    let offset = at(source, "getOr(1, key\n}", "getOr(1, k".len());
    // The file is genuinely broken: a hover in it is syntax-unavailable.
    assert!(
        matches!(
            snapshot.hover(&file, offset),
            Ok(Fact::Unavailable(Unavailability::Syntax))
        ),
        "the file must be broken for this to prove the law",
    );
    let active = present(&snapshot, offset);
    assert_eq!(active.active(), Some(1));
}

#[test]
fn active_call_on_a_generic_callee_shows_the_template_signature() {
    let source = "module app\n\n\
        fn wrap<T>(value: T, count: int): T {\n    return value\n}\n\n\
        fn caller(): int {\n    return wrap(1, 2)\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "wrap(1, 2)", "wrap(1".len());
    let active = present(&snapshot, offset);
    assert_eq!(active.signature(), "fn wrap<T>(value: T, count: int): T");
    assert_eq!(
        labels(&active),
        vec!["value: T".to_string(), "count: int".to_string()]
    );
    assert_eq!(active.active(), Some(0));
}

#[test]
fn active_call_inside_a_nested_call_marks_the_inner_call() {
    let source = "module app\n\n\
        fn inner(a: int, b: int): int {\n    return a\n}\n\n\
        fn outer(x: int): int {\n    return x\n}\n\n\
        fn caller(): int {\n    return outer(inner(1, 2))\n}\n";
    let snapshot = snap(source);
    // The cursor sits inside the inner call's second argument.
    let offset = at(source, "inner(1, 2)", "inner(1, ".len());
    let active = present(&snapshot, offset);
    assert_eq!(active.signature(), "fn inner(a: int, b: int): int");
    assert_eq!(active.active(), Some(1));
}

#[test]
fn active_call_zero_parameter_callee_has_no_active_parameter() {
    let source = "module app\n\n\
        fn now(): int {\n    return 1\n}\n\n\
        fn caller(): int {\n    return now()\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return now()", "return now(".len());
    let active = present(&snapshot, offset);
    assert_eq!(active.signature(), "fn now(): int");
    assert!(active.params().is_empty());
    assert_eq!(active.active(), None);
}

#[test]
fn active_call_on_a_builtin_callee_is_absent() {
    // A built-in callee resolves to no local declaration on this floor: a legitimate
    // absence, not a fabricated fact.
    let source = "module app\n\n\
        fn caller(): int {\n    return length(1, 2)\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "length(1, 2)", "length(1, ".len());
    assert!(
        matches!(
            snapshot.active_call(&identity("src/app.mw"), offset),
            Ok(ActiveCallOutcome::Ready(Fact::Absent))
        ),
        "a built-in callee has no local declaration",
    );
}

#[test]
fn active_call_outside_any_call_is_absent() {
    let source = "module app\n\n\
        fn caller(): int {\n    return 42\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return 42", "return 4".len());
    assert!(matches!(
        snapshot.active_call(&identity("src/app.mw"), offset),
        Ok(ActiveCallOutcome::Ready(Fact::Absent))
    ));
}

#[test]
fn active_call_render_bytes_refuses_a_pathological_display() {
    // A callee whose rendered signature and parameter pieces exceed the per-query render
    // budget refuses as a query-local resource limit, never a truncated display. Many
    // parameters with a long declared type-alias spelling overshoot the budget.
    let param_type = "a".repeat(64);
    let mut source = String::from("module app\n\n");
    source.push_str(&format!("alias {param_type} = int\n\n"));
    source.push_str("fn big(");
    let param_count = (MAX_ACTIVE_CALL_RENDER_BYTES as usize / (param_type.len() + 8)) + 4;
    for index in 0..param_count {
        if index > 0 {
            source.push_str(", ");
        }
        source.push_str(&format!("p{index}: {param_type}"));
    }
    source.push_str("): int {\n    return 1\n}\n\n");
    source.push_str("fn caller(): int {\n    return big()\n}\n");
    let snapshot = snap(&source);
    let offset = at(&source, "big()", "big(".len());
    match snapshot.active_call(&identity("src/app.mw"), offset) {
        Ok(ActiveCallOutcome::Refused(AnalysisResourceLimit::ActiveCallRenderBytes { limit })) => {
            assert_eq!(limit, MAX_ACTIVE_CALL_RENDER_BYTES);
        }
        other => panic!("expected a render-byte refusal, got {}", describe(&other)),
    }
}

#[test]
fn active_call_render_bytes_boundary_admits_max_and_refuses_one_more() {
    // The rendered-byte cap is a strict `>`: a display of exactly MAX bytes is admitted,
    // one byte more refuses — pinning the boundary so an off-by-one to `>=` cannot pass
    // unnoticed. For a callee `fn NAME(p: ALIAS): int`, the charged bytes are the signature
    // plus the one parameter label:
    //   signature = "fn " + NAME + "(" + "p: " + ALIAS + ")" + ": " + "int"
    //             = 13 + NAME.len() + ALIAS.len()
    //   label     = "p: " + ALIAS = 3 + ALIAS.len()
    //   total     = 16 + NAME.len() + 2 * ALIAS.len()
    // With a two-character name and an alias length chosen to land the total on the cap.
    let cap = MAX_ACTIVE_CALL_RENDER_BYTES as usize;
    assert_eq!(
        (cap - 18) % 2,
        0,
        "cap parity admits an exact-boundary alias"
    );
    let alias_len = (cap - 16 - 2) / 2;
    let alias = "a".repeat(alias_len);

    let build = |name: &str| {
        let mut source = String::from("module app\n\n");
        source.push_str(&format!("alias {alias} = int\n\n"));
        source.push_str(&format!(
            "fn {name}(p: {alias}): int {{\n    return 1\n}}\n\n"
        ));
        source.push_str(&format!("fn caller(): int {{\n    return {name}()\n}}\n"));
        source
    };

    // A two-character name renders exactly the cap: admitted, never refused.
    let at_cap = build("ff");
    let snapshot = snap(&at_cap);
    let offset = at(&at_cap, "ff()", "ff(".len());
    match snapshot.active_call(&identity("src/app.mw"), offset) {
        Ok(ActiveCallOutcome::Ready(Fact::Present(active))) => {
            assert_eq!(
                active.signature().len() + active.params()[0].label().len(),
                cap,
                "the admitted display is exactly the cap"
            );
        }
        other => panic!(
            "expected an exactly-cap present display, got {}",
            describe(&other)
        ),
    }

    // A three-character name adds one byte: one past the cap refuses.
    let over_cap = build("fff");
    let snapshot = snap(&over_cap);
    let offset = at(&over_cap, "fff()", "fff(".len());
    match snapshot.active_call(&identity("src/app.mw"), offset) {
        Ok(ActiveCallOutcome::Refused(AnalysisResourceLimit::ActiveCallRenderBytes { limit })) => {
            assert_eq!(limit, MAX_ACTIVE_CALL_RENDER_BYTES);
        }
        other => panic!(
            "expected a render-byte refusal one past the cap, got {}",
            describe(&other)
        ),
    }
}

#[test]
fn active_call_unknown_file() {
    let source = "module app\n\nfn f(): int {\n    return 1\n}\n";
    let snapshot = snap(source);
    assert!(matches!(
        snapshot.active_call(&identity("src/other.mw"), 0),
        Err(QueryError::UnknownFile)
    ));
}

#[test]
fn active_call_offset_out_of_range() {
    let source = "module app\n\nfn f(): int {\n    return 1\n}\n";
    let snapshot = snap(source);
    assert!(matches!(
        snapshot.active_call(&identity("src/app.mw"), source.len() + 1),
        Err(QueryError::OffsetOutOfRange)
    ));
}

#[test]
fn active_call_non_utf8_file_is_unavailable() {
    let files = [("src/app.mw", vec![0x66, 0x6e, 0xff, 0xfe])];
    let Ok(snapshot) = analyze(Arc::new(project_bytes(&files)), InputRevision::new(1)) else {
        panic!("expected a snapshot");
    };
    assert!(matches!(
        snapshot.active_call(&identity("src/app.mw"), 0),
        Ok(ActiveCallOutcome::Ready(Fact::Unavailable(
            Unavailability::Syntax
        )))
    ));
}
