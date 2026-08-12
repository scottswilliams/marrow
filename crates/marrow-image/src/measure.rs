//! The legacy-v0 measure core: coherence, policy, capped measurement, and planned
//! emission, in that order (design §B/§C/§D/§G).
//!
//! [`ImageDraft::encode`] drives four affine steps, each consuming the previous one:
//!
//! 1. **Coherence** ([`LegacyV0MeasureCore::coherence`]): every invariant-classified
//!    decision — the fixed per-construct widths, the durable graph walks, the
//!    application anchor, the site projection, and every structural reference the
//!    emitted sections would resolve — in the exact relative order the legacy encoder
//!    decided them (the stable partition; see the sequence note below). A coherence
//!    failure returns the existing invariant error before any policy candidate.
//! 2. **Policy** ([`CoherentDraft::policy`]): the resource-policy candidates in the
//!    exact legacy candidate order — the eleven aggregate caps, then per-function
//!    CodeBytes. A policy failure returns without measuring, hashing, or allocating.
//! 3. **Measurement** ([`PolicyClean::measure`]): every section counted through the
//!    one checked sink, in section order, driving the same sink-generic writers
//!    emission uses — base rows and constant tokens, whose lengths the
//!    counted==emitted KATs prove order-independent — saturating decisively one byte
//!    past [`bounds::MAX_IMAGE_BYTES`]. The result is an affine [`LegacyV0WirePlan`]:
//!    ten inline section lengths and a scalar total, no heap, or
//!    [`ImageBuildError::ImageTooLarge`] with nothing retained. The final whole-image
//!    ceiling is decided here, envelope and frames included, before any section is
//!    assembled.
//! 4. **Emission** ([`LegacyV0WirePlan::emit_image`]): the plan is consumed; each
//!    section is built through the same writer with the real permutations and tokens,
//!    compared byte-for-byte in length against the plan, and a disagreement is the
//!    typed [`ImageBuildError::EncodeDrift`] naming the drifted section. The durable
//!    contract identity is minted exactly once, inside `emit_image`.
//!
//! # The coherence sequence (stable partition)
//!
//! Step 1 preserves the legacy decision-site order: the `check_bounds` invariant
//! subsequence (per-record fields → per-enum definition widths → the Product claim
//! conflict → the per-declaration graph walk → the whole-arena value-shape walk →
//! per-occurrence key/index widths → per-function frames), then the application
//! anchor and the site projection with operand provenance, then the emission-order
//! reference checks hoisted from the writers: per function (name → source → signature
//! types → tape operands in tape order), the DURABLE body (root names, entry records,
//! branch names and records, in body order), TYPES, CONSTS, EXPORTS (targets and the
//! export relations), SPANS, TEST-ENTRY (names, targets, and the test relations),
//! ENUMS, COLLTYPES. Every hoisted check is a range check over a local ordinal (or a
//! locally decidable relation); an in-range id keeps its local meaning, and the
//! independent verifier remains the only decoder.
//!
//! # Allocation posture and the max-live term (LSPCAP pre-join term of record)
//!
//! The core's own state — the coherence walk (including its streaming site-projection
//! validation, which materializes no path), the policy audit, the counting sink, the
//! measured-length witness, and the plan — is zero-heap. What measurement drives is
//! the installed writers, whose own scratch predates the core and is unchanged. The
//! max-live expression therefore charges, beyond `B_LEGACY_DRAFT` (the retained draft
//! baseline) and the fitting emitter's post-`Fits` sort/section/output topology:
//!
//! - **zero bytes** for coherence, the policy audit, the sink (`usize`), the witness
//!   (`u32`), and the plan (`[u32; 10]` + two scalars, inline);
//! - the DURABLE traversal's ceiling-bounded expansion worklist — the one sanctioned
//!   pre-verdict allocation (stop adjudication of record). Measured on the 31-level
//!   64-edge compact-expansion regression: peak 1,954 pending tasks × 16 bytes/task
//!   = 31,264 bytes; `Vec`-doubling growth, so the transient old/new-buffer overlap
//!   is bounded by 2× that capacity;
//! - the site projection's transient per-row path, live one row at a time during the
//!   DURABLE counting and emission runs only: ≤ `MAX_SITE_PATH_STEPS` (18) steps ×
//!   17 bytes/step = 306 bytes + one `Vec` header (the step-count-first site codec
//!   is why the writer consumes a materialized path; the coherence walk streams);
//! - the tape codec's per-function layout scratch during the functions-section
//!   counting and emission runs. Measured on the full 4,096-row function partition:
//!   the per-function vector 4,096 × 32 bytes = 131,072 bytes, plus exact-capacity
//!   offset vectors totalling 1,064,960 bytes (widest single vector 260 bytes);
//!   both die with the run that built them.
//!
//! The `#[ignore]`d measurement harnesses that produced these numbers live beside
//! the owners they measure (`value_dag.rs`, `encode.rs`); re-run them with
//! `--ignored` when a representation widens, and re-gate through LSPCAP.

