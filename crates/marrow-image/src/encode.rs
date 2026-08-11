//! The canonical container encoder (design §C).
//!
//! Turns a validated [`ImageDraft`] into the sectioned, length-prefixed,
//! big-endian image bytes with a computed digest. The encoder sorts the string and
//! constant pools into canonical order, rewrites every reference through the sort
//! maps, and lays out each function's bytecode so jump targets — held as
//! instruction indices while drafting — become container byte offsets.
//!
//! # Why the row-count conversions cannot truncate
//!
//! Each table is length-prefixed, so the encoder narrows a `usize` row count to the
//! `u16` or `u8` the wire spells it in. Every such conversion below rests on the same
//! two-part derivation, stated once here rather than at each site:
//!
//! 1. [`ImageDraft::check_bounds`] runs before any section is built (through
//!    [`LegacyFullDraftCoherencePreflight`]) and refuses a draft whose row count
//!    exceeds the §E bound for that table, so the value converted is at most that
//!    bound.
//! 2. The `const _` encoded-width block in [`bounds`] asserts at compile time that
//!    every one of those bounds fits the width its count is spelled in, so widening a
//!    bound past its encoded width breaks the build rather than truncating a count in
//!    an emitted image.
//!
//! The counts that are *not* covered by a §E bound — a function's span table, a
//! sparse-write key path, and a section body's own byte length — carry their own
//! derivation at their site.

use crate::bounds;
use crate::digest::{ImageId, image_id};
use crate::draft::{CollectionTypeDef, ConstValue, ImageBuildError, ImageDraft, KeyColumn};
use crate::durable_id::{
    DurableContractId, DurableGraphTooLarge, DurableIndexComponent, DurableIndexShape,
};
use crate::instr::Instr;
use crate::product::{
    DeclarationMemberShape, DeclarationNode, ProductClaimConflict, ProductDeclarationGraph,
};
use crate::semantic::{SemanticPath, SemanticTarget};
use crate::ty::ImageType;
use crate::value_dag::{
    CanonicalValueShapeDag, ImageByteSink, ValueShapeView, ValueShapeWireForm, expand, push_u16,
};

/// Container magic and version.
const MAGIC: &[u8; 4] = b"MWI\0";
const VERSION: u8 = 0x00;
const SECTION_COUNT: u8 = 10;

/// The length of a DURABLE section body, counted without building it.
///
/// Every other section's size follows from row counts the draft already bounds. A durable
/// field value is different: the wire spells a value shape as its full expansion, and a
/// shape shared across nesting levels expands geometrically in its depth, so a draft well
/// inside every declared bound can still describe a body larger than any image may be.
///
/// This sink writes nothing. It saturates one byte past the whole-image ceiling rather
/// than at it, so "full" and "fits" stay distinguishable, and [`expand`] returns the
/// moment it saturates — the work an over-deep shape costs is the bytes the ceiling
/// admits, not the bytes its expansion would have produced, and no buffer, hash input, or
/// output is allocated to find that out.
#[derive(Default)]
struct DurableBodyLowerBound(usize);

impl DurableBodyLowerBound {
    /// Whether the body alone, contract identity included, is larger than any image may
    /// be.
    fn over_ceiling(&self) -> bool {
        self.0.saturating_add(DurableContractId::BYTES) > bounds::MAX_IMAGE_BYTES
    }
}

impl ImageByteSink for DurableBodyLowerBound {
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

/// The encoded image plus its digest.
#[derive(Debug, Clone)]
pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub image_id: ImageId,
}

/// A draft that has passed the complete legacy producer-side policy and coherence
/// journey — every result the old encoder could reach before the whole-image ceiling,
/// in the order the old encoder reached it.
///
/// **Temporary bridge.** Its only caller is [`ImageDraft::encode`]; it exists so
/// [`LegacyDurableBodyLowerBoundFence`] may refuse an over-ceiling DURABLE body
/// *early*, without letting the refusal overtake a result the old encoder reported
/// first. Nothing else may mint one, and the borrow binds it to the one draft it
/// examined.
///
/// **Deletion condition.** This owner exists only to preserve the order in which the old
/// encoder reported producer-side results. It goes when the encoder decides every
/// producer-side result from the draft alone, before building any section, so that no
/// outcome depends on replaying a build order.
///
/// # What it replays, in order
///
/// 1. [`ImageDraft::check_bounds`] — every fixed §E count and width, the Product claim
///    conflict, the declaration graph's member/depth/value-depth walk, the value-shape
///    arena's fan-out walk, and the per-occurrence key/index widths.
/// 2. The application identity a non-empty durable graph requires.
/// 3. The site table's path projection, streamed and retained nowhere.
///
/// The one remaining producer-side result — CodeBytes, and its sibling operand
/// references — keeps its own position inside [`ImageDraft::encode`] rather than moving
/// here: it is discovered before the body's ceiling result there, which is exactly where
/// it was discovered when the body was built first and refused last.
struct LegacyFullDraftCoherencePreflight<'a>(&'a ImageDraft);

