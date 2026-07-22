//! The typed validating `ImageDraft` (design §C).
//!
//! The compiler builds an image through this owner: it interns strings and
//! constants, adds record types, roots, sites, functions, and exports, and calls
//! [`ImageDraft::encode`] to produce canonical container bytes with a computed
//! digest. Building works in logical intern ids (`StrId`, `ConstId`); the encoder
//! sorts the string and constant pools into their canonical order and rewrites
//! every reference, so the compiler never reasons about final pool positions.
//!
//! The draft enforces the §E representational bounds as it is built, so a
//! well-formed draft always encodes to bytes within them. The independent verifier
//! rechecks every bound against the received bytes; the draft's checks are a
//! producer-side guard, not the trust boundary.

use std::collections::HashMap;

use crate::durable_id::{DurableIndexShape, DurableValueShape, LedgerIdBytes};
use crate::export_id::ExportId;
use crate::instr::Instr;
use crate::semantic::{SemanticPath, SemanticTarget};
use crate::ty::{ImageType, Scalar};

/// A logical string-pool id, stable across the sort the encoder performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrId(u16);

/// A logical constant-pool id, stable across the sort the encoder performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstId(pub(crate) u16);

impl ConstId {
    /// The raw logical index, as carried in a `ConstLoad` operand until the encoder
    /// rewrites it to the final sorted pool position.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A record-type index (also the final container index; types keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub(crate) u16);

impl TypeId {
    /// The raw record-type index, as carried in a `RecordNew`/`FieldGet` operand and
    /// in an `ImageType::Record`.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// An enum-type index (also the final container index; enums keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub(crate) u16);

impl EnumId {
    /// The raw enum-type index, as carried in `EnumConstruct`/`EnumTag`/
    /// `EnumPayloadGet`/`EqEnum` operands and in an `ImageType::Enum`.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A collection-type index (also the final container index; collection types keep
/// insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollTypeId(pub(crate) u16);

impl CollTypeId {
    /// The raw collection-type index, as carried in `ListNew`/`MapNew` operands and
    /// in an `ImageType::Collection`.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A function index (also the final container index; functions keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuncId(pub(crate) u16);

impl FuncId {
    /// The raw function index, as carried in a `Call` operand and an export.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A durable operation-site index (also the final container index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteId(pub(crate) u16);

impl SiteId {
    /// The raw site index, as carried in a `Dur*` operand.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// One record field. Its type is a bare (non-optional) [`ImageType`]: a scalar
/// for a durable-storable field, or a closed enum (`Option`/`Result`/a user
/// `enum`) for a local-only value field. Sparseness is the field's `required`
/// flag, not an optional wrapper on the type.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: StrId,
    pub ty: ImageType,
    pub required: bool,
}

/// A record type: an ordered field list. Field order is the declaration order.
#[derive(Debug, Clone)]
pub struct RecordTypeDef {
    pub name: StrId,
    pub fields: Vec<FieldDef>,
}

/// One enum variant: a member name, a `category` flag reserving the hierarchy
/// seam (always a leaf on the current flat line — the checker rejects category
/// members), and its ordered dense payload (empty for a payloadless member). Each
/// payload leaf is a bare (non-optional) [`ImageType`]: a user `enum` member
/// carries only bare scalars, while a built-in `Option`/`Result` instantiation
/// carries whatever concrete type its argument monomorphized to (a scalar, a
/// record, or another enum). Payload order is the declaration order — the
/// canonical product-leaf order the checker owns.
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: StrId,
    pub category: bool,
    pub payload: Vec<ImageType>,
}

/// A closed enum type: an ordered variant list in declaration order.
#[derive(Debug, Clone)]
pub struct EnumTypeDef {
    pub name: StrId,
    pub variants: Vec<VariantDef>,
}

/// One collection value type: a finite `List<T>` or ordered `Map<K, V>`. The
/// element/key/value types are bare (non-optional) [`ImageType`]s and may
/// themselves be `Collection` references, so a nested collection reaches its inner
/// shape through the COLLTYPES table. A `Map` key is a bare scalar key type
/// (`int`/`bool`/`string`/`bytes`; a nominal key is int-shaped), the one durable-key
/// scalar family the ordered map compares over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionTypeDef {
    List { elem: ImageType },
    Map { key: ImageType, value: ImageType },
}

/// The ledger identity block of a durable root: the entropy-minted ids of the
/// placement and the stored product, plus the resource's durable **member tree**
/// (its top-level fields interleaved with any static `group` namespaces and keyed
/// `branch` placements, each recursively holding its own members). Each key
/// column's id travels with its scalar in [`KeyColumn`]. The contract id is
/// computed over these, so a rename — which moves a ledger anchor while its id
/// stays — preserves the durable identity.
#[derive(Debug, Clone)]
pub struct RootIdentity {
    pub placement: LedgerIdBytes,
    pub product: LedgerIdBytes,
    pub members: Vec<DurableMemberDef>,
    /// The root's narrow compiler-maintained managed indexes, in source declaration
    /// order. Each projects an ordered leaf reference set from this root; it stores
    /// no data of its own and contributes only its identity and projection to the
    /// durable contract.
    pub indexes: Vec<DurableIndexShape>,
}

/// One member of a durable resource's shape as the draft carries it, in source
/// declaration order: a stored field (its ledger id, required flag, and value shape
/// from the closed acyclic durable value set), a static `group` field-path
/// namespace, or a keyed `branch` placement. Groups and branches recurse. This is
/// the image-side owner of the durable member tree; the verifier decodes an
/// independent copy and the contract id is computed over both.
#[derive(Debug, Clone)]
pub enum DurableMemberDef {
    Field {
        id: LedgerIdBytes,
        required: bool,
        value: DurableValueShape,
    },
    Group {
        id: LedgerIdBytes,
        members: Vec<DurableMemberDef>,
    },
    Branch {
        placement: LedgerIdBytes,
        /// The branch's source name, interned for the physical layer (which keys a
        /// branch family by name at this stage) and for its qualified constructor
        /// spelling. Like a root's name, it is carried for the surface and is not part
        /// of the durable identity — a rename preserves the contract id.
        name: StrId,
        /// The branch entry's materialized record type: its own direct scalar fields,
        /// named and ordered like the root's record. A whole branch-entry read yields
        /// this record and a create/replace supplies it. Carried for the surface, not
        /// the identity (the member tree's field value shapes carry the identity).
        record: TypeId,
        keys: Vec<KeyColumn>,
        members: Vec<DurableMemberDef>,
    },
}

/// One key column of a durable root or branch placement: its orderable durable-key
/// scalar and the entropy-minted ledger id anchored at `<placement>.<column>`.
/// Column order is the declared tuple order and is part of the durable identity.
#[derive(Debug, Clone)]
pub struct KeyColumn {
    pub scalar: Scalar,
    pub id: LedgerIdBytes,
}

/// A durable root placement of a record type, plus its ledger identity block. A
/// singleton root has an empty key tuple; a keyed root has one or more ordered
/// [`KeyColumn`]s drawn from the closed orderable durable-key scalar set.
#[derive(Debug, Clone)]
pub struct RootDef {
    pub name: StrId,
    pub keys: Vec<KeyColumn>,
    pub record: TypeId,
    pub identity: RootIdentity,
}

/// A durable operation site: the [`SemanticPath`] of the graph node it addresses
/// plus the closed [`SemanticTarget`] kind it performs there. The path — not a
/// container-table index — names the node, so the site follows the durable graph's
/// ledger ids; the verifier resolves the path against its own reconstructed node
/// set and rejects a path that names no node or whose target kind disagrees with the
/// resolved node's kind.
#[derive(Debug, Clone)]
pub struct SiteDef {
    pub path: SemanticPath,
    pub target: SemanticTarget,
}

impl SiteDef {
    /// A whole-payload site over the keyed placement `path` names.
    pub fn whole_payload(path: SemanticPath) -> Self {
        Self {
            path,
            target: SemanticTarget::WholePayload,
        }
    }

