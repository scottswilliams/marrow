//! Module-ownership scans for the presence subsystem.
//!
//! These rules — presence owns direct-effect, proof, and write-effect
//! classification; sibling modules consume those facts instead of redefining
//! them — are not expressible as a Rust type boundary, so they live as tidy
//! source scans. Each is paired with positive behavior coverage of the
//! canonical classifiers in the `catalog_presence_*.rs` and `discharge_*.rs` suites.
//!
//! The scans match on identifiers, not bare substrings: a `fn` definition is
//! found only when the name is followed by a non-identifier character (so a
//! renamed `collect_statement_effects_v2` neither satisfies nor defeats a
//! check), and the absence assertions reject any such definition regardless of
//! whitespace or visibility prefix.

/// Whether `c` separates two identifiers (so it bounds an identifier token).
fn boundary(c: char) -> bool {
    !(c.is_alphanumeric() || c == '_')
}

/// Whether `name` occurs in `src` as a standalone identifier, bounded on both
/// sides by a non-identifier character (so `name` never matches inside `name_v2`
/// or `prefix_name`).
fn mentions_ident(src: &str, name: &str) -> bool {
    src.match_indices(name).any(|(start, _)| {
        let before_ok = start == 0 || src[..start].chars().next_back().is_some_and(boundary);
        let after_ok = src[start + name.len()..]
            .chars()
            .next()
            .is_none_or(boundary);
        before_ok && after_ok
    })
}

/// Whether `src` defines a function named exactly `name`: the `fn` keyword is a
/// standalone token (not the tail of an identifier such as `myfn`), `name`
/// follows it as a whole identifier, and a `(` or generic `<` opens the
/// signature.
fn defines_fn(src: &str, name: &str) -> bool {
    src.match_indices(name).any(|(start, _)| {
        let before = src[..start].trim_end();
        let after = src[start + name.len()..].trim_start();
        let fn_keyword = before.ends_with("fn")
            && before[..before.len() - "fn".len()]
                .chars()
                .next_back()
                .is_none_or(boundary);
        fn_keyword && after.starts_with(['(', '<'])
    })
}

#[test]
fn direct_effect_classification_is_owned_by_presence() {
    let facts = include_str!("../src/facts.rs");
    let presence = include_str!("../src/presence.rs");
    let direct = include_str!("../src/presence/direct.rs");
    let target = include_str!("../src/presence/target.rs");

    assert!(
        mentions_ident(presence, "direct_effects_for_block"),
        "presence must expose the canonical direct-effect classifier"
    );
    for duplicate in [
        "collect_statement_effects",
        "collect_expr_reads",
        "collect_saved_path_key_reads",
        "collect_saved_write",
        "saved_place_effect",
    ] {
        assert!(
            !defines_fn(facts, duplicate),
            "facts.rs must consume presence-owned direct-effect facts, not define `{duplicate}`"
        );
    }
    assert!(
        defines_fn(target, "saved_place"),
        "presence target classification must own saved-place fact resolution"
    );
    for duplicate in ["saved_place_effect", "member_path_ids"] {
        assert!(
            !defines_fn(direct, duplicate),
            "direct-effect collection must consume target-owned saved-place facts, not define `{duplicate}`"
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
        defines_fn(proofs, "read_proof") && defines_fn(proofs, "record_read"),
        "proof source classification and recording must live in presence::proofs"
    );
    assert!(
        defines_fn(writes, "call_writes_saved_data"),
        "saved-write effect recursion must live in presence::writes"
    );
    for duplicate in ["read_proof", "record_read"] {
        assert!(
            !defines_fn(walk, duplicate),
            "presence AST walking must delegate proof handling, not define `{duplicate}`"
        );
    }
    for duplicate in [
        "block_calls_saved_writer",
        "statement_calls_saved_writer",
        "expr_calls_saved_writer",
    ] {
        assert!(
            !defines_fn(effects, duplicate),
            "presence narrowing effects must delegate write-effect recursion, not define `{duplicate}`"
        );
    }
}

#[test]
fn presence_facts_expose_proof_id_and_status() {
    let facts = include_str!("../src/facts.rs");
    let proofs = include_str!("../src/presence/proofs.rs");

    assert!(
        mentions_ident(facts, "PresenceProofId") && facts.contains("pub struct PresenceProofId"),
        "presence ledger facts must expose a typed proof id"
    );
    assert!(
        facts.contains("pub status: PresenceProofStatus"),
        "presence ledger facts must expose discharged/pending status separately from source"
    );
    assert!(
        mentions_ident(facts, "PendingAttachedData") && mentions_ident(facts, "Discharged"),
        "presence proof status must distinguish pending attached-data obligations"
    );
    assert!(
        !mentions_ident(facts, "AttachedDataPending"),
        "pending status must not be encoded as a proof source variant"
    );
    assert!(
        proofs.contains("PresenceProofStatus::PendingAttachedData")
            && proofs.contains("PresenceProofStatus::Discharged"),
        "presence::proofs must assign proof status at the ledger owner"
    );
}
