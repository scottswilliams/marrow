#[test]
fn direct_effect_classification_is_owned_by_presence() {
    let facts = include_str!("../src/facts.rs");
    let presence = include_str!("../src/presence.rs");
    let direct = include_str!("../src/presence/direct.rs");
    let target = include_str!("../src/presence/target.rs");

    assert!(
        presence.contains("direct_effects_for_block"),
        "presence must expose the canonical direct-effect classifier"
    );
    for duplicate in [
        "fn collect_statement_effects",
        "fn collect_expr_reads",
        "fn collect_saved_path_key_reads",
        "fn collect_saved_write",
        "fn saved_place_effect",
    ] {
        assert!(
            !facts.contains(duplicate),
            "facts.rs must consume presence-owned direct-effect facts, not keep `{duplicate}`"
        );
    }
    assert!(
        target.contains("pub(super) fn saved_place"),
        "presence target classification must own saved-place fact resolution"
    );
    for duplicate in ["fn saved_place_effect", "fn member_path_ids"] {
        assert!(
            !direct.contains(duplicate),
            "direct-effect collection must consume target-owned saved-place facts, not keep `{duplicate}`"
        );
    }
}

#[test]
fn presence_walkers_delegate_proof_and_write_effect_owners() {
    let effects = include_str!("../src/presence/effects.rs");
    let proofs = include_str!("../src/presence/proofs.rs");
    let walk = include_str!("../src/presence/walk.rs");
    let writes = include_str!("../src/presence/writes.rs");

    assert!(
        proofs.contains("pub(super) fn read_proof") && proofs.contains("pub(super) fn record_read"),
        "proof source classification and recording must live in presence::proofs"
    );
    assert!(
        writes.contains("pub(super) fn call_writes_saved_data"),
        "saved-write effect recursion must live in presence::writes"
    );
    for duplicate in ["fn read_proof", "fn record_read"] {
        assert!(
            !walk.contains(duplicate),
            "presence AST walking must delegate proof handling, not keep `{duplicate}`"
        );
    }
    for duplicate in [
        "fn block_calls_saved_writer",
        "fn statement_calls_saved_writer",
        "fn expr_calls_saved_writer",
    ] {
        assert!(
            !effects.contains(duplicate),
            "presence narrowing effects must delegate write-effect recursion, not keep `{duplicate}`"
        );
    }
}

#[test]
fn presence_facts_expose_proof_id_and_status() {
    let facts = include_str!("../src/facts.rs");
    let proofs = include_str!("../src/presence/proofs.rs");

    assert!(
        facts.contains("pub struct PresenceProofId"),
        "presence ledger facts must expose a typed proof id"
    );
    assert!(
        facts.contains("pub status: PresenceProofStatus"),
        "presence ledger facts must expose discharged/pending status separately from source"
    );
    assert!(
        facts.contains("PendingAttachedData") && facts.contains("Discharged"),
        "presence proof status must distinguish pending attached-data obligations"
    );
    assert!(
        !facts.contains("AttachedDataPending"),
        "pending status must not be encoded as a proof source variant"
    );
    assert!(
        proofs.contains("PresenceProofStatus::PendingAttachedData")
            && proofs.contains("PresenceProofStatus::Discharged"),
        "presence::proofs must assign proof status at the ledger owner"
    );
}

#[test]
fn catalog_ids_are_typed_optional_facts_not_empty_sentinels() {
    let facts = include_str!("../src/facts.rs");
    let place = include_str!("../src/executable/place.rs");

    assert!(
        facts.contains("pub catalog_id: Option<String>"),
        "checked facts must use typed optional catalog ids for proposal-only identities"
    );
    let catalog_id_helper = facts
        .split("fn catalog_id(")
        .nth(1)
        .and_then(|rest| rest.split("fn resource_member_name_path").next())
        .expect("catalog_id helper");
    assert!(
        !catalog_id_helper.contains("unwrap_or_default()"),
        "catalog id binding must not encode missing ids as empty strings"
    );
    assert!(
        !place.contains("catalog_id: String::new()"),
        "saved-place construction must not use an empty catalog-id sentinel"
    );
}

#[test]
fn enum_source_order_helpers_are_private_and_non_durable() {
    let facts = include_str!("../src/facts.rs");

    assert!(
        !facts.contains("pub fn enum_member_by_ordinal")
            && !facts.contains("pub fn enum_member_ordinal"),
        "enum source-order helpers must not be public durable APIs"
    );
    assert!(
        facts.contains("pub(crate) fn enum_member_by_source_order"),
        "internal enum helper names must say source order, not durable ordinal"
    );
}