impl<'a> LegacyFullDraftCoherencePreflight<'a> {
    fn run(draft: &'a ImageDraft) -> Result<Self, ImageBuildError> {
        draft.check_bounds()?;
        // A non-empty durable graph is anchored by the application's ledger id, and the
        // old encoder demanded it at the head of the DURABLE section, before any member
        // row. Demanding it here reports the same error at the same position.
        if !draft.root_occurrences().is_empty() && draft.application_identity().is_none() {
            return Err(ImageBuildError::InvalidReference("application identity"));
        }
        // The projection is walked once per reader — here, by the body's lower bound, and
        // by the body itself — rather than materialized once and held, because a retained
        // table is `MAX_SITES` owned paths live for the whole encode while a walk costs
        // only the rows it visits and retains none of them.
        draft.project_sites(|_| {})?;
        Ok(Self(draft))
    }
}

/// A draft whose DURABLE section body, contract identity included, is small enough for
/// an image to carry.
///
/// **Temporary bridge.** The body's length is counted through
/// [`DurableBodyLowerBound`] before a single byte of it is built, so a body no image
/// could carry is refused with the existing [`ImageBuildError::ImageTooLarge`] before any
/// expansion buffer, contract preimage, hash input, or output is allocated, and only
/// fitting input reaches the old encoder.
///
/// The fence bounds *this producer's* peak allocation, which is what makes the phase
/// maxima below statable. It does not bound what any other caller of
/// [`DurableContractView::contract_id`] can ask for, and it never could: a
/// view is a view over an arena, and this type has no reach into one. That the
/// contract hash is never computed over bytes no image can carry is a property of the
/// identity owner's own ceiling on its canonical payload, which holds for every caller
/// including this one.
///
/// # `B_LEGACY_DRAFT` — the retained baseline this bridge stands on
///
/// `B_LEGACY_DRAFT[target]` is the retained [`ImageDraft`] baseline every encode carries:
/// the string pool and its bytes, the constant pool, the record/enum/collection tables,
/// the canonical Product declaration table with its member rows and its one
/// [`CanonicalValueShapeDag`], the root-occurrence table with its key tuples and managed
/// indexes, the site-demand plan with its retained rows, the function table with its
/// instruction tapes and span tables, the export table, and the test-entry table —
/// each at its own `MAX_*` bound in [`crate::bounds`], plus the handles and backing
/// allocations `Vec` and `HashMap` reserve for them. It is live throughout
/// [`ImageDraft::encode`] and so is stated once, as a baseline, never as a per-phase term.
/// It dies with the draft owner it describes, at a later row.
///
/// The contract identity contributes no term of its own. It is computed by streaming the
/// canonical payload into the hash, over a length the identity owner counts without
/// writing it, so no preimage buffer coexists with the body at any size — which is why
/// this bridge no longer states a peak-allocation maximum.
///
/// **Deletion condition.** This owner exists only to refuse an over-ceiling body before
/// the old encoder allocates one. It goes when IMGMEASURE installs policy-clean
/// whole-image-ceiling-capped paired-codec measurement before any encoder allocation.
struct LegacyDurableBodyLowerBoundFence<'a> {
    draft: &'a ImageDraft,
    /// The exact body length the count measured, so the body is allocated once at its
    /// final size. A `Vec` grown by doubling would transiently hold two buffers and up
    /// to twice the bytes, which is exactly the term phase (A) above states.
    body_len: usize,
}

impl<'a> LegacyDurableBodyLowerBoundFence<'a> {
    /// Count the DURABLE body a coherent draft would write, and admit only a body an
    /// image can carry.
    fn admit(
        preflight: LegacyFullDraftCoherencePreflight<'a>,
        str_map: &[u16],
    ) -> Result<Self, ImageBuildError> {
        let draft = preflight.0;
        let mut lower_bound = DurableBodyLowerBound::default();
        draft.write_durable_body(&mut lower_bound, str_map)?;
        if lower_bound.over_ceiling() {
            return Err(ImageBuildError::ImageTooLarge);
        }
        Ok(Self {
            draft,
            body_len: lower_bound.0,
        })
    }

