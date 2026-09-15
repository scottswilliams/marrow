//! Alias normalization through the production `compile` path: an accepted chain
//! collapses to its terminal shape, and an unsupported target refuses typed.

use std::fmt::Write as _;

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

fn chain_source(signature_type: &str, count: usize) -> String {
    let mut source = String::new();
    for alias in 0..count - 1 {
        writeln!(source, "alias A{alias:03} = A{:03}", alias + 1).expect("write alias");
    }
    writeln!(source, "alias A{:03} = int", count - 1).expect("write terminal alias");
    writeln!(
        source,
        "\npub fn identity(value: {signature_type}): {signature_type} {{\n    return value\n}}"
    )
    .expect("write function");
    source
}

/// However long the chain, the signature that names its head compiles to the same
/// image bytes as one that names the terminal directly: an alias carries no shape
/// of its own into the image.
#[test]
fn an_alias_chain_compiles_to_the_terminal_shape() {
    for count in [8, 64, 256] {
        let compiled =
            compile(&project(chain_source("A000", count))).expect("acyclic alias chain compiles");
        let direct =
            compile(&project(chain_source("int", count))).expect("direct int control compiles");
        assert_eq!(
            compiled.image.bytes, direct.image.bytes,
            "every accepted alias in the chain expands to the same terminal int shape"
        );
    }
}

#[test]
fn an_alias_to_an_unsupported_application_refuses_typed() {
    let mut source = String::from("alias A0 = int\n");
    for index in 1..=8 {
        writeln!(
            source,
            "alias A{index} = Pair<A{}, A{}>",
            index - 1,
            index - 1
        )
        .expect("write alias");
    }
    source.push_str("pub fn driver(): int { return 0 }\n");
    let Err(crate::CompileFailure::Diagnostics(diagnostics)) = compile(&project(source)) else {
        panic!("an unsupported alias target must be a source refusal");
    };
    assert!(
        diagnostics
            .iter()
            .all(|row| row.code().as_str() == "check.unsupported"),
        "every row refuses the unsupported target itself",
    );
}

#[test]
fn many_aliases_share_one_named_target() {
    let terminal = format!("Type{}", "x".repeat(1024));
    let mut source = format!("struct {terminal} {{ value: int }}\nalias Root = {terminal}\n");
    for index in 0..256 {
        writeln!(source, "alias A{index} = Root").expect("write alias");
    }
    source.push_str("pub fn driver(): int { return 0 }\n");
    compile(&project(source)).expect("the shared named target compiles");
}

#[test]
fn composed_optional_aliases_refuse_at_each_dependent_declaration() {
    let source = "alias A = int?\nalias B = A?\nalias C = B\npub fn driver(): int { return 0 }\n";
    let Err(crate::CompileFailure::Diagnostics(diagnostics)) = compile(&project(source.into()))
    else {
        panic!("double optionality must be a source refusal");
    };
    let rows: Vec<_> = diagnostics
        .iter()
        .map(|row| (row.code().as_str(), row.line(), row.column()))
        .collect();
    assert_eq!(
        rows,
        [("check.unsupported", 2, 1), ("check.unsupported", 3, 1)]
    );
    assert!(
        diagnostics
            .iter()
            .last()
            .expect("dependent refusal")
            .refused_declaration()
            .is_some()
    );
}
