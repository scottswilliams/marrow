//! The legacy-v0 measure core: coherence, policy, capped measurement, and planned
//! emission, in that order.
//!
//! [`ImageDraft::encode`] drives four affine steps, each consuming the previous one:
//!
//! 1. **Coherence** ([`LegacyV0MeasureCore::coherence`]): every invariant-classified
//!    decision — the fixed per-construct widths, the durable graph walks, the
//!    application anchor, the site projection, and every structural reference the
//!    emitted sections would resolve — in the exact relative order the legacy encoder
//!    decided them (the stable partition; the sequence note below names it item by
//!    item). A coherence failure returns the existing invariant error before any
//!    policy candidate.
//! 2. **Policy** ([`CoherentDraft::policy`]): the resource-policy candidates in the
//!    exact legacy candidate order — the eleven aggregate caps, then per-function
//!    CodeBytes. A policy failure returns without measuring, hashing, or allocating.
//! 3. **Measurement** ([`PolicyClean::measure`]): the whole image counted through the
//!    one checked sink in exact assembly order — the envelope head and section-count
//!    prelude, then per section its frame and its body — driving the same codecs
//!    emission uses over the base rows with constant tokens, whose lengths the
//!    counted==emitted KATs prove order-independent. FUNCTIONS and SPANS are counted
//!    arithmetically from the same per-item width owners their writers spell, so no
//!    layout offsets exist before a verdict. The sink saturates decisively one byte
//!    past [`bounds::MAX_IMAGE_BYTES`], and every independently unbounded or outer
//!    row loop polls it — bounded inner runs (a record's fields, an enum's variants,
//!    a function's parameters) are already covered by coherence's own width bounds.
//!    The
//!    result is an affine [`LegacyV0WirePlan`] — inline per-section body and framed
//!    spans plus the envelope spans, no heap — or
//!    [`ImageBuildError::ImageTooLarge`] with nothing retained; the whole-image
//!    ceiling is decided here, before any section is assembled.
//! 4. **Emission** ([`LegacyV0WirePlan::emit_image`]): the plan is consumed; each
//!    section is built through the same writer with the real permutations and tokens;
//!    every body length, framed span, the tail cursor (the digest input range), the
//!    envelope head, and the final total are compared to the plan, and a
//!    disagreement is the typed [`ImageBuildError::EncodeDrift`] naming the drifted
//!    region. The durable contract identity is minted exactly once, inside
//!    `emit_image`.
//!
//! # The coherence sequence (stable partition)
//!
//! Step 1 preserves the legacy decision-site order as eleven items: (i) the
//! `check_bounds` invariant subsequence — per-record fields, per-enum definition
//! widths, the Product claim conflict, the per-declaration graph walk, the
//! whole-arena value-shape walk, per-occurrence key/index widths, per-function
//! frames; (ii) the application anchor, the streamed site projection, and operand
//! provenance; then the emission-order reference checks hoisted from the writers:
//! (iii) per function its name, source, signature types, and tape operands in tape
//! order; (iv) the DURABLE body's root names, entry records, and branch names and
//! records in body order; (v) TYPES; (vi) CONSTS; (vii) EXPORTS with the export
//! relations; (viii) SPANS; (ix) TEST-ENTRY with the test relations; (x) ENUMS;
//! (xi) COLLTYPES. Every hoisted check is a range check over a local ordinal (or a
//! locally decidable relation); an in-range id keeps its local meaning, and the
//! independent verifier remains the only decoder.
//!
//! # Allocation posture and the max-live term (the editor-capacity pre-join term)
//!
//! Every pre-verdict path is zero-heap — the coherence walk (its site-projection
//! validation streams and materializes no path), the policy audit, the counting
//! sink (`usize`), the witness (`u32`), the plan (inline arrays and scalars), and
//! the offset-free FUNCTIONS/SPANS counting arithmetic — with exactly one
//! exception, the adjudicated DURABLE expansion worklist below. Layout offsets and
//! sort scratch are emission-only. Beyond the retained draft baseline (the pools,
//! tables, and plans the draft owns for its whole life) and the fitting emitter's
//! post-plan sort/section/output topology, the max-live expression charges:
//!
//! - the DURABLE traversal's ceiling-bounded expansion worklist — the one
//!   sanctioned pre-verdict allocation (stop adjudication of record). Measured
//!   through the scheduling harness over both worst-case corpora, recording `Vec`
//!   capacity (what lives), not length: the 31-level 64-edge struct chain peaks at
//!   1,954 pending tasks, capacity 2,048 × 16 bytes/task = 32,768 live bytes; the
//!   31-level enum chain of 256 members × 64 payload references — the widest
//!   scheduling fan-out the §E bounds admit — peaks at 9,859 pending tasks,
//!   capacity 16,384 × 16 bytes/task = **262,144 live bytes**, with a
//!   `Vec`-doubling old/new-buffer overlap of at most **393,216 bytes**. The enum
//!   figure bounds the admitted domain.
//!
//! The `#[ignore]`d measurement harness that produced these numbers lives beside
//! the traversal owner (`value_dag.rs`); re-run it with `--ignored` when a
//! representation widens, and re-gate through the persistent editor-capacity law.

use crate::bounds;
use crate::digest::image_id;
use crate::draft::{
    CollTypeId, CollectionTypeDef, ConstId, ConstValue, ImageBuildError, ImageDraft, StrId, TypeId,
};
use crate::durable_id::{DurableContractId, DurableGraphTooLarge};
use crate::encode::{
    EncodedImage, SECTION_COUNT, SPAN_ROW_BYTES, laid_out_code_len, push_frame, push_section,
    remap_of, write_image_header,
};
use crate::instr::Instr;
use crate::policy_ledger::{LedgerSlotIndex, TablePolicyAudit, TablePolicyKind};
use crate::product::{
    DeclarationMemberShape, DeclarationNode, ProductClaimConflict, ProductDeclarationGraph,
};
use crate::remap::{ConstRemap, SectionSink, StringRemap};
use crate::ty::ImageType;
use crate::value_dag::{CanonicalValueShapeDag, ImageByteSink, ValueShapeView};

/// The wire ids of the ten sections, in assembly order, and the envelope marker.
const SECTION_IDS: [u8; 10] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A];
/// The plan slot of the DURABLE section (wire id 0x03).
const DURABLE_SLOT: usize = 2;

