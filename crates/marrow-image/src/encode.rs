//! The canonical container encoder.
//!
//! Turns a validated [`ImageDraft`] into the sectioned, length-prefixed,
//! big-endian image bytes with a computed digest. The encoder sorts the string and
//! constant pools into canonical order, rewrites every reference through the sort
//! maps, and lays out each function's bytecode so jump targets — held as
//! instruction indices while drafting — become container byte offsets.
//!
//! # Row law and token law
//!
//! Each canonicalized pool — strings, constants, exports, test entries — is its one
//! retained base row set plus one permutation computed by the pool's one comparator;
//! emission iterates the permutation-mapped base rows, so no second sorted copy of a
//! pool exists to disagree with the rows it came from. The nine non-DURABLE section
//! writers receive the sealed [`crate::remap::SectionSink`] their driver hands them
//! (the DURABLE writer keeps its pinned [`ImageByteSink`] bound), and a writer
//! resolves a string or constant reference only as an opaque [`crate::remap`] token
//! whose sole operation appends two bytes — so a section's byte length cannot depend
//! on which permutation or sink drives the writer, which is what lets one writer
//! serve counting and building alike.
//!
//! # Why the row-count conversions cannot truncate
//!
//! Each table is length-prefixed, so the encoder narrows a `usize` row count (and each
//! owned wide logical ordinal) to the `u16` or `u8` the wire spells it in. Every owned
//! narrowing below goes through the measure core's checked policy-clean path
//! ([`crate::measure::wire_ordinal`]/[`crate::measure::wire_len`]), which rests on the
//! same two-part derivation, stated once here rather than at each site:
//!
//! 1. The measure core's coherence and policy walks (`crate::measure`) run before any
//!    section is built and refuse a draft whose row count exceeds the
//!    bound for that table, so the value converted is at most that bound.
//! 2. The `const _` encoded-width block in [`bounds`] asserts at compile time that
//!    every one of those bounds fits the width its count is spelled in, so widening a
//!    bound past its encoded width breaks the build rather than truncating a count in
//!    an emitted image.
//!
//! The counts that are *not* covered by a `bounds` maximum — a function's span table, a
//! sparse-write key path, and a section body's own byte length — carry their own
//! derivation at their site.

use crate::bounds;
use crate::digest::ImageId;
use crate::draft::{
    CollectionTypeDef, ConstValue, ImageBuildError, ImageDraft, KeyColumn, ReferenceKind,
};
use crate::durable_id::{DurableGraphTooLarge, DurableIndexComponent, DurableIndexShape};
use crate::instr::Instr;
use crate::measure::{CoherentDraft, wire_len, wire_ordinal};
use crate::product::{DeclarationMemberShape, DeclarationNode, ProductDeclarationGraph};
use crate::remap::{ConstRemap, SectionSink, StringRemap};
use crate::ty::ImageType;
use crate::value_dag::{
    CanonicalValueShapeDag, ImageByteSink, ValueShapeWireForm, expand, push_u16,
};

/// Container magic, version, and envelope widths, shared with the measure core.
pub(crate) const MAGIC: &[u8; 4] = b"MWI\0";
/// The image generation this toolchain emits and admits.
pub const IMAGE_FORMAT_VERSION: u8 = 0x01;
pub(crate) const SECTION_COUNT: u8 = 10;
/// One SPANS row: `u32(offset) ‖ u32(line) ‖ u32(column)` — the row width
/// `encode_spans` spells in three `u32` pushes and the measure core's span counting
/// consumes arithmetically; the counted==emitted KATs pin the two against each other.
pub const SPAN_ROW_BYTES: usize = 12;

/// The encoded image plus its digest.
#[derive(Debug, Clone)]
pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub image_id: ImageId,
}

impl ImageDraft {
    /// Encode the draft into canonical container bytes, or fail with a producer-side
    /// [`ImageBuildError`] when a reference is incoherent, a bound is exceeded, or
    /// the measured image cannot fit.
    ///
    /// The thin driver over the measure core's three steps: coherence, the policy
    /// walk, and the one capped emission pass (`crate::measure`).
    pub fn encode(&self) -> Result<EncodedImage, ImageBuildError> {
        let coherent = CoherentDraft::of(self)?;
        coherent.check_policy()?;
        coherent.emit_image()
    }