    /// The DURABLE section body: the graph the lower bound just measured, written this
    /// time, and closed by the 32-byte durable-contract identity.
    ///
    /// This is the only site in the crate that mints a contract identity, so the identity
    /// exists only for a graph that already fits.
    fn encode_body(self, str_map: &[u16]) -> Result<Vec<u8>, ImageBuildError> {
        let mut body = Vec::with_capacity(self.body_len + DurableContractId::BYTES);
        self.draft.write_durable_body(&mut body, str_map)?;
        let identity = self
            .draft
            .contract_view()
            .contract_id()
            .map_err(|DurableGraphTooLarge| ImageBuildError::ImageTooLarge)?;
        body.extend_from_slice(identity.bytes());
        Ok(body)
    }
}

impl ImageDraft {
    /// Encode the draft into canonical container bytes, or fail with a producer-side
    /// [`ImageBuildError`] when a §E bound is exceeded or a reference is invalid.
    pub fn encode(&self) -> Result<EncodedImage, ImageBuildError> {
        let preflight = LegacyFullDraftCoherencePreflight::run(self)?;

        let str_map = self.string_sort_map();
        let sorted_strings = self.sorted_strings(&str_map);
        let (const_map, sorted_consts) = self.const_sort(&str_map);

        // CodeBytes and its sibling operand references are discovered before the
        // whole-image ceiling result, so they are computed before the body is measured.
        // Their section lands at 0x05 below.
        let function_offsets = self.encode_functions(&str_map, &const_map)?;

        // The DURABLE body is the one section whose size is not bounded by the draft's
        // own row counts: a value shape's wire form is its expansion, and a shape shared
        // across levels expands geometrically in its depth. Its length is therefore
        // counted before it is built, and a body no image could carry is refused here,
        // with the same ImageBytes result it has always drawn.
        let durable =
            LegacyDurableBodyLowerBoundFence::admit(preflight, &str_map)?.encode_body(&str_map)?;

        let mut tail = Vec::new();
        tail.push(SECTION_COUNT);
        push_section(&mut tail, 0x01, encode_strings(&sorted_strings))?;
        push_section(&mut tail, 0x02, self.encode_types(&str_map))?;
        push_section(&mut tail, 0x03, durable)?;
        push_section(&mut tail, 0x04, encode_consts(&sorted_consts, &str_map))?;
        push_section(&mut tail, 0x05, function_offsets.body)?;
        push_section(&mut tail, 0x06, self.encode_exports())?;
        push_section(&mut tail, 0x07, self.encode_spans(&function_offsets.per_fn))?;
        push_section(&mut tail, 0x08, self.encode_test_entries(&str_map))?;
        push_section(&mut tail, 0x09, self.encode_enums(&str_map))?;
        push_section(&mut tail, 0x0A, self.encode_collections())?;

        let id = image_id(&tail);
        let mut bytes = Vec::with_capacity(37 + tail.len());
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&id.0);
        bytes.extend_from_slice(&tail);