    /// A field-leaf site over the stored field `path` names.
    pub fn field_leaf(path: SemanticPath) -> Self {
        Self {
            path,
            target: SemanticTarget::FieldLeaf,
        }
    }

    /// A whole-group site over the unkeyed `group` node `path` names.
    pub fn group_entry(path: SemanticPath) -> Self {
        Self {
            path,
            target: SemanticTarget::GroupEntry,
        }
    }

    /// A nonunique progressive-prefix read site over the managed index `path` names.
    pub fn index_scan(path: SemanticPath) -> Self {
        Self {
            path,
            target: SemanticTarget::IndexScan,
        }
    }

    /// A unique complete-key exact-read site over the managed index `path` names.
    pub fn index_lookup(path: SemanticPath) -> Self {
        Self {
            path,
            target: SemanticTarget::IndexLookup,
        }
    }
}

/// A source-position mapping for one instruction. The encoder converts the
/// instruction index to its container byte offset.
#[derive(Debug, Clone)]
pub struct SpanEntry {
    pub instr_index: u32,
    pub line: u32,
    pub column: u32,
}

/// A function body.
#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: StrId,
    pub source: StrId,
    /// Parameter types in declaration order. Each is a bare scalar or a bare record
    /// (a dense `struct` value); the verifier rechecks the same restriction.
    pub params: Vec<ImageType>,
    pub ret: ImageType,
    /// Total local slots, including params (which occupy slots `0..params.len()`).
    pub local_count: u16,
    pub code: Vec<Instr>,
    pub spans: Vec<SpanEntry>,
}

/// An export: a stable [`ExportId`] bound to a function. The image carries the id,
/// never the source name — the VM looks an export up by its verified id, so no
/// human-readable name crosses the trust boundary.
#[derive(Debug, Clone)]
struct ExportDef {
    id: ExportId,
    func: FuncId,
}

/// A test entry: a report-name string bound to a storeless zero-argument function
/// `marrow test` runs. Unlike an export it carries no wire identity — the name is a
/// human report label only, never an interface, demand, or durable identity.
#[derive(Debug, Clone)]
struct TestEntryDef {
    name: StrId,
    func: FuncId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ConstValue {
    Int(i64),
    Bool(bool),
    Text(StrId),
    /// A temporal scalar folded from a compile-time-validated canonical text
    /// literal: a `date` (days since the Unix epoch), an `instant` (signed
    /// nanoseconds since the epoch), or a `duration` (signed nanoseconds). The raw
    /// scalar is stored directly, so the runtime loads it without re-parsing text.
    Date(i32),
    Instant(i128),
    Duration(i128),
}

/// A failure to build a well-formed draft: a §E bound exceeded or an invalid
/// cross-reference. These are producer-side (compiler) faults, not artifact
/// rejections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageBuildError {
    TooManyStrings,
    StringTooLong,
    TooManyConsts,
    TooManyTypes,
    TooManyFields,
    TooManyStructLeaves,
    TooManyEnums,
    TooManyVariants,
    TooManyPayloadFields,
    TooManyCollections,
    TooManyRoots,
    TooManyIndexes,
    TooManyIndexComponents,
    TooManyKeyColumns,
    TooManyDurableMembers,
    DurableTreeTooDeep,
    DurableValueTooDeep,
    TooManySites,
    SitePathTooDeep,
    SitePathTooShort,
    TooManyFunctions,
    TooManyParams,
    TooManyLocals,
    TooManyExports,
    TooManyTestEntries,
    CodeTooLong,
    LocalCountBelowParams,
    ImageTooLarge,
    InvalidReference(&'static str),
}

impl std::fmt::Display for ImageBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "image build error: {self:?}")
    }
}