    /// The canonical string permutation (row law): the base-row indices in emitted
    /// order, computed by the pool's one byte comparator over the retained base rows.
    pub(crate) fn string_permutation(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.strings().len()).collect();
        order.sort_by(|&a, &b| {
            self.strings()[a]
                .as_bytes()
                .cmp(self.strings()[b].as_bytes())
        });
        order
    }

    /// The canonical constant permutation (row law), ordered by each base row's
    /// `(tag, wire-byte)` sort key with text payloads resolved to final string indices.
    pub(crate) fn const_permutation(&self, str_map: &[u16]) -> Vec<usize> {
        // A key depends only on its row and the already-final string permutation.
        // Retaining one per row avoids rebuilding two owned payloads at every sort
        // comparison; the permutation still sorts the same keys with the same stable
        // comparator.
        let keys: Vec<(u8, Vec<u8>)> = self
            .consts()
            .iter()
            .map(|value| value.sort_key(str_map))
            .collect();
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
        order
    }

    /// The canonical export permutation (row law): the base-row indices ascending by
    /// the 32 [`crate::export_id::ExportId`] bytes.
    pub(crate) fn export_permutation(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.export_count()).collect();
        order.sort_by(|&a, &b| {
            self.export_rows()[a]
                .id()
                .bytes()
                .cmp(self.export_rows()[b].id().bytes())
        });
        order
    }

    /// Encode the STRINGS table (section 0x01): a count, then per row its
    /// length-prefixed text, iterating the base rows in the order `order` states.
    pub(crate) fn encode_strings<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        order: impl Iterator<Item = usize>,
    ) {
        push_u16(sink, wire_len(self.strings().len()));
        for row in order {
            if sink.is_full() {
                return;
            }
            let text = &self.strings()[row];
            push_u16(sink, wire_len(text.len()));
            sink.extend_bytes(text.as_bytes());
        }
    }

    /// Encode the CONSTS table (section 0x04): a count, then per row its tag and
    /// payload, iterating the base rows in the order `order` states. A text payload is
    /// its remapped string reference, written as an opaque token.
    pub(crate) fn encode_consts<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        strings: &StringRemap<'_>,
        order: impl Iterator<Item = usize>,
    ) {
        push_u16(sink, wire_len(self.consts().len()));
        for row in order {
            if sink.is_full() {
                return;
            }
            match self.consts()[row] {
                ConstValue::Int(v) => {
                    sink.push(0x01);
                    sink.extend_bytes(&v.to_be_bytes());
                }
                ConstValue::Bool(v) => {
                    sink.push(0x02);
                    sink.push(u8::from(v));
                }
                ConstValue::Text(str_id) => {
                    sink.push(0x03);
                    strings.token(str_id).emit(sink);
                }
                ConstValue::Date(v) => {
                    sink.push(0x04);
                    sink.extend_bytes(&v.to_be_bytes());
                }
                ConstValue::Instant(v) => {
                    sink.push(0x05);
                    sink.extend_bytes(&v.to_be_bytes());
                }
                ConstValue::Duration(v) => {
                    sink.push(0x06);
                    sink.extend_bytes(&v.to_be_bytes());
                }
            }
        }
    }

    pub(crate) fn encode_types<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        strings: &StringRemap<'_>,
    ) {
        push_u16(sink, wire_len(self.types().len()));
        for record in self.types() {
            if sink.is_full() {
                return;
            }
            strings.token(record.name).emit(sink);
            push_u16(sink, wire_len(record.fields.len()));
            for field in &record.fields {
                strings.token(field.name).emit(sink);
                field.ty.encode(sink);
                sink.push(u8::from(field.required));
            }
        }
    }

    /// Encode the ENUMS table (section 0x09): a count, then per enum its name
    /// string index, a variant count, and per variant a name string index, a
    /// `category` flag byte, a payload count, and one bare-`ImageType` reference per
    /// payload leaf in declaration order (a scalar tag, or a tag plus a big-endian
    /// `u16` index for a record or enum leaf).
    pub(crate) fn encode_enums<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        strings: &StringRemap<'_>,
    ) {
        push_u16(sink, wire_len(self.enums().len()));
        for enum_def in self.enums() {
            if sink.is_full() {
                return;
            }
            strings.token(enum_def.name).emit(sink);
            push_u16(sink, wire_len(enum_def.variants.len()));
            for variant in &enum_def.variants {
                strings.token(variant.name).emit(sink);
                sink.push(u8::from(variant.category));
                sink.push(variant.payload.len() as u8);
                for ty in &variant.payload {
                    ty.encode(sink);
                }
            }
        }
    }

    /// Encode the COLLTYPES table (section 0x0A): a count, then per collection type
    /// a one-byte kind tag (`0x00` List, `0x01` Map) followed by its bare-`ImageType`
    /// element reference (List) or key then value references (Map). Element/key/value
    /// references may themselves be `Collection` tags into an earlier COLLTYPES row.
    pub(crate) fn encode_collections<S: ImageByteSink>(&self, sink: &mut SectionSink<'_, S>) {
        push_u16(sink, wire_len(self.collections().len()));
        for coll in self.collections() {
            if sink.is_full() {
                return;
            }
            match coll {
                CollectionTypeDef::List { elem } => {
                    sink.push(0x00);
                    elem.encode(sink);
                }
                CollectionTypeDef::Map { key, value } => {
                    sink.push(0x01);
                    key.encode(sink);
                    value.encode(sink);
                }
            }
        }
    }

    /// Write the DURABLE section body, minus its closing contract identity, into `sink`.
    ///
    /// One owner writes the section for both of its readers: the measure core's capped
    /// counting run and the emission buffer. A body admitted by the count is therefore
    /// the body that is built, because it is the same walk over the same rows.
    pub(crate) fn write_durable_body(
        &self,
        sink: &mut impl ImageByteSink,
        strings: &StringRemap<'_>,
    ) -> Result<(), ImageBuildError> {
        push_u16(sink, wire_len(self.root_occurrences().len()));
        // The application's ledger id anchors a non-empty durable graph; a
        // storeless image carries none.
        if !self.root_occurrences().is_empty() {
            let application =
                self.application_identity()
                    .ok_or(ImageBuildError::InvalidReference(
                        ReferenceKind::ApplicationIdentity,
                    ))?;
            sink.extend_bytes(application.bytes());
        }
        // v0 carries the whole member graph per root, so each occurrence projects the
        // one retained declaration it references rather than carrying its own copy.
        for occurrence in self.root_occurrences() {
            let declaration = self.declaration_of(occurrence);
            strings.token(occurrence.name()).emit_durable(sink);
            // The key tuple: a count, then each column's scalar type and ledger id.
            // Zero columns is a singleton root; more than one is a composite key.
            encode_key_tuple(sink, occurrence.keys());
            push_u16(sink, wire_ordinal(declaration.root_entry_record().index()));
            // The root's remaining ledger identity block: placement and product,
            // then the resource's durable member tree (top-level fields interleaved
            // with static `group` namespaces and keyed `branch` placements).
            sink.extend_bytes(occurrence.placement().ledger_id().bytes());
            sink.extend_bytes(declaration.identity().ledger_id().bytes());
            let graph = declaration.graph();
            encode_declaration_members(sink, graph, graph.members(), strings, self.value_shapes())?;
            // A body already past the ceiling is decided; the remaining roots would only
            // add to a count that is already refused.
            if sink.is_full() {
                return Ok(());
            }
            encode_durable_indexes(sink, occurrence.indexes());
        }
        // The retained row count, not the demand: the plan refuses to mint past its
        // capacity, so rows never exceed the demand the policy walk measured against
        // `MAX_SITES`, and the count fits the `u16` the table is prefixed with.
        push_u16(sink, wire_len(self.site_row_count()));
        self.write_site_rows(sink)
    }
}

