//! Durable-graph breadth: singleton roots and multi-column key tuples.
//!
//! A `store` root is either a singleton (no key) or a keyed tuple of one or more
//! ordered key columns. Every such root is a distinct graph node with a complete
//! ledger identity (its placement, its product, one identity per key column, and
//! one per stored field), a slot in the image DURABLE table, and a contribution
//! to the durable-contract identity the verifier independently re-encodes. The
//! wider runtime (multi-column keys, singleton entry addressing) does not execute
//! yet: these shapes compile, verify, and complete their identity, but an operation
//! over a shape the single-root kernel cannot serve is a precise typed
//! `check.unsupported` rejection rather than a silent drop.

use crate::common::{Diagnostics, Project};
use marrow_verify::DurableContractId;

/// Compile and independently verify, returning the durable-contract identity.
fn contract_of(source: &str, ids: &str) -> DurableContractId {
    Project::single(source).ids(ids).image().durable_contract()
}

/// The typed rejection diagnostics for a project that must not compile; `why` names the
/// defect the fixture carries.
fn rejection(source: &str, ids: &str, why: &str) -> Diagnostics {
    match Project::single(source).ids(ids).try_image() {
        Ok(_) => panic!("expected a rejection: {why}"),
        Err(diagnostics) => diagnostics,
    }
}

// --- Singleton roots: `store ^name: Resource` with no key column. ---

const SETTINGS_SOURCE: &str = r#"resource Settings {
    required locale: string
}

store ^settings: Settings

pub fn label(): string {
    return "settings"
}
"#;

/// A singleton root's ledger carries no key anchor: application, product, one
/// field, and the placement, but no `key` row.
const SETTINGS_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Settings 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Settings.locale 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root settings 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     high-water 0\n\
     end\n";

#[test]
fn a_singleton_root_compiles_and_completes_its_identity() {
    // Declaration-only: the singleton root is identity-complete and verifies.
    let id = contract_of(SETTINGS_SOURCE, SETTINGS_IDS);
    // Stable across recompilation.
    assert_eq!(id, contract_of(SETTINGS_SOURCE, SETTINGS_IDS));
    // Distinct from a keyed graph.
    assert_ne!(id, contract_of(ENROLLMENTS_SOURCE, ENROLLMENTS_IDS));
}

#[test]
fn a_singleton_root_missing_its_placement_identity_fails_precisely() {
    let without_root =
        SETTINGS_IDS.replace("id root settings 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n", "");
    let diagnostics = rejection(SETTINGS_SOURCE, &without_root, "incomplete identity");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
}

// --- Multi-column key tuples: `store ^name(k1: K1, k2: K2): Resource`. ---

const ENROLLMENTS_SOURCE: &str = r#"resource Enrollment {
    required grade: int
}

store ^enrollments[student: string, course: string]: Enrollment

pub fn label(): string {
    return "enrollments"
}
"#;

const ENROLLMENTS_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Enrollment 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Enrollment.grade 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root enrollments 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key enrollments.student 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id key enrollments.course 02020202020202020202020202020202\n\
     high-water 0\n\
     end\n";

#[test]
fn a_composite_key_root_compiles_and_completes_its_identity() {
    let id = contract_of(ENROLLMENTS_SOURCE, ENROLLMENTS_IDS);
    assert_eq!(id, contract_of(ENROLLMENTS_SOURCE, ENROLLMENTS_IDS));
}

#[test]
fn a_composite_key_root_missing_one_key_identity_fails_precisely() {
    let without_course = ENROLLMENTS_IDS.replace(
        "id key enrollments.course 02020202020202020202020202020202\n",
        "",
    );
    let diagnostics = rejection(ENROLLMENTS_SOURCE, &without_course, "incomplete identity");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
}

#[test]
fn key_column_order_is_part_of_the_durable_identity() {
    let base = contract_of(ENROLLMENTS_SOURCE, ENROLLMENTS_IDS);

    // Swap the two key columns in source (and move each anchor with its name):
    // the identity depends on column order, so this is a different graph.
    let swapped_source = ENROLLMENTS_SOURCE.replace(
        "[student: string, course: string]",
        "[course: string, student: string]",
    );
    assert_ne!(
        base,
        contract_of(swapped_source.as_str(), ENROLLMENTS_IDS),
        "reordering key columns changes the durable identity"
    );
}

#[test]
fn renaming_a_key_column_with_a_moved_anchor_preserves_the_identity() {
    let base = contract_of(ENROLLMENTS_SOURCE, ENROLLMENTS_IDS);

    // Rename `course` → `class`, moving the ledger anchor while its id stays.
    let renamed_source = ENROLLMENTS_SOURCE.replace("course: string", "class: string");
    let renamed_ids = ENROLLMENTS_IDS.replace("enrollments.course", "enrollments.class");
    assert_eq!(
        base,
        contract_of(renamed_source.as_str(), renamed_ids.as_str()),
        "a key rename whose anchor moved (id unchanged) preserves the identity"
    );

    // A re-minted key id at the same column is a different graph.
    let re_minted = renamed_ids.replace(
        "02020202020202020202020202020202",
        "12121212121212121212121212121212",
    );
    assert_ne!(
        base,
        contract_of(renamed_source.as_str(), re_minted.as_str()),
        "a fresh key id is a different durable identity"
    );
}

// --- The executable-vs-identity boundary: operations over shapes the single-root
// kernel cannot yet serve are a precise typed rejection, not a silent drop. A
// composite-key root is executable (see `durable_composite_keys`); a singleton root
// parks. ---

#[test]
fn operating_on_a_singleton_root_is_not_yet_executable() {
    let source = r#"resource Settings {
    required locale: string
}

store ^settings: Settings

pub fn locale(): string? {
    return ^settings.locale
}
"#;
    let diagnostics = rejection(source, SETTINGS_IDS, "not yet executable");
    assert!(
        diagnostics.has_code("check.unsupported"),
        "{:?}",
        diagnostics.all()
    );
}

// --- The single-column keyed root remains executable end to end. ---

const COUNTER_SOURCE: &str = r#"resource Counter {
    required value: int
}

store ^counters[name: string]: Counter

pub fn get(name: string): int? {
    return ^counters[name].value
}
"#;

const COUNTER_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.name 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// A single-column keyed root with a missing field identity fails with the
/// precise identity gap — never mislabelled "not yet executable" (the single-key
/// shape *is* executable; it only lacks a ledger identity, which the gap names).
#[test]
fn a_single_key_root_with_a_missing_identity_reports_the_gap_not_executability() {
    let source = r#"resource Counter {
    required value: int
}

store ^counters[name: string]: Counter

pub fn get(name: string): int? {
    return ^counters[name].value
}
"#;
    let without_field = COUNTER_IDS.replace(
        "id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n",
        "",
    );
    let diagnostics = rejection(source, &without_field, "incomplete identity");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
    assert!(
        !diagnostics.has_code("check.unsupported"),
        "a single-key root must not be mislabelled not-yet-executable: {:?}",
        diagnostics.all()
    );
}

#[test]
fn a_single_column_keyed_root_still_compiles_and_verifies() {
    // The one kernel-serviceable shape: it both completes its identity and lowers
    // an executable read.
    let _ = contract_of(COUNTER_SOURCE, COUNTER_IDS);
    let _ = Project::single(COUNTER_SOURCE).ids(COUNTER_IDS).image();
}