use crate::bounds;
use crate::digest::image_id;
use crate::draft::{CollectionTypeDef, ConstValue, ImageBuildError, ImageDraft, StrId};
use crate::durable_id::{DurableContractId, DurableGraphTooLarge};
use crate::encode::{
    EncodedImage, HEADER_BYTES, MAGIC, SECTION_COUNT, VERSION, laid_out_code_len, push_section,
    remap_of,
};
use crate::instr::Instr;
use crate::product::{
    DeclarationMemberShape, DeclarationNode, ProductClaimConflict, ProductDeclarationGraph,
};
use crate::remap::{ConstRemap, StringRemap};
use crate::ty::ImageType;
use crate::value_dag::{CanonicalValueShapeDag, ImageByteSink, ValueShapeView};

/// The wire ids of the ten sections, in assembly order, and the envelope marker.
const SECTION_IDS: [u8; 10] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A];
/// The plan slot of the DURABLE section (wire id 0x03).
const DURABLE_SLOT: usize = 2;
/// One section frame: `u8(id) ‖ u32(body_len)`.
const SECTION_FRAME_BYTES: usize = 5;

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

/// The measured wire plan: ten inline section body lengths and the whole-image total,
/// envelope and frames included. No heap; affine — emission consumes it.
pub(crate) struct LegacyV0WirePlan<'d> {
    draft: &'d ImageDraft,
    sections: [u32; 10],
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
        Ok(PolicyClean(draft))
    }
}

impl<'d> PolicyClean<'d> {
    /// Step 5: count all ten sections through one checked sink, in section order,
    /// driving the same sink-generic writers emission uses over the base rows with
    /// constant tokens. The sink saturates decisively one byte past the whole-image
    /// ceiling, and the plan constructor decides the ceiling over the full total —
    /// envelope and frames included — so a final-only overage refuses here, before
    /// any section is assembled.
    pub(crate) fn measure(self) -> Result<LegacyV0WirePlan<'d>, ImageBuildError> {
        let draft = self.0;
        let strings = StringRemap::counting();
        let consts = ConstRemap::counting();
        let mut counter = CappedImageCount::default();
        let mut sections = [0u32; 10];

