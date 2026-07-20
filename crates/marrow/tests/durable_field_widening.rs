//! D00 slice 3c: durable-field value widening + enum member identity.
//!
//! A durable field's stored value is drawn from the closed acyclic durable value
//! set: a nominal scalar (erased to its base scalar), a dense `struct` (its leaves
//! recorded positionally as shape bytes, minting no per-leaf id), a closed `enum`
//! (`Option`/`Result`/a user `enum`), or an `Option` of one. The field anchors the
//! ledger id; a durable-reachable enum additionally carries a sum identity (kind 5)
//! and one member identity (kind 6) per variant, so append-only member evolution has
//! stable per-member codes. A resource with a widened (struct/enum/`Option`) field
//! completes its identity, verifies, and is executable — the durable value codec frames
//! the composite inline in the one field-leaf cell (end-to-end store/read coverage lives
//! in `durable_widened_values.rs`). A nominal field is admitted and uses its base `int`
//! shape while operations over its root remain parked; a collection field is a precise
//! `check.unsupported`.

use marrow_compile::{Compiled, SourceDiagnostic};
use marrow_image::{ImageType, Scalar};
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
    match marrow_compile::compile(&project) {
        Ok(compiled) => Ok(compiled),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => {
            Err(diagnostics.into_vec())
        }
        Err(marrow_compile::CompileFailure::Invariant(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
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
const ACCOUNT_SOURCE: &str = r#"resource Account {
    required id: int
    required kind: Access
    balance: Money
    owner: Name
    note: Option<string>
}

struct Name {
    first: string
    last: string
}

enum Access {
    reader
    writer
    admin
}

type Money: int in 0..=1000000

store ^accounts[id: int]: Account

pub fn label(): string {
    return "accounts"
}
"#;

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

const NOMINAL_BRANCH_SOURCE: &str = r#"type Money: int in 0..=1000000

resource Ledger {
    required label: string

    entries[amount: Money] {
        required note: string
    }
}

store ^ledgers[id: int]: Ledger

pub fn label(): string {
    return "ledgers"
}
"#;

const NOMINAL_BRANCH_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 70707070707070707070707070707070\n\
     id product Ledger 71717171717171717171717171717171\n\
     id root ledgers 72727272727272727272727272727272\n\
     id key ledgers.id 73737373737373737373737373737373\n\
     id field Ledger.label 74747474747474747474747474747474\n\
     id root Ledger.entries 75757575757575757575757575757575\n\
     id field Ledger.entries.note 76767676767676767676767676767676\n\
     high-water 0\n\
     end\n";

#[test]
fn a_widened_field_resource_completes_its_identity_and_verifies() {
    let id = contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS);
    assert_eq!(id, contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS), "stable");
}

