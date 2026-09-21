//! Coherence, policy, and the one capped emission pass behind [`ImageDraft::encode`].
//!
//! 1. Coherence ([`CoherentDraft::of`]): every invariant-classified decision — the fixed
//!    per-construct widths, the durable graph walks, the application anchor, the site
//!    projection, and every structural reference the sections resolve — in emission
//!    order, before any policy candidate. Each hoisted check is a range check over a
//!    local ordinal or a locally decidable relation; the verifier stays the only decoder.
//! 2. Policy ([`CoherentDraft::check_policy`]): the aggregate caps, then per-function
//!    CodeBytes, returning without hashing or allocating.
//! 3. Emission ([`CoherentDraft::emit_image`]): the image is written once into a sink that
//!    stops one byte past [`bounds::MAX_IMAGE_BYTES`]; a saturated sink is
//!    [`ImageBuildError::ImageTooLarge`], and the durable contract identity is minted
//!    exactly once, closing the DURABLE body.
//!
//! Pre-verdict heap scratch is population-bounded: one byte per retained function for the
//! export/test relations, and the DURABLE traversal's worklist.
use std::collections::HashSet;

use crate::bounds;
use crate::digest::image_id;
use crate::draft::{
    CollTypeId, CollectionTypeDef, ConstId, ConstValue, FillState, ImageBuildError, ImageDraft,
    ReferenceKind, StrId, TypeId,
};
use crate::durable_id::DurableGraphTooLarge;
use crate::encode::{
    EncodedImage, SECTION_COUNT, laid_out_code_len, push_frame, remap_of, write_image_header,
};
use crate::instr::Instr;
use crate::product::{
    DeclarationMemberShape, DeclarationNode, ProductClaimConflict, ProductDeclarationGraph,
};
use crate::remap::{ConstRemap, SectionSink, StringRemap};
use crate::ty::ImageType;
use crate::value_dag::{CanonicalValueShapeDag, ImageByteSink, ValueShapeView};

/// The policy-clean `u16` narrowing of one owned wide logical ordinal — the one
/// sanctioned narrowing direction for an owned pre-seal id.
///
/// Every caller runs strictly after [`CoherentDraft::check_policy`] proved every owned
/// table within its maximum, and `crate::bounds` const-asserts each of those maxima at
/// or below `u16::MAX`, so the conversion is total there. It is spelled checked so a
/// value outside the proved envelope is a producer invariant, never a wrapped wire
/// value.
pub(crate) fn wire_ordinal(ordinal: u32) -> u16 {
    u16::try_from(ordinal).expect("a policy-clean owned ordinal fits the u16 wire domain")
}

/// The policy-clean `u16` narrowing of one owned row or byte count, on the same
/// implication as [`wire_ordinal`].
pub(crate) fn wire_len(count: usize) -> u16 {
    u16::try_from(count).expect("a policy-clean owned count fits the u16 wire domain")
}

/// A draft every invariant-classified decision has admitted. Minted only by
/// [`CoherentDraft::of`], and bound by the lifetime of the immutable draft it
/// certifies.
pub(crate) struct CoherentDraft<'d>(&'d ImageDraft);

