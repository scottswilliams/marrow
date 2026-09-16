//! The analysis snapshot answers hover at a source position with the compiler's
//! canonical type display for a resolved local or parameter use, and distinguishes a
//! genuine absence from a syntax-unavailable position and from an invalid coordinate.

use std::sync::Arc;

use marrow_compile::{Definition, Fact, InputRevision, QueryError, Unavailability, analyze};

use super::{at, identity, project_bytes, snap};

#[test]
fn hover_on_a_parameter_use_shows_its_value_type() {
    let source = "pub fn f(x: int): int {\n    return x\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = at(source, "return x", 0) + "return ".len();
    match snapshot.hover(&identity("src/main.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!(
            "expected Present(int), got a different outcome: {}",
            label(&other)
        ),
    }
}

#[test]
fn hover_on_a_local_use_shows_its_inferred_type() {
    let source = "pub fn f(): int {\n    const n = 7\n    return n\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = at(source, "return n", 0) + "return ".len();
    match snapshot.hover(&identity("src/main.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!("expected Present(int), got {}", label(&other)),
    }
}

#[test]
fn hover_on_a_valid_position_with_no_fact_is_absent() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    // The `1` literal is a valid position with no local/parameter fact.
    let literal = at(source, "return 1", 0) + "return ".len();
    assert!(matches!(
        snapshot.hover(&identity("src/main.mw"), literal),
        Ok(Fact::Absent)
    ));
}

#[test]
fn hover_in_an_unknown_file_is_a_query_error() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    assert!(matches!(
        snapshot.hover(&identity("src/other.mw"), 0),
        Err(QueryError::UnknownFile)
    ));
}

#[test]
fn hover_at_an_out_of_range_offset_is_a_query_error_not_absence() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    assert!(matches!(
        snapshot.hover(&identity("src/main.mw"), source.len() + 1),
        Err(QueryError::OffsetOutOfRange)
    ));
}

#[test]
fn hover_in_a_parse_failed_module_is_syntax_unavailable() {
    // The broken module still parses to an identity.
    let broken = "module broken\n\npub fn g(: int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/broken.mw", broken)]);
    assert!(matches!(
        snapshot.hover(&identity("src/broken.mw"), 0),
        Ok(Fact::Unavailable(Unavailability::Syntax))
    ));
}

#[test]
fn a_valid_module_keeps_hover_facts_past_a_sibling_parse_error() {
    let valid = "module valid\n\npub fn h(x: int): int {\n    return x\n}\n";
    let broken = "module broken\n\npub fn g(: int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/valid.mw", valid), ("src/broken.mw", broken)]);
    let use_offset = at(valid, "return x", 0) + "return ".len();
    match snapshot.hover(&identity("src/valid.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!(
            "expected Present(int) in the valid module, got {}",
            label(&other)
        ),
    }
}

/// A fact is admitted where it is derived, and a later failure of the pass that derived it
/// does not retract it: a body that fails to type-check still answers hover for the uses
/// that resolved before the failure.
#[test]
fn a_body_that_fails_to_check_keeps_the_facts_it_already_derived() {
    let source = "module main

pub fn f(x: int): int {
    var t: int = x
    return \"not an int\"
}
";
    let snapshot = snap(&[("src/main.mw", source)]);
    assert!(
        !snapshot.diagnostics().is_empty(),
        "the fixture's body fails to check"
    );
    let use_offset = at(source, "= x", 0) + "= ".len();
    match snapshot.hover(&identity("src/main.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!(
            "expected the resolved use's fact to survive the body's failure, got {}",
            label(&other)
        ),
    }
}

