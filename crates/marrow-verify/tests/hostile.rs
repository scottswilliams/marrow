//! Hostile-image evidence: no byte string reaches execution except through the phase
//! that owns the invariant it violates.
//!
//! A flip with no rehash must reject at the envelope, because the digest no longer covers
//! the payload. A rehashed artifact that violates exactly one later invariant must reject
//! at the phase owning that invariant. A semantically valid rewrite is allowed to verify.
//!
//! A defect the producer's own coherence walk refuses never reaches these bytes; those
//! are pinned against the producer in `legacy_ok_pins.rs` rather than probed twice.

use marrow_image::{
    AdmittedRoot, CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, DraftTxn,
    DurableIndexComponent, DurableIndexShape, EnumTypeDef, ExportId, FieldDef, FuncId, FunctionDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, PlannedSiteRef, RecordTypeDef,
    RootOccurrenceDef, Scalar, SemanticStepKind, SemanticTarget, SpanEntry, TypeId, VariantDef,
};
use marrow_test_support::{admitted, admitted_plan, rehash, site};
use marrow_verify::{VerifyPhase, verify};

#[path = "hostile/group_presence.rs"]
mod group_presence;

#[path = "hostile/complete_entries.rs"]
mod complete_entries;

#[path = "hostile/guard_provenance.rs"]
mod guard_provenance;
#[path = "hostile/required_reads.rs"]
mod required_reads;

#[path = "hostile/legacy_artifact.rs"]
mod legacy_artifact;

#[path = "common/tracer_schema.rs"]
#[allow(
    dead_code,
    reason = "each verifier test binary uses the slice of the shared tracer fixture its pins need"
)]
mod tracer_schema;
use tracer_schema::Verdict::{Refused, Verified};
use tracer_schema::*;

/// A well-formed multi-function image: a caller exporting `main` that calls a helper,
/// plus a couple of constants. Every hostile case derives from this.
fn good_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let seven = ok(draft.intern_int(7));
    let helper_code = vec![Instr::ConstLoad(seven), Instr::Return];
    let helper = add_int_fn(&mut draft, "helper", helper_code);
    let main_code = vec![Instr::Call(helper.index()), Instr::Return];
    let main = add_int_fn(&mut draft, "main", main_code);
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.encode().expect("encode").bytes
}

/// The ten section frames as `(id, body_offset, body_len)` (header is 38 bytes).
fn sections(bytes: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut off = 38usize;
    for _ in 0..10 {
        let id = bytes[off];
        off += 1;
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        out.push((id, off, len));
        off += len;
    }
    out
}

/// The `(body_offset, body_len)` frame the section `id` carries.
fn section_frame(bytes: &[u8], id: u8) -> (usize, usize) {
    let (_, body, len) = *sections(bytes)
        .iter()
        .find(|(sid, ..)| *sid == id)
        .expect("the section is present");
    (body, len)
}

/// One operation site exactly as the encoder writes it into the DURABLE section:
/// `u8(step_count)`, each step's frozen ledger-kind byte and 16 id bytes, then the
/// target byte (`WholePayload` 0x00, `FieldLeaf` 0x01, `IndexScan` 0x02,
/// `IndexLookup` 0x03, `GroupEntry` 0x04).
fn encoded_site(steps: &[(SemanticStepKind, [u8; 16])], target: u8) -> Vec<u8> {
    let mut out = vec![steps.len() as u8];
    for (kind, id) in steps {
        out.push(kind.ledger_kind());
        out.extend_from_slice(id);
    }
    out.push(target);
    out
}

/// The tracer root's own whole-payload site, as encoded bytes.
fn encoded_root_site() -> Vec<u8> {
    encoded_site(
        &[
            (SemanticStepKind::Application, APPLICATION_ID),
            (SemanticStepKind::Placement, PLACEMENT_ID),
        ],
        0x00,
    )
}

/// A top-level field leaf of the tracer root, as encoded site bytes.
fn encoded_field_site(field_id: [u8; 16]) -> Vec<u8> {
    encoded_site(
        &[
            (SemanticStepKind::Application, APPLICATION_ID),
            (SemanticStepKind::Placement, PLACEMENT_ID),
            (SemanticStepKind::Field, field_id),
        ],
        0x01,
    )
}

/// Replace the encoded site `original` in the DURABLE section (id 3) with `forged`,
/// repair that section's length field, and revalidate the digest.
///
/// The producer binds a site against its own declaration rows and admits exactly the
/// target the named node carries, so a divergent site — an unresolvable path, a target
/// the node refuses, a path past the depth bound — exists only as forged bytes over a
/// valid image. This is the trust boundary the verifier owns.
fn forge_site(bytes: &mut Vec<u8>, original: &[u8], forged: &[u8]) {
    let (body, len) = section_frame(bytes, 3);
    let at = bytes[body..body + len]
        .windows(original.len())
        .position(|window| window == original)
        .map(|offset| body + offset)
        .expect("the site is present in the durable section");
    bytes.splice(at..at + original.len(), forged.iter().copied());
    let forged_len = (len - original.len() + forged.len()) as u32;
    bytes[body - 4..body].copy_from_slice(&forged_len.to_be_bytes());
    rehash(bytes);
}

/// The same encoded site path with its trailing target byte replaced: the shape a
/// retarget forgery takes, where only the claimed target disagrees with the node the
/// path resolves to.
fn retargeted(mut site: Vec<u8>, target: u8) -> Vec<u8> {
    let at = site.len() - 1;
    site[at] = target;
    site
}

/// One byte-poke case: a good image, the bytes poked into it, and the phase that owns
/// the invariant the poke breaks. A case that rehashes repairs the envelope digest, so
/// exactly one later invariant is violated; a case that does not leaves the digest stale,
/// which the envelope alone must answer whatever else the poked bytes would have meant.
struct Poke {
    what: &'static str,
    image: fn() -> Vec<u8>,
    poke: fn(&mut Vec<u8>),
    rehash: bool,
    phase: VerifyPhase,
}

