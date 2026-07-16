//! The canonical container encoder (design §C).
//!
//! Turns a validated [`ImageDraft`] into the sectioned, length-prefixed,
//! big-endian image bytes with a computed digest. The encoder sorts the string and
//! constant pools into canonical order, rewrites every reference through the sort
//! maps, and lays out each function's bytecode so jump targets — held as
//! instruction indices while drafting — become container byte offsets.

use crate::bounds;
use crate::digest::{ImageId, image_id};
use crate::draft::{
    CollectionTypeDef, ConstValue, DurableMemberDef, ImageBuildError, ImageDraft, KeyColumn,
};
use crate::durable_id::{
    DurableBranchShape, DurableContractDescriptor, DurableFieldShape, DurableGroupShape,
    DurableIndexComponent, DurableIndexShape, DurableKeyShape, DurableMemberShape,
    DurableRootShape, DurableValueShape,
};
use crate::instr::Instr;
use crate::semantic::{SemanticPath, SemanticTarget};
use crate::ty::ImageType;

/// Container magic and version.
const MAGIC: &[u8; 4] = b"MWI\0";
const VERSION: u8 = 0x00;
const SECTION_COUNT: u8 = 10;

/// The encoded image plus its digest.
#[derive(Debug, Clone)]
pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub image_id: ImageId,
}