/// The policy-clean `u16` narrowing of one owned wide logical ordinal — the one
/// sanctioned narrowing direction for an owned pre-seal id.
///
/// Every caller runs on the measure core's fitting arm: during capped measurement or
/// planned emission, both strictly after [`CoherentDraft::policy`] proved every owned
/// table within its §E maximum, and `crate::bounds` const-asserts each of those
/// maxima at or below `u16::MAX`, so the conversion is total there. It is spelled
/// checked so a value outside the proved envelope is a producer invariant, never a
/// wrapped wire value.
pub(crate) fn wire_ordinal(ordinal: u32) -> u16 {
    u16::try_from(ordinal).expect("a policy-clean owned ordinal fits the u16 wire domain")
}

/// The policy-clean `u16` narrowing of one owned row or byte count, on the same
/// fitting-arm implication as [`wire_ordinal`].
pub(crate) fn wire_len(count: usize) -> u16 {
    u16::try_from(count).expect("a policy-clean owned count fits the u16 wire domain")
}

/// Which emitted region disagreed with the measured plan: one of the ten sections, by
/// its wire id, or the image envelope. Opaque — it renders the region's name and can
/// be stated only by the emission comparison itself, so no other owner can claim a
/// drift the plan never measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeDriftSection(u8);

impl EncodeDriftSection {
    /// The envelope marker: the assembled header/frame total, not a section body.
    const ENVELOPE: u8 = 0x00;

    fn of(section_id: u8) -> Self {
        Self(section_id)
    }

    fn envelope() -> Self {
        Self(Self::ENVELOPE)
    }
}

impl std::fmt::Display for EncodeDriftSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            0x01 => "STRINGS",
            0x02 => "TYPES",
            0x03 => "DURABLE",
            0x04 => "CONSTS",
            0x05 => "FUNCTIONS",
            0x06 => "EXPORTS",
            0x07 => "SPANS",
            0x08 => "TEST-ENTRY",
            0x09 => "ENUMS",
            0x0A => "COLLTYPES",
            _ => "image envelope",
        })
    }
}

/// The measure core: the namespace of the four-step journey's entry point.
pub(crate) struct LegacyV0MeasureCore;

/// A draft every invariant-classified decision has admitted. Affine and borrow-bound:
/// it is minted only by [`LegacyV0MeasureCore::coherence`], consumed by the policy
/// walk, and cannot outlive or outnumber the draft it certifies.
pub(crate) struct CoherentDraft<'d>(&'d ImageDraft);

/// A coherent draft every resource-policy candidate has admitted. Affine: consumed by
/// measurement.
pub(crate) struct PolicyClean<'d>(&'d ImageDraft);

/// The DURABLE section length the counting run measured, contract identity included.
///
/// Private and mintable only inside [`PolicyClean::measure`]'s counting run over the
/// one installed [`ImageDraft::write_durable_body`] writer; [`LegacyV0WirePlan`]'s
/// constructor demands it, so a second traversal cannot feed a plan.
struct MeasuredDurableLen(u32);

/// The measured wire plan: per section its body length and its framed span, plus the
/// envelope-head span, the tail span (the section-count prelude and every framed
/// section — the digest input), and the whole-image total, all counted through the
/// same codecs emission drives. No heap; affine — emission consumes it.
pub(crate) struct LegacyV0WirePlan<'d> {
    draft: &'d ImageDraft,
    bodies: [u32; 10],
    framed: [u32; 10],
    header: u32,
    tail: u32,
    total: u32,
}

impl LegacyV0MeasureCore {
    /// Step 1–2: the complete coherence walk (see the module doc for the sequence).
    pub(crate) fn coherence(draft: &ImageDraft) -> Result<CoherentDraft<'_>, ImageBuildError> {
        invariant_bounds(draft)?;
        anchor_and_sites(draft)?;
        function_references(draft)?;
        durable_references(draft)?;
        types_references(draft)?;
        consts_references(draft)?;
        exports_relations(draft)?;
        span_references(draft)?;
        test_entry_relations(draft)?;
        enums_references(draft)?;
        collections_references(draft)?;
        Ok(CoherentDraft(draft))
    }
}

impl<'d> CoherentDraft<'d> {
    /// Step 3: the resource-policy walk, in the exact legacy candidate order — the
    /// eleven aggregate caps as `check_bounds` declared them, then per-function
    /// CodeBytes in function order. Nothing is measured, hashed, or allocated here.
    pub(crate) fn policy(self) -> Result<PolicyClean<'d>, ImageBuildError> {
        let draft = self.0;
        // The independent eight-slot ledger audit runs over the coherent draft before
        // the walk: every slot is recomputed from final draft state and compared
        // byte-exactly. It authorizes nothing; the walk below remains the public
        // candidate authority, and its verdict is shadow-compared afterwards.
        if let Err(drift) = TablePolicyAudit::cross_validate(draft) {
            return Err(ImageBuildError::LedgerDrift(drift));
        }
        let verdict = Self::legacy_walk(draft);
        Self::shadow_compare(draft, &verdict)?;
        verdict?;
        Ok(PolicyClean(draft))
    }

    /// The exact legacy candidate walk, unchanged in order and authority.
    fn legacy_walk(draft: &ImageDraft) -> Result<(), ImageBuildError> {
        if draft.strings().len() > bounds::MAX_STRINGS {
            return Err(ImageBuildError::TooManyStrings);
        }
        for text in draft.strings() {
            if text.len() > bounds::MAX_STRING_BYTES {
                return Err(ImageBuildError::StringTooLong);
            }
        }
        if draft.consts().len() > bounds::MAX_CONSTS {
            return Err(ImageBuildError::TooManyConsts);
        }
        if draft.types().len() > bounds::MAX_TYPES {
            return Err(ImageBuildError::TooManyTypes);
        }
        if draft.enums().len() > bounds::MAX_ENUMS {
            return Err(ImageBuildError::TooManyEnums);
        }
        if draft.collections().len() > bounds::MAX_COLLECTIONS {
            return Err(ImageBuildError::TooManyCollections);
        }
        if draft.root_occurrences().len() > bounds::MAX_ROOTS {
            return Err(ImageBuildError::TooManyRoots);
        }
        if draft.site_demand() > bounds::MAX_SITES {
            return Err(ImageBuildError::TooManySites);
        }
        if draft.functions().len() > bounds::MAX_FUNCTIONS {
            return Err(ImageBuildError::TooManyFunctions);
        }
        if draft.export_count() > bounds::MAX_EXPORTS {
            return Err(ImageBuildError::TooManyExports);
        }
        if draft.test_entry_count() > bounds::MAX_TEST_ENTRIES {
            return Err(ImageBuildError::TooManyTestEntries);
        }
        for function in draft.functions() {
            if laid_out_code_len(&function.code) as usize > bounds::MAX_CODE_BYTES {
                return Err(ImageBuildError::CodeTooLong);
            }
        }
        Ok(())
    }

    /// Shadow-compare the walk's verdict against the audited ledger: a table-owned
    /// refusal must name exactly the ledger's canonical minimum, and any other
    /// verdict (clean or a function-family kind) admits no active-slot crossing — a
    /// lower-ranked crossing would have been the walk's own earlier candidate. A
    /// function result grants the audit no authority.
    fn shadow_compare(
        draft: &ImageDraft,
        verdict: &Result<(), ImageBuildError>,
    ) -> Result<(), ImageBuildError> {
        let owned_rank = match verdict {
            Err(ImageBuildError::TooManyStrings) => Some(TablePolicyKind::Strings),
            Err(ImageBuildError::StringTooLong) => Some(TablePolicyKind::StringBytes),
            Err(ImageBuildError::TooManyConsts) => Some(TablePolicyKind::Consts),
            Err(ImageBuildError::TooManyTypes) => Some(TablePolicyKind::Types),
            Err(ImageBuildError::TooManyEnums) => Some(TablePolicyKind::Enums),
            Err(ImageBuildError::TooManyCollections) => Some(TablePolicyKind::Collections),
            Err(ImageBuildError::TooManyRoots) => Some(TablePolicyKind::Roots),
            Err(ImageBuildError::TooManySites) => Some(TablePolicyKind::Sites),
            _ => None,
        };
        let minimum = draft.policy_ledger().cached_minimum();
        let agrees = match owned_rank {
            Some(kind) => minimum == Some(LedgerSlotIndex::of(kind)),
            None => minimum.is_none(),
        };
        if agrees {
            Ok(())
        } else {
            Err(ImageBuildError::LedgerDrift(
                "the legacy walk's verdict disagrees with the ledger's canonical minimum",
            ))
        }
    }
}