const POKES: &[Poke] = &[
    Poke {
        what: "a header slot flip with a stale digest",
        image: good_image,
        poke: |bytes| bytes[10] ^= 0xFF,
        rehash: false,
        phase: VerifyPhase::Envelope,
    },
    Poke {
        what: "a section-body flip with a stale digest",
        image: good_image,
        poke: |bytes| {
            let last = bytes.len() - 1;
            bytes[last] ^= 0xFF;
        },
        rehash: false,
        phase: VerifyPhase::Envelope,
    },
    Poke {
        what: "a truncation with a stale digest",
        image: good_image,
        poke: |bytes| bytes.truncate(bytes.len() - 3),
        rehash: false,
        phase: VerifyPhase::Envelope,
    },
    Poke {
        what: "an unknown format version",
        image: good_image,
        poke: |bytes| bytes[4] = 0xFF,
        rehash: true,
        phase: VerifyPhase::Envelope,
    },
    Poke {
        what: "a section count the frame run contradicts",
        image: good_image,
        poke: |bytes| bytes[37] = 6,
        rehash: true,
        phase: VerifyPhase::Envelope,
    },
    // EXPORTS (id 6): count(u16), then per export id(32) func(u16), ids strictly ascending
    // and no function the target of two exports.
    Poke {
        what: "an export naming no function row",
        image: good_image,
        poke: |bytes| {
            let at = section_frame(bytes, 6).0 + 2 + 32;
            bytes[at..at + 2].fill(0xFF);
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "export ids out of ascending order",
        image: two_export_image,
        poke: |bytes| {
            let first = section_frame(bytes, 6).0 + 2;
            for offset in 0..34 {
                bytes.swap(first + offset, first + 34 + offset);
            }
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "two exports of one function",
        image: two_export_image,
        poke: |bytes| {
            let first_func = section_frame(bytes, 6).0 + 2 + 32;
            bytes.copy_within(first_func..first_func + 2, first_func + 34);
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // CONSTS (id 4): count(u16), then per const tag(u8) + payload.
    Poke {
        what: "an unknown constant tag",
        image: good_image,
        poke: |bytes| {
            let at = section_frame(bytes, 4).0 + 2;
            bytes[at] = 0x7F;
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // The DURABLE section (id 3) closes with the 32-byte contract id, which the verifier
    // recomputes from the decoded graph rather than trusting.
    Poke {
        what: "a mutated durable contract id",
        image: good_durable_image,
        poke: |bytes| {
            let (body, len) = section_frame(bytes, 3);
            bytes[body + len - 1] ^= 0xFF;
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // TYPES (id 2): count(2) | type: name(2) field_count(2) | field0: name(2) tag(1)
    // required(1). The contract binds the field profile, not only the root and its key.
    Poke {
        what: "a mutated durable field required flag",
        image: good_durable_image,
        poke: |bytes| {
            let body = section_frame(bytes, 2).0;
            bytes[body + 2 + 2 + 2 + 2 + 1] ^= 0x01;
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // DURABLE: count(2) | application(16) | root: name(2) key_count(2)
    // [key-tag(1) key_id(16)] record(2) placement(16)… The contract binds the ledger
    // identities, not the source names.
    Poke {
        what: "a mutated application ledger id",
        image: good_durable_image,
        poke: |bytes| {
            let body = section_frame(bytes, 3).0;
            bytes[body + 2] ^= 0xFF;
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "a mutated key-column ledger id",
        image: good_durable_image,
        poke: |bytes| {
            let body = section_frame(bytes, 3).0;
            bytes[body + 2 + 16 + 2 + 2 + 1] ^= 0xFF;
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // Entropy-minted ids are pairwise distinct by construction, so two equal identities in
    // one durable table are refused before the contract recomputation runs.
    Poke {
        what: "a duplicated ledger id",
        image: good_durable_image,
        poke: |bytes| {
            let body = section_frame(bytes, 3).0;
            let application = body + 2;
            let placement = body + 2 + 16 + 2 + 2 + (1 + 16) + 2;
            bytes.copy_within(application..application + 16, placement);
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // The site table's own copy of a ledger id is the *last* occurrence, after the member
    // tree. Flipping it leaves the graph — and so the contract id — intact, isolating the
    // site-path gate from the contract-id gate a member-tree flip trips.
    Poke {
        what: "a mutated site-path ledger id",
        image: good_durable_image,
        poke: |bytes| flip_last_ledger_id(bytes, VALUE_FIELD_ID),
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // Each id below is part of the durable member tree the contract binds, so append-only
    // evolution keeps stable codes: index identity, group structure, branch placement, and
    // each closed-enum member.
    Poke {
        what: "a mutated managed-index id",
        image: indexed_image,
        poke: |bytes| flip_ledger_id(bytes, BY_VALUE_INDEX_ID),
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "a mutated group id",
        image: group_branch_image,
        poke: |bytes| flip_ledger_id(bytes, GROUP_ID),
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "a mutated branch placement id",
        image: group_branch_image,
        poke: |bytes| flip_ledger_id(bytes, BRANCH_PLACEMENT_ID),
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "a mutated enum member id",
        image: widened_image,
        poke: |bytes| flip_ledger_id(bytes, ACCESS_READER_ID),
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // The durable value shape is self-describing (scalar 0, struct 1, enum 2); the `kind`
    // field's id is followed by its required flag and that tag, so an unknown tag there is
    // out of the closed value-shape domain.
    Poke {
        what: "an out-of-domain durable value tag",
        image: widened_image,
        poke: |bytes| {
            let mut needle = LABEL_FIELD_ID.to_vec();
            needle.extend_from_slice(&[0x01, 0x02]);
            let at = bytes
                .windows(needle.len())
                .position(|window| window == needle.as_slice())
                .expect("the kind field's value tag appears in the image");
            bytes[at + needle.len() - 1] = 0x7F;
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // TEST-ENTRY (id 8): count(u16), then per row name(u16) func(u16), names strictly
    // ascending and no two names aliasing one function.
    Poke {
        what: "a test entry naming no function row",
        image: two_test_image,
        poke: |bytes| {
            let at = section_frame(bytes, 8).0 + 4;
            bytes[at..at + 2].fill(0xFF);
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "a test entry naming no string row",
        image: two_test_image,
        poke: |bytes| {
            let at = section_frame(bytes, 8).0 + 2;
            bytes[at..at + 2].fill(0xFF);
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "two test entries of one name",
        image: two_test_image,
        poke: |bytes| {
            let body = section_frame(bytes, 8).0;
            bytes.copy_within(body + 2..body + 4, body + 6);
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "test entry names out of ascending order",
        image: two_test_image,
        poke: |bytes| {
            let body = section_frame(bytes, 8).0;
            for offset in 0..4 {
                bytes.swap(body + 2 + offset, body + 6 + offset);
            }
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "a test entry count past the section body",
        image: two_test_image,
        poke: |bytes| {
            let body = section_frame(bytes, 8).0;
            bytes[body..body + 2].copy_from_slice(&3u16.to_be_bytes());
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    Poke {
        what: "a test entry count short of the section body",
        image: two_test_image,
        poke: |bytes| {
            let body = section_frame(bytes, 8).0;
            bytes[body..body + 2].copy_from_slice(&1u16.to_be_bytes());
        },
        rehash: true,
        phase: VerifyPhase::Table,
    },
    // Assert-free bodies isolate the aliasing rule: the orphaned function carries no
    // `Assert`, so only the two-names-one-function check can answer here. An
    // assert-bearing body would trip the assert-outside-a-test-entry rule in the same
    // phase and mask a revert of the aliasing check.
    Poke {
        what: "two test names aliasing one function",
        image: assert_free_test_image,
        poke: |bytes| {
            let body = section_frame(bytes, 8).0;
            bytes.copy_within(body + 4..body + 6, body + 8);
        },
        rehash: true,
        phase: VerifyPhase::TestEntry,
    },
];

#[test]
fn every_byte_poke_rejects_at_the_phase_owning_its_invariant() {
    let mut wrong = Vec::new();
    for case in POKES {
        let mut bytes = (case.image)();
        assert_eq!(
            verdict_of(&bytes),
            Verified,
            "{} starts from a good image",
            case.what
        );
        (case.poke)(&mut bytes);
        if case.rehash {
            rehash(&mut bytes);
        }
        let answer = verdict_of(&bytes);
        if answer != Refused(case.phase) {
            wrong.push(format!("{}: {answer:?}, want {:?}", case.what, case.phase));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// One forged-site case: an image whose encoded site `original` is replaced by `forged`.
///
/// The binder publishes only canonical paths of the declaration graph, each carrying the
/// one target its resolved node admits, so every site below exists only as forged bytes
/// over a valid image. Resolving each site path against the verifier's own reconstructed
/// node set is the trust boundary this table pins: a path that resolves to nothing, or to
/// a node whose kind the claimed target contradicts, must be refused before any function
/// is typed and whether or not an opcode names it.
struct SiteForgery {
    what: &'static str,
    image: fn() -> Vec<u8>,
    original: fn() -> Vec<u8>,
    forged: fn() -> Vec<u8>,
}

const SITE_FORGERIES: &[SiteForgery] = &[
    // The unique `byValue` admits only a complete-key exact lookup; a progressive-prefix
    // scan target over it would traverse a unique index and observe siblings.
    SiteForgery {
        what: "a scan target over a unique index",
        image: unique_index_site_image,
        original: || encoded_index_site(BY_VALUE_INDEX_ID, 0x03),
        forged: || encoded_index_site(BY_VALUE_INDEX_ID, 0x02),
    },
    // The mirror: the nonunique `byLabel` admits only a progressive-prefix scan.
    SiteForgery {
        what: "an exact-lookup target over a nonunique index",
        image: nonunique_index_site_image,
        original: || encoded_index_site(BY_LABEL_INDEX_ID, 0x02),
        forged: || encoded_index_site(BY_LABEL_INDEX_ID, 0x03),
    },
    // A GroupEntry target must resolve to a group node; `details.pages` is a field leaf.
    SiteForgery {
        what: "a whole-group target over a field node",
        image: group_field_site_image,
        original: || encoded_group_field_site(0x01),
        forged: || encoded_group_field_site(0x04),
    },
    // A ledger id absent from the durable graph resolves against no reconstructed node.
    SiteForgery {
        what: "a site path naming no graph node",
        image: good_durable_image,
        original: || encoded_field_site(LABEL_FIELD_ID),
        forged: || encoded_field_site([0xbb; 16]),
    },
    // A field site cannot acquire a whole-payload target: the path resolves to a field
    // node while the target claims a keyed placement.
    SiteForgery {
        what: "a whole-payload target over a field path",
        image: good_durable_image,
        original: || encoded_field_site(LABEL_FIELD_ID),
        forged: || retargeted(encoded_field_site(LABEL_FIELD_ID), 0x00),
    },
    // The mirror: a stored-field target over the root's own placement path.
    SiteForgery {
        what: "a field-leaf target over the root path",
        image: good_durable_image,
        original: encoded_root_site,
        forged: || retargeted(encoded_root_site(), 0x01),
    },
    // A path routing the `tags` branch under the `text` field of `notes`: a field has no
    // branch child, so the path names no node.
    SiteForgery {
        what: "a branch path routed through a field",
        image: nested_branch_site_image,
        original: || encoded_tag_entry_site(&[BRANCH_PLACEMENT_ID, TAG_PLACEMENT_ID]),
        forged: || {
            encoded_site(
                &[
                    (SemanticStepKind::Application, APPLICATION_ID),
                    (SemanticStepKind::Placement, PLACEMENT_ID),
                    (SemanticStepKind::Placement, BRANCH_PLACEMENT_ID),
                    (SemanticStepKind::Field, BRANCH_FIELD_ID),
                    (SemanticStepKind::Placement, TAG_PLACEMENT_ID),
                ],
                0x00,
            )
        },
    },
    // A second hop naming a placement that is no branch of `notes`.
    SiteForgery {
        what: "a branch path naming a nonexistent hop",
        image: nested_branch_site_image,
        original: || encoded_tag_entry_site(&[BRANCH_PLACEMENT_ID, TAG_PLACEMENT_ID]),
        forged: || encoded_tag_entry_site(&[BRANCH_PLACEMENT_ID, [0x99; 16]]),
    },
];

#[test]
fn every_forged_site_is_refused_where_the_table_resolves_it() {
    let mut wrong = Vec::new();
    for case in SITE_FORGERIES {
        let mut bytes = (case.image)();
        assert_eq!(
            verdict_of(&bytes),
            Verified,
            "{} starts from a good image",
            case.what
        );
        forge_site(&mut bytes, &(case.original)(), &(case.forged)());
        let answer = verdict_of(&bytes);
        if answer != Refused(VerifyPhase::Table) {
            wrong.push(format!("{}: {answer:?}", case.what));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// An image with two exported functions, `a` and `b`, each returning a constant.
/// The EXPORTS entries are two `32-byte id ‖ u16 func` records the encoder writes
/// in ascending id order.
fn two_export_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let one = ok(draft.intern_int(1));
    let a_code = vec![Instr::ConstLoad(one), Instr::Return];
    let a = add_int_fn(&mut draft, "a", a_code);
    let b_code = vec![Instr::ConstLoad(one), Instr::Return];
    let b = add_int_fn(&mut draft, "b", b_code);
    draft.add_export(ExportId::of_local("", "a"), a);
    draft.add_export(ExportId::of_local("", "b"), b);
    draft.encode().expect("encode").bytes
}

/// Patch a table section's leading `u16` count to `bound + 1` and revalidate the
/// digest, so only the count-bound invariant is violated. The verifier's decode-time
/// guard rejects the claimed count before it allocates the table, at the raised bound.
fn count_over_bound_rejects(section: u8, bound: usize) -> Verdict {
    let mut bytes = good_image();
    let (_, body, _) = *sections(&bytes)
        .iter()
        .find(|(id, ..)| *id == section)
        .expect("section present");
    let over = u16::try_from(bound + 1).expect("bound + 1 fits the u16 count field");
    bytes[body..body + 2].copy_from_slice(&over.to_be_bytes());
    rehash(&mut bytes);
    verdict_of(&bytes)
}

/// A hostile image claiming more record types, enum types, functions, or collection
/// types than its widened bound is rejected by the decode-time count guard before the
/// verifier allocates the table. Each bound is read from `marrow_image::bounds`, so
/// the rejection tracks the widened scale floor rather than a pinned literal.
#[test]
fn rehashed_type_family_counts_over_bound_reject_at_table() {
    use marrow_image::bounds;
    assert_eq!(
        count_over_bound_rejects(0x02, bounds::MAX_TYPES),
        Refused(VerifyPhase::Table),
        "type count over MAX_TYPES",
    );
    assert_eq!(
        count_over_bound_rejects(0x09, bounds::MAX_ENUMS),
        Refused(VerifyPhase::Table),
        "enum count over MAX_ENUMS",
    );
    assert_eq!(
        count_over_bound_rejects(0x05, bounds::MAX_FUNCTIONS),
        Refused(VerifyPhase::Table),
        "function count over MAX_FUNCTIONS",
    );
    assert_eq!(
        count_over_bound_rejects(0x0A, bounds::MAX_COLLECTIONS),
        Refused(VerifyPhase::Table),
        "collection count over MAX_COLLECTIONS",
    );
}

#[test]
fn function_phase_unreachable_instruction() {
    // Built through the draft, so the digest is valid; the extra Return is dead.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let one = ok(draft.intern_int(1));
    let code = vec![
        Instr::ConstLoad(one),
        Instr::Return,
        Instr::ConstLoad(one),
        Instr::Return,
    ];
    let func = add_int_fn(&mut draft, "main", code);
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

#[test]
fn function_phase_call_argument_type_mismatch() {
    // helper(n: int); main() calls it with a bool argument.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let helper_code = vec![Instr::LocalGet(0), Instr::Return];
    let helper = add_fn(
        &mut draft,
        "helper",
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::scalar(Scalar::Int),
        1,
        helper_code,
    );
    let flag = ok(draft.intern_bool(true));
    let main_code = vec![
        Instr::ConstLoad(flag),
        Instr::Call(helper.index()),
        Instr::Return,
    ];
    let main = add_int_fn(&mut draft, "main", main_code);
    draft.add_export(ExportId::of_local("", "main"), main);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

#[test]
fn closure_phase_mutual_recursion() {
    // ping -> pong -> ping: a two-node cycle.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    // ping calls function index 1 (pong); pong calls index 0 (ping).
    let ping_code = vec![Instr::Call(1), Instr::Return];
    let ping = add_int_fn(&mut draft, "ping", ping_code);
    let pong_code = vec![Instr::Call(0), Instr::Return];
    add_int_fn(&mut draft, "pong", pong_code);
    draft.add_export(ExportId::of_local("", "ping"), ping);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Closure)
    );
}

// --- Durable transaction-flow hostiles. ---

/// Encode a read-only export `read(k:string): T?` that reads the field at the site
/// `pick` selects from the sites `durable_schema` registers, returning `ret`.
fn read_field_export(pick: impl FnOnce(&Sites) -> PlannedSiteRef, ret: ImageType) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let site = pick(&durable_schema(&mut draft));
    let code = vec![Instr::LocalGet(0), Instr::DurReadField(site), Instr::Return];
    let func = add_fn(
        &mut draft,
        "read",
        vec![ImageType::scalar(Scalar::Text)],
        ret,
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "read"), func);
    draft.commit();
    draft_owner
}

#[test]
fn the_durable_opcode_site_determines_the_reconstructed_demand() {
    // Two read exports that differ only in the field a durable opcode reads. Demand
    // is reconstructed from the sealed site the bytecode names — there is no
    // serialized demand summary — so a change to which site an opcode reads changes
    // the reconstructed atom's path and the export's demand id.
    let value = read_field_export(
        |sites| sites.value.clone(),
        ImageType::opt_scalar(Scalar::Int),
    );
    let label = read_field_export(
        |sites| sites.label.clone(),
        ImageType::opt_scalar(Scalar::Text),
    );

    let value_image = verify(&value.encode().unwrap().bytes).expect("value read verifies");
    let label_image = verify(&label.encode().unwrap().bytes).expect("label read verifies");

    let value_export = &value_image.exports()[0];
    let label_export = &label_image.exports()[0];
    let value_demand = value_image
        .function(value_export.function())
        .expect("verified function")
        .demand();
    let label_demand = label_image
        .function(label_export.function())
        .expect("verified function")
        .demand();

    // Each demands a single read atom on its own field node.
    assert_eq!(value_demand.atoms().len(), 1);
    assert_eq!(
        *value_demand
            .atoms()
            .next()
            .expect("single demand atom")
            .path()
            .node_id()
            .bytes(),
        VALUE_FIELD_ID
    );
    assert_eq!(
        *label_demand
            .atoms()
            .next()
            .expect("single demand atom")
            .path()
            .node_id()
            .bytes(),
        LABEL_FIELD_ID
    );
    // Different sites read, so the demand identities differ.
    assert_ne!(value_export.demand_id(), label_export.demand_id());
}

/// Encode a read-only export whose body runs a bounded-traversal opcode over the root
/// entry site with the given `limit`/`from`, then balances the frozen `List[string]`
/// and on-more `Bool` off the stack (the export returns Unit). When `from`, a string
/// key param is pushed first as the inclusive lower bound so the head type-checks up to
/// the opcode.
fn iterate_root_export(limit: u32, from: bool) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let list_ty = ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Text),
    }));
    let (params, mut code): (Vec<ImageType>, Vec<Instr>) = if from {
        (
            vec![ImageType::scalar(Scalar::Text)],
            vec![Instr::LocalGet(0)],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let local_count = params.len() as u16;
    code.push(Instr::DurIterateBounded {
        site: sites.entry,
        limit,
        from,
        list_ty,
    });
    // Discard the on-more Bool then the frozen List so the Unit return sees an empty
    // stack; the opcode's stack effect is what these hostiles exercise.
    code.push(Instr::Pop);
    code.push(Instr::Pop);
    code.push(Instr::Return);
    let func = add_fn(
        &mut draft,
        "iter",
        params,
        ImageType::Unit,
        local_count,
        code,
    );
    draft.add_export(ExportId::of_local("", "iter"), func);
    draft.commit();
    draft_owner
}

#[test]
fn a_bounded_traversal_over_a_root_verifies_and_type_checks() {
    // The freeze-then-run opcode types and executes: over the root entry family it pops
    // the inclusive `from` key when present, pushes the frozen `List[string]` and the
    // on-more `Bool`, and the image seals. Both the no-`from` and inclusive-`from` forms
    // verify.
    for from in [false, true] {
        assert_eq!(
            verdict_of(&iterate_root_export(2, from).encode().unwrap().bytes),
            Verified,
        );
    }
}

#[test]
fn a_bounded_traversal_over_a_branch_verifies_and_type_checks() {
    // A branch site traverses the branch family beneath a fixed root entry: the ancestor
    // root key (int) is popped, the frozen `List[string]` of branch keys and the on-more
    // `Bool` are pushed, and the image seals.
    let (mut draft_owner, root, _branch_record) = flat_branch_draft();
    let mut draft = admitted(&mut draft_owner);
    let site = flat_branch_entry_site(&mut draft, &root);
    let list_ty = ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Text),
    }));
    let code = vec![
        Instr::LocalGet(0), // the root key: the ancestor locating the branch parent
        Instr::DurIterateBounded {
            site,
            limit: 3,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "notes",
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::Unit,
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "notes"), func);
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

#[test]
fn a_zero_or_oversized_traversal_bound_is_refused() {
    // The `at most N` bound is a positive compile-time constant: zero and a bound above
    // `MAX_TRAVERSAL_BOUND` are refused as out of range, so a hostile image cannot
    // smuggle an unbounded or overlarge frozen-key allocation.
    for limit in [0, marrow_image::bounds::MAX_TRAVERSAL_BOUND + 1] {
        let rejection = verify(&iterate_root_export(limit, false).encode().unwrap().bytes)
            .expect_err("an out-of-range traversal bound is refused");
        assert_eq!(rejection.phase(), VerifyPhase::Function);
        assert_eq!(
            rejection.detail(),
            "bounded traversal bound is out of range"
        );
    }
}

#[test]
fn a_bounded_traversal_with_a_mismatched_list_type_rejects() {
    // The frozen-list COLLTYPES index must name exactly `List[K]` for the traversed key
    // `K` (here the root key is `string`). An image naming a `List[int]` is a forged
    // frozen-list type the verifier refuses before the runtime materializes it.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let wrong_list = ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Int),
    }));
    let code = vec![
        Instr::DurIterateBounded {
            site: sites.entry,
            limit: 2,
            from: false,
            list_ty: wrong_list,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(&mut draft, "iter", Vec::new(), ImageType::Unit, 0, code);
    draft.add_export(ExportId::of_local("", "iter"), func);
    let rejection = verify(&draft.encode().unwrap().bytes)
        .expect_err("a mismatched frozen-list type is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(
        rejection.detail(),
        "bounded traversal list type does not name a list of the traversed key"
    );
}

#[test]
fn a_bounded_branch_traversal_missing_its_ancestor_key_rejects() {
    // A branch traversal pops the ancestor root key locating the parent entry. Pushing
    // no ancestor key leaves that pop against an empty stack — a key-arity forgery the
    // verifier refuses.
    let (mut draft_owner, root, _branch_record) = flat_branch_draft();
    let mut draft = admitted(&mut draft_owner);
    let site = flat_branch_entry_site(&mut draft, &root);
    let list_ty = ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Text),
    }));
    let code = vec![
        // No ancestor root key pushed before the opcode.
        Instr::DurIterateBounded {
            site,
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "notes",
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::Unit,
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "notes"), func);
    let rejection =
        verify(&draft.encode().unwrap().bytes).expect_err("a missing ancestor key is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(rejection.detail(), "operand stack underflow");
}

#[test]
fn a_bounded_traversal_over_a_field_leaf_site_rejects() {
    // Bounded traversal iterates the layer a site's placement belongs to. A field-leaf
    // site names a single scalar leaf, not a traversable entry family, so an image aiming
    // the opcode at a field site is refused before any frozen-key allocation.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let list_ty = ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Text),
    }));
    let code = vec![
        Instr::DurIterateBounded {
            site: sites.value,
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(&mut draft, "iter", Vec::new(), ImageType::Unit, 0, code);
    draft.add_export(ExportId::of_local("", "iter"), func);
    let rejection = verify(&draft.encode().unwrap().bytes)
        .expect_err("a traversal over a field-leaf site is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(rejection.detail(), "operation requires an entry site");
}

#[test]
fn a_family_populated_probe_over_a_field_leaf_site_rejects() {
    // The family-populated probe names a whole-entry family; a field-leaf site names a
    // scalar leaf, not a family, so an image aiming the probe at a field site is refused
    // as `DurExists`/`DurIterateBounded` over a field site are.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let code = vec![
        Instr::DurFamilyExists(sites.value),
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(&mut draft, "probe", Vec::new(), ImageType::Unit, 0, code);
    draft.add_export(ExportId::of_local("", "probe"), func);
    let rejection = verify(&draft.encode().unwrap().bytes)
        .expect_err("a family probe over a field site is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(rejection.detail(), "operation requires an entry site");
}

#[test]
fn a_managed_index_probe_over_a_field_leaf_site_rejects() {
    // `DurIndexExists` (the unique-index arm of `exists`) is executable only over an index
    // site. A forged image aiming it at a field-leaf site is refused at the opcode — a
    // trust-boundary reject, not a fall-through to the closed-complement `unreachable` — the
    // same family guard the scan and lookup opcodes share.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurIndexExists(sites.value),
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "probe",
        vec![ImageType::scalar(Scalar::Text)],
        ImageType::Unit,
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "probe"), func);
    let rejection = verify(&draft.encode().unwrap().bytes)
        .expect_err("a managed-index probe over a field site is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(
        rejection.detail(),
        "a managed-index opcode over a non-index site"
    );
}

#[test]
fn a_non_index_opcode_over_a_managed_index_site_rejects() {
    // The mirror of the field-leaf probe: `apply_durable` dispatches to the index-read path
    // by SITE kind, so a forged image aiming a whole-entry/field opcode (here `DurReadField`)
    // at a managed-index site is routed there even though it is not an index read. The trust
    // boundary refuses it rather than reaching the closed-complement of the three index reads.
    let (mut draft_owner, root) = indexed_draft(by_label_projection());
    let mut draft = admitted(&mut draft_owner);
    // Index 1 is the unique `byValue`, so the exact-lookup target is the one it admits.
    let lookup_site = site(
        &mut draft,
        root.occurrence(),
        &root.index_paths()[1],
        SemanticTarget::IndexLookup,
    );
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurReadField(lookup_site),
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "probe",
        vec![ImageType::scalar(Scalar::Text)],
        ImageType::opt_scalar(Scalar::Int),
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "probe"), func);
    let rejection = verify(&draft.encode().unwrap().bytes)
        .expect_err("a non-index opcode over an index site is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(
        rejection.detail(),
        "a non-index opcode over a managed-index site"
    );
}

#[test]
fn a_bounded_traversal_after_commit_rejects() {
    // The commit consumes the session's engine transaction, so no durable operation may
    // follow it. A bounded traversal is a durable read; the flow lattice refuses it after
    // commit exactly as it refuses a post-commit field read, so the runtime never reaches
    // a consumed transaction.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let list_ty = ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Text),
    }));
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurEraseEntry(sites.entry.clone()),
        Instr::TxnCommit,
        Instr::DurIterateBounded {
            site: sites.entry,
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "put",
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ImageType::Unit,
        2,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    let rejection =
        verify(&draft.encode().unwrap().bytes).expect_err("a post-commit traversal is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Flow);
    assert_eq!(
        rejection.detail(),
        "a durable operation follows the transaction's commit"
    );
}

#[test]
fn a_traversal_list_type_naming_a_map_rejects() {
    // `list_ty` must name exactly `List[K]` for the traversed key; a `Map`-shaped
    // index is in range and remains the verifier's forged-frozen-list refusal.
    let build = |list_ty: u16| -> ImageDraft {
        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let sites = durable_schema(&mut draft);
        // One well-formed `Map` row at index 0: a valid collection, but the wrong kind for
        // a frozen key list. Index 1 dangles one past the single-row table.
        ok(draft.add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Text),
            value: ImageType::scalar(Scalar::Text),
        }));
        let code = vec![
            Instr::DurIterateBounded {
                site: sites.entry,
                limit: 2,
                from: false,
                list_ty: marrow_image::CollTypeId::from_index(list_ty),
            },
            Instr::Pop,
            Instr::Pop,
            Instr::Return,
        ];
        let func = add_fn(&mut draft, "iter", Vec::new(), ImageType::Unit, 0, code);
        draft.add_export(ExportId::of_local("", "iter"), func);
        draft.commit();
        draft_owner
    };
    // A dangling index never encodes — the producer refuses it — so only the wrong-KIND
    // index (row 0, a Map) reaches the bytes, where the verifier's own function-phase law
    // refuses it.
    let rejection = verify(&build(0).encode().unwrap().bytes)
        .expect_err("a non-List[K] frozen type is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(
        rejection.detail(),
        "bounded traversal list type does not name a list of the traversed key"
    );
}

#[test]
fn substituting_family_exists_leaves_invalid_bounded_traversal_operands() {
    // Active 0x39 is DurFamilyExists and consumes only the site operand. Substituting
    // it for a bounded traversal leaves that traversal's remaining operands on the
    // instruction tape; those bytes do not form a valid instruction sequence.
    let mut bytes = iterate_root_export(2, false).encode().unwrap().bytes;
    // Locate the bounded-traversal opcode and overwrite its opcode byte with 0x39.
    let opcode = [0x3B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00];
    let at = bytes
        .windows(opcode.len())
        .position(|w| w == opcode)
        .expect("the bounded-traversal opcode is present");
    bytes[at] = 0x39;
    rehash(&mut bytes);
    let rejection = verify(&bytes).expect_err("the leftover traversal operands are refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(rejection.detail(), "unknown or not-yet-supported opcode");
}

#[test]
fn a_malformed_from_flag_byte_is_refused_at_decode() {
    // The `from` operand is a strict 0/1 flag. A hostile image that sets it to 0x02 —
    // re-digested so the envelope passes — is refused when the opcode is decoded, so no
    // third from-state can be smuggled past the bounded-traversal decoder.
    let mut bytes = iterate_root_export(2, false).encode().unwrap().bytes;
    // The encoded opcode is the ten bytes `0x3B <site:2=0> <limit:4=2> <from:1=0>
    // <list_ty:2=0>`; locate the full encoding and flip the trailing from flag to an
    // out-of-range 0x02 (this also pins the opcode's exact width).
    let opcode = [0x3B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00];
    let at = bytes
        .windows(opcode.len())
        .position(|w| w == opcode)
        .expect("the bounded-traversal opcode is present");
    assert_eq!(bytes[at + 7], 0x00, "the from flag starts cleared");
    bytes[at + 7] = 0x02;
    rehash(&mut bytes);
    let rejection = verify(&bytes).expect_err("a malformed from flag is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Function);
    assert_eq!(rejection.detail(), "malformed bool operand");
}

#[test]
fn durable_put_export_verifies() {
    // The well-formed baseline the flow hostiles derive from.
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

/// A well-formed durable image (the tracer schema plus one verifying `put` export),
/// the baseline for the durable-contract-id hostiles.
fn good_durable_image() -> Vec<u8> {
    put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    })
    .encode()
    .unwrap()
    .bytes
}

#[test]
fn a_composite_root_write_opcode_with_a_truncated_key_path_rejects() {
    // A composite-key root is executable, addressed by its whole two-column key tuple. A
    // forged erase whose body supplies only one key column — too few for
    // the two-column key-path the verifier derives from the schema — cannot satisfy the
    // operand stack, so it is refused during per-function typing (the write-path
    // counterpart of the read-path truncation hostile).
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let counter = ok(draft.intern_string("Counter"));
    let value = ok(draft.intern_string("value"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![FieldDef {
            name: value,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    }));
    let root = ok(draft.intern_string("pairs"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![field_member(
                shapes,
                None,
                VALUE_FIELD_ID,
                true,
                Scalar::Int,
            )],
        )
        .expect("a well-formed declaration");
    let admitted = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![
                    KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                    },
                    KeyColumn {
                        scalar: Scalar::Int,
                        id: LedgerIdBytes::from_bytes([0x1c; 16]),
                    },
                ],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let entry_site = site(
        &mut draft,
        admitted.occurrence(),
        admitted.placement_path(),
        SemanticTarget::WholePayload,
    );
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurEraseEntry(entry_site),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "put",
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ImageType::Unit,
        2,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    // The truncated key-path is rejected during per-function structural/type recording
    // (the `image.function` phase), where the operation's key-path is typed.
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

const NON_INDEX_ELIGIBLE_FIELD_DETAIL: &str =
    "durable index field component names a field that is not index-eligible";

/// One managed index of the tracer root, as encoded site bytes: `application ->
/// placement -> index`, with the read target the index's unique flag admits.
fn encoded_index_site(index_id: [u8; 16], target: u8) -> Vec<u8> {
    encoded_site(
        &[
            (SemanticStepKind::Application, APPLICATION_ID),
            (SemanticStepKind::Placement, PLACEMENT_ID),
            (SemanticStepKind::Index, index_id),
        ],
        target,
    )
}

/// The well-formed indexed tracer graph, encoded.
fn indexed_image() -> Vec<u8> {
    indexed_draft(by_label_projection())
        .0
        .encode()
        .expect("encode")
        .bytes
}

/// The indexed graph carrying the one read site each index admits: an exact lookup over
/// the unique `byValue`, a progressive-prefix scan over the nonunique `byLabel`.
fn index_site_image(index: usize, target: SemanticTarget) -> Vec<u8> {
    let (mut draft_owner, root) = indexed_draft(by_label_projection());
    let mut draft = admitted(&mut draft_owner);
    site(
        &mut draft,
        root.occurrence(),
        &root.index_paths()[index],
        target,
    );
    draft.encode().expect("encode").bytes
}

fn unique_index_site_image() -> Vec<u8> {
    index_site_image(1, SemanticTarget::IndexLookup)
}

fn nonunique_index_site_image() -> Vec<u8> {
    index_site_image(0, SemanticTarget::IndexScan)
}

#[test]
fn a_well_formed_indexed_graph_verifies() {
    assert_eq!(
        verdict_of(
            &indexed_draft(by_label_projection())
                .0
                .encode()
                .unwrap()
                .bytes
        ),
        Verified,
    );
}

#[test]
fn an_index_component_naming_no_leaf_of_its_root_rejects() {
    // A forged projection component pointing at a leaf the root does not carry is
    // refused at decode: the verifier re-resolves every component against the root's
    // own fields and keys.
    let forged = vec![
        DurableIndexComponent::Field(LedgerIdBytes::from_bytes([0x99; 16])),
        DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
    ];
    assert_eq!(
        verdict_of(&indexed_draft(forged).0.encode().unwrap().bytes),
        Refused(VerifyPhase::Table),
    );
}

#[test]
fn a_nonunique_index_missing_its_identity_suffix_rejects() {
    // A non-unique index distinguishes rows by ending with the complete identity suffix
    // in declaration order. A forged `byLabel` projection that drops the trailing
    // identity key resolves every component to a real leaf, yet its ordering is
    // malformed — a hostile image the runtime must never trust to disambiguate rows.
    // The verifier re-enforces the suffix rule the compiler owns, so the image rejects.
    let no_suffix = vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
        LABEL_FIELD_ID,
    ))];
    assert_eq!(
        verdict_of(&indexed_draft(no_suffix).0.encode().unwrap().bytes),
        Refused(VerifyPhase::Table),
    );
}

#[test]
fn a_nonunique_index_with_a_leading_identity_key_rejects() {
    // A non-unique index carries no identity key before its trailing suffix. Because the
    // suffix must already hold every identity key, a leading identity key necessarily
    // duplicates a suffix key, so the verifier's distinctness rule refuses this forged
    // `byLabel` projection — the no-leading-key rule is enforced by distinctness + suffix.
    let leading_key = vec![
        DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
        DurableIndexComponent::Field(LedgerIdBytes::from_bytes(LABEL_FIELD_ID)),
        DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
    ];
    assert_eq!(
        verdict_of(&indexed_draft(leading_key).0.encode().unwrap().bytes),
        Refused(VerifyPhase::Table),
    );
}

#[test]
fn a_durable_index_repeating_a_component_rejects() {
    // Each projection component appears at most once. A forged `byLabel` projection that
    // repeats the `label` field keeps a valid trailing identity suffix, so only the
    // distinctness rule is violated — which the verifier re-enforces at decode.
    let repeated = vec![
        DurableIndexComponent::Field(LedgerIdBytes::from_bytes(LABEL_FIELD_ID)),
        DurableIndexComponent::Field(LedgerIdBytes::from_bytes(LABEL_FIELD_ID)),
        DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
    ];
    assert_eq!(
        verdict_of(&indexed_draft(repeated).0.encode().unwrap().bytes),
        Refused(VerifyPhase::Table),
    );
}

#[test]
fn a_unique_index_with_an_empty_projection_rejects() {
    // A unique index may omit the identity suffix, but it must still project at least one
    // leaf: an empty projection has no key to look up and is meaningless. A forged image
    // that carries one is refused at decode.
    let (draft, _root) = indexed_draft_full(by_label_projection(), Vec::new());
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Table)
    );
}

/// A root with one required scalar field and a unique index projecting that field.
/// The scalar is variable so the closed managed-index eligibility domain can be
/// exercised without changing any other image fact.
fn scalar_field_indexed_draft(scalar: Scalar) -> ImageDraft {
    const FIELD_ID: [u8; 16] = [0x1d; 16];
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let record_name = ok(draft.intern_string("IndexedScalar"));
    let field_name = ok(draft.intern_string("value"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: record_name,
        fields: vec![FieldDef {
            name: field_name,
            ty: ImageType::scalar(scalar),
            required: true,
        }],
    }));
    let root = ok(draft.intern_string("indexedScalars"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![field_member(shapes, None, FIELD_ID, true, scalar)],
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Text,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: vec![DurableIndexShape {
                    id: LedgerIdBytes::from_bytes(BY_VALUE_INDEX_ID),
                    unique: true,
                    components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                        FIELD_ID,
                    ))],
                }]
                .into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    draft_owner
}

#[test]
fn every_orderable_scalar_field_managed_index_verifies() {
    for scalar in [
        Scalar::Int,
        Scalar::Text,
        Scalar::Bool,
        Scalar::Bytes,
        Scalar::Date,
        Scalar::Instant,
    ] {
        let bytes = scalar_field_indexed_draft(scalar)
            .encode()
            .expect("encode orderable scalar-field index")
            .bytes;
        verify(&bytes).unwrap_or_else(|rejection| {
            panic!("{scalar:?} field index should verify: {rejection}")
        });
    }
}

#[test]
fn a_duration_field_is_not_a_managed_index_component() {
    let bytes = scalar_field_indexed_draft(Scalar::Duration)
        .encode()
        .expect("encode hostile duration-field index")
        .bytes;
    let rejection = verify(&bytes).expect_err("a duration-field managed index is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(rejection.detail(), NON_INDEX_ELIGIBLE_FIELD_DETAIL);
}

/// A `Counter` root whose `owner` field is a widened dense struct, with a unique index
/// forging a component over that widened field. A widened field is executable (framed
/// inline in its cell) but never index-eligible — an index component must project an
/// orderable durable-key scalar, which a composite is not — so the verifier refuses the
/// component independently of the compiler. This is the index-eligibility decouple: the
/// same field keeps the root flat-executable yet stays out of every index.
fn widened_field_indexed_draft() -> ImageDraft {
    const OWNER_FIELD_ID: [u8; 16] = [0x1e; 16];
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let name_ty = ok(draft.intern_string("Name"));
    let first = ok(draft.intern_string("first"));
    let last = ok(draft.intern_string("last"));
    let name_record = ok(draft.add_record_type(RecordTypeDef {
        name: name_ty,
        fields: vec![
            FieldDef {
                name: first,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
            FieldDef {
                name: last,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
        ],
    }));
    let counter = ok(draft.intern_string("Counter"));
    let value = ok(draft.intern_string("value"));
    let owner = ok(draft.intern_string("owner"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: owner,
                ty: ImageType::Record {
                    idx: name_record,
                    optional: false,
                },
                required: true,
            },
        ],
    }));
    let root = ok(draft.intern_string("counters"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    // A dense struct value of two text leaves, minted into this draft's own arena.
    let owner_value = ok(draft.value_struct(vec![shapes.text; 2]));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![
                field_member(shapes, None, VALUE_FIELD_ID, true, Scalar::Int),
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(OWNER_FIELD_ID),
                        required: true,
                        value: owner_value,
                    },
                },
            ],
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Text,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: vec![DurableIndexShape {
                    id: LedgerIdBytes::from_bytes(BY_VALUE_INDEX_ID),
                    unique: true,
                    components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                        OWNER_FIELD_ID,
                    ))],
                }]
                .into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    draft_owner
}

#[test]
fn an_index_component_over_a_widened_field_rejects() {
    // The widened `owner` field is admitted (its root is flat-executable), but naming it
    // as an index component is refused at decode — index eligibility is decoupled from
    // field executability, so a widened field is never an index leaf.
    let bytes = widened_field_indexed_draft()
        .encode()
        .expect("encode hostile widened-field index")
        .bytes;
    let rejection = verify(&bytes).expect_err("a widened-field managed index is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(rejection.detail(), NON_INDEX_ELIGIBLE_FIELD_DETAIL);
}

/// Flip the first occurrence of a 16-byte ledger id in `bytes`. The distinct test
/// ids never collide with a string or other field, so this reliably mutates the
/// targeted node.
fn flip_ledger_id(bytes: &mut [u8], id: [u8; 16]) {
    let at = bytes
        .windows(16)
        .position(|window| window == id)
        .expect("the ledger id appears in the image");
    bytes[at] ^= 0xFF;
}

/// The group/branch graph, encoded, and the same graph carrying the field-leaf site the
/// binder admits over the `details.pages` group leaf.
fn group_branch_image() -> Vec<u8> {
    group_branch_draft(false).0.encode().expect("encode").bytes
}

fn group_field_site_image() -> Vec<u8> {
    let (mut draft_owner, root) = group_branch_draft(false);
    let mut draft = admitted(&mut draft_owner);
    book_group_field_site(&mut draft, &root);
    draft.encode().expect("encode").bytes
}

#[test]
fn a_field_site_over_a_root_level_group_bearing_root_verifies() {
    // A keyed root whose resource declares a root-level unkeyed `group` of storable-value
    // fields is flat-executable: the group is a value unit of the root entry, framed in
    // the entry payload. A site over the root's own `title` field seals executable and its
    // read opcode verifies (a keyed branch and a root-level group no longer park the root).
    assert_eq!(
        verdict_of(&group_branch_draft(true).0.encode().unwrap().bytes),
        Verified
    );
}

/// A keyed root whose member tree and record slots run `[Group, Field]` — a group before
/// a top-level field — while `field_order` selects whether the record's own slots run
/// group-first (matching the members) or field-first. Sealing computes a group's slot as
/// `field_count + ordinal`, which is only correct when every field precedes every group;
/// a group-first member tree therefore mis-indexes unless the verifier refuses it. The
/// group holds one sparse `pages:int` leaf; the top-level field is a required `title:text`.
fn group_before_field_draft(record_group_first: bool) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let book = ok(draft.intern_string("Book"));
    let title = ok(draft.intern_string("title"));
    let details_qualified = ok(draft.intern_string("Book.details"));
    let details_pages = ok(draft.intern_string("pages"));
    let details_record = ok(draft.add_record_type(RecordTypeDef {
        name: details_qualified,
        fields: vec![FieldDef {
            name: details_pages,
            ty: ImageType::scalar(Scalar::Int),
            required: false,
        }],
    }));
    let details = ok(draft.intern_string("details"));
    let title_slot = FieldDef {
        name: title,
        ty: ImageType::scalar(Scalar::Text),
        required: true,
    };
    let group_slot = FieldDef {
        name: details,
        ty: ImageType::Record {
            idx: details_record,
            optional: false,
        },
        required: true,
    };
    // The record slots either run group-first (matching the forged member order, so the
    // record/member tie itself passes and only the fields-first invariant refuses it) or
    // field-first (the ordinary shape, exercised for contrast).
    let fields = if record_group_first {
        vec![group_slot, title_slot]
    } else {
        vec![title_slot, group_slot]
    };
    let record = ok(draft.add_record_type(RecordTypeDef { name: book, fields }));
    let root = ok(draft.intern_string("books"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    // The commands state the group before the top-level field, so the Product's direct
    // members run `[Group, Field]` — the order the fields-first invariant refuses.
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Group {
                        id: LedgerIdBytes::from_bytes([0x20; 16]),
                    },
                },
                field_member(shapes, Some(0), [0x21; 16], false, Scalar::Int),
                field_member(shapes, None, VALUE_FIELD_ID, true, Scalar::Text),
            ],
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    draft_owner
}

#[test]
fn a_root_member_tree_with_a_field_after_a_group_rejects() {
    // The record↔member tie walks the member tree in its own order, so a forged
    // `[Group, Field]` member tree with matching group-first record slots ties cleanly.
    // Sealing then reads a group's slot as `field_count + ordinal`, which lands on the
    // trailing scalar slot — an out-of-place index that would panic the sealer. The
    // fields-first invariant is enforced at the tie: a field after a group is refused at
    // the table phase, so every byte string yields VERIFIED or a typed rejection.
    let rejection = verify(&group_before_field_draft(true).encode().unwrap().bytes)
        .expect_err("a field member after a group member is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "root member tree places a field after a group"
    );
}

/// A keyed root whose top-level field members and record field slots disagree in count:
/// `member_fields` scalar-text field members against `record_fields` scalar-text record
/// slots. The record/member tie counts these against each other, so an image with more
/// members than slots (or more slots than members) is a count forgery the verifier
/// refuses independent of the fields-first order check.
fn field_count_mismatch_draft(member_fields: usize, record_fields: usize) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let rec = ok(draft.intern_string("Rec"));
    let fields = (0..record_fields)
        .map(|i| FieldDef {
            name: ok(draft.intern_string(&format!("f{i}"))),
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        })
        .collect();
    let record = ok(draft.add_record_type(RecordTypeDef { name: rec, fields }));
    let root = ok(draft.intern_string("recs"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let members = (0..member_fields)
        .map(|i| field_member(shapes, None, [0x40 + i as u8; 16], true, Scalar::Text))
        .collect();
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            members,
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    draft_owner
}

#[test]
fn a_root_member_tree_with_more_members_than_record_slots_rejects() {
    // Two field members against one record slot: the tie runs out of slots on the second
    // member and refuses the short-record forgery at the table phase.
    let rejection = verify(&field_count_mismatch_draft(2, 1).encode().unwrap().bytes)
        .expect_err("more members than record slots is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "root member tree has more top-level members than the record"
    );
}

#[test]
fn a_root_member_tree_with_fewer_members_than_record_slots_rejects() {
    // One field member against two record slots: a slot is left over after the members are
    // consumed and the leftover-slot forgery is refused at the table phase.
    let rejection = verify(&field_count_mismatch_draft(1, 2).encode().unwrap().bytes)
        .expect_err("fewer members than record slots is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "root member tree has fewer top-level members than the record"
    );
}

/// The `details.pages` group field leaf, as encoded site bytes: application -> root
/// placement -> group -> field.
fn encoded_group_field_site(target: u8) -> Vec<u8> {
    encoded_site(
        &[
            (SemanticStepKind::Application, APPLICATION_ID),
            (SemanticStepKind::Placement, PLACEMENT_ID),
            (SemanticStepKind::Group, [0x20; 16]),
            (SemanticStepKind::Field, [0x21; 16]),
        ],
        target,
    )
}

#[test]
fn a_whole_group_site_over_a_root_group_seals_executable_and_its_opcode_verifies() {
    // A GroupEntry site over a root-level group node seals Flat and a DurReadGroup opcode
    // over it types (`K -> Rec?`): the whole materialized group value is read as a unit
    // through the root's key-path. The read record is popped so the export return type
    // stays decoupled from the group record index.
    let (mut draft_owner, root) = group_branch_draft(false);
    let mut draft = admitted(&mut draft_owner);
    let site = book_group_site(&mut draft, &root);
    let zero = ok(draft.intern_int(0));
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurReadGroup(site),
        Instr::Pop,
        Instr::ConstLoad(zero),
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "readGroup",
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::scalar(Scalar::Int),
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "readGroup"), func);
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

#[test]
fn a_group_opcode_over_a_non_group_site_rejects() {
    // A DurReadGroup opcode requires a GroupEntry site. A forged image pointing it at a
    // field-leaf site (the root's own `title`) is refused during per-function typing
    // (`image.function`), independently of the compiler's boundary.
    let (mut draft_owner, root) = group_branch_draft(false);
    let mut draft = admitted(&mut draft_owner);
    let site = book_title_site(&mut draft, &root);
    let zero = ok(draft.intern_int(0));
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurReadGroup(site),
        Instr::Pop,
        Instr::ConstLoad(zero),
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "readGroup",
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::scalar(Scalar::Int),
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "readGroup"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

#[test]
fn a_group_scoped_field_site_seals_parked() {
    // A site over a group-scoped field leaf (`details.pages`) resolves against the
    // reconstructed node set and seals *parked* — a group leaf is not directly executable
    // as a field-leaf site; a group is read/written as a whole unit through a `GroupEntry`
    // site (group-leaf assignment lowers to a whole-group RMW). No opcode references it, so
    // the image verifies.
    let (mut draft_owner, root) = group_branch_draft(false);
    let mut draft = admitted(&mut draft_owner);
    book_group_field_site(&mut draft, &root);
    assert!(
        verify(&draft.encode().unwrap().bytes).is_ok(),
        "a group-scoped field site seals parked",
    );
}

#[test]
fn an_opcode_over_a_parked_group_field_site_rejects() {
    // A durable opcode that references a parked group-scoped field-leaf site is refused
    // during per-function typing (`image.function`), independently of the compiler's
    // boundary — a group leaf is reached only through a whole-group `GroupEntry` site, never
    // a direct field-leaf opcode.
    let (mut draft_owner, root) = group_branch_draft(false);
    let mut draft = admitted(&mut draft_owner);
    let site = book_group_field_site(&mut draft, &root);
    let code = vec![Instr::LocalGet(0), Instr::DurReadField(site), Instr::Return];
    let func = add_fn(
        &mut draft,
        "read",
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::opt_scalar(Scalar::Int),
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "read"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

#[test]
fn a_deep_nested_branch_field_site_seals_executable() {
    // A site over a keyed branch's field resolves against the reconstructed node set and
    // seals executable — a root-level unkeyed `group` (here `details`) does not park the
    // root's sibling scalar-field branches. No opcode references the site, so the image
    // verifies regardless.
    let (mut draft_owner, root) = group_branch_draft(false);
    let mut draft = admitted(&mut draft_owner);
    book_branch_field_site(&mut draft, &root);
    assert!(
        verify(&draft.encode().unwrap().bytes).is_ok(),
        "a nested branch-field site seals executable"
    );
}

#[test]
fn a_branch_record_disagreeing_with_its_member_fields_rejects() {
    // A branch's materialized record is surface (not identity), so the verifier ties
    // it to the branch's own field members — order, value shape, and required flag —
    // exactly as a root's record ties to its member tree. Here the branch record
    // marks `text` sparse while the branch member marks it required; the image
    // encodes (identity is unchanged), but the independent record/member cross-check
    // refuses it at the table phase.
    let bytes = group_branch_draft_with_branch_record(false, false)
        .0
        .encode()
        .unwrap()
        .bytes;
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Table));
}

/// Build a flat-executable `Book { title:string required }` root at `^books(id:int)`
/// whose only extra is one single-level single-column-keyed scalar-field branch,
/// `notes(noteId:string)` holding `text:string required` — the executable branch
/// shape. No group, so the root is flat-executable and its branch whole-payload
/// site seals executable. Returns the draft, its admitted root occurrence, and the
/// branch's materialized record type index (the whole branch-entry read's result type).
fn flat_branch_draft() -> (ImageDraft, AdmittedRoot, TypeId) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let book = ok(draft.intern_string("Book"));
    let title = ok(draft.intern_string("title"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    }));
    let notes = ok(draft.intern_string("notes"));
    let notes_qualified = ok(draft.intern_string("Book.notes"));
    let notes_text = ok(draft.intern_string("text"));
    let notes_record = ok(draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    }));
    let root = ok(draft.intern_string("books"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![
                field_member(shapes, None, [0x0e; 16], true, Scalar::Text),
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Branch {
                        placement: LedgerIdBytes::from_bytes([0x30; 16]),
                        name: notes,
                        record: notes_record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Text,
                            id: LedgerIdBytes::from_bytes([0x31; 16]),
                        }],
                    },
                },
                field_member(shapes, Some(1), [0x32; 16], true, Scalar::Text),
            ],
        )
        .expect("a well-formed declaration");
    let admitted = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    (draft_owner, admitted, notes_record)
}

