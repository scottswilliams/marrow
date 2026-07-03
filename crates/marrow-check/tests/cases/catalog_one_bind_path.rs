//! The RC2 enforcement artifact: the dual identity-binding path is unrepresentable.
//!
//! Path-spelled durable identity once bound through two parallel machines — a store bind and a
//! separate store-less adoption/mint loop that each carried identity forward, resolved renames, and
//! minted new ids. Any drift between them was a data-soundness hole. That second machine is deleted:
//! exactly one `fn bind`, behind a resolved `AcceptedAuthority`, binds every entry kind for every
//! authority. This tidy scan fails loudly if a second adoption/mint path ever returns.

const CATALOG_SOURCE: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/catalog/mod.rs"));

/// None of the deleted parallel-path functions may reappear, and the single bind entry must exist.
/// Each name below was a store-less counterpart to a step the one bind now performs; their return
/// is the dual-path smell.
#[test]
fn no_second_adoption_or_mint_binding_path() {
    for gone in [
        "fn adopt_or_mint_first_run",
        "fn adopt_first_run_entries",
        "fn adopt_first_run_entries_with",
        "fn mint_first_run",
        "fn renamed_carry_forward",
        "fn reserve_retired_committed_entries",
        "fn lock_adopts_source_cleanly",
    ] {
        assert!(
            !CATALOG_SOURCE.contains(gone),
            "a second adoption/mint binding path returned (`{gone}`); identity must bind through \
             the one `fn bind`"
        );
    }
    assert!(
        CATALOG_SOURCE.contains("\nfn bind("),
        "the single binding path `fn bind` must exist"
    );
}

/// The per-entry carry-forward/mint loop `bind_source_entries` has exactly one production call
/// site — inside `fn bind`. A second call site would be a second place that mints or carries
/// identity, the dual-path smell in another form. The definition and the one `#[cfg(test)]` harness
/// caller are excluded so the count reflects production wiring only.
#[test]
fn bind_source_entries_has_one_production_call_site() {
    let production_calls = CATALOG_SOURCE
        .match_indices("bind_source_entries(")
        .filter(|(offset, _)| {
            let preceding = &CATALOG_SOURCE[..*offset];
            // Exclude the function definition itself.
            !preceding.ends_with("fn ")
                // Exclude call sites inside the test module (everything after `mod tests`).
                && !preceding.contains("\nmod tests {")
        })
        .count();
    assert_eq!(
        production_calls, 1,
        "the per-entry mint/carry-forward loop must have exactly one production call site"
    );
}