impl<'d> PolicyClean<'d> {
    /// Step 3: count the whole image through one checked sink, in exact assembly
    /// order — the envelope head (zero digest), the section-count prelude, then per
    /// section its frame (zero length) and its body — driving the same codecs
    /// emission uses over the base rows with constant tokens. FUNCTIONS and SPANS
    /// are counted arithmetically from the same per-item width owners their writers
    /// spell, so no layout offsets exist before a verdict. The sink saturates
    /// decisively at [`bounds::MAX_IMAGE_BYTES`]` + 1`, every row loop polls it, and
    /// the ceiling is decided here, before any section is assembled.
    pub(crate) fn measure(self) -> Result<LegacyV0WirePlan<'d>, ImageBuildError> {
        let draft = self.0;
        let strings = StringRemap::counting();
        let mut counter = CappedImageCount::default();
        let mut bodies = [0u32; 10];
        let mut framed = [0u32; 10];

        let header = counter.measured(|sink| {
            write_image_header(sink, &[0u8; 32]);
            Ok(())
        })?;
        let tail_start = counter.total();
        counter.measured(|sink| {
            sink.push(SECTION_COUNT);
            Ok(())
        })?;

        let mut durable = None;
        for slot in 0..SECTION_IDS.len() {
            let framed_start = counter.total();
            counter.measured(|sink| {
                push_frame(sink, SECTION_IDS[slot], 0);
                Ok(())
            })?;
            bodies[slot] = counter
                .measured(|sink| count_section_body(draft, slot, &strings, sink, &mut durable))?;
            // Within the ceiling the sink just re-proved, so the span fits `u32`.
            framed[slot] = (counter.total() - framed_start) as u32;
        }
        let durable = durable.expect("the DURABLE slot was just counted");
        let tail = (counter.total() - tail_start) as u32;
        LegacyV0WirePlan::new(draft, bodies, framed, header, tail, durable)
    }
}

/// Count one section body over the base rows with constant tokens, minting the
/// measured-DURABLE witness at the DURABLE slot. FUNCTIONS and SPANS consume the
/// shared per-item width owners ([`laid_out_code_len`], [`crate::ty::ImageType`]'s
/// encoded widths, [`SPAN_ROW_BYTES`]) rather than the offset-laying writers, so
/// measurement allocates nothing; the counted==emitted KATs pin the arithmetic
/// against the writers per section.
fn count_section_body(
    draft: &ImageDraft,
    slot: usize,
    strings: &StringRemap<'_>,
    sink: &mut CappedImageCount,
    durable: &mut Option<MeasuredDurableLen>,
) -> Result<(), ImageBuildError> {
    match SECTION_IDS[slot] {
        0x01 => draft.encode_strings(&mut SectionSink::over(sink), 0..draft.strings().len()),
        0x02 => draft.encode_types(&mut SectionSink::over(sink), strings),
        0x03 => {
            // The one installed DURABLE writer drives the one installed expansion,
            // exactly as emission will; the section closes with the 32-byte contract
            // identity the emission run appends after the body.
            let body_start = sink.total();
            draft.write_durable_body(sink, strings)?;
            sink.pad(DurableContractId::BYTES);
            // Within the capped count, so the length fits `u32`.
            *durable = Some(MeasuredDurableLen((sink.total() - body_start) as u32));
        }
        0x04 => draft.encode_consts(
            &mut SectionSink::over(sink),
            strings,
            0..draft.consts().len(),
        ),
        0x05 => count_functions(draft, sink),
        0x06 => draft.encode_exports(&mut SectionSink::over(sink), 0..draft.export_count()),
        0x07 => count_spans(draft, sink),
        0x08 => draft.encode_test_entries(
            &mut SectionSink::over(sink),
            strings,
            0..draft.test_entry_count(),
        ),
        0x09 => draft.encode_enums(&mut SectionSink::over(sink), strings),
        0x0A => draft.encode_collections(&mut SectionSink::over(sink)),
        _ => unreachable!("the section id table is closed"),
    }
    Ok(())
}

/// The FUNCTIONS section body, counted without laying out offsets: the row-count
/// prefix, then per function its fixed header widths — the two remap tokens, the
/// parameter-count byte, each parameter's and the return's encoded type width, the
/// local count, the code-length prefix — plus [`laid_out_code_len`], the one
/// per-instruction width owner the emission layout accumulates into offsets. Jump
/// operands are fixed-width, so no offset value affects a length.
pub(crate) fn count_functions(draft: &ImageDraft, sink: &mut CappedImageCount) {
    sink.pad(2);
    for function in draft.functions() {
        if sink.is_full() {
            return;
        }
        sink.pad(2 + 2 + 1);
        for param in &function.params {
            sink.pad(param.encoded_len());
        }
        sink.pad(function.ret.encoded_len());
        sink.pad(2 + 4);
        sink.pad(laid_out_code_len(&function.code) as usize);
    }
}