/// The whole-payload site of the flat-branch graph's `notes` branch entry: the branch
/// is the root Product's second declared member, and a branch admits `WholePayload`.
fn flat_branch_entry_site(draft: &mut DraftTxn<'_>, root: &AdmittedRoot) -> PlannedSiteRef {
    let members = product_members(draft);
    site(
        draft,
        root.occurrence(),
        members[1].path(),
        SemanticTarget::WholePayload,
    )
}

#[test]
fn a_branch_whole_entry_read_over_a_flat_root_seals_and_type_checks() {
    // A single-level branch whole-payload site on a flat-executable root now seals
    // executable, and a read over it type-checks the two-element key-path
    // `[root_key, branch_key]` (int then string) and yields the branch's own record.
    let (mut draft_owner, root, branch_record) = flat_branch_draft();
    let mut draft = admitted(&mut draft_owner);
    let site = flat_branch_entry_site(&mut draft, &root);
    let code = vec![
        Instr::LocalGet(0), // id: the root key
        Instr::LocalGet(1), // noteId: the branch key, on top of the stack
        Instr::DurReadEntry(site),
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "note",
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
        ],
        ImageType::Record {
            idx: branch_record,
            optional: true,
        },
        2,
        code,
    );
    draft.add_export(ExportId::of_local("", "note"), func);
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

