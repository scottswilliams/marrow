//! One module's parse failure does not suppress another module's editor facts, and the
//! projection that makes that true is the same one an abandoned body's facts would reach.
//!
//! The production compile refuses a project whose parse stage produced diagnostics before
//! it consults the semantic outcome. The complete-union projection that `analyze` and
//! `check` share deliberately does not: it continues semantic work over the cleanly-parsed
//! modules, and with any parse or structural precheck present it yields the diagnostic
//! union over a semantic resource stop. `analyze` therefore takes the diagnostics arm and
//! reads the fact terminal on exactly the input where a semantic body was abandoned.
//!
//! These pin the live half of that projection — that a clean module's facts really are
//! published while another module carries a parse diagnostic — so the custody seam beneath
//! it has a demonstrated consumer rather than an argued one.

use std::sync::Arc;

use marrow_compile::{AnalysisSnapshot, Fact, InputRevision, Unavailability, analyze};
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

fn snap(files: &[(&str, &str)]) -> Arc<AnalysisSnapshot> {
    let Ok(snapshot) = analyze(Arc::new(project(files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot for {files:?}");
    };
    snapshot
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path).expect("canonical identity").0
}

fn offset_of(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle present in source")
}

/// A module that does not parse.
const BROKEN: &str = "module broken\n\npub fn wrong(: int {\n";

/// A module that parses and whose body produces editor facts.
const CLEAN: &str = "module clean\n\npub fn width(side: int): int {\n    var area: int = side\n\
                     \n    return area\n}\n";

/// With a parse diagnostic present in another module, `analyze` still returns a snapshot
/// and that snapshot still carries the cleanly-parsed module's facts.
///
/// This is the exact projection under which a semantic invariant is suppressed: the
/// analysis takes the diagnostics arm and reads the fact terminal. If it stopped reading
/// facts here — the shape that would make an abandoned body's facts structurally
/// unobservable — this fails, and the custody seam beneath it would no longer have a
/// consumer to protect.
#[test]
fn a_parse_failure_in_one_module_does_not_suppress_another_modules_facts() {
    let snapshot = snap(&[("src/broken.mw", BROKEN), ("src/clean.mw", CLEAN)]);

    assert!(
        !snapshot.diagnostics().is_empty(),
        "the broken module contributes a precheck diagnostic, so this fixture really is \
         the precheck-present projection",
    );

    let use_offset = offset_of(CLEAN, "return area") + "return ".len();
    match snapshot.hover(&identity("src/clean.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        Ok(Fact::Absent) => panic!("the clean module's body fact was not retained"),
        Ok(Fact::Unavailable(Unavailability::Syntax)) => {
            panic!("the clean module parsed, so its facts are not syntax-unavailable")
        }
        Ok(Fact::Unavailable(_)) => panic!("the clean module's fact became unavailable"),
        Err(_) => panic!("the clean module is an analyzed file at a valid offset"),
    }
}

/// A position inside the module that did not parse is syntax-unavailable, not absent, and
/// it contributes no fact of its own to the snapshot the clean module's facts reach.
#[test]
fn the_unparsed_modules_positions_are_syntax_unavailable_in_the_same_snapshot() {
    let snapshot = snap(&[("src/broken.mw", BROKEN), ("src/clean.mw", CLEAN)]);
    let at = offset_of(BROKEN, "wrong");
    match snapshot.hover(&identity("src/broken.mw"), at) {
        Ok(Fact::Unavailable(Unavailability::Syntax)) => {}
        Ok(_) => panic!("a position in an unparsed module is syntax-unavailable"),
        Err(_) => panic!("the broken module is still an analyzed file of this project"),
    }
}

/// The clean module's facts are the ones its own bodies produced: adding the broken module
/// beside it changes neither the fact nor its display.
///
/// Order matters to the seam beneath this: a settled body's rows are appended to the
/// ledger at settlement rather than written through as they are produced, so a fixture
/// whose facts moved or duplicated under that change would show here.
#[test]
fn a_clean_modules_facts_are_identical_with_and_without_a_broken_sibling() {
    let alone = snap(&[("src/clean.mw", CLEAN)]);
    let beside = snap(&[("src/broken.mw", BROKEN), ("src/clean.mw", CLEAN)]);
    let at = offset_of(CLEAN, "return area") + "return ".len();
    let file = identity("src/clean.mw");

    let (Ok(Fact::Present(alone)), Ok(Fact::Present(beside))) =
        (alone.hover(&file, at), beside.hover(&file, at))
    else {
        panic!("the clean module answers hover in both projects");
    };
    assert_eq!(
        alone.display(),
        beside.display(),
        "a sibling module's parse failure does not change the clean module's own fact",
    );
}