/// The SPANS section body, counted without offsets: per function its `u16` count
/// prefix plus [`SPAN_ROW_BYTES`] per row — the widths `encode_spans` spells.
pub(crate) fn count_spans(draft: &ImageDraft, sink: &mut CappedImageCount) {
    for function in draft.functions() {
        if sink.is_full() {
            return;
        }
        sink.pad(2 + SPAN_ROW_BYTES * function.spans.len());
    }
}

/// The plan-bound site projection: the only path from a [`crate::PlannedSiteRef`] to its
/// wire ordinal. It is minted exclusively by a measured [`LegacyV0WirePlan`], so a
/// numeric site id exists only downstream of fitting policy-clean capped measurement;
/// each projection revalidates the ref's provenance against the live plan and graph
/// before yielding the ordinal.
pub(crate) struct SiteWireProjection<'d>(&'d ImageDraft);

impl SiteWireProjection<'_> {
    /// The exact wire ordinal of one validated fitting site ref.
    pub(crate) fn ordinal(&self, site: &crate::PlannedSiteRef) -> Result<u16, ImageBuildError> {
        self.0.site_wire_ordinal(site)
    }
}

impl<'d> LegacyV0WirePlan<'d> {
    /// The plan-bound site projection over this plan's own draft.
    pub(crate) fn site_projection(&self) -> SiteWireProjection<'d> {
        SiteWireProjection(self.draft)
    }

    /// Complete the plan. Every span was counted through the one saturating sink, so
    /// the ceiling verdict is already decided when this is reached; demanding the
    /// witness is what makes a plan unforgeable — only the counting run over the
    /// installed DURABLE writer can mint one.
    fn new(
        draft: &'d ImageDraft,
        mut bodies: [u32; 10],
        framed: [u32; 10],
        header: u32,
        tail: u32,
        durable: MeasuredDurableLen,
    ) -> Result<Self, ImageBuildError> {
        bodies[DURABLE_SLOT] = durable.0;
        Ok(Self {
            draft,
            bodies,
            framed,
            header,
            tail,
            total: header + tail,
        })
    }

    /// Step 4: consume the plan and emit the image — the real permutations and remap
    /// tokens through the same writers measurement counted, every section body and
    /// framed span compared to the plan, the contract identity minted exactly once,
    /// the tail cursor (the digest input range) and the assembled envelope compared
    /// to the planned spans.
    pub(crate) fn emit_image(self) -> Result<EncodedImage, ImageBuildError> {
        let sites = self.site_projection();
        let Self {
            draft,
            bodies,
            framed,
            header,
            tail,
            total,
        } = self;

        // Row law: the canonical orders are one permutation each over the retained
        // base rows; every reference resolves through the permutation's inverse, read
        // by the writers only as opaque tokens.
        let string_order = draft.string_permutation();
        let str_map = remap_of(&string_order);
        let strings = StringRemap::new(&str_map);
        let const_order = draft.const_permutation(&str_map);
        let const_map = remap_of(&const_order);
        let consts = ConstRemap::new(&const_map);

        let mut out = Vec::with_capacity(tail as usize);
        out.push(SECTION_COUNT);
        let emit = |out: &mut Vec<u8>, slot: usize, body: Vec<u8>| {
            let drift = || ImageBuildError::EncodeDrift(EncodeDriftSection::of(SECTION_IDS[slot]));
            if body.len() != bodies[slot] as usize {
                return Err(drift());
            }
            let cursor = out.len();
            push_section(out, SECTION_IDS[slot], body);
            // The frame comparison: the section advanced the cursor by exactly the
            // framed span the one frame codec counted.
            if out.len() - cursor != framed[slot] as usize {
                return Err(drift());
            }
            Ok(())
        };

        let mut body = Vec::with_capacity(bodies[0] as usize);
        draft.encode_strings(
            &mut SectionSink::over(&mut body),
            string_order.iter().copied(),
        );
        emit(&mut out, 0, body)?;

        let mut body = Vec::with_capacity(bodies[1] as usize);
        draft.encode_types(&mut SectionSink::over(&mut body), &strings);
        emit(&mut out, 1, body)?;

        // The DURABLE body the plan measured, written this time, closed by the 32-byte
        // durable-contract identity: the one mint, for a graph the plan already fits.
        let mut body = Vec::with_capacity(bodies[DURABLE_SLOT] as usize);
        draft.write_durable_body(&mut body, &strings)?;
        let identity = draft
            .contract_view()
            .contract_id()
            .map_err(|DurableGraphTooLarge| ImageBuildError::ImageTooLarge)?;
        body.extend_from_slice(identity.bytes());
        emit(&mut out, DURABLE_SLOT, body)?;

        let mut body = Vec::with_capacity(bodies[3] as usize);
        draft.encode_consts(
            &mut SectionSink::over(&mut body),
            &strings,
            const_order.iter().copied(),
        );
        emit(&mut out, 3, body)?;

        let mut body = Vec::with_capacity(bodies[4] as usize);
        let per_fn =
            draft.encode_functions(&mut SectionSink::over(&mut body), &strings, &consts, &sites)?;
        emit(&mut out, 4, body)?;

        let export_order = draft.export_permutation();
        let mut body = Vec::with_capacity(bodies[5] as usize);
        draft.encode_exports(
            &mut SectionSink::over(&mut body),
            export_order.iter().copied(),
        );
        emit(&mut out, 5, body)?;

        let mut body = Vec::with_capacity(bodies[6] as usize);
        draft.encode_spans(&mut SectionSink::over(&mut body), &per_fn);
        emit(&mut out, 6, body)?;

        let test_entry_order = draft.test_entry_permutation(&str_map);
        let mut body = Vec::with_capacity(bodies[7] as usize);
        draft.encode_test_entries(
            &mut SectionSink::over(&mut body),
            &strings,
            test_entry_order.iter().copied(),
        );
        emit(&mut out, 7, body)?;

        let mut body = Vec::with_capacity(bodies[8] as usize);
        draft.encode_enums(&mut SectionSink::over(&mut body), &strings);
        emit(&mut out, 8, body)?;

        let mut body = Vec::with_capacity(bodies[9] as usize);
        draft.encode_collections(&mut SectionSink::over(&mut body));
        emit(&mut out, 9, body)?;

        // The final tail cursor: the digest input is exactly the measured tail span —
        // the section-count prelude and every framed section.
        if out.len() != tail as usize {
            return Err(ImageBuildError::EncodeDrift(EncodeDriftSection::envelope()));
        }
        let id = image_id(&out);
        let mut bytes = Vec::with_capacity(total as usize);
        write_image_header(&mut bytes, &id.0);
        // The envelope head matched its measured span, so the digest input begins at
        // exactly the counted header cursor.
        if bytes.len() != header as usize {
            return Err(ImageBuildError::EncodeDrift(EncodeDriftSection::envelope()));
        }
        bytes.extend_from_slice(&out);
        if bytes.len() != total as usize {
            return Err(ImageBuildError::EncodeDrift(EncodeDriftSection::envelope()));
        }
        Ok(EncodedImage {
            bytes,
            image_id: id,
        })
    }
}