impl<'d> CoherentDraft<'d> {
    /// Step 1: the complete coherence walk (see the module doc for the sequence).
    pub(crate) fn of(draft: &ImageDraft) -> Result<CoherentDraft<'_>, ImageBuildError> {
        invariant_bounds(draft)?;
        anchor_and_sites(draft)?;
        function_references(draft)?;
        durable_references(draft)?;
        types_references(draft)?;
        consts_references(draft)?;
        let mut function_relations = exports_relations(draft)?;
        span_references(draft)?;
        test_entry_relations(draft, &mut function_relations)?;
        enums_references(draft)?;
        collections_references(draft)?;
        Ok(CoherentDraft(draft))
    }

    /// Every function slot was checked by coherence. Borrow the definitions in
    /// reservation order without compacting or copying the owner's table.
    pub(crate) fn functions(&self) -> impl ExactSizeIterator<Item = &crate::FunctionDef> {
        self.0.functions().iter().map(|slot| {
            slot.as_ref()
                .expect("coherence requires every function body")
        })
    }

    /// Step 2: the resource-policy walk, in candidate order — the eleven aggregate
    /// caps, then per-function CodeBytes in function order. The first cap a draft
    /// crosses names the refusal. Nothing is hashed or allocated here.
    pub(crate) fn check_policy(&self) -> Result<(), ImageBuildError> {
        if self.strings().len() > bounds::MAX_STRINGS {
            return Err(ImageBuildError::TooManyStrings);
        }
        for text in self.strings() {
            if text.len() > bounds::MAX_STRING_BYTES {
                return Err(ImageBuildError::StringTooLong);
            }
        }
        if self.consts().len() > bounds::MAX_CONSTS {
            return Err(ImageBuildError::TooManyConsts);
        }
        if self.types().len() > bounds::MAX_TYPES {
            return Err(ImageBuildError::TooManyTypes);
        }
        if self.enums().len() > bounds::MAX_ENUMS {
            return Err(ImageBuildError::TooManyEnums);
        }
        if self.collections().len() > bounds::MAX_COLLECTIONS {
            return Err(ImageBuildError::TooManyCollections);
        }
        if self.root_occurrences().len() > bounds::MAX_ROOTS {
            return Err(ImageBuildError::TooManyRoots);
        }
        if self.site_demand() > bounds::MAX_SITES {
            return Err(ImageBuildError::TooManySites);
        }
        if self.0.functions().len() > bounds::MAX_FUNCTIONS {
            return Err(ImageBuildError::TooManyFunctions);
        }
        if self.export_count() > bounds::MAX_EXPORTS {
            return Err(ImageBuildError::TooManyExports);
        }
        if self.test_entry_count() > bounds::MAX_TEST_ENTRIES {
            return Err(ImageBuildError::TooManyTestEntries);
        }
        for function in self.functions() {
            if laid_out_code_len(&function.code)? > bounds::MAX_CODE_BYTES as u64 {
                return Err(ImageBuildError::CodeTooLong);
            }
        }
        Ok(())
    }

    /// The projection over this draft's sites.
    pub(crate) fn site_projection(&self) -> SiteWireProjection<'d> {
        SiteWireProjection(self.0)
    }

    /// Step 3: write the image once. The canonical permutations and remap tokens feed
    /// the section writers, each section's frame is patched with the body length its
    /// writer produced, and the digest is computed over the emitted tail and written
    /// back into the reserved head slot.
    pub(crate) fn emit_image(self) -> Result<EncodedImage, ImageBuildError> {
        let draft = self.0;
        let sites = self.site_projection();

        // Row law: the canonical orders are one permutation each over the retained
        // base rows; every reference resolves through the permutation's inverse, read
        // by the writers only as opaque tokens.
        let string_order = draft.string_permutation();
        let str_map = remap_of(&string_order);
        let strings = StringRemap::new(&str_map);
        let const_order = draft.const_permutation(&str_map);
        let const_map = remap_of(&const_order);
        let consts = ConstRemap::new(&const_map);
        let export_order = draft.export_permutation();
        let test_entry_order = draft.test_entry_permutation(&str_map);

        let mut out = CappedImage::default();
        write_image_header(&mut out, &[0u8; 32]);
        // The head is fixed width, so the digest input begins exactly here.
        let tail_start = out.len();
        out.push(SECTION_COUNT);

        out.section(0x01, |sink| {
            draft.encode_strings(&mut SectionSink::over(sink), string_order.iter().copied());
            Ok(())
        })?;
        out.section(0x02, |sink| {
            draft.encode_types(&mut SectionSink::over(sink), &strings);
            Ok(())
        })?;
        // The DURABLE body, closed by the 32-byte durable-contract identity: the one
        // mint. A preimage refusal is the same whole-image ceiling verdict the sink
        // reaches — the derivation `durable_id` const-asserts bounds the preimage by
        // the body, so a fitting body cannot refuse.
        out.section(0x03, |sink| {
            draft.write_durable_body(sink, &strings)?;
            if sink.is_full() {
                return Ok(());
            }
            let identity = draft
                .contract_view()
                .contract_id()
                .map_err(|DurableGraphTooLarge| ImageBuildError::ImageTooLarge)?;
            sink.extend_bytes(identity.bytes());
            Ok(())
        })?;
        out.section(0x04, |sink| {
            draft.encode_consts(
                &mut SectionSink::over(sink),
                &strings,
                const_order.iter().copied(),
            );
            Ok(())
        })?;
        let mut per_fn = Vec::new();
        out.section(0x05, |sink| {
            per_fn =
                self.encode_functions(&mut SectionSink::over(sink), &strings, &consts, &sites)?;
            Ok(())
        })?;
        out.section(0x06, |sink| {
            draft.encode_exports(&mut SectionSink::over(sink), export_order.iter().copied());
            Ok(())
        })?;
        out.section(0x07, |sink| {
            self.encode_spans(&mut SectionSink::over(sink), &per_fn);
            Ok(())
        })?;
        out.section(0x08, |sink| {
            draft.encode_test_entries(
                &mut SectionSink::over(sink),
                &strings,
                test_entry_order.iter().copied(),
            );
            Ok(())
        })?;
        out.section(0x09, |sink| {
            draft.encode_enums(&mut SectionSink::over(sink), &strings);
            Ok(())
        })?;
        out.section(0x0A, |sink| {
            draft.encode_collections(&mut SectionSink::over(sink));
            Ok(())
        })?;

        let mut bytes = out.bytes;
        let image_id = image_id(&bytes[tail_start..]);
        // The digest is the head's last field.
        bytes[tail_start - image_id.0.len()..tail_start].copy_from_slice(&image_id.0);
        Ok(EncodedImage { bytes, image_id })
    }
}