#[test]
fn nominal_durable_positions_and_reference_agree() {
    let compiled = compile(ACCOUNT_SOURCE, ACCOUNT_IDS).expect("nominal field admission");
    let image = marrow_verify::verify(&compiled.image.bytes).expect("independent verification");
    let root = image
        .roots()
        .iter()
        .find(|root| root.name() == "accounts")
        .expect("accounts root");
    let balance = image
        .record_type(root.record())
        .fields()
        .iter()
        .find(|field| field.name.as_ref() == "balance")
        .expect("balance field");
    assert_eq!(balance.ty, ImageType::scalar(Scalar::Int));
    assert!(!balance.required, "balance remains sparse");

    let nominal_root_key = ACCOUNT_SOURCE.replace(
        "store ^accounts[id: int]: Account",
        "store ^accounts[id: Money]: Account",
    );
    let diagnostics =
        compile(&nominal_root_key, ACCOUNT_IDS).expect_err("nominal root key must fail");
    assert_eq!(codes(&diagnostics), vec!["check.unsupported"]);

    let diagnostics = compile(NOMINAL_BRANCH_SOURCE, NOMINAL_BRANCH_IDS)
        .expect_err("nominal branch key must fail");
    assert_eq!(codes(&diagnostics), vec!["check.unsupported"]);

    let nominal_constant = ACCOUNT_SOURCE.replace(
        "store ^accounts[id: int]: Account",
        "const LIMIT: Money = 1\n\nstore ^accounts[id: int]: Account",
    );
    let diagnostics =
        compile(&nominal_constant, ACCOUNT_IDS).expect_err("nominal constant must fail");
    assert_eq!(codes(&diagnostics), vec!["check.unsupported"]);

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let types = normalize(include_str!("../../../docs/language/types-and-values.md"));
    let durable = normalize(include_str!("../../../docs/language/durable-places.md"));

    assert!(types.contains(
        "A nominal int type is admitted as a resource field. It retains its nominal source type, while its image record and durable stored shape use base `int`; sparse requiredness is recorded separately."
    ));
    assert!(types.contains(
        "Operations over a root containing a nominal field remain unimplemented and report `check.unsupported`."
    ));
    assert!(types.contains(
        "Nominal types are not admitted as store-root keys, branch keys, or module-constant types; each position reports `check.unsupported`."
    ));
    assert!(durable.contains(
        "Nominal source types are not durable identity keys and report `check.unsupported`."
    ));
    assert!(!types.contains("Nominal types are not yet admitted as resource field types"));
    assert!(
        !types.contains(
            "A nominal-typed stored field reports `check.unsupported` until its lane lands"
        )
    );
    assert!(
        !durable.contains("(a nominal type over one of these is admitted through its base scalar)")
    );
    assert!(types.contains(
        "A nominal Map key retains its source type and uses its base scalar for representation and ordering."
    ));
    assert!(types.contains("`duration` and nominal source types are not durable keys."));
    assert!(types.contains("A nominal stored field projects through its base scalar."));

    let test_source = include_str!("durable_field_widening.rs");
    let disallowed = [
        ["ro", "w"].concat(),
        ["ro", "ws"].concat(),
        ["col", "umn"].concat(),
        ["col", "umns"].concat(),
        ["ta", "ble"].concat(),
        ["ta", "bles"].concat(),
    ];
    for (name, text) in [
        ("types and values", types.as_str()),
        ("durable places", durable.as_str()),
        ("durable field widening", test_source),
    ] {
        let found = text
            .split(|ch: char| !ch.is_ascii_alphabetic())
            .filter(|word| {
                !word.is_empty()
                    && disallowed
                        .iter()
                        .any(|term| word.eq_ignore_ascii_case(term))
            })
            .collect::<Vec<_>>();
        assert!(found.is_empty(), "{name}: {found:?}");
    }
}

/// The enum sum/member ids (and every other kind) cannot drift under an unrelated
/// edit: adding unrelated storeless code and reordering declarations leaves the
/// widened graph's contract id — computed over the sum/member tree — unchanged.
#[test]
fn unrelated_source_edits_do_not_drift_the_widened_contract_id() {
    let base = contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS);

    let appended =
        format!("{ACCOUNT_SOURCE}\npub fn unrelated(n: int): int {{\n    return n + 1\n}}\n");
    assert_eq!(
        base,
        contract_of(&appended, ACCOUNT_IDS),
        "unrelated storeless code does not drift the enum sum/member identity"
    );

    let reordered =
        format!("pub fn unrelated(n: int): int {{\n    return n + 1\n}}\n\n{ACCOUNT_SOURCE}");
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
    // sibling's identity — every anchor needs its own ledger entry).
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
fn operating_on_a_widened_field_store_compiles_and_verifies() {
    // A read of a widened (enum) field is executable: it compiles, and the sealed image
    // verifies with a durable read opcode over the field-leaf site (no longer parked).
    let source = r#"resource Account {
    required id: int
    required kind: Access
}

enum Access {
    reader
    writer
}

store ^accounts[id: int]: Account

pub fn kind(id: int): Access? {
    return ^accounts[id].kind
}
"#;
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
    let compiled = compile(source, ids).expect("a widened-field read compiles");
    marrow_verify::verify(&compiled.image.bytes).expect("the image verifies with the read opcode");
}

#[test]
fn a_cyclic_value_graph_through_a_durable_field_is_rejected() {
    // A durable field's value must be acyclic. A self-referential struct reached
    // through a stored field is an infinite value, rejected at check time as a
    // recursion (and independently re-rejected by the verifier's value-type cycle
    // pass), so it never enters the durable graph.
    let source = r#"resource Tree {
    required id: int
    node: Node
}

struct Node {
    child: Node
}

store ^trees[id: int]: Tree

pub fn label(): string {
    return "trees"
}
"#;
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
fn an_index_over_a_widened_field_is_refused() {
    // Index eligibility is decoupled from executability: a widened (struct) field is
    // executable but is not an orderable durable-key scalar, so declaring an index over
    // it is a precise `check.type` — mirroring the verifier's independent refusal.
    let source = r#"resource Account {
    required id: int
    owner: Name
}

struct Name {
    first: string
    last: string
}

store ^accounts[id: int]: Account {
    index byOwner[owner] unique
}