/// The decisive sentinel: one byte past the whole-image ceiling, where the counting
/// sink saturates and stays.
const DECISIVE_TOTAL: usize = bounds::MAX_IMAGE_BYTES + 1;

/// The one checked counting sink: it keeps nothing, every accumulation saturates at
/// [`DECISIVE_TOTAL`], and every independently unbounded or outer counting row loop
/// polls [`ImageByteSink::is_full`] (bounded inner runs are covered by coherence's
/// own width bounds) — so an over-ceiling draft stops at the decisive byte with only
/// per-row granularity slack, "full" and "fits" stay distinguishable, and the work an
/// over-ceiling draft costs is the bytes the ceiling admits.
#[derive(Default)]
pub(crate) struct CappedImageCount(usize);

impl CappedImageCount {
    /// Run one region's writer against this sink and return the region's byte
    /// length, or refuse a saturated count with the whole-image ceiling verdict.
    fn measured(
        &mut self,
        write: impl FnOnce(&mut Self) -> Result<(), ImageBuildError>,
    ) -> Result<u32, ImageBuildError> {
        let before = self.0;
        write(self)?;
        if self.is_full() {
            return Err(ImageBuildError::ImageTooLarge);
        }
        // Within the ceiling, so the length fits `u32`.
        Ok((self.0 - before) as u32)
    }

    /// Count `bytes` at once — fixed widths a counting arithmetic states without a
    /// buffer (the contract identity, a function row's header, a span table).
    pub(crate) fn pad(&mut self, bytes: usize) {
        self.0 = self.0.saturating_add(bytes).min(DECISIVE_TOTAL);
    }

    /// The saturating running total, for the decisiveness pins and the plan spans.
    pub(crate) fn total(&self) -> usize {
        self.0
    }
}

impl ImageByteSink for CappedImageCount {
    fn push(&mut self, _byte: u8) {
        self.pad(1);
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        self.pad(bytes.len());
    }

    fn is_full(&self) -> bool {
        self.0 > bounds::MAX_IMAGE_BYTES
    }
}

// ---- Step 1a–1g: the `check_bounds` invariant subsequence, in its exact legacy
// relative order.

fn invariant_bounds(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for record in draft.types() {
        if record.fields.len() > bounds::MAX_RECORD_FIELDS {
            return Err(ImageBuildError::TooManyFields);
        }
    }
    for enum_def in draft.enums() {
        if enum_def.variants.len() > bounds::MAX_VARIANTS {
            return Err(ImageBuildError::TooManyVariants);
        }
        for variant in &enum_def.variants {
            if variant.payload.len() > bounds::MAX_PAYLOAD_FIELDS {
                return Err(ImageBuildError::TooManyPayloadFields);
            }
        }
    }
    // A Product identity that two occurrences claim differently is two declarations
    // wearing one identity. The draft records the first such conflict rather than
    // canonicalizing one of them away; refuse to encode it.
    if let Some(conflict) = draft.product_conflict() {
        return Err(match conflict {
            ProductClaimConflict::Graph(_) => ImageBuildError::ProductGraphConflict,
            ProductClaimConflict::EntryRecord(_) => ImageBuildError::ProductEntryRecordConflict,
        });
    }
    // A divergent application-identity replacement was latched rather than applied:
    // two applications wearing one draft, reported beside the Product claim conflict
    // by the owner that refuses artifacts.
    if draft.application_conflict().is_some() {
        return Err(ImageBuildError::ApplicationIdentityConflict);
    }
    // The member tree is a Product declaration fact, so it is validated once per
    // declaration however many roots project it; the key tuple and managed indexes
    // are occurrence facts and are validated per root.
    for declaration in draft.product_declarations() {
        validate_declaration_graph(declaration.graph(), draft.value_shapes())?;
    }
    // Every distinct value shape is measured once, whatever the number of fields
    // that reference it: a shape shared by a thousand fields is one node here.
    validate_value_shapes(draft.value_shapes())?;
    for occurrence in draft.root_occurrences() {
        if occurrence.keys().len() > bounds::MAX_KEY_COLUMNS {
            return Err(ImageBuildError::TooManyKeyColumns);
        }
        if occurrence.indexes().len() > bounds::MAX_INDEXES {
            return Err(ImageBuildError::TooManyIndexes);
        }
        for index in occurrence.indexes() {
            if index.components.len() > bounds::MAX_INDEX_COMPONENTS {
                return Err(ImageBuildError::TooManyIndexComponents);
            }
        }
    }
    for function in draft.functions() {
        if function.params.len() > bounds::MAX_PARAMS {
            return Err(ImageBuildError::TooManyParams);
        }
        if (function.local_count as usize) > bounds::MAX_LOCALS {
            return Err(ImageBuildError::TooManyLocals);
        }
        if (function.local_count as usize) < function.params.len() {
            return Err(ImageBuildError::LocalCountBelowParams);
        }
    }
    Ok(())
}

/// Recheck the durable member-graph bounds a well-formed declaration must satisfy:
/// total member rows within [`bounds::MAX_DURABLE_MEMBERS`], nesting within
/// [`bounds::MAX_DURABLE_DEPTH`], a branch's key tuple within
/// [`bounds::MAX_KEY_COLUMNS`], and each field value's nesting within
/// [`bounds::MAX_DURABLE_VALUE_DEPTH`] — through a *checked* arena lookup, so a field
/// referencing a node this draft's arena never minted (an id from a foreign arena's
/// wider range) is the typed `InvalidReference("value shape")` rather than an abort.
///
/// Every row carries its parent's ordinal and a parent always precedes its children,
/// so this is one forward pass over the rows rather than a descent. The nesting check
/// is defense in depth: no route builds an over-deep row, because
/// [`ProductDeclarationGraph::from_commands`] refuses an over-deep command vector
/// before materializing one.
fn validate_declaration_graph(
    graph: &ProductDeclarationGraph,
    values: &CanonicalValueShapeDag,
) -> Result<(), ImageBuildError> {
    if graph.over_member_bound() {
        return Err(ImageBuildError::TooManyDurableMembers);
    }
    for (node, depth) in graph.rows_with_depths() {
        if depth > bounds::MAX_DURABLE_DEPTH {
            return Err(ImageBuildError::DurableTreeTooDeep);
        }
        match node.shape() {
            // A field value rooted at this node occupies `depth(node)` levels, whatever
            // depth the same node reaches under some other field.
            DeclarationMemberShape::Field { value, .. } => {
                if !values.contains(*value) {
                    return Err(ImageBuildError::InvalidReference("value shape"));
                }
                if values.depth(*value) > bounds::MAX_DURABLE_VALUE_DEPTH {
                    return Err(ImageBuildError::DurableValueTooDeep);
                }
            }
            DeclarationMemberShape::Group { .. } => {}
            DeclarationMemberShape::Branch { keys, .. } => {
                if keys.len() > bounds::MAX_KEY_COLUMNS {
                    return Err(ImageBuildError::TooManyKeyColumns);
                }
            }
        }
    }
    Ok(())
}