#[test]
fn a_branch_entry_op_missing_its_root_key_rejects() {
    // A branch entry op addresses `[root_key, branch_key]`. Pushing only the branch key
    // leaves the second (root) key pop with an empty stack — a key-arity forgery the
    // verifier refuses during per-function typing.
    let (mut draft_owner, root, branch_record) = flat_branch_draft();
    let mut draft = admitted(&mut draft_owner);
    let site = flat_branch_entry_site(&mut draft, &root);
    let code = vec![
        Instr::LocalGet(1), // only the branch key; the root key is missing
        Instr::DurReadEntry(site),
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "note",
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
        ],
        ImageType::Record {
            idx: branch_record,
            optional: true,
        },
        2,
        code,
    );
    draft.add_export(ExportId::of_local("", "note"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

#[test]
fn a_branch_entry_op_with_the_wrong_branch_key_type_rejects() {
    // The branch key column is `string`; pushing an `int` where the branch key belongs
    // is a type mismatch the two-element key-path check refuses.
    let (mut draft_owner, root, branch_record) = flat_branch_draft();
    let mut draft = admitted(&mut draft_owner);
    let site = flat_branch_entry_site(&mut draft, &root);
    let code = vec![
        Instr::LocalGet(0), // id: the root key (int)
        Instr::LocalGet(1), // an int where the branch key (string) belongs
        Instr::DurReadEntry(site),
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "note",
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Int),
        ],
        ImageType::Record {
            idx: branch_record,
            optional: true,
        },
        2,
        code,
    );
    draft.add_export(ExportId::of_local("", "note"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

/// An encoded site whose path is exactly `n` steps: `Application`, `Placement`, then
/// `n - 2` field steps carrying the distinctive id `0x91`, with the field-leaf target.
/// The intermediate kinds are irrelevant to the length bound.
fn encoded_n_step_site(n: usize) -> Vec<u8> {
    let mut steps = vec![
        (SemanticStepKind::Application, APPLICATION_ID),
        (SemanticStepKind::Placement, PLACEMENT_ID),
    ];
    while steps.len() < n {
        steps.push((SemanticStepKind::Field, [0x91; 16]));
    }
    encoded_site(&steps, 0x01)
}

/// The tracer image whose sparse `label` field-leaf site has been rewritten to the
/// `n`-step path above, with the encoded step count overridden to `claimed_steps`.
///
/// Every path the binder can produce is a canonical path of the declaration graph, whose
/// nesting bounds its depth, so a path at (or past) the container's own depth bound that
/// resolves to nothing exists only as forged bytes. No opcode names the label site, so the
/// forgery isolates the site table's depth gate.
fn forged_deep_site_image(n: usize, claimed_steps: usize) -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    durable_schema(&mut draft);
    let mut bytes = finish_two_key(
        draft,
        vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return],
    );
    let mut forged = encoded_n_step_site(n);
    forged[0] = claimed_steps as u8;
    forge_site(&mut bytes, &encoded_field_site(LABEL_FIELD_ID), &forged);
    bytes
}

#[test]
fn a_site_path_at_the_maximum_depth_is_admitted_by_the_bound() {
    // A site path of exactly MAX_SITE_PATH_STEPS is admitted by the length gate; it then
    // fails only at node resolution — `image.table` "names no graph node", not "too deep".
    // This pins the bound as inclusive at the maximum concrete-address depth.
    let steps = marrow_image::bounds::MAX_SITE_PATH_STEPS;
    let bytes = forged_deep_site_image(steps, steps);
    let rejection = verify(&bytes).expect_err("the unresolved maximum-depth path must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "durable site path does not resolve to a graph node",
        "the inclusive maximum must pass the depth gate and fail only at node resolution",
    );
}

#[test]
fn a_forged_zero_step_site_path_is_refused_before_any_path_body() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    durable_schema(&mut draft);
    let mut bytes = finish_two_key(
        draft,
        vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return],
    );

    let root_site = encoded_root_site();
    let mut forged = root_site.clone();
    forged[0] = 0;
    forge_site(&mut bytes, &root_site, &forged);

    let rejection = verify(&bytes).expect_err("a zero-step site path must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "durable site path names no graph node",
        "the minimum-length gate rejects before reading a step or target",
    );
}