        sections[0] = counter.measured(|sink| {
            draft.encode_strings(sink, 0..draft.strings().len());
            Ok(())
        })?;
        sections[1] = counter.measured(|sink| {
            draft.encode_types(sink, &strings);
            Ok(())
        })?;
        // The DURABLE counting run: the one installed writer drives the one installed
        // expansion, exactly as emission will, and the section closes with the 32-byte
        // contract identity the emission run appends after the body.
        let durable = MeasuredDurableLen(counter.measured(|sink| {
            draft.write_durable_body(sink, &strings)?;
            sink.pad(DurableContractId::BYTES);
            Ok(())
        })?);
        sections[3] = counter.measured(|sink| {
            draft.encode_consts(sink, &strings, 0..draft.consts().len());
            Ok(())
        })?;
        let mut per_fn = Vec::new();
        sections[4] = counter.measured(|sink| {
            per_fn = draft.encode_functions(sink, &strings, &consts)?;
            Ok(())
        })?;
        sections[5] = counter.measured(|sink| {
            draft.encode_exports(sink, 0..draft.export_count());
            Ok(())
        })?;
        sections[6] = counter.measured(|sink| {
            draft.encode_spans(sink, &per_fn);
            Ok(())
        })?;
        sections[7] = counter.measured(|sink| {
            draft.encode_test_entries(sink, &strings, 0..draft.test_entry_count());
            Ok(())
        })?;
        sections[8] = counter.measured(|sink| {
            draft.encode_enums(sink, &strings);
            Ok(())
        })?;
        sections[9] = counter.measured(|sink| {
            draft.encode_collections(sink);
            Ok(())
        })?;
        LegacyV0WirePlan::new(draft, sections, durable)
    }
}

impl<'d> LegacyV0WirePlan<'d> {
    /// Complete the plan and decide the whole-image ceiling over the full framed
    /// total. Demanding the witness is what makes a plan unforgeable: only the
    /// counting run over the installed DURABLE writer can mint one.
    fn new(
        draft: &'d ImageDraft,
        mut sections: [u32; 10],
        durable: MeasuredDurableLen,
    ) -> Result<Self, ImageBuildError> {
        sections[DURABLE_SLOT] = durable.0;
        let framed: usize = sections
            .iter()
            .map(|len| SECTION_FRAME_BYTES + *len as usize)
            .sum();
        let total = HEADER_BYTES + 1 + framed;
        if total > bounds::MAX_IMAGE_BYTES {
            return Err(ImageBuildError::ImageTooLarge);
        }
        Ok(Self {
            draft,
            sections,
            // The ceiling was just decided, so the total fits far inside `u32`.
            total: total as u32,
        })
    }

    /// Step 6: consume the plan and emit the image — the real permutations and remap
    /// tokens through the same writers measurement counted, every section length
    /// compared to the plan, the contract identity minted exactly once, and the
    /// assembled envelope compared to the planned total.
    pub(crate) fn emit_image(self) -> Result<EncodedImage, ImageBuildError> {
        let Self {
            draft,
            sections,
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

        let mut tail = Vec::with_capacity(total as usize - HEADER_BYTES);
        tail.push(SECTION_COUNT);
        let emit = |tail: &mut Vec<u8>, slot: usize, body: Vec<u8>| {
            if body.len() != sections[slot] as usize {
                return Err(ImageBuildError::EncodeDrift(EncodeDriftSection::of(
                    SECTION_IDS[slot],
                )));
            }
            push_section(tail, SECTION_IDS[slot], body)
        };

        let mut body = Vec::with_capacity(sections[0] as usize);
        draft.encode_strings(&mut body, string_order.iter().copied());
        emit(&mut tail, 0, body)?;

        let mut body = Vec::with_capacity(sections[1] as usize);
        draft.encode_types(&mut body, &strings);
        emit(&mut tail, 1, body)?;

        // The DURABLE body the plan measured, written this time, closed by the 32-byte
        // durable-contract identity: the one mint, for a graph the plan already fits.
        let mut body = Vec::with_capacity(sections[DURABLE_SLOT] as usize);
        draft.write_durable_body(&mut body, &strings)?;
        let identity = draft
            .contract_view()
            .contract_id()
            .map_err(|DurableGraphTooLarge| ImageBuildError::ImageTooLarge)?;
        body.extend_from_slice(identity.bytes());
        emit(&mut tail, DURABLE_SLOT, body)?;

        let mut body = Vec::with_capacity(sections[3] as usize);
        draft.encode_consts(&mut body, &strings, const_order.iter().copied());
        emit(&mut tail, 3, body)?;

        let mut body = Vec::with_capacity(sections[4] as usize);
        let per_fn = draft.encode_functions(&mut body, &strings, &consts)?;
        emit(&mut tail, 4, body)?;

        let export_order = draft.export_permutation();
        let mut body = Vec::with_capacity(sections[5] as usize);
        draft.encode_exports(&mut body, export_order.iter().copied());
        emit(&mut tail, 5, body)?;

        let mut body = Vec::with_capacity(sections[6] as usize);
        draft.encode_spans(&mut body, &per_fn);
        emit(&mut tail, 6, body)?;

        let test_entry_order = draft.test_entry_permutation(&str_map);
        let mut body = Vec::with_capacity(sections[7] as usize);
        draft.encode_test_entries(&mut body, &strings, test_entry_order.iter().copied());
        emit(&mut tail, 7, body)?;

        let mut body = Vec::with_capacity(sections[8] as usize);
        draft.encode_enums(&mut body, &strings);
        emit(&mut tail, 8, body)?;

        let mut body = Vec::with_capacity(sections[9] as usize);
        draft.encode_collections(&mut body);
        emit(&mut tail, 9, body)?;

        let id = image_id(&tail);
        let mut bytes = Vec::with_capacity(HEADER_BYTES + tail.len());
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&id.0);
        bytes.extend_from_slice(&tail);
        if bytes.len() != total as usize {
            return Err(ImageBuildError::EncodeDrift(EncodeDriftSection::envelope()));
        }
        Ok(EncodedImage {
            bytes,
            image_id: id,
        })
    }
}