impl CoherentDraft<'_> {
    pub(crate) fn encode_functions<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        strings: &StringRemap<'_>,
        consts: &ConstRemap<'_>,
        sites: &crate::measure::SiteWireProjection<'_>,
    ) -> Result<Vec<CodeLayout>, ImageBuildError> {
        push_u16(sink, self.functions().len() as u16);
        let mut per_fn = Vec::with_capacity(self.functions().len());
        for function in self.functions() {
            if sink.is_full() {
                return Ok(per_fn);
            }
            let layout = code_layout(&function.code)?;
            if layout.total_len as usize > bounds::MAX_CODE_BYTES {
                return Err(ImageBuildError::CodeTooLong);
            }
            strings.token(function.name).emit(sink);
            strings.token(function.source).emit(sink);
            sink.push(function.params.len() as u8);
            for param in &function.params {
                param.encode(sink);
            }
            function.ret.encode(sink);
            push_u16(sink, function.local_count);
            push_u32(sink, layout.total_len);
            encode_code(sink, &function.code, &layout, consts, sites)?;
            per_fn.push(layout);
        }
        Ok(per_fn)
    }
}

impl ImageDraft {
    /// Encode the EXPORTS table: a count, then each `32-byte ExportId ‖ u16 func`
    /// entry, iterating the base rows in the order `order` states — strictly ascending
    /// id order under the canonical permutation. The id is the only export key carried;
    /// the source name is not, so the VM can only dispatch on a verified id.
    pub(crate) fn encode_exports<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        order: impl Iterator<Item = usize>,
    ) {
        push_u16(sink, self.export_count() as u16);
        for row in order {
            if sink.is_full() {
                return;
            }
            let export = &self.export_rows()[row];
            sink.extend_bytes(export.id().bytes());
            push_u16(sink, export.func());
        }
    }

    /// Encode the TEST-ENTRY table (section 0x08): a count, then each
    /// `u16 name-string-index ‖ u16 function-index` entry, iterating the base rows in
    /// the order `order` states — strictly ascending remapped-name order under the
    /// canonical permutation. Names are unique across the project, so the sort is
    /// total and the verifier rechecks the strict ordering.
    pub(crate) fn encode_test_entries<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        strings: &StringRemap<'_>,
        order: impl Iterator<Item = usize>,
    ) {
        push_u16(sink, self.test_entry_count() as u16);
        for row in order {
            if sink.is_full() {
                return;
            }
            let entry = &self.test_entry_rows()[row];
            strings.token(entry.name()).emit(sink);
            push_u16(sink, entry.func());
        }
    }
}

impl CoherentDraft<'_> {
    /// Encode the SPANS section: per function in table order, a `u16` span count then
    /// that many `u32(offset) ‖ u32(line) ‖ u32(column)` rows.
    ///
    /// A span table is the one encoder row set no bound guards — spans are debug
    /// positions the producer supplies, not a declared program shape, and nothing ties
    /// their number to the instruction count. The whole-image ceiling is the binder
    /// instead: a count the `u16` prefix cannot spell needs at least
    /// `65_536 * SPAN_ROW_BYTES` bytes of rows behind it, which exceeds
    /// [`bounds::MAX_IMAGE_BYTES`] on its own, so [`ImageDraft::encode`] refuses such a
    /// draft with [`ImageBuildError::ImageTooLarge`] and no image carrying a truncated
    /// span count can be accepted. The assertion below fails the build if a later
    /// ceiling widening breaks that derivation, at which point this count needs a
    /// bound and a typed refusal of its own rather than a comment.
    pub(crate) fn encode_spans<S: ImageByteSink>(
        &self,
        sink: &mut SectionSink<'_, S>,
        per_fn: &[CodeLayout],
    ) {
        const _: () = assert!(
            bounds::MAX_IMAGE_BYTES / SPAN_ROW_BYTES < u16::MAX as usize,
            "the image ceiling must refuse a span table before its count outgrows the u16 prefix",
        );

        for (function, layout) in self.functions().zip(per_fn) {
            if sink.is_full() {
                return;
            }
            push_u16(sink, function.spans.len() as u16);
            for span in &function.spans {
                let offset = layout.offsets[span.instr_index as usize];
                push_u32(sink, offset);
                push_u32(sink, span.line);
                push_u32(sink, span.column);
            }
        }
    }
}