impl std::ops::Deref for CoherentDraft<'_> {
    type Target = ImageDraft;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

/// The site projection: the only path from a [`crate::PlannedSiteRef`] to its wire
/// ordinal. It is minted only by a [`CoherentDraft`], and each projection
/// revalidates the ref's provenance against the live plan and graph before yielding
/// the ordinal.
pub(crate) struct SiteWireProjection<'d>(&'d ImageDraft);

impl SiteWireProjection<'_> {
    /// The exact wire ordinal of one validated fitting site ref.
    pub(crate) fn ordinal(&self, site: &crate::PlannedSiteRef) -> Result<u16, ImageBuildError> {
        self.0.site_wire_ordinal(site)
    }
}

/// The decisive sentinel: one byte past the whole-image ceiling, where the emitting
/// sink saturates and stays.
const DECISIVE_TOTAL: usize = bounds::MAX_IMAGE_BYTES + 1;

/// The one emitting sink: it appends image bytes until the decisive byte and refuses
/// every byte after it, and every independently unbounded or outer row loop polls
/// [`ImageByteSink::is_full`] (bounded inner runs are covered by coherence's own
/// width bounds). An over-ceiling draft therefore costs the bytes the ceiling admits
/// and no more, "full" and "fits" stay distinguishable, and the ceiling verdict comes
/// from the bytes actually written rather than from a second measuring pass.
#[derive(Default)]
pub(crate) struct CappedImage {
    bytes: Vec<u8>,
}

impl CappedImage {
    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Write one framed section: reserve its `u8(id) ‖ u32(len)` frame, run the body
    /// writer, refuse a saturated sink, then patch the measured body length back into
    /// the reserved prefix.
    fn section(
        &mut self,
        id: u8,
        write: impl FnOnce(&mut Self) -> Result<(), ImageBuildError>,
    ) -> Result<(), ImageBuildError> {
        let frame_at = self.bytes.len();
        push_frame(self, id, 0);
        let body_start = self.bytes.len();
        write(self)?;
        if self.is_full() {
            return Err(ImageBuildError::ImageTooLarge);
        }
        // Within the ceiling the sink just re-proved, so the body length fits `u32`.
        let body_len = (self.bytes.len() - body_start) as u32;
        self.bytes[frame_at + 1..body_start].copy_from_slice(&body_len.to_be_bytes());
        Ok(())
    }
}