/// The one checked counting sink: it keeps nothing and saturates decisively one byte
/// past the whole-image ceiling, so "full" and "fits" stay distinguishable and the
/// work an over-ceiling draft costs is the bytes the ceiling admits.
#[derive(Default)]
struct CappedImageCount(usize);

impl CappedImageCount {
    /// Run one section's writer against this sink and return the section's byte
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

    /// Count fixed trailing bytes a writer does not produce (the contract identity).
    fn pad(&mut self, bytes: usize) {
        self.0 += bytes;
    }
}

impl ImageByteSink for CappedImageCount {
    fn push(&mut self, _byte: u8) {
        self.0 += 1;
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        self.0 += bytes.len();
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
    for (node, depth) in graph.rows().iter().zip(graph.depths()) {
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
    // Operand/receipt provenance — the plan's `validate`, never `encodable`: an
    // over-policy operand is live provenance the Sites policy candidate reports.
    for function in draft.functions() {
        for instr in &function.code {
            if let Some(site) = instr.site_operand()
                && !draft.site_operand_is_live(site)
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

fn const_ref(draft: &ImageDraft, raw: u16) -> Result<(), ImageBuildError> {
    if (raw as usize) < draft.consts().len() {
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

fn type_row_ref(draft: &ImageDraft, raw: u16) -> Result<(), ImageBuildError> {
    if (raw as usize) < draft.types().len() {
        Ok(())
    } else {
        Err(ImageBuildError::InvalidReference("type table"))
    }
}

fn collection_row_ref(draft: &ImageDraft, raw: u16) -> Result<(), ImageBuildError> {
    if (raw as usize) < draft.collections().len() {
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
            if (idx as usize) < draft.enums().len() {
                Ok(())
            } else {
                Err(ImageBuildError::InvalidReference("enum type"))
            }
        }
        ImageType::Collection { idx, .. } => collection_row_ref(draft, idx),
        ImageType::Identity { root, .. } => {
            if (root as usize) < draft.root_occurrences().len() {
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
/// provenance-validated typed state checked in step 2 and consumed via `encodable`
/// before any write — all excluded here.
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
                let Some(enum_def) = draft.enums().get(*enum_idx as usize) else {
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
                let Some(occurrence) = draft.root_occurrences().get(*root as usize) else {
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
        type_row_ref(draft, declaration.root_entry_record().0)?;
        let graph = declaration.graph();
        member_references(draft, graph, graph.members())?;
    }
    Ok(())
}

/// One run of declaration members, in declaration order, descending exactly as the
/// body writer does. Field value shapes were validated arena-wide in step 1.
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
                type_row_ref(draft, record.0)?;
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