/// The byte offset of each instruction plus the total code length.
pub(crate) struct CodeLayout {
    offsets: Vec<u32>,
    total_len: u32,
}

/// One function's laid-out code length: the sum of the per-instruction widths the
/// one width owner ([`Instr::encoded_len`]) states — the same widths `code_layout`
/// accumulates into offsets. The policy walk reads the length without materializing
/// the offsets.
///
/// Each addition is checked before the result can be observed. Instruction widths are
/// variable — a sparse durable write carries one slot operand per key column — so there is
/// no fixed per-instruction maximum that proves an unchecked sum safe for the public draft
/// surface.
pub(crate) fn laid_out_code_len(code: &[Instr]) -> Result<u64, ImageBuildError> {
    code.iter().try_fold(0, checked_code_offset)
}

/// Add one instruction's exact width to a laid-out offset without wrapping either the
/// platform-width conversion or the accumulator.
fn checked_code_offset(offset: u64, instr: &Instr) -> Result<u64, ImageBuildError> {
    let width = u64::try_from(instr.encoded_len()).map_err(|_| ImageBuildError::CodeTooLong)?;
    offset
        .checked_add(width)
        .ok_or(ImageBuildError::CodeTooLong)
}

/// Lay out one function's code: each instruction's byte offset, and the total width.
///
/// Offsets are `u32` because the container spells them so, and the running total is
/// accumulated in `u64` and narrowed once per consuming boundary: a total no `u32`
/// can carry is [`ImageBuildError::CodeTooLong`], the same verdict the
/// [`bounds::MAX_CODE_BYTES`] comparison below it reaches, rather than a debug
/// overflow or a release wrap.
fn code_layout(code: &[Instr]) -> Result<CodeLayout, ImageBuildError> {
    let mut offsets = Vec::with_capacity(code.len());
    let mut offset: u64 = 0;
    for instr in code {
        offsets.push(u32::try_from(offset).map_err(|_| ImageBuildError::CodeTooLong)?);
        offset = checked_code_offset(offset, instr)?;
    }
    let total_len = u32::try_from(offset).map_err(|_| ImageBuildError::CodeTooLong)?;
    Ok(CodeLayout { offsets, total_len })
}

