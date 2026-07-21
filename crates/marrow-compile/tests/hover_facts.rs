//! The analysis snapshot answers hover at a source position with the compiler's
//! canonical type display for a resolved local or parameter use, and distinguishes a
//! genuine absence from a syntax-unavailable position and from an invalid coordinate.

use std::sync::Arc;

use marrow_compile::{
    AnalysisSnapshot, Definition, Fact, InputRevision, QueryError, Unavailability, analyze,
};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};

fn project(files: &[(&str, &str)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// Analyze a project and unwrap its snapshot (the opaque `AnalysisFailure` is not
/// `Debug`, so a `let`-else keeps the failure boundary opaque).
fn snap(files: &[(&str, &str)]) -> Arc<AnalysisSnapshot> {
    let Ok(snapshot) = analyze(Arc::new(project(files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot for {files:?}");
    };
    snapshot
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path).expect("canonical identity").0
}

/// The byte offset of the first occurrence of `needle` in `source`.
fn offset_of(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle present in source")
}

#[test]
fn hover_on_a_parameter_use_shows_its_value_type() {
    let source = "pub fn f(x: int): int {\n    return x\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = offset_of(source, "return x") + "return ".len();
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
    let use_offset = offset_of(source, "return n") + "return ".len();
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
    let literal = offset_of(source, "return 1") + "return ".len();
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
    // The broken module still parses to an identity; a hover in it is Unavailable(Syntax).
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
    let use_offset = offset_of(valid, "return x") + "return ".len();
    match snapshot.hover(&identity("src/valid.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!(
            "expected Present(int) in the valid module, got {}",
            label(&other)
        ),
    }
}

#[test]
fn hover_on_a_same_module_call_shows_the_resolved_signature() {
    let source = "pub fn add(a: int, b: int): int {\n    return a\n}\n\n\
                  pub fn f(): int {\n    return add(1, 2)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let call_offset = offset_of(source, "add(1, 2)");
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
    let call_offset = offset_of(main, "lib::helper") + "lib::".len();
    match snapshot.hover(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "fn helper(int): int"),
        other => panic!("expected the resolved signature, got {}", label(&other)),
    }
}

#[test]
fn hover_inside_a_generic_body_is_absent_on_this_floor() {
    // Only monomorphic function and test bodies are collected; a position inside a
    // generic function's body is honestly `Absent` on this floor. A future change to
    // this boundary must be a deliberate red here, not silent drift.
    let source = "pub fn id<T>(x: T): T {\n    return x\n}\n\n\
                  pub fn f(): int {\n    return id(1)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = offset_of(source, "return x") + "return ".len();
    assert!(matches!(
        snapshot.hover(&identity("src/main.mw"), use_offset),
        Ok(Fact::Absent)
    ));
}

#[test]
fn definition_on_a_same_module_call_targets_the_declaration() {
    let source = "pub fn add(a: int, b: int): int {\n    return a\n}\n\n\
                  pub fn f(): int {\n    return add(1, 2)\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let call_offset = offset_of(source, "add(1, 2)");
    match snapshot.definition(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(def)) => {
            assert_eq!(def.file().as_str(), "src/main.mw");
            // The selection range is the declaration's name, not the call's.
            let name = &source[def.name_span().start_byte..def.name_span().end_byte];
            assert_eq!(name, "add");
            assert_eq!(def.name_span().start_byte, offset_of(source, "add"));
            // The declaration range runs from the header start through the body end.
            assert_eq!(def.declaration_range().start_byte, 0);
            assert!(def.declaration_range().end_byte > offset_of(source, "return a"));
        }
        other => panic!("expected a definition, got {}", label_def(&other)),
    }
}

#[test]
fn definition_on_a_cross_module_call_targets_the_other_file() {
    let lib = "module lib\n\npub fn helper(x: int): int {\n    return x\n}\n";
    let main = "module main\nuse lib\n\npub fn f(): int {\n    return lib::helper(1)\n}\n";
    let snapshot = snap(&[("src/lib.mw", lib), ("src/main.mw", main)]);
    let call_offset = offset_of(main, "lib::helper") + "lib::".len();
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
    let use_offset = offset_of(source, "return x") + "return ".len();
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
    let call_offset = offset_of(source, "id(1)");
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
    let call_offset = offset_of(source, "id(1)");
    match snapshot.definition(&identity("src/main.mw"), call_offset) {
        Ok(Fact::Present(def)) => {
            assert_eq!(def.file().as_str(), "src/main.mw");
            // The target is the template declaration, not a minted instance.
            let name = &source[def.name_span().start_byte..def.name_span().end_byte];
            assert_eq!(name, "id");
            assert_eq!(def.name_span().start_byte, offset_of(source, "id"));
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
    let call_offset = offset_of(main, "lib::wrap") + "lib::".len();
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

fn label_def(fact: &Result<Fact<Definition>, QueryError>) -> &'static str {
    label(fact)
}

fn label<T>(fact: &Result<Fact<T>, QueryError>) -> &'static str {
    match fact {
        Ok(Fact::Present(_)) => "Present",
        Ok(Fact::Absent) => "Absent",
        Ok(Fact::Unavailable(Unavailability::Syntax)) => "Unavailable(Syntax)",
        Ok(Fact::Unavailable(Unavailability::Dependency)) => "Unavailable(Dependency)",
        Err(QueryError::UnknownFile) => "Err(UnknownFile)",
        Err(QueryError::OffsetOutOfRange) => "Err(OffsetOutOfRange)",
    }
}