impl ImageDraft {
    /// Encode the draft into canonical container bytes, or fail with a producer-side
    /// [`ImageBuildError`] when a §E bound is exceeded or a reference is invalid.
    pub fn encode(&self) -> Result<EncodedImage, ImageBuildError> {
        self.check_bounds()?;

        let str_map = self.string_sort_map();
        let sorted_strings = self.sorted_strings(&str_map);
        let (const_map, sorted_consts) = self.const_sort(&str_map);

        let mut tail = Vec::new();
        tail.push(SECTION_COUNT);
        push_section(&mut tail, 0x01, encode_strings(&sorted_strings))?;
        push_section(&mut tail, 0x02, self.encode_types(&str_map))?;
        push_section(&mut tail, 0x03, self.encode_durable(&str_map)?)?;
        push_section(&mut tail, 0x04, encode_consts(&sorted_consts, &str_map))?;
        let function_offsets = self.encode_functions(&str_map, &const_map)?;
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
            if record.fields.len() > bounds::MAX_FIELDS {
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
        if self.roots().len() > bounds::MAX_ROOTS {
            return Err(ImageBuildError::TooManyRoots);
        }
        for root in self.roots() {
            if root.keys.len() > bounds::MAX_KEY_COLUMNS {
                return Err(ImageBuildError::TooManyKeyColumns);
            }
            let mut member_count = 0usize;
            validate_member_tree(&root.identity.members, 1, &mut member_count)?;
            if root.identity.indexes.len() > bounds::MAX_INDEXES {
                return Err(ImageBuildError::TooManyIndexes);
            }
            for index in &root.identity.indexes {
                if index.components.len() > bounds::MAX_INDEX_COMPONENTS {
                    return Err(ImageBuildError::TooManyIndexComponents);
                }
            }
        }
        if self.sites().len() > bounds::MAX_SITES {
            return Err(ImageBuildError::TooManySites);
        }
        for site in self.sites() {
            let steps = site.path.steps().len();
            if steps < bounds::MIN_SITE_PATH_STEPS {
                return Err(ImageBuildError::SitePathTooShort);
            }
            if steps > bounds::MAX_SITE_PATH_STEPS {
                return Err(ImageBuildError::SitePathTooDeep);
            }
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

    fn encode_durable(&self, str_map: &[u16]) -> Result<Vec<u8>, ImageBuildError> {
        let mut body = Vec::new();
        push_u16(&mut body, self.roots().len() as u16);
        // The application's ledger id anchors a non-empty durable graph; a
        // storeless image carries none.
        if !self.roots().is_empty() {
            let application = self
                .application_identity()
                .ok_or(ImageBuildError::InvalidReference("application identity"))?;
            body.extend_from_slice(application.bytes());
        }
        for root in self.roots() {
            push_u16(&mut body, str_map[root.name.raw() as usize]);
            // The key tuple: a count, then each column's scalar type and ledger id.
            // Zero columns is a singleton root; more than one is a composite key.
            encode_key_tuple(&mut body, &root.keys);
            push_u16(&mut body, root.record.0);
            // The root's remaining ledger identity block: placement and product,
            // then the resource's durable member tree (top-level fields interleaved
            // with static `group` namespaces and keyed `branch` placements).
            body.extend_from_slice(root.identity.placement.bytes());
            body.extend_from_slice(root.identity.product.bytes());
            encode_durable_members(&mut body, &root.identity.members, str_map);
            encode_durable_indexes(&mut body, &root.identity.indexes);
        }
        push_u16(&mut body, self.sites().len() as u16);
        for site in self.sites() {
            encode_site_path(&mut body, &site.path);
            body.push(match site.target {
                SemanticTarget::WholePayload => 0x00,
                SemanticTarget::FieldLeaf => 0x01,
                SemanticTarget::IndexScan => 0x02,
                SemanticTarget::IndexLookup => 0x03,
            });
        }
        // The durable-contract identity closes the section: a 32-byte
        // `DurableContractId` over the canonical graph descriptor. The verifier
        // recomputes it from the decoded roots/records and rejects a mismatch, so
        // these bytes are a producer-side commitment, not a trusted input.
        body.extend_from_slice(self.durable_descriptor()?.contract_id().bytes());
        Ok(body)
    }

    /// The canonical durable-graph descriptor for this draft, built from its
    /// application id and each root's ledger identity block (placement, product,
    /// key tuple, and the resource's durable member tree). The member tree is
    /// self-describing, so the descriptor no longer derives field shapes from the
    /// materialized record type.
    fn durable_descriptor(&self) -> Result<DurableContractDescriptor, ImageBuildError> {
        if self.roots().is_empty() {
            return Ok(DurableContractDescriptor::empty());
        }
        let application = self
            .application_identity()
            .ok_or(ImageBuildError::InvalidReference("application identity"))?;
        let roots = self
            .roots()
            .iter()
            .map(|root| DurableRootShape {
                placement: root.identity.placement,
                product: root.identity.product,
                keys: key_shapes(&root.keys),
                members: member_shapes(&root.identity.members),
                indexes: root.identity.indexes.clone(),
            })
            .collect();
        Ok(DurableContractDescriptor::new(application, roots))
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

    fn encode_spans(&self, per_fn: &[CodeLayout]) -> Vec<u8> {
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
            Instr::ConstLoad(raw) | Instr::Unreachable(raw) => {
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
            | Instr::DurReadField(s)
            | Instr::DurReadEntry(s)
            | Instr::DurSetRequired(s)
            | Instr::DurSetSparse(s)
            | Instr::DurCreateEntry(s)
            | Instr::DurReplaceEntry(s)
            | Instr::DurEraseField(s)
            | Instr::DurEraseEntry(s)
            | Instr::DurNextKey(s) => push_u16(&mut out, *s),
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
            Instr::DurSetSparsePresent { site, key_slot } => {
                push_u16(&mut out, *site);
                push_u16(&mut out, *key_slot);
            }
            Instr::DurIterateBounded { site, limit, from } => {
                push_u16(&mut out, *site);
                push_u32(&mut out, *limit);
                out.push(u8::from(*from));
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

fn push_section(out: &mut Vec<u8>, id: u8, body: Vec<u8>) -> Result<(), ImageBuildError> {
    out.push(id);
    push_u32(out, body.len() as u32);
    out.extend_from_slice(&body);
    Ok(())
}

/// Encode a placement key tuple into the DURABLE section: `u16(count) ‖
/// [scalar_tag ‖ id(16)]*`. Shared by roots and branches; column order is
/// load-bearing.
fn encode_key_tuple(body: &mut Vec<u8>, keys: &[KeyColumn]) {
    push_u16(body, keys.len() as u16);
    for key in keys {
        ImageType::scalar(key.scalar).encode(body);
        body.extend_from_slice(key.id.bytes());
    }
}

/// Encode one operation site's semantic path: `u8(step_count) ‖ step*`, each step a
/// `u8(ledger_kind) ‖ 16 id bytes`. The step kind byte is the same frozen ledger
/// `IDREF` tag a durable node's identity uses, so the verifier decodes the path's
/// kinds exactly as it spells its own node paths. The step count fits one byte
/// (`MAX_SITE_PATH_STEPS` is far below 256, rechecked in `check_bounds`).
fn encode_site_path(body: &mut Vec<u8>, path: &SemanticPath) {
    body.push(path.steps().len() as u8);
    for step in path.steps() {
        body.push(step.kind.ledger_kind());
        body.extend_from_slice(step.id.bytes());
    }
}

/// Encode a durable member tree into the DURABLE section: `u16(count) ‖ member*`.
/// A field is tag `0x00`, its ledger id, a required flag, and its value shape; a
/// group is tag `0x01`, its ledger id, and its own members; a branch is tag
/// `0x02`, its placement id, its key tuple, and its own members. Recurses through
/// groups and branches in source declaration order. A field's value shape is the
/// canonical [`DurableValueShape`] encoding, so a durable field and its
/// contract-identity contribution spell the value one way.
fn encode_durable_members(body: &mut Vec<u8>, members: &[DurableMemberDef], str_map: &[u16]) {
    push_u16(body, members.len() as u16);
    for member in members {
        match member {
            DurableMemberDef::Field {
                id,
                required,
                value,
            } => {
                body.push(0x00);
                body.extend_from_slice(id.bytes());
                body.push(u8::from(*required));
                encode_value_shape_section(body, value);
            }
            DurableMemberDef::Group { id, members } => {
                body.push(0x01);
                body.extend_from_slice(id.bytes());
                encode_durable_members(body, members, str_map);
            }
            DurableMemberDef::Branch {
                placement,
                name,
                record,
                keys,
                members,
            } => {
                body.push(0x02);
                body.extend_from_slice(placement.bytes());
                push_u16(body, str_map[name.raw() as usize]);
                push_u16(body, record.0);
                encode_key_tuple(body, keys);
                encode_durable_members(body, members, str_map);
            }
        }
    }
}

/// Encode a root's managed indexes into the DURABLE section: `u16(count) ‖ index*`.
/// Each index is its raw 16-byte `Index` ledger id, a `unique` flag byte, a
/// `u16(component_count)`, and per component a one-byte leaf kind (`0x02` field,
/// `0x04` key — the frozen IDREF kind bytes) and the leaf's raw 16-byte ledger id.
/// An index carries no value shape: it is derived from the leaves it projects.
fn encode_durable_indexes(body: &mut Vec<u8>, indexes: &[DurableIndexShape]) {
    push_u16(body, indexes.len() as u16);
    for index in indexes {
        body.extend_from_slice(index.id.bytes());
        body.push(u8::from(index.unique));
        push_u16(body, index.components.len() as u16);
        for component in &index.components {
            let (kind, id) = match component {
                DurableIndexComponent::Field(id) => (0x02u8, id),
                DurableIndexComponent::Key(id) => (0x04u8, id),
            };
            body.push(kind);
            body.extend_from_slice(id.bytes());
        }
    }
}

/// Recheck the durable member-tree bounds a well-formed draft must satisfy: total
/// member records within [`bounds::MAX_DURABLE_MEMBERS`] and nesting within
/// [`bounds::MAX_DURABLE_DEPTH`]. A branch's key tuple is bounded by the same
/// [`bounds::MAX_KEY_COLUMNS`] as a root's. `depth` is 1 for a top-level member.
fn validate_member_tree(
    members: &[DurableMemberDef],
    depth: usize,
    count: &mut usize,
) -> Result<(), ImageBuildError> {
    if depth > bounds::MAX_DURABLE_DEPTH {
        return Err(ImageBuildError::DurableTreeTooDeep);
    }
    for member in members {
        *count += 1;
        if *count > bounds::MAX_DURABLE_MEMBERS {
            return Err(ImageBuildError::TooManyDurableMembers);
        }
        match member {
            DurableMemberDef::Field { value, .. } => {
                validate_value_shape(value, 1)?;
            }
            DurableMemberDef::Group { members, .. } => {
                validate_member_tree(members, depth + 1, count)?;
            }
            DurableMemberDef::Branch { keys, members, .. } => {
                if keys.len() > bounds::MAX_KEY_COLUMNS {
                    return Err(ImageBuildError::TooManyKeyColumns);
                }
                validate_member_tree(members, depth + 1, count)?;
            }
        }
    }
    Ok(())
}

/// Encode a durable field value shape into the DURABLE section: `u8(value_tag) ‖
/// body`. A scalar is tag `0x00` and a bare scalar tag; a struct is tag `0x01`,
/// `u16(leaf_count)`, and its leaves; an enum is tag `0x02`, its raw 16-byte sum id,
/// `u16(member_count)`, and per member a raw 16-byte id, `u16(payload_count)`, and
/// its payload leaves. Ledger ids are raw 16 bytes here (as everywhere in this
/// section); the canonical contract-identity payload re-encodes them as kind-tagged
/// IDREFs through [`DurableValueShape::encode`], which the verifier recomputes.
fn encode_value_shape_section(body: &mut Vec<u8>, value: &DurableValueShape) {
    match value {
        DurableValueShape::Scalar(scalar) => {
            body.push(0x00);
            body.push(scalar.tag());
        }
        DurableValueShape::Struct(leaves) => {
            body.push(0x01);
            push_u16(body, leaves.len() as u16);
            for leaf in leaves {
                encode_value_shape_section(body, leaf);
            }
        }
        DurableValueShape::Enum { sum, members } => {
            body.push(0x02);
            body.extend_from_slice(sum.bytes());
            push_u16(body, members.len() as u16);
            for member in members {
                body.extend_from_slice(member.id.bytes());
                push_u16(body, member.payload.len() as u16);
                for leaf in &member.payload {
                    encode_value_shape_section(body, leaf);
                }
            }
        }
    }
}

/// Recheck a durable field value shape's nesting depth against
/// [`bounds::MAX_DURABLE_VALUE_DEPTH`], and every struct-leaf / enum-variant /
/// payload fan-out against the existing value-type bounds, so a well-formed draft
/// always encodes within the limits the verifier rechecks (§ law 9). `depth` is 1
/// for a top-level field value.
fn validate_value_shape(value: &DurableValueShape, depth: usize) -> Result<(), ImageBuildError> {
    if depth > bounds::MAX_DURABLE_VALUE_DEPTH {
        return Err(ImageBuildError::DurableValueTooDeep);
    }
    match value {
        DurableValueShape::Scalar(_) => {}
        DurableValueShape::Struct(leaves) => {
            if leaves.len() > bounds::MAX_FIELDS {
                return Err(ImageBuildError::TooManyFields);
            }
            for leaf in leaves {
                validate_value_shape(leaf, depth + 1)?;
            }
        }
        DurableValueShape::Enum { members, .. } => {
            if members.len() > bounds::MAX_VARIANTS {
                return Err(ImageBuildError::TooManyVariants);
            }
            for member in members {
                if member.payload.len() > bounds::MAX_PAYLOAD_FIELDS {
                    return Err(ImageBuildError::TooManyPayloadFields);
                }
                for leaf in &member.payload {
                    validate_value_shape(leaf, depth + 1)?;
                }
            }
        }
    }
    Ok(())
}

/// The descriptor key-tuple shapes for a placement's key columns.
fn key_shapes(keys: &[KeyColumn]) -> Vec<DurableKeyShape> {
    keys.iter()
        .map(|key| DurableKeyShape {
            scalar: key.scalar,
            id: key.id,
        })
        .collect()
}

/// Convert a draft member tree into the descriptor's member shapes, recursing
/// through groups and branches. The descriptor is the canonical identity owner;
/// this is the compiler-side projection into it.
fn member_shapes(members: &[DurableMemberDef]) -> Vec<DurableMemberShape> {
    members
        .iter()
        .map(|member| match member {
            DurableMemberDef::Field {
                id,
                required,
                value,
            } => DurableMemberShape::Field(DurableFieldShape {
                id: *id,
                required: *required,
                value: value.clone(),
            }),
            DurableMemberDef::Group { id, members } => {
                DurableMemberShape::Group(DurableGroupShape {
                    id: *id,
                    members: member_shapes(members),
                })
            }
            // The branch's name and record type are surface, not identity: the
            // descriptor carries only its placement, key tuple, and member value
            // shapes, so a rename or a record retype preserves the contract id.
            DurableMemberDef::Branch {
                placement,
                keys,
                members,
                ..
            } => DurableMemberShape::Branch(DurableBranchShape {
                placement: *placement,
                keys: key_shapes(keys),
                members: member_shapes(members),
            }),
        })
        .collect()
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
