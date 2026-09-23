//! The effect-ceiling honesty guarantee over the real lifecycle actor.
//!
//! A store records its accepted deployment ceiling at provision — the separately owned
//! standing maximum authority — and the atom-granular admission check enforces it at attach:
//! an image whose verified demand *exceeds* the accepted ceiling (a read-only export broadened
//! to also mutate, its deployment authority not yet updated) is refused before any engine
//! call, naming the exceeding export, effect, and path; an image whose demand fits *within*
//! the ceiling (even when narrower than a prior image's) is admitted.

use marrow_codes::Code;
use marrow_lifecycle::{AdmissionRefusal, AttachOutcome, LifecycleError};
use marrow_verify::VerifiedImage;

use crate::support::ceiling::{attach_image, image, provision, source_broadened, source_read_only};
use marrow_test_support::Scratch;

/// The MUST-WIN: a store provisioned under the read-only image refuses the broadened image —
/// the demand now exceeds the accepted ceiling — naming the export, the new effect, and the
/// path in source vocabulary, before any engine call, leaving the store intact and usable.
#[test]
fn a_broadened_demand_is_refused_naming_the_exceeding_place() {
    let scratch = Scratch::new("refuse");
    let read_only = image(&source_read_only());
    let broadened = image(&source_broadened());

    // The broadening changes the code and the demand, but not the durable contract or the
    // exported interface — so the refusal is specifically an authority refusal, not a
    // contract-changed one.
    provision(scratch.store(), &read_only);
    let head_before = std::fs::read(scratch.store().join("head")).expect("read head");

    let refusal = match attach_image(scratch.store(), &broadened) {
        Err(LifecycleError::Refused(AdmissionRefusal::Exceeds(refusal))) => refusal,
        Err(other) => panic!(
            "the broadened image must be refused as demand-exceeds-ceiling, got: {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("the broadened image must be refused, not admitted"),
    };

    assert_eq!(refusal.code(), Code::StoreDemandExceedsCeiling);
    // The refusal carries the export, the new effect, and the path as typed atoms.
    assert!(
        refusal
            .exceeding
            .iter()
            .any(|atom| atom.export == "readValue"
                && atom.effect == marrow_image::OperationClass::Write
                && atom.path.as_deref() == Some("^counters.label")),
        "a typed exceeding atom names readValue's write of ^counters.label: {:?}",
        refusal.exceeding,
    );

    // Zero engine calls / store intact: the head is byte-unchanged and the store still opens
    // and serves the prior (read-only) program as already-active.
    let head_after = std::fs::read(scratch.store().join("head")).expect("read head");
    assert_eq!(head_before, head_after, "the refusal wrote nothing");
    assert!(
        matches!(
            attach_image(scratch.store(), &read_only),
            Ok(AttachOutcome::AlreadyActive(_))
        ),
        "the prior program remains usable after the refusal",
    );
}

/// The admission gate precedes the binding-fact classification, so an image that both
/// broadens its demand beyond the accepted ceiling and changes the durable contract is
/// refused as the authority refusal, never as the contract one. The two name different
/// remedies — consciously expand the accepted ceiling, or review the change with `marrow
/// apply` — so which one arrives is part of the attach contract rather than a detail of the
/// order the checks happen to run in.
#[test]
fn a_demand_beyond_the_ceiling_preempts_the_contract_refusal() {
    let scratch = Scratch::new("preempt");
    let read_only = image(&source_read_only());
    // Broadened *and* contract-changed: the sparse `label` the broadened export writes is
    // promoted to required, which moves the durable contract on its own.
    let both =
        image(&source_broadened().replace("    label: string\n", "    required label: string\n"));
    assert_ne!(
        marrow_lifecycle::active_binding(&read_only).durable_contract,
        marrow_lifecycle::active_binding(&both).durable_contract,
        "the variant must really change the contract, or the preemption proves nothing",
    );

    provision(scratch.store(), &read_only);
    match attach_image(scratch.store(), &both) {
        Err(LifecycleError::Refused(AdmissionRefusal::Exceeds(refusal))) => {
            assert_eq!(refusal.code(), Code::StoreDemandExceedsCeiling);
        }
        Err(other) => panic!(
            "the ceiling refusal must preempt the contract one, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("an over-ceiling image must be refused, not admitted"),
    }
}

/// The naming join spells the Workshop catalog's paths correctly through the recursive
/// walk: a store provisioned under a Workshop variant with one export refuses a variant
/// broadened to touch a two-root spread — the refusal spells the second root (`^tallies`) and
/// its field (`^tallies.count`) exactly, proving the join is not a single-root special case.
#[test]
fn the_refusal_spells_places_across_roots() {
    const WORKSHOP_IDS: &str = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
         id product Asset 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
         id field Asset.name 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
         id product Tally 2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d\n\
         id field Tally.count 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
         id root assets 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
         id key assets.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
         id root tallies 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
         id key tallies.name 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
         high-water 0\n\
         end\n";
    const TWO_ROOT_SHAPE: &str = r#"resource Asset {
    required name: string
}

resource Tally {
    required count: int
}

store ^assets[id: int]: Asset

store ^tallies[name: string]: Tally
"#;
    let read_only = format!(
        "{TWO_ROOT_SHAPE}\npub fn assetName(id: int): string? {{\n    \
         return ^assets[id].name\n}}\n"
    );
    let broadened = format!(
        "{TWO_ROOT_SHAPE}\npub fn assetName(id: int): string? {{\n    \
         var found: string? = absent\n    transaction {{\n        \
         found = ^assets[id].name\n        \
         ref tally = ^tallies[\"reads\"] else {{ return found }}\n        \
         if exists(tally) {{\n            \
         tally.count = tally.count + 1\n        }}\n    }}\n    return found\n}}\n"
    );

    let compile_with = |source: &str| -> VerifiedImage {
        marrow_test_programs::program::compile(source, WORKSHOP_IDS)
    };

    let scratch = Scratch::new("two-root");
    let image_a = compile_with(&read_only);
    let image_b = compile_with(&broadened);
    provision(scratch.store(), &image_a);

    let refusal = match attach_image(scratch.store(), &image_b) {
        Err(LifecycleError::Refused(AdmissionRefusal::Exceeds(refusal))) => refusal,
        Err(other) => panic!(
            "expected demand-exceeds-ceiling, got {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("the two-root broadening must be refused"),
    };
    assert!(
        refusal
            .exceeding
            .iter()
            .any(|atom| atom.effect == marrow_image::OperationClass::Write
                && atom.path.as_deref() == Some("^tallies.count")),
        "a typed atom spells the second root's written field: {:?}",
        refusal.exceeding,
    );
    assert!(
        refusal
            .exceeding
            .iter()
            .all(|atom| atom.export == "assetName"),
        "every exceeding atom names the broadened export: {:?}",
        refusal.exceeding,
    );
}

/// The dual: a store provisioned under the broad image *admits* the narrower read-only image —
/// its demand is a strict subset of the accepted ceiling — as a binding-only rebind (same
/// durable contract and interface, different code). This proves the check is a real
/// intersection, not equality: demand ⊊ ceiling is admitted, not refused.
#[test]
fn a_narrowed_demand_within_the_ceiling_is_admitted() {
    let scratch = Scratch::new("narrow");
    let read_only = image(&source_read_only());
    let broadened = image(&source_broadened());

    provision(scratch.store(), &broadened);
    match attach_image(scratch.store(), &read_only) {
        Ok(AttachOutcome::Rebound { .. }) => {}
        Ok(AttachOutcome::AlreadyActive(_)) => {
            panic!("the narrower image differs in code, so it rebinds rather than already-active")
        }
        Err(other) => panic!(
            "a demand within the accepted ceiling must be admitted, got refusal: {}",
            other.code().as_str()
        ),
    }
}

/// The accepted ceiling belongs to the STORE, not to whatever image is bound to it, so a
/// binding-only rebind carries it forward verbatim. A rebind that wrote the incoming image's
/// own ceiling instead would silently shrink the standing maximum to the narrower image's
/// demand — and the store would then refuse the very image it was provisioned under. That is
/// the harm this pins: not a lost write, but a healthy store over-refused later by an attach
/// that reported success at the time.
#[test]
fn a_rebind_preserves_the_stores_standing_ceiling() {
    let scratch = Scratch::new("standing");
    let read_only = image(&source_read_only());
    let broadened = image(&source_broadened());
    let broad_ceiling = marrow_lifecycle::accepted_ceiling(&broadened);
    assert_ne!(
        broad_ceiling,
        marrow_lifecycle::accepted_ceiling(&read_only),
        "the two ceilings must really differ, or preservation proves nothing",
    );

    provision(scratch.store(), &broadened);
    match attach_image(scratch.store(), &read_only) {
        Ok(AttachOutcome::Rebound { .. }) => {}
        Ok(AttachOutcome::AlreadyActive(_)) => panic!("the narrower image differs in code"),
        Err(other) => panic!(
            "the narrower image must rebind, got {}",
            other.code().as_str()
        ),
    }

    let head = marrow_lifecycle::LogicalHead::decode(
        &std::fs::read(scratch.store().join("head")).expect("read head"),
    )
    .expect("decode head");
    assert_eq!(
        marrow_lifecycle::active_binding(&read_only),
        head.binding,
        "the rebind must really have rewritten the head, or nothing was preserved through it",
    );
    assert_eq!(
        head.accepted_ceiling, broad_ceiling,
        "the rebind rewrote the store's standing maximum to the incoming image's demand",
    );

    // And the maximum still admits what the store was provisioned under.
    match attach_image(scratch.store(), &broadened) {
        Ok(AttachOutcome::Rebound { .. }) => {}
        Ok(AttachOutcome::AlreadyActive(_)) => panic!("the broader image differs in code"),
        Err(other) => panic!(
            "the store over-refuses the image it was provisioned under: {}",
            other.code().as_str()
        ),
    }
}