fn encode_code<S: ImageByteSink>(
    sink: &mut SectionSink<'_, S>,
    code: &[Instr],
    layout: &CodeLayout,
    consts: &ConstRemap<'_>,
    sites: &crate::measure::SiteWireProjection<'_>,
) -> Result<(), ImageBuildError> {
    for instr in code {
        sink.push(instr.opcode());
        match instr {
            Instr::ConstLoad(raw) | Instr::Unreachable(raw) | Instr::Todo(raw) => {
                consts.token(*raw).emit(sink)
            }
            Instr::LocalGet(l) | Instr::LocalSet(l) => push_u16(sink, *l),
            Instr::Call(f) => push_u16(sink, *f),
            Instr::RecordNew(t) => push_u16(sink, wire_ordinal(t.index())),
            Instr::ListNew(c) | Instr::MapNew(c) | Instr::TextSplit(c) | Instr::TextLines(c) => {
                push_u16(sink, wire_ordinal(c.index()))
            }
            Instr::FieldGet(f) | Instr::FieldSet(f) | Instr::FieldUnset(f) => push_u16(sink, *f),
            Instr::DurExists(s)
            | Instr::DurFamilyExists(s)
            | Instr::DurReadField(s)
            | Instr::DurReadEntry(s)
            | Instr::DurCreateEntry(s)
            | Instr::DurReplaceEntry(s)
            | Instr::DurEraseField(s)
            | Instr::DurEraseEntry(s)
            | Instr::DurReadGroup(s)
            | Instr::DurEraseGroup(s) => push_u16(sink, sites.ordinal(s)?),
            Instr::Jump(target)
            | Instr::JumpIfFalse(target)
            | Instr::BranchPresent(target)
            | Instr::IntAddChecked(target)
            | Instr::IntSubChecked(target)
            | Instr::IntMulChecked(target)
            | Instr::IntNegChecked(target)
            | Instr::IntDivChecked(target)
            | Instr::IntRemChecked(target) => {
                let byte_offset = *layout
                    .offsets
                    .get(*target as usize)
                    .ok_or(ImageBuildError::InvalidReference(ReferenceKind::JumpTarget))?;
                push_u32(sink, byte_offset);
            }
            Instr::VacantLoad(ty) => ty.encode(sink),
            Instr::RangeGuard { lo, hi } => {
                sink.extend_bytes(&lo.to_be_bytes());
                sink.extend_bytes(&hi.to_be_bytes());
            }
            Instr::EnumConstruct { enum_idx, variant } => {
                push_u16(sink, wire_ordinal(enum_idx.index()));
                push_u16(sink, *variant);
            }
            Instr::EnumPayloadGet { variant, field } => {
                push_u16(sink, *variant);
                push_u16(sink, *field);
            }
            Instr::DurSetField { site, key_slots }
            | Instr::DurReadFieldPresent { site, key_slots }
            | Instr::DurReadGroupPresent { site, key_slots }
            | Instr::DurReplaceGroup { site, key_slots } => {
                push_u16(sink, sites.ordinal(site)?);
                // The slot count fits its `u16` prefix: each slot is two more bytes of
                // this instruction's operand, and `encode_functions` has already
                // refused a function whose laid-out code exceeds
                // `bounds::MAX_CODE_BYTES` — half that ceiling in slots is well inside
                // the prefix.
                push_u16(sink, key_slots.len() as u16);
                for slot in key_slots {
                    push_u16(sink, *slot);
                }
            }
            Instr::DurIterateBounded {
                site,
                limit,
                from,
                list_ty,
            } => {
                push_u16(sink, sites.ordinal(site)?);
                push_u32(sink, *limit);
                sink.push(u8::from(*from));
                push_u16(sink, wire_ordinal(list_ty.index()));
            }
            Instr::MakeIdentity { root, cols } => {
                push_u16(sink, wire_ordinal(root.index()));
                push_u16(sink, *cols);
            }
            Instr::IdentityKeyPath(cols) => push_u16(sink, *cols),
            Instr::DurIndexScan {
                site,
                limit,
                from,
                list_ty,
            } => {
                push_u16(sink, sites.ordinal(site)?);
                push_u32(sink, *limit);
                sink.push(u8::from(*from));
                push_u16(sink, wire_ordinal(list_ty.index()));
            }
            Instr::DurIndexLookup(site) | Instr::DurIndexExists(site) => {
                push_u16(sink, sites.ordinal(site)?)
            }
            _ => {}
        }
    }
    Ok(())
}

/// `map[base] = final index` — the inverse of one canonical permutation (row law).
///
/// The permutation is the base-row indices sorted into emitted order, so inverting it
/// is the whole remap; the narrowing rests on the same bound derivation the module
/// doc states for every row count.
pub(crate) fn remap_of(permutation: &[usize]) -> Vec<u16> {
    let mut map = vec![0u16; permutation.len()];
    for (final_index, &base) in permutation.iter().enumerate() {
        map[base] = wire_len(final_index);
    }
    map
}

/// Write the fixed image envelope head — magic, version, and the 32-byte digest slot —
/// the one envelope-head codec: emission drives it with the computed [`ImageId`] and
/// measurement with the zero digest, since the slot's width, not its value, is the
/// fact a count consumes.
pub(crate) fn write_image_header(sink: &mut impl ImageByteSink, digest: &[u8; 32]) {
    sink.extend_bytes(MAGIC);
    sink.push(IMAGE_FORMAT_VERSION);
    sink.extend_bytes(digest);
}

/// Write one section frame: `u8(id) ‖ u32(body_len)` — the one frame codec, driven by
/// emission ahead of each assembled body and by measurement ahead of each counted body
/// (with the zero length, for the same width-not-value reason as the header).
pub(crate) fn push_frame(sink: &mut impl ImageByteSink, id: u8, body_len: u32) {
    sink.push(id);
    push_u32(sink, body_len);
}

/// Encode a placement key tuple into the DURABLE section: `u16(count) ‖
/// [scalar_tag ‖ id(16)]*`. Shared by roots and branches; column order is
/// load-bearing.
fn encode_key_tuple(body: &mut impl ImageByteSink, keys: &[KeyColumn]) {
    push_u16(body, wire_len(keys.len()));
    for key in keys {
        ImageType::scalar(key.scalar).encode(body);
        body.extend_bytes(key.id.bytes());
    }
}

