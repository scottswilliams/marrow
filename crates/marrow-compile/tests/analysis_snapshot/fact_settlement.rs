//! One module's parse failure does not suppress another module's editor facts.
//!
//! The production compile refuses a project whose parse stage produced diagnostics before it
//! consults the semantic outcome. The complete-union projection `analyze` and `check` share
//! deliberately does not: it continues semantic work over the cleanly-parsed modules and
//! yields the diagnostic union over a semantic resource stop, so `analyze` takes the
//! diagnostics arm and still reads the fact terminal.

use std::sync::Arc;

use marrow_compile::{AnalysisSnapshot, Fact, InputRevision, Unavailability, analyze};

use super::{identity, project};

/// Analyze a project and unwrap its snapshot; `AnalysisFailure` is deliberately not `Debug`.
fn snap(files: &[(&str, &str)]) -> Arc<AnalysisSnapshot> {
    let Ok(snapshot) = analyze(Arc::new(project(files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot for {files:?}");
    };
    snapshot
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
/// carrying the cleanly-parsed module's facts: the diagnostics arm still reads the fact
/// terminal, which is what keeps an abandoned body's facts observable at all.
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
/// beside it changes neither the fact nor its display. A settled body's rows are appended to
/// the ledger at settlement rather than written through as produced, so fact order is part
/// of what this pins.
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