/// The same for the once-checked template pass: it admits facts through the same sink at
/// each push, then rewinds its proof-appended draft and registry suffixes. The rewind
/// restores those owners and nothing else, so already-admitted facts survive its failure.
///
/// The fixture's `==` over an unconstrained parameter is a constraint violation the proof
/// reports without instantiating, so no instantiation could have re-derived the hover.
#[test]
fn a_failed_template_proof_keeps_the_facts_it_already_derived() {
    let source = "module main

pub fn same<T>(a: T, b: T): bool {
    var left: T = a
    return left == b
}

pub fn run(): bool {
    return same(1, 2)
}
";
    let snapshot = snap(&[("src/main.mw", source)]);
    assert!(
        !snapshot.diagnostics().is_empty(),
        "the fixture's template proof fails on the unconstrained `==`"
    );
    let use_offset = at(source, "= a", 0) + "= ".len();
    match snapshot.hover(&identity("src/main.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "T"),
        other => panic!(
            "expected the template proof's own fact to survive its failure, got {}",
            label(&other)
        ),
    }
}

#[test]
fn hover_on_a_same_module_call_shows_the_resolved_signature() {
    let source = "pub fn add(a: int, b: int): int {\n    return a\n}\n\n\
                  pub fn f(): int {\n    return add(1, 2)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let call_offset = at(source, "add(1, 2)", 0);
    match snapshot.hover(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "fn add(int, int): int"),
        other => panic!("expected the resolved signature, got {}", label(&other)),
    }
}

#[test]
fn hover_on_a_cross_module_call_shows_the_resolved_signature() {
    let lib = "module lib\n\npub fn helper(x: int): int {\n    return x\n}\n";
    let main = "module main\nuse lib\n\npub fn f(): int {\n    return lib::helper(1)\n}\n";
    let snapshot = snap(&[("src/lib.mw", lib), ("src/main.mw", main)]);
    // The origin is the callee leaf `helper`, not the `lib` prefix.
    let call_offset = at(main, "lib::helper", 0) + "lib::".len();
    match snapshot.hover(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "fn helper(int): int"),
        other => panic!("expected the resolved signature, got {}", label(&other)),
    }
}

#[test]
fn hover_inside_a_generic_body_shows_the_template_parameter_spelling() {
    // A template-parameter use renders by its declared spelling (`T`), not the positional
    // `type parameter #0` form.
    let source = "pub fn id<T>(x: T): T {\n    return x\n}\n\n\
                  pub fn f(): int {\n    return id(1)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = at(source, "return x", 0) + "return ".len();
    match snapshot.hover(&identity("src/main.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "T"),
        other => panic!(
            "expected the template-parameter spelling, got {}",
            label(&other)
        ),
    }
}

#[test]
fn definition_inside_a_generic_body_targets_a_called_helper() {
    // A call inside a template body resolves to its callee's declaration.
    let source = "pub fn helper(n: int): int {\n    return n\n}\n\n\
                  pub fn wrap<T>(x: T): int {\n    return helper(1)\n}\n\n\
                  pub fn f(): int {\n    return wrap(1)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let call_offset = at(source, "return helper(1)", 0) + "return ".len();
    match snapshot.definition(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(def)) => {
            let name = &source[def.name_span().start_byte..def.name_span().end_byte];
            assert_eq!(name, "helper");
            assert_eq!(def.name_span().start_byte, at(source, "helper", 0));
        }
        other => panic!("expected the helper definition, got {}", label_def(&other)),
    }
}

#[test]
fn definition_on_a_same_module_call_targets_the_declaration() {
    let source = "pub fn add(a: int, b: int): int {\n    return a\n}\n\n\
                  pub fn f(): int {\n    return add(1, 2)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let call_offset = at(source, "add(1, 2)", 0);
    match snapshot.definition(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(def)) => {
            assert_eq!(def.file().as_str(), "src/main.mw");
            // The selection range is the declaration's name, not the call's.
            let name = &source[def.name_span().start_byte..def.name_span().end_byte];
            assert_eq!(name, "add");
            assert_eq!(def.name_span().start_byte, at(source, "add", 0));
            // The declaration range runs from the header start through the body end.
            assert_eq!(def.declaration_range().start_byte, 0);
            assert!(def.declaration_range().end_byte > at(source, "return a", 0));
        }
        other => panic!("expected a definition, got {}", label_def(&other)),
    }
}