pub fn label(): string {
    return "accounts"
}
"#;
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Account 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id root accounts 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key accounts.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         id field Account.id 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id field Account.owner 11111111111111111111111111111111\n\
         id index accounts.byOwner 70707070707070707070707070707070\n\
         high-water 0\n\
         end\n";
    let diagnostics = compile(source, ids).expect_err("an index over a widened field is refused");
    assert!(
        codes(&diagnostics).contains(&"check.type"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_collection_durable_field_is_unsupported() {
    // A collection is not a durable value leaf (a large collection belongs under a
    // keyed branch), so a resource field storing one is a precise `check.unsupported`
    // rather than being admitted into the durable graph.
    let source = r#"resource Bag {
    required id: int
    items: List<int>
}

store ^bags[id: int]: Bag

pub fn label(): string {
    return "bags"
}
"#;
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

// --- Nested multi-argument generic enum anchor (KAT). ---

// A resource storing a nested, multi-argument generic enum: `Result<Option<int>,
// string>`. The checker prints this type in the canonical angle form in diagnostics,
// but its *durable* identity is the opaque space-free bracket spelling
// `Result[Option[int],string]` — note the comma carries no space, so the anchor is a
// valid `marrow.ids` path. The `Option<int>` reached through the `ok` payload is
// itself durable-reachable and carries its own `Option[int]` sum/member anchors.
// These bracket bytes are a frozen ledger contract, deliberately independent of the
// display spelling; this KAT pins them through compile + independent verify.
const OUTCOME_SOURCE: &str = r#"resource Outcome {
    required id: int
    required result: Result<Option<int>, string>
}

store ^outcomes[id: int]: Outcome

pub fn label(): string {
    return "outcomes"
}
"#;

const OUTCOME_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Outcome 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id root outcomes 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key outcomes.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id field Outcome.id 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Outcome.result 1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f\n\
     id sum Result[Option[int],string] 80808080808080808080808080808080\n\
     id member Result[Option[int],string].ok 81818181818181818181818181818181\n\
     id member Result[Option[int],string].err 82828282828282828282828282828282\n\
     id sum Option[int] 90909090909090909090909090909090\n\
     id member Option[int].none 91919191919191919191919191919191\n\
     id member Option[int].some 92929292929292929292929292929292\n\
     high-water 0\n\
     end\n";

#[test]
fn a_nested_multi_arg_generic_enum_field_pins_its_bracket_anchors() {
    // Completing identity and verifying the sealed image *requires* the compiler to
    // ask for exactly the space-free bracket anchors named in `OUTCOME_IDS`. Any
    // other spelling (angle form, or a comma with a space) would leave a gap and fail
    // the compile, so a successful, stable durable contract pins the anchor bytes,
    // including the multi-argument comma, through compile + independent verify.
    let id = contract_of(OUTCOME_SOURCE, OUTCOME_IDS);
    assert_eq!(id, contract_of(OUTCOME_SOURCE, OUTCOME_IDS), "stable");
}

#[test]
fn a_missing_nested_result_sum_reports_the_space_free_bracket_anchor() {
    // `identity_gap` names the opaque durable anchor, not the angle-form display
    // spelling: the multi-argument sum reports `Result[Option[int],string]` with the
    // comma carrying no space.
    let without_sum = OUTCOME_IDS.replace(
        "id sum Result[Option[int],string] 80808080808080808080808080808080\n",
        "",
    );
    let diagnostics = compile(OUTCOME_SOURCE, &without_sum).expect_err("incomplete identity");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("sum `Result[Option[int],string]`")),
        "the gap names the multi-argument enum sum anchor with a space-free comma: {diagnostics:?}"
    );
}

#[test]
fn a_missing_nested_result_member_reports_the_space_free_bracket_anchor() {
    let without_member = OUTCOME_IDS.replace(
        "id member Result[Option[int],string].ok 81818181818181818181818181818181\n",
        "",
    );
    let diagnostics = compile(OUTCOME_SOURCE, &without_member).expect_err("incomplete identity");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("member `Result[Option[int],string].ok`")),
        "the gap names the multi-argument enum member anchor: {diagnostics:?}"
    );
}

#[test]
fn the_nested_option_reached_through_the_result_mints_its_own_anchor() {
    // The `Option<int>` reached through the `ok` payload is durable-reachable and
    // anchored at its own space-free bracket spelling `Option[int]`.
    let without_option_sum =
        OUTCOME_IDS.replace("id sum Option[int] 90909090909090909090909090909090\n", "");
    let diagnostics =
        compile(OUTCOME_SOURCE, &without_option_sum).expect_err("incomplete identity");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("sum `Option[int]`")),
        "{diagnostics:?}"
    );
}
