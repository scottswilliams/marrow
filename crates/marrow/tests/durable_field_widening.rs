//! D00 slice 3c: durable-field value widening + enum member identity.
//!
//! A durable field's stored value is drawn from the closed acyclic durable value
//! set: a nominal scalar (erased to its base scalar), a dense `struct` (its leaves
//! recorded positionally as shape bytes, minting no per-leaf id), a closed `enum`
//! (`Option`/`Result`/a user `enum`), or an `Option` of one. The field anchors the
//! ledger id; a durable-reachable enum additionally carries a sum identity (kind 5)
//! and one member identity (kind 6) per variant, so append-only member evolution has
//! stable per-member codes. A resource with any widened (non-plain-scalar) field
//! completes its identity and verifies but is not yet executable — an operation over
//! it is a precise `check.unsupported`, never a silent drop, and never the pre-slice
//! `check.type` on the declaration.

use marrow_compile::{Compiled, SourceDiagnostic};
use marrow_verify::DurableContractId;

fn compile(source: &str, ids: &str) -> Result<Compiled, Vec<SourceDiagnostic>> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    marrow_compile::compile(&project)
}

fn contract_of(source: &str, ids: &str) -> DurableContractId {
    let compiled = compile(source, ids).expect("compile");
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    image.durable_contract()
}

fn codes(diagnostics: &[SourceDiagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|d| d.code).collect()
}

// A resource storing every widened value kind: a plain scalar (`id`), a user enum
// (`kind`), a nominal scalar (`balance`), a dense struct (`owner`), and an
// `Option` (`note`).
const ACCOUNT_SOURCE: &str = "resource Account\n\
     \x20   required id: int\n\
     \x20   required kind: Access\n\
     \x20   balance: Money\n\
     \x20   owner: Name\n\
     \x20   note: Option[string]\n\
     \n\
     struct Name\n\
     \x20   first: string\n\
     \x20   last: string\n\
     \n\
     enum Access\n\
     \x20   reader\n\
     \x20   writer\n\
     \x20   admin\n\
     \n\
     type Money: int in 0..=1000000\n\
     \n\
     store ^accounts(id: int): Account\n\
     \n\
     pub fn label(): string\n\
     \x20   return \"accounts\"\n";

// The full ledger. Note the struct `Name`'s leaves (`first`/`last`) mint no ids —
// they are shape bytes — while each durable-reachable enum carries a sum id and one
// member id per variant, anchored at its canonical (space-free) spelling.
const ACCOUNT_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Account 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id root accounts 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key accounts.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id field Account.id 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Account.kind 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id field Account.balance 10101010101010101010101010101010\n\
     id field Account.owner 11111111111111111111111111111111\n\
     id field Account.note 12121212121212121212121212121212\n\
     id sum Access 50505050505050505050505050505050\n\
     id member Access.reader 51515151515151515151515151515151\n\
     id member Access.writer 52525252525252525252525252525252\n\
     id member Access.admin 53535353535353535353535353535353\n\
     id sum Option[string] 60606060606060606060606060606060\n\
     id member Option[string].none 61616161616161616161616161616161\n\
     id member Option[string].some 62626262626262626262626262626262\n\
     high-water 0\n\
     end\n";

#[test]
fn a_widened_field_resource_completes_its_identity_and_verifies() {
    let id = contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS);
    assert_eq!(id, contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS), "stable");
}

/// The enum sum/member ids (and every other kind) cannot drift under an unrelated
/// edit: adding unrelated storeless code and reordering declarations leaves the
/// widened graph's contract id — computed over the sum/member tree — unchanged.
#[test]
fn unrelated_source_edits_do_not_drift_the_widened_contract_id() {
    let base = contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS);

    let appended = format!("{ACCOUNT_SOURCE}\npub fn unrelated(n: int): int\n    return n + 1\n");
    assert_eq!(
        base,
        contract_of(&appended, ACCOUNT_IDS),
        "unrelated storeless code does not drift the enum sum/member identity"
    );

    let reordered = format!("pub fn unrelated(n: int): int\n    return n + 1\n\n{ACCOUNT_SOURCE}");
    assert_eq!(
        base,
        contract_of(&reordered, ACCOUNT_IDS),
        "declaration order does not drift the widened durable identity"
    );
}

#[test]
fn a_missing_enum_sum_identity_fails_precisely() {
    let without_sum = ACCOUNT_IDS.replace("id sum Access 50505050505050505050505050505050\n", "");
    let diagnostics = compile(ACCOUNT_SOURCE, &without_sum).expect_err("incomplete identity");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("sum `Access`")),
        "the gap names the enum sum anchor: {diagnostics:?}"
    );
}

#[test]
fn a_missing_enum_member_identity_fails_precisely() {
    let without_member = ACCOUNT_IDS.replace(
        "id member Access.writer 52525252525252525252525252525252\n",
        "",
    );
    let diagnostics = compile(ACCOUNT_SOURCE, &without_member).expect_err("incomplete identity");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("member `Access.writer`")),
        "the gap names the enum member anchor: {diagnostics:?}"
    );
}