impl ImageByteSink for CappedImage {
    fn push(&mut self, byte: u8) {
        if self.bytes.len() < DECISIVE_TOTAL {
            self.bytes.push(byte);
        }
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        let room = DECISIVE_TOTAL - self.bytes.len();
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    fn is_full(&self) -> bool {
        self.bytes.len() > bounds::MAX_IMAGE_BYTES
    }
}

// ---- The invariant-bounds subsequence.

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
        let function = function.as_ref().ok_or(ImageBuildError::InvalidReference(
            ReferenceKind::VacantFunction,
        ))?;
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
/// wider range) is the typed `InvalidReference(ReferenceKind::ValueShape)` rather than an abort.
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
                let Some(value_depth) = values.depth(*value) else {
                    return Err(ImageBuildError::InvalidReference(ReferenceKind::ValueShape));
                };
                if value_depth > bounds::MAX_DURABLE_VALUE_DEPTH {
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
/// so a well-formed draft always encodes within the limits the verifier rechecks.
/// Nesting depth is decided per durable field, at its value's root node, by
/// [`validate_declaration_graph`].
///
/// This is one pass over the arena: each distinct shape is measured once however many
/// fields, structs, or enum payloads reference it, and no occurrence is expanded. The
/// arena is validated **as a whole**, not through the fields that reference it: a node
/// no declaration reaches is measured like any other, so every downstream owner may
/// assume an encoded arena is within the value bounds, whatever part a walk visits.
fn validate_value_shapes(values: &CanonicalValueShapeDag) -> Result<(), ImageBuildError> {
    for node in values.nodes() {
        let Some(view) = values.view(node) else {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::ValueShape));
        };
        match view {
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

// ---- The application anchor, the site projection, and operand provenance.

fn anchor_and_sites(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    // A non-empty durable graph is anchored by the application's ledger id; the legacy
    // encoder demanded it at the head of the DURABLE section, before any member row.
    if !draft.root_occurrences().is_empty() && draft.application_identity().is_none() {
        return Err(ImageBuildError::InvalidReference(
            ReferenceKind::ApplicationIdentity,
        ));
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
        let function = function.as_ref().ok_or(ImageBuildError::InvalidReference(
            ReferenceKind::VacantFunction,
        ))?;
        for instr in &function.code {
            if let Some(site) = instr.site()
                && !draft.site_ref_is_live(site)
            {
                return Err(ImageBuildError::InvalidReference(
                    ReferenceKind::OperationSite,
                ));
            }
        }
    }
    Ok(())
}

// ---- The emission-order reference checks, hoisted from the writers.

/// One drafted string-pool reference, checked against the pool: the range predicate
/// the raw sort-map indexing decided by aborting.
fn string_ref(draft: &ImageDraft, id: StrId, site: ReferenceKind) -> Result<(), ImageBuildError> {
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
        Err(ImageBuildError::InvalidReference(ReferenceKind::Constant))
    }
}

fn func_ref(draft: &ImageDraft, raw: u16, site: ReferenceKind) -> Result<(), ImageBuildError> {
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
        Err(ImageBuildError::InvalidReference(ReferenceKind::TypeTable))
    }
}

fn collection_row_ref(draft: &ImageDraft, id: CollTypeId) -> Result<(), ImageBuildError> {
    if (id.index() as usize) < draft.collections().len() {
        Ok(())
    } else {
        Err(ImageBuildError::InvalidReference(
            ReferenceKind::CollectionType,
        ))
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
                Err(ImageBuildError::InvalidReference(ReferenceKind::EnumType))
            }
        }
        ImageType::Collection { idx, .. } => collection_row_ref(draft, idx),
        ImageType::Identity { root, .. } => {
            if (root.index() as usize) < draft.root_occurrences().len() {
                Ok(())
            } else {
                Err(ImageBuildError::InvalidReference(ReferenceKind::RootTable))
            }
        }
    }
}