        if bytes.len() > bounds::MAX_IMAGE_BYTES {
            return Err(ImageBuildError::ImageTooLarge);
        }
        Ok(EncodedImage {
            bytes,
            image_id: id,
        })
    }

    fn check_bounds(&self) -> Result<(), ImageBuildError> {
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
        for record in self.types() {
            if record.fields.len() > bounds::MAX_RECORD_FIELDS {
                return Err(ImageBuildError::TooManyFields);
            }
        }
        if self.enums().len() > bounds::MAX_ENUMS {
            return Err(ImageBuildError::TooManyEnums);
        }
        if self.collections().len() > bounds::MAX_COLLECTIONS {
            return Err(ImageBuildError::TooManyCollections);
        }
        for enum_def in self.enums() {
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
        if let Some(conflict) = self.product_conflict() {
            return Err(match conflict {
                ProductClaimConflict::Graph(_) => ImageBuildError::ProductGraphConflict,
                ProductClaimConflict::EntryRecord(_) => ImageBuildError::ProductEntryRecordConflict,
            });
        }
        if self.root_occurrences().len() > bounds::MAX_ROOTS {
            return Err(ImageBuildError::TooManyRoots);
        }
        // The member tree is a Product declaration fact, so it is validated once per
        // declaration however many roots project it; the key tuple and managed indexes
        // are occurrence facts and are validated per root.
        for declaration in self.product_declarations() {
            validate_declaration_graph(declaration.graph(), self.value_shapes())?;
        }
        // Every distinct value shape is measured once, whatever the number of fields
        // that reference it: a shape shared by a thousand fields is one node here.
        validate_value_shapes(self.value_shapes())?;
        for occurrence in self.root_occurrences() {
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
        if self.site_demand() > bounds::MAX_SITES {
            return Err(ImageBuildError::TooManySites);
        }
        if self.functions().len() > bounds::MAX_FUNCTIONS {
            return Err(ImageBuildError::TooManyFunctions);
        }
        if self.export_entries().len() > bounds::MAX_EXPORTS {
            return Err(ImageBuildError::TooManyExports);
        }
        if self.test_entry_rows().len() > bounds::MAX_TEST_ENTRIES {
            return Err(ImageBuildError::TooManyTestEntries);
        }
        for function in self.functions() {
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

    /// `str_map[old_id] = final byte-sorted index`.
    fn string_sort_map(&self) -> Vec<u16> {
        let mut order: Vec<usize> = (0..self.strings().len()).collect();
        order.sort_by(|&a, &b| {
            self.strings()[a]
                .as_bytes()
                .cmp(self.strings()[b].as_bytes())
        });
        let mut map = vec![0u16; self.strings().len()];
        for (final_index, &old) in order.iter().enumerate() {
            map[old] = final_index as u16;
        }
        map
    }

    fn sorted_strings(&self, str_map: &[u16]) -> Vec<String> {
        let mut sorted = vec![String::new(); self.strings().len()];
        for (old, text) in self.strings().iter().enumerate() {
            sorted[str_map[old] as usize] = text.clone();
        }
        sorted
    }

    /// Returns `const_map[old_id] = final index` and the constants in canonical order.
    fn const_sort(&self, str_map: &[u16]) -> (Vec<u16>, Vec<ConstValue>) {
        let mut order: Vec<usize> = (0..self.consts().len()).collect();
        order.sort_by(|&a, &b| {
            self.consts()[a]
                .sort_key(str_map)
                .cmp(&self.consts()[b].sort_key(str_map))
        });
        let mut map = vec![0u16; self.consts().len()];
        let mut sorted = Vec::with_capacity(self.consts().len());
        for (final_index, &old) in order.iter().enumerate() {
            map[old] = final_index as u16;
            sorted.push(self.consts()[old]);
        }
        (map, sorted)
    }

    fn encode_types(&self, str_map: &[u16]) -> Vec<u8> {
        let mut body = Vec::new();
        push_u16(&mut body, self.types().len() as u16);
        for record in self.types() {
            push_u16(&mut body, str_map[record.name.raw() as usize]);
            push_u16(&mut body, record.fields.len() as u16);
            for field in &record.fields {
                push_u16(&mut body, str_map[field.name.raw() as usize]);
                field.ty.encode(&mut body);
                body.push(u8::from(field.required));
            }
        }
        body
    }

    /// Encode the ENUMS table (section 0x09): a count, then per enum its name
    /// string index, a variant count, and per variant a name string index, a
    /// `category` flag byte, a payload count, and one bare-`ImageType` reference per
    /// payload leaf in declaration order (a scalar tag, or a tag plus a big-endian
    /// `u16` index for a record or enum leaf).
    fn encode_enums(&self, str_map: &[u16]) -> Vec<u8> {
        let mut body = Vec::new();
        push_u16(&mut body, self.enums().len() as u16);
        for enum_def in self.enums() {
            push_u16(&mut body, str_map[enum_def.name.raw() as usize]);
            push_u16(&mut body, enum_def.variants.len() as u16);
            for variant in &enum_def.variants {
                push_u16(&mut body, str_map[variant.name.raw() as usize]);
                body.push(u8::from(variant.category));
                body.push(variant.payload.len() as u8);
                for ty in &variant.payload {
                    ty.encode(&mut body);
                }
            }
        }
        body
    }

    /// Encode the COLLTYPES table (section 0x0A): a count, then per collection type
    /// a one-byte kind tag (`0x00` List, `0x01` Map) followed by its bare-`ImageType`
    /// element reference (List) or key then value references (Map). Element/key/value
    /// references may themselves be `Collection` tags into an earlier COLLTYPES row.
    fn encode_collections(&self) -> Vec<u8> {
        let mut body = Vec::new();
        push_u16(&mut body, self.collections().len() as u16);
        for coll in self.collections() {
            match coll {
                CollectionTypeDef::List { elem } => {
                    body.push(0x00);
                    elem.encode(&mut body);
                }
                CollectionTypeDef::Map { key, value } => {
                    body.push(0x01);
                    key.encode(&mut body);
                    value.encode(&mut body);
                }
            }
        }
        body
    }

    /// Write the DURABLE section body, minus its closing contract identity, into `sink`.
    ///
    /// One owner writes the section for both of its readers: the lower bound that only
    /// counts the bytes, and the buffer that keeps them. A body admitted by the count is
    /// therefore the body that is built, because it is the same walk over the same rows.
    fn write_durable_body(
        &self,
        sink: &mut impl ImageByteSink,
        str_map: &[u16],
    ) -> Result<(), ImageBuildError> {
        push_u16(sink, self.root_occurrences().len() as u16);
        // The application's ledger id anchors a non-empty durable graph; a
        // storeless image carries none.
        if !self.root_occurrences().is_empty() {
            let application = self
                .application_identity()
                .ok_or(ImageBuildError::InvalidReference("application identity"))?;
            sink.extend_bytes(application.bytes());
        }
        // v0 carries the whole member graph per root, so each occurrence projects the
        // one retained declaration it references rather than carrying its own copy.
        for occurrence in self.root_occurrences() {
            let declaration = self.declaration_of(occurrence);
            push_u16(sink, str_map[occurrence.name().raw() as usize]);
            // The key tuple: a count, then each column's scalar type and ledger id.
            // Zero columns is a singleton root; more than one is a composite key.
            encode_key_tuple(sink, occurrence.keys());
            push_u16(sink, declaration.root_entry_record().0);
            // The root's remaining ledger identity block: placement and product,
            // then the resource's durable member tree (top-level fields interleaved
            // with static `group` namespaces and keyed `branch` placements).
            sink.extend_bytes(occurrence.placement().ledger_id().bytes());
            sink.extend_bytes(declaration.identity().ledger_id().bytes());
            let graph = declaration.graph();
            encode_declaration_members(sink, graph, graph.members(), str_map, self.value_shapes())?;
            // A body already past the ceiling is decided; the remaining roots would only
            // add to a count that is already refused.
            if sink.is_full() {
                return Ok(());
            }
            encode_durable_indexes(sink, occurrence.indexes());
        }
        // The retained row count, not the demand: the plan refuses to mint past its
        // capacity, so rows never exceed the demand `check_bounds` measured against
        // `MAX_SITES`, and the count fits the `u16` the table is prefixed with.
        push_u16(sink, self.site_row_count() as u16);
        self.project_sites(|site| {
            encode_site_path(sink, &site.path);
            sink.push(match site.target {
                SemanticTarget::WholePayload => 0x00,
                SemanticTarget::FieldLeaf => 0x01,
                SemanticTarget::IndexScan => 0x02,
                SemanticTarget::IndexLookup => 0x03,
                SemanticTarget::GroupEntry => 0x04,
            });
        })
    }

    fn encode_functions(
        &self,
        str_map: &[u16],
        const_map: &[u16],
    ) -> Result<EncodedFunctions, ImageBuildError> {
        let mut body = Vec::new();
        push_u16(&mut body, self.functions().len() as u16);
        let mut per_fn = Vec::with_capacity(self.functions().len());
        for function in self.functions() {
            let layout = code_layout(&function.code);
            if layout.total_len as usize > bounds::MAX_CODE_BYTES {
                return Err(ImageBuildError::CodeTooLong);
            }
            push_u16(&mut body, str_map[function.name.raw() as usize]);
            push_u16(&mut body, str_map[function.source.raw() as usize]);
            body.push(function.params.len() as u8);
            for param in &function.params {
                param.encode(&mut body);
            }
            function.ret.encode(&mut body);
            push_u16(&mut body, function.local_count);
            push_u32(&mut body, layout.total_len);
            let code = encode_code(&function.code, &layout, const_map)?;
            body.extend_from_slice(&code);
            per_fn.push(layout);
        }
        Ok(EncodedFunctions { body, per_fn })
    }

    /// Encode the EXPORTS table: a count, then each `32-byte ExportId ‖ u16 func`
    /// entry in strictly ascending id order. The id is the only export key carried;
    /// the source name is not, so the VM can only dispatch on a verified id.
    fn encode_exports(&self) -> Vec<u8> {
        let mut entries = self.export_entries();
        entries.sort_by(|a, b| a.0.bytes().cmp(b.0.bytes()));
        let mut body = Vec::new();
        push_u16(&mut body, entries.len() as u16);
        for (id, func) in entries {
            body.extend_from_slice(id.bytes());
            push_u16(&mut body, func);
        }
        body
    }

    /// Encode the TEST-ENTRY table (section 0x08): a count, then each
    /// `u16 name-string-index ‖ u16 function-index` entry in strictly ascending
    /// name-index order. The name index is remapped through the string sort map;
    /// names are unique across the project, so the sort is total and the verifier
    /// rechecks the strict ordering.
    fn encode_test_entries(&self, str_map: &[u16]) -> Vec<u8> {
        let mut entries: Vec<(u16, u16)> = self
            .test_entry_rows()
            .into_iter()
            .map(|(name, func)| (str_map[name as usize], func))
            .collect();
        entries.sort_by_key(|(name, _)| *name);
        let mut body = Vec::new();
        push_u16(&mut body, entries.len() as u16);
        for (name, func) in entries {
            push_u16(&mut body, name);
            push_u16(&mut body, func);
        }
        body
    }

    /// Encode the SPANS section: per function in table order, a `u16` span count then
    /// that many `u32(offset) ‖ u32(line) ‖ u32(column)` rows.
    ///
    /// A span table is the one encoder row set no §E bound guards — spans are debug
    /// positions the producer supplies, not a declared program shape, and nothing ties
    /// their number to the instruction count. The whole-image ceiling is the binder
    /// instead: a count the `u16` prefix cannot spell needs at least
    /// `65_536 * SPAN_ROW_BYTES` bytes of rows behind it, which exceeds
    /// [`bounds::MAX_IMAGE_BYTES`] on its own, so [`ImageDraft::encode`] refuses such a
    /// draft with [`ImageBuildError::ImageTooLarge`] and no image carrying a truncated
    /// span count can be accepted. The assertion below fails the build if a later
    /// ceiling widening breaks that derivation, at which point this count needs a
    /// bound and a typed refusal of its own rather than a comment.
    fn encode_spans(&self, per_fn: &[CodeLayout]) -> Vec<u8> {
        const SPAN_ROW_BYTES: usize = 12;
        const _: () = assert!(
            bounds::MAX_IMAGE_BYTES / SPAN_ROW_BYTES < u16::MAX as usize,
            "the image ceiling must refuse a span table before its count outgrows the u16 prefix",
        );

        let mut body = Vec::new();
        for (function, layout) in self.functions().iter().zip(per_fn) {
            push_u16(&mut body, function.spans.len() as u16);
            for span in &function.spans {
                let offset = layout.offsets[span.instr_index as usize];
                push_u32(&mut body, offset);
                push_u32(&mut body, span.line);
                push_u32(&mut body, span.column);
            }
        }
        body
    }
}

struct EncodedFunctions {
    body: Vec<u8>,
    per_fn: Vec<CodeLayout>,
}

/// The byte offset of each instruction plus the total code length.
struct CodeLayout {
    offsets: Vec<u32>,
    total_len: u32,
}

/// Lay out one function's code: each instruction's byte offset, and the total width.
///
/// Offsets are `u32` because the container spells them so. The running total cannot
/// reach that width from a draft an encode can hold: an instruction is at least one
/// encoded byte, so reaching `u32::MAX` would need a code vector of four billion
/// instructions — orders of magnitude past the memory the draft itself occupies, and
/// past [`bounds::MAX_CODE_BYTES`], which `encode_functions` rechecks against
/// `total_len` before the layout is used for anything.
fn code_layout(code: &[Instr]) -> CodeLayout {
    let mut offsets = Vec::with_capacity(code.len());
    let mut offset = 0u32;
    for instr in code {
        offsets.push(offset);
        offset += instr.encoded_len() as u32;
    }
    CodeLayout {
        offsets,
        total_len: offset,
    }
}

fn encode_code(
    code: &[Instr],
    layout: &CodeLayout,
    const_map: &[u16],
) -> Result<Vec<u8>, ImageBuildError> {
    let mut out = Vec::with_capacity(layout.total_len as usize);
    for instr in code {
        out.push(instr.opcode());
        match instr {
            Instr::ConstLoad(raw) | Instr::Unreachable(raw) | Instr::Todo(raw) => {
                push_u16(&mut out, const_map[*raw as usize])
            }
            Instr::LocalGet(l) | Instr::LocalSet(l) => push_u16(&mut out, *l),
            Instr::Call(f) => push_u16(&mut out, *f),
            Instr::RecordNew(t) => push_u16(&mut out, *t),
            Instr::ListNew(c) | Instr::MapNew(c) | Instr::TextSplit(c) | Instr::TextLines(c) => {
                push_u16(&mut out, *c)
            }
            Instr::FieldGet(f) | Instr::FieldSet(f) | Instr::FieldUnset(f) => {
                push_u16(&mut out, *f)
            }
            Instr::DurExists(s)
            | Instr::DurFamilyExists(s)
            | Instr::DurReadField(s)
            | Instr::DurReadEntry(s)
            | Instr::DurSetRequired(s)
            | Instr::DurSetSparse(s)
            | Instr::DurCreateEntry(s)
            | Instr::DurReplaceEntry(s)
            | Instr::DurEraseField(s)
            | Instr::DurEraseEntry(s)
            | Instr::DurReadGroup(s)
            | Instr::DurReplaceGroup(s)
            | Instr::DurEraseGroup(s) => push_u16(&mut out, s.encodable()?),
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
                    .ok_or(ImageBuildError::InvalidReference("jump target"))?;
                push_u32(&mut out, byte_offset);
            }
            Instr::VacantLoad(ty) => ty.encode(&mut out),
            Instr::RangeGuard { lo, hi } => {
                out.extend_from_slice(&lo.to_be_bytes());
                out.extend_from_slice(&hi.to_be_bytes());
            }
            Instr::EnumConstruct { enum_idx, variant } => {
                push_u16(&mut out, *enum_idx);
                push_u16(&mut out, *variant);
            }
            Instr::EnumPayloadGet { variant, field } => {
                push_u16(&mut out, *variant);
                push_u16(&mut out, *field);
            }
            Instr::DurSetSparsePresent { site, key_slots } => {
                push_u16(&mut out, site.encodable()?);
                // The slot count fits its `u16` prefix: each slot is two more bytes of
                // this instruction's operand, and `encode_functions` has already
                // refused a function whose laid-out code exceeds
                // `bounds::MAX_CODE_BYTES` — half that ceiling in slots is well inside
                // the prefix.
                push_u16(&mut out, key_slots.len() as u16);
                for slot in key_slots {
                    push_u16(&mut out, *slot);
                }
            }
            Instr::DurIterateBounded {
                site,
                limit,
                from,
                list_ty,
            } => {
                push_u16(&mut out, site.encodable()?);
                push_u32(&mut out, *limit);
                out.push(u8::from(*from));
                push_u16(&mut out, *list_ty);
            }
            Instr::MakeIdentity { root, cols } => {
                push_u16(&mut out, *root);
                push_u16(&mut out, *cols);
            }
            Instr::IdentityKeyPath(cols) => push_u16(&mut out, *cols),
            Instr::DurIndexScan {
                site,
                limit,
                from,
                list_ty,
            } => {
                push_u16(&mut out, site.encodable()?);
                push_u32(&mut out, *limit);
                out.push(u8::from(*from));
                push_u16(&mut out, *list_ty);
            }
            Instr::DurIndexLookup(site) | Instr::DurIndexExists(site) => {
                push_u16(&mut out, site.encodable()?)
            }
            _ => {}
        }
    }
    Ok(out)
}

fn encode_strings(sorted: &[String]) -> Vec<u8> {
    let mut body = Vec::new();
    push_u16(&mut body, sorted.len() as u16);
    for text in sorted {
        push_u16(&mut body, text.len() as u16);
        body.extend_from_slice(text.as_bytes());
    }
    body
}

fn encode_consts(sorted: &[ConstValue], str_map: &[u16]) -> Vec<u8> {
    let mut body = Vec::new();
    push_u16(&mut body, sorted.len() as u16);
    for value in sorted {
        match value {
            ConstValue::Int(v) => {
                body.push(0x01);
                body.extend_from_slice(&v.to_be_bytes());
            }
            ConstValue::Bool(v) => {
                body.push(0x02);
                body.push(u8::from(*v));
            }
            ConstValue::Text(str_id) => {
                body.push(0x03);
                push_u16(&mut body, str_map[str_id.raw() as usize]);
            }
            ConstValue::Date(v) => {
                body.push(0x04);
                body.extend_from_slice(&v.to_be_bytes());
            }
            ConstValue::Instant(v) => {
                body.push(0x05);
                body.extend_from_slice(&v.to_be_bytes());
            }
            ConstValue::Duration(v) => {
                body.push(0x06);
                body.extend_from_slice(&v.to_be_bytes());
            }
        }
    }
    body
}

/// Append one section: `u8(id) ‖ u32(body_len) ‖ body`.
///
/// The body length fits its `u32` prefix. Every section but two is built from rows the
/// §E bounds cap, and the widest such body — `MAX_FUNCTIONS` × `MAX_CODE_BYTES` of code
/// — is a few hundred mebibytes. The DURABLE body's size does not follow from a row
/// count, because a value shape expands geometrically in its depth; it is fenced against
/// the whole-image ceiling by [`DurableBodyLowerBound`] before it is ever built. SPANS
/// is the remaining one: its rows are producer-supplied and no bound caps their number
/// (see `encode_spans`), so this length rests on the producer, which would have to hold
/// four gibibytes of spans in memory to reach the prefix width.
fn push_section(out: &mut Vec<u8>, id: u8, body: Vec<u8>) -> Result<(), ImageBuildError> {
    out.push(id);
    push_u32(out, body.len() as u32);
    out.extend_from_slice(&body);
    Ok(())
}

/// Encode a placement key tuple into the DURABLE section: `u16(count) ‖
/// [scalar_tag ‖ id(16)]*`. Shared by roots and branches; column order is
/// load-bearing.
fn encode_key_tuple(body: &mut impl ImageByteSink, keys: &[KeyColumn]) {
    push_u16(body, keys.len() as u16);
    for key in keys {
        ImageType::scalar(key.scalar).encode(body);
        body.extend_bytes(key.id.bytes());
    }
}

/// Encode one operation site's semantic path: `u8(step_count) ‖ step*`, each step a
/// `u8(ledger_kind) ‖ 16 id bytes`. The step kind byte is the same frozen ledger
/// `IDREF` tag a durable node's identity uses, so the verifier decodes the path's
/// kinds exactly as it spells its own node paths. The step count fits one byte: a path
/// is projected from the declaration graph, so it is the two root steps plus that node's
/// nesting depth, and `validate_declaration_graph` has already refused a graph nesting
/// past `MAX_DURABLE_DEPTH` — well inside `MAX_SITE_PATH_STEPS`, itself far below 256.
fn encode_site_path(body: &mut impl ImageByteSink, path: &SemanticPath) {
    body.push(path.steps().len() as u8);
    for step in path.steps() {
        body.push(step.kind.ledger_kind());
        body.extend_bytes(step.id.bytes());
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
    str_map: &[u16],
    values: &CanonicalValueShapeDag,
) -> Result<(), ImageBuildError> {
    push_u16(body, members.len() as u16);
    for member in members {
        match member.shape() {
            DeclarationMemberShape::Field {
                id,
                required,
                value,
            } => {
                body.push(0x00);
                body.extend_bytes(id.bytes());
                body.push(u8::from(*required));
                // An arity the section's own `u16` cannot spell has no encodable image,
                // which is the whole-image ceiling's answer under a different name. The
                // draft's arena fan-out recheck runs first and bounds every arity far
                // below that width, so this propagates a refusal rather than opening a
                // second way to reach one.
                expand(values, *value, ValueShapeWireForm::DurableSection, body)
                    .map_err(|DurableGraphTooLarge| ImageBuildError::ImageTooLarge)?;
            }
            DeclarationMemberShape::Group { id } => {
                body.push(0x01);
                body.extend_bytes(id.bytes());
                encode_declaration_members(body, graph, graph.members_of(member), str_map, values)?;
            }
            DeclarationMemberShape::Branch {
                placement,
                name,
                record,
                keys,
            } => {
                body.push(0x02);
                body.extend_bytes(placement.bytes());
                push_u16(body, str_map[name.raw() as usize]);
                push_u16(body, record.0);
                encode_key_tuple(body, keys);
                encode_declaration_members(body, graph, graph.members_of(member), str_map, values)?;
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
    push_u16(body, indexes.len() as u16);
    for index in indexes {
        body.extend_bytes(index.id.bytes());
        body.push(u8::from(index.unique));
        push_u16(body, index.components.len() as u16);
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

/// Recheck the durable member-graph bounds a well-formed declaration must satisfy: total
/// member rows within [`bounds::MAX_DURABLE_MEMBERS`] and nesting within
/// [`bounds::MAX_DURABLE_DEPTH`]. A branch's key tuple is bounded by the same
/// [`bounds::MAX_KEY_COLUMNS`] as a root's.
///
/// Every row carries its parent's ordinal and a parent always precedes its children, so
/// this is one forward pass over the rows rather than a descent of the graph. A graph that
/// exceeded the member bound holds only the prefix the flattening was willing to
/// materialize; it reports the exceeded bound here and is never encoded.
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
/// fields, structs, or enum payloads reference it, and no occurrence is expanded.
///
/// # Declared precondition
///
/// The draft's arena is validated **as a whole**, not through the fields that reference
/// it. A node no declaration reaches is measured here like any other, and an over-bound
/// one refuses the encode. That is deliberate: the arena is the draft's own retained
/// state, so a node past a value bound is a producer defect wherever it came from, and
/// deciding it by reachability would mean the same draft encodes or refuses depending on
/// a traversal — while costing a reachability walk to learn something no correct producer
/// can produce. Every downstream owner may therefore assume that an encoded arena is
/// within the value bounds, whatever part of it a walk happens to visit.
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

fn push_u32(out: &mut impl ImageByteSink, value: u32) {
    out.extend_bytes(&value.to_be_bytes());
}