impl std::error::Error for ImageBuildError {}

/// The mutable image builder.
///
/// The once-checked generic template pass appends its throwaway image directly to the real
/// draft inside a [`DraftSavepoint`], rewinding it on exit (see [`Self::savepoint`] /
/// [`Self::rewind_to`]); the `Clone` derive is used only by tests that need an independent
/// copy of a built draft.
#[derive(Debug, Default, Clone)]
pub struct ImageDraft {
    strings: Vec<String>,
    consts: Vec<ConstValue>,
    types: Vec<RecordTypeDef>,
    enums: Vec<EnumTypeDef>,
    colls: Vec<CollectionTypeDef>,
    roots: Vec<RootDef>,
    application: Option<LedgerIdBytes>,
    sites: Vec<SiteDef>,
    /// Deduplicating index for lazily-allocated operation sites, keyed by the addressed
    /// node's entropy-minted ledger id and the operation target. The `sites` vector stays
    /// the sole site-table authority and emission order; this index only lets a repeated
    /// reference to one `(node, target)` return the site already minted, so the table
    /// carries a site per *referenced* node rather than one per declared graph node. The
    /// node id is globally unique (the verifier enforces pairwise-distinct ledger ids), so
    /// the key identifies exactly one site.
    site_index: HashMap<(LedgerIdBytes, SemanticTarget), SiteId>,
    functions: Vec<FunctionDef>,
    exports: Vec<ExportDef>,
    test_entries: Vec<TestEntryDef>,
}

