//! Durable-field value widening and enum member identity.
//!
//! A durable field's stored value is drawn from the closed acyclic durable value
//! set: a scalar, a dense `struct` (its leaves
//! carry their declared names in the contract, minting no per-leaf id), a closed `enum`
//! (`Option`/`Result`/a user `enum`), or an `Option` of one. The field anchors the
//! ledger id; a durable-reachable enum additionally carries a sum identity (kind 5)
//! and one member identity (kind 6) per variant, so append-only member evolution has
//! stable per-member codes. A resource with a widened (struct/enum/`Option`) field
//! completes its identity, verifies, and is executable — the durable value codec frames
//! the composite inline in the one field-leaf cell (end-to-end store/read coverage lives
//! in `durable_widened_values.rs`). Nominal-bearing store bindings and collection
//! fields report `check.unsupported`.

use marrow_image::{
    CanonicalValueShapeDag, DurableMemberViewKind, ImageType, Scalar, ValueShapeNodeId,
    ValueShapeView,
};
use marrow_verify::{DurableContractId, VerifiedImage};

use crate::common::{Diagnostics, Project};

/// The project for `source` against the identity ledger `ids`.
fn project(source: &str, ids: &str) -> Project {
    Project::single(source).ids(ids)
}

/// The diagnostics a source the checker must reject reports.
fn errors(source: &str, ids: &str) -> Diagnostics {
    let Err(diagnostics) = project(source, ids).try_image() else {
        panic!("the checker must reject this program");
    };
    diagnostics
}

/// The durable contract identity of a graph that compiles and verifies.
fn contract_of(source: &str, ids: &str) -> DurableContractId {
    project(source, ids).image().durable_contract()
}

// A resource storing supported widened values: plain scalars (`id`/`balance`), a
// user enum (`kind`), a dense struct (`owner`), and an
// `Option` (`note`).
const ACCOUNT_SOURCE: &str = r#"resource Account {
    required id: int
    required kind: Access
    balance: int
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

store ^accounts[id: int]: Account

pub fn label(): string {
    return "accounts"
}
"#;

// The full ledger. Note the struct `Name`'s leaves (`first`/`last`) mint no ids —
// their declared names are part of the value shape — while each durable-reachable enum
// carries a sum id and one member id per variant, anchored at its canonical
// (space-free) spelling.
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
    let image = project(ACCOUNT_SOURCE, ACCOUNT_IDS).image();
    let root = image
        .roots()
        .iter()
        .find(|root| root.name() == "accounts")
        .expect("accounts root");
    let balance = image
        .record_type(root.record())
        .fields()
        .iter()
        .find(|field| field.name().as_ref() == "balance")
        .expect("balance field");
    assert_eq!(balance.ty(), ImageType::scalar(Scalar::Int));
    assert!(!balance.required(), "balance remains sparse");
    assert_eq!(
        image.durable_contract(),
        contract_of(ACCOUNT_SOURCE, ACCOUNT_IDS),
        "stable"
    );
}

#[test]
fn a_nominal_field_binding_is_refused_without_a_durable_operation() {
    let source = format!(
        "type Money: int in 0..=1000000\n\n{}",
        ACCOUNT_SOURCE.replace("balance: int", "balance: Money")
    );
    let diagnostics = errors(&source, ACCOUNT_IDS);
    let sites: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| {
            let span = diagnostic.span();
            (diagnostic.code().as_str(), span.start_byte, span.end_byte)
        })
        .collect();
    assert_eq!(
        sites,
        [("check.unsupported", 262, 295)],
        "{:?}",
        diagnostics.all()
    );
    assert_eq!(&source[262..295], "store ^accounts[id: int]: Account");
}