/// Recheck every distinct durable value shape's fan-out against the value-type bounds,
/// so a well-formed draft always encodes within the limits the verifier rechecks
/// (§ law 9). Nesting depth is decided per durable field, at its value's root node, by
/// [`validate_declaration_graph`].
///
/// This is one pass over the arena: each distinct shape is measured once however many
/// fields, structs, or enum payloads reference it, and no occurrence is expanded. The
/// arena is validated **as a whole**, not through the fields that reference it: a node
/// no declaration reaches is measured like any other, so every downstream owner may
/// assume an encoded arena is within the value bounds, whatever part a walk visits.
fn validate_value_shapes(values: &CanonicalValueShapeDag) -> Result<(), ImageBuildError> {
    for node in values.nodes() {
        match values.view(node) {
            ValueShapeView::Scalar(_) => {}
            ValueShapeView::Struct(leaves) => {
                if leaves.len() > bounds::MAX_STRUCT_LEAVES {
                    return Err(ImageBuildError::TooManyStructLeaves);
                }
            }
            ValueShapeView::Enum { members, .. } => {
                if members.len() > bounds::MAX_VARIANTS {
                    return Err(ImageBuildError::TooManyVariants);
                }
                for member in members {
                    if member.payload().len() > bounds::MAX_PAYLOAD_FIELDS {
                        return Err(ImageBuildError::TooManyPayloadFields);
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- Step 2: the application anchor, the site projection, and operand provenance.

fn anchor_and_sites(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    // A non-empty durable graph is anchored by the application's ledger id; the legacy
    // encoder demanded it at the head of the DURABLE section, before any member row.
    if !draft.root_occurrences().is_empty() && draft.application_identity().is_none() {
        return Err(ImageBuildError::InvalidReference("application identity"));
    }
    // The projection is validated by streaming each row's steps through the one
    // projection grammar, materializing no path: the coherence walk stays
    // zero-allocation. (The DURABLE writer's own projection keeps its transient
    // per-row path — its site codec spells a step count before the steps.)
    draft.validate_site_projection()?;
    // Ref/receipt provenance — the plan's validate plus the graph's row-identity
    // recheck, never a numeric projection: an over-policy ref is live provenance the
    // Sites policy candidate reports.
    for function in draft.functions() {
        for instr in &function.code {
            if let Some(site) = instr.site_operand()
                && !draft.site_ref_is_live(site)
            {
                return Err(ImageBuildError::InvalidReference("operation site"));
            }
        }
    }
    Ok(())
}

// ---- Steps 3–11: the emission-order reference checks, hoisted from the writers.

/// One drafted string-pool reference, checked against the pool: the range predicate
/// the raw sort-map indexing decided by aborting.
fn string_ref(draft: &ImageDraft, id: StrId, site: &'static str) -> Result<(), ImageBuildError> {
    if (id.raw() as usize) < draft.strings().len() {
        Ok(())
    } else {
        Err(ImageBuildError::InvalidReference(site))
    }
}

fn const_ref(draft: &ImageDraft, id: ConstId) -> Result<(), ImageBuildError> {
    if (id.index() as usize) < draft.consts().len() {
        Ok(())
    } else {
        Err(ImageBuildError::InvalidReference("constant"))
    }
}

fn func_ref(draft: &ImageDraft, raw: u16, site: &'static str) -> Result<(), ImageBuildError> {
    if (raw as usize) < draft.functions().len() {
        Ok(())
    } else {
        Err(ImageBuildError::InvalidReference(site))
    }
}

fn type_row_ref(draft: &ImageDraft, id: TypeId) -> Result<(), ImageBuildError> {
    if (id.index() as usize) < draft.types().len() {
        Ok(())
    } else {
        Err(ImageBuildError::InvalidReference("type table"))
    }
}

fn collection_row_ref(draft: &ImageDraft, id: CollTypeId) -> Result<(), ImageBuildError> {
    if (id.index() as usize) < draft.collections().len() {
        Ok(())
    } else {
        Err(ImageBuildError::InvalidReference("collection type"))
    }
}

/// One `ImageType` reference, resolved against its target domain: Record → TYPES,
/// Enum → ENUMS, Collection → COLLTYPES, Identity → ROOTS. A check consulting the
/// wrong table would accept an ordinal the right one refuses.
fn image_type_ref(draft: &ImageDraft, ty: ImageType) -> Result<(), ImageBuildError> {
    match ty {
        ImageType::Unit | ImageType::Scalar { .. } => Ok(()),
        ImageType::Record { idx, .. } => type_row_ref(draft, idx),
        ImageType::Enum { idx, .. } => {
            if (idx.index() as usize) < draft.enums().len() {
                Ok(())
            } else {
                Err(ImageBuildError::InvalidReference("enum type"))
            }
        }
        ImageType::Collection { idx, .. } => collection_row_ref(draft, idx),
        ImageType::Identity { root, .. } => {
            if (root.index() as usize) < draft.root_occurrences().len() {
                Ok(())
            } else {
                Err(ImageBuildError::InvalidReference("root table"))
            }
        }
    }
}

/// Step 3: per function in table order — name, source, signature types (params in
/// order, then the return), then the tape's operands exactly as the tape visits them.
fn function_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for function in draft.functions() {
        string_ref(draft, function.name, "function name")?;
        string_ref(draft, function.source, "function source")?;
        for param in &function.params {
            image_type_ref(draft, *param)?;
        }
        image_type_ref(draft, function.ret)?;
        tape_references(draft, &function.code)?;
    }
    Ok(())
}

/// The tape-order operand checks. Locals, stack-relative field slots, enum payload
/// reads, identity key-path arities, range-guard immediates, and key-slot elements
/// are frame- or runtime-relative, not table ordinals; site operands are opaque
/// provenance-validated typed state checked in step 2 and projected only through the
/// measured plan's site projection before any write — all excluded here.
fn tape_references(draft: &ImageDraft, code: &[Instr]) -> Result<(), ImageBuildError> {
    let instruction_count = code.len();
    let jump_ref = |target: u32| {
        if (target as usize) < instruction_count {
            Ok(())
        } else {
            Err(ImageBuildError::InvalidReference("jump target"))
        }
    };
    for instr in code {
        match instr {
            Instr::ConstLoad(raw) | Instr::Unreachable(raw) | Instr::Todo(raw) => {
                const_ref(draft, *raw)?
            }
            Instr::Call(target) => func_ref(draft, *target, "call target")?,
            Instr::RecordNew(idx) => type_row_ref(draft, *idx)?,
            Instr::ListNew(idx)
            | Instr::MapNew(idx)
            | Instr::TextSplit(idx)
            | Instr::TextLines(idx) => collection_row_ref(draft, *idx)?,
            // The variant is subordinate: it is checked against the resolved enum,
            // not a table of its own.
            Instr::EnumConstruct { enum_idx, variant } => {
                let Some(enum_def) = draft.enums().get(enum_idx.index() as usize) else {
                    return Err(ImageBuildError::InvalidReference("enum type"));
                };
                if (*variant as usize) >= enum_def.variants.len() {
                    return Err(ImageBuildError::InvalidReference("enum type"));
                }
            }
            Instr::VacantLoad(ty) => image_type_ref(draft, *ty)?,
            Instr::DurIterateBounded { list_ty, .. } | Instr::DurIndexScan { list_ty, .. } => {
                collection_row_ref(draft, *list_ty)?
            }
            // `cols` is not an ordinal but is statically decidable: it must equal the
            // referenced root's key arity, and it is reachable only past a valid root.
            Instr::MakeIdentity { root, cols } => {
                let Some(occurrence) = draft.root_occurrences().get(root.index() as usize) else {
                    return Err(ImageBuildError::InvalidReference("root table"));
                };
                if (*cols as usize) != occurrence.keys().len() {
                    return Err(ImageBuildError::InvalidReference("root table"));
                }
            }
            Instr::Jump(target)
            | Instr::JumpIfFalse(target)
            | Instr::BranchPresent(target)
            | Instr::IntAddChecked(target)
            | Instr::IntSubChecked(target)
            | Instr::IntMulChecked(target)
            | Instr::IntNegChecked(target)
            | Instr::IntDivChecked(target)
            | Instr::IntRemChecked(target) => jump_ref(*target)?,
            _ => {}
        }
    }
    Ok(())
}

/// Step 4: the DURABLE body's references, in body order — per occurrence its root
/// name, its entry record, then its member run (a branch's name, record, and
/// descendants at their body positions).
fn durable_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for occurrence in draft.root_occurrences() {
        string_ref(draft, occurrence.name(), "root name")?;
        let declaration = draft.declaration_of(occurrence);
        type_row_ref(draft, declaration.root_entry_record())?;
        let graph = declaration.graph();
        member_references(draft, graph, graph.members())?;
    }
    Ok(())
}

/// One run of declaration members, in declaration order, descending exactly as the
/// body writer does. Field value shapes were validated arena-wide in item (i).
fn member_references(
    draft: &ImageDraft,
    graph: &ProductDeclarationGraph,
    members: &[DeclarationNode],
) -> Result<(), ImageBuildError> {
    for member in members {
        match member.shape() {
            DeclarationMemberShape::Field { .. } => {}
            DeclarationMemberShape::Group { .. } => {
                member_references(draft, graph, graph.members_of(member))?;
            }
            DeclarationMemberShape::Branch { name, record, .. } => {
                string_ref(draft, *name, "branch name")?;
                type_row_ref(draft, *record)?;
                member_references(draft, graph, graph.members_of(member))?;
            }
        }
    }
    Ok(())
}

/// Step 5: TYPES — per record its name, then per field its name and its type
/// reference, in row order.
fn types_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for record in draft.types() {
        string_ref(draft, record.name, "record name")?;
        for field in &record.fields {
            string_ref(draft, field.name, "field name")?;
            image_type_ref(draft, field.ty)?;
        }
    }
    Ok(())
}

/// Step 6: CONSTS — a text constant's string reference. Unconstructible through the
/// public draft API (`intern_text` interns the text itself), held as the section's
/// own check regardless.
fn consts_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for value in draft.consts() {
        if let ConstValue::Text(id) = value {
            string_ref(draft, *id, "text constant")?;
        }
    }
    Ok(())
}

/// Step 7: EXPORTS — every target in range, then the export relations: one export per
/// function, and one row per `ExportId`.
fn exports_relations(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    let rows = draft.export_rows();
    for export in rows {
        func_ref(draft, export.func(), "export target")?;
    }
    for (index, export) in rows.iter().enumerate() {
        for later in &rows[index + 1..] {
            if export.func() == later.func() || export.id() == later.id() {
                return Err(ImageBuildError::InvalidReference("export table"));
            }
        }
    }
    Ok(())
}

/// Step 8: SPANS — every span's instruction index names an instruction of its
/// function.
fn span_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for function in draft.functions() {
        for span in &function.spans {
            if (span.instr_index as usize) >= function.code.len() {
                return Err(ImageBuildError::InvalidReference("span instruction"));
            }
        }
    }
    Ok(())
}

/// Step 9: TEST-ENTRY — names and targets in range, then the test relations the
/// verifier's seal phase rechecks independently: unique names, unique targets, the
/// assert-membership law, export/test disjointness, the signature law (zero
/// parameters first, then the unit return), the no-calls-into-a-test-entry law, and
/// the driver-mix law (a direct durable operation beside a drive of a
/// transaction-owning callee).
fn test_entry_relations(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    let entries = draft.test_entry_rows();
    for entry in entries {
        string_ref(draft, entry.name(), "test name")?;
    }
    for entry in entries {
        func_ref(draft, entry.func(), "test target")?;
    }
    for (index, entry) in entries.iter().enumerate() {
        for later in &entries[index + 1..] {
            if entry.name() == later.name() || entry.func() == later.func() {
                return Err(ImageBuildError::InvalidReference("test table"));
            }
        }
    }
    let is_test_entry = |func: u16| entries.iter().any(|entry| entry.func() == func);
    for (index, function) in draft.functions().iter().enumerate() {
        let has_assert = function
            .code
            .iter()
            .any(|instr| matches!(instr, Instr::Assert));
        if has_assert && !is_test_entry(index as u16) {
            return Err(ImageBuildError::InvalidReference("test table"));
        }
    }
    for entry in entries {
        let function = &draft.functions()[entry.func() as usize];
        if draft
            .export_rows()
            .iter()
            .any(|export| export.func() == entry.func())
        {
            return Err(ImageBuildError::InvalidReference("test table"));
        }
        if !function.params.is_empty() {
            return Err(ImageBuildError::InvalidReference("test table"));
        }
        if function.ret != ImageType::Unit {
            return Err(ImageBuildError::InvalidReference("test table"));
        }
    }
    for function in draft.functions() {
        for instr in &function.code {
            if let Instr::Call(target) = instr
                && is_test_entry(*target)
            {
                return Err(ImageBuildError::InvalidReference("test table"));
            }
        }
    }
    for entry in entries {
        let function = &draft.functions()[entry.func() as usize];
        let has_direct_durable = function
            .code
            .iter()
            .any(|instr| instr.site_operand().is_some());
        if !has_direct_durable {
            continue;
        }
        let drives_owner = function.code.iter().any(|instr| {
            matches!(instr, Instr::Call(target) if draft.functions()[*target as usize]
                .code
                .iter()
                .any(|callee_instr| matches!(callee_instr, Instr::TxnBegin)))
        });
        if drives_owner {
            return Err(ImageBuildError::InvalidReference("test table"));
        }
    }
    Ok(())
}

/// Step 10: ENUMS — per definition its name, then per variant its name and its
/// payload type references, in row order.
fn enums_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for enum_def in draft.enums() {
        string_ref(draft, enum_def.name, "enum name")?;
        for variant in &enum_def.variants {
            string_ref(draft, variant.name, "variant name")?;
            for ty in &variant.payload {
                image_type_ref(draft, *ty)?;
            }
        }
    }
    Ok(())
}