/// The append-only lengths of every [`ImageDraft`] owner at one instant, plus the
/// application-identity slot. Recorded before an isolated generic-template proof pass and
/// consumed by [`ImageDraft::rewind_to`] to erase everything that pass appended. A proof
/// pass only appends new entries and fills freshly-reserved records/enums (never a
/// pre-existing index), so truncating each owner back to its recorded length restores the
/// draft to the exact bytes it held before the pass.
#[derive(Clone)]
#[must_use]
pub struct DraftSavepoint {
    strings: usize,
    consts: usize,
    types: usize,
    enums: usize,
    colls: usize,
    roots: usize,
    sites: usize,
    functions: usize,
    exports: usize,
    test_entries: usize,
    application: Option<LedgerIdBytes>,
}

impl ImageDraft {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a string, returning its logical id. Repeated interning of the same
    /// text returns the same id.
    pub fn intern_string(&mut self, text: &str) -> StrId {
        if let Some(index) = self.strings.iter().position(|existing| existing == text) {
            return StrId(index as u16);
        }
        let id = StrId(self.strings.len() as u16);
        self.strings.push(text.to_string());
        id
    }

    pub fn intern_int(&mut self, value: i64) -> ConstId {
        self.intern_const(ConstValue::Int(value))
    }

    pub fn intern_bool(&mut self, value: bool) -> ConstId {
        self.intern_const(ConstValue::Bool(value))
    }

    /// Intern a text constant, interning its backing string as needed.
    pub fn intern_text(&mut self, text: &str) -> ConstId {
        let str_id = self.intern_string(text);
        self.intern_const(ConstValue::Text(str_id))
    }

    /// Intern a `date` constant (days since the Unix epoch).
    pub fn intern_date(&mut self, days: i32) -> ConstId {
        self.intern_const(ConstValue::Date(days))
    }

    /// Intern an `instant` constant (signed nanoseconds since the epoch).
    pub fn intern_instant(&mut self, nanos: i128) -> ConstId {
        self.intern_const(ConstValue::Instant(nanos))
    }

    /// Intern a `duration` constant (signed nanoseconds).
    pub fn intern_duration(&mut self, nanos: i128) -> ConstId {
        self.intern_const(ConstValue::Duration(nanos))
    }

    fn intern_const(&mut self, value: ConstValue) -> ConstId {
        if let Some(index) = self
            .consts
            .iter()
            .position(|existing| const_eq(*existing, value))
        {
            return ConstId(index as u16);
        }
        let id = ConstId(self.consts.len() as u16);
        self.consts.push(value);
        id
    }

    pub fn add_record_type(&mut self, def: RecordTypeDef) -> TypeId {
        let id = TypeId(self.types.len() as u16);
        self.types.push(def);
        id
    }

    pub fn add_enum_type(&mut self, def: EnumTypeDef) -> EnumId {
        let id = EnumId(self.enums.len() as u16);
        self.enums.push(def);
        id
    }

    /// The number of collection types already appended to this draft.
    pub fn collection_type_count(&self) -> usize {
        self.colls.len()
    }

    /// Add a collection type (a concrete `List`/`Map` instantiation), returning its
    /// index. Unlike records and enums a collection type has no forward-reference
    /// need — its element/key/value types are already-resolved [`ImageType`]s — so
    /// there is no two-pass reserve/fill; the caller interns the inner types first.
    ///
    /// This appends unconditionally rather than deduplicating by image content: the
    /// compiler's type registry is the single owner of collection instantiation
    /// identity and dedups by the *source* element/key/value types (so `List[Age]`
    /// and `List[int]` stay distinct even though a nominal element erases to the same
    /// image `int`), minting one row here per distinct source instantiation.
    pub fn add_collection_type(&mut self, def: CollectionTypeDef) -> CollTypeId {
        let id = CollTypeId(self.colls.len() as u16);
        self.colls.push(def);
        id
    }