/// Encode one run of a Product declaration's member rows into the DURABLE section:
/// `u16(count) ‖ member*`. A field is tag `0x00`, its ledger id, a required flag, and its
/// value shape; a group is tag `0x01`, its ledger id, and its own members; a branch is tag
/// `0x02`, its placement id, its key tuple, and its own members. Descends into each
/// group's and branch's own run in declaration order. A field's value shape is expanded
/// straight into `body` from the draft's one arena, by the same
/// [`expand`] owner the contract-identity preimage uses, so a durable field and its
/// identity contribution can never spell the value two ways.
///
/// The nesting bound is rechecked before any encoding, so the descent is bounded by
/// [`bounds::MAX_DURABLE_DEPTH`].
fn encode_declaration_members(
    body: &mut impl ImageByteSink,
    graph: &ProductDeclarationGraph,
    members: &[DeclarationNode],
    strings: &StringRemap<'_>,
    values: &CanonicalValueShapeDag,
) -> Result<(), ImageBuildError> {
    push_u16(body, wire_len(members.len()));
    for member in members {
        if body.is_full() {
            return Ok(());
        }
        match member.shape() {
            DeclarationMemberShape::Field {
                id,
                required,
                value,
            } => {
                body.push(0x00);
                body.extend_bytes(id.bytes());
                body.push(u8::from(*required));
                // Coherence's arena fan-out recheck bounds every arity far below the
                // width this section's `u16` spells, so the arm is unreachable; a
                // value that wide is an image no ceiling admits.
                expand(values, *value, ValueShapeWireForm::DurableSection, body)
                    .map_err(|DurableGraphTooLarge| ImageBuildError::ImageTooLarge)?;
            }
            DeclarationMemberShape::Group { id } => {
                body.push(0x01);
                body.extend_bytes(id.bytes());
                encode_declaration_members(body, graph, graph.members_of(member), strings, values)?;
            }
            DeclarationMemberShape::Branch {
                placement,
                name,
                record,
                keys,
            } => {
                body.push(0x02);
                body.extend_bytes(placement.bytes());
                strings.token(*name).emit_durable(body);
                push_u16(body, wire_ordinal(record.index()));
                encode_key_tuple(body, keys);
                encode_declaration_members(body, graph, graph.members_of(member), strings, values)?;
            }
        }
    }
    Ok(())
}

/// Encode a root's managed indexes into the DURABLE section: `u16(count) ‖ index*`.
/// Each index is its raw 16-byte `Index` ledger id, a `unique` flag byte, a
/// `u16(component_count)`, and per component a one-byte leaf kind (`0x02` field,
/// `0x04` key — the frozen IDREF kind bytes) and the leaf's raw 16-byte ledger id.
/// An index carries no value shape: it is derived from the leaves it projects.
fn encode_durable_indexes(body: &mut impl ImageByteSink, indexes: &[DurableIndexShape]) {
    push_u16(body, wire_len(indexes.len()));
    for index in indexes {
        body.extend_bytes(index.id.bytes());
        body.push(u8::from(index.unique));
        push_u16(body, wire_len(index.components.len()));
        for component in &index.components {
            let (kind, id) = match component {
                DurableIndexComponent::Field(id) => (0x02u8, id),
                DurableIndexComponent::Key(id) => (0x04u8, id),
            };
            body.push(kind);
            body.extend_bytes(id.bytes());
        }
    }
}

fn push_u32(out: &mut impl ImageByteSink, value: u32) {
    out.extend_bytes(&value.to_be_bytes());
}

/// Encoder fixtures built directly through the draft API — this crate cannot compile
/// the frozen corpus programs, whose byte digests stay pinned in `marrow-compile`.
/// They cover every section family those programs cover: sorted strings and constants
/// of every tag, records, enums, collections, a keyed and an indexed durable root with
/// group/branch/struct-shape members and an operation site, functions with jumps and
/// remapped operands, spans, exports, and test entries.
#[cfg(test)]
mod encoder_fixtures {
    use super::checked_code_offset;
    use crate::draft::{
        AdmittedGraphInputPlan, CollectionTypeDef, FieldDef, FunctionDef, ImageDraft,
        RecordTypeDef, RootOccurrenceDef, SpanEntry, VariantDef,
    };
    use crate::durable_id::{DurableIndexComponent, DurableIndexShape, LedgerIdBytes};
    use crate::instr::Instr;
    use crate::product::{DeclarationMemberDef, DeclarationMemberShape};
    use crate::semantic::SemanticTarget;
    use crate::ty::{ImageType, Scalar};

