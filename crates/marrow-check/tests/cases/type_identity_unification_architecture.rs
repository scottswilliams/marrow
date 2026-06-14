const TYPERULES: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/typerules.rs"));
const TYPES_DOC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/implementation/check/types.md"
));
const CHECKED_PROGRAM_KEYS: &str = include_str!("checked_program_keys.rs");
const ROADMAP: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ROADMAP.md"));

#[test]
fn type_identity_unification_has_no_stale_deferral_contract() {
    assert_absent(
        TYPERULES,
        "permissive until the type IR is unified",
        "typerules still describes cross-module nominal identity as permissive",
    );
    assert_absent(
        TYPES_DOC,
        "documented soundness gap",
        "types documentation still records a closed soundness gap",
    );
    assert_absent(
        TYPES_DOC,
        "permissive until the type IR is unified",
        "types documentation still records the old permissive clause",
    );
    assert_absent(
        CHECKED_PROGRAM_KEYS,
        "cross_module_qualified_identity_splice_defers",
        "checked_program_keys still contains the defer-era regression name",
    );
    assert_absent(
        CHECKED_PROGRAM_KEYS,
        "left to the runtime key guard",
        "checked_program_keys still describes a checker-owned identity rule as runtime-only",
    );
    assert_absent(
        ROADMAP,
        "W5.3 → Type-identity unification",
        "ROADMAP still lists W5.3 as an active lane",
    );
}

fn assert_absent(haystack: &str, needle: &str, message: &str) {
    assert!(!haystack.contains(needle), "{message}: {needle}");
}