    /// Replace the fields of an already-reserved record type. Value types are built
    /// in two passes — every record and enum reserves its type index first so a
    /// field or payload may reference any other value type (including a mutually
    /// referential `struct`), then each definition's fields are filled in. Only the
    /// reserving pass may set fields; the index is fixed at reservation.
    pub fn set_record_fields(&mut self, ty: TypeId, fields: Vec<FieldDef>) {
        self.types[ty.0 as usize].fields = fields;
    }

    /// Replace the variants of an already-reserved enum type (see
    /// [`Self::set_record_fields`] for the two-pass rationale).
    pub fn set_enum_variants(&mut self, id: EnumId, variants: Vec<VariantDef>) {
        self.enums[id.0 as usize].variants = variants;
    }

    /// Append a durable root and return its DURABLE-table index (its declaration-ordered
    /// RootId). The index is the discriminant an entry identity `Id(^root)` carries.
    pub fn add_root(&mut self, def: RootDef) -> u16 {
        let index = self.roots.len() as u16;
        self.roots.push(def);
        index
    }

    /// Record the application's ledger id. Required exactly when the draft has a
    /// durable root — the durable graph's identity is anchored to it; a storeless
    /// image carries none.
    pub fn set_application_identity(&mut self, id: LedgerIdBytes) {
        self.application = Some(id);
    }

    pub fn add_site(&mut self, def: SiteDef) -> SiteId {
        let id = SiteId(self.sites.len() as u16);
        self.sites.push(def);
        id
    }

    /// Mint-or-return the operation site for `def`, deduplicated by the addressed node's
    /// ledger id and target. The first reference to a `(node, target)` appends a site; a
    /// later reference to the same one returns the existing index. Field-leaf sites are
    /// allocated this way at lowering time, so the site table carries a leaf site per
    /// *referenced* field rather than one per declared field. Non-deduplicated bounded
    /// sites (whole-payload, group-entry, index) are emitted eagerly through
    /// [`Self::add_site`]; they are unique per graph node and never re-minted.
    pub fn alloc_site(&mut self, def: SiteDef) -> SiteId {
        let key = (def.path.node_id(), def.target);
        if let Some(existing) = self.site_index.get(&key) {
            return *existing;
        }
        let id = self.add_site(def);
        self.site_index.insert(key, id);
        id
    }

    pub fn add_function(&mut self, def: FunctionDef) -> FuncId {
        let id = FuncId(self.functions.len() as u16);
        self.functions.push(def);
        id
    }

    /// Bind the export identity `id` to function `func`. The compiler mints `id`
    /// with [`ExportId::of_local`] from the export's declaration path; at v0 each
    /// public function is one export, so `func` is unique across the table.
    pub fn add_export(&mut self, id: ExportId, func: FuncId) {
        self.exports.push(ExportDef { id, func });
    }

    /// Bind the report name `name` to the storeless test function `func`. Test names
    /// are unique across the project (the compiler rejects a duplicate), so the
    /// encoder sorts entries by their final name-string index.
    pub fn add_test_entry(&mut self, name: StrId, func: FuncId) {
        self.test_entries.push(TestEntryDef { name, func });
    }

    /// The number of record types (image `TypeId` ceiling) currently reserved.
    pub fn record_type_count(&self) -> usize {
        self.types.len()
    }

    /// The number of enum types (image `EnumId` ceiling) currently reserved.
    pub fn enum_type_count(&self) -> usize {
        self.enums.len()
    }

    /// Capture the current append-only lengths so a later [`Self::rewind_to`] can discard
    /// every entry appended after this point. See [`DraftSavepoint`].
    pub fn savepoint(&self) -> DraftSavepoint {
        DraftSavepoint {
            strings: self.strings.len(),
            consts: self.consts.len(),
            types: self.types.len(),
            enums: self.enums.len(),
            colls: self.colls.len(),
            roots: self.roots.len(),
            sites: self.sites.len(),
            functions: self.functions.len(),
            exports: self.exports.len(),
            test_entries: self.test_entries.len(),
            application: self.application,
        }
    }