    #[test]
    fn code_offset_overflow_is_the_typed_code_length_refusal() {
        assert_eq!(
            checked_code_offset(u64::MAX, &Instr::Return),
            Err(crate::draft::ImageBuildError::CodeTooLong),
        );
        assert_eq!(
            Instr::RangeGuard {
                lo: i64::MIN,
                hi: i64::MAX,
            }
            .encoded_len(),
            17,
            "the old sixteen-byte instruction proof is false",
        );
    }

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    /// A storeless draft whose insertion orders disagree with every canonical order.
    fn storeless() -> ImageDraft {
        let mut draft = ImageDraft::new();
        let source = draft
            .intern_string("src/main.mw")
            .expect("a within-domain mint");
        let zeta = draft.intern_string("zeta").expect("a within-domain mint");
        let alpha = draft.intern_string("alpha").expect("a within-domain mint");
        let record_name = draft.intern_string("record").expect("a within-domain mint");
        let field_name = draft.intern_string("field").expect("a within-domain mint");
        let enum_name = draft.intern_string("choice").expect("a within-domain mint");
        let variant_one = draft.intern_string("one").expect("a within-domain mint");
        let variant_two = draft.intern_string("two").expect("a within-domain mint");

        let text = draft.intern_text("zeta").expect("a within-domain mint");
        draft.intern_int(-1).expect("a within-domain mint");
        draft.intern_int(0).expect("a within-domain mint");
        draft.intern_bool(true).expect("a within-domain mint");
        draft.intern_date(20_000).expect("a within-domain mint");
        draft.intern_instant(7).expect("a within-domain mint");
        draft.intern_duration(-7).expect("a within-domain mint");

        let record = draft
            .reserve_record_type(record_name)
            .expect("a within-domain mint");
        let savepoint = draft.savepoint();
        let mut fills = draft
            .begin_transaction(savepoint)
            .expect("a fresh savepoint admits");
        fills
            .set_record_fields(
                record,
                vec![
                    FieldDef {
                        name: field_name,
                        ty: ImageType::scalar(Scalar::Int),
                        required: true,
                    },
                    FieldDef {
                        name: alpha,
                        ty: ImageType::scalar(Scalar::Text),
                        required: false,
                    },
                ],
            )
            .expect("the reserved row fills once");
        fills.commit();
        let choice = draft
            .reserve_enum_type(enum_name)
            .expect("a within-domain mint");
        let savepoint = draft.savepoint();
        let mut fills = draft
            .begin_transaction(savepoint)
            .expect("a fresh savepoint admits");
        fills
            .set_enum_variants(
                choice,
                vec![
                    VariantDef {
                        name: variant_one,
                        category: false,
                        payload: vec![ImageType::scalar(Scalar::Int)],
                    },
                    VariantDef {
                        name: variant_two,
                        category: false,
                        payload: Vec::new(),
                    },
                ],
            )
            .expect("the reserved row fills once");
        fills.commit();
        let list = draft
            .add_collection_type(CollectionTypeDef::List {
                elem: ImageType::scalar(Scalar::Int),
            })
            .expect("a within-domain mint");
        draft
            .add_collection_type(CollectionTypeDef::Map {
                key: ImageType::scalar(Scalar::Text),
                value: ImageType::scalar(Scalar::Bool),
            })
            .expect("a within-domain mint");

        let mut funcs = Vec::new();
        for name in [zeta, alpha] {
            let func = draft
                .add_function(FunctionDef {
                    name,
                    source,
                    params: vec![ImageType::scalar(Scalar::Int)],
                    ret: ImageType::Unit,
                    local_count: 2,
                    code: vec![
                        Instr::ConstLoad(text),
                        Instr::LocalSet(1),
                        Instr::LocalGet(0),
                        Instr::JumpIfFalse(6),
                        Instr::RecordNew(record),
                        Instr::EnumConstruct {
                            enum_idx: choice,
                            variant: 0,
                        },
                        Instr::ListNew(list),
                        Instr::VacantLoad(ImageType::scalar(Scalar::Text)),
                        Instr::Jump(9),
                        Instr::Return,
                    ],
                    spans: vec![
                        SpanEntry {
                            instr_index: 0,
                            line: 1,
                            column: 1,
                        },
                        SpanEntry {
                            instr_index: 9,
                            line: 2,
                            column: 5,
                        },
                    ],
                })
                .expect("no site operand needs validating");
            funcs.push(func);
        }
        // Exports inserted in descending id order; test entries in descending name
        // order — the canonical permutations must reorder both.
        let mut exports: Vec<crate::export_id::ExportId> = ["a", "b"]
            .into_iter()
            .map(|item| crate::export_id::ExportId::of_local("m", item))
            .collect();
        exports.sort_by(|left, right| left.bytes().cmp(right.bytes()));
        for (export, func) in exports.into_iter().rev().zip(funcs.iter()) {
            draft.add_export(export, *func);
        }
        // Test entries name their own unexported zero-parameter unit functions — the
        // test relations the coherence walk enforces — under names whose remapped
        // order disagrees with insertion order.
        for name in [zeta, alpha] {
            let test_fn = draft
                .add_function(FunctionDef {
                    name,
                    source,
                    params: Vec::new(),
                    ret: ImageType::Unit,
                    local_count: 0,
                    code: vec![Instr::Return],
                    spans: Vec::new(),
                })
                .expect("no site operand needs validating");
            draft.add_test_entry(name, test_fn);
        }
        draft
    }

    /// A relation-heavy but coherent draft. Each relation has `rows` distinct input
    /// rows, and each exported body contributes one call-membership question.
    fn algorithmic_work_draft(rows: usize) -> ImageDraft {
        let mut draft = ImageDraft::new();
        let source = draft
            .intern_string("src/algorithmic.mw")
            .expect("a within-domain mint");
        let helper_name = draft.intern_string("helper").expect("a within-domain mint");
        let helper = draft
            .add_function(FunctionDef {
                name: helper_name,
                source,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                code: vec![Instr::Return],
                spans: Vec::new(),
            })
            .expect("the helper has no site operand");

        for row in 0..rows {
            draft
                .intern_int(row as i64)
                .expect("the constant population is within domain");
        }
        let true_const = draft
            .intern_bool(true)
            .expect("the assertion constant is within domain");

        for row in 0..rows {
            let item = format!("export{row:03}");
            let name = draft
                .intern_string(&item)
                .expect("the export name is within domain");
            let function = draft
                .add_function(FunctionDef {
                    name,
                    source,
                    params: Vec::new(),
                    ret: ImageType::Unit,
                    local_count: 0,
                    code: vec![Instr::Call(helper.index()), Instr::Return],
                    spans: Vec::new(),
                })
                .expect("the exported function has no site operand");
            draft.add_export(
                crate::export_id::ExportId::of_local("algorithmic", &item),
                function,
            );
        }

        for row in 0..rows {
            let item = format!("test{row:03}");
            let name = draft
                .intern_string(&item)
                .expect("the test name is within domain");
            let function = draft
                .add_function(FunctionDef {
                    name,
                    source,
                    params: Vec::new(),
                    ret: ImageType::Unit,
                    local_count: 0,
                    code: vec![Instr::ConstLoad(true_const), Instr::Assert, Instr::Return],
                    spans: Vec::new(),
                })
                .expect("the test function has no site operand");
            draft.add_test_entry(name, function);
        }
        draft
    }