#[test]
fn nominal_durable_positions_and_reference_agree() {
    let source = format!("type Money: int in 0..=1000000\n\n{ACCOUNT_SOURCE}");

    let nominal_root_key = source.replace(
        "store ^accounts[id: int]: Account",
        "store ^accounts[id: Money]: Account",
    );
    let diagnostics = errors(&nominal_root_key, ACCOUNT_IDS);
    assert_eq!(diagnostics.codes(), vec!["check.unsupported"]);

    let diagnostics = errors(NOMINAL_BRANCH_SOURCE, NOMINAL_BRANCH_IDS);
    assert_eq!(diagnostics.codes(), vec!["check.unsupported"]);

    let nominal_constant = source.replace(
        "store ^accounts[id: int]: Account",
        "const LIMIT: Money = 1\n\nstore ^accounts[id: int]: Account",
    );
    let diagnostics = errors(&nominal_constant, ACCOUNT_IDS);
    assert_eq!(diagnostics.codes(), vec!["check.unsupported"]);

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let types = normalize(include_str!(
        "../../../../docs/language/types-and-values.md"
    ));
    let durable = normalize(include_str!("../../../../docs/language/durable-data.md"));

    assert!(types.contains("A nominal int type is admitted as a local resource field."));
    assert!(types.contains(
        "Binding a resource containing a nominal value to a store reports `check.unsupported`, including nested and sparse fields and bindings with no durable operations."
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
    assert!(!types.contains("A nominal stored field projects through its base scalar."));

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
        ("durable paths", durable.as_str()),
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
    let diagnostics = errors(ACCOUNT_SOURCE, &without_sum);
    assert!(
        diagnostics.codes().contains(&"check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics
            .messages()
            .iter()
            .any(|message| message.contains("sum `Access`")),
        "the gap names the enum sum anchor: {:?}",
        diagnostics.all()
    );
}

#[test]
fn a_missing_enum_member_identity_fails_precisely() {
    let without_member = ACCOUNT_IDS.replace(
        "id member Access.writer 52525252525252525252525252525252\n",
        "",
    );
    let diagnostics = errors(ACCOUNT_SOURCE, &without_member);
    assert!(
        diagnostics.codes().contains(&"check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics
            .messages()
            .iter()
            .any(|message| message.contains("member `Access.writer`")),
        "the gap names the enum member anchor: {:?}",
        diagnostics.all()
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
    let diagnostics = errors(ACCOUNT_SOURCE, &without_option_sum);
    assert!(
        diagnostics
            .messages()
            .iter()
            .any(|message| message.contains("sum `Option[string]`")),
        "{:?}",
        diagnostics.all()
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
    let diagnostics = errors(appended_source.as_str(), ACCOUNT_IDS);
    assert!(
        diagnostics
            .messages()
            .iter()
            .any(|message| message.contains("member `Access.auditor`")),
        "{:?}",
        diagnostics.all()
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
    // verifies with a durable read opcode over the field-leaf site.
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
    // `.image()` compiles and independently verifies, so the durable read opcode over
    // the field-leaf site survives the verifier.
    project(source, ids).image();
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
    let diagnostics = errors(source, ids);
    assert!(
        diagnostics.codes().contains(&"check.recursion"),
        "{:?}",
        diagnostics.all()
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
    let diagnostics = errors(source, ids);
    assert!(
        diagnostics.codes().contains(&"check.type"),
        "{:?}",
        diagnostics.all()
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
    let diagnostics = errors(source, ids);
    assert!(
        diagnostics.codes().contains(&"check.unsupported"),
        "{:?}",
        diagnostics.all()
    );
}

// --- Nested multi-argument generic enum anchor (KAT). ---

// A resource storing a nested, multi-argument generic enum: `Result<Option<int>,
// string>`. The checker prints this type in the canonical angle form in diagnostics,
// but its *durable* identity is the opaque space-free bracket spelling
// `Result[Option[int],string]` — note the comma carries no space, so the anchor is a
// valid `.marrow/ids` path. The `Option<int>` reached through the `ok` payload is
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
    let with_read = format!(
        "{OUTCOME_SOURCE}{}",
        r#"
pub fn resultOf(id: int): Result<Option<int>, string> {
    ref p = ^outcomes[id] else { return err("missing") }
    return p.result
}
"#
    );
    assert_eq!(
        id,
        contract_of(&with_read, OUTCOME_IDS),
        "a required read preserves the durable contract"
    );
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
    let diagnostics = errors(OUTCOME_SOURCE, &without_sum);
    assert!(
        diagnostics.codes().contains(&"check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics
            .messages()
            .iter()
            .any(|message| message.contains("sum `Result[Option[int],string]`")),
        "the gap names the multi-argument enum sum anchor with a space-free comma: {:?}",
        diagnostics.all()
    );
}

#[test]
fn a_missing_nested_result_member_reports_the_space_free_bracket_anchor() {
    let without_member = OUTCOME_IDS.replace(
        "id member Result[Option[int],string].ok 81818181818181818181818181818181\n",
        "",
    );
    let diagnostics = errors(OUTCOME_SOURCE, &without_member);
    assert!(
        diagnostics
            .messages()
            .iter()
            .any(|message| message.contains("member `Result[Option[int],string].ok`")),
        "the gap names the multi-argument enum member anchor: {:?}",
        diagnostics.all()
    );
}

#[test]
fn the_nested_option_reached_through_the_result_mints_its_own_anchor() {
    // The `Option<int>` reached through the `ok` payload is durable-reachable and
    // anchored at its own space-free bracket spelling `Option[int]`.
    let without_option_sum =
        OUTCOME_IDS.replace("id sum Option[int] 90909090909090909090909090909090\n", "");
    let diagnostics = errors(OUTCOME_SOURCE, &without_option_sum);
    assert!(
        diagnostics
            .messages()
            .iter()
            .any(|message| message.contains("sum `Option[int]`")),
        "{:?}",
        diagnostics.all()
    );
}

// --- Stored positional leaves: struct leaves and enum payload leaves. ---

/// One ledger for every positional-leaf program below: a `markers` root whose `Marker`
/// resource stores `at` (or the top-level pair `x`/`y`), plus the sum and member anchors
/// each stored enum needs.
const POSITIONAL_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Marker 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id root markers 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key markers.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id field Marker.at 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Marker.x 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
     id field Marker.y 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id sum Shape 50505050505050505050505050505050\n\
     id member Shape.rect 51515151515151515151515151515151\n\
     id sum Place 52525252525252525252525252525252\n\
     id member Place.at 53535353535353535353535353535353\n\
     id sum Option[Pos] 60606060606060606060606060606060\n\
     id member Option[Pos].none 61616161616161616161616161616161\n\
     id member Option[Pos].some 62626262626262626262626262626262\n\
     id sum Result[Pos,int] 70707070707070707070707070707070\n\
     id member Result[Pos,int].ok 71717171717171717171717171717171\n\
     id member Result[Pos,int].err 72727272727272727272727272727272\n\
     high-water 0\n\
     end\n";

/// A program storing `field` in `Marker.at`, beside the declarations `decls`.
fn marker_program(decls: &str, field: &str) -> String {
    format!(
        "{decls}\nresource Marker {{\n    required at: {field}\n}}\n\nstore ^markers[id: int]: Marker\n"
    )
}

const POS: &str = "struct Pos {\n    x: int\n    y: int\n}\n";
const POS_SWAPPED: &str = "struct Pos {\n    y: int\n    x: int\n}\n";
const POS_RENAMED: &str = "struct Pos {\n    z: int\n    y: int\n}\n";
const SHAPE: &str = "enum Shape {\n    rect(width: int, height: int)\n}\n";
const INNER: &str = "struct Inner {\n    a: int\n    b: int\n}\n";
const TOP_LEVEL: &str = "resource Marker {\n    required x: int\n    required y: int\n}\n\nstore ^markers[id: int]: Marker\n";

/// A stored struct or enum payload value lives positionally in one cell, so each leaf's
/// declared name and position is part of what its bytes mean: any edit to them moves the
/// durable contract. Top-level fields keep their ledger identity (their reorder moves the
/// contract too, as a control), and a code-only edit moves nothing.
#[test]
fn reordering_or_renaming_a_stored_positional_leaf_changes_the_contract() {
    let nested = |inner: &str| format!("{inner}struct Pos {{\n    i: Inner\n    c: int\n}}\n");
    let cases = [
        (
            "struct swap",
            marker_program(POS, "Pos"),
            marker_program(POS_SWAPPED, "Pos"),
        ),
        (
            "struct rename",
            marker_program(POS, "Pos"),
            marker_program(POS_RENAMED, "Pos"),
        ),
        (
            "payload swap",
            marker_program(SHAPE, "Shape"),
            marker_program(
                &SHAPE.replace("width: int, height: int", "height: int, width: int"),
                "Shape",
            ),
        ),
        (
            "payload rename",
            marker_program(SHAPE, "Shape"),
            marker_program(&SHAPE.replace("width:", "wide:"), "Shape"),
        ),
        (
            "nested struct swap",
            marker_program(&nested(INNER), "Pos"),
            marker_program(
                &nested(&INNER.replace("    a: int\n    b: int\n", "    b: int\n    a: int\n")),
                "Pos",
            ),
        ),
        (
            "Option<Pos> swap",
            marker_program(POS, "Option<Pos>"),
            marker_program(POS_SWAPPED, "Option<Pos>"),
        ),
        (
            "Result<Pos, int> swap",
            marker_program(POS, "Result<Pos, int>"),
            marker_program(POS_SWAPPED, "Result<Pos, int>"),
        ),
        (
            "top-level field swap",
            TOP_LEVEL.to_string(),
            TOP_LEVEL.replace(
                "    required x: int\n    required y: int\n",
                "    required y: int\n    required x: int\n",
            ),
        ),
    ];
    let unchanged: Vec<_> = cases
        .iter()
        .filter(|(_, before, after)| {
            contract_of(before, POSITIONAL_IDS) == contract_of(after, POSITIONAL_IDS)
        })
        .map(|(label, _, _)| *label)
        .collect();
    assert!(
        unchanged.is_empty(),
        "these edits must change the durable contract: {unchanged:?}"
    );
    let stored = marker_program(POS, "Pos");
    let code_only = format!("{stored}\npub fn one(): int {{\n    return 1\n}}\n");
    assert_eq!(
        contract_of(&stored, POSITIONAL_IDS),
        contract_of(&code_only, POSITIONAL_IDS),
        "a code-only edit keeps the durable contract"
    );
}

/// One stored enum member as an image states it: the enum's name, the member's name, and
/// each payload leaf's DURABLE name paired with the enum table's payload type.
type StoredMember = (String, String, Vec<(String, String)>);

/// A readable spelling of a payload leaf type: a scalar by its source name, an enum by
/// its name, and a record by its fields.
fn type_word(image: &VerifiedImage, ty: ImageType) -> String {
    match ty {
        ImageType::Scalar {
            scalar: Scalar::Int,
            ..
        } => "int".into(),
        ImageType::Scalar {
            scalar: Scalar::Text,
            ..
        } => "string".into(),
        ImageType::Record { idx, .. } => {
            let fields: Vec<_> = image
                .record_type(idx)
                .fields()
                .iter()
                .map(|field| format!("{}: {}", field.name(), type_word(image, field.ty())))
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
        ImageType::Enum { idx, .. } => image.enums()[idx.index() as usize].name().into(),
        other => format!("{other:?}"),
    }
}

/// Walk a verified stored value beside its sealed type, collecting every enum member it
/// reaches with its DURABLE payload names paired, position by position, with the enum
/// table's payload types.
fn stored_members(
    image: &VerifiedImage,
    values: &CanonicalValueShapeDag,
    shape: ValueShapeNodeId,
    ty: ImageType,
    out: &mut Vec<StoredMember>,
) {
    match (values.view(shape).expect("a verified shape"), ty) {
        (ValueShapeView::Struct(leaves), ImageType::Record { idx, .. }) => {
            for (leaf, field) in leaves.iter().zip(image.record_type(idx).fields()) {
                stored_members(image, values, leaf.shape(), field.ty(), out);
            }
        }
        (ValueShapeView::Enum { members, .. }, ImageType::Enum { idx, .. }) => {
            let sealed = &image.enums()[idx.index() as usize];
            for (member, variant) in members.iter().zip(sealed.variants()) {
                assert_eq!(member.payload().len(), variant.payload().len());
                out.push((
                    sealed.name().into(),
                    variant.name().to_string(),
                    member
                        .payload()
                        .iter()
                        .zip(variant.payload())
                        .map(|(leaf, ty)| (leaf.name().into(), type_word(image, *ty)))
                        .collect(),
                ));
                for (leaf, ty) in member.payload().iter().zip(variant.payload()) {
                    stored_members(image, values, leaf.shape(), *ty, out);
                }
            }
        }
        _ => {}
    }
}

/// The compiler takes a stored enum's DURABLE payload names from the declaration, in the
/// order lowering gives the payload, so each name sits beside the payload type the VM
/// reads at that position. A row whose leaves have distinct types makes a misordered
/// name observable.
#[test]
fn stored_enum_payload_names_follow_the_declared_payload_in_lowering_order() {
    let int = || "int".to_string();
    let pos = || "{x: int, y: int}".to_string();
    let rect = |name: &str| {
        (
            name.to_string(),
            "rect".to_string(),
            vec![("width".into(), int()), ("height".into(), int())],
        )
    };
    let cases: [(&str, String, Vec<StoredMember>); 6] = [
        (
            "payload",
            marker_program(SHAPE, "Shape"),
            vec![rect("Shape")],
        ),
        (
            "struct in payload",
            marker_program(
                &format!("{POS}enum Place {{\n    at(pos: Pos)\n}}\n"),
                "Place",
            ),
            vec![("Place".into(), "at".into(), vec![("pos".into(), pos())])],
        ),
        (
            "enum in struct",
            marker_program(
                &format!("{SHAPE}struct Box {{\n    s: Shape\n    n: int\n}}\n"),
                "Box",
            ),
            vec![rect("Shape")],
        ),
        (
            "Option<Pos>",
            marker_program(POS, "Option<Pos>"),
            vec![
                ("Option".into(), "none".into(), vec![]),
                (
                    "Option".into(),
                    "some".into(),
                    vec![("value".into(), pos())],
                ),
            ],
        ),
        (
            "Result<Pos, int>",
            marker_program(POS, "Result<Pos, int>"),
            vec![
                ("Result".into(), "ok".into(), vec![("value".into(), pos())]),
                ("Result".into(), "err".into(), vec![("value".into(), int())]),
            ],
        ),
        (
            "distinct payload types",
            marker_program(
                "enum Shape {\n    rect(label: string, n: int)\n}\n",
                "Shape",
            ),
            vec![(
                "Shape".into(),
                "rect".into(),
                vec![("label".into(), "string".into()), ("n".into(), int())],
            )],
        ),
    ];
    for (label, source, expected) in cases {
        let image = project(&source, POSITIONAL_IDS).image();
        let graph = image.durable_graph();
        let root = graph.roots().next().expect("the markers root");
        let DurableMemberViewKind::Field(field) =
            root.members().next().expect("the stored field").kind()
        else {
            panic!("{label}: `at` is a field");
        };
        let ty = image.record_type(image.roots()[0].record()).fields()[0].ty();
        let mut found = Vec::new();
        stored_members(&image, graph.value_shapes(), field.value(), ty, &mut found);
        assert_eq!(found, expected, "{label}");
    }
}