#[test]
fn a_forged_over_deep_site_path_is_refused_by_the_verifier() {
    // The at-bound forgery with only its step-count byte bumped to one past the bound.
    // The verifier's own length check trips before it decodes any step — `image.table`
    // "durable site path too deep" — so a forged image cannot smuggle an unbounded path
    // past the container.
    let bytes = forged_deep_site_image(
        marrow_image::bounds::MAX_SITE_PATH_STEPS,
        marrow_image::bounds::MAX_SITE_PATH_STEPS + 1,
    );
    let rejection = verify(&bytes).expect_err("the forged over-deep path must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "durable site path too deep",
        "the length gate trips before any step is decoded",
    );
}

/// Flip the *last* occurrence of a 16-byte ledger id — the copy carried in the site
/// table, which follows the member tree — leaving the graph's own id intact.
fn flip_last_ledger_id(bytes: &mut [u8], id: [u8; 16]) {
    let at = bytes
        .windows(16)
        .rposition(|window| window == id)
        .expect("the ledger id appears in the image");
    bytes[at] ^= 0xFF;
}

#[test]
fn flow_mutation_outside_transaction_rejects() {
    let draft = put_export(|sites| {
        vec![
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

#[test]
fn flow_return_without_commit_rejects() {
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// Separate exits may return before begin or after commit without merging states.
#[test]
fn flow_return_before_begin_verifies() {
    let draft = put_export(|sites| {
        vec![
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry.clone()),
            Instr::JumpIfFalse(4),
            Instr::Return,
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

/// Individually legal return states must not merge into one instruction state.
#[test]
fn flow_before_begin_and_after_commit_join_rejects() {
    let draft = put_export(|sites| {
        vec![
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry.clone()),
            Instr::JumpIfFalse(8),
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::Jump(8),
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

#[test]
fn flow_double_commit_rejects() {
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// An in-region `return` on a guarded branch verifies
/// when a `TxnCommit` precedes the `Return` on that path. The present edge commits and
/// returns (indices 4–5); the absent edge writes, then commits at the closing brace and
/// returns (indices 9–10). The flow lattice admits this because every return is reached
/// in the `AfterCommit` state — it verifies the commit-before-return ordering the
/// lowering places rather than trusting it. The sibling
/// [`flow_return_without_commit_rejects`] pins the tamper: a return that skips the
/// commit is refused.
#[test]
fn flow_in_region_return_commits_then_returns_verifies() {
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry.clone()),
            Instr::JumpIfFalse(6),
            Instr::TxnCommit,
            Instr::Return,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

#[test]
fn flow_double_begin_rejects() {
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// A durable operation after the commit is refused: the commit consumes the
/// session's engine transaction, so a read placed after it would reach a dead
/// transaction at runtime. Here the tape writes, commits, then reads the value
/// field — the flow lattice rejects the post-commit read.
#[test]
fn flow_durable_read_after_commit_rejects() {
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::LocalGet(0),
            Instr::DurReadField(sites.value.clone()),
            Instr::Pop,
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// A durable mutation after the commit is refused, the write sibling of
/// [`flow_durable_read_after_commit_rejects`]: the commit consumes the session's
/// engine transaction, so a second write sits outside any live region and the flow
/// lattice rejects it.
#[test]
fn flow_mutation_after_commit_rejects() {
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// Encode a mutating helper `writer(k:string, v:int)` that sets the required field
/// with no transaction markers of its own, plus an exported caller `put(k:string,
/// v:int)` whose body `caller_body` receives the field-leaf site index and the
/// helper's function index. Only the caller is an export entry; the helper is an
/// ordinary mutating helper. The two-function shape is what proves the verifier
/// reconstructs the mutation closure across the call graph rather than trusting a
/// per-function summary.
fn mutating_helper_and_caller(
    caller_body: impl FnOnce(PlannedSiteRef, u16) -> Vec<Instr>,
) -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let two_keys = || {
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ]
    };
    let helper_code = vec![
        Instr::LocalGet(0),
        Instr::DurEraseEntry(sites.entry.clone()),
        Instr::Return,
    ];
    let helper = add_fn(
        &mut draft,
        "writer",
        two_keys(),
        ImageType::Unit,
        2,
        helper_code,
    );
    let caller_code = caller_body(sites.value, helper.index());
    let caller = add_fn(
        &mut draft,
        "put",
        two_keys(),
        ImageType::Unit,
        2,
        caller_code,
    );
    draft.add_export(ExportId::of_local("", "e"), caller);
    draft.encode().unwrap().bytes
}

/// A mutating helper called from an export with no ambient transaction rejects at
/// verify: the export's mutation closure includes the helper's write, so the flow
/// lattice sees a mutating call outside any region. This is the artifact half of the
/// requires-ambient-transaction invariant the checker enforces at check time — the
/// verifier re-derives the same closure from the opcodes and call graph, so a
/// tampered image that reached verify without the checker's refusal is still refused.
#[test]
fn flow_mutating_helper_called_outside_transaction_rejects() {
    let bytes = mutating_helper_and_caller(|_value_site, helper| {
        vec![
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::Call(helper),
            Instr::Return,
        ]
    });
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

/// The positive control: the same helper call wrapped in the caller's own
/// `transaction` region verifies. Rejection above is owed to the missing region, not
/// to the cross-function call itself.
#[test]
fn flow_mutating_helper_inside_transaction_verifies() {
    let bytes = mutating_helper_and_caller(|_value_site, helper| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::Call(helper),
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(verdict_of(&bytes), Verified);
}

/// A transaction owner may not be called, isolated to the call rule. Unlike
/// [`flow_transaction_owner_may_not_be_called_rejects`], whose owner is a non-export
/// (so the marker-outside-owning-export rule could also fire), here both functions
/// are exports and `owner` is a fully valid owner — its own region opens once and
/// commits on every path. The only violation is that `caller` invokes it, so the
/// rejection pins the call rule alone: a mutating export owns exactly one region and
/// is never re-entered through a call.
#[test]
fn flow_calling_a_valid_owner_export_rejects() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let two_keys = || {
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ]
    };
    let owner_code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurEraseEntry(sites.entry),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let owner = add_fn(
        &mut draft,
        "owner",
        two_keys(),
        ImageType::Unit,
        2,
        owner_code,
    );
    let caller_code = vec![
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::Call(owner.index()),
        Instr::Return,
    ];
    let caller = add_fn(
        &mut draft,
        "caller",
        two_keys(),
        ImageType::Unit,
        2,
        caller_code,
    );
    draft.add_export(ExportId::of_local("", "owner"), owner);
    draft.add_export(ExportId::of_local("", "caller"), caller);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// The well-formed shape: `if exists(p)` (LocalGet(S); DurExists(entry);
/// JumpIfFalse) dominates the strict set on its present edge, so the present-entry
/// sparse set verifies. The positive control the presence-lattice hostiles perturb.
#[test]
fn a_guarded_strict_sparse_set_verifies() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    // JumpIfFalse targets the TxnCommit at instruction index 6 (the guard's absent
    // edge); the encoder maps the index to a byte offset.
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(6),
            Instr::ConstLoad(text),
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Verified);
}

/// A strict present-entry sparse set with no dominating presence fact on its key
/// slot is refused at the flow phase, independently of the compiler.
#[test]
fn a_strict_sparse_set_without_a_presence_fact_rejects() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::ConstLoad(text),
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

/// The presence fact is proven for the guarded slot only: a strict set that names a
/// different, unproven key slot is refused even though that slot is initialized and
/// key-typed. This is the mutated-place-slot-index gate.
#[test]
fn a_strict_sparse_set_naming_an_unproven_slot_rejects() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(6),
            Instr::ConstLoad(text),
            // Slot 0 is proven present by the guard; naming slot 1 is unproven.
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![1],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

/// A presence fact established before a loop does not survive the loop header when the
/// body erases the entry through the same key slot. The header is a merge of the
/// pre-loop edge (slot 0 proven present by the `exists` guard) and the back edge (slot
/// 0 erased in the body); the intersection-join kills the fact, so a strict set placed
/// after the loop is refused at the flow phase. This pins the backedge kill explicitly:
/// without intersection at the header the stale pre-loop fact would wrongly dominate.
#[test]
fn a_strict_sparse_set_after_a_loop_that_erases_the_entry_rejects() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    // Instruction-index layout (targets are draft-form indices):
    //   0 TxnBegin
    //   1 LocalGet(0); 2 DurExists(0); 3 JumpIfFalse(14) — present edge proves slot 0.
    //   4 loop header (merge of the pre-loop edge and the back edge at 9).
    //   4 LocalGet(1); 5 DurExists(0); 6 JumpIfFalse(10) — loop-continue test on slot 1.
    //   7 LocalGet(0); 8 DurEraseEntry(0) — body erases the entry keyed by slot 0.
    //   9 Jump(4) — back edge; slot 0 is absent on this edge.
    //   10 ConstLoad; 11 field set on slot 0 (rejected: killed by the header
    //   intersection).
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry.clone()),
            Instr::JumpIfFalse(14),
            Instr::LocalGet(1),
            Instr::DurExists(sites.entry.clone()),
            Instr::JumpIfFalse(10),
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry),
            Instr::Jump(4),
            Instr::ConstLoad(text),
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

/// A presence fact established on a key slot is dead once that slot is rebound — the exact
/// interaction a two-binding traversal relies on. The traversal rebinds its key slot each
/// iteration (`ListGet; LocalSet(key_slot)`), so a fact an `exists` guard proved in
/// iteration N must not dominate a strict present-entry set in iteration N+1. Model the
/// rebind directly: `exists` proves slot 0 present, a `LocalSet` rebinds slot 0, and a
/// strict set relying on the stale fact is refused at the flow phase.
#[test]
fn a_strict_sparse_set_after_a_key_rebind_rejects() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    // Instruction-index layout:
    //   0 TxnBegin
    //   1 LocalGet(0); 2 DurExists(0); 3 JumpIfFalse(8) — present edge proves slot 0.
    //   4 LocalGet(1); 5 LocalSet(0) — rebind slot 0 to the next iteration's key.
    //   6 ConstLoad; 7 field set on slot 0 (rejected: killed by the rebind).
    //   8 TxnCommit; 9 Return.
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(8),
            Instr::LocalGet(1),
            Instr::LocalSet(0),
            Instr::ConstLoad(text),
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

/// The tracer schema plus a string-keyed `notes(noteId:string)` branch of one required
/// `text` field. The branch key is `string` to match the root key type, so a strict
/// root-field set naming the branch key slot type-checks — isolating the presence
/// lattice as the sole gate.
struct BranchPresenceSchema {
    owner: ImageDraft,
    root: tracer_schema::Sites,
    entry: PlannedSiteRef,
    field: PlannedSiteRef,
    record: TypeId,
}

fn branch_presence_schema() -> BranchPresenceSchema {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let counter = ok(draft.intern_string("Counter"));
    let value = ok(draft.intern_string("value"));
    let label = ok(draft.intern_string("label"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    }));
    let notes = ok(draft.intern_string("notes"));
    let notes_qualified = ok(draft.intern_string("Counter.notes"));
    let notes_text = ok(draft.intern_string("text"));
    let notes_record = ok(draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    }));
    let root = ok(draft.intern_string("counters"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let admitted = declare_counters_with_notes_branch(
        &mut draft,
        root,
        record,
        notes,
        notes_record,
        /* branch_field_required */ true,
    );
    let members = product_members(&draft);
    let branch_path = members[2].path().clone();
    let root_entry = site(
        &mut draft,
        admitted.occurrence(),
        admitted.placement_path(),
        SemanticTarget::WholePayload,
    );
    let root_value = site(
        &mut draft,
        admitted.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let label_site = site(
        &mut draft,
        admitted.occurrence(),
        members[1].path(),
        SemanticTarget::FieldLeaf,
    );
    let branch_entry = site(
        &mut draft,
        admitted.occurrence(),
        &branch_path,
        SemanticTarget::WholePayload,
    );
    let branch_field_path = draft.members_of(&branch_path).expect("branch members")[0]
        .path()
        .clone();
    let branch_field = site(
        &mut draft,
        admitted.occurrence(),
        &branch_field_path,
        SemanticTarget::FieldLeaf,
    );
    draft.commit();
    BranchPresenceSchema {
        owner: draft_owner,
        root: tracer_schema::Sites {
            record,
            entry: root_entry,
            value: root_value,
            label: label_site,
        },
        entry: branch_entry,
        field: branch_field,
        record: notes_record,
    }
}

/// Declare the tracer `Counter` Product extended with a string-keyed `notes` branch of
/// one `text:string` field, and admit the `^counters(name:string)` root over it. The
/// branch field's required flag is the caller's, so a fixture can make the branch sparse.
fn declare_counters_with_notes_branch(
    draft: &mut DraftTxn<'_>,
    root: marrow_image::StrId,
    record: marrow_image::TypeId,
    notes: marrow_image::StrId,
    notes_record: marrow_image::TypeId,
    branch_field_required: bool,
) -> AdmittedRoot {
    let shapes = scalar_shapes(draft);
    let mut members = counters_members(shapes);
    members.push(DeclarationMemberDef {
        parent: None,
        shape: DeclarationMemberShape::Branch {
            placement: LedgerIdBytes::from_bytes([0x30; 16]),
            name: notes,
            record: notes_record,
            keys: vec![KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x31; 16]),
            }],
        },
    });
    members.push(field_member(
        shapes,
        Some(2),
        [0x32; 16],
        branch_field_required,
        Scalar::Text,
    ));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            members,
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Text,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared")
}

/// A branch whole-entry create does not establish root-entry presence: it leaves the
/// root descendant-only, so its marker is still absent. A forged image that creates a
/// branch entry keyed by slot 1 and then does a strict present-entry root-field set
/// naming slot 1 — relying on the branch create to dominate it — is refused at the flow
/// phase. Without the `is_entry_site` gate in the presence lattice the branch create
/// would wrongly mark slot 1 present and this image would verify. The branch key is
/// `string` (the root key type), so the strict set type-checks and the presence lattice
/// is the sole gate.
#[test]
fn a_branch_create_does_not_dominate_a_strict_root_field_set_rejects() {
    let BranchPresenceSchema {
        owner: mut draft_owner,
        root,
        entry: branch_entry,
        record: notes_record,
        ..
    } = branch_presence_schema();
    let mut draft = admitted(&mut draft_owner);
    let text = ok(draft.intern_text("t"));
    // Slots: 0 = root key (string param), 1 = branch key (string param), 2 = the branch
    // record local (so the create matches the `LocalGet(rec); LocalGet(key)` shape the
    // presence lattice keys on).
    let code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(text),
        Instr::RecordNew(notes_record),
        Instr::LocalSet(2),
        Instr::LocalGet(0), // root key
        Instr::LocalGet(1), // branch key
        Instr::LocalGet(2), // branch record
        Instr::DurCreateEntry(branch_entry),
        Instr::ConstLoad(text),
        // Claims slot 1's *root* entry is present, relying on the branch create above.
        Instr::DurSetField {
            site: root.label,
            key_slots: vec![1],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "e",
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        ImageType::Unit,
        3,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// The tracer schema plus a string-keyed `notes(noteId:string)` branch of one *sparse*
/// `body:text` field, and a site on that branch field. The branch key is `string` (the
/// root key type) so a strict set naming the root key slot type-checks, and the field is
/// sparse so it clears the required-field gate — isolating the site-target check as the
/// sole remaining gate. Returns (draft, root whole-payload site operand, branch-field site
/// operand).
fn branch_field_schema() -> (ImageDraft, PlannedSiteRef, PlannedSiteRef) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let counter = ok(draft.intern_string("Counter"));
    let value = ok(draft.intern_string("value"));
    let label = ok(draft.intern_string("label"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    }));
    let notes = ok(draft.intern_string("notes"));
    let notes_qualified = ok(draft.intern_string("Counter.notes"));
    let notes_body = ok(draft.intern_string("body"));
    let notes_record = ok(draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_body,
            ty: ImageType::scalar(Scalar::Text),
            required: false,
        }],
    }));
    let root = ok(draft.intern_string("counters"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let admitted = declare_counters_with_notes_branch(
        &mut draft,
        root,
        record,
        notes,
        notes_record,
        /* branch_field_required */ false,
    );
    let branch_path = product_members(&draft)[2].path().clone();
    let body_path = draft
        .members_of(&branch_path)
        .expect("the declaration row is live")[0]
        .path()
        .clone();
    let root_entry = site(
        &mut draft,
        admitted.occurrence(),
        admitted.placement_path(),
        SemanticTarget::WholePayload,
    );
    let branch_field = site(
        &mut draft,
        admitted.occurrence(),
        &body_path,
        SemanticTarget::FieldLeaf,
    );
    draft.commit();
    (draft_owner, root_entry, branch_field)
}

/// A branch-field site's key-path is the two-element `[root_key, branch_key]`. The strict
/// present-entry sparse set carries one slot per key column, so a forged image that proves
/// the root entry present with an `exists` guard and then drives the strict set over a
/// branch-field site supplying only the *root* key (one slot) is a key-path arity mismatch
/// and must be refused at the function phase. Accepting it would let the kernel drop the
/// branch hop and mis-address the write. The correct two-slot branch strict set is
/// admitted, and is exercised through the production path.
#[test]
fn a_strict_sparse_set_over_a_branch_field_with_a_single_root_key_rejects() {
    let (mut draft_owner, root_entry, branch_field) = branch_field_schema();
    let mut draft = admitted(&mut draft_owner);
    let text = ok(draft.intern_text("x"));
    // Slot 0 is the root key (string param). The guard `LocalGet(0); DurExists(root
    // whole payload); JumpIfFalse` proves slot 0's root entry present on its taken edge;
    // the strict set then names the branch-field site with only that one slot — a
    // one-element key-path over a two-element branch-field site.
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurExists(root_entry),
        Instr::JumpIfFalse(6),
        Instr::ConstLoad(text),
        Instr::DurSetField {
            site: branch_field,
            key_slots: vec![0],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "e",
        vec![ImageType::scalar(Scalar::Text)],
        ImageType::Unit,
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

/// A two-slot branch-field strict set with the correct `[root, branch]` key-path arity and
/// key types, but with no `exists`/`if const` guard dominating it, passes the arity and
/// type checks yet is refused at the flow phase: the key-path presence lattice holds no
/// fact for the branch entry `[0, 1]`. This isolates the presence requirement of the
/// generalized (branch) strict form from its arity/type gate.
#[test]
fn a_two_slot_branch_strict_set_without_a_presence_fact_rejects() {
    let (mut draft_owner, _root_entry, branch_field) = branch_field_schema();
    let mut draft = admitted(&mut draft_owner);
    let text = ok(draft.intern_text("x"));
    let code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(text),
        // Slots 0,1 are the [root, branch] key params — arity- and type-correct for the
        // branch-field site — but no guard proves the branch entry present.
        Instr::DurSetField {
            site: branch_field,
            key_slots: vec![0, 1],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "e",
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        ImageType::Unit,
        2,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

#[test]
fn flow_transaction_owner_may_not_be_called_rejects() {
    // A helper owns a transaction (contains TxnBegin); an export that calls it is a
    // flow violation — helpers cannot own the transaction.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let key = ok(draft.intern_text("x"));
    let helper_code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(key),
        Instr::DurEraseEntry(sites.entry),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let helper = add_fn(
        &mut draft,
        "helper",
        Vec::new(),
        ImageType::Unit,
        0,
        helper_code,
    );
    let main_code = vec![Instr::Call(helper.index()), Instr::Return];
    let main = add_fn(
        &mut draft,
        "main",
        Vec::new(),
        ImageType::Unit,
        0,
        main_code,
    );
    draft.add_export(ExportId::of_local("", "main"), main);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

#[test]
fn erase_field_on_a_required_field_rejects_at_function() {
    // A required field is present whenever its entry is; erasing the required `value`
    // field on its own is a phase-3 site/target error.
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseField(sites.value.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

#[test]
fn create_on_a_field_site_rejects_at_function() {
    // `create` requires an entry site; a field-target site is a phase-3 error.
    let draft = put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurCreateEntry(sites.value.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    });
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

// --- TEST-ENTRY section and `assert` hostiles. ---

/// A well-formed image with one storeless test entry whose body asserts a
/// constant true, returning the draft and the test function's id. The
/// TEST-ENTRY hostiles derive from this.
fn test_entry_image() -> (ImageDraft, FuncId) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let title = ok(draft.intern_string("holds"));
    let truth = ok(draft.intern_bool(true));
    let code = vec![Instr::ConstLoad(truth), Instr::Assert, Instr::Return];
    let func = add_fn(&mut draft, "holds", Vec::new(), ImageType::Unit, 0, code);
    draft.add_test_entry(title, func);
    draft.commit();
    (draft_owner, func)
}

#[test]
fn assert_in_a_test_entry_verifies() {
    // The well-formed baseline the TEST-ENTRY hostiles derive from.
    let (draft, _) = test_entry_image();
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

#[test]
fn assert_on_a_non_bool_operand_rejects_at_function() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let title = ok(draft.intern_string("holds"));
    let seven = ok(draft.intern_int(7));
    let code = vec![Instr::ConstLoad(seven), Instr::Assert, Instr::Return];
    let func = add_fn(&mut draft, "holds", Vec::new(), ImageType::Unit, 0, code);
    draft.add_test_entry(title, func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

#[test]
fn test_entry_may_carry_durable_demand() {
    // A private reader carries the test's durable demand without adding an export.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let title = ok(draft.intern_string("holds"));
    let key = ok(draft.intern_text("x"));
    let code = vec![
        Instr::ConstLoad(key),
        Instr::DurExists(sites.entry),
        Instr::Return,
    ];
    let reader = add_fn(
        &mut draft,
        "holds",
        Vec::new(),
        ImageType::scalar(Scalar::Bool),
        0,
        code,
    );
    let code = vec![Instr::Call(reader.index()), Instr::Assert, Instr::Return];
    let func = add_fn(&mut draft, "holds", Vec::new(), ImageType::Unit, 0, code);
    draft.add_test_entry(title, func);
    let image = verify(&draft.encode().unwrap().bytes).expect("durable test entry verifies");
    let entry = &image.test_entries()[0];
    assert_eq!(entry.name(), "holds");
    // A presence probe on the root: the test-image demand union is nonempty and
    // reads without writing.
    let union = image.test_demand_union();
    assert!(!union.is_empty());
    assert!(union.reads());
    assert!(!union.writes());
    // A test entry is never an export.
    assert!(image.exports().is_empty());
}

/// A two-test image whose bodies carry no `Assert`, so the TEST-ENTRY aliasing rule is
/// the only check a patched row can trip.
fn assert_free_test_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    for title_text in ["alpha", "beta"] {
        let title = ok(draft.intern_string(title_text));
        let func = add_fn(
            &mut draft,
            title_text,
            Vec::new(),
            ImageType::Unit,
            0,
            vec![Instr::Return],
        );
        draft.add_test_entry(title, func);
    }
    draft.encode().expect("encode").bytes
}

#[test]
fn rehashed_direct_durable_test_operations_reject_at_test_entry() {
    for mutates in [false, true] {
        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let sites = durable_schema(&mut draft);
        let title = ok(draft.intern_string("holds"));
        let key = ok(draft.intern_text("x"));
        let mut code = vec![Instr::ConstLoad(key)];
        if mutates {
            let value = ok(draft.intern_int(5));
            code.extend([
                Instr::ConstLoad(value),
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Text)),
                Instr::RecordNew(sites.record),
                Instr::DurReplaceEntry(sites.entry),
            ]);
        } else {
            code.extend([Instr::DurExists(sites.entry), Instr::Pop]);
        }
        code.push(Instr::Return);
        let helper = add_fn(&mut draft, "holds", Vec::new(), ImageType::Unit, 0, code);
        let code = vec![Instr::Return];
        let test = add_fn(&mut draft, "holds", Vec::new(), ImageType::Unit, 0, code);
        draft.add_test_entry(title, test);
        let mut bytes = draft
            .encode()
            .expect("the helper is not a test entry")
            .bytes;
        let image = verify(&bytes).expect("the unused helper and storeless test are legal");
        assert!(image.test_demand_union().is_empty());
        assert!(image.exports().is_empty());
        let body = section_frame(&bytes, 8).0;
        let target = body + 4;
        assert_eq!(&bytes[target..target + 2], &test.index().to_be_bytes());
        bytes[target..target + 2].copy_from_slice(&helper.index().to_be_bytes());
        rehash(&mut bytes);
        let refusal = verify(&bytes).expect_err("a test cannot perform a durable operation");
        assert_eq!(refusal.phase(), VerifyPhase::TestEntry);
        assert_eq!(refusal.code(), VerifyPhase::TestEntry.code());
    }
}

/// A two-test image whose TEST-ENTRY section rows the byte-patch hostiles edit.
fn two_test_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let truth = ok(draft.intern_bool(true));
    for title_text in ["alpha", "beta"] {
        let title = ok(draft.intern_string(title_text));
        let code = vec![Instr::ConstLoad(truth), Instr::Assert, Instr::Return];
        let func = add_fn(&mut draft, title_text, Vec::new(), ImageType::Unit, 0, code);
        draft.add_test_entry(title, func);
    }
    draft.encode().expect("encode").bytes
}

#[test]
fn transaction_marker_in_a_test_entry_rejects_at_flow() {
    // A TxnBegin inside a test entry: a transaction marker may only sit in a
    // mutating export entry, so the flow phase rejects it before the TestEntry
    // phase ever runs.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let title = ok(draft.intern_string("holds"));
    let code = vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return];
    let func = add_fn(&mut draft, "holds", Vec::new(), ImageType::Unit, 0, code);
    draft.add_test_entry(title, func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Flow)
    );
}

/// A well-formed range guard over a bare int verifies: it peeks the int and
/// leaves the stack unchanged, so the guarded value still returns.
#[test]
fn range_guard_over_a_bare_int_verifies() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let seven = ok(draft.intern_int(7));
    let code = vec![
        Instr::ConstLoad(seven),
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::Return,
    ];
    let func = add_int_fn(&mut draft, "main", code);
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

/// A range guard with nothing on the stack rejects at per-function
/// verification: the guard peeks its operand, so an operand must exist.
#[test]
fn range_guard_on_an_empty_stack_rejects_at_function() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let seven = ok(draft.intern_int(7));
    let code = vec![
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::ConstLoad(seven),
        Instr::Return,
    ];
    let func = add_int_fn(&mut draft, "main", code);
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

/// A range guard over a non-int (here a bool) rejects at per-function
/// verification: the guarded value must be a bare int.
#[test]
fn range_guard_on_a_non_int_rejects_at_function() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let flag = ok(draft.intern_bool(true));
    let code = vec![
        Instr::ConstLoad(flag),
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "main",
        Vec::new(),
        ImageType::scalar(Scalar::Bool),
        0,
        code,
    );
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

/// A range guard whose interval is empty (`lo > hi`) rejects at decode: no
/// value satisfies it, so a compiler never emits one and an image carrying one
/// is hostile.
#[test]
fn range_guard_with_an_empty_interval_rejects_at_function() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let seven = ok(draft.intern_int(7));
    let code = vec![
        Instr::ConstLoad(seven),
        Instr::RangeGuard { lo: 5, hi: 4 },
        Instr::Return,
    ];
    let func = add_int_fn(&mut draft, "main", code);
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

/// A function body truncated mid-way through a range guard's 16-byte interval
/// immediate rejects at per-function verification (a short operand), not with
/// a panic or an out-of-bounds read. The truncation shortens the code length,
/// the section frame, and the payload consistently, then rehashes, so only the
/// operand-boundary invariant is violated.
#[test]
fn range_guard_with_a_truncated_operand_rejects_at_function() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = ok(draft.intern_string("src/main.mw"));
    let name = ok(draft.intern_string("main"));
    let seven = ok(draft.intern_int(7));
    let code = vec![
        Instr::ConstLoad(seven),
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::Return,
    ];
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            // One span at instruction 0, so span offsets stay valid after the cut.
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), func);
    let mut bytes = draft.encode().unwrap().bytes;

    // Locate the FUNCTIONS section (id 0x05) and its one function's code-length
    // field: body = u16 count, u16 name, u16 source, u8 param_count, ret tag,
    // u16 local_count, u32 code_len, code bytes.
    let (offset, len) = sections(&bytes)
        .into_iter()
        .find_map(|(id, offset, len)| (id == 0x05).then_some((offset, len)))
        .expect("functions section");
    let code_len_at = offset + 2 + 2 + 2 + 1 + 1 + 2;
    let code_len =
        u32::from_be_bytes(bytes[code_len_at..code_len_at + 4].try_into().unwrap()) as usize;
    let code_end = code_len_at + 4 + code_len;
    assert_eq!(code_end, offset + len, "one function fills the section");

    // Cut the final 9 bytes: the Return opcode plus the interval's second i64,
    // leaving the RangeGuard opcode with a short immediate at end of code.
    let cut = 9;
    bytes.drain(code_end - cut..code_end);
    let new_code_len = (code_len - cut) as u32;
    bytes[code_len_at..code_len_at + 4].copy_from_slice(&new_code_len.to_be_bytes());
    let new_section_len = (len - cut) as u32;
    bytes[offset - 4..offset].copy_from_slice(&new_section_len.to_be_bytes());
    rehash(&mut bytes);

    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Function));
}

// --- record-typed parameter and return references (V3b) ---

/// A one-int-field record type, a function that constructs and returns it, and a
/// function that takes it by value and reads a field all verify — the well-formed
/// baseline the record-ref hostiles derive from.
#[test]
fn record_param_and_return_refs_verify() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let field = ok(draft.intern_string("x"));
    let rec = ok(draft.add_record_type(RecordTypeDef {
        name: field,
        fields: vec![FieldDef {
            name: field,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    }));
    let zero = ok(draft.intern_int(0));
    let make_code = vec![Instr::ConstLoad(zero), Instr::RecordNew(rec), Instr::Return];
    add_fn(
        &mut draft,
        "make",
        Vec::new(),
        ImageType::Record {
            idx: rec,
            optional: false,
        },
        0,
        make_code,
    );
    let take_code = vec![Instr::LocalGet(0), Instr::FieldGet(0), Instr::Return];
    let take = add_fn(
        &mut draft,
        "take",
        vec![ImageType::Record {
            idx: rec,
            optional: false,
        }],
        ImageType::scalar(Scalar::Int),
        1,
        take_code,
    );
    draft.add_export(ExportId::of_local("", "take"), take);
    assert_eq!(verdict_of(&draft.encode().unwrap().bytes), Verified);
}

/// An optional parameter type is outside the parameter subset and rejects at the
/// table phase.
#[test]
fn optional_parameter_type_rejects() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let code = vec![Instr::Return];
    let f = add_fn(
        &mut draft,
        "f",
        vec![ImageType::opt_scalar(Scalar::Int)],
        ImageType::Unit,
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "f"), f);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Table)
    );
}