#[test]
fn definition_on_a_cross_module_call_targets_the_other_file() {
    let lib = "module lib\n\npub fn helper(x: int): int {\n    return x\n}\n";
    let main = "module main\nuse lib\n\npub fn f(): int {\n    return lib::helper(1)\n}\n";
    let snapshot = snap(&[("src/lib.mw", lib), ("src/main.mw", main)]);
    let call_offset = at(main, "lib::helper", 0) + "lib::".len();
    match snapshot.definition(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(def)) => {
            assert_eq!(def.file().as_str(), "src/lib.mw");
            let name = &lib[def.name_span().start_byte..def.name_span().end_byte];
            assert_eq!(name, "helper");
        }
        other => panic!(
            "expected a cross-module definition, got {}",
            label_def(&other)
        ),
    }
}

#[test]
fn definition_on_a_local_use_is_absent() {
    let source = "pub fn f(x: int): int {\n    return x\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = at(source, "return x", 0) + "return ".len();
    assert!(matches!(
        snapshot.definition(&identity("src/main.mw"), use_offset),
        Ok(Fact::Absent)
    ));
}

#[test]
fn definition_in_an_unknown_file_is_a_query_error() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    assert!(matches!(
        snapshot.definition(&identity("src/other.mw"), 0),
        Err(QueryError::UnknownFile)
    ));
}

#[test]
fn hover_on_a_generic_call_shows_the_template_signature() {
    let source = "pub fn id<T>(x: T): T {\n    return x\n}\n\n\
                  pub fn f(): int {\n    return id(1)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let call_offset = at(source, "id(1)", 0);
    match snapshot.hover(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "fn id<T>(T): T"),
        other => panic!("expected the template signature, got {}", label(&other)),
    }
}

#[test]
fn definition_on_a_generic_call_targets_the_source_template() {
    let source = "pub fn id<T>(x: T): T {\n    return x\n}\n\n\
                  pub fn f(): int {\n    return id(1)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let call_offset = at(source, "id(1)", 0);
    match snapshot.definition(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(def)) => {
            assert_eq!(def.file().as_str(), "src/main.mw");
            // The target is the template declaration, not a minted instance.
            let name = &source[def.name_span().start_byte..def.name_span().end_byte];
            assert_eq!(name, "id");
            assert_eq!(def.name_span().start_byte, at(source, "id", 0));
        }
        other => panic!(
            "expected the template definition, got {}",
            label_def(&other)
        ),
    }
}

#[test]
fn definition_on_a_cross_module_generic_call_targets_the_template_file() {
    let lib = "module lib\n\npub fn wrap<T>(x: T): T {\n    return x\n}\n";
    let main = "module main\nuse lib\n\npub fn f(): int {\n    return lib::wrap(1)\n}\n";
    let snapshot = snap(&[("src/lib.mw", lib), ("src/main.mw", main)]);
    let call_offset = at(main, "lib::wrap", 0) + "lib::".len();
    match snapshot.definition(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(def)) => {
            assert_eq!(def.file().as_str(), "src/lib.mw");
            let name = &lib[def.name_span().start_byte..def.name_span().end_byte];
            assert_eq!(name, "wrap");
        }
        other => panic!(
            "expected a cross-module template definition, got {}",
            label_def(&other)
        ),
    }
}

#[test]
fn hover_on_a_call_to_a_parse_failed_module_is_dependency_unavailable() {
    let broken = "module broken\n\npub fn helper(: int {\n    return 1\n}\n";
    let main = "module main\nuse broken\n\npub fn f(): int {\n    return broken::helper()\n}\n";
    let snapshot = snap(&[("src/broken.mw", broken), ("src/main.mw", main)]);
    // The callee leaf `helper` targets a module that did not parse.
    let call_offset = at(main, "broken::helper", 0) + "broken::".len();
    assert!(matches!(
        snapshot.hover(&identity("src/main.mw"), call_offset),
        Ok(Fact::Unavailable(Unavailability::Dependency))
    ));
}