#[test]
fn an_option_field_mints_its_generic_enum_sum_and_members() {
    // The `Option[string]` reachable through the store carries its own sum/member
    // identities anchored at its space-free spelling.
    let without_option_sum = ACCOUNT_IDS.replace(
        "id sum Option[string] 60606060606060606060606060606060\n",
        "",
    );
    let diagnostics =
        compile(ACCOUNT_SOURCE, &without_option_sum).expect_err("incomplete identity");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("sum `Option[string]`")),
        "{diagnostics:?}"
    );
}

// --- Enum member evolution: rename preserves, append changes and cannot reuse. ---

#[test]
fn renaming_an_enum_member_with_a_moved_anchor_preserves_the_identity() {
    // A rename edits the source member and moves its ledger anchor while keeping the
    // same id: identity follows the id, not the spelling.
    let renamed_source = ACCOUNT_SOURCE.replace("\x20   reader\n", "\x20   viewer\n");
    let renamed_ids = ACCOUNT_IDS.replace("member Access.reader", "member Access.viewer");
    assert_eq!(
        contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS),
        contract_of(renamed_source.as_str(), renamed_ids.as_str()),
        "an enum member rename whose anchor moved (id unchanged) preserves the identity"
    );

    // Re-minting the member id (a delete-then-re-add) is a different graph.
    let re_minted = ACCOUNT_IDS.replace(
        "id member Access.reader 51515151515151515151515151515151\n",
        "id member Access.reader 71717171717171717171717171717171\n",
    );
    assert_ne!(
        contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS),
        contract_of(ACCOUNT_SOURCE, re_minted.as_str()),
        "a re-minted member id changes the identity"
    );
}

#[test]
fn appending_an_enum_member_changes_the_identity_and_mints_a_fresh_id() {
    // Append a fourth variant to `Access`. A fresh member id is required; the
    // existing members keep their ids and positions.
    let appended_source =
        ACCOUNT_SOURCE.replace("\x20   admin\n", "\x20   admin\n\x20   auditor\n");

    // Without the fresh member id the append fails precisely (append cannot reuse a
    // sibling's identity — every anchor needs its own row).
    let diagnostics = compile(appended_source.as_str(), ACCOUNT_IDS).expect_err("needs a fresh id");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("member `Access.auditor`")),
        "{diagnostics:?}"
    );

    // With a fresh id the appended enum verifies and its identity changed, while the
    // pre-existing member ids are untouched.
    let appended_ids = ACCOUNT_IDS.replace(
        "id member Access.admin 53535353535353535353535353535353\n",
        "id member Access.admin 53535353535353535353535353535353\n\
         id member Access.auditor 54545454545454545454545454545454\n",
    );
    assert_ne!(
        contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS),
        contract_of(appended_source.as_str(), appended_ids.as_str()),
        "appending a member changes the durable identity"
    );
}

// --- The executable-vs-identity boundary. ---

#[test]
fn operating_on_a_widened_field_store_is_not_yet_executable() {
    let source = "resource Account\n\
         \x20   required id: int\n\
         \x20   required kind: Access\n\
         \n\
         enum Access\n\
         \x20   reader\n\
         \x20   writer\n\
         \n\
         store ^accounts(id: int): Account\n\
         \n\
         pub fn kind(id: int): Access?\n\
         \x20   return ^accounts(id).kind\n";
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Account 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id root accounts 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key accounts.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         id field Account.id 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id field Account.kind 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
         id sum Access 50505050505050505050505050505050\n\
         id member Access.reader 51515151515151515151515151515151\n\
         id member Access.writer 52525252525252525252525252525252\n\
         high-water 0\n\
         end\n";
    let diagnostics = compile(source, ids).expect_err("not yet executable");
    assert!(
        codes(&diagnostics).contains(&"check.unsupported"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("not yet executable")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_cyclic_value_graph_through_a_durable_field_is_rejected() {
    // A durable field's value must be acyclic. A self-referential struct reached
    // through a stored field is an infinite value, rejected at check time as a
    // recursion (and independently re-rejected by the verifier's value-type cycle
    // pass), so it never enters the durable graph.
    let source = "resource Tree\n\
         \x20   required id: int\n\
         \x20   node: Node\n\
         \n\
         struct Node\n\
         \x20   child: Node\n\
         \n\
         store ^trees(id: int): Tree\n\
         \n\
         pub fn label(): string\n\
         \x20   return \"trees\"\n";
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         high-water 0\n\
         end\n";
    let diagnostics = compile(source, ids).expect_err("cyclic value graph");
    assert!(
        codes(&diagnostics).contains(&"check.recursion"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_collection_durable_field_is_unsupported() {
    // A collection is not a durable value leaf (a large collection belongs under a
    // keyed branch), so a resource field storing one is a precise `check.unsupported`
    // rather than being admitted into the durable graph.
    let source = "resource Bag\n\
         \x20   required id: int\n\
         \x20   items: List[int]\n\
         \n\
         store ^bags(id: int): Bag\n\
         \n\
         pub fn label(): string\n\
         \x20   return \"bags\"\n";
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         high-water 0\n\
         end\n";
    let diagnostics = compile(source, ids).expect_err("unsupported collection field");
    assert!(
        codes(&diagnostics).contains(&"check.unsupported"),
        "{diagnostics:?}"
    );
}