// --- Record field-type table hostiles: a field type is a bare scalar or enum. ---

/// Build a minimal image whose single record type's fields come from `fields`,
/// plus a trivial `fn f(): int` export. The record table decodes before the
/// function, so a malformed field type rejects at the table phase.
fn record_table_image(fields: impl FnOnce(&mut DraftTxn<'_>) -> Vec<FieldDef>) -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let rname = ok(draft.intern_string("R"));
    let field_defs = fields(&mut draft);
    ok(draft.add_record_type(RecordTypeDef {
        name: rname,
        fields: field_defs,
    }));
    let zero = ok(draft.intern_int(0));
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let func = add_int_fn(&mut draft, "f", code);
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn record_field_with_optional_type_rejects() {
    // A field type is bare; an optional flag on it rejects at the table phase.
    let bytes = record_table_image(|draft| {
        let name = ok(draft.intern_string("x"));
        vec![FieldDef {
            name,
            ty: ImageType::opt_scalar(Scalar::Int),
            required: true,
        }]
    });
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Table));
}

#[test]
fn value_type_cycle_through_a_record_field_rejects() {
    // Record R (idx 0) has an enum field E (idx 0), whose one variant carries a
    // Record(0) payload: a value type that contains itself. The combined
    // record+enum acyclicity pass rejects it.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let rname = ok(draft.intern_string("R"));
    let ename = ok(draft.intern_string("E"));
    let vname = ok(draft.intern_string("wrap"));
    let fname = ok(draft.intern_string("inner"));
    ok(draft.add_record_type(RecordTypeDef {
        name: rname,
        fields: vec![FieldDef {
            name: fname,
            ty: ImageType::Enum {
                idx: marrow_image::EnumId::from_index(0),
                optional: false,
            },
            required: true,
        }],
    }));
    ok(draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: vec![ImageType::Record {
                idx: marrow_image::TypeId::from_index(0),
                optional: false,
            }],
        }],
    }));
    let zero = ok(draft.intern_int(0));
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let func = add_int_fn(&mut draft, "f", code);
    draft.add_export(ExportId::of_local("", "f"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Table)
    );
}