/// per function in table order — name, source, signature types (params in
/// order, then the return), then the tape's operands exactly as the tape visits them.
fn function_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for function in draft.functions() {
        let function = function.as_ref().ok_or(ImageBuildError::InvalidReference(
            ReferenceKind::VacantFunction,
        ))?;
        string_ref(draft, function.name, ReferenceKind::FunctionName)?;
        string_ref(draft, function.source, ReferenceKind::FunctionSource)?;
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
            Err(ImageBuildError::InvalidReference(ReferenceKind::JumpTarget))
        }
    };
    for instr in code {
        match instr {
            Instr::ConstLoad(raw) | Instr::Unreachable(raw) | Instr::Todo(raw) => {
                const_ref(draft, *raw)?
            }
            Instr::Call(target) => func_ref(draft, *target, ReferenceKind::CallTarget)?,
            Instr::RecordNew(idx) => type_row_ref(draft, *idx)?,
            Instr::ListNew(idx)
            | Instr::MapNew(idx)
            | Instr::TextSplit(idx)
            | Instr::TextLines(idx) => collection_row_ref(draft, *idx)?,
            // The variant is subordinate: it is checked against the resolved enum,
            // not a table of its own.
            Instr::EnumConstruct { enum_idx, variant } => {
                let Some(enum_def) = draft.enums().get(enum_idx.index() as usize) else {
                    return Err(ImageBuildError::InvalidReference(ReferenceKind::EnumType));
                };
                if (*variant as usize) >= enum_def.variants.len() {
                    return Err(ImageBuildError::InvalidReference(ReferenceKind::EnumType));
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
                    return Err(ImageBuildError::InvalidReference(ReferenceKind::RootTable));
                };
                if (*cols as usize) != occurrence.keys().len() {
                    return Err(ImageBuildError::InvalidReference(ReferenceKind::RootTable));
                }
            }
            _ => {}
        }
        if let Some(target) = instr.jump_target() {
            jump_ref(*target)?;
        }
    }
    Ok(())
}

/// the DURABLE body's references, in body order — per occurrence its root
/// name, its entry record, then its member run (a branch's name, record, and
/// descendants at their body positions).
fn durable_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for occurrence in draft.root_occurrences() {
        string_ref(draft, occurrence.name(), ReferenceKind::RootName)?;
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
                string_ref(draft, *name, ReferenceKind::BranchName)?;
                type_row_ref(draft, *record)?;
                member_references(draft, graph, graph.members_of(member))?;
            }
        }
    }
    Ok(())
}

/// TYPES — per record its name, then per field its name and its type
/// reference, in row order. A reserved row still `Vacant` at the fence is the
/// coherence invariant: a reservation is a producer promise to fill, distinct from
/// a valid filled-empty definition.
fn types_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    if draft.types_fill().contains(&FillState::Unfilled) {
        return Err(ImageBuildError::InvalidReference(
            ReferenceKind::VacantRecordType,
        ));
    }
    for record in draft.types() {
        string_ref(draft, record.name, ReferenceKind::RecordName)?;
        for field in &record.fields {
            string_ref(draft, field.name, ReferenceKind::FieldName)?;
            image_type_ref(draft, field.ty)?;
        }
    }
    Ok(())
}

/// CONSTS — a text constant's string reference. Unconstructible through the
/// public draft API (`intern_text` interns the text itself), held as the section's
/// own check regardless.
fn consts_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for value in draft.consts() {
        if let ConstValue::Text(id) = value {
            string_ref(draft, *id, ReferenceKind::TextConstant)?;
        }
    }
    Ok(())
}

/// EXPORTS — every target in range, then the export relations: one export per
/// function, and one row per `ExportId`.
struct FunctionRelations {
    flags: Vec<u8>,
}

const EXPORTED_FUNCTION: u8 = 0b01;
const TEST_ENTRY_FUNCTION: u8 = 0b10;