    /// Discard every entry appended since `savepoint`, restoring the draft to its recorded
    /// state. Each owner is truncated back to its recorded length; the `site_index`
    /// secondary map drops exactly the keys of the truncated sites (reconstructed from the
    /// removed rows, so the purge is proportional to the appended sites, never the whole
    /// table). The append-only discipline of the proof pass — reserve-then-fill confined to
    /// freshly-appended indices — makes this a total inverse.
    pub fn rewind_to(&mut self, savepoint: DraftSavepoint) {
        for site in self.sites.drain(savepoint.sites..) {
            self.site_index.remove(&(site.path.node_id(), site.target));
        }
        self.strings.truncate(savepoint.strings);
        self.consts.truncate(savepoint.consts);
        self.types.truncate(savepoint.types);
        self.enums.truncate(savepoint.enums);
        self.colls.truncate(savepoint.colls);
        self.roots.truncate(savepoint.roots);
        self.functions.truncate(savepoint.functions);
        self.exports.truncate(savepoint.exports);
        self.test_entries.truncate(savepoint.test_entries);
        self.application = savepoint.application;
    }

    // --- accessors used by the encoder ---
    pub(crate) fn strings(&self) -> &[String] {
        &self.strings
    }
    pub(crate) fn consts(&self) -> &[ConstValue] {
        &self.consts
    }
    pub(crate) fn types(&self) -> &[RecordTypeDef] {
        &self.types
    }
    pub(crate) fn enums(&self) -> &[EnumTypeDef] {
        &self.enums
    }
    pub(crate) fn collections(&self) -> &[CollectionTypeDef] {
        &self.colls
    }
    pub(crate) fn roots(&self) -> &[RootDef] {
        &self.roots
    }
    pub(crate) fn application_identity(&self) -> Option<LedgerIdBytes> {
        self.application
    }
    pub(crate) fn sites(&self) -> &[SiteDef] {
        &self.sites
    }
    pub(crate) fn functions(&self) -> &[FunctionDef] {
        &self.functions
    }

    /// The `(ExportId, function-index)` pairs for the export table.
    pub(crate) fn export_entries(&self) -> Vec<(ExportId, u16)> {
        self.exports
            .iter()
            .map(|export| (export.id, export.func.0))
            .collect()
    }

    /// The `(raw name StrId, function-index)` pairs for the TEST-ENTRY table. The
    /// encoder remaps the name index through the string sort map and sorts.
    pub(crate) fn test_entry_rows(&self) -> Vec<(u16, u16)> {
        self.test_entries
            .iter()
            .map(|entry| (entry.name.raw(), entry.func.0))
            .collect()
    }
}

fn const_eq(a: ConstValue, b: ConstValue) -> bool {
    match (a, b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => x == y,
        (ConstValue::Bool(x), ConstValue::Bool(y)) => x == y,
        (ConstValue::Text(x), ConstValue::Text(y)) => x.0 == y.0,
        (ConstValue::Date(x), ConstValue::Date(y)) => x == y,
        (ConstValue::Instant(x), ConstValue::Instant(y)) => x == y,
        (ConstValue::Duration(x), ConstValue::Duration(y)) => x == y,
        _ => false,
    }
}

impl StrId {
    pub(crate) fn raw(self) -> u16 {
        self.0
    }
}

impl ConstValue {
    /// A sort key `(tag, payload-bytes)` where the Text payload is the *final*
    /// string index resolved through `str_map`.
    pub(crate) fn sort_key(self, str_map: &[u16]) -> (u8, Vec<u8>) {
        match self {
            ConstValue::Int(v) => (0x01, v.to_be_bytes().to_vec()),
            ConstValue::Bool(v) => (0x02, vec![u8::from(v)]),
            ConstValue::Text(s) => (0x03, str_map[s.0 as usize].to_be_bytes().to_vec()),
            ConstValue::Date(v) => (0x04, v.to_be_bytes().to_vec()),
            ConstValue::Instant(v) => (0x05, v.to_be_bytes().to_vec()),
            ConstValue::Duration(v) => (0x06, v.to_be_bytes().to_vec()),
        }
    }
}

#[cfg(test)]
mod collection_count_tests {
    use super::{CollectionTypeDef, ImageDraft};
    use crate::{ImageType, Scalar};

    #[test]
    fn collection_type_count_tracks_the_next_published_id() {
        let mut draft = ImageDraft::new();
        assert_eq!(draft.collection_type_count(), 0);

        let list = draft.add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        });
        assert_eq!(list.index(), 0);
        assert_eq!(draft.collection_type_count(), 1);

        let map = draft.add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Text),
            value: ImageType::scalar(Scalar::Bool),
        });
        assert_eq!(map.index(), 1);
        assert_eq!(draft.collection_type_count(), 2);
    }
}
