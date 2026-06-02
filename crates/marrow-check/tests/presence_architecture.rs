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