#[test]
fn definition_on_a_call_to_a_parse_failed_module_is_dependency_unavailable() {
    let broken = "module broken\n\npub fn helper(: int {\n    return 1\n}\n";
    let main = "module main\nuse broken\n\npub fn f(): int {\n    return broken::helper()\n}\n";
    let snapshot = snap(&[("src/broken.mw", broken), ("src/main.mw", main)]);
    let call_offset = at(main, "broken::helper", 0) + "broken::".len();
    assert!(matches!(
        snapshot.definition(&identity("src/main.mw"), call_offset),
        Ok(Fact::Unavailable(Unavailability::Dependency))
    ));
}

#[test]
fn an_unrelated_valid_position_is_absent_not_dependency_unavailable() {
    // The dependency gap is per-position: with a broken sibling in the project, an
    // unrelated valid position with no fact is `Absent`, not `Unavailable(Dependency)`.
    let broken = "module broken\n\npub fn helper(: int {\n    return 1\n}\n";
    let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/broken.mw", broken), ("src/main.mw", main)]);
    let literal = at(main, "return 1", 0) + "return ".len();
    assert!(matches!(
        snapshot.hover(&identity("src/main.mw"), literal),
        Ok(Fact::Absent)
    ));
}

#[test]
fn a_call_to_a_non_utf8_module_is_dependency_unavailable() {
    // A non-UTF-8 source never enters parsing, but it is still a module that did not parse:
    // a qualified call into it is a dependency gap, not an absence.
    let main = "module main\nuse broken\n\n\
                pub fn f(): int {\n    return broken::helper()\n}\n\n\
                pub fn g(): int {\n    return 5\n}\n";
    let input = project_bytes(&[
        ("src/broken.mw", vec![0xff, 0xfe, 0x00]),
        ("src/main.mw", main.as_bytes().to_vec()),
    ]);
    let Ok(snapshot) = analyze(Arc::new(input), InputRevision::new(1)) else {
        panic!("a resilient snapshot is produced");
    };
    let main_id = identity("src/main.mw");
    let call_offset = at(main, "broken::helper", 0) + "broken::".len();
    assert!(matches!(
        snapshot.hover(&main_id, call_offset),
        Ok(Fact::Unavailable(Unavailability::Dependency))
    ));
    assert!(matches!(
        snapshot.definition(&main_id, call_offset),
        Ok(Fact::Unavailable(Unavailability::Dependency))
    ));
    // A valid literal in the same file with a non-UTF-8 sibling stays Absent, not Dependency.
    let literal = at(main, "return 5", 0) + "return ".len();
    assert!(matches!(
        snapshot.hover(&main_id, literal),
        Ok(Fact::Absent)
    ));
}

#[test]
fn analysis_floor_boundary_comments_are_gone() {
    // Hover and definition cover positions inside a generic template body, so no comment
    // may claim that deferral.
    let analysis = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/analysis.rs"))
        .expect("analysis.rs is readable");
    assert!(
        !analysis.contains("Floor boundary"),
        "the stale `Floor boundary` comments must be deleted once generic-template-body \
         facts are collected",
    );
}

fn label_def(fact: &Result<Fact<Definition>, QueryError>) -> &'static str {
    label(fact)
}

fn label<T>(fact: &Result<Fact<T>, QueryError>) -> &'static str {
    match fact {
        Ok(Fact::Present(_)) => "Present",
        Ok(Fact::Absent) => "Absent",
        Ok(Fact::Unavailable(Unavailability::Syntax)) => "Unavailable(Syntax)",
        Ok(Fact::Unavailable(Unavailability::Dependency)) => "Unavailable(Dependency)",
        Ok(Fact::Unavailable(Unavailability::Bounded)) => "Unavailable(Bounded)",
        Err(QueryError::UnknownFile) => "Err(UnknownFile)",
        Err(QueryError::OffsetOutOfRange) => "Err(OffsetOutOfRange)",
    }
}