/// Step 11: COLLTYPES — per row its element (List) or key-then-value (Map) type
/// references.
fn collections_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for coll in draft.collections() {
        match coll {
            CollectionTypeDef::List { elem } => image_type_ref(draft, *elem)?,
            CollectionTypeDef::Map { key, value } => {
                image_type_ref(draft, *key)?;
                image_type_ref(draft, *value)?;
            }
        }
    }
    Ok(())
}

/// Decisive-saturation pins: on every over-ceiling corpus family the counting sink's
/// final value is exactly [`DECISIVE_TOTAL`] — accumulation saturates there and every
/// counting row loop polls fullness — so over-ceiling measurement performs only
/// decisive capped work and the `N`/`N + 1` boundary is byte-exact. The boundary
/// corpora in `tests/ceiling_boundary.rs` pin the same families end to end; these
/// unit pins see the sink itself.
#[cfg(test)]
mod decisive_saturation {
    use super::*;
    use crate::draft::{FunctionDef, RootOccurrenceDef, SpanEntry};
    use crate::product::{DeclarationMemberDef, DeclarationMemberShape};
    use crate::ty::Scalar;

    fn saturated(drive: impl FnOnce(&mut CappedImageCount)) -> usize {
        let mut counter = CappedImageCount::default();
        drive(&mut counter);
        counter.total()
    }