fn exports_relations(draft: &ImageDraft) -> Result<FunctionRelations, ImageBuildError> {
    let rows = draft.export_rows();
    for export in rows {
        func_ref(draft, export.func(), ReferenceKind::ExportTarget)?;
    }

    // Function indices are dense, so one flag byte is the keyed relation owner for
    // both export membership here and test-entry membership below. Export identities
    // have no dense ordinal and use the one keyed set this coherence walk needs. The
    // set is never iterated, so its randomized layout cannot affect a verdict or byte.
    let mut relations = FunctionRelations {
        flags: vec![0; draft.functions().len()],
    };
    let mut seen_ids: HashSet<&crate::export_id::ExportId> = HashSet::with_capacity(rows.len());
    for export in rows {
        let Some(flags) = relations.flags.get_mut(export.func() as usize) else {
            return Err(ImageBuildError::InvalidReference(
                ReferenceKind::ExportTable,
            ));
        };
        if *flags & EXPORTED_FUNCTION != 0 {
            return Err(ImageBuildError::InvalidReference(
                ReferenceKind::ExportTable,
            ));
        }
        *flags |= EXPORTED_FUNCTION;

        if !seen_ids.insert(export.id()) {
            return Err(ImageBuildError::InvalidReference(
                ReferenceKind::ExportTable,
            ));
        }
    }
    Ok(relations)
}

/// SPANS — every span's instruction index names an instruction of its
/// function.
fn span_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    for function in draft.functions() {
        let function = function.as_ref().ok_or(ImageBuildError::InvalidReference(
            ReferenceKind::VacantFunction,
        ))?;
        for span in &function.spans {
            if (span.instr_index as usize) >= function.code.len() {
                return Err(ImageBuildError::InvalidReference(
                    ReferenceKind::SpanInstruction,
                ));
            }
        }
    }
    Ok(())
}

/// TEST-ENTRY — names and targets in range, then the test relations the
/// verifier's seal phase rechecks independently: unique names, unique targets, the
/// assert-membership law, export/test disjointness, the signature law (zero
/// parameters first, then the unit return), the no-calls-into-a-test-entry law, and
/// the absence of direct durable operations in test bodies.
fn test_entry_relations(
    draft: &ImageDraft,
    function_relations: &mut FunctionRelations,
) -> Result<(), ImageBuildError> {
    let entries = draft.test_entry_rows();
    for entry in entries {
        string_ref(draft, entry.name(), ReferenceKind::TestName)?;
    }
    for entry in entries {
        func_ref(draft, entry.func(), ReferenceKind::TestTarget)?;
    }
    // Names are dense string ordinals and targets are dense function ordinals, so
    // both uniqueness laws are direct keyed insertion. The function flags are kept
    // for every membership question that follows.
    let mut seen_names = vec![false; draft.strings().len()];
    for entry in entries {
        let Some(seen_name) = seen_names.get_mut(entry.name().index() as usize) else {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        };
        if *seen_name {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        }
        *seen_name = true;

        let Some(flags) = function_relations.flags.get_mut(entry.func() as usize) else {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        };
        if *flags & TEST_ENTRY_FUNCTION != 0 {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        }
        *flags |= TEST_ENTRY_FUNCTION;
    }
    let is_test_entry = |func: u16| {
        function_relations
            .flags
            .get(func as usize)
            .is_some_and(|flags| *flags & TEST_ENTRY_FUNCTION != 0)
    };
    for (index, function) in draft.functions().iter().enumerate() {
        let function = function.as_ref().ok_or(ImageBuildError::InvalidReference(
            ReferenceKind::VacantFunction,
        ))?;
        let has_assert = function
            .code
            .iter()
            .any(|instr| matches!(instr, Instr::Assert));
        if has_assert && !is_test_entry(index as u16) {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        }
    }
    for entry in entries {
        let function = draft.functions()[entry.func() as usize].as_ref().ok_or(
            ImageBuildError::InvalidReference(ReferenceKind::VacantFunction),
        )?;
        if function_relations.flags[entry.func() as usize] & EXPORTED_FUNCTION != 0 {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        }
        if !function.params.is_empty() {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        }
        if function.ret != ImageType::Unit {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        }
    }
    for function in draft.functions() {
        let function = function.as_ref().ok_or(ImageBuildError::InvalidReference(
            ReferenceKind::VacantFunction,
        ))?;
        for instr in &function.code {
            if let Instr::Call(target) = instr
                && is_test_entry(*target)
            {
                return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
            }
        }
    }
    for entry in entries {
        let function = draft.functions()[entry.func() as usize].as_ref().ok_or(
            ImageBuildError::InvalidReference(ReferenceKind::VacantFunction),
        )?;
        let has_direct_durable = function.code.iter().any(|instr| instr.site().is_some());
        if has_direct_durable {
            return Err(ImageBuildError::InvalidReference(ReferenceKind::TestTable));
        }
    }
    Ok(())
}

