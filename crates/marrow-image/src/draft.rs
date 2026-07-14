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

use crate::instr::Instr;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeId(pub(crate) u16);

impl TypeId {
    /// The raw record-type index, as carried in a `RecordNew`/`FieldGet` operand and
    /// in an `ImageType::Record`.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A function index (also the final container index; functions keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuncId(pub(crate) u16);

/// A durable operation-site index (also the final container index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteId(pub(crate) u16);

/// One record field.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: StrId,
    pub ty: Scalar,
    pub required: bool,
}

/// A record type: an ordered field list. Field order is the declaration order.
#[derive(Debug, Clone)]
pub struct RecordTypeDef {
    pub name: StrId,
    pub fields: Vec<FieldDef>,
}

/// A durable root: one keyed placement of a record type.
#[derive(Debug, Clone)]
pub struct RootDef {
    pub name: StrId,
    /// The key column type (`Int` or `Text` at v0).
    pub key: Scalar,
    pub record: TypeId,
}

/// What an operation site addresses within its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteTarget {
    Entry,
    /// A field of the root's record, by field index.
    Field(u16),
}

/// A durable operation site: a root plus an entry-or-field target.
#[derive(Debug, Clone)]
pub struct SiteDef {
    pub root: u16,
    pub target: SiteTarget,
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
    pub params: Vec<Scalar>,
    pub ret: ImageType,
    /// Total local slots, including params (which occupy slots `0..params.len()`).
    pub local_count: u16,
    pub code: Vec<Instr>,
    pub spans: Vec<SpanEntry>,
}

/// An export: a public name bound to a function.
#[derive(Debug, Clone)]
struct ExportDef {
    name: StrId,
    func: FuncId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ConstValue {
    Int(i64),
    Bool(bool),
    Text(StrId),
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
    TooManyRoots,
    TooManySites,
    TooManyFunctions,
    TooManyParams,
    TooManyLocals,
    TooManyExports,
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
#[derive(Debug, Default)]
pub struct ImageDraft {
    strings: Vec<String>,
    consts: Vec<ConstValue>,
    types: Vec<RecordTypeDef>,
    roots: Vec<RootDef>,
    sites: Vec<SiteDef>,
    functions: Vec<FunctionDef>,
    exports: Vec<ExportDef>,
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

    pub fn add_root(&mut self, def: RootDef) {
        self.roots.push(def);
    }

    pub fn add_site(&mut self, def: SiteDef) -> SiteId {
        let id = SiteId(self.sites.len() as u16);
        self.sites.push(def);
        id
    }

    pub fn add_function(&mut self, def: FunctionDef) -> FuncId {
        let id = FuncId(self.functions.len() as u16);
        self.functions.push(def);
        id
    }

    pub fn add_export(&mut self, name: StrId, func: FuncId) {
        self.exports.push(ExportDef { name, func });
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
    pub(crate) fn roots(&self) -> &[RootDef] {
        &self.roots
    }
    pub(crate) fn sites(&self) -> &[SiteDef] {
        &self.sites
    }
    pub(crate) fn functions(&self) -> &[FunctionDef] {
        &self.functions
    }

    /// The `(name-str-id, function-index)` pairs for the export table.
    pub(crate) fn export_pairs(&self) -> Vec<(u16, u16)> {
        self.exports
            .iter()
            .map(|export| (export.name.0, export.func.0))
            .collect()
    }
}

fn const_eq(a: ConstValue, b: ConstValue) -> bool {
    match (a, b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => x == y,
        (ConstValue::Bool(x), ConstValue::Bool(y)) => x == y,
        (ConstValue::Text(x), ConstValue::Text(y)) => x.0 == y.0,
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
        }
    }
}
