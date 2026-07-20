//! Every source diagnostic names one real FIDB01-bounded `FileIdentity`, never an
//! empty or sentinel filename. The editor analysis floor (H00f) attributes each
//! diagnostic to a captured source file; a diagnostic with no truthful file is not
//! a source diagnostic.
//!
//! This pins the behavioral half of the sentinel-elimination checkpoint: an
//! instantiation-limit diagnostic — the one path that previously fell back to a
//! reserved template's empty file and a 0:0 span — now carries the real use-site
//! file. The structural half (the reserved `TypeTemplate` no longer being able to
//! hold an empty file) is enforced by the type: `TypeTemplate::file` is
//! `Option<FileIdentity>`, so an empty-string file cannot be constructed.

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};

fn project(files: Vec<(&str, &str)>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .into_iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn diagnostics(files: Vec<(&str, &str)>) -> Vec<SourceDiagnostic> {
    match compile(&project(files)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        other => panic!("expected source diagnostics, got {other:?}"),
    }
}

/// A source diagnostic's file is a canonical captured identity, exactly equal to
/// the identity the project captured it under.
#[test]
fn a_source_diagnostic_names_a_real_captured_file_identity() {
    let source = "fn main() {\n    let x: Nonexistent = 0\n}\n";
    let produced = diagnostics(vec![("src/main.mw", source)]);
    let expected = FileIdentity::validate("src/main.mw")
        .expect("canonical identity")
        .0;
    for diagnostic in &produced {
        assert_eq!(
            diagnostic.file(),
            &expected,
            "every diagnostic names the captured file identity",
        );
        assert!(!diagnostic.file().as_str().is_empty());
    }
}

/// The instantiation-limit diagnostic attributes to the real use site's file,
/// where the deleted `site.file.is_empty()` fallback would have emitted a reserved
/// template's empty file and a 0:0 span.
#[test]
fn the_instantiation_limit_diagnostic_carries_the_real_use_site_file() {
    let library = "module library\n\nstruct Grow<T> {\n    next: Grow<List<T>>\n}\n\n\
                   pub fn deepen<T>(x: T): Grow<T> {\n    return deepen(x)\n}\n";
    let main = "module main\nuse library\n\n\
                pub fn driver(): int {\n    const ignored = library::deepen(1)\n    return 0\n}\n";
    let produced = diagnostics(vec![("src/library.mw", library), ("src/main.mw", main)]);
    let limit = produced
        .iter()
        .find(|d| d.code == "check.instantiation_limit")
        .expect("an instantiation-limit diagnostic");
    let expected = FileIdentity::validate("src/library.mw")
        .expect("canonical identity")
        .0;
    assert_eq!(limit.file(), &expected);
    assert!(!limit.file().as_str().is_empty());
    assert!(limit.line() >= 1 && limit.column() >= 1);
}