    /// A relation-heavy draft encodes to the same bytes every time: the coherence
    /// walk's keyed sets are never iterated, so their randomized layout cannot reach
    /// the output.
    #[test]
    fn a_relation_heavy_draft_encodes_deterministically() {
        for rows in [64usize, 128] {
            let draft = algorithmic_work_draft(rows);
            let first = draft.encode().expect("the coherent draft encodes");
            let second = draft.encode().expect("the coherent draft encodes");
            assert_eq!(first.bytes, second.bytes, "{rows} rows encode identically");
            assert_eq!(first.image_id, second.image_id);
        }
    }

    /// A durable draft: a keyed root and an indexed root over one Product whose members
    /// nest a struct-shaped field, a group, and a keyed branch, plus one operation site.
    fn durable() -> ImageDraft {
        let mut draft = ImageDraft::new();
        draft.set_application_identity(id(0x01));
        let record_name = draft.intern_string("entry").expect("a within-domain mint");
        let keyed = draft.intern_string("keyed").expect("a within-domain mint");
        let indexed = draft
            .intern_string("indexed")
            .expect("a within-domain mint");
        let branch_name = draft.intern_string("branch").expect("a within-domain mint");

        let entry = draft
            .add_record_type(RecordTypeDef {
                name: record_name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
        let leaf = draft
            .value_shapes_mut()
            .scalar(Scalar::Int)
            .expect("the test arena mints");
        let pair = draft
            .value_shapes_mut()
            .struct_shape(vec![leaf, leaf])
            .expect("the test arena mints");
        let sum = draft
            .value_shapes_mut()
            .enum_shape(
                id(0x60),
                vec![(id(0x61), vec![leaf]), (id(0x62), Vec::new())],
            )
            .expect("the test arena mints");

        let plan = AdmittedGraphInputPlan::admit(1, 2, 8);
        let product = id(0x10);
        draft
            .declare_product(
                &plan,
                product,
                entry,
                vec![
                    DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Field {
                            id: id(0x20),
                            required: true,
                            value: pair,
                        },
                    },
                    DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Group { id: id(0x21) },
                    },
                    DeclarationMemberDef {
                        parent: Some(1),
                        shape: DeclarationMemberShape::Field {
                            id: id(0x22),
                            required: false,
                            value: sum,
                        },
                    },
                    DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Branch {
                            placement: id(0x30),
                            name: branch_name,
                            record: entry,
                            keys: vec![crate::draft::KeyColumn {
                                scalar: Scalar::Int,
                                id: id(0x31),
                            }],
                        },
                    },
                    DeclarationMemberDef {
                        parent: Some(3),
                        shape: DeclarationMemberShape::Field {
                            id: id(0x32),
                            required: true,
                            value: leaf,
                        },
                    },
                ],
            )
            .expect("a well-formed declaration");
        let admitted = draft
            .add_root_occurrence(
                &plan,
                product,
                RootOccurrenceDef {
                    name: keyed,
                    keys: vec![crate::draft::KeyColumn {
                        scalar: Scalar::Int,
                        id: id(0x40),
                    }],
                    placement: id(0x41),
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");
        draft
            .add_root_occurrence(
                &plan,
                product,
                RootOccurrenceDef {
                    name: indexed,
                    keys: Vec::new(),
                    placement: id(0x51),
                    indexes: vec![DurableIndexShape {
                        id: id(0x52),
                        unique: true,
                        components: vec![DurableIndexComponent::Field(id(0x20))],
                    }]
                    .into(),
                },
            )
            .expect("the Product is declared");
        let handle = draft
            .bind_occurrence_site(
                admitted.occurrence(),
                admitted.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("a root admits a whole-payload site");
        draft
            .request_site(&handle)
            .expect("the plan has capacity for one site");
        draft
    }

    /// Both fixtures must be populated, so the encoder is exercised over every
    /// section rather than over empty row sets.
    #[test]
    fn the_fixtures_are_populated_and_encode() {
        let storeless = storeless();
        storeless.encode().expect("the storeless fixture encodes");
        assert!(!storeless.strings().is_empty());
        assert!(!storeless.consts().is_empty());
        assert!(!storeless.types().is_empty());
        assert!(!storeless.enums().is_empty());
        assert!(!storeless.collections().is_empty());
        assert!(!storeless.functions().is_empty());
        assert!(storeless.export_count() > 0);
        assert!(storeless.test_entry_count() > 0);

        let durable = durable();
        durable.encode().expect("the durable fixture encodes");
        assert_eq!(durable.root_occurrences().len(), 2);
        assert!(durable.site_row_count() > 0);
    }
}