/// Add a trivial `fn f(): int` export to `draft`, encode, and report the verifier's
/// verdict. Shared by the value-graph hostiles below, which populate the record and enum
/// tables before calling this.
fn value_graph_verdict(draft: &mut DraftTxn<'_>) -> Verdict {
    let zero = ok(draft.intern_int(0));
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let func = add_int_fn(draft, "f", code);
    draft.add_export(ExportId::of_local("", "f"), func);
    verdict_of(&draft.encode().unwrap().bytes)
}

#[test]
fn self_referential_record_field_rejects() {
    // Record 0 has a field of type Record(0): a value that directly contains itself.
    let bytes = record_table_image(|draft| {
        let name = ok(draft.intern_string("me"));
        vec![FieldDef {
            name,
            ty: ImageType::Record {
                idx: marrow_image::TypeId::from_index(0),
                optional: false,
            },
            required: true,
        }]
    });
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Table));
}

#[test]
fn value_type_cycle_through_two_records_rejects() {
    // Record 0 has a field of type Record(1) and Record 1 a field of type Record(0):
    // a struct-to-struct cycle the widened record-field edge now catches.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let a = ok(draft.intern_string("A"));
    let b = ok(draft.intern_string("B"));
    let fb = ok(draft.intern_string("b"));
    let fa = ok(draft.intern_string("a"));
    ok(draft.add_record_type(RecordTypeDef {
        name: a,
        fields: vec![FieldDef {
            name: fb,
            ty: ImageType::Record {
                idx: marrow_image::TypeId::from_index(1),
                optional: false,
            },
            required: true,
        }],
    }));
    ok(draft.add_record_type(RecordTypeDef {
        name: b,
        fields: vec![FieldDef {
            name: fa,
            ty: ImageType::Record {
                idx: marrow_image::TypeId::from_index(0),
                optional: false,
            },
            required: true,
        }],
    }));
    assert_eq!(value_graph_verdict(&mut draft), Refused(VerifyPhase::Table));
}

#[test]
fn self_referential_enum_payload_rejects() {
    // Enum 0's one variant carries a payload of type Enum(0): a value that contains
    // itself with no record on the cycle.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let ename = ok(draft.intern_string("E"));
    let vname = ok(draft.intern_string("wrap"));
    ok(draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: vec![ImageType::Enum {
                idx: marrow_image::EnumId::from_index(0),
                optional: false,
            }],
        }],
    }));
    assert_eq!(value_graph_verdict(&mut draft), Refused(VerifyPhase::Table));
}

#[test]
fn value_type_cycle_through_mixed_records_and_enums_rejects() {
    // A three-hop cycle Record0 -> Enum0 -> Record1 -> Record0 mixing record fields
    // and an enum payload leaf.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let r0 = ok(draft.intern_string("R0"));
    let r1 = ok(draft.intern_string("R1"));
    let ename = ok(draft.intern_string("E"));
    let vname = ok(draft.intern_string("wrap"));
    let f_e = ok(draft.intern_string("e"));
    let f_back = ok(draft.intern_string("back"));
    ok(draft.add_record_type(RecordTypeDef {
        name: r0,
        fields: vec![FieldDef {
            name: f_e,
            ty: ImageType::Enum {
                idx: marrow_image::EnumId::from_index(0),
                optional: false,
            },
            required: true,
        }],
    }));
    ok(draft.add_record_type(RecordTypeDef {
        name: r1,
        fields: vec![FieldDef {
            name: f_back,
            ty: ImageType::Record {
                idx: marrow_image::TypeId::from_index(0),
                optional: false,
            },
            required: true,
        }],
    }));
    ok(draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: vec![ImageType::Record {
                idx: marrow_image::TypeId::from_index(1),
                optional: false,
            }],
        }],
    }));
    assert_eq!(value_graph_verdict(&mut draft), Refused(VerifyPhase::Table));
}

#[test]
fn enum_payload_with_a_collection_leaf_rejects_at_table() {
    // A collection tag (`TAG_COLLECTION`, 0x07) is representable in an enum-variant
    // payload-leaf byte via `ImageType::Collection`, and `encode_enums` writes it
    // verbatim with an encoder-computed digest. The independent verifier's Table-phase
    // `decode_bare_payload_type` admits only a bare scalar, record, or enum leaf, so its
    // `_ =>` arm refuses the collection tag before the COLLTYPES index is even read.
    // This pins the retained defense-in-depth for the checker's construction/annotation-
    // site `check.unsupported` refusal of collection-typed generic-enum payloads: a
    // checker-clean program can never mint this leaf, and a hand-forged image carrying
    // it is refused at decode rather than at run time.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let ename = ok(draft.intern_string("E"));
    let vname = ok(draft.intern_string("hold"));
    // The collection row exists, so the ordinal is in range and the producer's
    // coherence walk admits it: the KIND law stays the verifier's own.
    ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Int),
    }));
    ok(draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: vec![ImageType::Collection {
                idx: marrow_image::CollTypeId::from_index(0),
                optional: false,
            }],
        }],
    }));
    let zero = ok(draft.intern_int(0));
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let func = add_int_fn(&mut draft, "f", code);
    draft.add_export(ExportId::of_local("", "f"), func);
    let bytes = draft
        .encode()
        .expect("encode collection-payload enum")
        .bytes;
    let rejection = verify(&bytes).expect_err("a collection enum-payload leaf is refused");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "enum payload leaf type tag is not admitted here"
    );
}

#[test]
fn deep_acyclic_record_chain_verifies() {
    // A long but acyclic chain Record0 -> Record1 -> ... -> Record(N-1) verifies:
    // depth is not a restriction, only cycles are.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    const N: u16 = 24;
    for i in 0..N {
        let name = ok(draft.intern_string(&format!("R{i}")));
        let fields = if i + 1 < N {
            let fname = ok(draft.intern_string("next"));
            vec![FieldDef {
                name: fname,
                ty: ImageType::Record {
                    idx: marrow_image::TypeId::from_index(i + 1),
                    optional: false,
                },
                required: true,
            }]
        } else {
            let fname = ok(draft.intern_string("v"));
            vec![FieldDef {
                name: fname,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }]
        };
        ok(draft.add_record_type(RecordTypeDef { name, fields }));
    }
    assert_eq!(value_graph_verdict(&mut draft), Verified);
}

/// Two payloadless members with distinct ids, matching a `reader`/`writer` enum.
const ACCESS_READER_ID: [u8; 16] = [0x51; 16];

fn access_members() -> Vec<[u8; 16]> {
    vec![ACCESS_READER_ID, [0x52; 16]]
}

/// A valid widened durable image: `Widget { id:int required, kind:Access required }`
/// stored at `^widgets(id:int)`, where `Access` is a two-variant payloadless enum.
/// The `kind` field's durable value shape is a closed enum carrying a sum id and one
/// member id per variant, so the member tree matches the materialized record's
/// widened value shape. These fixtures exercise the widened durable member shape at
/// seal, so the root carries no operation sites and a pure function completes the
/// storeless image; the widened field's executability is covered elsewhere.
fn widened_draft(members: Vec<[u8; 16]>) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    // The `kind` field's value shape: a two-variant payloadless enum with a sum id and
    // one member id per variant, minted into this draft's own arena.
    let kind_value = draft
        .value_enum(
            LedgerIdBytes::from_bytes([0x50; 16]),
            members
                .iter()
                .map(|member| (LedgerIdBytes::from_bytes(*member), Vec::new()))
                .collect(),
        )
        .expect("a within-bounds shape appends");
    let access = ok(draft.intern_string("Access"));
    let reader = ok(draft.intern_string("reader"));
    let writer = ok(draft.intern_string("writer"));
    ok(draft.add_enum_type(EnumTypeDef {
        name: access,
        variants: vec![
            VariantDef {
                name: reader,
                category: false,
                payload: Vec::new(),
            },
            VariantDef {
                name: writer,
                category: false,
                payload: Vec::new(),
            },
        ],
    }));
    let widget = ok(draft.intern_string("Widget"));
    let idn = ok(draft.intern_string("id"));
    let kindn = ok(draft.intern_string("kind"));
    let rec = ok(draft.add_record_type(RecordTypeDef {
        name: widget,
        fields: vec![
            FieldDef {
                name: idn,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: kindn,
                ty: ImageType::Enum {
                    idx: marrow_image::EnumId::from_index(0),
                    optional: false,
                },
                required: true,
            },
        ],
    }));
    let root = ok(draft.intern_string("widgets"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            rec,
            vec![
                field_member(shapes, None, VALUE_FIELD_ID, true, Scalar::Int),
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(LABEL_FIELD_ID),
                        required: true,
                        value: kind_value,
                    },
                },
            ],
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let zero = ok(draft.intern_int(0));
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let func = add_int_fn(&mut draft, "f", code);
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.commit();
    draft_owner
}

/// The valid widened durable image, encoded.
fn widened_image() -> Vec<u8> {
    widened_draft(access_members())
        .encode()
        .expect("encode")
        .bytes
}

#[test]
fn a_widened_enum_field_image_verifies() {
    // A durable resource with a closed-enum field is now identity-complete and
    // verifies when its member tree's value shape matches the record's enum field.
    assert_eq!(
        verdict_of(&widened_draft(access_members()).encode().unwrap().bytes),
        Verified
    );
}

#[test]
fn forged_duplicate_enum_member_id_rejects() {
    // Entropy-minted ids are pairwise distinct; two members claiming one id forge a
    // duplicate identity in the durable table and reject before the contract
    // recomputation, so a hostile image cannot alias two members to one code.
    let dup = vec![[0x51; 16], [0x51; 16]];
    assert_eq!(
        verdict_of(&widened_draft(dup).encode().unwrap().bytes),
        Refused(VerifyPhase::Table)
    );
}

#[test]
fn enum_value_shape_that_mismatches_the_record_rejects() {
    // The member tree's value shape must match the materialized record's enum field.
    // A value shape with three members when the enum table has two variants cannot be
    // reconciled, so the cross-check rejects — a hostile image cannot claim one
    // durable identity while its executable record carries a different value shape.
    let mut extra = access_members();
    extra.push([0x53; 16]);
    assert_eq!(
        verdict_of(&widened_draft(extra).encode().unwrap().bytes),
        Refused(VerifyPhase::Table)
    );
}

// --- Nested-branch admission hostiles ---
//
// A nested single-column scalar-field branch is executable: `^books(id).notes(nid).tags(tid)`
// addresses a durable node two levels below the root. The verifier resolves a branch site's
// path level by level through the reconstructed member tree; a path that routes a branch
// under a field, or names a branch that does not exist at its level, resolves to no branch
// and seals *parked*, so a durable opcode over it is refused rather than mis-addressed.

/// Build a flat-executable `Book { title }` root at `^books(id:int)` whose `notes` branch
/// (`noteId:string`, `text:string required`) itself holds a nested `tags` branch
/// (`tagId:int`, `weight:int required`) — the executable nested-branch shape. The verifier
/// seals the whole recursive branch tree, so a valid deep site seals executable.
fn nested_branch_draft() -> (ImageDraft, AdmittedRoot) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let book = ok(draft.intern_string("Book"));
    let title = ok(draft.intern_string("title"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    }));
    let notes = ok(draft.intern_string("notes"));
    let notes_qualified = ok(draft.intern_string("Book.notes"));
    let notes_text = ok(draft.intern_string("text"));
    let notes_record = ok(draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    }));
    let tags = ok(draft.intern_string("tags"));
    let tags_qualified = ok(draft.intern_string("Book.notes.tags"));
    let tags_weight = ok(draft.intern_string("weight"));
    let tags_record = ok(draft.add_record_type(RecordTypeDef {
        name: tags_qualified,
        fields: vec![FieldDef {
            name: tags_weight,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    }));
    let root = ok(draft.intern_string("books"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![
                field_member(shapes, None, VALUE_FIELD_ID, true, Scalar::Text),
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Branch {
                        placement: LedgerIdBytes::from_bytes([0x30; 16]),
                        name: notes,
                        record: notes_record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Text,
                            id: LedgerIdBytes::from_bytes([0x31; 16]),
                        }],
                    },
                },
                field_member(shapes, Some(1), [0x32; 16], true, Scalar::Text),
                DeclarationMemberDef {
                    parent: Some(1),
                    shape: DeclarationMemberShape::Branch {
                        placement: LedgerIdBytes::from_bytes([0x40; 16]),
                        name: tags,
                        record: tags_record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Int,
                            id: LedgerIdBytes::from_bytes([0x41; 16]),
                        }],
                    },
                },
                field_member(shapes, Some(3), [0x42; 16], true, Scalar::Int),
            ],
        )
        .expect("a well-formed declaration");
    let admitted = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    (draft_owner, admitted)
}