/// ENUMS — per definition its name, then per variant its name and its
/// payload type references, in row order. A reserved row still `Vacant` at the
/// fence is the coherence invariant, exactly as for records.
fn enums_references(draft: &ImageDraft) -> Result<(), ImageBuildError> {
    if draft.enums_fill().contains(&FillState::Unfilled) {
        return Err(ImageBuildError::InvalidReference(
            ReferenceKind::VacantEnumType,
        ));
    }
    for enum_def in draft.enums() {
        string_ref(draft, enum_def.name, ReferenceKind::EnumName)?;
        for variant in &enum_def.variants {
            string_ref(draft, variant.name, ReferenceKind::VariantName)?;
            for ty in &variant.payload {
                image_type_ref(draft, *ty)?;
            }
        }
    }
    Ok(())
}

/// COLLTYPES — per row its element (List) or key-then-value (Map) type
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

/// Decisive-saturation pins: on every over-ceiling corpus family the emitting sink
/// stops at exactly [`DECISIVE_TOTAL`] — it refuses every byte past the decisive one,
/// and every row loop polls fullness — so an over-ceiling draft costs the bytes the
/// ceiling admits and the `N`/`N + 1` boundary is byte-exact. The boundary corpora in
/// `tests/ceiling_boundary.rs` pin the same families end to end; these unit pins see
/// the sink itself.
#[cfg(test)]
mod decisive_saturation {
    use super::*;
    use crate::draft::{FunctionDef, RootOccurrenceDef, SpanEntry};
    use crate::product::{DeclarationMemberDef, DeclarationMemberShape};
    use crate::ty::Scalar;

    fn saturated(drive: impl FnOnce(&mut CappedImage)) -> usize {
        let mut sink = CappedImage::default();
        drive(&mut sink);
        sink.len()
    }

    #[test]
    fn an_over_ceiling_span_table_is_the_ceiling_refusal() {
        let mut owner = ImageDraft::new();
        let mut draft = owner.begin_transaction();
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
        assert_eq!(draft.encode().unwrap_err(), ImageBuildError::ImageTooLarge);
    }

    #[test]
    fn an_over_ceiling_function_table_is_the_ceiling_refusal() {
        let mut owner = ImageDraft::new();
        let mut draft = owner.begin_transaction();
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
        assert_eq!(draft.encode().unwrap_err(), ImageBuildError::ImageTooLarge);
    }

    #[test]
    fn an_over_ceiling_string_pool_saturates_at_the_decisive_byte() {
        let mut owner = ImageDraft::new();
        let mut draft = owner.begin_transaction();
        for index in 0..200 {
            draft
                .intern_string(&format!("{index:04}{}", "x".repeat(3_996)))
                .expect("a within-domain mint");
        }
        assert_eq!(
            saturated(|sink| {
                draft.encode_strings(&mut SectionSink::over(sink), 0..draft.strings().len());
            }),
            DECISIVE_TOTAL,
        );
    }

    #[test]
    fn an_over_ceiling_durable_expansion_saturates_at_the_decisive_byte() {
        let mut owner = ImageDraft::new();
        let mut draft = owner.begin_transaction();
        draft.set_application_identity(crate::durable_id::LedgerIdBytes::from_bytes([0x01; 16]));
        let mut value = draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        for _ in 0..31 {
            value = draft
                .value_struct(vec![value; 64])
                .expect("sixty-four leaves fit the checked surface");
        }
        let type_name = draft.intern_string("R").expect("a within-domain mint");
        let record = draft
            .add_record_type(crate::draft::RecordTypeDef {
                name: type_name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
        let plan = crate::draft::AdmittedGraphInputPlan::admit(1, 1, 4);
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
        let map = remap_of(&draft.string_permutation());
        assert_eq!(
            saturated(|sink| {
                draft
                    .write_durable_body(sink, &StringRemap::new(&map))
                    .expect("the capped write stops early");
            }),
            DECISIVE_TOTAL,
        );
    }
}