    #[test]
    fn an_over_ceiling_span_table_saturates_at_the_decisive_byte() {
        let mut draft = ImageDraft::new();
        let src = draft.intern_string("s").expect("a within-domain mint");
        let name = draft.intern_string("f").expect("a within-domain mint");
        draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                spans: vec![
                    SpanEntry {
                        instr_index: 0,
                        line: 1,
                        column: 1,
                    };
                    44_000
                ],
                code: vec![Instr::Return],
            })
            .expect("no site operand needs validating");
        assert_eq!(
            saturated(|counter| count_spans(&draft, counter)),
            DECISIVE_TOTAL,
        );
    }

    #[test]
    fn an_over_ceiling_function_table_saturates_at_the_decisive_byte() {
        let mut draft = ImageDraft::new();
        let src = draft.intern_string("s").expect("a within-domain mint");
        let name = draft.intern_string("f").expect("a within-domain mint");
        let zero = draft.intern_int(0).expect("a within-domain mint");
        let body: Vec<Instr> = std::iter::repeat_n(Instr::ConstLoad(zero), 400)
            .chain([Instr::Return])
            .collect();
        for _ in 0..512 {
            draft
                .add_function(FunctionDef {
                    name,
                    source: src,
                    params: Vec::new(),
                    ret: ImageType::Unit,
                    local_count: 0,
                    spans: Vec::new(),
                    code: body.clone(),
                })
                .expect("no site operand needs validating");
        }
        assert_eq!(
            saturated(|counter| count_functions(&draft, counter)),
            DECISIVE_TOTAL,
        );
    }

    #[test]
    fn an_over_ceiling_string_pool_saturates_at_the_decisive_byte() {
        let mut draft = ImageDraft::new();
        for index in 0..200 {
            draft
                .intern_string(&format!("{index:04}{}", "x".repeat(3_996)))
                .expect("a within-domain mint");
        }
        assert_eq!(
            saturated(|counter| {
                draft.encode_strings(&mut SectionSink::over(counter), 0..draft.strings().len());
            }),
            DECISIVE_TOTAL,
        );
    }

    #[test]
    fn an_over_ceiling_durable_expansion_saturates_at_the_decisive_byte() {
        let mut draft = ImageDraft::new();
        draft.set_application_identity(crate::durable_id::LedgerIdBytes::from_bytes([0x01; 16]));
        let value = {
            let values = draft.value_shapes_mut();
            let mut level = values.scalar(Scalar::Int);
            for _ in 0..31 {
                level = values.struct_shape(vec![level; 64]);
            }
            level
        };
        let type_name = draft.intern_string("R").expect("a within-domain mint");
        let record = draft
            .add_record_type(crate::draft::RecordTypeDef {
                name: type_name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
        let plan = crate::draft::AdmittedGraphInputPlan::admit(1, 1, 4)
            .expect("a small census is admitted");
        let product = crate::durable_id::LedgerIdBytes::from_bytes([0x0d; 16]);
        draft
            .declare_product(
                &plan,
                product,
                record,
                vec![DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: crate::durable_id::LedgerIdBytes::from_bytes([0x0e; 16]),
                        required: true,
                        value,
                    },
                }],
            )
            .expect("a well-formed declaration");
        let root_name = draft.intern_string("r").expect("a within-domain mint");
        draft
            .add_root_occurrence(
                &plan,
                product,
                RootOccurrenceDef {
                    name: root_name,
                    keys: Vec::new(),
                    placement: crate::durable_id::LedgerIdBytes::from_bytes([0x0b; 16]),
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");
        assert_eq!(
            saturated(|counter| {
                draft
                    .write_durable_body(counter, &StringRemap::counting())
                    .expect("the capped count stops early");
            }),
            DECISIVE_TOTAL,
        );
    }
}