/// The nested `tags` branch's own placement id.
const TAG_PLACEMENT_ID: [u8; 16] = [0x40; 16];

/// The whole-payload site of the nested `tags` branch entry: the `notes` branch is the
/// Product's second declared member and `tags` is that branch's second member.
fn nested_tag_entry_site(draft: &mut DraftTxn<'_>, root: &AdmittedRoot) -> PlannedSiteRef {
    let notes = product_members(draft)[1].path().clone();
    let tags = draft
        .members_of(&notes)
        .expect("the declaration row is live")[1]
        .path()
        .clone();
    site(
        draft,
        root.occurrence(),
        &tags,
        SemanticTarget::WholePayload,
    )
}

/// The nested `tags` branch entry site, as encoded bytes: `application -> root placement
/// -> notes placement -> tags placement`, whole payload.
fn encoded_tag_entry_site(chain: &[[u8; 16]]) -> Vec<u8> {
    let mut steps = vec![
        (SemanticStepKind::Application, APPLICATION_ID),
        (SemanticStepKind::Placement, PLACEMENT_ID),
    ];
    steps.extend(chain.iter().map(|id| (SemanticStepKind::Placement, *id)));
    encoded_site(&steps, 0x00)
}

/// Add a read-only export that runs `DurExists` over the whole-payload `site` at the given
/// key arity (root-first `int, string, int` for the tag entry), and encode. The opcode is
/// the observation that separates an executable deep site from a parked one.
fn exists_over_tag_entry(mut draft: DraftTxn<'_>, site: PlannedSiteRef) -> Vec<u8> {
    let code = vec![
        Instr::LocalGet(0), // root key: int
        Instr::LocalGet(1), // note key: string
        Instr::LocalGet(2), // tag key: int
        Instr::DurExists(site),
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "has",
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ImageType::scalar(Scalar::Bool),
        3,
        code,
    );
    draft.add_export(ExportId::of_local("", "has"), func);
    draft.encode().unwrap().bytes
}

#[test]
fn a_valid_deep_nested_branch_entry_site_seals_executable_and_its_opcode_verifies() {
    // The positive control: a whole-payload site over the concrete `notes -> tags` chain
    // resolves to the nested branch, seals executable, and an `exists` opcode over its
    // three-column key-path type-checks and verifies.
    let (mut draft_owner, root) = nested_branch_draft();
    let mut draft = admitted(&mut draft_owner);
    let site = nested_tag_entry_site(&mut draft, &root);
    assert_eq!(verdict_of(&exists_over_tag_entry(draft, site)), Verified);
}

/// The nested-branch image carrying the valid `notes -> tags` whole-payload site.
fn nested_branch_site_image() -> Vec<u8> {
    let (mut draft_owner, root) = nested_branch_draft();
    let mut draft = admitted(&mut draft_owner);
    let site = nested_tag_entry_site(&mut draft, &root);
    exists_over_tag_entry(draft, site)
}

// --- Composite-key admission hostiles ---
//
// A composite-key root addresses each entry by its whole ordered key tuple. The verifier
// derives the expected key-path — one column per key column, in order — from the sealed
// schema and type-checks the operand stack against it, so a forged opcode key-path that is
// too short, or whose columns are the wrong type or transposed to the wrong type, is
// refused during per-function typing. A same-typed transposition is a distinct valid
// program the physical layout distinguishes (see the composite kernel and source tests);
// the verifier catches only the count/type skews here.

/// A flat-executable root `^cells(row: int, col: text)` — a two-column composite key of
/// distinct column types, so a transposed key-path is type-detectable — with one required
/// int field `v`. Returns the draft and the whole-entry site operand.
fn composite_root_draft() -> (ImageDraft, PlannedSiteRef) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let cell = ok(draft.intern_string("Cell"));
    let v = ok(draft.intern_string("v"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: cell,
        fields: vec![FieldDef {
            name: v,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    }));
    let root = ok(draft.intern_string("cells"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![field_member(
                shapes,
                None,
                VALUE_FIELD_ID,
                true,
                Scalar::Int,
            )],
        )
        .expect("a well-formed declaration");
    let admitted = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![
                    KeyColumn {
                        scalar: Scalar::Int,
                        id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                    },
                    KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes([0x1c; 16]),
                    },
                ],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let entry = site(
        &mut draft,
        admitted.occurrence(),
        admitted.placement_path(),
        SemanticTarget::WholePayload,
    );
    draft.commit();
    (draft_owner, entry)
}

/// Encode a read-only `has` export over the composite `entry` site whose body pushes the
/// given key locals (by type) then runs `DurExists`. `params` types the locals the body
/// reads; a correct call pushes `[Int, Text]` (row then col, root-first).
fn composite_exists_export(
    mut owner: ImageDraft,
    entry: PlannedSiteRef,
    params: Vec<ImageType>,
) -> Vec<u8> {
    let mut draft = admitted(&mut owner);
    let mut code: Vec<Instr> = (0..params.len() as u16).map(Instr::LocalGet).collect();
    code.push(Instr::DurExists(entry));
    code.push(Instr::Return);
    let local_count = params.len() as u16;
    let func = add_fn(
        &mut draft,
        "has",
        params,
        ImageType::scalar(Scalar::Bool),
        local_count,
        code,
    );
    draft.add_export(ExportId::of_local("", "has"), func);
    draft.encode().unwrap().bytes
}

#[test]
fn a_composite_root_opcode_with_the_full_ordered_key_path_verifies() {
    // The positive control: pushing both columns in order (row:int, col:text) type-checks
    // the two-column key-path and verifies.
    let (draft, entry) = composite_root_draft();
    let bytes = composite_exists_export(
        draft,
        entry,
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
        ],
    );
    assert_eq!(verdict_of(&bytes), Verified);
}

#[test]
fn a_composite_root_opcode_with_a_truncated_key_path_rejects() {
    // Only one key column pushed where the two-column composite key-path needs two: the
    // operand stack cannot satisfy the derived key-path, refused at per-function typing.
    let (draft, entry) = composite_root_draft();
    let bytes = composite_exists_export(draft, entry, vec![ImageType::scalar(Scalar::Int)]);
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Function));
}

#[test]
fn a_composite_root_opcode_with_transposed_column_types_rejects() {
    // The columns pushed in the wrong order (text then int, where int then text is
    // required): the key-path type check fails, so a transposed key-path over distinctly
    // typed columns cannot mis-address a durable operation.
    let (draft, entry) = composite_root_draft();
    let bytes = composite_exists_export(
        draft,
        entry,
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Function));
}

#[test]
fn a_bounded_traversal_over_a_composite_keyed_root_layer_rejects() {
    // Bounded traversal iterates a single key column. A forged image that drives
    // `DurIterateBounded` over a composite-keyed root (two key columns), bypassing the
    // compiler's own park, is refused during per-function typing: the verifier computes the
    // traversed layer's arity from the schema and rejects any layer that is not
    // single-column, so no composite-key traversal reaches the kernel.
    let (mut draft_owner, entry) = composite_root_draft();
    let mut draft = admitted(&mut draft_owner);
    let list_ty = ok(draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Int),
    }));
    let code = vec![
        Instr::DurIterateBounded {
            site: entry,
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = add_fn(&mut draft, "iter", Vec::new(), ImageType::Unit, 0, code);
    draft.add_export(ExportId::of_local("", "iter"), func);
    assert_eq!(
        verdict_of(&draft.encode().unwrap().bytes),
        Refused(VerifyPhase::Function)
    );
}

// --- Suite: the verifier's own durable-graph bounds, forged N/N+1. ---
//
// A coherent producer cannot emit any of these images: the encoder rechecks the member
// bound, the depth bound, and the index-component bound before it writes a byte. Each of
// these bounds is therefore reachable only from forged bytes over a valid image, which is
// what makes it the verifier's own bound rather than a restatement of the producer's.
//
// Every one of the three refusals fires **inside the decode**, from the bytes read so far
// rather than from a reconstructed graph. The pair per bound is exact: at `N` the decode
// passes the bound and a later, differently named invariant answers; at `N + 1` the
// bound's own detail answers. Freezing both sides is what makes the pair a statement about
// which bound answers, rather than only that something refused.

/// A hand-built DURABLE section body over one root of one Product, closed by a
/// placeholder contract identity.
///
/// `members` and `indexes` are spliced in whole, so a caller states exactly the member run
/// and index run it wants the verifier to read — including runs no encoder would write.
fn forged_durable_body(members: Vec<u8>, indexes: Vec<u8>) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&1u16.to_be_bytes()); // one root occurrence
    body.extend_from_slice(&APPLICATION_ID);
    body.extend_from_slice(&0u16.to_be_bytes()); // root name: string 0
    // The tracer root's real key tuple: one `text` column.
    body.extend_from_slice(&1u16.to_be_bytes());
    body.push(Scalar::Text.tag());
    body.extend_from_slice(&ROOT_KEY_ID);
    body.extend_from_slice(&0u16.to_be_bytes()); // root entry record: type 0
    body.extend_from_slice(&PLACEMENT_ID);
    body.extend_from_slice(&PRODUCT_ID);
    body.extend_from_slice(&members);
    body.extend_from_slice(&indexes);
    body.extend_from_slice(&0u16.to_be_bytes()); // no site rows
    body.extend_from_slice(&[0u8; 32]); // the carried contract identity
    body
}

/// One durable field member: `tag(0x00) ‖ id ‖ required ‖ value`, its value the bare
/// `int` scalar shape (`0x00 ‖ scalar_tag`).
fn forged_field_member(n: usize) -> Vec<u8> {
    let mut id = [0x60u8; 16];
    id[0] = (n & 0xff) as u8;
    id[1] = ((n >> 8) & 0xff) as u8;
    id[2] = ((n >> 16) & 0xff) as u8;
    let mut out = vec![0x00u8];
    out.extend_from_slice(&id);
    out.push(1);
    out.push(0x00);
    out.push(Scalar::Int.tag());
    out
}

/// The member run the tracer's `Counter` record materializes: `required value: int` then
/// optional `label: text`. A forged graph that must survive the record invariant to reach
/// a later bound states this run.
fn matching_member_run() -> Vec<u8> {
    let mut out = 2u16.to_be_bytes().to_vec();
    out.push(0x00);
    out.extend_from_slice(&VALUE_FIELD_ID);
    out.push(1);
    out.push(0x00);
    out.push(Scalar::Int.tag());
    out.push(0x00);
    out.extend_from_slice(&LABEL_FIELD_ID);
    out.push(0);
    out.push(0x00);
    out.push(Scalar::Text.tag());
    out
}

/// A member run of `count` distinct top-level fields.
fn forged_field_run(count: usize) -> Vec<u8> {
    let mut out = (count as u16).to_be_bytes().to_vec();
    for n in 0..count {
        out.extend_from_slice(&forged_field_member(n));
    }
    out
}

/// A member run one field deep inside `nesting` static `group` namespaces. The root's own
/// run is depth 1, so the innermost field sits at depth `nesting + 1`.
fn forged_group_nest(nesting: usize) -> Vec<u8> {
    let mut inner = forged_field_run(1);
    for level in (0..nesting).rev() {
        let mut id = [0x80u8; 16];
        id[0] = (level & 0xff) as u8;
        let mut out = 1u16.to_be_bytes().to_vec();
        out.push(0x01);
        out.extend_from_slice(&id);
        out.extend_from_slice(&inner);
        inner = out;
    }
    inner
}

/// One managed index projecting `components` copies of the root's single field.
fn forged_index_run(components: usize) -> Vec<u8> {
    let mut out = 1u16.to_be_bytes().to_vec();
    out.extend_from_slice(&[0x70u8; 16]);
    out.push(0); // nonunique
    out.extend_from_slice(&(components as u16).to_be_bytes());
    for _ in 0..components {
        out.push(0x02);
        out.extend_from_slice(&VALUE_FIELD_ID);
    }
    out
}

/// Replace the DURABLE section body of a valid image with `forged`, repair the section
/// length, revalidate the digest, and report the rejection the verifier answers with.
fn forged_durable_rejection(forged: Vec<u8>) -> marrow_verify::VerifyRejection {
    let mut bytes = good_durable_image();
    let (_, body, len) = *sections(&bytes).iter().find(|(id, ..)| *id == 3).unwrap();
    let forged_len = forged.len() as u32;
    bytes.splice(body..body + len, forged);
    bytes[body - 4..body].copy_from_slice(&forged_len.to_be_bytes());
    rehash(&mut bytes);
    verify(&bytes).expect_err("a forged durable graph never verifies")
}

/// The member budget bounds the whole tree, and it answers at exactly one member past it.
#[test]
fn a_forged_durable_member_run_past_the_budget_rejects_with_the_budget_detail() {
    const N: usize = marrow_image::bounds::MAX_DURABLE_MEMBERS;

    let over = forged_durable_rejection(forged_durable_body(forged_field_run(N + 1), vec![0, 0]));
    assert_eq!(over.phase(), VerifyPhase::Table);
    assert_eq!(over.detail(), "too many durable members");

    // At the budget the decode completes and a later invariant — the member tree against
    // the materialized record — is what answers instead.
    let at = forged_durable_rejection(forged_durable_body(forged_field_run(N), vec![0, 0]));
    assert_eq!(at.phase(), VerifyPhase::Table);
    assert_ne!(at.detail(), "too many durable members");
    assert_eq!(at.detail(), AT_BUDGET_DETAIL);
}

/// The depth bound answers at exactly one level past it, and the nesting one level
/// shallower reaches the record invariant instead.
#[test]
fn a_forged_durable_member_tree_past_the_depth_bound_rejects_with_the_depth_detail() {
    const N: usize = marrow_image::bounds::MAX_DURABLE_DEPTH;

    let over = forged_durable_rejection(forged_durable_body(forged_group_nest(N), vec![0, 0]));
    assert_eq!(over.phase(), VerifyPhase::Table);
    assert_eq!(over.detail(), "durable member tree too deep");

    let at = forged_durable_rejection(forged_durable_body(forged_group_nest(N - 1), vec![0, 0]));
    assert_eq!(at.phase(), VerifyPhase::Table);
    assert_ne!(at.detail(), "durable member tree too deep");
    assert_eq!(at.detail(), AT_DEPTH_DETAIL);
}

/// The index-component bound answers at exactly one component past it.
#[test]
fn a_forged_durable_index_past_the_component_bound_rejects_with_the_component_detail() {
    const N: usize = marrow_image::bounds::MAX_INDEX_COMPONENTS;

    let over = forged_durable_rejection(forged_durable_body(
        matching_member_run(),
        forged_index_run(N + 1),
    ));
    assert_eq!(over.phase(), VerifyPhase::Table);
    assert_eq!(over.detail(), "too many durable index components");

    let at = forged_durable_rejection(forged_durable_body(
        matching_member_run(),
        forged_index_run(N),
    ));
    assert_eq!(at.phase(), VerifyPhase::Table);
    assert_ne!(at.detail(), "too many durable index components");
    assert_eq!(at.detail(), AT_COMPONENTS_DETAIL);
}

/// The exact detail each corpus draws at `N`, frozen so the pair states which bound
/// answers on each side rather than only that something refused.
const AT_BUDGET_DETAIL: &str = "root member tree fields do not match the record fields";
const AT_DEPTH_DETAIL: &str = "a root group slot is not a group record";
const AT_COMPONENTS_DETAIL: &str = "durable index repeats a projection component";
