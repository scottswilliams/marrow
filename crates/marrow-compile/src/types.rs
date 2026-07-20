//! The project named-type registry: transparent aliases and the record type.
//!
//! This is the single owner of what a source type name denotes. A transparent
//! `alias Name = Type` denotes exactly its expansion — it mints no identity and
//! no constructor — so every annotation classification calls [`TypeRegistry::expand`]
//! before reading the spelling. A nominal `type Name: int in lo..hi` mints a
//! distinct type: the registry owns its identity — name, inclusive interval, and
//! `supports` capability set — while the image records only its base scalar, so
//! the interval is carried by the guard instructions the compiler emits, not by
//! an image type table. Two product kinds lower into image [`RecordTypeDef`]s,
//! the single canonical product-leaf order owner: the optional `resource` (a record
//! with required and sparse scalar, nominal, dense-struct, or closed-enum fields plus
//! materialized unkeyed groups) and any number of dense `struct` value types (every
//! field required, non-durable, constructible and read by value). Keyed resource
//! children belong to the durable graph rather than this record. Value types are built
//! declare-then-fill so a field may name any other value type regardless of order; the
//! sole nesting restriction is acyclicity.

#[cfg(test)]
use std::cell::Cell;
use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use marrow_codes::Code;
use marrow_image::{
    CollectionTypeDef, EnumId, EnumTypeDef, FieldDef, ImageDraft, ImageType, RecordTypeDef, Scalar,
    TypeId, VariantDef,
};
use marrow_syntax::{
    AliasDecl, EnumDecl, EnumMember, Expression, GroupDecl, LiteralKind, NominalDecl, ResourceDecl,
    ResourceMember, SourceSpan, StructDecl, TypeExpr, UnaryOp, range_expr,
};

use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;

/// The identity of a nominal type in [`TypeRegistry`] order, carried by the
/// lowered type so classification never re-reads the source spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NominalId(pub(crate) u16);

/// A concrete bare (non-optional) value type used as a `Option`/`Result` type
/// argument. Monomorphization keys an instantiation on the exact argument types,
/// so `Option[int]` and `Option[string]` are distinct instantiations, and
/// `Option[Option[int]]` nests through the [`GArg::Enum`] case. A resource record
/// is not a value type, so it is not a representable argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GArg {
    Scalar(ScalarType),
    Nominal(NominalId),
    Struct(TypeId),
    /// An unkeyed `group` namespace materialized as a nested sub-record value by its
    /// image record-type index. A group is a value unit — read, assigned, and copied
    /// whole — whose leaves carry their own required/sparse flags; unlike a
    /// [`Struct`](GArg::Struct) it is not a user-nameable value type and it admits
    /// sparse leaves. It shares the image [`RecordTypeDef`] representation, so it
    /// erases to [`ImageType::Record`] like a struct.
    Group(TypeId),
    Enum(EnumId),
    /// A finite collection value (`List<T>` / `Map<K, V>`) by its image COLLTYPES
    /// index. The element/key/value source types live in the registry's collection
    /// table (`CollSpec`), so a nested collection or a nominal element keeps its
    /// source identity even though the image erases a nominal element to `int`.
    Collection(u16),
    /// An abstract generic type parameter by its declaration index, present only
    /// during the once-checked template pass of a generic function. A monomorphized
    /// instantiation carries no `Param`: every parameter is substituted by its
    /// concrete argument first. `image()` returns a sentinel that only ever reaches
    /// the throwaway draft the template pass discards.
    Param(u16),
}

impl GArg {
    /// The image type this argument monomorphizes to as an enum payload leaf or a
    /// record field. A nominal erases to its base `int` (its interval is carried by
    /// guards, not the image), matching how a nominal is recorded everywhere else.
    pub(crate) fn image(self) -> ImageType {
        match self {
            GArg::Scalar(scalar) => ImageType::scalar(scalar.image()),
            GArg::Nominal(_) => ImageType::scalar(Scalar::Int),
            GArg::Struct(ty) | GArg::Group(ty) => ImageType::Record {
                idx: ty.index(),
                optional: false,
            },
            GArg::Enum(id) => ImageType::Enum {
                idx: id.index(),
                optional: false,
            },
            GArg::Collection(idx) => ImageType::Collection {
                idx,
                optional: false,
            },
            // A `Param` only exists inside the discarded template-check draft; the
            // sentinel keeps that throwaway image well-formed and is never encoded
            // or run. A real image carries the substituted concrete type instead.
            GArg::Param(_) => ImageType::scalar(Scalar::Int),
        }
    }

    /// Whether a concrete argument supports the given generic constraint, checked
    /// at every application of a constrained generic. The equality domain is every
    /// type the `==`/`!=` operator admits (scalar, nominal, enum); the order domain
    /// is every type the `<`/`>` operators admit (`int`/`text`/`bytes`/`date`/
    /// `instant`/`duration` and nominal int). A struct or collection supports
    /// neither; `bool` and an enum support equality but not order. `Param` never
    /// reaches a concrete revalidation.
    pub(crate) fn satisfies(self, constraint: TypeConstraint) -> bool {
        match constraint {
            TypeConstraint::Equality => {
                matches!(self, GArg::Scalar(_) | GArg::Nominal(_) | GArg::Enum(_))
            }
            TypeConstraint::Order => matches!(
                self,
                GArg::Scalar(
                    ScalarType::Int
                        | ScalarType::Text
                        | ScalarType::Bytes
                        | ScalarType::Date
                        | ScalarType::Instant
                        | ScalarType::Duration
                ) | GArg::Nominal(_)
            ),
        }
    }
}

/// The closed generic type-parameter constraint set, mirroring
/// [`marrow_syntax::TypeConstraint`] as a checker-owned fact. `Order` also licenses
/// equality (every orderable type compares for equality), so an order-constrained
/// parameter admits `==` as well as `<`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeConstraint {
    Equality,
    Order,
}

impl TypeConstraint {
    pub(crate) fn from_syntax(constraint: marrow_syntax::TypeConstraint) -> Self {
        match constraint {
            marrow_syntax::TypeConstraint::Equality => TypeConstraint::Equality,
            marrow_syntax::TypeConstraint::Order => TypeConstraint::Order,
        }
    }

    /// Whether this constraint licenses `==`/`!=` over the parameter.
    pub(crate) fn admits_equality(self) -> bool {
        matches!(self, TypeConstraint::Equality | TypeConstraint::Order)
    }

    /// Whether this constraint licenses `<`/`<=`/`>`/`>=` over the parameter.
    pub(crate) fn admits_order(self) -> bool {
        matches!(self, TypeConstraint::Order)
    }

    pub(crate) fn spelling(self) -> &'static str {
        match self {
            TypeConstraint::Equality => "equality",
            TypeConstraint::Order => "order",
        }
    }
}

/// One concrete collection instantiation, keyed by the *source* element/key/value
/// types so `List[Age]` and `List[int]` stay distinct even though both erase to the
/// same image. The registry's collection table indexes these in the same order the
/// image COLLTYPES table records them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CollSpec {
    List { elem: GArg },
    Map { key: GArg, value: GArg },
}

impl CollSpec {
    fn kind(self) -> CollectionKind {
        match self {
            Self::List { .. } => CollectionKind::List,
            Self::Map { .. } => CollectionKind::Map,
        }
    }

    fn definition(self) -> CollectionTypeDef {
        match self {
            Self::List { elem } => CollectionTypeDef::List { elem: elem.image() },
            Self::Map { key, value } => CollectionTypeDef::Map {
                key: key.image(),
                value: value.image(),
            },
        }
    }
}

/// The `none`/`some` and `ok`/`err` variant indices, fixed for every `Option` and
/// `Result` instantiation so construction, `match`, and `try` agree on the tag.
/// They follow from the declaration order of the reserved templates' variants.
pub(crate) const OPTION_NONE: u16 = 0;
pub(crate) const OPTION_SOME: u16 = 1;
pub(crate) const RESULT_OK: u16 = 0;
pub(crate) const RESULT_ERR: u16 = 1;

/// The maximum number of distinct generic instantiations (functions and value
/// types together) one program may mint. A well-typed program with an acyclic call
/// and containment graph produces a finite set; this bound (campaign law 9) fails a
/// divergent monomorphization — a generic that recurses into itself over an
/// ever-growing type — with a typed `check.instantiation_limit` before the
/// worklist allocates unboundedly, rather than looping.
pub(crate) const MAX_INSTANTIATIONS: usize = 4096;

/// The maximum nesting depth of generic type instantiation minting. A member of a
/// minted type may itself mint a type, recursing natively; this bound (at the
/// parser's type-nesting limit, so any finite source-shaped nesting fits) stops a
/// divergent chain — a generic type whose field grows the argument at every level —
/// before it can exhaust the native stack, reporting `check.instantiation_limit`.
pub(crate) const MINT_DEPTH_LIMIT: usize = 256;

/// Why resolution of a generic value type could not produce a usable type.
/// `Limit` is diagnosed once by the shared monomorphization owner; `Unsupported`
/// is contextualized by each declaration or lowering consumer at its current site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveRefusal {
    Limit,
    Unsupported,
}

impl ResolveRefusal {
    /// Combine refusals for one provisional row. A terminal shared limit dominates
    /// a contextual unsupported use regardless of discovery or edge order.
    fn join(self, other: Self) -> Self {
        if matches!(self, Self::Limit) || matches!(other, Self::Limit) {
            Self::Limit
        } else {
            Self::Unsupported
        }
    }
}

/// A generic-resolution failure is either a source-semantic refusal or a compiler
/// coherence failure. Only the refusal arm may enter a rejected cache row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveError {
    Refusal(ResolveRefusal),
    Invariant(GenericInvariant),
}

impl From<ResolveRefusal> for ResolveError {
    fn from(refusal: ResolveRefusal) -> Self {
        Self::Refusal(refusal)
    }
}

impl From<GenericInvariant> for ResolveError {
    fn from(invariant: GenericInvariant) -> Self {
        Self::Invariant(invariant)
    }
}

/// Whether a generic value-type template or instantiated body is product- or
/// sum-shaped. This remains compiler-private bookkeeping, not a source type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeInstKind {
    Struct,
    Enum,
}

/// Which compiler-owned collection family participates in a cache/draft mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectionKind {
    List,
    Map,
}

/// Why a proof-clone boundary could not produce an isolated coherent owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProofCloneError {
    UnstableFillState,
    LimitOwnerNotOpen,
}

/// A closed classification of malformed generic-cache bookkeeping. These cases are
/// compiler coherence failures and cannot be contextualized as source Unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericCacheInvariant {
    ActiveBatchMissing,
    ActiveBatchRange,
    ActiveRowCardinality,
    ActiveRowKeyMismatch,
    ActiveFillStackNotEmpty,
    FailureIndexOutOfRange,
    DependentIndexOutOfRange,
    StableRowInActiveBatch,
    IncompleteRowWithoutRefusal,
    FillingReuseOutsideBatch,
    SettledRowMissing,
    SettledRowStillFilling,
    FillStackMismatch,
    /// A lookup-only mint-dedup index (`type_index`/`fn_index`) resolved a
    /// `(template, args)` key to a row that does not carry that key: the secondary
    /// index diverged from its append-order authority vector.
    MintIndexDrift,
}

/// Detailed compiler-private causes that cross the build boundary only through the
/// redacted public `CompileInvariant` wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericInvariant {
    ProofClone(ProofCloneError),
    CacheState(GenericCacheInvariant),
    ReservedTemplateMissing(Reserved),
    TypeTemplateMissing(usize),
    TypeArgumentCountMismatch {
        template: usize,
        expected: usize,
        actual: usize,
    },
    TemplateKindMismatch {
        template: usize,
        expected: TypeInstKind,
        actual: TypeInstKind,
    },
    TypeBodyKindMismatch {
        id: TypeInstId,
        body: TypeInstKind,
    },
    ReadyBodyShapeMismatch(TypeInstId),
    ReadyBodyMissing(TypeInstId),
    ReadyEnumVariantMissing {
        id: EnumId,
        template: usize,
        variant: usize,
    },
    TypeIdentityCollision(TypeInstId),
    TypeInstantiationKeyCollision {
        first: TypeInstId,
        duplicate: TypeInstId,
    },
    TypeArgumentOrderViolation {
        owner: TypeInstId,
        target: TypeInstId,
    },
    TypeArgumentTargetMissing(GArg),
    TypeArgumentParameter(u16),
    CollectionIndexMismatch {
        kind: CollectionKind,
        cache_index: usize,
        draft_index: usize,
    },
}

/// A row position already proven to be relative to the active fill batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FillOffset(usize);

/// One monotone refusal update waiting to traverse reverse dependency edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingRefusal {
    offset: FillOffset,
    refusal: ResolveRefusal,
}

/// Whether `name` is a reserved generic type name the user cannot redeclare. The
/// toolchain owns `Option`/`Result` (as generic enums) and `List`/`Map` (as
/// compiler collections).
pub(crate) fn is_reserved_type_name(name: &str) -> bool {
    matches!(name, "Option" | "Result" | "List" | "Map")
}

/// Which reserved toolchain generic a template is. `Option` and `Result` are
/// ordinary generic enums the toolchain registers through the same instantiation
/// machinery user generic enums use; only their names and constructor spellings
/// (`none`/`some`/`ok`/`err`, prefix `try`) are reserved, so the lowerer recovers
/// them from the minting template rather than a bespoke instantiation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reserved {
    Option,
    Result,
}

/// The closed argument shape of one Ready reserved enum instantiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReservedEnumArgs {
    Option(GArg),
    Result(GArg, GArg),
    Other,
}

#[derive(Clone, Copy)]
pub(crate) enum StaticNamedType {
    Struct(TypeId),
    Enum(EnumId),
    Record(TypeId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProductFieldProjection {
    Field {
        index: u16,
        ty: GArg,
        required: bool,
    },
    Group {
        index: u16,
        ty: TypeId,
    },
    MissingRecordField,
    MissingGroupField,
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructFieldProjection {
    Field { index: u16, ty: GArg },
    Missing,
    Absent,
}

/// One payload leaf of a generic enum template variant: its field name and the
/// type expression it carries (over the template's type parameters).
#[derive(Clone)]
struct TemplatePayload {
    name: String,
    ty: TypeExpr,
}

/// One variant of a generic enum template: its name and named payload fields.
#[derive(Clone)]
struct TemplateVariant {
    name: String,
    payload: Vec<TemplatePayload>,
}

type TemplateVariantPayload = (usize, Vec<(String, TypeExpr)>);
pub(crate) type ResolvedEnumVariants = Vec<(String, Vec<GArg>)>;

/// The member shape of a generic type template: a `struct`'s named fields or an
/// `enum`'s variants, each carried as a type expression over the template's type
/// parameters and substituted at instantiation.
#[derive(Clone)]
enum TemplateBody {
    Struct(Vec<(String, TypeExpr)>),
    Enum(Vec<TemplateVariant>),
}

impl TemplateBody {
    fn kind(&self) -> TypeInstKind {
        match self {
            Self::Struct(_) => TypeInstKind::Struct,
            Self::Enum(_) => TypeInstKind::Enum,
        }
    }
}

/// One generic value-type template: a `struct Name[T, ...]` or `enum Name[T, ...]`
/// (or a reserved toolchain generic), held for lazy monomorphization. A template
/// mints no image index of its own; each distinct `Name<Args>` application mints one
/// through the shared instantiation owner.
#[derive(Clone)]
struct TypeTemplate {
    name: String,
    file: String,
    name_span: SourceSpan,
    reserved: Option<Reserved>,
    type_params: Vec<(String, Option<TypeConstraint>)>,
    body: TemplateBody,
}

impl TypeTemplate {
    fn is_enum(&self) -> bool {
        matches!(self.body, TemplateBody::Enum(_))
    }
}

/// The resolved member shape of one minted type instantiation, read by the lowerer
/// for construction, `match`, field access, and cycle checking without re-resolving
/// the template.
#[derive(Clone)]
pub(crate) enum InstBody {
    Struct(Vec<(String, GArg)>),
    Enum(Vec<InstVariant>),
}

impl InstBody {
    fn kind(&self) -> TypeInstKind {
        match self {
            Self::Struct(_) => TypeInstKind::Struct,
            Self::Enum(_) => TypeInstKind::Enum,
        }
    }
}

/// One resolved variant of a minted enum instantiation: its name and the concrete
/// value types its payload fields carry, in declaration order.
#[derive(Clone)]
pub(crate) struct InstVariant {
    pub(crate) name: String,
    pub(crate) payload: Vec<(String, GArg)>,
}

/// The image index a minted type instantiation occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeInstId {
    Record(TypeId),
    Enum(EnumId),
}

impl TypeInstId {
    fn kind(self) -> TypeInstKind {
        match self {
            Self::Record(_) => TypeInstKind::Struct,
            Self::Enum(_) => TypeInstKind::Enum,
        }
    }
}

/// A generic enum member whose template, arguments, body kind, ordinal, and name
/// have all been checked by the registry owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnumVariantInstance {
    pub(crate) enum_id: EnumId,
    pub(crate) variant: u16,
}

trait ReadyInstanceRequirement: Copy {
    fn allows_provisional(self) -> bool {
        false
    }

    fn validate(self, inst: &TypeInst, body: &InstBody) -> Result<(), GenericInvariant>;
}

#[derive(Clone, Copy)]
struct AnyReadyInstance;

impl ReadyInstanceRequirement for AnyReadyInstance {
    fn allows_provisional(self) -> bool {
        true
    }

    fn validate(self, _inst: &TypeInst, _body: &InstBody) -> Result<(), GenericInvariant> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct StructReadyInstance;

impl ReadyInstanceRequirement for StructReadyInstance {
    fn validate(self, inst: &TypeInst, body: &InstBody) -> Result<(), GenericInvariant> {
        match body {
            InstBody::Struct(_) => Ok(()),
            InstBody::Enum(_) => Err(GenericInvariant::TypeBodyKindMismatch {
                id: inst.id,
                body: TypeInstKind::Enum,
            }),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EnumVariantSelection<'a> {
    pub(crate) index: usize,
    pub(crate) name: &'a str,
}

impl ReadyInstanceRequirement for EnumVariantSelection<'_> {
    fn validate(self, inst: &TypeInst, body: &InstBody) -> Result<(), GenericInvariant> {
        let InstBody::Enum(variants) = body else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id: inst.id,
                body: TypeInstKind::Struct,
            });
        };
        if variants
            .get(self.index)
            .is_some_and(|member| member.name == self.name)
        {
            Ok(())
        } else {
            let TypeInstId::Enum(id) = inst.id else {
                return Err(GenericInvariant::TypeBodyKindMismatch {
                    id: inst.id,
                    body: TypeInstKind::Enum,
                });
            };
            Err(GenericInvariant::ReadyEnumVariantMissing {
                id,
                template: inst.template,
                variant: self.index,
            })
        }
    }
}

/// A sortable active-batch key for a generic type row. Image IDs are insertion
/// ordered within their own record/enum tables; the variant keeps those domains
/// disjoint without searching the stable cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TypeInstKey {
    Record(u16),
    Enum(u16),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TypeInstSemanticKey<'a> {
    template: usize,
    args: &'a [GArg],
}

impl Hash for TypeInstSemanticKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.template.hash(state);
        self.args.len().hash(state);
        for arg in self.args {
            match arg {
                GArg::Scalar(scalar) => {
                    0_u8.hash(state);
                    (*scalar as u8).hash(state);
                }
                GArg::Nominal(id) => {
                    1_u8.hash(state);
                    id.0.hash(state);
                }
                GArg::Struct(id) => {
                    2_u8.hash(state);
                    id.index().hash(state);
                }
                GArg::Group(id) => {
                    3_u8.hash(state);
                    id.index().hash(state);
                }
                GArg::Enum(id) => {
                    4_u8.hash(state);
                    id.index().hash(state);
                }
                GArg::Collection(index) => {
                    5_u8.hash(state);
                    index.hash(state);
                }
                GArg::Param(index) => {
                    6_u8.hash(state);
                    index.hash(state);
                }
            }
        }
    }
}

impl From<TypeInstId> for TypeInstKey {
    fn from(id: TypeInstId) -> Self {
        match id {
            TypeInstId::Record(ty) => Self::Record(ty.index()),
            TypeInstId::Enum(id) => Self::Enum(id.index()),
        }
    }
}

/// One minted generic type instantiation: which template and concrete arguments
/// produced it, the image index it occupies, and its resolved member shape.
#[derive(Clone)]
struct TypeInst {
    template: usize,
    args: Vec<GArg>,
    id: TypeInstId,
    state: TypeInstState,
    /// Provisional rows that semantically refer to this row during the active fill
    /// batch. Empty for every settled row.
    dependents: Vec<usize>,
}

/// A reserved type row is visible to recursive filling before its body is
/// committed, but semantic consumers can observe only `Ready` rows.
#[derive(Clone)]
enum TypeInstState {
    Filling { staged: Option<InstBody> },
    Ready(InstBody),
    Rejected(ResolveRefusal),
}

/// One minted generic function instantiation awaiting body lowering: its function
/// template index (into the lowerer's generic registry), concrete arguments, and
/// the reserved image function index.
#[derive(Clone)]
struct FnInst {
    template: usize,
    args: Vec<GArg>,
    func: u16,
}

/// The source location a type instantiation is minted from, threaded through
/// [`TypeRegistry::mint_type_instance`] so a mint-time rejection — a collection as
/// an enum payload leaf — points at the construction or annotation site rather than
/// the reserved `Option`/`Result` template, which carries no user span. `file` is
/// empty only for a synthetic mint with no source anchor.
#[derive(Clone, Copy)]
pub(crate) struct MintSite<'a> {
    pub(crate) file: &'a str,
    pub(crate) span: SourceSpan,
}

/// The lifecycle of the one terminal instantiation-limit diagnostic. The first
/// refusal owns its source location; taking it advances the owner to `Reported`, so
/// cached `Rejected(Limit)` rows replay without duplicating or relocating it.
#[derive(Default)]
enum LimitState {
    #[default]
    Open,
    Pending(SourceDiagnostic),
    Reported,
}

/// Which argument domain one generic owner may admit. Concrete compilation never
/// carries an abstract parameter into a published image; only the isolated proof
/// clone may use `Param` while checking one generic template body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ArgumentDomain {
    #[default]
    Concrete,
    TemplateProof,
}

/// One owner-ordered transfer from an isolated generic proof pass: the optional
/// terminal limit first, followed by payload diagnostics gathered before it.
#[must_use = "generic diagnostics must be adopted or reported as one ordered outcome"]
pub(crate) struct GenericDiagnostics {
    limit: Option<SourceDiagnostic>,
    payloads: Vec<SourceDiagnostic>,
}

impl GenericDiagnostics {
    /// Consume this transfer in its canonical owner order.
    pub(crate) fn into_ordered(self) -> Vec<SourceDiagnostic> {
        let mut diagnostics =
            Vec::with_capacity(self.payloads.len() + usize::from(self.limit.is_some()));
        if let Some(limit) = self.limit {
            diagnostics.push(limit);
        }
        diagnostics.extend(self.payloads);
        diagnostics
    }
}

/// The single owner of generic instantiation identity across functions and value
/// types. Interior-mutable so a shared `&TypeRegistry` mints instances during field
/// resolution and body lowering. Type instantiations mint their image record/enum
/// eagerly (declare-then-fill, so a self-referential instantiation terminates and
/// the containment-cycle check rejects it); function instantiations reserve an image
/// index and enqueue their body for the driver to drain in mint order. The explicit
/// [`TypeRegistry::clone_for_generic_check`] boundary is the only supported way to
/// copy stable owner state into an isolated proof pass.
#[derive(Default)]
struct Monomorph {
    type_insts: Vec<TypeInst>,
    /// Lookup-only secondary index `(template, args) -> row in type_insts`. The
    /// append-order `type_insts` vector remains the sole authority for instantiation
    /// identity, mint order, and image emission; this index is never iterated to
    /// assign an id, select a diagnostic, drain work, or emit bytes — it only
    /// accelerates the mint-dedup reuse probe from a linear key scan to a keyed
    /// lookup. It is append-only in lockstep with `type_insts`, so a lookup whose row
    /// does not carry the looked-up key is index/authority drift, reported as the
    /// typed coherence failure `MintIndexDrift` rather than silently trusted.
    type_index: HashMap<(usize, Vec<GArg>), usize>,
    fn_base: u16,
    fn_insts: Vec<FnInst>,
    /// Lookup-only secondary index `(template, args) -> row in fn_insts`, with the
    /// same authority discipline and drift detection as `type_index`. `fn_insts`
    /// stays the sole reservation-order authority; the reserved image function index
    /// is always read from the row, never from this index.
    fn_index: HashMap<(usize, Vec<GArg>), usize>,
    fn_queue: VecDeque<FnInst>,
    /// The first row appended by the active outermost fill. Settlement
    /// touches only this contiguous suffix, never the stable prefix.
    fill_batch_start: Option<usize>,
    /// Direct image-id lookup for rows in that active suffix. Cleared atomically at
    /// settlement, so semantic dependency discovery never scans the stable cache.
    fill_rows: BTreeMap<TypeInstKey, usize>,
    /// The active fill stack. Its length is the native recursion depth
    /// bounded by [`MINT_DEPTH_LIMIT`].
    fill_stack: Vec<usize>,
    fill_failures: Vec<(usize, ResolveRefusal)>,
    /// One owner for the shared type/function instantiation limit, kept separate
    /// from ordered collection-payload diagnostics.
    limit: LimitState,
    collection_payloads: Vec<SourceDiagnostic>,
    /// A declare/fill coherence failure discovered while building concrete source
    /// types. Kept inside the defaulted generic owner so private test fixtures cannot
    /// accidentally bypass a newly added top-level registry field.
    build_invariant: Option<GenericInvariant>,
    argument_domain: ArgumentDomain,
}

/// The closed capability set a nominal declaration's `supports` list unlocks.
/// Each flag independently admits operators over the nominal (see the lowerer's
/// operator mapping); construction and `.checked` need no capability.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SupportSet {
    pub(crate) add: bool,
    pub(crate) subtract: bool,
    pub(crate) step: bool,
    pub(crate) scale: bool,
}

/// One nominal type: a distinct int-based type whose every value lies in the
/// inclusive interval `[lo, hi]`.
#[derive(Clone)]
pub(crate) struct NominalInfo {
    pub(crate) name: String,
    pub(crate) lo: i64,
    pub(crate) hi: i64,
    pub(crate) supports: SupportSet,
}

/// One resolved record field, in declaration order. A resource field is a scalar,
/// nominal scalar, dense struct, or closed enum (`Option`/`Result`/a user `enum`);
/// a struct field may additionally use a collection. Nesting is admitted behind the
/// value-graph acyclicity proof.
#[derive(Clone)]
pub(crate) struct FieldInfo {
    pub(crate) name: String,
    pub(crate) ty: GArg,
    pub(crate) required: bool,
}

/// One unkeyed `group` namespace of the resource, materialized as a nested
/// sub-record value. `type_id` is the group's image [`RecordTypeDef`] (a value
/// record, not a durable root); `fields` are the group's direct scalar/enum leaves
/// in declaration order, each carrying its own required/sparse flag. A group value
/// occupies one required slot in the containing record's materialized value.
#[derive(Clone)]
pub(crate) struct GroupInfo {
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) fields: Vec<FieldInfo>,
}

impl GroupInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }
}

/// The project's single record type. `type_id` is the group-inclusive materialized
/// record: its top-level scalar/enum field slots followed by one slot per unkeyed group
/// (a nested group sub-record). The verifier ties the field slots to the durable member
/// tree's fields and each trailing group slot to a `Group` member, so one record type
/// serves both the durable graph and the storeless value model.
#[derive(Clone)]
pub(crate) struct RecordInfo {
    pub(crate) type_id: TypeId,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
    pub(crate) groups: Vec<GroupInfo>,
}

impl RecordInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }

    /// The materialized-record slot of the unkeyed group named `name`, if any. Group
    /// slots follow the top-level fields in `type_id`, so the slot index is the field
    /// count plus the group's declaration ordinal.
    pub(crate) fn group(&self, name: &str) -> Option<(u16, &GroupInfo)> {
        self.groups
            .iter()
            .enumerate()
            .find(|(_, group)| group.name == name)
            .map(|(ordinal, group)| ((self.fields.len() + ordinal) as u16, group))
    }
}

/// One dense product type: a `struct` whose every field is present inline. It
/// shares the image [`RecordTypeDef`] representation with the resource record —
/// the single canonical product-leaf order owner — but is a distinct value type:
/// non-durable, constructed and read by value, every field required. A struct is
/// admitted as a parameter and a return type (carried as an `ImageType::Record`).
#[derive(Clone)]
pub(crate) struct StructInfo {
    pub(crate) type_id: TypeId,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
}

impl StructInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }
}

/// One enum-variant payload leaf: a named scalar carried by that variant, in
/// declaration order. The name is used for named construction; the image records
/// only the scalar.
#[derive(Clone)]
pub(crate) struct EnumPayloadInfo {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
}

/// One selectable enum variant: its member name and dense scalar payload.
#[derive(Clone)]
pub(crate) struct VariantInfo {
    pub(crate) name: String,
    pub(crate) payload: Vec<EnumPayloadInfo>,
}

/// One closed flat enum value type. It lowers to an image [`EnumTypeDef`]; its
/// distinct nominal identity lives here. Hierarchical categories are deferred, so
/// every variant is a selectable leaf.
#[derive(Clone)]
pub(crate) struct EnumInfo {
    pub(crate) enum_id: EnumId,
    pub(crate) name: String,
    pub(crate) variants: Vec<VariantInfo>,
}

impl EnumInfo {
    /// The index and info of the variant named `name` in declaration order.
    pub(crate) fn variant(&self, name: &str) -> Option<(u16, &VariantInfo)> {
        self.variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.name == name)
            .map(|(index, variant)| (index as u16, variant))
    }
}

/// The index and info of the field named `name` in declaration order, shared by
/// the resource record and the dense struct so field lookup has one owner.
fn field_index<'f>(fields: &'f [FieldInfo], name: &str) -> Option<(u16, &'f FieldInfo)> {
    fields
        .iter()
        .enumerate()
        .find(|(_, field)| field.name == name)
        .map(|(index, field)| (index as u16, field))
}

/// The project named-type registry: the transparent aliases, the nominal int
/// types, the dense struct value types, and the durable-capable record types.
#[derive(Default)]
pub(crate) struct TypeRegistry {
    /// `alias name -> alias-free expanded target`. Cyclic aliases are reported
    /// at build and never enter this map.
    aliases: BTreeMap<String, TypeExpr>,
    nominals: Vec<NominalInfo>,
    structs: Vec<StructInfo>,
    enums: Vec<EnumInfo>,
    /// The project's `resource` record types, in source order. Each is a value
    /// record type; at most one backs a durable store this line. Names are unique
    /// (a duplicate is rejected at declare), so a name selects at most one.
    records: Vec<RecordInfo>,
    /// The generic value-type templates: the reserved toolchain generics
    /// (`Option`/`Result`) followed by the user `struct`/`enum` templates. Fixed
    /// after `build`; instantiations reference a template by index.
    type_templates: Vec<TypeTemplate>,
    generics: RefCell<Monomorph>,
    /// The concrete collection instantiations minted so far, in image COLLTYPES
    /// order. Interior-mutable so a shared `&TypeRegistry` can mint one on first use
    /// of a concrete `List`/`Map`, deduping by source element/key/value types.
    collections: RefCell<Vec<CollSpec>>,
}

/// One immutable view of the generic and collection owners for a complete metadata
/// validation walk. Keeping both `Ref`s here prevents recursive reborrowing and
/// guarantees they are dropped before any cache or image mutation.
struct TypeMetadataView<'a> {
    registry: &'a TypeRegistry,
    generics: Ref<'a, Monomorph>,
    collections: Ref<'a, Vec<CollSpec>>,
}

#[derive(Debug, Clone, Copy)]
enum MetadataTask {
    Argument {
        arg: GArg,
        collection_parent: Option<u16>,
        generic_parent: Option<usize>,
    },
    ReadyBody {
        row: usize,
    },
}

/// Dense, validation-local lookup and visitation state. The directory is rebuilt
/// from immutable registry rows for one metadata walk and is dropped before any
/// cache or image mutation; it is not a persistent mint/dedup index.
#[derive(Clone, Copy)]
enum RecordMetadataOwner {
    ResourceRecord(usize),
    DeclaredStruct(usize),
    Group(usize, usize),
    GenericRow(usize),
}

#[derive(Clone, Copy)]
enum EnumMetadataOwner {
    DeclaredEnum(usize),
    GenericRow(usize),
}

#[derive(Clone, Copy)]
struct GenericRowRef {
    row: usize,
    id: TypeInstId,
}

struct MetadataScratch {
    records: Vec<Option<RecordMetadataOwner>>,
    enums: Vec<Option<EnumMetadataOwner>>,
    collection_generic_targets: Vec<Option<GenericRowRef>>,
    seen_rows: Vec<bool>,
    seen_collections: Vec<bool>,
    tasks: Vec<MetadataTask>,
}

/// One immutable registry snapshot and its validation directory. A session is
/// deliberately short-lived: holding it keeps both metadata owners immutably
/// borrowed, so callers must drop it before minting or settling another row.
/// Only owned or copy projections leave the session. Its first invariant poisons
/// every later projection, so partially marked traversal state is never reused.
pub(crate) struct TypeMetadataSession<'a> {
    view: TypeMetadataView<'a>,
    metadata: MetadataScratch,
    display: DisplayScratch,
    failure: Option<GenericInvariant>,
}

/// Active-path marks for best-effort diagnostic spelling. Compiler-owned metadata
/// validation rejects cycles before semantic or durable use; these marks keep the
/// display-only fallback total even when it is asked to render a hostile cache.
struct DisplayScratch {
    active_rows: Vec<u8>,
    active_collections: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayNode {
    Row(usize),
    Collection(u16),
}

impl DisplayScratch {
    fn for_view(view: &TypeMetadataView<'_>) -> Self {
        Self {
            active_rows: vec![0; view.generics.type_insts.len()],
            active_collections: vec![0; view.collections.len()],
        }
    }

    fn enter_row(&mut self, row: usize) -> bool {
        let Some(active) = self.active_rows.get_mut(row) else {
            return false;
        };
        std::mem::replace(active, 1) == 0
    }

    fn leave_row(&mut self, row: usize) {
        let active = &mut self.active_rows[row];
        debug_assert_eq!(*active, 1);
        *active = 0;
    }

    fn enter_collection(&mut self, index: u16) -> bool {
        let Some(active) = self.active_collections.get_mut(index as usize) else {
            return false;
        };
        std::mem::replace(active, 1) == 0
    }

    fn leave_collection(&mut self, index: u16) {
        let active = &mut self.active_collections[index as usize];
        debug_assert_eq!(*active, 1);
        *active = 0;
    }

    fn leave(&mut self, node: DisplayNode) {
        match node {
            DisplayNode::Row(row) => self.leave_row(row),
            DisplayNode::Collection(index) => self.leave_collection(index),
        }
    }
}

#[cfg(test)]
thread_local! {
    static METADATA_DIRECTORY_BUILDS: Cell<usize> = const { Cell::new(0) };
    static READY_BODY_MATCH_VISITS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
struct MetadataBuildCounter {
    previous: usize,
}

#[cfg(test)]
impl Drop for MetadataBuildCounter {
    fn drop(&mut self) {
        METADATA_DIRECTORY_BUILDS.with(|count| count.set(self.previous));
    }
}

/// Observe directory construction in one single-threaded production journey. This
/// test-only counter cannot alter registry state or make a hostile state reachable.
#[cfg(test)]
pub(crate) fn count_metadata_directory_builds<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let previous = METADATA_DIRECTORY_BUILDS.with(|count| count.replace(0));
    let guard = MetadataBuildCounter { previous };
    let result = run();
    let builds = METADATA_DIRECTORY_BUILDS.with(Cell::get);
    drop(guard);
    (result, builds)
}

#[cfg(test)]
struct ReadyBodyMatchCounter {
    previous: usize,
}

#[cfg(test)]
impl Drop for ReadyBodyMatchCounter {
    fn drop(&mut self) {
        READY_BODY_MATCH_VISITS.with(|count| count.set(self.previous));
    }
}

/// Count borrowed template-body matcher frames in one test journey. The counter
/// observes work only; it cannot alter metadata or make a hostile row reachable.
#[cfg(test)]
fn count_ready_body_match_visits<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let previous = READY_BODY_MATCH_VISITS.with(|count| count.replace(0));
    let guard = ReadyBodyMatchCounter { previous };
    let result = run();
    let visits = READY_BODY_MATCH_VISITS.with(Cell::get);
    drop(guard);
    (result, visits)
}

/// Deterministic operation counts for the generic-scaling KATs. Every field counts
/// work performed by one production owner during a single-threaded test journey;
/// the counter observes work only and cannot alter registry state or make a hostile
/// row reachable. It is not a public hook and not a canonical fact.
#[cfg(test)]
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct ScalingCounts {
    /// `MetadataScratch::try_new` invocations (directory builds).
    pub(crate) directory_builds: usize,
    /// Per-type-inst rows visited while building directories: the total directory
    /// work, which is `directory_builds * type_insts.len()` when the directory is
    /// rebuilt per mint.
    pub(crate) directory_row_visits: usize,
    /// Elements examined by the `(template, args)` primary-key scan in
    /// `existing_type_instance` (type-mint reuse).
    pub(crate) type_inst_scan_steps: usize,
    /// Elements examined by the `(template, args)` primary-key scan in
    /// `reserve_fn_instance` (function-mint reuse).
    pub(crate) fn_inst_scan_steps: usize,
    /// Value-graph edges traversed across every `cycle_through` start.
    pub(crate) cycle_walk_steps: usize,
    /// `clone_for_generic_check` invocations (proof forks).
    pub(crate) proof_clones: usize,
    /// Type-inst rows replayed across every proof fork (`proof_clones *
    /// type_insts.len()`).
    pub(crate) proof_clone_rows: usize,
}

#[cfg(test)]
thread_local! {
    static SCALING_COUNTS: Cell<ScalingCounts> = const { Cell::new(ScalingCounts {
        directory_builds: 0,
        directory_row_visits: 0,
        type_inst_scan_steps: 0,
        fn_inst_scan_steps: 0,
        cycle_walk_steps: 0,
        proof_clones: 0,
        proof_clone_rows: 0,
    }) };
}

#[cfg(test)]
fn bump_scaling(update: impl FnOnce(&mut ScalingCounts)) {
    SCALING_COUNTS.with(|cell| {
        let mut counts = cell.get();
        update(&mut counts);
        cell.set(counts);
    });
}

/// Run `run` with a fresh scaling-count window and restore the prior window after,
/// returning its result paired with the deterministic operation counts observed.
#[cfg(test)]
pub(crate) fn capture_scaling_counts<T>(run: impl FnOnce() -> T) -> (T, ScalingCounts) {
    let previous = SCALING_COUNTS.with(|cell| cell.replace(ScalingCounts::default()));
    let result = run();
    let counts = SCALING_COUNTS.with(Cell::get);
    SCALING_COUNTS.with(|cell| cell.set(previous));
    (result, counts)
}

/// One durable-validation walk over all resource leaves. These marks are separate
/// from metadata preflight: preflight may visit a generic argument or collection
/// before the durable walk expands that value's body.
struct DurableMetadataScratch {
    pending: VecDeque<(GArg, usize)>,
    expanded_records: Vec<bool>,
    expanded_enums: Vec<bool>,
    expanded_collections: Vec<bool>,
}

impl DurableMetadataScratch {
    fn new(metadata: &MetadataScratch, roots: Vec<GArg>) -> Self {
        Self {
            pending: roots.into_iter().map(|arg| (arg, 0)).collect(),
            expanded_records: vec![false; metadata.records.len()],
            expanded_enums: vec![false; metadata.enums.len()],
            expanded_collections: vec![false; metadata.seen_collections.len()],
        }
    }

    fn first_record(&mut self, id: TypeId) -> Option<bool> {
        let seen = self.expanded_records.get_mut(id.index() as usize)?;
        Some(!std::mem::replace(seen, true))
    }

    fn first_enum(&mut self, id: EnumId) -> Option<bool> {
        let seen = self.expanded_enums.get_mut(id.index() as usize)?;
        Some(!std::mem::replace(seen, true))
    }

    fn first_collection(&mut self, index: u16) -> Option<bool> {
        let seen = self.expanded_collections.get_mut(index as usize)?;
        Some(!std::mem::replace(seen, true))
    }

    fn push(&mut self, arg: GArg, depth: usize) {
        self.pending.push_back((arg, depth));
    }
}

impl MetadataScratch {
    fn try_new(view: &TypeMetadataView<'_>) -> Result<Self, GenericInvariant> {
        #[cfg(test)]
        METADATA_DIRECTORY_BUILDS.with(|count| count.set(count.get() + 1));
        #[cfg(test)]
        bump_scaling(|counts| counts.directory_builds += 1);
        let mut records = Vec::new();
        let mut enums = Vec::new();
        for (record_row, record) in view.registry.records.iter().enumerate() {
            let index = record.type_id.index() as usize;
            if records.len() <= index {
                records.resize(index + 1, None);
            }
            let slot = &mut records[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
                    record.type_id,
                )));
            }
            *slot = Some(RecordMetadataOwner::ResourceRecord(record_row));
            for (group_row, group) in record.groups.iter().enumerate() {
                let index = group.type_id.index() as usize;
                if records.len() <= index {
                    records.resize(index + 1, None);
                }
                let slot = &mut records[index];
                if slot.is_some() {
                    return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
                        group.type_id,
                    )));
                }
                *slot = Some(RecordMetadataOwner::Group(record_row, group_row));
            }
        }
        for (row, info) in view.registry.structs.iter().enumerate() {
            let index = info.type_id.index() as usize;
            if records.len() <= index {
                records.resize(index + 1, None);
            }
            let slot = &mut records[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
                    info.type_id,
                )));
            }
            *slot = Some(RecordMetadataOwner::DeclaredStruct(row));
        }
        for (row, info) in view.registry.enums.iter().enumerate() {
            let index = info.enum_id.index() as usize;
            if enums.len() <= index {
                enums.resize(index + 1, None);
            }
            let slot = &mut enums[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Enum(
                    info.enum_id,
                )));
            }
            *slot = Some(EnumMetadataOwner::DeclaredEnum(row));
        }
        let mut semantic_keys = HashMap::with_capacity(view.generics.type_insts.len());
        for (row, inst) in view.generics.type_insts.iter().enumerate() {
            #[cfg(test)]
            bump_scaling(|counts| counts.directory_row_visits += 1);
            match inst.id {
                TypeInstId::Record(id) => {
                    let index = id.index() as usize;
                    if records.len() <= index {
                        records.resize(index + 1, None);
                    }
                    let slot = &mut records[index];
                    if slot.is_some() {
                        return Err(GenericInvariant::TypeIdentityCollision(inst.id));
                    }
                    *slot = Some(RecordMetadataOwner::GenericRow(row));
                }
                TypeInstId::Enum(id) => {
                    let index = id.index() as usize;
                    if enums.len() <= index {
                        enums.resize(index + 1, None);
                    }
                    let slot = &mut enums[index];
                    if slot.is_some() {
                        return Err(GenericInvariant::TypeIdentityCollision(inst.id));
                    }
                    *slot = Some(EnumMetadataOwner::GenericRow(row));
                }
            }
            let key = TypeInstSemanticKey {
                template: inst.template,
                args: &inst.args,
            };
            if let Some(first) = semantic_keys.insert(key, inst.id) {
                return Err(GenericInvariant::TypeInstantiationKeyCollision {
                    first,
                    duplicate: inst.id,
                });
            }
        }
        let direct_generic_target = |arg: GArg| match arg {
            GArg::Struct(id) => records
                .get(id.index() as usize)
                .and_then(|owner| match owner {
                    Some(RecordMetadataOwner::GenericRow(row)) => Some(GenericRowRef {
                        row: *row,
                        id: TypeInstId::Record(id),
                    }),
                    Some(
                        RecordMetadataOwner::ResourceRecord(_)
                        | RecordMetadataOwner::DeclaredStruct(_)
                        | RecordMetadataOwner::Group(_, _),
                    )
                    | None => None,
                }),
            GArg::Enum(id) => enums
                .get(id.index() as usize)
                .and_then(|owner| match owner {
                    Some(EnumMetadataOwner::GenericRow(row)) => Some(GenericRowRef {
                        row: *row,
                        id: TypeInstId::Enum(id),
                    }),
                    Some(EnumMetadataOwner::DeclaredEnum(_)) | None => None,
                }),
            GArg::Scalar(_)
            | GArg::Nominal(_)
            | GArg::Group(_)
            | GArg::Collection(_)
            | GArg::Param(_) => None,
        };
        let mut collection_generic_targets = Vec::with_capacity(view.collections.len());
        for (index, spec) in view.collections.iter().copied().enumerate() {
            let mut latest = None;
            let mut include = |candidate: Option<GenericRowRef>| {
                if candidate.is_some_and(|candidate| {
                    latest.is_none_or(|current: GenericRowRef| candidate.row > current.row)
                }) {
                    latest = candidate;
                }
            };
            let mut include_arg = |arg: GArg| {
                include(direct_generic_target(arg));
                if let GArg::Collection(child) = arg
                    && (child as usize) < index
                {
                    include(
                        collection_generic_targets
                            .get(child as usize)
                            .copied()
                            .flatten(),
                    );
                }
            };
            match spec {
                CollSpec::List { elem } => include_arg(elem),
                CollSpec::Map { key, value } => {
                    include_arg(key);
                    include_arg(value);
                }
            }
            collection_generic_targets.push(latest);
        }
        Ok(Self {
            records,
            enums,
            collection_generic_targets,
            seen_rows: vec![false; view.generics.type_insts.len()],
            seen_collections: vec![false; view.collections.len()],
            tasks: Vec::new(),
        })
    }

    fn row(&self, id: TypeInstId) -> Option<usize> {
        match id {
            TypeInstId::Record(id) => {
                self.records
                    .get(id.index() as usize)
                    .and_then(|owner| match owner {
                        Some(RecordMetadataOwner::GenericRow(row)) => Some(*row),
                        Some(
                            RecordMetadataOwner::ResourceRecord(_)
                            | RecordMetadataOwner::DeclaredStruct(_)
                            | RecordMetadataOwner::Group(_, _),
                        )
                        | None => None,
                    })
            }
            TypeInstId::Enum(id) => {
                self.enums
                    .get(id.index() as usize)
                    .and_then(|owner| match owner {
                        Some(EnumMetadataOwner::GenericRow(row)) => Some(*row),
                        Some(EnumMetadataOwner::DeclaredEnum(_)) | None => None,
                    })
            }
        }
    }

    fn declared_struct(&self, id: TypeId) -> Option<usize> {
        self.records
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(RecordMetadataOwner::DeclaredStruct(row)) => Some(*row),
                Some(
                    RecordMetadataOwner::ResourceRecord(_)
                    | RecordMetadataOwner::Group(_, _)
                    | RecordMetadataOwner::GenericRow(_),
                )
                | None => None,
            })
    }

    fn resource_record(&self, id: TypeId) -> Option<usize> {
        self.records
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(RecordMetadataOwner::ResourceRecord(row)) => Some(*row),
                Some(
                    RecordMetadataOwner::DeclaredStruct(_)
                    | RecordMetadataOwner::Group(_, _)
                    | RecordMetadataOwner::GenericRow(_),
                )
                | None => None,
            })
    }

    fn group(&self, id: TypeId) -> Option<(usize, usize)> {
        self.records
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(RecordMetadataOwner::Group(record, group)) => Some((*record, *group)),
                Some(
                    RecordMetadataOwner::ResourceRecord(_)
                    | RecordMetadataOwner::DeclaredStruct(_)
                    | RecordMetadataOwner::GenericRow(_),
                )
                | None => None,
            })
    }

    fn declared_enum(&self, id: EnumId) -> Option<usize> {
        self.enums
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(EnumMetadataOwner::DeclaredEnum(row)) => Some(*row),
                Some(EnumMetadataOwner::GenericRow(_)) | None => None,
            })
    }

    fn first_row_visit(&mut self, row: usize) -> bool {
        let seen = &mut self.seen_rows[row];
        if *seen {
            false
        } else {
            *seen = true;
            true
        }
    }

    fn first_collection_visit(&mut self, index: u16) -> bool {
        let seen = &mut self.seen_collections[index as usize];
        if *seen {
            false
        } else {
            *seen = true;
            true
        }
    }
}

impl TypeMetadataView<'_> {
    fn active_filling_row(&self, index: usize, id: TypeInstId) -> bool {
        let Some(start) = self.generics.fill_batch_start else {
            return false;
        };
        index >= start
            && index < self.generics.type_insts.len()
            && !self.generics.fill_stack.is_empty()
            && self.generics.fill_rows.get(&TypeInstKey::from(id)) == Some(&index)
    }

    fn validate_args(
        &self,
        args: &[GArg],
        owner: Option<TypeInstId>,
    ) -> Result<(), GenericInvariant> {
        let mut scratch = MetadataScratch::try_new(self)?;
        self.validate_args_with(args, owner, &mut scratch)
    }

    fn validate_args_with(
        &self,
        args: &[GArg],
        owner: Option<TypeInstId>,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        self.validate_arg_iter_with(args.iter().copied(), owner, scratch)
    }

    fn validate_arg_iter_with<I>(
        &self,
        args: I,
        owner: Option<TypeInstId>,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant>
    where
        I: DoubleEndedIterator<Item = GArg>,
    {
        debug_assert!(scratch.tasks.is_empty());
        let generic_parent = match owner {
            Some(id) => {
                let row = scratch
                    .row(id)
                    .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
                scratch.first_row_visit(row);
                Some(row)
            }
            None => None,
        };
        for arg in args.rev() {
            scratch.tasks.push(MetadataTask::Argument {
                arg,
                collection_parent: None,
                generic_parent,
            });
        }

        while let Some(task) = scratch.tasks.pop() {
            match task {
                MetadataTask::Argument {
                    arg,
                    collection_parent,
                    generic_parent,
                } => match arg {
                    GArg::Scalar(_) => {}
                    GArg::Nominal(id) => {
                        if self.registry.nominals.get(id.0 as usize).is_none() {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                    }
                    GArg::Struct(id) => {
                        if scratch.declared_struct(id).is_some() {
                            continue;
                        }
                        self.queue_generic_target(
                            TypeInstId::Record(id),
                            arg,
                            generic_parent,
                            scratch,
                        )?;
                    }
                    GArg::Group(id) => {
                        if scratch.group(id).is_none() {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                    }
                    GArg::Enum(id) => {
                        if scratch.declared_enum(id).is_some() {
                            continue;
                        }
                        self.queue_generic_target(
                            TypeInstId::Enum(id),
                            arg,
                            generic_parent,
                            scratch,
                        )?;
                    }
                    GArg::Collection(index) => {
                        if collection_parent.is_some_and(|parent| index >= parent) {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                        let Some(spec) = self.collections.get(index as usize).copied() else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !scratch.first_collection_visit(index) {
                            self.validate_revisited_collection_order(
                                index,
                                generic_parent,
                                scratch,
                            )?;
                            continue;
                        }
                        match spec {
                            CollSpec::List { elem } => scratch.tasks.push(MetadataTask::Argument {
                                arg: elem,
                                collection_parent: Some(index),
                                generic_parent,
                            }),
                            CollSpec::Map { key, value } => {
                                scratch.tasks.push(MetadataTask::Argument {
                                    arg: value,
                                    collection_parent: Some(index),
                                    generic_parent,
                                });
                                scratch.tasks.push(MetadataTask::Argument {
                                    arg: key,
                                    collection_parent: Some(index),
                                    generic_parent,
                                });
                            }
                        }
                    }
                    GArg::Param(index) => {
                        if self.generics.argument_domain != ArgumentDomain::TemplateProof {
                            return Err(GenericInvariant::TypeArgumentParameter(index));
                        }
                    }
                },
                MetadataTask::ReadyBody { row } => {
                    let inst = &self.generics.type_insts[row];
                    let TypeInstState::Ready(body) = &inst.state else {
                        return Err(GenericInvariant::ReadyBodyMissing(inst.id));
                    };
                    self.registry.validate_inst_body_metadata(
                        inst.template,
                        &inst.args,
                        inst.id,
                        body,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn validate_revisited_collection_order(
        &self,
        index: u16,
        generic_parent: Option<usize>,
        scratch: &MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        let Some(parent) = generic_parent else {
            return Ok(());
        };
        let Some(summary) = scratch
            .collection_generic_targets
            .get(index as usize)
            .copied()
            .flatten()
        else {
            return Ok(());
        };
        if summary.row < parent {
            return Ok(());
        }

        let mut pending = Vec::new();
        let mut seen = vec![false; self.collections.len()];
        pending.push(GArg::Collection(index));
        while let Some(arg) = pending.pop() {
            match arg {
                GArg::Struct(id) => {
                    if let Some(row) = scratch.row(TypeInstId::Record(id))
                        && row >= parent
                    {
                        return Err(GenericInvariant::TypeArgumentOrderViolation {
                            owner: self.generics.type_insts[parent].id,
                            target: TypeInstId::Record(id),
                        });
                    }
                }
                GArg::Enum(id) => {
                    if let Some(row) = scratch.row(TypeInstId::Enum(id))
                        && row >= parent
                    {
                        return Err(GenericInvariant::TypeArgumentOrderViolation {
                            owner: self.generics.type_insts[parent].id,
                            target: TypeInstId::Enum(id),
                        });
                    }
                }
                GArg::Collection(child) => {
                    let Some(child_summary) = scratch
                        .collection_generic_targets
                        .get(child as usize)
                        .copied()
                        .flatten()
                    else {
                        continue;
                    };
                    if child_summary.row < parent {
                        continue;
                    }
                    let Some(mark) = seen.get_mut(child as usize) else {
                        continue;
                    };
                    if std::mem::replace(mark, true) {
                        continue;
                    }
                    match self.collections[child as usize] {
                        CollSpec::List { elem } => pending.push(elem),
                        CollSpec::Map { key, value } => {
                            pending.push(value);
                            pending.push(key);
                        }
                    }
                }
                GArg::Scalar(_) | GArg::Nominal(_) | GArg::Group(_) | GArg::Param(_) => {}
            }
        }
        Err(GenericInvariant::TypeArgumentOrderViolation {
            owner: self.generics.type_insts[parent].id,
            target: summary.id,
        })
    }

    fn queue_generic_target(
        &self,
        id: TypeInstId,
        arg: GArg,
        generic_parent: Option<usize>,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        let Some(index) = scratch.row(id) else {
            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
        };
        if let Some(parent) = generic_parent
            && index >= parent
        {
            return Err(GenericInvariant::TypeArgumentOrderViolation {
                owner: self.generics.type_insts[parent].id,
                target: id,
            });
        }
        let inst = &self.generics.type_insts[index];
        match &inst.state {
            TypeInstState::Ready(_) => {
                self.registry.template_for_args(inst.template, &inst.args)?;
                if !scratch.first_row_visit(index) {
                    return Ok(());
                }
                scratch.tasks.push(MetadataTask::ReadyBody { row: index });
                for &nested in inst.args.iter().rev() {
                    scratch.tasks.push(MetadataTask::Argument {
                        arg: nested,
                        collection_parent: None,
                        generic_parent: Some(index),
                    });
                }
                Ok(())
            }
            TypeInstState::Filling { .. } if self.active_filling_row(index, id) => Ok(()),
            TypeInstState::Filling { .. } | TypeInstState::Rejected(_) => {
                Err(GenericInvariant::ReadyBodyMissing(id))
            }
        }
    }

    fn ready_inst_header_with<'a>(
        &'a self,
        inst: &'a TypeInst,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<&'a InstBody>, GenericInvariant> {
        let TypeInstState::Ready(body) = &inst.state else {
            return Ok(None);
        };
        let index = scratch
            .row(inst.id)
            .ok_or(GenericInvariant::ReadyBodyMissing(inst.id))?;
        self.registry.template_for_args(inst.template, &inst.args)?;
        self.validate_args_with(&inst.args, Some(inst.id), scratch)?;
        self.registry
            .validate_inst_body_metadata(inst.template, &inst.args, inst.id, body)?;
        debug_assert!(scratch.seen_rows[index]);
        Ok(Some(body))
    }

    fn ready_inst_body_with<'a>(
        &'a self,
        inst: &'a TypeInst,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<&'a InstBody>, GenericInvariant> {
        let Some(body) = self.ready_inst_header_with(inst, scratch)? else {
            return Ok(None);
        };
        self.validate_ready_body_with(inst, body, scratch)?;
        Ok(Some(body))
    }

    fn validate_ready_body_with(
        &self,
        inst: &TypeInst,
        body: &InstBody,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        self.validate_ready_body_shape(inst, body, scratch)?;
        match body {
            InstBody::Struct(fields) => {
                self.validate_arg_iter_with(fields.iter().map(|(_, arg)| *arg), None, scratch)?
            }
            InstBody::Enum(variants) => self.validate_arg_iter_with(
                variants
                    .iter()
                    .flat_map(|variant| variant.payload.iter().map(|(_, arg)| *arg)),
                None,
                scratch,
            )?,
        }
        Ok(())
    }

    fn ready_struct_field_with(
        &self,
        inst: &TypeInst,
        name: &str,
        scratch: &mut MetadataScratch,
    ) -> Result<StructFieldProjection, GenericInvariant> {
        let Some(body) = self.ready_inst_header_with(inst, scratch)? else {
            return Ok(StructFieldProjection::Absent);
        };
        self.validate_ready_body_shape(inst, body, scratch)?;
        let InstBody::Struct(fields) = body else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id: inst.id,
                body: body.kind(),
            });
        };
        let Some((index, (_, ty))) = fields
            .iter()
            .enumerate()
            .find(|(_, (field_name, _))| field_name == name)
        else {
            return Ok(StructFieldProjection::Missing);
        };
        self.validate_args_with(std::slice::from_ref(ty), None, scratch)?;
        Ok(StructFieldProjection::Field {
            index: index as u16,
            ty: *ty,
        })
    }

    fn validate_ready_body_shape(
        &self,
        inst: &TypeInst,
        body: &InstBody,
        scratch: &MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        let template = self.registry.template_for_args(inst.template, &inst.args)?;
        let mismatch = || GenericInvariant::ReadyBodyShapeMismatch(inst.id);
        let mut param_indices = HashMap::with_capacity(template.type_params.len());
        for (index, (name, _)) in template.type_params.iter().enumerate() {
            param_indices.entry(name.as_str()).or_insert(index);
        }
        match (&template.body, body) {
            (TemplateBody::Struct(expected), InstBody::Struct(actual)) => {
                if expected.len() != actual.len() {
                    return Err(mismatch());
                }
                for ((expected_name, expected_ty), (actual_name, actual_arg)) in
                    expected.iter().zip(actual)
                {
                    if expected_name != actual_name
                        || !self.ready_body_arg_matches(
                            expected_ty,
                            *actual_arg,
                            &inst.args,
                            &param_indices,
                            scratch,
                        )?
                    {
                        return Err(mismatch());
                    }
                }
            }
            (TemplateBody::Enum(expected), InstBody::Enum(actual)) => {
                if expected.len() != actual.len() {
                    return Err(mismatch());
                }
                for (expected_variant, actual_variant) in expected.iter().zip(actual) {
                    if expected_variant.name != actual_variant.name
                        || expected_variant.payload.len() != actual_variant.payload.len()
                    {
                        return Err(mismatch());
                    }
                    for (expected_field, (actual_name, actual_arg)) in
                        expected_variant.payload.iter().zip(&actual_variant.payload)
                    {
                        if expected_field.name != *actual_name
                            || !self.ready_body_arg_matches(
                                &expected_field.ty,
                                *actual_arg,
                                &inst.args,
                                &param_indices,
                                scratch,
                            )?
                        {
                            return Err(mismatch());
                        }
                    }
                }
            }
            (TemplateBody::Struct(_), InstBody::Enum(_))
            | (TemplateBody::Enum(_), InstBody::Struct(_)) => return Err(mismatch()),
        }
        Ok(())
    }

    fn ready_body_arg_matches<'a>(
        &'a self,
        expected: &'a TypeExpr,
        actual: GArg,
        args: &[GArg],
        param_indices: &HashMap<&str, usize>,
        scratch: &MetadataScratch,
    ) -> Result<bool, GenericInvariant> {
        let mut pending: Vec<(&TypeExpr, GArg)> = vec![(expected, actual)];
        while let Some((expected, actual)) = pending.pop() {
            #[cfg(test)]
            READY_BODY_MATCH_VISITS.with(|count| count.set(count.get() + 1));
            match expected {
                TypeExpr::Name { text, .. } => {
                    if let Some(expanded) = self.registry.aliases.get(text) {
                        pending.push((expanded, actual));
                        continue;
                    }
                    if let Some(&index) = param_indices.get(text.as_str()) {
                        if args.get(index).copied() != Some(actual) {
                            return Ok(false);
                        }
                        continue;
                    }
                    if let Some(scalar) = ScalarType::from_spelling(text) {
                        if actual != GArg::Scalar(scalar) {
                            return Ok(false);
                        }
                        continue;
                    }
                    let matches = match actual {
                        GArg::Nominal(id) => self
                            .registry
                            .nominals
                            .get(id.0 as usize)
                            .is_some_and(|info| info.name.as_str() == text.as_str()),
                        GArg::Struct(id) => scratch.declared_struct(id).is_some_and(|row| {
                            self.registry.structs[row].name.as_str() == text.as_str()
                        }),
                        GArg::Enum(id) => scratch.declared_enum(id).is_some_and(|row| {
                            self.registry.enums[row].name.as_str() == text.as_str()
                        }),
                        GArg::Scalar(_) | GArg::Group(_) | GArg::Collection(_) | GArg::Param(_) => {
                            false
                        }
                    };
                    if !matches {
                        return Ok(false);
                    }
                }
                TypeExpr::Apply {
                    head, args: nested, ..
                } if head == "List" => {
                    let [expected_elem] = nested.as_slice() else {
                        return Ok(false);
                    };
                    let GArg::Collection(index) = actual else {
                        return Ok(false);
                    };
                    let Some(CollSpec::List { elem }) =
                        self.collections.get(index as usize).copied()
                    else {
                        return Ok(false);
                    };
                    pending.push((expected_elem, elem));
                }
                TypeExpr::Apply {
                    head, args: nested, ..
                } if head == "Map" => {
                    let [expected_key, expected_value] = nested.as_slice() else {
                        return Ok(false);
                    };
                    let GArg::Collection(index) = actual else {
                        return Ok(false);
                    };
                    let Some(CollSpec::Map { key, value }) =
                        self.collections.get(index as usize).copied()
                    else {
                        return Ok(false);
                    };
                    pending.push((expected_value, value));
                    pending.push((expected_key, key));
                }
                TypeExpr::Apply {
                    head, args: nested, ..
                } => {
                    let id = match actual {
                        GArg::Struct(id) => TypeInstId::Record(id),
                        GArg::Enum(id) => TypeInstId::Enum(id),
                        GArg::Scalar(_)
                        | GArg::Nominal(_)
                        | GArg::Group(_)
                        | GArg::Collection(_)
                        | GArg::Param(_) => return Ok(false),
                    };
                    let Some(row) = scratch.row(id) else {
                        return Ok(false);
                    };
                    let nested_inst = &self.generics.type_insts[row];
                    let nested_template = self
                        .registry
                        .template_for_args(nested_inst.template, &nested_inst.args)?;
                    let expected_kind = id.kind();
                    let actual_kind = nested_template.body.kind();
                    if expected_kind != actual_kind {
                        return Err(GenericInvariant::TemplateKindMismatch {
                            template: nested_inst.template,
                            expected: actual_kind,
                            actual: expected_kind,
                        });
                    }
                    if nested_template.name.as_str() != head.as_str()
                        || nested.len() != nested_inst.args.len()
                    {
                        return Ok(false);
                    }
                    for (expected, actual) in nested.iter().zip(&nested_inst.args).rev() {
                        pending.push((expected, *actual));
                    }
                }
                TypeExpr::Optional { .. } | TypeExpr::Identity(_) => return Ok(false),
            }
        }
        Ok(true)
    }

    fn ready_inst_header_by_id<'a>(
        &'a self,
        id: TypeInstId,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<(&'a TypeInst, &'a InstBody)>, GenericInvariant> {
        let Some(row) = scratch.row(id) else {
            return Ok(None);
        };
        let inst = self
            .generics
            .type_insts
            .get(row)
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        self.ready_inst_header_with(inst, scratch)
            .map(|body| body.map(|body| (inst, body)))
    }

    fn ready_inst_by_id<'a>(
        &'a self,
        id: TypeInstId,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<(&'a TypeInst, &'a InstBody)>, GenericInvariant> {
        let Some(row) = scratch.row(id) else {
            return Ok(None);
        };
        let inst = self
            .generics
            .type_insts
            .get(row)
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        self.ready_inst_body_with(inst, scratch)
            .map(|body| body.map(|body| (inst, body)))
    }
}

impl TypeMetadataSession<'_> {
    fn ensure_healthy(&self) -> Result<(), GenericInvariant> {
        match self.failure {
            Some(invariant) => Err(invariant),
            None => Ok(()),
        }
    }

    fn remember<T>(&mut self, result: Result<T, GenericInvariant>) -> Result<T, GenericInvariant> {
        if let Err(invariant) = result
            && self.failure.is_none()
        {
            self.failure = Some(invariant);
        }
        result
    }

    pub(crate) fn validate_type_arguments(
        &mut self,
        args: &[GArg],
    ) -> Result<(), GenericInvariant> {
        self.ensure_healthy()?;
        let result = self.view.validate_args_with(args, None, &mut self.metadata);
        self.remember(result)
    }

    pub(crate) fn static_record_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<RecordInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(info) = self
                .view
                .registry
                .records
                .iter()
                .find(|info| info.name == name)
            else {
                return Ok(None);
            };
            let args = info
                .fields
                .iter()
                .chain(info.groups.iter().flat_map(|group| group.fields.iter()))
                .map(|field| field.ty)
                .collect::<Vec<_>>();
            self.view
                .validate_args_with(&args, None, &mut self.metadata)?;
            Ok(Some(info.clone()))
        })();
        self.remember(result)
    }

    pub(crate) fn static_group_by_name(
        &mut self,
        record: &str,
        group: &str,
    ) -> Result<Option<GroupInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(info) = self
                .view
                .registry
                .records
                .iter()
                .find(|info| info.name == record)
                .and_then(|info| info.groups.iter().find(|info| info.name == group))
            else {
                return Ok(None);
            };
            let args = info.fields.iter().map(|field| field.ty).collect::<Vec<_>>();
            self.view
                .validate_args_with(&args, None, &mut self.metadata)?;
            Ok(Some(info.clone()))
        })();
        self.remember(result)
    }

    pub(crate) fn static_struct_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<StructInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(info) = self
                .view
                .registry
                .structs
                .iter()
                .find(|info| info.name == name)
            else {
                return Ok(None);
            };
            let args = info.fields.iter().map(|field| field.ty).collect::<Vec<_>>();
            self.view
                .validate_args_with(&args, None, &mut self.metadata)?;
            Ok(Some(info.clone()))
        })();
        self.remember(result)
    }

    pub(crate) fn static_enum_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<EnumInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = Ok(self
            .view
            .registry
            .enums
            .iter()
            .find(|info| info.name == name)
            .cloned());
        self.remember(result)
    }

    pub(crate) fn static_named_type(
        &mut self,
        name: &str,
    ) -> Result<Option<StaticNamedType>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = Ok(
            if let Some(info) = self
                .view
                .registry
                .structs
                .iter()
                .find(|info| info.name == name)
            {
                Some(StaticNamedType::Struct(info.type_id))
            } else if let Some(info) = self
                .view
                .registry
                .enums
                .iter()
                .find(|info| info.name == name)
            {
                Some(StaticNamedType::Enum(info.enum_id))
            } else {
                self.view
                    .registry
                    .records
                    .iter()
                    .find(|info| info.name == name)
                    .map(|info| StaticNamedType::Record(info.type_id))
            },
        );
        self.remember(result)
    }

    pub(crate) fn product_field(
        &mut self,
        ty: TypeId,
        name: &str,
    ) -> Result<ProductFieldProjection, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(owner) = self
                .metadata
                .records
                .get(ty.index() as usize)
                .copied()
                .flatten()
            else {
                return Ok(ProductFieldProjection::Absent);
            };
            match owner {
                RecordMetadataOwner::ResourceRecord(record) => {
                    let info = &self.view.registry.records[record];
                    if let Some((index, field)) = info.field(name) {
                        self.view.validate_args_with(
                            std::slice::from_ref(&field.ty),
                            None,
                            &mut self.metadata,
                        )?;
                        return Ok(ProductFieldProjection::Field {
                            index,
                            ty: field.ty,
                            required: field.required,
                        });
                    }
                    if let Some((index, group)) = info.group(name) {
                        return Ok(ProductFieldProjection::Group {
                            index,
                            ty: group.type_id,
                        });
                    }
                    Ok(ProductFieldProjection::MissingRecordField)
                }
                RecordMetadataOwner::Group(record, group) => {
                    let info = &self.view.registry.records[record].groups[group];
                    let Some((index, field)) = info.field(name) else {
                        return Ok(ProductFieldProjection::MissingGroupField);
                    };
                    self.view.validate_args_with(
                        std::slice::from_ref(&field.ty),
                        None,
                        &mut self.metadata,
                    )?;
                    Ok(ProductFieldProjection::Field {
                        index,
                        ty: field.ty,
                        required: field.required,
                    })
                }
                RecordMetadataOwner::DeclaredStruct(_) | RecordMetadataOwner::GenericRow(_) => {
                    Ok(ProductFieldProjection::Absent)
                }
            }
        })();
        self.remember(result)
    }

    pub(crate) fn struct_field(
        &mut self,
        ty: TypeId,
        name: &str,
    ) -> Result<StructFieldProjection, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(owner) = self
                .metadata
                .records
                .get(ty.index() as usize)
                .copied()
                .flatten()
            else {
                return Ok(StructFieldProjection::Absent);
            };
            match owner {
                RecordMetadataOwner::DeclaredStruct(row) => {
                    let info = &self.view.registry.structs[row];
                    let Some((index, field)) = info.field(name) else {
                        return Ok(StructFieldProjection::Missing);
                    };
                    self.view.validate_args_with(
                        std::slice::from_ref(&field.ty),
                        None,
                        &mut self.metadata,
                    )?;
                    Ok(StructFieldProjection::Field {
                        index,
                        ty: field.ty,
                    })
                }
                RecordMetadataOwner::GenericRow(row) => self.view.ready_struct_field_with(
                    &self.view.generics.type_insts[row],
                    name,
                    &mut self.metadata,
                ),
                RecordMetadataOwner::ResourceRecord(_) | RecordMetadataOwner::Group(_, _) => {
                    Ok(StructFieldProjection::Absent)
                }
            }
        })();
        self.remember(result)
    }

    pub(crate) fn instantiation_of(
        &mut self,
        id: TypeInstId,
    ) -> Result<Option<(usize, Vec<GArg>)>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some((inst, _)) = self.view.ready_inst_header_by_id(id, &mut self.metadata)? else {
                return Ok(None);
            };
            Ok(Some((inst.template, inst.args.clone())))
        })();
        self.remember(result)
    }

    pub(crate) fn collection_spec(&mut self, index: u16) -> Result<CollSpec, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let arg = GArg::Collection(index);
            self.view
                .validate_args_with(std::slice::from_ref(&arg), None, &mut self.metadata)?;
            self.view
                .collections
                .get(index as usize)
                .copied()
                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))
        })();
        self.remember(result)
    }

    pub(crate) fn reserved_instantiation(
        &mut self,
        id: EnumId,
    ) -> Result<Option<ReservedEnumArgs>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some((inst, body)) = self
                .view
                .ready_inst_by_id(TypeInstId::Enum(id), &mut self.metadata)?
            else {
                return Ok(None);
            };
            match self.view.registry.type_templates[inst.template].reserved {
                Some(Reserved::Option) => {
                    let [inner] = inst.args.as_slice() else {
                        return Err(GenericInvariant::TypeArgumentCountMismatch {
                            template: inst.template,
                            expected: 1,
                            actual: inst.args.len(),
                        });
                    };
                    let InstBody::Enum(variants) = body else {
                        return Err(GenericInvariant::TypeBodyKindMismatch {
                            id: inst.id,
                            body: body.kind(),
                        });
                    };
                    let exact = variants.len() == 2
                        && variants[OPTION_NONE as usize].name == "none"
                        && variants[OPTION_NONE as usize].payload.is_empty()
                        && variants[OPTION_SOME as usize].name == "some"
                        && variants[OPTION_SOME as usize].payload.len() == 1
                        && variants[OPTION_SOME as usize].payload[0].0 == "value"
                        && variants[OPTION_SOME as usize].payload[0].1 == *inner;
                    if !exact {
                        return Err(GenericInvariant::ReadyBodyShapeMismatch(inst.id));
                    }
                    Ok(Some(ReservedEnumArgs::Option(*inner)))
                }
                Some(Reserved::Result) => {
                    let [ok, err] = inst.args.as_slice() else {
                        return Err(GenericInvariant::TypeArgumentCountMismatch {
                            template: inst.template,
                            expected: 2,
                            actual: inst.args.len(),
                        });
                    };
                    let InstBody::Enum(variants) = body else {
                        return Err(GenericInvariant::TypeBodyKindMismatch {
                            id: inst.id,
                            body: body.kind(),
                        });
                    };
                    let exact = variants.len() == 2
                        && variants[RESULT_OK as usize].name == "ok"
                        && variants[RESULT_OK as usize].payload.len() == 1
                        && variants[RESULT_OK as usize].payload[0].0 == "value"
                        && variants[RESULT_OK as usize].payload[0].1 == *ok
                        && variants[RESULT_ERR as usize].name == "err"
                        && variants[RESULT_ERR as usize].payload.len() == 1
                        && variants[RESULT_ERR as usize].payload[0].0 == "value"
                        && variants[RESULT_ERR as usize].payload[0].1 == *err;
                    if !exact {
                        return Err(GenericInvariant::ReadyBodyShapeMismatch(inst.id));
                    }
                    Ok(Some(ReservedEnumArgs::Result(*ok, *err)))
                }
                None => Ok(Some(ReservedEnumArgs::Other)),
            }
        })();
        self.remember(result)
    }

    pub(crate) fn garg_spelling(&mut self, arg: GArg) -> Result<String, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            self.view
                .validate_args_with(std::slice::from_ref(&arg), None, &mut self.metadata)?;
            garg_spelling_validated(
                self.view.registry,
                &self.view,
                &self.metadata,
                arg,
                &mut self.display,
            )
        })();
        self.remember(result)
    }

    pub(crate) fn durable_enum_shape_and_anchor(
        &mut self,
        id: EnumId,
    ) -> Result<Option<(ResolvedEnumVariants, String)>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            if let Some(info) = self.view.registry.enum_by_id(id) {
                let variants = info
                    .variants
                    .iter()
                    .map(|variant| {
                        (
                            variant.name.clone(),
                            variant
                                .payload
                                .iter()
                                .map(|field| GArg::Scalar(field.scalar))
                                .collect(),
                        )
                    })
                    .collect();
                return Ok(Some((variants, info.name.clone())));
            }
            let inst_id = TypeInstId::Enum(id);
            let Some((_, body)) = self.view.ready_inst_by_id(inst_id, &mut self.metadata)? else {
                return Ok(None);
            };
            let InstBody::Enum(variants) = body else {
                return Err(GenericInvariant::TypeBodyKindMismatch {
                    id: inst_id,
                    body: TypeInstKind::Struct,
                });
            };
            let variants = variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant.payload.iter().map(|(_, arg)| *arg).collect(),
                    )
                })
                .collect();
            let spelling = self
                .view
                .registry
                .inst_anchor_spelling_validated(
                    &self.view,
                    &self.metadata,
                    inst_id,
                    &mut self.display,
                )?
                .ok_or(GenericInvariant::ReadyBodyMissing(inst_id))?;
            Ok(Some((variants, spelling)))
        })();
        self.remember(result)
    }

    pub(crate) fn validate_durable_value_metadata(
        &mut self,
        roots: impl IntoIterator<Item = GArg>,
    ) -> Result<(), GenericInvariant> {
        self.ensure_healthy()?;
        let roots: Vec<GArg> = roots.into_iter().collect();
        let result = (|| {
            self.view
                .validate_args_with(&roots, None, &mut self.metadata)?;
            let mut durable = DurableMetadataScratch::new(&self.metadata, roots);

            while let Some((arg, depth)) = durable.pending.pop_front() {
                if depth > marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH {
                    continue;
                }
                self.view.validate_args_with(
                    std::slice::from_ref(&arg),
                    None,
                    &mut self.metadata,
                )?;
                let next_depth = depth + 1;
                match arg {
                    GArg::Struct(id) => {
                        let Some(first) = durable.first_record(id) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !first {
                            continue;
                        }
                        if let Some(row) = self.metadata.declared_struct(id) {
                            let Some(info) = self.view.registry.structs.get(row) else {
                                return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                            };
                            for field in &info.fields {
                                durable.push(field.ty, next_depth);
                            }
                            continue;
                        }
                        let Some(row) = self.metadata.row(TypeInstId::Record(id)) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        let inst = &self.view.generics.type_insts[row];
                        let Some(body) =
                            self.view.ready_inst_body_with(inst, &mut self.metadata)?
                        else {
                            return Err(GenericInvariant::ReadyBodyMissing(inst.id));
                        };
                        for &nested in &inst.args {
                            durable.push(nested, next_depth);
                        }
                        let InstBody::Struct(fields) = body else {
                            return Err(GenericInvariant::TypeBodyKindMismatch {
                                id: inst.id,
                                body: body.kind(),
                            });
                        };
                        for (_, field) in fields {
                            durable.push(*field, next_depth);
                        }
                    }
                    GArg::Enum(id) => {
                        let Some(first) = durable.first_enum(id) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !first || self.metadata.declared_enum(id).is_some() {
                            continue;
                        }
                        let Some(row) = self.metadata.row(TypeInstId::Enum(id)) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        let inst = &self.view.generics.type_insts[row];
                        let Some(body) =
                            self.view.ready_inst_body_with(inst, &mut self.metadata)?
                        else {
                            return Err(GenericInvariant::ReadyBodyMissing(inst.id));
                        };
                        for &nested in &inst.args {
                            durable.push(nested, next_depth);
                        }
                        let InstBody::Enum(variants) = body else {
                            return Err(GenericInvariant::TypeBodyKindMismatch {
                                id: inst.id,
                                body: body.kind(),
                            });
                        };
                        for variant in variants {
                            for (_, field) in &variant.payload {
                                durable.push(*field, next_depth);
                            }
                        }
                    }
                    GArg::Collection(index) => {
                        let Some(first) = durable.first_collection(index) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !first {
                            continue;
                        }
                        let Some(spec) = self.view.collections.get(index as usize).copied() else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        match spec {
                            CollSpec::List { elem } => durable.push(elem, next_depth),
                            CollSpec::Map { key, value } => {
                                durable.push(key, next_depth);
                                durable.push(value, next_depth);
                            }
                        }
                    }
                    GArg::Scalar(_) | GArg::Nominal(_) | GArg::Group(_) => {}
                    GArg::Param(index) => {
                        return Err(GenericInvariant::TypeArgumentParameter(index));
                    }
                }
            }
            Ok(())
        })();
        self.remember(result)
    }
}

impl TypeRegistry {
    pub(crate) fn with_metadata_session<'registry, T, E>(
        &'registry self,
        use_session: impl FnOnce(&mut TypeMetadataSession<'registry>) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<GenericInvariant>,
    {
        let view = self.metadata_view();
        let metadata = MetadataScratch::try_new(&view).map_err(E::from)?;
        let mut session = TypeMetadataSession {
            display: DisplayScratch::for_view(&view),
            view,
            metadata,
            failure: None,
        };
        use_session(&mut session)
    }

    pub(crate) fn static_record_projection(
        &self,
        name: &str,
    ) -> Result<Option<RecordInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_record_by_name(name))
    }

    pub(crate) fn static_group_projection(
        &self,
        record: &str,
        group: &str,
    ) -> Result<Option<GroupInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_group_by_name(record, group))
    }

    pub(crate) fn static_struct_projection(
        &self,
        name: &str,
    ) -> Result<Option<StructInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_struct_by_name(name))
    }

    pub(crate) fn static_enum_projection(
        &self,
        name: &str,
    ) -> Result<Option<EnumInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_enum_by_name(name))
    }

    pub(crate) fn static_named_type_projection(
        &self,
        name: &str,
    ) -> Result<Option<StaticNamedType>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_named_type(name))
    }

    pub(crate) fn product_field_projection(
        &self,
        ty: TypeId,
        name: &str,
    ) -> Result<ProductFieldProjection, GenericInvariant> {
        self.with_metadata_session(|session| session.product_field(ty, name))
    }

    pub(crate) fn struct_field_projection(
        &self,
        ty: TypeId,
        name: &str,
    ) -> Result<StructFieldProjection, GenericInvariant> {
        self.with_metadata_session(|session| session.struct_field(ty, name))
    }

    fn metadata_view(&self) -> TypeMetadataView<'_> {
        TypeMetadataView {
            registry: self,
            generics: self.generics.borrow(),
            collections: self.collections.borrow(),
        }
    }

    /// Select one template only after proving the cache key has exactly the
    /// declaration's argument cardinality. This owner never indexes or zips an
    /// unchecked template/argument pair.
    fn template_for_args(
        &self,
        template: usize,
        args: &[GArg],
    ) -> Result<&TypeTemplate, GenericInvariant> {
        let template_info = self
            .type_templates
            .get(template)
            .ok_or(GenericInvariant::TypeTemplateMissing(template))?;
        let expected = template_info.type_params.len();
        let actual = args.len();
        if actual != expected {
            return Err(GenericInvariant::TypeArgumentCountMismatch {
                template,
                expected,
                actual,
            });
        }
        Ok(template_info)
    }

    fn validate_inst_body_metadata(
        &self,
        template: usize,
        args: &[GArg],
        id: TypeInstId,
        body: &InstBody,
    ) -> Result<(), GenericInvariant> {
        let template_info = self.template_for_args(template, args)?;
        let body_kind = body.kind();
        if id.kind() != body_kind {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id,
                body: body_kind,
            });
        }
        let template_kind = template_info.body.kind();
        if template_kind != id.kind() {
            return Err(GenericInvariant::TemplateKindMismatch {
                template,
                expected: template_kind,
                actual: id.kind(),
            });
        }
        Ok(())
    }

    fn validate_ready_requirement<R: ReadyInstanceRequirement>(
        &self,
        inst: &TypeInst,
        body: &InstBody,
        requirement: R,
    ) -> Result<(), GenericInvariant> {
        requirement.validate(inst, body)
    }

    pub(crate) fn validate_type_arguments(&self, args: &[GArg]) -> Result<(), GenericInvariant> {
        self.metadata_view().validate_args(args, None)
    }

    /// The image enum index of the reserved `Option[inner]`, minting it on first use.
    pub(crate) fn instantiate_reserved_option(
        &self,
        draft: &mut ImageDraft,
        inner: GArg,
        site: MintSite<'_>,
    ) -> Result<EnumId, ResolveError> {
        let template = self.application_template("Option")?;
        match self.mint_type_instance(draft, template, &[inner], site) {
            Ok(TypeInstId::Enum(id)) => Ok(id),
            Ok(TypeInstId::Record(_)) => Err(ResolveError::Invariant(
                GenericInvariant::TemplateKindMismatch {
                    template,
                    expected: TypeInstKind::Enum,
                    actual: TypeInstKind::Struct,
                },
            )),
            Err(error) => Err(error),
        }
    }

    /// Select the compiler-owned template for one generic application. Reserved
    /// applications resolve by their reserved identity, never by a same-spelled
    /// user row, and both reserved templates are required to remain enums.
    pub(crate) fn application_template(&self, head: &str) -> Result<usize, ResolveError> {
        let reserved = match head {
            "Option" => Some(Reserved::Option),
            "Result" => Some(Reserved::Result),
            _ => None,
        };
        let template = if let Some(reserved) = reserved {
            self.type_templates
                .iter()
                .position(|template| template.reserved == Some(reserved))
                .ok_or(ResolveError::Invariant(
                    GenericInvariant::ReservedTemplateMissing(reserved),
                ))?
        } else {
            self.type_template_by_name(head)
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?
        };
        if reserved.is_some() {
            let actual = self.type_templates[template].body.kind();
            if actual != TypeInstKind::Enum {
                return Err(ResolveError::Invariant(
                    GenericInvariant::TemplateKindMismatch {
                        template,
                        expected: TypeInstKind::Enum,
                        actual,
                    },
                ));
            }
        }
        Ok(template)
    }

    /// The template index of a reserved toolchain generic.
    #[cfg(test)]
    fn reserved_template(&self, reserved: Reserved) -> usize {
        match reserved {
            Reserved::Option => 0,
            Reserved::Result => 1,
        }
    }

    /// The template index of a generic value type named `head` (a reserved
    /// `Option`/`Result` or a user `struct`/`enum` template), if one exists.
    pub(crate) fn type_template_by_name(&self, head: &str) -> Option<usize> {
        self.type_templates
            .iter()
            .position(|template| template.name == head)
    }

    /// Whether a generic type template's head names an enum (versus a struct).
    pub(crate) fn template_is_enum(&self, template: usize) -> bool {
        self.type_templates[template].is_enum()
    }

    /// The declared type-parameter names and constraints of a generic type template.
    pub(crate) fn template_type_params(
        &self,
        template: usize,
    ) -> &[(String, Option<TypeConstraint>)] {
        &self.type_templates[template].type_params
    }

    /// The source name of a generic type template.
    pub(crate) fn template_name(&self, template: usize) -> &str {
        &self.type_templates[template].name
    }

    /// The declared field names and type expressions (over the template's type
    /// parameters) of a generic struct template, for construction inference. `None`
    /// if the template is an enum.
    pub(crate) fn template_struct_fields(
        &self,
        template: usize,
    ) -> Result<Vec<(String, TypeExpr)>, GenericInvariant> {
        let template_info = self
            .type_templates
            .get(template)
            .ok_or(GenericInvariant::TypeTemplateMissing(template))?;
        match &template_info.body {
            TemplateBody::Struct(fields) => Ok(fields.clone()),
            TemplateBody::Enum(_) => Err(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Struct,
                actual: TypeInstKind::Enum,
            }),
        }
    }

    /// The declared payload field names and type expressions of one variant of a
    /// generic enum template, for construction inference. The returned ordinal binds
    /// the later Ready-body lookup to the exact template member selected here.
    /// `None` means an enum template has no such variant; a struct template is an
    /// exact kind invariant rather than an absent enum member.
    pub(crate) fn template_variant_payload(
        &self,
        template: usize,
        variant: &str,
    ) -> Result<Option<TemplateVariantPayload>, GenericInvariant> {
        let template_info = self
            .type_templates
            .get(template)
            .ok_or(GenericInvariant::TypeTemplateMissing(template))?;
        match &template_info.body {
            TemplateBody::Enum(variants) => Ok(variants
                .iter()
                .enumerate()
                .find(|(_, candidate)| candidate.name == variant)
                .map(|(index, candidate)| {
                    (
                        index,
                        candidate
                            .payload
                            .iter()
                            .map(|field| (field.name.clone(), field.ty.clone()))
                            .collect(),
                    )
                })),
            TemplateBody::Struct(_) => Err(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Enum,
                actual: TypeInstKind::Struct,
            }),
        }
    }

    /// Resolve a type annotation to a bare value type (a [`GArg`]), monomorphizing
    /// any `Option`/`Result`/user generic application into `draft` on first use.
    /// `None` for an optional, the resource record, or a name not yet
    /// declared as a value type.
    pub(crate) fn resolve_garg(
        &self,
        draft: &mut ImageDraft,
        annotation: &TypeExpr,
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        self.resolve_garg_expanded(draft, &self.expand(annotation), &[], site)
    }

    /// Resolve a type expression under a substitution environment (`param name ->
    /// concrete argument`), used when a generic template body is monomorphized. The
    /// expression is already alias-expanded.
    #[inline(always)]
    fn resolve_garg_env(
        &self,
        draft: &mut ImageDraft,
        ty: &TypeExpr,
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        self.resolve_garg_expanded(draft, &self.expand(ty), subst, site)
    }

    fn resolve_garg_expanded(
        &self,
        draft: &mut ImageDraft,
        ty: &TypeExpr,
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        match ty {
            TypeExpr::Name { text, .. } => self.resolve_garg_name(text, subst),
            TypeExpr::Apply { head, args, .. } if head == "List" => {
                self.resolve_list_garg(draft, args, subst, site)
            }
            TypeExpr::Apply { head, args, .. } if head == "Map" => {
                self.resolve_map_garg(draft, args, subst, site)
            }
            TypeExpr::Apply { head, args, .. } => {
                self.resolve_template_garg(draft, head, args, subst, site)
            }
            _ => Err(ResolveError::Refusal(ResolveRefusal::Unsupported)),
        }
    }

    fn resolve_garg_name(
        &self,
        text: &str,
        subst: &[(String, GArg)],
    ) -> Result<GArg, ResolveError> {
        if let Some((_, arg)) = subst.iter().find(|(name, _)| name == text) {
            Ok(*arg)
        } else if let Some(scalar) = ScalarType::from_spelling(text) {
            Ok(GArg::Scalar(scalar))
        } else if let Some((id, _)) = self.nominal_by_name(text) {
            Ok(GArg::Nominal(id))
        } else if let Some(info) = self.struct_by_name(text) {
            Ok(GArg::Struct(info.type_id))
        } else {
            self.enum_by_name(text)
                .map(|info| GArg::Enum(info.enum_id))
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))
        }
    }

    fn resolve_list_garg(
        &self,
        draft: &mut ImageDraft,
        args: &[TypeExpr],
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        let [elem] = args else {
            return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
        };
        let elem = self.resolve_garg_expanded(draft, &self.expand(elem), subst, site)?;
        Ok(GArg::Collection(self.instantiate_list(draft, elem)?))
    }

    fn resolve_map_garg(
        &self,
        draft: &mut ImageDraft,
        args: &[TypeExpr],
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        let [key, value] = args else {
            return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
        };
        let key = self.resolve_garg_expanded(draft, &self.expand(key), subst, site)?;
        self.check_map_key_admissibility(key)?;
        let value = self.resolve_garg_expanded(draft, &self.expand(value), subst, site)?;
        Ok(GArg::Collection(self.instantiate_map(draft, key, value)?))
    }

    fn resolve_template_garg(
        &self,
        draft: &mut ImageDraft,
        head: &str,
        args: &[TypeExpr],
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        let template = self.application_template(head)?;
        let mut resolved = Vec::with_capacity(args.len());
        for arg in args {
            resolved.push(self.resolve_garg_expanded(draft, &self.expand(arg), subst, site)?);
        }
        if resolved.len() != self.type_templates[template].type_params.len() {
            return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
        }
        // Concrete constraint revalidation: every resolved argument (a `Param`
        // only reaches here in the throwaway template-check draft) must support
        // its parameter's constraint.
        for ((_, constraint), arg) in self.type_templates[template]
            .type_params
            .iter()
            .zip(&resolved)
        {
            if let Some(constraint) = constraint
                && !matches!(arg, GArg::Param(_))
                && !arg.satisfies(*constraint)
            {
                // Metadata invariants dominate an ordinary constraint refusal,
                // but the successful mint path performs this same preflight and
                // must not rebuild it here.
                self.validate_type_arguments(&resolved)?;
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            }
        }
        self.mint_type_instance(draft, template, &resolved, site)
            .map(|id| match id {
                TypeInstId::Record(ty) => GArg::Struct(ty),
                TypeInstId::Enum(id) => GArg::Enum(id),
            })
    }

    /// Validate one instantiation key and resolve any existing row without keeping
    /// validation scratch in the recursive mint frame. A missing key returns `None`
    /// only after its complete metadata preflight succeeds.
    fn existing_type_instance<R: ReadyInstanceRequirement>(
        &self,
        template: usize,
        args: &[GArg],
        requirement: R,
    ) -> Result<Option<TypeInstId>, ResolveError> {
        let filling = {
            let view = self.metadata_view();
            self.template_for_args(template, args)?;
            let mut metadata = MetadataScratch::try_new(&view)?;
            view.validate_args_with(args, None, &mut metadata)?;
            // Mint-dedup reuse probe: a keyed lookup into the append-only secondary
            // index, not a linear scan of the authority vector. The row it names is
            // re-checked against the looked-up key so index/authority drift surfaces
            // as a typed coherence failure rather than a wrong reuse.
            #[cfg(test)]
            bump_scaling(|counts| counts.type_inst_scan_steps += 1);
            let existing = match view.generics.type_index.get(&(template, args.to_vec())) {
                Some(&index) => {
                    let drifted = view
                        .generics
                        .type_insts
                        .get(index)
                        .is_none_or(|inst| inst.template != template || inst.args != args);
                    if drifted {
                        return Err(GenericInvariant::CacheState(
                            GenericCacheInvariant::MintIndexDrift,
                        )
                        .into());
                    }
                    Some(index)
                }
                None => None,
            };
            match existing {
                Some(index) => {
                    let inst = &view.generics.type_insts[index];
                    match &inst.state {
                        TypeInstState::Ready(_) => {
                            let body = view
                                .ready_inst_header_with(inst, &mut metadata)?
                                .ok_or(GenericInvariant::ReadyBodyMissing(inst.id))?;
                            self.validate_ready_requirement(inst, body, requirement)?;
                            view.validate_ready_body_with(inst, body, &mut metadata)?;
                            return Ok(Some(inst.id));
                        }
                        TypeInstState::Rejected(refusal) => {
                            return Err(ResolveError::Refusal(*refusal));
                        }
                        TypeInstState::Filling { .. } => Some((index, inst.id)),
                    }
                }
                None => None,
            }
        };
        let Some((index, id)) = filling else {
            return Ok(None);
        };

        let mut generics = self.generics.borrow_mut();
        let Some(start) = generics.fill_batch_start else {
            return Err(GenericInvariant::CacheState(
                GenericCacheInvariant::FillingReuseOutsideBatch,
            )
            .into());
        };
        let Some(&dependent) = generics.fill_stack.last() else {
            return Err(GenericInvariant::CacheState(
                GenericCacheInvariant::FillingReuseOutsideBatch,
            )
            .into());
        };
        let valid = index >= start
            && index < generics.type_insts.len()
            && dependent >= start
            && dependent < generics.type_insts.len()
            && generics.fill_rows.get(&TypeInstKey::from(id)) == Some(&index)
            && matches!(
                generics.type_insts[dependent].state,
                TypeInstState::Filling { .. }
            )
            && generics
                .fill_rows
                .get(&TypeInstKey::from(generics.type_insts[dependent].id))
                == Some(&dependent);
        if !valid {
            return Err(GenericInvariant::CacheState(
                GenericCacheInvariant::FillingReuseOutsideBatch,
            )
            .into());
        }
        if dependent != index {
            generics.type_insts[index].dependents.push(dependent);
        }
        Ok(Some(id))
    }

    /// Mint (or reuse) the instantiation of a generic type template at concrete
    /// arguments, returning its image index. Declare-then-fill reserves the record or
    /// enum and a provisional cache row before resolving members, so recursive lookup
    /// can reuse its identity without exposing a semantic body. The outermost fill
    /// settles every provisional row to `Ready` or `Rejected` through the recorded
    /// dependency graph; the containment-cycle check then rejects a real value cycle.
    /// A shared bound or depth refusal returns `Err(Limit)` and records the one owned
    /// `check.instantiation_limit` diagnostic.
    #[inline(always)]
    pub(crate) fn mint_type_instance(
        &self,
        draft: &mut ImageDraft,
        template: usize,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<TypeInstId, ResolveError> {
        self.mint_type_instance_with_requirement(draft, template, args, site, AnyReadyInstance)
    }

    #[inline(never)]
    fn mint_type_instance_with_requirement<R: ReadyInstanceRequirement>(
        &self,
        draft: &mut ImageDraft,
        template: usize,
        args: &[GArg],
        site: MintSite<'_>,
        requirement: R,
    ) -> Result<TypeInstId, ResolveError> {
        if let Some(id) = self.existing_type_instance(template, args, requirement)? {
            return Ok(id);
        }
        let template_info = self.template_for_args(template, args)?;
        {
            let generics = self.generics.borrow();
            let over_count =
                generics.type_insts.len() + generics.fn_insts.len() >= MAX_INSTANTIATIONS;
            let over_depth = generics.fill_stack.len() >= MINT_DEPTH_LIMIT;
            if over_count || over_depth {
                drop(generics);
                let site = if site.file.is_empty() {
                    MintSite {
                        file: &template_info.file,
                        span: template_info.name_span,
                    }
                } else {
                    site
                };
                self.record_limit(
                    site,
                    "a generic type likely nests inside itself over an ever-growing type",
                );
                return Err(ResolveError::Refusal(ResolveRefusal::Limit));
            }
        }
        // Reserve the image index and a provisional cache row before filling, so a
        // member that names this same instantiation finds its identity and the fill
        // terminates without making an unfinished body semantically readable.
        let name_id = draft.intern_string(&template_info.name);
        let id = if template_info.is_enum() {
            let enum_id = draft.add_enum_type(EnumTypeDef {
                name: name_id,
                variants: Vec::new(),
            });
            TypeInstId::Enum(enum_id)
        } else {
            let type_id = draft.add_record_type(RecordTypeDef {
                name: name_id,
                fields: Vec::new(),
            });
            TypeInstId::Record(type_id)
        };
        let inst_index = {
            let mut generics = self.generics.borrow_mut();
            let index = generics.type_insts.len();
            if generics.fill_stack.is_empty() && generics.fill_batch_start.is_none() {
                generics.fill_batch_start = Some(index);
            }
            generics.type_insts.push(TypeInst {
                template,
                args: args.to_vec(),
                id,
                state: TypeInstState::Filling { staged: None },
                dependents: Vec::new(),
            });
            // Keep the lookup-only reuse index in lockstep with its authority. A mint
            // only appends on a dedup miss, so this key is new; a pre-existing entry
            // would be a mint/dedup coherence failure.
            let displaced = generics.type_index.insert((template, args.to_vec()), index);
            debug_assert!(displaced.is_none(), "type mint index key already present");
            generics.fill_rows.insert(id.into(), index);
            index
        };
        self.record_active_dependency(inst_index);
        self.record_semantic_dependencies(inst_index, args.iter().copied());
        // Fill the reserved members. A member may recursively mint further
        // instantiations; the fill-stack length bounds that native recursion so a
        // divergent chain (an ever-growing argument) trips the limit before it can
        // overflow the stack, while any finite nesting (source nesting is itself
        // depth-bounded) completes.
        {
            let mut generics = self.generics.borrow_mut();
            generics.fill_stack.push(inst_index);
        }
        let filled = self.fill_type_body(draft, template, id, args, site);
        let outermost = self.finish_fill_stack(inst_index)?;
        let immediate_refusal = match filled {
            Ok(body) => {
                self.record_inst_body_dependencies(inst_index, &body);
                self.generics.borrow_mut().type_insts[inst_index].state =
                    TypeInstState::Filling { staged: Some(body) };
                None
            }
            Err(ResolveError::Refusal(refusal)) => {
                self.generics
                    .borrow_mut()
                    .fill_failures
                    .push((inst_index, refusal));
                Some(refusal)
            }
            Err(ResolveError::Invariant(invariant)) => {
                return Err(ResolveError::Invariant(invariant));
            }
        };
        if outermost {
            self.settle_fill_batch()?;
            return self.settled_type_result(inst_index, id, requirement);
        }
        match immediate_refusal {
            Some(refusal) => Err(ResolveError::Refusal(refusal)),
            None if requirement.allows_provisional() => Ok(id),
            None => Err(GenericInvariant::ReadyBodyMissing(id).into()),
        }
    }

    /// Mint one generic struct only after the registry proves the template and the
    /// returned row are both record-shaped and Ready.
    pub(crate) fn mint_struct_instance(
        &self,
        draft: &mut ImageDraft,
        template: usize,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<TypeId, ResolveError> {
        let template_info = self.template_for_args(template, args)?;
        let actual = template_info.body.kind();
        if actual != TypeInstKind::Struct {
            return Err(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Struct,
                actual,
            }
            .into());
        }
        let id = self.mint_type_instance_with_requirement(
            draft,
            template,
            args,
            site,
            StructReadyInstance,
        )?;
        let TypeInstId::Record(record) = id else {
            return Err(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Struct,
                actual: TypeInstKind::Enum,
            }
            .into());
        };
        Ok(record)
    }

    /// Mint one generic enum constructor and return only the exact Ready member
    /// selected during source-template inference.
    pub(crate) fn mint_enum_variant_instance(
        &self,
        draft: &mut ImageDraft,
        template: usize,
        args: &[GArg],
        selection: EnumVariantSelection<'_>,
        site: MintSite<'_>,
    ) -> Result<EnumVariantInstance, ResolveError> {
        let template_info = self.template_for_args(template, args)?;
        let actual = template_info.body.kind();
        if actual != TypeInstKind::Enum {
            return Err(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Enum,
                actual,
            }
            .into());
        }
        let id =
            self.mint_type_instance_with_requirement(draft, template, args, site, selection)?;
        let TypeInstId::Enum(enum_id) = id else {
            return Err(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Enum,
                actual: TypeInstKind::Struct,
            }
            .into());
        };
        let variant_index = u16::try_from(selection.index).map_err(|_| {
            ResolveError::Invariant(GenericInvariant::ReadyEnumVariantMissing {
                id: enum_id,
                template,
                variant: selection.index,
            })
        })?;
        Ok(EnumVariantInstance {
            enum_id,
            variant: variant_index,
        })
    }

    /// Close one native fill frame. A mismatch is observed without consuming the
    /// actual top frame so the first cache invariant preserves all hostile state.
    fn finish_fill_stack(&self, inst_index: usize) -> Result<bool, ResolveError> {
        let mut generics = self.generics.borrow_mut();
        if generics.fill_stack.last() != Some(&inst_index) {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::FillStackMismatch,
            )));
        }
        generics.fill_stack.pop();
        Ok(generics.fill_stack.is_empty())
    }

    /// Resolve a reserved type instantiation's members under its argument
    /// substitution, writing the image record/enum fields and returning the resolved
    /// body. A member refusal returns its typed `Unsupported` or `Limit` variant for
    /// outermost dependency settlement.
    fn fill_type_body(
        &self,
        draft: &mut ImageDraft,
        template: usize,
        id: TypeInstId,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<InstBody, ResolveError> {
        let template_info = self.template_for_args(template, args)?;
        let body_kind = template_info.body.kind();
        if id.kind() != body_kind {
            return Err(ResolveError::Invariant(
                GenericInvariant::TypeBodyKindMismatch {
                    id,
                    body: body_kind,
                },
            ));
        }
        match body_kind {
            TypeInstKind::Struct => self.fill_struct_type_body(draft, template, id, args, site),
            TypeInstKind::Enum => self.fill_enum_type_body(draft, template, id, args, site),
        }
    }

    fn fill_struct_type_body(
        &self,
        draft: &mut ImageDraft,
        template: usize,
        id: TypeInstId,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<InstBody, ResolveError> {
        let template_info = self.template_for_args(template, args)?;
        let subst: Vec<(String, GArg)> = template_info
            .type_params
            .iter()
            .map(|(name, _)| name.clone())
            .zip(args.iter().copied())
            .collect();
        let TemplateBody::Struct(fields) = &template_info.body else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id,
                body: TypeInstKind::Enum,
            }
            .into());
        };
        let mut resolved = Vec::with_capacity(fields.len());
        let mut defs = Vec::with_capacity(fields.len());
        for (fname, fty) in fields {
            let arg = self.resolve_garg_env(draft, fty, &subst, site)?;
            defs.push(FieldDef {
                name: draft.intern_string(fname),
                ty: arg.image(),
                required: true,
            });
            resolved.push((fname.clone(), arg));
        }
        let TypeInstId::Record(ty) = id else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id,
                body: TypeInstKind::Struct,
            }
            .into());
        };
        draft.set_record_fields(ty, defs);
        Ok(InstBody::Struct(resolved))
    }

    fn fill_enum_type_body(
        &self,
        draft: &mut ImageDraft,
        template: usize,
        id: TypeInstId,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<InstBody, ResolveError> {
        let template_info = self.template_for_args(template, args)?;
        let subst: Vec<(String, GArg)> = template_info
            .type_params
            .iter()
            .map(|(name, _)| name.clone())
            .zip(args.iter().copied())
            .collect();
        let TemplateBody::Enum(variants) = &template_info.body else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id,
                body: TypeInstKind::Struct,
            }
            .into());
        };
        let enum_name = &template_info.name;
        let mut reported = false;
        let mut resolved = Vec::with_capacity(variants.len());
        let mut defs = Vec::with_capacity(variants.len());
        for variant in variants {
            let mut payload = Vec::with_capacity(variant.payload.len());
            let mut leaves = Vec::with_capacity(variant.payload.len());
            for field in &variant.payload {
                let arg = self.resolve_garg_env(draft, &field.ty, &subst, site)?;
                // The image admits a bare scalar, record, or enum as an enum
                // payload leaf; a collection is not a payload type. Reject at the
                // mint so a checker-clean program can never emit an image the
                // verifier rejects at the Table phase.
                if let GArg::Collection(coll) = arg
                    && !reported
                {
                    self.record_collection_payload_rejection(site, enum_name, &variant.name, coll);
                    reported = true;
                }
                leaves.push(arg.image());
                payload.push((field.name.clone(), arg));
            }
            defs.push(VariantDef {
                name: draft.intern_string(&variant.name),
                category: false,
                payload: leaves,
            });
            resolved.push(InstVariant {
                name: variant.name.clone(),
                payload,
            });
        }
        let TypeInstId::Enum(enum_id) = id else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id,
                body: TypeInstKind::Enum,
            }
            .into());
        };
        draft.set_enum_variants(enum_id, defs);
        Ok(InstBody::Enum(resolved))
    }

    fn record_active_dependency(&self, dependency: usize) {
        let mut generics = self.generics.borrow_mut();
        let Some(&dependent) = generics.fill_stack.last() else {
            return;
        };
        let dependency_is_provisional = generics
            .type_insts
            .get(dependency)
            .is_some_and(|inst| matches!(inst.state, TypeInstState::Filling { .. }));
        if dependent == dependency || !dependency_is_provisional {
            return;
        }
        if let Some(inst) = generics.type_insts.get_mut(dependency) {
            inst.dependents.push(dependent);
        }
    }

    fn record_semantic_dependencies(&self, dependent: usize, args: impl IntoIterator<Item = GArg>) {
        let mut pending: Vec<GArg> = args.into_iter().collect();
        let mut dependency_ids = Vec::new();
        while let Some(arg) = pending.pop() {
            match arg {
                GArg::Struct(ty) => dependency_ids.push(TypeInstId::Record(ty)),
                GArg::Enum(id) => dependency_ids.push(TypeInstId::Enum(id)),
                GArg::Collection(index) => match self.collection_spec(index) {
                    CollSpec::List { elem } => pending.push(elem),
                    CollSpec::Map { key, value } => {
                        pending.push(key);
                        pending.push(value);
                    }
                },
                GArg::Scalar(_) | GArg::Nominal(_) | GArg::Group(_) | GArg::Param(_) => {}
            }
        }
        let mut generics = self.generics.borrow_mut();
        for dependency_id in dependency_ids {
            let Some(&dependency) = generics.fill_rows.get(&dependency_id.into()) else {
                continue;
            };
            let dependency_is_provisional = generics
                .type_insts
                .get(dependency)
                .is_some_and(|inst| matches!(inst.state, TypeInstState::Filling { .. }));
            if dependent != dependency && dependency_is_provisional {
                generics.type_insts[dependency].dependents.push(dependent);
            }
        }
    }

    fn record_inst_body_dependencies(&self, dependent: usize, body: &InstBody) {
        let args: Vec<GArg> = match body {
            InstBody::Struct(fields) => fields.iter().map(|(_, arg)| *arg).collect(),
            InstBody::Enum(variants) => variants
                .iter()
                .flat_map(|variant| variant.payload.iter().map(|(_, arg)| *arg))
                .collect(),
        };
        self.record_semantic_dependencies(dependent, args);
    }

    fn strengthen_refusal(
        refusals: &mut [Option<ResolveRefusal>],
        offset: FillOffset,
        incoming: ResolveRefusal,
    ) -> Option<ResolveRefusal> {
        let slot = &mut refusals[offset.0];
        let joined = slot.map_or(incoming, |current| current.join(incoming));
        if *slot == Some(joined) {
            None
        } else {
            *slot = Some(joined);
            Some(joined)
        }
    }

    /// Publish one prevalidated staged body. The helper remains typed even though
    /// settlement validates the complete plan first, so a hostile internal caller
    /// cannot silently leave a row provisional or publish an incoherent body.
    fn commit_ready_state(&self, inst: &mut TypeInst) -> Result<(), ResolveError> {
        let TypeInstState::Filling { staged } = &inst.state else {
            return Err(GenericInvariant::CacheState(
                GenericCacheInvariant::StableRowInActiveBatch,
            )
            .into());
        };
        let Some(body) = staged.as_ref() else {
            return Err(GenericInvariant::CacheState(
                GenericCacheInvariant::IncompleteRowWithoutRefusal,
            )
            .into());
        };
        self.validate_inst_body_metadata(inst.template, &inst.args, inst.id, body)?;

        let body = match &mut inst.state {
            TypeInstState::Filling { staged } => staged.take().ok_or({
                ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant::IncompleteRowWithoutRefusal,
                ))
            })?,
            TypeInstState::Ready(_) | TypeInstState::Rejected(_) => {
                return Err(GenericInvariant::CacheState(
                    GenericCacheInvariant::StableRowInActiveBatch,
                )
                .into());
            }
        };
        inst.state = TypeInstState::Ready(body);
        Ok(())
    }

    fn settle_fill_batch(&self) -> Result<(), ResolveError> {
        let mut generics = self.generics.borrow_mut();
        let Some(start) = generics.fill_batch_start else {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::ActiveBatchMissing,
            )));
        };
        let end = generics.type_insts.len();
        let Some(active_len) = end.checked_sub(start) else {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::ActiveBatchRange,
            )));
        };
        if !generics.fill_stack.is_empty() {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::ActiveFillStackNotEmpty,
            )));
        }
        if generics.fill_rows.len() != active_len {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::ActiveRowCardinality,
            )));
        }
        if !generics.fill_rows.iter().all(|(key, index)| {
            (*index >= start)
                && (*index < end)
                && generics
                    .type_insts
                    .get(*index)
                    .is_some_and(|inst| TypeInstKey::from(inst.id) == *key)
        }) {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::ActiveRowKeyMismatch,
            )));
        }
        if generics
            .fill_failures
            .iter()
            .any(|(index, _)| *index < start || *index >= end)
        {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::FailureIndexOutOfRange,
            )));
        }

        let mut refusals = vec![None; active_len];
        let mut pending = VecDeque::new();
        for &(index, refusal) in &generics.fill_failures {
            let offset = FillOffset(index - start);
            if let Some(refusal) = Self::strengthen_refusal(&mut refusals, offset, refusal) {
                pending.push_back(PendingRefusal { offset, refusal });
            }
        }
        for (offset, inst) in generics.type_insts[start..].iter().enumerate() {
            let TypeInstState::Filling { staged } = &inst.state else {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant::StableRowInActiveBatch,
                )));
            };
            if inst
                .dependents
                .iter()
                .any(|dependent| *dependent < start || *dependent >= end)
            {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant::DependentIndexOutOfRange,
                )));
            }
            if staged.is_none() && refusals[offset].is_none() {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant::IncompleteRowWithoutRefusal,
                )));
            }
        }

        while let Some(work) = pending.pop_front() {
            // An earlier weaker update may still be queued after this row has joined
            // a stronger refusal. Only the current lattice value traverses edges.
            if refusals[work.offset.0] != Some(work.refusal) {
                continue;
            }
            for &dependent in &generics.type_insts[start + work.offset.0].dependents {
                let offset = FillOffset(dependent - start);
                if let Some(refusal) = Self::strengthen_refusal(&mut refusals, offset, work.refusal)
                {
                    pending.push_back(PendingRefusal { offset, refusal });
                }
            }
        }

        // Validate the complete commit plan before moving any body.
        for (offset, inst) in generics.type_insts[start..].iter().enumerate() {
            let TypeInstState::Filling { staged } = &inst.state else {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant::StableRowInActiveBatch,
                )));
            };
            if refusals[offset].is_none() {
                let Some(body) = staged.as_ref() else {
                    return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                        GenericCacheInvariant::IncompleteRowWithoutRefusal,
                    )));
                };
                self.validate_inst_body_metadata(inst.template, &inst.args, inst.id, body)?;
            }
        }

        // Every coherence check and refusal propagation above is read-only with
        // respect to the owner. Move state only after the whole batch is validated.
        for (inst, refusal) in generics.type_insts[start..].iter_mut().zip(refusals) {
            if let Some(refusal) = refusal {
                inst.state = TypeInstState::Rejected(refusal);
            } else {
                self.commit_ready_state(inst)?;
            }
        }
        generics.fill_batch_start = None;
        generics.fill_rows.clear();
        generics.fill_failures = Vec::new();
        for inst in &mut generics.type_insts[start..] {
            inst.dependents = Vec::new();
        }
        Ok(())
    }

    fn settled_type_result<R: ReadyInstanceRequirement>(
        &self,
        index: usize,
        id: TypeInstId,
        requirement: R,
    ) -> Result<TypeInstId, ResolveError> {
        let generics = self.generics.borrow();
        let Some(inst) = generics.type_insts.get(index) else {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::SettledRowMissing,
            )));
        };
        match &inst.state {
            TypeInstState::Ready(_) => {}
            TypeInstState::Rejected(refusal) => {
                return Err(ResolveError::Refusal(*refusal));
            }
            TypeInstState::Filling { .. } => {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant::SettledRowStillFilling,
                )));
            }
        }
        drop(generics);
        let view = self.metadata_view();
        let Some(inst) = view.generics.type_insts.get(index) else {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::SettledRowMissing,
            )));
        };
        let mut metadata = MetadataScratch::try_new(&view)?;
        let body = view
            .ready_inst_header_with(inst, &mut metadata)?
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        self.validate_ready_requirement(inst, body, requirement)?;
        view.validate_ready_body_with(inst, body, &mut metadata)?;
        Ok(id)
    }

    fn record_limit(&self, site: MintSite<'_>, subject: &str) {
        let mut generics = self.generics.borrow_mut();
        if matches!(generics.limit, LimitState::Open) {
            generics.limit = LimitState::Pending(SourceDiagnostic::at(
                Code::CheckInstantiationLimit.as_str(),
                site.file,
                site.span,
                format!(
                    "monomorphizing this program requires more than {MAX_INSTANTIATIONS} generic \
                     instantiations; {subject}"
                ),
            ));
        }
    }

    /// Record the mint-time rejection of a collection payload leaf at the construction
    /// or annotation site. The instantiation still fills its body so the shared
    /// instance cache stays consistent; the non-empty pending queue makes the driver
    /// reject before the image is encoded, so the collection leaf never reaches the
    /// verifier.
    fn record_collection_payload_rejection(
        &self,
        site: MintSite<'_>,
        enum_name: &str,
        variant_name: &str,
        coll: u16,
    ) {
        let kind = match self.collection_spec(coll) {
            CollSpec::List { .. } => "List",
            CollSpec::Map { .. } => "Map",
        };
        self.generics
            .borrow_mut()
            .collection_payloads
            .push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                site.file,
                site.span,
                format!(
                    "the `{variant_name}` payload of `{enum_name}` is a `{kind}` value. An enum \
                 member payload is a bare scalar, a struct, or another enum; a collection is not a \
                 payload type. Declare a struct that holds the collection and use that struct as \
                 the payload."
                ),
            ));
    }

    /// The template index and concrete arguments a minted type instantiation came
    /// from, if `id` names one. Used by generic-function inference to unify a
    /// parameter type `Pair<T, U>` against an argument's instantiation.
    #[cfg(test)]
    pub(crate) fn instantiation_of(
        &self,
        id: TypeInstId,
    ) -> Result<Option<(usize, Vec<GArg>)>, GenericInvariant> {
        let view = self.metadata_view();
        let mut metadata = MetadataScratch::try_new(&view)?;
        let Some((inst, _)) = view.ready_inst_header_by_id(id, &mut metadata)? else {
            return Ok(None);
        };
        Ok(Some((inst.template, inst.args.clone())))
    }

    /// The resolved member shape of a minted type instantiation, if `id` names one.
    pub(crate) fn type_inst_body(
        &self,
        id: TypeInstId,
    ) -> Result<Option<InstBody>, GenericInvariant> {
        let view = self.metadata_view();
        let mut metadata = MetadataScratch::try_new(&view)?;
        Ok(view
            .ready_inst_by_id(id, &mut metadata)?
            .map(|(_, body)| body.clone()))
    }

    /// The `Option<T>` argument an enum instantiation carries, if it is the reserved
    /// `Option` template's.
    #[cfg(test)]
    pub(crate) fn as_option(&self, id: EnumId) -> Result<Option<GArg>, GenericInvariant> {
        self.reserved_enum_args(id).map(|args| match args {
            Some(ReservedEnumArgs::Option(inner)) => Some(inner),
            Some(ReservedEnumArgs::Result(_, _) | ReservedEnumArgs::Other) | None => None,
        })
    }

    /// The `Result<T, E>` arguments an enum instantiation carries, if it is the
    /// reserved `Result` template's.
    #[cfg(test)]
    pub(crate) fn as_result(&self, id: EnumId) -> Result<Option<(GArg, GArg)>, GenericInvariant> {
        self.reserved_enum_args(id).map(|args| match args {
            Some(ReservedEnumArgs::Result(ok, err)) => Some((ok, err)),
            Some(ReservedEnumArgs::Option(_) | ReservedEnumArgs::Other) | None => None,
        })
    }

    /// Classify one Ready reserved enum through one immutable metadata snapshot.
    pub(crate) fn reserved_enum_args(
        &self,
        id: EnumId,
    ) -> Result<Option<ReservedEnumArgs>, GenericInvariant> {
        self.with_metadata_session(|session| session.reserved_instantiation(id))
    }

    /// The variants (name plus resolved payload types) of an enum value, whether a
    /// concrete user `enum` or a generic enum instantiation, for `match` lowering.
    pub(crate) fn enum_variants(
        &self,
        id: EnumId,
    ) -> Result<Option<ResolvedEnumVariants>, GenericInvariant> {
        match self.type_inst_body(TypeInstId::Enum(id))? {
            Some(InstBody::Enum(variants)) => Ok(Some(
                variants
                    .into_iter()
                    .map(|variant| {
                        (
                            variant.name,
                            variant.payload.into_iter().map(|(_, arg)| arg).collect(),
                        )
                    })
                    .collect(),
            )),
            Some(InstBody::Struct(_)) => Err(GenericInvariant::TypeBodyKindMismatch {
                id: TypeInstId::Enum(id),
                body: TypeInstKind::Struct,
            }),
            None => Ok(self.enum_by_id(id).map(|info| {
                info.variants
                    .iter()
                    .map(|variant| {
                        (
                            variant.name.clone(),
                            variant
                                .payload
                                .iter()
                                .map(|field| GArg::Scalar(field.scalar))
                                .collect(),
                        )
                    })
                    .collect()
            })),
        }
    }

    /// The durable-ledger anchor spelling of an enum value: a concrete user `enum`
    /// by its declared name, and a generic enum instantiation (`Option`, `Result`, a
    /// user generic) by its space-free `Name[arg,...]` spelling. Space-free so the
    /// result is a valid `marrow.ids` anchor path (printable ASCII, no spaces). The
    /// spelling is stable across appending an enum member, so an append preserves the
    /// sum anchor while minting only the new member.
    ///
    /// The bracket, space-free-comma recursion below is deliberately independent of
    /// the angle-form display owner ([`inst_spelling`](Self::inst_spelling) and its
    /// family): the two never call each other, so changing a user-facing diagnostic
    /// delimiter can never move an opaque durable identity byte. The near-duplication
    /// is the isolation boundary, not accidental repetition.
    #[cfg(test)]
    pub(crate) fn enum_anchor_spelling(
        &self,
        id: EnumId,
    ) -> Result<Option<String>, GenericInvariant> {
        match self.inst_anchor_spelling(TypeInstId::Enum(id))? {
            Some(spelling) => Ok(Some(spelling)),
            None => Ok(self.enum_by_id(id).map(|info| info.name.clone())),
        }
    }

    /// Validate all durable resource leaves through one metadata view and one
    /// breadth-first expansion. A shared value is expanded at its shortest depth,
    /// so deduplication cannot hide descendants that remain inside the image's
    /// durable-value depth bound.
    #[cfg(test)]
    pub(crate) fn validate_durable_value_metadata(
        &self,
        roots: impl IntoIterator<Item = GArg>,
    ) -> Result<(), GenericInvariant> {
        self.with_metadata_session(|session| session.validate_durable_value_metadata(roots))
    }

    /// The durable-anchor spelling of a generic instantiation, `Name[arg,arg]` with a
    /// space-free comma, or `None` if `id` names no instantiation. The opaque-ledger
    /// twin of [`inst_spelling`](Self::inst_spelling); it never calls the display
    /// family.
    #[cfg(test)]
    fn inst_anchor_spelling(&self, id: TypeInstId) -> Result<Option<String>, GenericInvariant> {
        let view = self.metadata_view();
        let mut metadata = MetadataScratch::try_new(&view)?;
        let Some((_, _)) = view.ready_inst_header_by_id(id, &mut metadata)? else {
            return Ok(None);
        };
        let mut display = DisplayScratch::for_view(&view);
        self.inst_anchor_spelling_validated(&view, &metadata, id, &mut display)
    }

    fn inst_anchor_spelling_validated(
        &self,
        view: &TypeMetadataView<'_>,
        metadata: &MetadataScratch,
        id: TypeInstId,
        display: &mut DisplayScratch,
    ) -> Result<Option<String>, GenericInvariant> {
        let Some(row) = metadata.row(id) else {
            return Ok(None);
        };
        let inst = &view.generics.type_insts[row];
        if !matches!(inst.state, TypeInstState::Ready(_)) {
            return Ok(None);
        }
        let arg = match id {
            TypeInstId::Record(id) => GArg::Struct(id),
            TypeInstId::Enum(id) => GArg::Enum(id),
        };
        render_validated_anchor_arg(self, view, metadata, arg, display).map(Some)
    }

    /// The source spelling of a generic type instantiation, `Name<arg, ...>`, if
    /// `id` names one. The canonical angle-form display owner for diagnostics and
    /// cycle labels; durable identity uses [`enum_anchor_spelling`](Self::enum_anchor_spelling).
    pub(crate) fn inst_spelling(&self, id: TypeInstId) -> Option<String> {
        let view = self.metadata_view();
        let mut display = DisplayScratch::for_view(&view);
        inst_spelling_for_display(self, &view, id, None, &mut display)
            .ok()
            .flatten()
    }

    fn inst_spelling_validated(
        &self,
        view: &TypeMetadataView<'_>,
        metadata: &MetadataScratch,
        id: TypeInstId,
        display: &mut DisplayScratch,
    ) -> Result<Option<String>, GenericInvariant> {
        let arg = match id {
            TypeInstId::Record(id) => GArg::Struct(id),
            TypeInstId::Enum(id) => GArg::Enum(id),
        };
        let row = metadata
            .row(id)
            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
        let inst = view
            .generics
            .type_insts
            .get(row)
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        if matches!(inst.state, TypeInstState::Filling { .. }) {
            return Ok(None);
        }
        render_validated_display_arg(self, view, metadata, arg, display).map(Some)
    }

    /// Set the base image function index for generic function instantiations, once
    /// every monomorphic function and test has consumed its index.
    pub(crate) fn set_fn_base(&self, base: u16) {
        self.generics.borrow_mut().fn_base = base;
    }

    /// Reserve the image function index for `(fn template, args)`, minting and
    /// enqueuing a fresh instance on first request and reusing it thereafter. A shared
    /// bound refusal records the first coherent mint site and returns `Err(Limit)`.
    pub(crate) fn reserve_fn_instance(
        &self,
        template: usize,
        args: Vec<GArg>,
        site: MintSite<'_>,
    ) -> Result<u16, ResolveError> {
        self.validate_type_arguments(&args)?;
        let mut generics = self.generics.borrow_mut();
        // Reservation-dedup reuse probe: a keyed lookup into the append-only secondary
        // index. The reserved image function index is read from the named row (the
        // authority), and a row that does not carry the looked-up key is drift.
        #[cfg(test)]
        bump_scaling(|counts| counts.fn_inst_scan_steps += 1);
        if let Some(&row) = generics.fn_index.get(&(template, args.clone())) {
            let reused = generics
                .fn_insts
                .get(row)
                .filter(|inst| inst.template == template && inst.args == args);
            let Some(inst) = reused else {
                return Err(
                    GenericInvariant::CacheState(GenericCacheInvariant::MintIndexDrift).into(),
                );
            };
            return Ok(inst.func);
        }
        if generics.type_insts.len() + generics.fn_insts.len() >= MAX_INSTANTIATIONS {
            drop(generics);
            self.record_limit(
                site,
                "a generic function likely recurses over an ever-growing type",
            );
            return Err(ResolveRefusal::Limit.into());
        }
        let row = generics.fn_insts.len();
        let func = generics.fn_base + row as u16;
        let inst = FnInst {
            template,
            args,
            func,
        };
        // Keep the lookup-only reuse index in lockstep with its authority; a reserve
        // only appends on a dedup miss, so this key is new.
        let displaced = generics
            .fn_index
            .insert((inst.template, inst.args.clone()), row);
        debug_assert!(displaced.is_none(), "fn reserve index key already present");
        generics.fn_insts.push(inst.clone());
        generics.fn_queue.push_back(inst);
        Ok(func)
    }

    /// The next generic function instance awaiting body lowering: its template index,
    /// concrete arguments, and reserved image function index.
    pub(crate) fn next_fn_pending(&self) -> Option<(usize, Vec<GArg>, u16)> {
        self.generics
            .borrow_mut()
            .fn_queue
            .pop_front()
            .map(|inst| (inst.template, inst.args, inst.func))
    }

    /// Drain the one owner-ordered generic outcome. Taking a pending limit advances
    /// its owner to `Reported`, so cached `Rejected(Limit)` rows replay silently.
    pub(crate) fn take_generic_diagnostics(&self) -> GenericDiagnostics {
        let mut generics = self.generics.borrow_mut();
        let limit = match std::mem::replace(&mut generics.limit, LimitState::Reported) {
            LimitState::Open => {
                generics.limit = LimitState::Open;
                None
            }
            LimitState::Pending(diagnostic) => Some(diagnostic),
            LimitState::Reported => None,
        };
        GenericDiagnostics {
            limit,
            payloads: std::mem::take(&mut generics.collection_payloads),
        }
    }

    pub(crate) fn has_instantiation_limit(&self) -> bool {
        !matches!(self.generics.borrow().limit, LimitState::Open)
    }

    pub(crate) fn adopt_generic_diagnostics(&self, outcome: GenericDiagnostics) {
        let GenericDiagnostics {
            limit,
            mut payloads,
        } = outcome;
        let mut generics = self.generics.borrow_mut();
        if matches!(generics.limit, LimitState::Open)
            && let Some(diagnostic) = limit
        {
            generics.limit = LimitState::Pending(diagnostic);
        }
        generics.collection_payloads.append(&mut payloads);
    }

    /// The image COLLTYPES index of `List[elem]`, minting it into `draft` on first
    /// use and reusing it thereafter. Dedup is by the *source* element type, so
    /// `List[Age]` and `List[int]` stay distinct rows even though both erase to
    /// `List[int]` in the image.
    pub(crate) fn instantiate_list(
        &self,
        draft: &mut ImageDraft,
        elem: GArg,
    ) -> Result<u16, ResolveError> {
        self.instantiate_collection(draft, CollSpec::List { elem })
    }

    /// Reject a non-key argument only after proving that its metadata is coherent.
    /// Scalars and existing nominal keys take the allocation-free fast path;
    /// malformed metadata remains an invariant rather than becoming a semantic
    /// refusal.
    pub(crate) fn check_map_key_admissibility(&self, key: GArg) -> Result<(), ResolveError> {
        match key {
            GArg::Scalar(_) => return Ok(()),
            GArg::Nominal(id) => {
                return if self.nominals.get(id.0 as usize).is_some() {
                    Ok(())
                } else {
                    Err(GenericInvariant::TypeArgumentTargetMissing(key).into())
                };
            }
            GArg::Struct(_)
            | GArg::Group(_)
            | GArg::Enum(_)
            | GArg::Collection(_)
            | GArg::Param(_) => {}
        }
        self.validate_type_arguments(&[key])?;
        Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
    }

    /// The image COLLTYPES index of `Map[key, value]`, minting it on first use and
    /// reusing it thereafter, deduped by source key/value types.
    pub(crate) fn instantiate_map(
        &self,
        draft: &mut ImageDraft,
        key: GArg,
        value: GArg,
    ) -> Result<u16, ResolveError> {
        self.check_map_key_admissibility(key)?;
        self.instantiate_collection(draft, CollSpec::Map { key, value })
    }

    fn instantiate_collection(
        &self,
        draft: &mut ImageDraft,
        spec: CollSpec,
    ) -> Result<u16, ResolveError> {
        match spec {
            CollSpec::List { elem } => self.validate_type_arguments(&[elem])?,
            CollSpec::Map { key, value } => self.validate_type_arguments(&[key, value])?,
        }
        let kind = spec.kind();
        let collections = self.collections.borrow();
        let cache_index = collections.len();
        let draft_index = draft.collection_type_count();
        if cache_index != draft_index {
            return Err(ResolveError::Invariant(
                GenericInvariant::CollectionIndexMismatch {
                    kind,
                    cache_index,
                    draft_index,
                },
            ));
        }
        if let Some(index) = collections.iter().position(|candidate| *candidate == spec) {
            return Ok(index as u16);
        }
        drop(collections);

        let id = draft.add_collection_type(spec.definition());
        debug_assert_eq!(id.index() as usize, cache_index);
        let mut collections = self.collections.borrow_mut();
        debug_assert_eq!(collections.len(), cache_index);
        collections.push(spec);
        Ok(id.index())
    }

    /// The source element/key/value spec of a minted collection instantiation.
    pub(crate) fn collection_spec(&self, idx: u16) -> CollSpec {
        self.collections.borrow()[idx as usize]
    }

    /// The source spelling of a collection instantiation (`List<T>` / `Map<K, V>`),
    /// used in diagnostics and cycle labels. The canonical angle-form display owner.
    pub(crate) fn collection_spelling(&self, idx: u16) -> String {
        let view = self.metadata_view();
        let mut display = DisplayScratch::for_view(&view);
        collection_spelling_for_display(self, &view, idx, None, None, &mut display)
            .unwrap_or_else(|_| "collection".to_string())
    }

    pub(crate) fn by_name(&self, name: &str) -> Option<&RecordInfo> {
        self.records.iter().find(|info| info.name == name)
    }

    pub(crate) fn struct_by_name(&self, name: &str) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.name == name)
    }

    pub(crate) fn struct_by_type(&self, ty: TypeId) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.type_id == ty)
    }

    pub(crate) fn enum_by_name(&self, name: &str) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.name == name)
    }

    pub(crate) fn enum_by_id(&self, id: EnumId) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.enum_id == id)
    }

    pub(crate) fn nominal_by_name(&self, name: &str) -> Option<(NominalId, &NominalInfo)> {
        self.nominals
            .iter()
            .position(|info| info.name == name)
            .map(|index| (NominalId(index as u16), &self.nominals[index]))
    }

    pub(crate) fn nominal(&self, id: NominalId) -> &NominalInfo {
        &self.nominals[id.0 as usize]
    }

    /// The alias-free form of a type annotation: every name that is an alias is
    /// replaced by its expanded target, structurally, so classification reads
    /// only scalar spellings and declared type names. Diagnostics stay on the
    /// caller's annotation span — expansion carries the alias target's spans,
    /// which point at another declaration.
    pub(crate) fn expand(&self, ty: &TypeExpr) -> TypeExpr {
        match ty {
            TypeExpr::Name { text, .. } => match self.aliases.get(text) {
                Some(target) => target.clone(),
                None => ty.clone(),
            },
            TypeExpr::Optional { inner, span } => TypeExpr::Optional {
                inner: Box::new(self.expand(inner)),
                span: *span,
            },
            TypeExpr::Apply { head, args, span } => TypeExpr::Apply {
                head: head.clone(),
                args: args.iter().map(|arg| self.expand(arg)).collect(),
                span: *span,
            },
            TypeExpr::Identity(_) => ty.clone(),
        }
    }

    /// Build the registry: the alias table (duplicates, resource-name collisions,
    /// and cycles rejected; targets pre-expanded to alias-free form and validated
    /// against the known types), then the value types in two passes.
    ///
    /// Value types (the resource records, the dense structs, and the closed
    /// enums) are built declare-then-fill: pass one reserves every type's image
    /// index with empty members and decides name conflicts, so pass two can resolve
    /// each field or payload against the full set of declared types regardless of
    /// declaration order — a struct field may name a later struct or enum, two
    /// structs may reference each other, and a resource field may name a user enum.
    /// The only nesting restriction is acyclicity: a value type may not contain
    /// itself directly or transitively, reported at check time (and independently
    /// re-rejected by the verifier). The resource records reserve their image
    /// indices before the structs, so a project's durable root and sites keep the
    /// same record index whether or not dense structs are also declared.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        draft: &mut ImageDraft,
        aliases: &[(String, &AliasDecl)],
        nominals: &[(String, &NominalDecl)],
        structs: &[(String, &StructDecl)],
        enums: &[(String, &EnumDecl)],
        resources: &[(String, &ResourceDecl)],
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Self {
        let mut registry = Self {
            aliases: build_alias_table(aliases, resources, structs, enums, diagnostics),
            nominals: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            records: Vec::new(),
            type_templates: reserved_templates(),
            generics: RefCell::default(),
            collections: RefCell::default(),
        };
        registry.nominals =
            build_nominals(&registry, nominals, resources, structs, enums, diagnostics);

        // A generic `struct`/`enum` (one carrying type parameters) is a template
        // monomorphized on use, not a concrete image type; the concrete declarations
        // are declared-then-filled below, the templates registered aside.
        let concrete_structs: Vec<(String, &StructDecl)> = structs
            .iter()
            .filter(|(_, decl)| decl.type_params.is_empty())
            .map(|(file, decl)| (file.clone(), *decl))
            .collect();
        let concrete_enums: Vec<(String, &EnumDecl)> = enums
            .iter()
            .filter(|(_, decl)| decl.type_params.is_empty())
            .map(|(file, decl)| (file.clone(), *decl))
            .collect();
        register_type_templates(&mut registry, structs, enums, resources, diagnostics);

        // Pass one: reserve every value type's image index with empty members and
        // decide name conflicts. The records reserve first (image indices `0..n`),
        // so a project's durable root and sites keep the same record index whether
        // or not dense structs are also declared.
        let record_decls = declare_records(draft, &mut registry, resources, diagnostics);
        let struct_decls = declare_structs(
            draft,
            &mut registry,
            &concrete_structs,
            resources,
            diagnostics,
        );
        let enum_decls = declare_enums(
            draft,
            &mut registry,
            &concrete_enums,
            resources,
            diagnostics,
        );

        // Pass two: resolve and fill each definition's members against the full
        // registry, monomorphizing any generic field type on first use.
        let result = fill_records(draft, &mut registry, &record_decls, diagnostics)
            .and_then(|()| fill_structs(draft, &mut registry, &struct_decls, diagnostics));
        match result {
            Ok(()) => {
                fill_enums(draft, &mut registry, &enum_decls, diagnostics);
                validate_alias_targets(&registry, aliases, diagnostics);
            }
            Err(invariant) => registry.generics.get_mut().build_invariant = Some(invariant),
        }
        registry
    }

    pub(crate) fn build_invariant(&self) -> Option<GenericInvariant> {
        self.generics.borrow().build_invariant
    }

    /// A stable semantic copy of this registry, including the collection and
    /// built-in-generic instantiation tables. The once-checked template pass of a generic function
    /// runs against this clone paired with a clone of the in-progress
    /// [`ImageDraft`], so it sees every already-minted collection/enum at the same
    /// index the real image records. Stable type rows and function reservations are
    /// copied without reindexing, so the shared bound has the same headroom; any new
    /// type instantiation over abstract parameters lands only in the discarded clone.
    /// The clone starts a fresh diagnostic/fill owner, and the pass discards its
    /// emitted code, so nothing crosses back except the consumed diagnostic outcome.
    pub(crate) fn clone_for_generic_check(&self) -> Result<Self, GenericInvariant> {
        let generics = self
            .generics
            .try_borrow()
            .map_err(|_| GenericInvariant::ProofClone(ProofCloneError::UnstableFillState))?;
        let collections = self
            .collections
            .try_borrow()
            .map_err(|_| GenericInvariant::ProofClone(ProofCloneError::UnstableFillState))?;
        let has_unstable_row = generics.type_insts.iter().any(|inst| {
            matches!(inst.state, TypeInstState::Filling { .. }) || !inst.dependents.is_empty()
        });
        if generics.fill_batch_start.is_some()
            || !generics.fill_rows.is_empty()
            || !generics.fill_stack.is_empty()
            || !generics.fill_failures.is_empty()
            || has_unstable_row
            || generics.build_invariant.is_some()
        {
            return Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState,
            ));
        }
        if !matches!(generics.limit, LimitState::Open) {
            return Err(GenericInvariant::ProofClone(
                ProofCloneError::LimitOwnerNotOpen,
            ));
        }
        let view = TypeMetadataView {
            registry: self,
            generics: Ref::clone(&generics),
            collections: Ref::clone(&collections),
        };
        #[cfg(test)]
        bump_scaling(|counts| counts.proof_clones += 1);
        let mut scratch = MetadataScratch::try_new(&view)?;
        for inst in &generics.type_insts {
            #[cfg(test)]
            bump_scaling(|counts| counts.proof_clone_rows += 1);
            view.ready_inst_body_with(inst, &mut scratch)?;
        }
        drop(view);
        let clone = Self {
            aliases: self.aliases.clone(),
            nominals: self.nominals.clone(),
            structs: self.structs.clone(),
            enums: self.enums.clone(),
            records: self.records.clone(),
            type_templates: self.type_templates.clone(),
            generics: RefCell::new(Monomorph {
                type_insts: generics.type_insts.clone(),
                type_index: generics.type_index.clone(),
                fn_base: generics.fn_base,
                fn_insts: generics.fn_insts.clone(),
                fn_index: generics.fn_index.clone(),
                fn_queue: generics.fn_queue.clone(),
                fill_batch_start: None,
                fill_rows: BTreeMap::new(),
                fill_stack: Vec::new(),
                fill_failures: Vec::new(),
                limit: LimitState::Open,
                collection_payloads: Vec::new(),
                build_invariant: None,
                argument_domain: ArgumentDomain::TemplateProof,
            }),
            collections: RefCell::new(collections.clone()),
        };
        Ok(clone)
    }
}

/// The reserved toolchain generic templates, in fixed order (`Option` then
/// `Result`), registered before any user template. They are ordinary generic enums
/// defined here rather than by user source: the `some`/`none`/`ok`/`err` payload
/// leaves reference the templates' own type parameters, so instantiation
/// monomorphizes them exactly like a user generic enum, and the lowerer recovers
/// their reserved constructor/`try`/spelling behavior from the minting template.
fn reserved_templates() -> Vec<TypeTemplate> {
    let param = |name: &str| TypeExpr::Name {
        text: name.to_string(),
        span: SourceSpan::default(),
    };
    let payload = |ty: TypeExpr| TemplatePayload {
        name: "value".to_string(),
        ty,
    };
    vec![
        TypeTemplate {
            name: "Option".to_string(),
            file: String::new(),
            name_span: SourceSpan::default(),
            reserved: Some(Reserved::Option),
            type_params: vec![("T".to_string(), None)],
            body: TemplateBody::Enum(vec![
                TemplateVariant {
                    name: "none".to_string(),
                    payload: Vec::new(),
                },
                TemplateVariant {
                    name: "some".to_string(),
                    payload: vec![payload(param("T"))],
                },
            ]),
        },
        TypeTemplate {
            name: "Result".to_string(),
            file: String::new(),
            name_span: SourceSpan::default(),
            reserved: Some(Reserved::Result),
            type_params: vec![("T".to_string(), None), ("E".to_string(), None)],
            body: TemplateBody::Enum(vec![
                TemplateVariant {
                    name: "ok".to_string(),
                    payload: vec![payload(param("T"))],
                },
                TemplateVariant {
                    name: "err".to_string(),
                    payload: vec![payload(param("E"))],
                },
            ]),
        },
    ]
}

/// Register every generic `struct`/`enum` (one carrying type parameters) as a
/// value-type template, after the reserved toolchain generics. A template mints no
/// concrete image type; a name collision with a scalar, reserved name, alias,
/// nominal, resource, or another declared type is a `check.name_conflict`, and a
/// structurally unadmitted member (a group, key, `required` keyword, optional field,
/// or category/nested enum member) is a `check.unsupported`; a defective template is
/// dropped so no `Name<Args>` use resolves against it.
fn register_type_templates(
    registry: &mut TypeRegistry,
    structs: &[(String, &StructDecl)],
    enums: &[(String, &EnumDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let type_param_names =
        |params: &[marrow_syntax::TypeParamDecl]| -> Vec<(String, Option<TypeConstraint>)> {
            params
                .iter()
                .map(|param| {
                    (
                        param.name.clone(),
                        param.constraint.map(TypeConstraint::from_syntax),
                    )
                })
                .collect()
        };
    let name_taken = |registry: &TypeRegistry, name: &str| -> bool {
        ScalarType::from_spelling(name).is_some()
            || registry.aliases.contains_key(name)
            || registry.nominal_by_name(name).is_some()
            || resources.iter().any(|(_, r)| r.name == name)
            || structs
                .iter()
                .filter(|(_, d)| d.type_params.is_empty())
                .any(|(_, d)| d.name == name)
            || enums
                .iter()
                .filter(|(_, d)| d.type_params.is_empty())
                .any(|(_, d)| d.name == name)
            || registry
                .type_templates
                .iter()
                .any(|template| template.name == name)
    };
    for (file, decl) in structs {
        if decl.type_params.is_empty() {
            continue;
        }
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if name_taken(registry, &decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let Some(fields) = template_struct_fields(file, decl, diagnostics) else {
            continue;
        };
        registry.type_templates.push(TypeTemplate {
            name: decl.name.clone(),
            file: file.clone(),
            name_span: decl.name_span,
            reserved: None,
            type_params: type_param_names(&decl.type_params),
            body: TemplateBody::Struct(fields),
        });
    }
    for (file, decl) in enums {
        if decl.type_params.is_empty() {
            continue;
        }
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if name_taken(registry, &decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let Some(variants) = template_enum_variants(file, decl, diagnostics) else {
            continue;
        };
        registry.type_templates.push(TypeTemplate {
            name: decl.name.clone(),
            file: file.clone(),
            name_span: decl.name_span,
            reserved: None,
            type_params: type_param_names(&decl.type_params),
            body: TemplateBody::Enum(variants),
        });
    }
}

/// The named field-type expressions of a generic struct template, or `None` if any
/// member is not the bare `name: Type` form (matching the concrete-struct rule; the
/// field types themselves are resolved per instantiation).
fn template_struct_fields(
    file: &str,
    decl: &StructDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Vec<(String, TypeExpr)>> {
    let mut fields = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            diagnostics.push(unsupported(file, member.span(), "a struct group"));
            ok = false;
            continue;
        };
        if !field.keys.is_empty() {
            diagnostics.push(unsupported(file, field.span, "a keyed struct field"));
            ok = false;
            continue;
        }
        if field.required {
            diagnostics.push(unsupported(
                file,
                field.span,
                "the `required` keyword on a struct field (struct fields are always required)",
            ));
            ok = false;
            continue;
        }
        if matches!(field.ty, TypeExpr::Optional { .. }) {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "an optional struct field type",
            ));
            ok = false;
            continue;
        }
        fields.push((field.name.clone(), field.ty.clone()));
    }
    ok.then_some(fields)
}

/// The variants (name plus named payload leaves) of a generic enum template, or
/// `None` if any member is a `category` or a nested member (a generic enum is flat;
/// its payload field types are resolved per instantiation).
fn template_enum_variants(
    file: &str,
    decl: &EnumDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Vec<TemplateVariant>> {
    let mut variants = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        if member.category || !member.members.is_empty() {
            diagnostics.push(unsupported(
                file,
                member.span,
                "a category or nested member on a generic enum",
            ));
            ok = false;
            continue;
        }
        variants.push(TemplateVariant {
            name: member.name.clone(),
            payload: member
                .payload
                .iter()
                .map(|field| TemplatePayload {
                    name: field.name.clone(),
                    ty: field.ty.clone(),
                })
                .collect(),
        });
    }
    ok.then_some(variants)
}

/// Resolve the alias declarations to an alias-free name → target map. A
/// duplicate alias name or a collision with a resource name is a
/// `check.name_conflict`; an alias on a cyclic chain is a `check.recursion`
/// and does not enter the map.
fn build_alias_table(
    aliases: &[(String, &AliasDecl)],
    resources: &[(String, &ResourceDecl)],
    structs: &[(String, &StructDecl)],
    enums: &[(String, &EnumDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> BTreeMap<String, TypeExpr> {
    let mut raw: BTreeMap<String, TypeExpr> = BTreeMap::new();
    for (file, decl) in aliases {
        // A parse error blocks compilation before this runs, so a missing target
        // only means the declaration itself was reported; skip it quietly.
        let Some(ty) = &decl.ty else { continue };
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if raw.contains_key(&decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("an alias named `{}` is already declared", decl.name),
            ));
            continue;
        }
        if resources
            .iter()
            .any(|(_, resource)| resource.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a resource", decl.name),
            ));
            continue;
        }
        if structs.iter().any(|(_, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a struct", decl.name),
            ));
            continue;
        }
        if enums.iter().any(|(_, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as an enum", decl.name),
            ));
            continue;
        }
        raw.insert(decl.name.clone(), ty.clone());
    }

    // Reject every alias that can reach itself through the names its target
    // mentions. Reported per alias on the cycle, at its declaration.
    let cyclic: Vec<String> = raw
        .keys()
        .filter(|name| reaches_itself(name, &raw))
        .cloned()
        .collect();
    for name in &cyclic {
        #[expect(
            clippy::expect_used,
            reason = "lowering bookkeeping: `name` was collected from the alias declaration map being searched, so the lookup finds its declaration"
        )]
        let (file, decl) = aliases
            .iter()
            .find(|(_, decl)| &decl.name == name)
            .expect("cyclic alias came from the declaration list");
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckRecursion.as_str(),
            file,
            decl.name_span,
            format!("alias `{name}` is part of a cyclic alias chain"),
        ));
        raw.remove(name);
    }

    // The survivors are acyclic; expand each target to alias-free form.
    let expanded: BTreeMap<String, TypeExpr> = raw
        .keys()
        .map(|name| (name.clone(), expand_in(&raw, &raw[name])))
        .collect();
    expanded
}

/// Whether alias `start` can reach itself by following alias-name references.
fn reaches_itself(start: &str, table: &BTreeMap<String, TypeExpr>) -> bool {
    let mut stack: Vec<&str> = Vec::new();
    referenced_names(&table[start], &mut |name| {
        if table.contains_key(name) {
            stack.push(name);
        }
    });
    let mut visited: Vec<&str> = Vec::new();
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if visited.contains(&node) {
            continue;
        }
        visited.push(node);
        if let Some(target) = table.get(node) {
            referenced_names(target, &mut |name| {
                if table.contains_key(name) {
                    stack.push(name);
                }
            });
        }
    }
    false
}

/// Visit every type name a target mentions. `referenced_names` and `expand_in`
/// walk the same structure; keeping the traversal here keeps them aligned.
fn referenced_names<'t>(ty: &'t TypeExpr, visit: &mut impl FnMut(&'t str)) {
    match ty {
        TypeExpr::Name { text, .. } => visit(text),
        TypeExpr::Optional { inner, .. } => referenced_names(inner, visit),
        TypeExpr::Apply { args, .. } => {
            for arg in args {
                referenced_names(arg, visit);
            }
        }
        TypeExpr::Identity(_) => {}
    }
}

/// Expand a target over an acyclic raw table (build-time twin of
/// [`TypeRegistry::expand`], which reads the finished alias-free map).
fn expand_in(table: &BTreeMap<String, TypeExpr>, ty: &TypeExpr) -> TypeExpr {
    match ty {
        TypeExpr::Name { text, .. } => match table.get(text) {
            Some(target) => expand_in(table, target),
            None => ty.clone(),
        },
        TypeExpr::Optional { inner, span } => TypeExpr::Optional {
            inner: Box::new(expand_in(table, inner)),
            span: *span,
        },
        TypeExpr::Apply { head, args, span } => TypeExpr::Apply {
            head: head.clone(),
            args: args.iter().map(|arg| expand_in(table, arg)).collect(),
            span: *span,
        },
        TypeExpr::Identity(_) => ty.clone(),
    }
}

/// Every alias must denote a known type, used or not: its expansion is a scalar,
/// the record type, or one optional over either. An unknown name is a
/// `check.type` at the alias; a well-formed but unadmitted shape is a
/// `check.unsupported`.
fn validate_alias_targets(
    registry: &TypeRegistry,
    aliases: &[(String, &AliasDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    for (file, decl) in aliases {
        let Some(expanded) = registry.aliases.get(&decl.name) else {
            continue; // duplicate or cyclic: already reported
        };
        let head = match expanded {
            TypeExpr::Optional { inner, .. } => inner.as_ref(),
            other => other,
        };
        match head {
            TypeExpr::Name { text, .. } => {
                if ScalarType::from_spelling(text).is_none()
                    && registry.by_name(text).is_none()
                    && registry.nominal_by_name(text).is_none()
                    && registry.struct_by_name(text).is_none()
                    && registry.enum_by_name(text).is_none()
                {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        file,
                        decl.span,
                        format!("alias `{}` does not name a known type: `{text}`", decl.name),
                    ));
                }
            }
            _ => diagnostics.push(unsupported(
                file,
                decl.span,
                &format!("the target type of alias `{}`", decl.name),
            )),
        }
    }
}

/// Resolve the nominal type declarations against the aliases already installed
/// in `registry`. A name collision with an alias, resource, or earlier nominal
/// is a `check.name_conflict`; a base that does not expand to `int` is a
/// `check.unsupported`; a non-literal, stepped, or empty interval is a
/// `check.type`; the capability list must draw from the closed set without
/// repeats. A declaration with a defect is dropped whole rather than admitted
/// half-checked.
#[allow(clippy::too_many_arguments)]
fn build_nominals(
    registry: &TypeRegistry,
    nominals: &[(String, &NominalDecl)],
    resources: &[(String, &ResourceDecl)],
    structs: &[(String, &StructDecl)],
    enums: &[(String, &EnumDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<NominalInfo> {
    let mut built: Vec<NominalInfo> = Vec::new();
    for (file, decl) in nominals {
        // A parse error blocks compilation before this runs; a missing piece
        // only means the declaration itself was reported, so skip it quietly.
        let (Some(base), Some(interval)) = (&decl.base, &decl.interval) else {
            continue;
        };
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        // Scalar spellings are keywords the parser already rejects as names;
        // owning them here keeps the conflict predicate self-contained.
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || structs.iter().any(|(_, item)| item.name == decl.name)
            || enums.iter().any(|(_, item)| item.name == decl.name)
            || built.iter().any(|info| info.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        match scalar_of(&registry.expand(base)) {
            Some(ScalarType::Int) => {}
            Some(other) => {
                diagnostics.push(unsupported(
                    file,
                    base.span(),
                    &format!("a nominal type over `{}`", other.spelling()),
                ));
                continue;
            }
            None => {
                diagnostics.push(unsupported(file, base.span(), "this nominal base type"));
                continue;
            }
        }
        let Some((lo, hi)) = nominal_interval(file, interval, diagnostics) else {
            continue;
        };
        let Some(supports) = support_set(file, decl, diagnostics) else {
            continue;
        };
        built.push(NominalInfo {
            name: decl.name.clone(),
            lo,
            hi,
            supports,
        });
    }
    built
}

/// Evaluate a nominal `in` range to its inclusive `[lo, hi]` bounds. The range
/// follows the language's range operators — `lo..hi` excludes the end, `lo..=hi`
/// includes it — with int-literal bounds (a leading `-` allowed), no step, and
/// at least one admitted value.
fn nominal_interval(
    file: &str,
    interval: &Expression,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(i64, i64)> {
    let error = |diagnostics: &mut Vec<SourceDiagnostic>, span, message: &str| {
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            file,
            span,
            message.to_string(),
        ));
        None
    };
    let Some(range) = range_expr(interval) else {
        return error(
            diagnostics,
            interval.span(),
            "a nominal interval is a range of int literals, such as `0..150`",
        );
    };
    if range.step.is_some() {
        return error(diagnostics, range.span, "a nominal interval takes no step");
    }
    let (Some(start), Some(end)) = (range.start, range.end) else {
        return error(
            diagnostics,
            range.span,
            "a nominal interval needs both bounds",
        );
    };
    let (Some(lo), Some(end_value)) = (literal_int(start), literal_int(end)) else {
        return error(
            diagnostics,
            range.span,
            "a nominal interval's bounds are int literals",
        );
    };
    // Normalize the end-exclusive spelling to the inclusive upper bound. A
    // literal never spells `i64::MIN`, so the exclusive form always has a
    // representable predecessor; the checked form keeps that self-evident.
    let hi = if range.inclusive_end {
        Some(end_value)
    } else {
        end_value.checked_sub(1)
    };
    match hi {
        Some(hi) if lo <= hi => Some((lo, hi)),
        _ => error(diagnostics, range.span, "this interval admits no values"),
    }
}

/// The value of an int literal, or a negated int literal, or `None`.
fn literal_int(expr: &Expression) -> Option<i64> {
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => crate::lower::parse_int(text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match &**operand {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => crate::lower::parse_int(text).and_then(i64::checked_neg),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a declaration's `supports` spellings against the closed capability
/// set, rejecting an unknown or repeated capability.
fn support_set(
    file: &str,
    decl: &NominalDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<SupportSet> {
    let mut supports = SupportSet::default();
    for spelling in &decl.supports {
        let flag = match spelling.name.as_str() {
            "add" => &mut supports.add,
            "subtract" => &mut supports.subtract,
            "step" => &mut supports.step,
            "scale" => &mut supports.scale,
            other => {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    spelling.span,
                    format!(
                        "unknown capability `{other}`; the capabilities are add, subtract, step, scale"
                    ),
                ));
                return None;
            }
        };
        if *flag {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                spelling.span,
                format!("capability `{}` is repeated", spelling.name),
            ));
            return None;
        }
        *flag = true;
    }
    Some(supports)
}

/// One struct reserved in pass one: the file it was declared in, its declaration,
/// and the image record index it will fill in pass two.
struct ReservedStruct<'a> {
    file: String,
    decl: &'a StructDecl,
    type_id: TypeId,
}

/// Pass one for the dense struct types: reserve each admitted struct's image
/// [`RecordTypeDef`] index (empty for now) and register its name, so pass two may
/// resolve a field that names any other struct or enum. A name collision with a
/// scalar, alias, nominal, resource, or earlier struct is a `check.name_conflict`;
/// a colliding or reserved-name struct is dropped and never reserved.
fn declare_structs<'a>(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    structs: &'a [(String, &StructDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<ReservedStruct<'a>> {
    let mut reserved: Vec<ReservedStruct<'a>> = Vec::new();
    for (file, decl) in structs {
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || registry.struct_by_name(&decl.name).is_some()
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let name_id = draft.intern_string(&decl.name);
        let type_id = draft.add_record_type(RecordTypeDef {
            name: name_id,
            fields: Vec::new(),
        });
        registry.structs.push(StructInfo {
            type_id,
            name: decl.name.clone(),
            fields: Vec::new(),
        });
        reserved.push(ReservedStruct {
            file: file.clone(),
            decl,
            type_id,
        });
    }
    reserved
}

/// Pass two for the dense struct types: resolve each reserved struct's fields
/// against the full registry and fill both the registry info and the image record.
/// A struct field is the bare `name: Type` form over any value type — a scalar,
/// nominal, another struct, or a closed enum (`Option`/`Result`/a user `enum`);
/// a group, keyed field, the `required` keyword, an optional type, or an unknown
/// type is `check.unsupported`. A declaration with a member defect is dropped
/// whole (its reserved image record stays empty and its name leaves the registry)
/// so a later construction or match cannot resolve against a broken struct.
fn fill_structs(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    reserved: &[ReservedStruct<'_>],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<(), GenericInvariant> {
    let mut dropped: Vec<TypeId> = Vec::new();
    for item in reserved {
        match struct_fields(draft, registry, &item.file, item.decl, diagnostics)? {
            Some((fields, field_defs)) => {
                draft.set_record_fields(item.type_id, field_defs);
                if let Some(info) = registry
                    .structs
                    .iter_mut()
                    .find(|info| info.type_id == item.type_id)
                {
                    info.fields = fields;
                }
            }
            None => dropped.push(item.type_id),
        }
    }
    registry
        .structs
        .retain(|info| !dropped.contains(&info.type_id));
    Ok(())
}

/// Resolve a struct's members to its required value fields and their image
/// definitions, or `None` if any member is not the bare `name: Type` form over a
/// value type.
type ResolvedStructFields = (Vec<FieldInfo>, Vec<FieldDef>);

fn struct_fields(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    file: &str,
    decl: &StructDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<Option<ResolvedStructFields>, GenericInvariant> {
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            diagnostics.push(unsupported(file, member.span(), "a struct group"));
            ok = false;
            continue;
        };
        if !field.keys.is_empty() {
            diagnostics.push(unsupported(file, field.span, "a keyed struct field"));
            ok = false;
            continue;
        }
        if field.required {
            diagnostics.push(unsupported(
                file,
                field.span,
                "the `required` keyword on a struct field (struct fields are always required)",
            ));
            ok = false;
            continue;
        }
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "an optional struct field type",
            ));
            ok = false;
            continue;
        }
        let field_ty = match registry.resolve_garg(
            draft,
            &field.ty,
            MintSite {
                file,
                span: field.ty.span(),
            },
        ) {
            Ok(ty) => ty,
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                diagnostics.push(unsupported(file, field.ty.span(), "this struct field type"));
                ok = false;
                continue;
            }
            Err(ResolveError::Refusal(ResolveRefusal::Limit)) => {
                ok = false;
                continue;
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant),
        };
        let field_name_id = draft.intern_string(&field.name);
        field_defs.push(FieldDef {
            name: field_name_id,
            ty: field_ty.image(),
            required: true,
        });
        fields.push(FieldInfo {
            name: field.name.clone(),
            ty: field_ty,
            required: true,
        });
    }
    Ok(ok.then_some((fields, field_defs)))
}

/// One enum reserved in pass one: the file it was declared in, its declaration,
/// and the image ENUMS index it will fill in pass two.
struct ReservedEnum<'a> {
    file: String,
    decl: &'a EnumDecl,
    enum_id: EnumId,
}

/// Pass one for the closed flat enum types: reserve each admitted enum's image
/// [`EnumTypeDef`] index (empty for now) and register its name. A name collision
/// with a scalar, alias, nominal, resource, struct, or earlier enum is a
/// `check.name_conflict`; a colliding or reserved-name enum is dropped and never
/// reserved. Reserving user enums before pass two resolves any field types keeps
/// their image indices ahead of the `Option`/`Result` instantiations minted lazily
/// during field resolution.
fn declare_enums<'a>(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    enums: &'a [(String, &EnumDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<ReservedEnum<'a>> {
    let mut reserved: Vec<ReservedEnum<'a>> = Vec::new();
    for (file, decl) in enums {
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || registry.struct_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || registry.enum_by_name(&decl.name).is_some()
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        if decl.members.len() > marrow_image::bounds::MAX_VARIANTS {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckResourceLimit.as_str(),
                file,
                decl.name_span,
                format!(
                    "an enum declares {} members; the fixed limit is {}",
                    decl.members.len(),
                    marrow_image::bounds::MAX_VARIANTS
                ),
            ));
            continue;
        }
        let name_id = draft.intern_string(&decl.name);
        let enum_id = draft.add_enum_type(EnumTypeDef {
            name: name_id,
            variants: Vec::new(),
        });
        registry.enums.push(EnumInfo {
            enum_id,
            name: decl.name.clone(),
            variants: Vec::new(),
        });
        reserved.push(ReservedEnum {
            file: file.clone(),
            decl,
            enum_id,
        });
    }
    reserved
}

/// Pass two for the closed flat enum types: resolve each reserved enum's variants
/// and fill both the registry info and the image ENUMS entry. Hierarchy is
/// deferred: a `category` member or a member with nested members is
/// `check.unsupported`. A member's payload is the dense `name: Type` form over bare
/// scalars; an optional or non-scalar payload type is `check.unsupported`. A
/// declaration with a defect is dropped whole (its reserved image entry stays empty
/// and its name leaves the registry) so a later match cannot resolve against a
/// broken enum.
fn fill_enums(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    reserved: &[ReservedEnum<'_>],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let mut dropped: Vec<EnumId> = Vec::new();
    for item in reserved {
        match enum_variants(draft, registry, &item.file, item.decl, diagnostics) {
            Some((variants, variant_defs)) => {
                draft.set_enum_variants(item.enum_id, variant_defs);
                if let Some(info) = registry
                    .enums
                    .iter_mut()
                    .find(|info| info.enum_id == item.enum_id)
                {
                    info.variants = variants;
                }
            }
            None => dropped.push(item.enum_id),
        }
    }
    registry
        .enums
        .retain(|info| !dropped.contains(&info.enum_id));
}

/// Resolve an enum's members to its selectable variants and their image
/// definitions, or `None` if any member is unsupported. On the flat line every
/// member is a leaf: a `category` member or one with nested members is deferred.
fn enum_variants(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    file: &str,
    decl: &EnumDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<VariantInfo>, Vec<VariantDef>)> {
    let mut variants = Vec::new();
    let mut variant_defs = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        if member.category {
            diagnostics.push(unsupported(
                file,
                member.span,
                "a `category` enum member (hierarchical enums are deferred)",
            ));
            ok = false;
            continue;
        }
        if !member.members.is_empty() {
            diagnostics.push(unsupported(
                file,
                member.span,
                "a nested enum member (hierarchical enums are deferred)",
            ));
            ok = false;
            continue;
        }
        if seen.contains(&member.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                member.name_span,
                format!("enum member `{}` is already declared", member.name),
            ));
            ok = false;
            continue;
        }
        seen.push(member.name.clone());
        let Some((payload, payload_scalars)) = enum_payload(registry, file, member, diagnostics)
        else {
            ok = false;
            continue;
        };
        let name_id = draft.intern_string(&member.name);
        variant_defs.push(VariantDef {
            name: name_id,
            category: false,
            payload: payload_scalars
                .iter()
                .map(|scalar| ImageType::scalar(scalar.image()))
                .collect(),
        });
        variants.push(VariantInfo {
            name: member.name.clone(),
            payload,
        });
    }
    ok.then_some((variants, variant_defs))
}

/// Resolve one member's payload fields to their scalars and info, or `None` when
/// a field is not the bare `name: scalar` form.
fn enum_payload(
    registry: &TypeRegistry,
    file: &str,
    member: &EnumMember,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<EnumPayloadInfo>, Vec<ScalarType>)> {
    if member.payload.len() > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckResourceLimit.as_str(),
            file,
            member.span,
            format!(
                "an enum member carries {} payload fields; the fixed limit is {}",
                member.payload.len(),
                marrow_image::bounds::MAX_PAYLOAD_FIELDS
            ),
        ));
        return None;
    }
    let mut payload = Vec::new();
    let mut scalars = Vec::new();
    let mut ok = true;
    for field in &member.payload {
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "an optional enum payload field type",
            ));
            ok = false;
            continue;
        }
        let Some(scalar) = scalar_of(&registry.expand(&field.ty)) else {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "this enum payload field type",
            ));
            ok = false;
            continue;
        };
        payload.push(EnumPayloadInfo {
            name: field.name.clone(),
            scalar,
        });
        scalars.push(scalar);
    }
    ok.then_some((payload, scalars))
}

/// Pass one for the admitted record types: reserve each resource's image
/// [`RecordTypeDef`] index (empty for now, ahead of the structs) and register its
/// name, returning the surviving resource declarations for pass two in the same
/// order as [`TypeRegistry::records`]. A reserved resource name, or a name a prior
/// resource already declared, drops that resource with a precise diagnostic; the
/// first declaration of a name stands. The durable graph still admits one store
/// this line, so a second resource is a value record type, never a second root.
fn declare_records<'a>(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    resources: &'a [(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<(String, &'a ResourceDecl)> {
    let mut survivors = Vec::new();
    for (file, resource) in resources {
        if is_reserved_type_name(&resource.name) {
            diagnostics.push(reserved_name(file, resource.name_span, &resource.name));
            continue;
        }
        // Two resources of the same name have no unambiguous record identity, so a
        // repeat is a precise typed rejection and the first declaration stands.
        if registry
            .records
            .iter()
            .any(|info| info.name == resource.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                resource.name_span,
                format!("`{}` is already declared as a resource", resource.name),
            ));
            continue;
        }
        let name_id = draft.intern_string(&resource.name);
        let type_id = draft.add_record_type(RecordTypeDef {
            name: name_id,
            fields: Vec::new(),
        });
        registry.records.push(RecordInfo {
            type_id,
            name: resource.name.clone(),
            fields: Vec::new(),
            groups: Vec::new(),
        });
        survivors.push((file.clone(), *resource));
    }
    survivors
}

/// Pass two for the record types: fill each reserved record from its surviving
/// declaration, in the reserved order.
fn fill_records(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    record_decls: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<(), GenericInvariant> {
    // The survivors are in the same order as the reserved records, so record `index`
    // is the one this declaration reserved.
    for (index, (file, resource)) in record_decls.iter().enumerate() {
        fill_record(draft, registry, index, file, resource, diagnostics)?;
    }
    Ok(())
}

/// Fill one reserved record (`registry.records[index]`) from its resource
/// declaration: resolve each field against the full registry and fill both the
/// registry info and the image record. A resource field is a scalar, nominal scalar,
/// dense struct, or closed enum value (`Option`/`Result`/a user `enum`). A collection,
/// keyed field, or unknown spelling is not admitted; an unkeyed group is materialized
/// separately below. A bad member is `check.unsupported` and only that member is
/// skipped (the record keeps its other fields).
fn fill_record(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    index: usize,
    file: &str,
    resource: &ResourceDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<(), GenericInvariant> {
    let type_id = registry.records[index].type_id;
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    let mut groups = Vec::new();
    let mut group_slot_defs = Vec::new();
    for member in &resource.members {
        match member {
            ResourceMember::Field(field) => {
                if !field.keys.is_empty() {
                    // A keyed scalar leaf (`tags(pos: int): string`) is a keyed
                    // positional layer, not yet part of the beta durable graph. It is
                    // reported so the shape is a precise rejection, not a silent drop.
                    diagnostics.push(unsupported(file, field.span, "a keyed field"));
                    continue;
                }
                // A resource field is a value drawn from the closed acyclic durable
                // value set: a scalar, a nominal scalar, a dense struct, or a closed
                // enum (`Option`/`Result`/a user `enum`). A collection is not a durable
                // field value; an abstract parameter never reaches a concrete record.
                let field_ty = match registry.resolve_garg(
                    draft,
                    &field.ty,
                    MintSite {
                        file,
                        span: field.ty.span(),
                    },
                ) {
                    Ok(
                        ty @ (GArg::Scalar(_) | GArg::Nominal(_) | GArg::Struct(_) | GArg::Enum(_)),
                    ) => ty,
                    Ok(_) | Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                        diagnostics.push(unsupported(file, field.ty.span(), "this field type"));
                        continue;
                    }
                    Err(ResolveError::Refusal(ResolveRefusal::Limit)) => continue,
                    Err(ResolveError::Invariant(invariant)) => return Err(invariant),
                };
                let field_name_id = draft.intern_string(&field.name);
                field_defs.push(FieldDef {
                    name: field_name_id,
                    ty: field_ty.image(),
                    required: field.required,
                });
                fields.push(FieldInfo {
                    name: field.name.clone(),
                    ty: field_ty,
                    required: field.required,
                });
            }
            ResourceMember::Group(group) if group.keys.is_empty() => {
                // An unkeyed `group` is a nested sub-record value: its scalar/enum
                // leaves become a group record type, and the containing value gains one
                // required slot holding that record. Its durable identity is owned
                // separately by `durable.rs`; this is the materialized-value side only.
                let (leaf_fields, leaf_defs) =
                    build_group_leaves(draft, registry, group, file, diagnostics)?;
                let anchor = format!("{}.{}", resource.name, group.name);
                let group_name_id = draft.intern_string(&anchor);
                let group_type_id = draft.add_record_type(RecordTypeDef {
                    name: group_name_id,
                    fields: leaf_defs,
                });
                group_slot_defs.push(FieldDef {
                    name: draft.intern_string(&group.name),
                    ty: ImageType::Record {
                        idx: group_type_id.index(),
                        optional: false,
                    },
                    required: true,
                });
                groups.push(GroupInfo {
                    name: group.name.clone(),
                    type_id: group_type_id,
                    fields: leaf_fields,
                });
            }
            ResourceMember::Group(_) => {
                // A keyed `branch` (a `group` with key parameters) is a durable-graph
                // member, resolved by `durable.rs`; it is an addressed collection, not
                // part of the materialized value.
            }
        }
    }
    // The record is group-inclusive: its top-level field slots followed by one
    // group-record slot per unkeyed group, in declaration order. The verifier ties the
    // field slots to the durable member tree's fields and each trailing group slot to a
    // `Group` member, so this one record type serves both the durable graph and the
    // storeless value model.
    field_defs.extend(group_slot_defs);
    draft.set_record_fields(type_id, field_defs);
    let info = &mut registry.records[index];
    info.fields = fields;
    info.groups = groups;
    Ok(())
}

/// The direct scalar/enum leaves of an unkeyed group, in declaration order,
/// returning both the registry field infos and the image field defs. A keyed leaf,
/// a nested group or keyed branch inside the group, or a non-value leaf type is a
/// precise `check.unsupported` that skips only that leaf. Nested groups and
/// group-scoped branches are deferred; reporting keeps them from silently dropping.
fn build_group_leaves(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    group: &GroupDecl,
    file: &str,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<(Vec<FieldInfo>, Vec<FieldDef>), GenericInvariant> {
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    for member in &group.members {
        let field = match member {
            ResourceMember::Field(field) => field,
            ResourceMember::Group(inner) => {
                let what = if inner.keys.is_empty() {
                    "a nested group"
                } else {
                    "a keyed branch inside a group"
                };
                diagnostics.push(unsupported(file, inner.span, what));
                continue;
            }
        };
        if !field.keys.is_empty() {
            diagnostics.push(unsupported(file, field.span, "a keyed field"));
            continue;
        }
        let field_ty = match registry.resolve_garg(
            draft,
            &field.ty,
            MintSite {
                file,
                span: field.ty.span(),
            },
        ) {
            Ok(ty @ (GArg::Scalar(_) | GArg::Nominal(_) | GArg::Struct(_) | GArg::Enum(_))) => ty,
            Ok(_) | Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                diagnostics.push(unsupported(file, field.ty.span(), "this group field type"));
                continue;
            }
            Err(ResolveError::Refusal(ResolveRefusal::Limit)) => continue,
            Err(ResolveError::Invariant(invariant)) => return Err(invariant),
        };
        let field_name_id = draft.intern_string(&field.name);
        field_defs.push(FieldDef {
            name: field_name_id,
            ty: field_ty.image(),
            required: field.required,
        });
        fields.push(FieldInfo {
            name: field.name.clone(),
            ty: field_ty,
            required: field.required,
        });
    }
    Ok((fields, field_defs))
}

/// Reject a cycle in the value-containment graph at check time: a struct, record,
/// or enum that (directly or transitively) contains itself would be an infinite
/// value. Edges run from a product's fields and an enum's payload leaves to the
/// value types they name, including through the built-in `Option`/`Result`
/// instantiations minted during field resolution. Every struct or record on a cycle
/// is reported at its declaration with the cycle path; the verifier independently
/// re-rejects any cycle that still reaches it, so this is a source-facing check, not
/// the trust boundary.
pub(crate) fn reject_value_cycles(
    registry: &TypeRegistry,
    structs: &[(String, &StructDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<(), GenericInvariant> {
    let view = registry.metadata_view();
    let mut metadata = MetadataScratch::try_new(&view)?;
    let graph = ValueGraph::build_validated(registry, &view, &mut metadata)?;
    for info in &registry.structs {
        if let Some(path) = graph.cycle_through(ValueNode::Record(info.type_id)) {
            #[expect(
                clippy::expect_used,
                reason = "lowering bookkeeping: every registered struct was reserved from this declaration list, so its declaration survives to be found"
            )]
            let (file, span) = structs
                .iter()
                .find(|(_, decl)| decl.name == info.name)
                .map(|(file, decl)| (file.clone(), decl.name_span))
                .expect("a reserved struct has a surviving declaration");
            diagnostics.push(value_cycle_diagnostic(&file, span, &info.name, &path));
        }
    }
    for record in &registry.records {
        if let Some(path) = graph.cycle_through(ValueNode::Record(record.type_id)) {
            #[expect(
                clippy::expect_used,
                reason = "lowering bookkeeping: every registered record was reserved from this declaration list, so its declaration survives to be found"
            )]
            let (file, span) = resources
                .iter()
                .find(|(_, decl)| decl.name == record.name)
                .map(|(file, decl)| (file.clone(), decl.name_span))
                .expect("a reserved record has a surviving declaration");
            diagnostics.push(value_cycle_diagnostic(&file, span, &record.name, &path));
        }
    }
    // A monomorphized generic type on a cycle (`Tree[int]` containing `Tree[int]`)
    // is an ordinary record/enum cycle per instantiation; report each once at its
    // template's declaration.
    let mut reported: Vec<usize> = Vec::new();
    for inst in &view.generics.type_insts {
        if view.ready_inst_body_with(inst, &mut metadata)?.is_none() {
            continue;
        }
        let node = match inst.id {
            TypeInstId::Record(ty) => ValueNode::Record(ty),
            TypeInstId::Enum(id) => ValueNode::Enum(id),
        };
        if reported.contains(&inst.template) {
            continue;
        }
        if let Some(path) = graph.cycle_through(node) {
            reported.push(inst.template);
            let template = &registry.type_templates[inst.template];
            diagnostics.push(value_cycle_diagnostic(
                &template.file,
                template.name_span,
                &template.name,
                &path,
            ));
        }
    }
    Ok(())
}

fn value_cycle_diagnostic(
    file: &str,
    span: SourceSpan,
    name: &str,
    path: &[String],
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckRecursion.as_str(),
        file,
        span,
        format!(
            "value type `{name}` contains itself through the cycle {}",
            path.join(" -> ")
        ),
    )
}

/// A node in the value-containment graph: a record type (the resource record or a
/// struct — both are image records) or an enum type (a user enum or a built-in
/// `Option`/`Result` instantiation).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueNode {
    Record(TypeId),
    Enum(EnumId),
}

/// The value-containment graph over the project's records and enums, used to prove
/// acyclicity at check time.
struct ValueGraph {
    nodes: Vec<ValueNode>,
    labels: Vec<String>,
    edges: Vec<Vec<usize>>,
    /// Whether any node lies on a cycle, decided once by a single shared O(V + E)
    /// traversal at build time. A node is on a cycle exactly when it can reach
    /// itself, so on an acyclic graph — the only graph that compiles — every
    /// `cycle_through` query answers `None` in O(1) without a per-start walk.
    has_any_cycle: bool,
}

impl ValueGraph {
    #[cfg(test)]
    fn build(registry: &TypeRegistry) -> Result<Self, GenericInvariant> {
        let view = registry.metadata_view();
        let mut metadata = MetadataScratch::try_new(&view)?;
        Self::build_validated(registry, &view, &mut metadata)
    }

    fn build_validated(
        registry: &TypeRegistry,
        view: &TypeMetadataView<'_>,
        metadata: &mut MetadataScratch,
    ) -> Result<Self, GenericInvariant> {
        let mut display = DisplayScratch::for_view(view);
        let mut nodes: Vec<ValueNode> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        let mut targets: Vec<Vec<GArg>> = Vec::new();
        let mut push = |node: ValueNode, label: String, outgoing: Vec<GArg>| {
            nodes.push(node);
            labels.push(label);
            targets.push(outgoing);
        };
        for record in &registry.records {
            let outgoing = record
                .fields
                .iter()
                .map(|field| field.ty)
                .collect::<Vec<_>>();
            view.validate_args_with(&outgoing, None, metadata)?;
            push(
                ValueNode::Record(record.type_id),
                record.name.clone(),
                outgoing,
            );
        }
        for info in &registry.structs {
            let outgoing = info.fields.iter().map(|field| field.ty).collect::<Vec<_>>();
            view.validate_args_with(&outgoing, None, metadata)?;
            push(ValueNode::Record(info.type_id), info.name.clone(), outgoing);
        }
        for info in &registry.enums {
            push(ValueNode::Enum(info.enum_id), info.name.clone(), Vec::new());
        }
        for inst in &view.generics.type_insts {
            let Some(body) = view.ready_inst_body_with(inst, metadata)? else {
                continue;
            };
            let node = match inst.id {
                TypeInstId::Record(ty) => ValueNode::Record(ty),
                TypeInstId::Enum(id) => ValueNode::Enum(id),
            };
            let label = registry
                .inst_spelling_validated(view, metadata, inst.id, &mut display)?
                .ok_or(GenericInvariant::ReadyBodyMissing(inst.id))?;
            let outgoing: Vec<GArg> = match body {
                InstBody::Struct(fields) => fields.iter().map(|(_, arg)| *arg).collect(),
                InstBody::Enum(variants) => variants
                    .iter()
                    .flat_map(|variant| variant.payload.iter().map(|(_, arg)| *arg))
                    .collect(),
            };
            view.validate_args_with(&outgoing, None, metadata)?;
            push(node, label, outgoing);
        }

        // Image IDs are dense within their record and enum domains. Parallel dense
        // maps keep edge construction O(V + E) without adding a second semantic
        // classifier or a whole-cache lookup index.
        let record_len = nodes
            .iter()
            .filter_map(|node| match node {
                ValueNode::Record(id) => Some(id.index() as usize + 1),
                ValueNode::Enum(_) => None,
            })
            .max()
            .unwrap_or(0);
        let enum_len = nodes
            .iter()
            .filter_map(|node| match node {
                ValueNode::Enum(id) => Some(id.index() as usize + 1),
                ValueNode::Record(_) => None,
            })
            .max()
            .unwrap_or(0);
        let mut record_index = vec![None; record_len];
        let mut enum_index = vec![None; enum_len];
        for (index, node) in nodes.iter().copied().enumerate() {
            match node {
                ValueNode::Record(id) => record_index[id.index() as usize] = Some(index),
                ValueNode::Enum(id) => enum_index[id.index() as usize] = Some(index),
            }
        }
        let index_of = |target: ValueNode| match target {
            ValueNode::Record(id) => record_index.get(id.index() as usize).copied().flatten(),
            ValueNode::Enum(id) => enum_index.get(id.index() as usize).copied().flatten(),
        };
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        for (from, outgoing) in targets.iter().enumerate() {
            for &arg in outgoing {
                match arg {
                    GArg::Struct(id) => {
                        let to = index_of(ValueNode::Record(id))
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        edges[from].push(to);
                    }
                    GArg::Enum(id) => {
                        let to = index_of(ValueNode::Enum(id))
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        edges[from].push(to);
                    }
                    // Groups and collections are finite value boundaries, so they
                    // validate their complete target metadata but add no direct
                    // containment edge.
                    GArg::Group(_) | GArg::Collection(_) | GArg::Scalar(_) | GArg::Nominal(_) => {}
                    GArg::Param(index) => {
                        return Err(GenericInvariant::TypeArgumentParameter(index));
                    }
                }
            }
        }
        let has_any_cycle = Self::detect_any_cycle(&edges);
        Ok(ValueGraph {
            nodes,
            labels,
            edges,
            has_any_cycle,
        })
    }

    /// Whether the directed graph holds any cycle, decided by one shared iterative
    /// three-colour DFS over every node: a back edge to a node still on the active
    /// stack (grey) witnesses a cycle. The single shared traversal is O(V + E) total,
    /// replacing the former per-start reachability walks whose combined cost grew with
    /// the number of start nodes. Explicit stacks keep the walk iterative, so a deep
    /// value graph cannot overflow the native call stack.
    fn detect_any_cycle(edges: &[Vec<usize>]) -> bool {
        const WHITE: u8 = 0;
        const GREY: u8 = 1;
        const BLACK: u8 = 2;
        let mut colour = vec![WHITE; edges.len()];
        let mut has_cycle = false;
        for root in 0..edges.len() {
            if colour[root] != WHITE {
                continue;
            }
            colour[root] = GREY;
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            while let Some(&(node, edge)) = stack.last() {
                if edge < edges[node].len() {
                    #[expect(
                        clippy::expect_used,
                        reason = "lowering bookkeeping: the enclosing `while let Some(..) = stack.last()` established the stack is non-empty"
                    )]
                    let top = stack.last_mut().expect("stack is non-empty");
                    top.1 += 1;
                    #[cfg(test)]
                    bump_scaling(|counts| counts.cycle_walk_steps += 1);
                    let next = edges[node][edge];
                    match colour[next] {
                        GREY => has_cycle = true,
                        WHITE => {
                            colour[next] = GREY;
                            stack.push((next, 0));
                        }
                        _ => {}
                    }
                } else {
                    colour[node] = BLACK;
                    stack.pop();
                }
            }
        }
        has_cycle
    }

    /// The label path of a cycle that passes through `node`, or `None` if `node` is
    /// not on any cycle. The path starts and ends at `node`'s label. An acyclic graph
    /// answers `None` immediately from the shared build-time verdict; only a graph
    /// that already holds a cycle (a program that fails to compile) walks to recover
    /// the exact path, in the same edge order the former recursive walk used.
    fn cycle_through(&self, node: ValueNode) -> Option<Vec<String>> {
        if !self.has_any_cycle {
            return None;
        }
        let target = self.nodes.iter().position(|n| *n == node)?;
        let mut visited = vec![false; self.nodes.len()];
        // The start node is never marked visited, so an edge back to it is recognised
        // as closing the cycle rather than skipped. `stack` is the active DFS path
        // (the trail): reaching an edge to `target` from its top node yields that path.
        let mut stack: Vec<(usize, usize)> = vec![(target, 0)];
        let mut found = false;
        while let Some(&(current, edge)) = stack.last() {
            if edge < self.edges[current].len() {
                #[expect(
                    clippy::expect_used,
                    reason = "lowering bookkeeping: the enclosing `while let Some(..) = stack.last()` established the stack is non-empty"
                )]
                let top = stack.last_mut().expect("stack is non-empty");
                top.1 += 1;
                let next = self.edges[current][edge];
                if next == target {
                    found = true;
                    break;
                }
                if !visited[next] {
                    visited[next] = true;
                    stack.push((next, 0));
                }
            } else {
                stack.pop();
            }
        }
        if !found {
            return None;
        }
        let mut path: Vec<String> = stack
            .iter()
            .map(|(node, _)| self.labels[*node].clone())
            .collect();
        path.push(self.labels[target].clone());
        Some(path)
    }
}

fn claim_record_display_owner(
    owner: &mut Option<RecordMetadataOwner>,
    candidate: RecordMetadataOwner,
    id: TypeId,
) -> Result<(), GenericInvariant> {
    if owner.replace(candidate).is_some() {
        Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
            id,
        )))
    } else {
        Ok(())
    }
}

fn record_display_owner(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: TypeId,
) -> Result<Option<RecordMetadataOwner>, GenericInvariant> {
    let mut owner = None;
    for (record_row, record) in registry.records.iter().enumerate() {
        if record.type_id == id {
            claim_record_display_owner(
                &mut owner,
                RecordMetadataOwner::ResourceRecord(record_row),
                id,
            )?;
        }
        for (group_row, group) in record.groups.iter().enumerate() {
            if group.type_id == id {
                claim_record_display_owner(
                    &mut owner,
                    RecordMetadataOwner::Group(record_row, group_row),
                    id,
                )?;
            }
        }
    }
    for (row, info) in registry.structs.iter().enumerate() {
        if info.type_id == id {
            claim_record_display_owner(&mut owner, RecordMetadataOwner::DeclaredStruct(row), id)?;
        }
    }
    for (row, inst) in view.generics.type_insts.iter().enumerate() {
        if inst.id == TypeInstId::Record(id) {
            claim_record_display_owner(&mut owner, RecordMetadataOwner::GenericRow(row), id)?;
        }
    }
    Ok(owner)
}

fn claim_enum_display_owner(
    owner: &mut Option<EnumMetadataOwner>,
    candidate: EnumMetadataOwner,
    id: EnumId,
) -> Result<(), GenericInvariant> {
    if owner.replace(candidate).is_some() {
        Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Enum(
            id,
        )))
    } else {
        Ok(())
    }
}

fn enum_display_owner(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: EnumId,
) -> Result<Option<EnumMetadataOwner>, GenericInvariant> {
    let mut owner = None;
    for (row, info) in registry.enums.iter().enumerate() {
        if info.enum_id == id {
            claim_enum_display_owner(&mut owner, EnumMetadataOwner::DeclaredEnum(row), id)?;
        }
    }
    for (row, inst) in view.generics.type_insts.iter().enumerate() {
        if inst.id == TypeInstId::Enum(id) {
            claim_enum_display_owner(&mut owner, EnumMetadataOwner::GenericRow(row), id)?;
        }
    }
    Ok(owner)
}

fn validate_display_semantic_key(
    view: &TypeMetadataView<'_>,
    row: usize,
    id: TypeInstId,
) -> Result<(), GenericInvariant> {
    let inst = view
        .generics
        .type_insts
        .get(row)
        .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
    let mut first = None;
    for candidate in &view.generics.type_insts {
        if candidate.template == inst.template && candidate.args == inst.args {
            if let Some(first) = first {
                return Err(GenericInvariant::TypeInstantiationKeyCollision {
                    first,
                    duplicate: candidate.id,
                });
            }
            first = Some(candidate.id);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum BestEffortDisplayRoot {
    Inst {
        id: TypeInstId,
        generic_parent: Option<usize>,
    },
    Collection {
        index: u16,
        generic_parent: Option<usize>,
        collection_parent: Option<u16>,
    },
}

#[derive(Clone, Copy)]
enum BestEffortDisplayFrame {
    Arg {
        arg: GArg,
        generic_parent: Option<usize>,
        collection_parent: Option<u16>,
    },
    Inst {
        id: TypeInstId,
        generic_parent: Option<usize>,
        root: bool,
    },
    Text(&'static str),
    LeaveRow(usize),
    LeaveCollection(u16),
}

fn best_effort_display_inst_row(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: TypeInstId,
) -> Result<Option<usize>, GenericInvariant> {
    Ok(match id {
        TypeInstId::Record(id) => match record_display_owner(registry, view, id)? {
            Some(RecordMetadataOwner::GenericRow(row)) => Some(row),
            Some(
                RecordMetadataOwner::ResourceRecord(_)
                | RecordMetadataOwner::DeclaredStruct(_)
                | RecordMetadataOwner::Group(_, _),
            )
            | None => None,
        },
        TypeInstId::Enum(id) => match enum_display_owner(registry, view, id)? {
            Some(EnumMetadataOwner::GenericRow(row)) => Some(row),
            Some(EnumMetadataOwner::DeclaredEnum(_)) | None => None,
        },
    })
}

fn render_best_effort_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    root: BestEffortDisplayRoot,
    display: &mut DisplayScratch,
) -> Result<Option<String>, GenericInvariant> {
    let mut frames = Vec::new();
    match root {
        BestEffortDisplayRoot::Inst { id, generic_parent } => {
            frames.push(BestEffortDisplayFrame::Inst {
                id,
                generic_parent,
                root: true,
            });
        }
        BestEffortDisplayRoot::Collection {
            index,
            generic_parent,
            collection_parent,
        } => frames.push(BestEffortDisplayFrame::Arg {
            arg: GArg::Collection(index),
            generic_parent,
            collection_parent,
        }),
    }
    let mut output = String::new();
    let mut entered = Vec::new();
    let result = (|| {
        while let Some(frame) = frames.pop() {
            match frame {
                BestEffortDisplayFrame::Text(text) => output.push_str(text),
                BestEffortDisplayFrame::LeaveRow(row) => {
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(DisplayNode::Row(row)));
                    display.leave_row(row);
                }
                BestEffortDisplayFrame::LeaveCollection(index) => {
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(DisplayNode::Collection(index)));
                    display.leave_collection(index);
                }
                BestEffortDisplayFrame::Inst {
                    id,
                    generic_parent,
                    root,
                } => {
                    let Some(row) = best_effort_display_inst_row(registry, view, id)? else {
                        if root {
                            return Ok(None);
                        }
                        let arg = match id {
                            TypeInstId::Record(id) => GArg::Struct(id),
                            TypeInstId::Enum(id) => GArg::Enum(id),
                        };
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    };
                    if let Some(parent) = generic_parent
                        && row >= parent
                    {
                        return Err(GenericInvariant::TypeArgumentOrderViolation {
                            owner: view.generics.type_insts[parent].id,
                            target: id,
                        });
                    }
                    validate_display_semantic_key(view, row, id)?;
                    let inst = &view.generics.type_insts[row];
                    if matches!(inst.state, TypeInstState::Filling { .. })
                        || !display.enter_row(row)
                    {
                        if root {
                            return Ok(None);
                        }
                        let arg = match id {
                            TypeInstId::Record(id) => GArg::Struct(id),
                            TypeInstId::Enum(id) => GArg::Enum(id),
                        };
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    entered.push(DisplayNode::Row(row));
                    let template = registry.template_for_args(inst.template, &inst.args)?;
                    if let TypeInstState::Ready(body) = &inst.state {
                        registry.validate_inst_body_metadata(
                            inst.template,
                            &inst.args,
                            inst.id,
                            body,
                        )?;
                    }
                    output.push_str(&template.name);
                    output.push('<');
                    frames.push(BestEffortDisplayFrame::LeaveRow(row));
                    frames.push(BestEffortDisplayFrame::Text(">"));
                    for (index, arg) in inst.args.iter().copied().enumerate().rev() {
                        frames.push(BestEffortDisplayFrame::Arg {
                            arg,
                            generic_parent: Some(row),
                            collection_parent: None,
                        });
                        if index > 0 {
                            frames.push(BestEffortDisplayFrame::Text(", "));
                        }
                    }
                }
                BestEffortDisplayFrame::Arg {
                    arg,
                    generic_parent,
                    collection_parent,
                } => match arg {
                    GArg::Scalar(scalar) => output.push_str(scalar.spelling()),
                    GArg::Nominal(id) => output.push_str(
                        &registry
                            .nominals
                            .get(id.0 as usize)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                            .name,
                    ),
                    GArg::Struct(id) => match record_display_owner(registry, view, id)? {
                        Some(RecordMetadataOwner::GenericRow(_)) => {
                            frames.push(BestEffortDisplayFrame::Inst {
                                id: TypeInstId::Record(id),
                                generic_parent,
                                root: false,
                            });
                        }
                        Some(RecordMetadataOwner::DeclaredStruct(row)) => output.push_str(
                            &registry
                                .structs
                                .get(row)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        ),
                        Some(
                            RecordMetadataOwner::ResourceRecord(_)
                            | RecordMetadataOwner::Group(_, _),
                        )
                        | None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
                    },
                    GArg::Group(id) => match record_display_owner(registry, view, id)? {
                        Some(RecordMetadataOwner::Group(record, group)) => output.push_str(
                            &registry
                                .records
                                .get(record)
                                .and_then(|record| record.groups.get(group))
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        ),
                        Some(
                            RecordMetadataOwner::ResourceRecord(_)
                            | RecordMetadataOwner::DeclaredStruct(_)
                            | RecordMetadataOwner::GenericRow(_),
                        )
                        | None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
                    },
                    GArg::Enum(id) => match enum_display_owner(registry, view, id)? {
                        Some(EnumMetadataOwner::GenericRow(_)) => {
                            frames.push(BestEffortDisplayFrame::Inst {
                                id: TypeInstId::Enum(id),
                                generic_parent,
                                root: false,
                            });
                        }
                        Some(EnumMetadataOwner::DeclaredEnum(row)) => output.push_str(
                            &registry
                                .enums
                                .get(row)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        ),
                        None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
                    },
                    GArg::Collection(index) => {
                        if collection_parent.is_some_and(|parent| index >= parent)
                            || !display.enter_collection(index)
                        {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                        entered.push(DisplayNode::Collection(index));
                        let spec = view
                            .collections
                            .get(index as usize)
                            .copied()
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        frames.push(BestEffortDisplayFrame::LeaveCollection(index));
                        frames.push(BestEffortDisplayFrame::Text(">"));
                        match spec {
                            CollSpec::List { elem } => {
                                output.push_str("List<");
                                frames.push(BestEffortDisplayFrame::Arg {
                                    arg: elem,
                                    generic_parent,
                                    collection_parent: Some(index),
                                });
                            }
                            CollSpec::Map { key, value } => {
                                output.push_str("Map<");
                                frames.push(BestEffortDisplayFrame::Arg {
                                    arg: value,
                                    generic_parent,
                                    collection_parent: Some(index),
                                });
                                frames.push(BestEffortDisplayFrame::Text(", "));
                                frames.push(BestEffortDisplayFrame::Arg {
                                    arg: key,
                                    generic_parent,
                                    collection_parent: Some(index),
                                });
                            }
                        }
                    }
                    GArg::Param(index) => {
                        output.push_str(&format!("<type parameter {index}>"));
                    }
                },
            }
        }
        Ok(Some(output))
    })();
    while let Some(node) = entered.pop() {
        display.leave(node);
    }
    result
}

fn inst_spelling_for_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: TypeInstId,
    generic_parent: Option<usize>,
    display: &mut DisplayScratch,
) -> Result<Option<String>, GenericInvariant> {
    render_best_effort_display(
        registry,
        view,
        BestEffortDisplayRoot::Inst { id, generic_parent },
        display,
    )
}

fn collection_spelling_for_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    index: u16,
    generic_parent: Option<usize>,
    collection_parent: Option<u16>,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    render_best_effort_display(
        registry,
        view,
        BestEffortDisplayRoot::Collection {
            index,
            generic_parent,
            collection_parent,
        },
        display,
    )?
    .ok_or(GenericInvariant::TypeArgumentTargetMissing(
        GArg::Collection(index),
    ))
}

/// The canonical angle-form display spelling of a metadata-validated value-type
/// argument. The caller supplies the same immutable owner view and directory used
/// for semantic validation, so a graph walk never rebuilds or searches the cache.
fn garg_spelling_validated(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    render_validated_display_arg(registry, view, metadata, arg, display)
}

#[derive(Clone, Copy)]
enum ValidatedDisplayFrame {
    Arg(GArg),
    Inst {
        row: usize,
        id: TypeInstId,
        arg: GArg,
    },
    Collection(u16),
    Text(&'static str),
    Leave(DisplayNode),
}

fn render_validated_display_arg(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    let mut output = String::new();
    let mut frames = vec![ValidatedDisplayFrame::Arg(arg)];
    let mut entered = Vec::new();
    let result = (|| {
        while let Some(frame) = frames.pop() {
            match frame {
                ValidatedDisplayFrame::Text(text) => output.push_str(text),
                ValidatedDisplayFrame::Leave(node) => {
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(node));
                    display.leave(node);
                }
                ValidatedDisplayFrame::Arg(arg) => match arg {
                    GArg::Scalar(scalar) => output.push_str(scalar.spelling()),
                    GArg::Nominal(id) => output.push_str(
                        &registry
                            .nominals
                            .get(id.0 as usize)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                            .name,
                    ),
                    GArg::Struct(id) => {
                        if metadata.resource_record(id).is_some() {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                        if let Some(row) = metadata.row(TypeInstId::Record(id)) {
                            frames.push(ValidatedDisplayFrame::Inst {
                                row,
                                id: TypeInstId::Record(id),
                                arg,
                            });
                        } else {
                            let row = metadata
                                .declared_struct(id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            output.push_str(
                                &registry
                                    .structs
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        }
                    }
                    GArg::Group(id) => {
                        let (record, group) = metadata
                            .group(id)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        output.push_str(
                            &registry
                                .records
                                .get(record)
                                .and_then(|record| record.groups.get(group))
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        );
                    }
                    GArg::Enum(id) => {
                        if let Some(row) = metadata.row(TypeInstId::Enum(id)) {
                            frames.push(ValidatedDisplayFrame::Inst {
                                row,
                                id: TypeInstId::Enum(id),
                                arg,
                            });
                        } else {
                            let row = metadata
                                .declared_enum(id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            output.push_str(
                                &registry
                                    .enums
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        }
                    }
                    GArg::Collection(index) => {
                        frames.push(ValidatedDisplayFrame::Collection(index));
                    }
                    GArg::Param(index) => {
                        output.push_str(&format!("<type parameter {index}>"));
                    }
                },
                ValidatedDisplayFrame::Inst { row, id, arg } => {
                    let inst = view
                        .generics
                        .type_insts
                        .get(row)
                        .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
                    if !matches!(inst.state, TypeInstState::Ready(_)) || !display.enter_row(row) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Row(row);
                    entered.push(node);
                    let template = registry
                        .type_templates
                        .get(inst.template)
                        .ok_or(GenericInvariant::TypeTemplateMissing(inst.template))?;
                    output.push_str(&template.name);
                    output.push('<');
                    frames.push(ValidatedDisplayFrame::Leave(node));
                    frames.push(ValidatedDisplayFrame::Text(">"));
                    for (index, arg) in inst.args.iter().copied().enumerate().rev() {
                        frames.push(ValidatedDisplayFrame::Arg(arg));
                        if index > 0 {
                            frames.push(ValidatedDisplayFrame::Text(", "));
                        }
                    }
                }
                ValidatedDisplayFrame::Collection(index) => {
                    let arg = GArg::Collection(index);
                    if !display.enter_collection(index) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Collection(index);
                    entered.push(node);
                    let spec = view
                        .collections
                        .get(index as usize)
                        .copied()
                        .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                    frames.push(ValidatedDisplayFrame::Leave(node));
                    frames.push(ValidatedDisplayFrame::Text(">"));
                    match spec {
                        CollSpec::List { elem } => {
                            output.push_str("List<");
                            frames.push(ValidatedDisplayFrame::Arg(elem));
                        }
                        CollSpec::Map { key, value } => {
                            output.push_str("Map<");
                            frames.push(ValidatedDisplayFrame::Arg(value));
                            frames.push(ValidatedDisplayFrame::Text(", "));
                            frames.push(ValidatedDisplayFrame::Arg(key));
                        }
                    }
                }
            }
        }
        Ok(output)
    })();
    while let Some(node) = entered.pop() {
        display.leave(node);
    }
    result
}

/// The durable-anchor spelling of a bare value-type argument: the space-free,
/// bracket-form opaque-ledger twin of [`garg_spelling`], recursing through nested
/// generic instantiations. It never calls the angle-form display owner, so the
/// ledger bytes stay byte-stable and independent of diagnostic spelling. The
/// deliberate near-duplication is the isolation boundary the durable identity relies
/// on; do not merge the two behind a shared delimiter policy.
#[cfg(test)]
fn garg_anchor_spelling(registry: &TypeRegistry, arg: GArg) -> Result<String, GenericInvariant> {
    let view = registry.metadata_view();
    let mut metadata = MetadataScratch::try_new(&view)?;
    view.validate_args_with(std::slice::from_ref(&arg), None, &mut metadata)?;
    let mut display = DisplayScratch::for_view(&view);
    garg_anchor_spelling_validated(registry, &view, &metadata, arg, &mut display)
}

#[derive(Clone, Copy)]
enum ValidatedAnchorFrame {
    Arg(GArg),
    Inst {
        row: usize,
        id: TypeInstId,
        arg: GArg,
    },
    Collection(u16),
    Text(&'static str),
    Leave(DisplayNode),
}

#[cfg(test)]
fn garg_anchor_spelling_validated(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    render_validated_anchor_arg(registry, view, metadata, arg, display)
}

fn render_validated_anchor_arg(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    let mut output = String::new();
    let mut frames = vec![ValidatedAnchorFrame::Arg(arg)];
    let mut entered = Vec::new();
    let result = (|| {
        while let Some(frame) = frames.pop() {
            match frame {
                ValidatedAnchorFrame::Text(text) => output.push_str(text),
                ValidatedAnchorFrame::Leave(node) => {
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(node));
                    display.leave(node);
                }
                ValidatedAnchorFrame::Arg(arg) => match arg {
                    GArg::Scalar(scalar) => output.push_str(scalar.spelling()),
                    GArg::Nominal(id) => output.push_str(
                        &registry
                            .nominals
                            .get(id.0 as usize)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                            .name,
                    ),
                    GArg::Struct(id) => {
                        if let Some(row) = metadata.declared_struct(id) {
                            output.push_str(
                                &registry
                                    .structs
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        } else {
                            let inst_id = TypeInstId::Record(id);
                            let row = metadata
                                .row(inst_id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            frames.push(ValidatedAnchorFrame::Inst {
                                row,
                                id: inst_id,
                                arg,
                            });
                        }
                    }
                    GArg::Group(id) => {
                        let (record, group) = metadata
                            .group(id)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        output.push_str(
                            &registry
                                .records
                                .get(record)
                                .and_then(|record| record.groups.get(group))
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        );
                    }
                    GArg::Enum(id) => {
                        if let Some(row) = metadata.declared_enum(id) {
                            output.push_str(
                                &registry
                                    .enums
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        } else {
                            let inst_id = TypeInstId::Enum(id);
                            let row = metadata
                                .row(inst_id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            frames.push(ValidatedAnchorFrame::Inst {
                                row,
                                id: inst_id,
                                arg,
                            });
                        }
                    }
                    GArg::Collection(index) => {
                        frames.push(ValidatedAnchorFrame::Collection(index));
                    }
                    GArg::Param(index) => {
                        return Err(GenericInvariant::TypeArgumentParameter(index));
                    }
                },
                ValidatedAnchorFrame::Inst { row, id, arg } => {
                    let inst = view
                        .generics
                        .type_insts
                        .get(row)
                        .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
                    if !matches!(inst.state, TypeInstState::Ready(_)) || !display.enter_row(row) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Row(row);
                    entered.push(node);
                    let template = registry
                        .type_templates
                        .get(inst.template)
                        .ok_or(GenericInvariant::TypeTemplateMissing(inst.template))?;
                    output.push_str(&template.name);
                    output.push('[');
                    frames.push(ValidatedAnchorFrame::Leave(node));
                    frames.push(ValidatedAnchorFrame::Text("]"));
                    for (index, arg) in inst.args.iter().copied().enumerate().rev() {
                        frames.push(ValidatedAnchorFrame::Arg(arg));
                        if index > 0 {
                            frames.push(ValidatedAnchorFrame::Text(","));
                        }
                    }
                }
                ValidatedAnchorFrame::Collection(index) => {
                    let arg = GArg::Collection(index);
                    if !display.enter_collection(index) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Collection(index);
                    entered.push(node);
                    let spec = view
                        .collections
                        .get(index as usize)
                        .copied()
                        .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                    frames.push(ValidatedAnchorFrame::Leave(node));
                    frames.push(ValidatedAnchorFrame::Text("]"));
                    match spec {
                        CollSpec::List { elem } => {
                            output.push_str("List[");
                            frames.push(ValidatedAnchorFrame::Arg(elem));
                        }
                        CollSpec::Map { key, value } => {
                            output.push_str("Map[");
                            frames.push(ValidatedAnchorFrame::Arg(value));
                            frames.push(ValidatedAnchorFrame::Text(","));
                            frames.push(ValidatedAnchorFrame::Arg(key));
                        }
                    }
                }
            }
        }
        Ok(output)
    })();
    while let Some(node) = entered.pop() {
        display.leave(node);
    }
    result
}

fn scalar_of(ty: &TypeExpr) -> Option<ScalarType> {
    match ty {
        TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
        _ => None,
    }
}

/// The diagnostic for a declaration that reuses a built-in generic type name.
fn reserved_name(file: &str, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict.as_str(),
        file,
        span,
        format!("`{name}` is a built-in generic type and cannot be redeclared"),
    )
}

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}

#[cfg(test)]
#[path = "types_metadata_successor_tests.rs"]
mod types_metadata_successor_tests;

#[cfg(test)]
#[path = "generic_scaling_counts_tests.rs"]
mod generic_scaling_counts_tests;

#[cfg(test)]
mod instantiation_state_tests {
    use super::*;
    use marrow_syntax::{Declaration, parse_source};

    fn name(text: &str) -> TypeExpr {
        TypeExpr::Name {
            text: text.to_string(),
            span: SourceSpan::default(),
        }
    }

    fn apply(head: &str, args: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr::Apply {
            head: head.to_string(),
            args,
            span: SourceSpan::default(),
        }
    }

    fn template(name: &str, fields: Vec<(&str, TypeExpr)>) -> TypeTemplate {
        TypeTemplate {
            name: name.to_string(),
            file: "src/main.mw".to_string(),
            name_span: SourceSpan::default(),
            reserved: None,
            type_params: vec![("T".to_string(), None)],
            body: TemplateBody::Struct(
                fields
                    .into_iter()
                    .map(|(field, ty)| (field.to_string(), ty))
                    .collect(),
            ),
        }
    }

    fn enum_template(name: &str, payload: TypeExpr) -> TypeTemplate {
        TypeTemplate {
            name: name.to_string(),
            file: "src/main.mw".to_string(),
            name_span: SourceSpan::default(),
            reserved: None,
            type_params: vec![("T".to_string(), None)],
            body: TemplateBody::Enum(vec![TemplateVariant {
                name: "value".to_string(),
                payload: vec![TemplatePayload {
                    name: "item".to_string(),
                    ty: payload,
                }],
            }]),
        }
    }

    fn registry(templates: Vec<TypeTemplate>) -> TypeRegistry {
        TypeRegistry {
            aliases: BTreeMap::new(),
            nominals: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            records: Vec::new(),
            type_templates: templates,
            generics: RefCell::default(),
            collections: RefCell::default(),
        }
    }

    fn site(line: u32) -> MintSite<'static> {
        MintSite {
            file: "src/main.mw",
            span: SourceSpan {
                line,
                column: 9,
                ..SourceSpan::default()
            },
        }
    }

    fn row<'a>(registry: &'a TypeRegistry, name: &str) -> std::cell::Ref<'a, TypeInst> {
        std::cell::Ref::map(registry.generics.borrow(), |generics| {
            generics
                .type_insts
                .iter()
                .find(|inst| registry.type_templates[inst.template].name == name)
                .expect("named row exists")
        })
    }

    #[derive(Debug, PartialEq, Eq)]
    enum StableLimit {
        Open,
        Pending(SourceDiagnostic),
        Reported,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum StableRowState {
        Filling,
        Staged,
        Ready,
        RejectedLimit,
        RejectedUnsupported,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct StableRow {
        template: usize,
        args: Vec<GArg>,
        id: TypeInstId,
        state: StableRowState,
        body: Option<StableBody>,
        dependents: Vec<usize>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum StableBody {
        Struct(Vec<(String, GArg)>),
        Enum(Vec<(String, Vec<(String, GArg)>)>),
    }

    fn stable_body(body: &InstBody) -> StableBody {
        match body {
            InstBody::Struct(fields) => StableBody::Struct(fields.clone()),
            InstBody::Enum(variants) => StableBody::Enum(
                variants
                    .iter()
                    .map(|variant| (variant.name.clone(), variant.payload.clone()))
                    .collect(),
            ),
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct StableSnapshot {
        rows: Vec<StableRow>,
        collections: Vec<CollSpec>,
        fn_base: u16,
        functions: Vec<(usize, Vec<GArg>, u16)>,
        queue: Vec<(usize, Vec<GArg>, u16)>,
        fill_batch_start: Option<usize>,
        fill_rows: Vec<(TypeInstKey, usize)>,
        fill_stack: Vec<usize>,
        fill_failures: Vec<(usize, ResolveRefusal)>,
        limit: StableLimit,
        payloads: Vec<SourceDiagnostic>,
        build_invariant: Option<GenericInvariant>,
    }

    fn stable_snapshot(registry: &TypeRegistry) -> StableSnapshot {
        let generics = registry.generics.borrow();
        let rows = generics
            .type_insts
            .iter()
            .map(|inst| {
                let (state, body) = match &inst.state {
                    TypeInstState::Filling { staged: None } => (StableRowState::Filling, None),
                    TypeInstState::Filling { staged: Some(body) } => {
                        (StableRowState::Staged, Some(stable_body(body)))
                    }
                    TypeInstState::Ready(body) => (StableRowState::Ready, Some(stable_body(body))),
                    TypeInstState::Rejected(ResolveRefusal::Limit) => {
                        (StableRowState::RejectedLimit, None)
                    }
                    TypeInstState::Rejected(ResolveRefusal::Unsupported) => {
                        (StableRowState::RejectedUnsupported, None)
                    }
                };
                StableRow {
                    template: inst.template,
                    args: inst.args.clone(),
                    id: inst.id,
                    state,
                    body,
                    dependents: inst.dependents.clone(),
                }
            })
            .collect();
        let functions = generics
            .fn_insts
            .iter()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
            .collect();
        let queue = generics
            .fn_queue
            .iter()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
            .collect();
        let limit = match &generics.limit {
            LimitState::Open => StableLimit::Open,
            LimitState::Pending(diagnostic) => StableLimit::Pending(diagnostic.clone()),
            LimitState::Reported => StableLimit::Reported,
        };
        StableSnapshot {
            rows,
            collections: registry.collections.borrow().clone(),
            fn_base: generics.fn_base,
            functions,
            queue,
            fill_batch_start: generics.fill_batch_start,
            fill_rows: generics
                .fill_rows
                .iter()
                .map(|(key, index)| (*key, *index))
                .collect(),
            fill_stack: generics.fill_stack.clone(),
            fill_failures: generics.fill_failures.clone(),
            limit,
            payloads: generics.collection_payloads.clone(),
            build_invariant: generics.build_invariant,
        }
    }

    fn draft_snapshot(draft: &ImageDraft) -> (Vec<u8>, marrow_image::ImageId) {
        let encoded = draft.encode().expect("test draft encodes");
        (encoded.bytes, encoded.image_id)
    }

    fn add_declared_struct(
        registry: &mut TypeRegistry,
        draft: &mut ImageDraft,
        name: &str,
        fields: Vec<(&str, GArg)>,
    ) -> TypeId {
        let record_name = draft.intern_string(name);
        let mut image_fields = Vec::with_capacity(fields.len());
        let mut field_infos = Vec::with_capacity(fields.len());
        for (field, ty) in fields {
            let field_name = draft.intern_string(field);
            image_fields.push(FieldDef {
                name: field_name,
                ty: ty.image(),
                required: true,
            });
            field_infos.push(FieldInfo {
                name: field.to_string(),
                ty,
                required: true,
            });
        }
        let type_id = draft.add_record_type(RecordTypeDef {
            name: record_name,
            fields: image_fields,
        });
        registry.structs.push(StructInfo {
            type_id,
            name: name.to_string(),
            fields: field_infos,
        });
        type_id
    }

    fn add_resource_record(
        registry: &mut TypeRegistry,
        draft: &mut ImageDraft,
        name: &str,
    ) -> TypeId {
        let record_name = draft.intern_string(name);
        let type_id = draft.add_record_type(RecordTypeDef {
            name: record_name,
            fields: Vec::new(),
        });
        registry.records.push(RecordInfo {
            type_id,
            name: name.to_string(),
            fields: Vec::new(),
            groups: Vec::new(),
        });
        type_id
    }

    fn add_resource_group(
        registry: &mut TypeRegistry,
        draft: &mut ImageDraft,
        record: usize,
        name: &str,
    ) -> TypeId {
        let group_name = draft.intern_string(name);
        let type_id = draft.add_record_type(RecordTypeDef {
            name: group_name,
            fields: Vec::new(),
        });
        registry.records[record].groups.push(GroupInfo {
            name: name.to_string(),
            type_id,
            fields: Vec::new(),
        });
        type_id
    }

    type MetadataOwnerSnapshot = (
        Vec<TypeId>,
        Vec<Vec<TypeId>>,
        Vec<TypeId>,
        Vec<EnumId>,
        StableSnapshot,
    );

    fn metadata_owner_snapshot(registry: &TypeRegistry) -> MetadataOwnerSnapshot {
        (
            registry
                .records
                .iter()
                .map(|record| record.type_id)
                .collect(),
            registry
                .records
                .iter()
                .map(|record| record.groups.iter().map(|group| group.type_id).collect())
                .collect(),
            registry.structs.iter().map(|info| info.type_id).collect(),
            registry.enums.iter().map(|info| info.enum_id).collect(),
            stable_snapshot(registry),
        )
    }

    fn assert_metadata_unchanged(
        registry: &TypeRegistry,
        draft: &ImageDraft,
        owner: &MetadataOwnerSnapshot,
        image: &(Vec<u8>, marrow_image::ImageId),
    ) {
        assert_eq!(&metadata_owner_snapshot(registry), owner);
        assert_eq!(&draft_snapshot(draft), image);
    }

    fn active_registry() -> TypeRegistry {
        let registry = registry(vec![template("Active", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(1))
            .expect("seed row mints ready");
        let mut generics = registry.generics.borrow_mut();
        let prior = std::mem::replace(
            &mut generics.type_insts[0].state,
            TypeInstState::Rejected(ResolveRefusal::Unsupported),
        );
        let TypeInstState::Ready(body) = prior else {
            panic!("seed row is Ready")
        };
        generics.type_insts[0].state = TypeInstState::Filling { staged: Some(body) };
        generics.fill_batch_start = Some(0);
        let id = generics.type_insts[0].id;
        generics.fill_rows.insert(id.into(), 0);
        drop(generics);
        registry
    }

    fn cache_invariant_name(cause: GenericCacheInvariant) -> &'static str {
        match cause {
            GenericCacheInvariant::ActiveBatchMissing => "active batch missing",
            GenericCacheInvariant::ActiveBatchRange => "active batch range",
            GenericCacheInvariant::ActiveRowCardinality => "active row cardinality",
            GenericCacheInvariant::ActiveRowKeyMismatch => "active row key mismatch",
            GenericCacheInvariant::ActiveFillStackNotEmpty => "active fill stack not empty",
            GenericCacheInvariant::FailureIndexOutOfRange => "failure index out of range",
            GenericCacheInvariant::DependentIndexOutOfRange => "dependent index out of range",
            GenericCacheInvariant::StableRowInActiveBatch => "stable row in active batch",
            GenericCacheInvariant::IncompleteRowWithoutRefusal => "incomplete row without refusal",
            GenericCacheInvariant::FillingReuseOutsideBatch => "Filling reuse outside batch",
            GenericCacheInvariant::SettledRowMissing => "settled row missing",
            GenericCacheInvariant::SettledRowStillFilling => "settled row still Filling",
            GenericCacheInvariant::FillStackMismatch => "fill stack mismatch",
            GenericCacheInvariant::MintIndexDrift => "mint index drift",
        }
    }

    #[test]
    fn settlement_precommit_faults_are_exact_and_read_only() {
        #[derive(Clone, Copy)]
        enum Fault {
            MissingBatch,
            BatchRange,
            NonemptyStack,
            RowCardinality,
            RowKey,
            FailureRange,
            StableRow,
            DependentRange,
            IncompleteRow,
        }

        let cases = [
            (
                Fault::MissingBatch,
                GenericCacheInvariant::ActiveBatchMissing,
            ),
            (Fault::BatchRange, GenericCacheInvariant::ActiveBatchRange),
            (
                Fault::NonemptyStack,
                GenericCacheInvariant::ActiveFillStackNotEmpty,
            ),
            (
                Fault::RowCardinality,
                GenericCacheInvariant::ActiveRowCardinality,
            ),
            (Fault::RowKey, GenericCacheInvariant::ActiveRowKeyMismatch),
            (
                Fault::FailureRange,
                GenericCacheInvariant::FailureIndexOutOfRange,
            ),
            (
                Fault::StableRow,
                GenericCacheInvariant::StableRowInActiveBatch,
            ),
            (
                Fault::DependentRange,
                GenericCacheInvariant::DependentIndexOutOfRange,
            ),
            (
                Fault::IncompleteRow,
                GenericCacheInvariant::IncompleteRowWithoutRefusal,
            ),
        ];

        for (fault, expected) in cases {
            let registry = active_registry();
            {
                let mut generics = registry.generics.borrow_mut();
                match fault {
                    Fault::MissingBatch => generics.fill_batch_start = None,
                    Fault::BatchRange => generics.fill_batch_start = Some(2),
                    Fault::NonemptyStack => generics.fill_stack.push(0),
                    Fault::RowCardinality => generics.fill_rows.clear(),
                    Fault::RowKey => {
                        let key = TypeInstKey::from(generics.type_insts[0].id);
                        generics.fill_rows.insert(key, 1);
                    }
                    Fault::FailureRange => generics
                        .fill_failures
                        .push((1, ResolveRefusal::Unsupported)),
                    Fault::StableRow => {
                        let prior = std::mem::replace(
                            &mut generics.type_insts[0].state,
                            TypeInstState::Rejected(ResolveRefusal::Unsupported),
                        );
                        let TypeInstState::Filling { staged: Some(body) } = prior else {
                            panic!("active row has a staged body")
                        };
                        generics.type_insts[0].state = TypeInstState::Ready(body);
                    }
                    Fault::DependentRange => generics.type_insts[0].dependents.push(1),
                    Fault::IncompleteRow => {
                        generics.type_insts[0].state = TypeInstState::Filling { staged: None };
                    }
                }
            }
            let before = stable_snapshot(&registry);
            assert_eq!(
                registry.settle_fill_batch(),
                Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    expected
                ))),
                "{}",
                cache_invariant_name(expected)
            );
            assert_eq!(stable_snapshot(&registry), before);
        }
    }

    #[test]
    fn nonsettlement_cache_faults_are_exact_and_read_only() {
        let registry = active_registry();
        registry.generics.borrow_mut().fill_batch_start = None;
        registry.generics.borrow_mut().fill_rows.clear();
        let id = registry.generics.borrow().type_insts[0].id;
        let before = stable_snapshot(&registry);
        let mut draft = ImageDraft::new();
        assert_eq!(
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2),),
            Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::FillingReuseOutsideBatch,
            )))
        );
        assert_eq!(stable_snapshot(&registry), before);

        let missing_before = stable_snapshot(&registry);
        assert_eq!(
            registry.settled_type_result(1, id, AnyReadyInstance),
            Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::SettledRowMissing,
            )))
        );
        assert_eq!(stable_snapshot(&registry), missing_before);

        let filling_before = stable_snapshot(&registry);
        assert_eq!(
            registry.settled_type_result(0, id, AnyReadyInstance),
            Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::SettledRowStillFilling,
            )))
        );
        assert_eq!(stable_snapshot(&registry), filling_before);

        registry.generics.borrow_mut().fill_stack.push(1);
        let stack_before = stable_snapshot(&registry);
        assert_eq!(
            registry.finish_fill_stack(0),
            Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::FillStackMismatch,
            )))
        );
        assert_eq!(stable_snapshot(&registry), stack_before);
    }

    #[test]
    fn failed_fill_rejects_reverse_dependent_rows_without_poisoning_siblings() {
        let registry = registry(vec![
            template("Good", vec![("value", name("T"))]),
            template(
                "Outer",
                vec![
                    ("good", apply("Good", vec![name("T")])),
                    ("inner", apply("Inner", vec![name("T")])),
                    ("bad", apply("Missing", vec![name("T")])),
                ],
            ),
            template("Inner", vec![("outer", apply("Outer", vec![name("T")]))]),
        ]);
        let mut draft = ImageDraft::new();
        assert_eq!(
            registry.mint_type_instance(&mut draft, 1, &[GArg::Scalar(ScalarType::Int)], site(10),),
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
        );

        assert!(matches!(
            row(&registry, "Good").state,
            TypeInstState::Ready(_)
        ));
        assert!(matches!(
            row(&registry, "Outer").state,
            TypeInstState::Rejected(ResolveRefusal::Unsupported)
        ));
        assert!(matches!(
            row(&registry, "Inner").state,
            TypeInstState::Rejected(ResolveRefusal::Unsupported)
        ));
        let (outer_id, before) = {
            let generics = registry.generics.borrow();
            (generics.type_insts[0].id, generics.type_insts.len())
        };
        assert_eq!(
            registry.mint_type_instance(&mut draft, 1, &[GArg::Scalar(ScalarType::Int)], site(20),),
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
        );
        let generics = registry.generics.borrow();
        assert_eq!(generics.type_insts.len(), before);
        assert_eq!(generics.type_insts[0].id, outer_id);
        assert!(generics.fill_stack.is_empty());
        assert!(
            generics
                .type_insts
                .iter()
                .all(|inst| !matches!(inst.state, TypeInstState::Filling { .. }))
        );
    }

    #[test]
    fn collection_substitution_edges_reject_dependents_of_a_failed_outer_row() {
        let registry = registry(vec![
            template(
                "Outer",
                vec![
                    (
                        "child",
                        apply(
                            "Child",
                            vec![apply("List", vec![apply("Outer", vec![name("T")])])],
                        ),
                    ),
                    ("bad", apply("Missing", vec![name("T")])),
                ],
            ),
            template("Child", vec![("value", name("T"))]),
        ]);
        let mut draft = ImageDraft::new();
        assert_eq!(
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(10),),
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
        );
        assert!(matches!(
            row(&registry, "Outer").state,
            TypeInstState::Rejected(ResolveRefusal::Unsupported)
        ));
        assert!(matches!(
            row(&registry, "Child").state,
            TypeInstState::Rejected(ResolveRefusal::Unsupported)
        ));
    }

    #[test]
    fn mixed_fill_refusals_join_to_limit_without_poisoning_an_independent_row() {
        for failures in [
            vec![(0, ResolveRefusal::Unsupported), (1, ResolveRefusal::Limit)],
            vec![(1, ResolveRefusal::Limit), (0, ResolveRefusal::Unsupported)],
        ] {
            let registry = registry(vec![
                template("Outer", vec![("value", name("T"))]),
                template("Dependency", vec![("value", name("T"))]),
                template("Sibling", vec![("value", name("T"))]),
            ]);
            let mut draft = ImageDraft::new();
            for template in 0..3 {
                registry
                    .mint_type_instance(
                        &mut draft,
                        template,
                        &[GArg::Scalar(ScalarType::Int)],
                        site(template as u32 + 1),
                    )
                    .expect("seed row mints ready");
            }

            let outer_id = {
                let mut generics = registry.generics.borrow_mut();
                for inst in &mut generics.type_insts {
                    let prior = std::mem::replace(
                        &mut inst.state,
                        TypeInstState::Rejected(ResolveRefusal::Unsupported),
                    );
                    let TypeInstState::Ready(body) = prior else {
                        panic!("seed row is ready")
                    };
                    inst.state = TypeInstState::Filling { staged: Some(body) };
                }
                generics.fill_batch_start = Some(0);
                generics.fill_rows = generics
                    .type_insts
                    .iter()
                    .enumerate()
                    .map(|(index, inst)| (TypeInstKey::from(inst.id), index))
                    .collect();
                // Outer depends on Dependency; Sibling is in the same batch but has
                // no path to either refusal and must remain Ready.
                generics.type_insts[1].dependents.push(0);
                generics.fill_failures = failures;
                generics.type_insts[0].id
            };

            registry
                .settle_fill_batch()
                .expect("well-formed provisional batch settles");
            assert_eq!(
                registry.settled_type_result(0, outer_id, AnyReadyInstance),
                Err(ResolveError::Refusal(ResolveRefusal::Limit)),
                "the settled row, not its local Unsupported, owns the return"
            );
            assert!(matches!(
                row(&registry, "Outer").state,
                TypeInstState::Rejected(ResolveRefusal::Limit)
            ));
            assert!(matches!(
                row(&registry, "Dependency").state,
                TypeInstState::Rejected(ResolveRefusal::Limit)
            ));
            assert!(matches!(
                row(&registry, "Sibling").state,
                TypeInstState::Ready(_)
            ));
            let generics = registry.generics.borrow();
            assert!(generics.fill_batch_start.is_none());
            assert!(generics.fill_rows.is_empty());
            assert!(generics.fill_stack.is_empty());
            assert!(generics.fill_failures.is_empty());
            assert_eq!(generics.fill_failures.capacity(), 0);
            assert!(generics.type_insts.iter().all(|inst| {
                !matches!(inst.state, TypeInstState::Filling { .. })
                    && inst.dependents.is_empty()
                    && inst.dependents.capacity() == 0
            }));
        }
    }

    #[test]
    fn divergent_limit_rejects_dependents_and_reports_once() {
        let registry = registry(vec![template(
            "Grow",
            vec![("next", apply("Grow", vec![apply("List", vec![name("T")])]))],
        )]);
        let mut draft = ImageDraft::new();
        assert_eq!(
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(10),),
            Err(ResolveError::Refusal(ResolveRefusal::Limit))
        );
        let (first_row_id, before) = {
            let generics = registry.generics.borrow();
            (generics.type_insts[0].id, generics.type_insts.len())
        };
        assert!(
            registry
                .generics
                .borrow()
                .type_insts
                .iter()
                .all(|inst| matches!(inst.state, TypeInstState::Rejected(ResolveRefusal::Limit)))
        );
        assert!(registry.generics.borrow().fill_stack.is_empty());
        let first = registry.take_generic_diagnostics().into_ordered();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].code, Code::CheckInstantiationLimit.as_str());
        assert_eq!((first[0].line, first[0].column), (10, 9));
        assert_eq!(
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(20),),
            Err(ResolveError::Refusal(ResolveRefusal::Limit))
        );
        let generics = registry.generics.borrow();
        assert_eq!(generics.type_insts.len(), before);
        assert_eq!(generics.type_insts[0].id, first_row_id);
        drop(generics);
        assert!(
            registry
                .take_generic_diagnostics()
                .into_ordered()
                .is_empty()
        );
    }

    #[test]
    fn rejected_rows_are_displayable_but_not_semantic_or_anchor_ready() {
        let registry = registry(vec![enum_template(
            "Bad",
            apply("Missing", vec![name("T")]),
        )]);
        let mut draft = ImageDraft::new();
        assert_eq!(
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(10),),
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
        );
        let id = row(&registry, "Bad").id;
        let TypeInstId::Enum(enum_id) = id else {
            panic!("enum template reserves an enum id")
        };
        assert_eq!(registry.inst_spelling(id).as_deref(), Some("Bad<int>"));
        assert!(registry.instantiation_of(id).unwrap().is_none());
        assert!(registry.type_inst_body(id).unwrap().is_none());
        assert!(registry.enum_variants(enum_id).unwrap().is_none());
        assert!(registry.enum_anchor_spelling(enum_id).unwrap().is_none());
        assert!(
            ValueGraph::build(&registry)
                .unwrap()
                .nodes
                .iter()
                .all(|node| *node != ValueNode::Enum(enum_id))
        );

        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Filling { staged: None };
        assert!(registry.inst_spelling(id).is_none());
        assert!(registry.instantiation_of(id).unwrap().is_none());
        assert!(registry.type_inst_body(id).unwrap().is_none());
        assert!(registry.enum_variants(enum_id).unwrap().is_none());
        assert!(registry.enum_anchor_spelling(enum_id).unwrap().is_none());
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
    }

    #[test]
    fn proof_clone_is_stable_isolated_and_transfers_once() {
        let registry = registry(vec![
            template("Leaf", vec![("value", name("T"))]),
            enum_template("Choice", apply("Leaf", vec![name("T")])),
            template(
                "Composite",
                vec![
                    ("scalar", name("T")),
                    ("record", apply("Leaf", vec![name("T")])),
                    ("enum", apply("Choice", vec![name("T")])),
                    ("collection", apply("List", vec![name("T")])),
                ],
            ),
        ]);
        let mut draft = ImageDraft::new();
        let scalar = GArg::Scalar(ScalarType::Int);
        let leaf_id = registry
            .mint_type_instance(&mut draft, 0, &[scalar], site(2))
            .expect("stable record seed mints");
        let TypeInstId::Record(leaf_record) = leaf_id else {
            panic!("Leaf is a struct template")
        };
        let choice_id = registry
            .mint_type_instance(&mut draft, 1, &[scalar], site(3))
            .expect("stable enum seed mints");
        let TypeInstId::Enum(choice_enum) = choice_id else {
            panic!("Choice is an enum template")
        };
        let collection = registry
            .instantiate_list(&mut draft, scalar)
            .expect("aligned collection owners mint");
        let composite_id = registry
            .mint_type_instance(&mut draft, 2, &[scalar], site(4))
            .expect("representative record seed mints");
        registry.set_fn_base(37);
        let reserved = registry
            .reserve_fn_instance(7, vec![scalar], site(5))
            .expect("stable function row reserves");
        assert_eq!(reserved, 37);
        let before = stable_snapshot(&registry);
        assert_eq!(
            before.rows,
            vec![
                StableRow {
                    template: 0,
                    args: vec![scalar],
                    id: leaf_id,
                    state: StableRowState::Ready,
                    body: Some(StableBody::Struct(vec![("value".to_string(), scalar)])),
                    dependents: Vec::new(),
                },
                StableRow {
                    template: 1,
                    args: vec![scalar],
                    id: choice_id,
                    state: StableRowState::Ready,
                    body: Some(StableBody::Enum(vec![(
                        "value".to_string(),
                        vec![("item".to_string(), GArg::Struct(leaf_record))],
                    )])),
                    dependents: Vec::new(),
                },
                StableRow {
                    template: 2,
                    args: vec![scalar],
                    id: composite_id,
                    state: StableRowState::Ready,
                    body: Some(StableBody::Struct(vec![
                        ("scalar".to_string(), scalar),
                        ("record".to_string(), GArg::Struct(leaf_record)),
                        ("enum".to_string(), GArg::Enum(choice_enum)),
                        ("collection".to_string(), GArg::Collection(collection)),
                    ])),
                    dependents: Vec::new(),
                },
            ]
        );
        assert_eq!(before.collections, vec![CollSpec::List { elem: scalar }]);
        assert_eq!(before.fn_base, 37);
        assert_eq!(before.functions, vec![(7, vec![scalar], reserved)]);
        assert_eq!(before.queue, vec![(7, vec![scalar], reserved)]);
        let draft_before = draft.encode().expect("stable draft encodes");
        let clone = registry
            .clone_for_generic_check()
            .expect("stable open registry clones");
        assert_eq!(stable_snapshot(&clone), before);
        assert_eq!(
            clone.reserve_fn_instance(7, vec![scalar], site(6)),
            Ok(reserved),
            "the clone reuses the exact nonzero function reservation"
        );
        assert_eq!(stable_snapshot(&clone), before);

        let mut local_draft = draft.clone();
        let local_before = local_draft.encode().expect("proof draft clone encodes");
        assert_eq!(local_before.bytes, draft_before.bytes);
        assert_eq!(local_before.image_id, draft_before.image_id);
        let local_id = clone
            .mint_type_instance(
                &mut local_draft,
                0,
                &[GArg::Scalar(ScalarType::Text)],
                site(28),
            )
            .expect("the clone mints a new isolated row");
        let TypeInstId::Record(local_record) = local_id else {
            panic!("Leaf is a struct template")
        };
        let clone_rows = stable_snapshot(&clone).rows;
        assert_eq!(clone_rows.len(), before.rows.len() + 1);
        assert_eq!(clone_rows[before.rows.len()].id, local_id);
        let marker_name = local_draft.intern_string("after-proof-clone");
        let after_local = local_draft.add_record_type(RecordTypeDef {
            name: marker_name,
            fields: Vec::new(),
        });
        assert_eq!(after_local.index(), local_record.index() + 1);

        let local_collection = clone
            .instantiate_list(&mut local_draft, GArg::Scalar(ScalarType::Text))
            .expect("aligned proof-clone collection owners mint");
        assert_eq!(
            stable_snapshot(&clone).collections,
            vec![
                CollSpec::List { elem: scalar },
                CollSpec::List {
                    elem: GArg::Scalar(ScalarType::Text),
                },
            ],
            "the proof clone gains a distinct collection without mutating the real registry",
        );
        clone.record_collection_payload_rejection(site(29), "Payload", "value", local_collection);
        clone.record_limit(site(30), "the proof clone reached its local bound");
        let outcome = clone.take_generic_diagnostics();
        assert_eq!(stable_snapshot(&registry), before);
        let draft_after = draft.encode().expect("real draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
        registry.adopt_generic_diagnostics(outcome);
        let adopted = registry.take_generic_diagnostics().into_ordered();
        assert_eq!(adopted.len(), 2);
        assert_eq!(adopted[0].code, Code::CheckInstantiationLimit.as_str());
        assert_eq!(adopted[1].code, Code::CheckUnsupported.as_str());
        assert!(
            registry
                .take_generic_diagnostics()
                .into_ordered()
                .is_empty()
        );
    }

    #[test]
    fn proof_clone_validates_every_ready_row_even_when_ids_are_duplicated() {
        let registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
            .expect("first Box row mints ready");
        registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Bool)], site(3))
            .expect("second Box row mints ready");
        let duplicate = {
            let mut generics = registry.generics.borrow_mut();
            let first_id = generics.type_insts[0].id;
            generics.type_insts[1].id = first_id;
            first_id
        };
        let expected = GenericInvariant::TypeIdentityCollision(duplicate);
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert!(matches!(
            registry.mint_type_instance(
                &mut draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(5),
            ),
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_eq!(registry.inst_spelling(duplicate), None);
        let arg = match duplicate {
            TypeInstId::Record(id) => GArg::Struct(id),
            TypeInstId::Enum(id) => GArg::Enum(id),
        };
        assert_eq!(garg_anchor_spelling(&registry, arg), Err(expected));
        assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn metadata_rejects_distinct_ids_with_the_same_semantic_cache_key() {
        let registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let first = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(3))
            .expect("first Box row mints ready");
        let duplicate = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Bool)], site(4))
            .expect("second Box row mints ready");
        {
            let mut generics = registry.generics.borrow_mut();
            let first_args = generics.type_insts[0].args.clone();
            let first_state = generics.type_insts[0].state.clone();
            generics.type_insts[1].args = first_args;
            generics.type_insts[1].state = first_state;
        }
        let expected = GenericInvariant::TypeInstantiationKeyCollision { first, duplicate };
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert!(matches!(
            registry.mint_type_instance(
                &mut draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(6),
            ),
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_eq!(registry.inst_spelling(duplicate), None);
        let arg = match duplicate {
            TypeInstId::Record(id) => GArg::Struct(id),
            TypeInstId::Enum(id) => GArg::Enum(id),
        };
        assert_eq!(garg_anchor_spelling(&registry, arg), Err(expected));
        assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn metadata_rejects_generic_ids_owned_by_declared_types() {
        let mut record_registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut record_draft = ImageDraft::new();
        let declared_record =
            add_declared_struct(&mut record_registry, &mut record_draft, "Plain", Vec::new());
        record_registry
            .mint_type_instance(
                &mut record_draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(4),
            )
            .expect("generic record mints ready");
        record_registry.generics.borrow_mut().type_insts[0].id =
            TypeInstId::Record(declared_record);
        let record_expected =
            GenericInvariant::TypeIdentityCollision(TypeInstId::Record(declared_record));
        let record_before = stable_snapshot(&record_registry);
        let record_draft_before = draft_snapshot(&record_draft);

        assert!(matches!(
            record_registry.clone_for_generic_check(),
            Err(found) if found == record_expected
        ));
        let (record_body, builds) = count_metadata_directory_builds(|| {
            record_registry.type_inst_body(TypeInstId::Record(declared_record))
        });
        assert!(matches!(record_body, Err(found) if found == record_expected));
        assert_eq!(builds, 1);
        assert!(matches!(
            record_registry.static_struct_projection("Plain"),
            Err(found) if found == record_expected
        ));
        assert!(matches!(
            record_registry.static_named_type_projection("Plain"),
            Err(found) if found == record_expected
        ));
        assert_eq!(
            record_registry.struct_field_projection(declared_record, "missing"),
            Err(record_expected)
        );
        assert_eq!(
            garg_anchor_spelling(&record_registry, GArg::Struct(declared_record)),
            Err(record_expected)
        );
        assert_eq!(stable_snapshot(&record_registry), record_before);
        assert_eq!(draft_snapshot(&record_draft), record_draft_before);

        let mut enum_registry = registry(vec![enum_template("Choice", name("T"))]);
        let mut enum_draft = ImageDraft::new();
        let declared_name = enum_draft.intern_string("PlainChoice");
        let declared_enum = enum_draft.add_enum_type(EnumTypeDef {
            name: declared_name,
            variants: Vec::new(),
        });
        enum_registry.enums.push(EnumInfo {
            enum_id: declared_enum,
            name: "PlainChoice".to_string(),
            variants: Vec::new(),
        });
        enum_registry
            .mint_type_instance(
                &mut enum_draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(5),
            )
            .expect("generic enum mints ready");
        enum_registry.generics.borrow_mut().type_insts[0].id = TypeInstId::Enum(declared_enum);
        let enum_expected =
            GenericInvariant::TypeIdentityCollision(TypeInstId::Enum(declared_enum));
        let enum_before = stable_snapshot(&enum_registry);
        let enum_draft_before = draft_snapshot(&enum_draft);

        assert!(matches!(
            enum_registry.clone_for_generic_check(),
            Err(found) if found == enum_expected
        ));
        let (variants, builds) =
            count_metadata_directory_builds(|| enum_registry.enum_variants(declared_enum));
        assert_eq!(variants, Err(enum_expected));
        assert_eq!(builds, 1);
        assert!(matches!(
            enum_registry.static_enum_projection("PlainChoice"),
            Err(found) if found == enum_expected
        ));
        assert!(matches!(
            enum_registry.static_named_type_projection("PlainChoice"),
            Err(found) if found == enum_expected
        ));
        let (anchor, builds) =
            count_metadata_directory_builds(|| enum_registry.enum_anchor_spelling(declared_enum));
        assert_eq!(anchor, Err(enum_expected));
        assert_eq!(builds, 1);
        assert_eq!(
            garg_anchor_spelling(&enum_registry, GArg::Enum(declared_enum)),
            Err(enum_expected)
        );
        assert_eq!(stable_snapshot(&enum_registry), enum_before);
        assert_eq!(draft_snapshot(&enum_draft), enum_draft_before);
    }

    #[test]
    fn metadata_rejects_generic_ids_owned_by_resource_records() {
        let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let resource = add_resource_record(&mut registry, &mut draft, "Account");
        registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(5))
            .expect("generic record mints ready");
        registry.generics.borrow_mut().type_insts[0].id = TypeInstId::Record(resource);
        let expected = GenericInvariant::TypeIdentityCollision(TypeInstId::Record(resource));
        let owner_before = metadata_owner_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert!(matches!(
            registry.mint_type_instance(
                &mut draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(6),
            ),
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            registry.instantiation_of(TypeInstId::Record(resource)),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert!(matches!(
            registry.type_inst_body(TypeInstId::Record(resource)),
            Err(found) if found == expected
        ));
        assert!(matches!(
            registry.static_record_projection("Account"),
            Err(found) if found == expected
        ));
        assert_eq!(
            registry.product_field_projection(resource, "missing"),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(registry.inst_spelling(TypeInstId::Record(resource)), None);
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Struct(resource)),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Scalar(ScalarType::Int)]),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    }

    #[test]
    fn metadata_rejects_resource_record_collisions_with_static_record_owners() {
        let mut struct_registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut struct_draft = ImageDraft::new();
        let resource = add_resource_record(&mut struct_registry, &mut struct_draft, "Account");
        add_declared_struct(&mut struct_registry, &mut struct_draft, "Plain", Vec::new());
        struct_registry.structs[0].type_id = resource;
        let expected = GenericInvariant::TypeIdentityCollision(TypeInstId::Record(resource));
        let owner_before = metadata_owner_snapshot(&struct_registry);
        let draft_before = draft_snapshot(&struct_draft);

        assert!(matches!(
            struct_registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_metadata_unchanged(
            &struct_registry,
            &struct_draft,
            &owner_before,
            &draft_before,
        );
        assert!(matches!(
            struct_registry.mint_type_instance(
                &mut struct_draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(7),
            ),
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_metadata_unchanged(
            &struct_registry,
            &struct_draft,
            &owner_before,
            &draft_before,
        );
        assert_eq!(
            struct_registry.validate_type_arguments(&[GArg::Struct(resource)]),
            Err(expected)
        );
        assert_metadata_unchanged(
            &struct_registry,
            &struct_draft,
            &owner_before,
            &draft_before,
        );
        assert_eq!(
            struct_registry.instantiation_of(TypeInstId::Record(resource)),
            Err(expected)
        );
        assert_metadata_unchanged(
            &struct_registry,
            &struct_draft,
            &owner_before,
            &draft_before,
        );
        assert_eq!(
            garg_anchor_spelling(&struct_registry, GArg::Struct(resource)),
            Err(expected)
        );
        assert_metadata_unchanged(
            &struct_registry,
            &struct_draft,
            &owner_before,
            &draft_before,
        );
        assert_eq!(
            struct_registry.validate_durable_value_metadata([GArg::Scalar(ScalarType::Int)]),
            Err(expected)
        );
        assert_metadata_unchanged(
            &struct_registry,
            &struct_draft,
            &owner_before,
            &draft_before,
        );
        assert!(matches!(
            ValueGraph::build(&struct_registry),
            Err(found) if found == expected
        ));
        assert_metadata_unchanged(
            &struct_registry,
            &struct_draft,
            &owner_before,
            &draft_before,
        );

        let mut group_registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut group_draft = ImageDraft::new();
        let resource = add_resource_record(&mut group_registry, &mut group_draft, "Account");
        add_resource_group(&mut group_registry, &mut group_draft, 0, "profile");
        group_registry.records[0].groups[0].type_id = resource;
        let expected = GenericInvariant::TypeIdentityCollision(TypeInstId::Record(resource));
        let owner_before = metadata_owner_snapshot(&group_registry);
        let draft_before = draft_snapshot(&group_draft);

        assert!(matches!(
            group_registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
        assert!(matches!(
            group_registry.mint_type_instance(
                &mut group_draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(8),
            ),
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
        assert_eq!(
            group_registry.validate_type_arguments(&[GArg::Group(resource)]),
            Err(expected)
        );
        assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
        assert_eq!(
            group_registry.instantiation_of(TypeInstId::Record(resource)),
            Err(expected)
        );
        assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
        assert_eq!(
            garg_anchor_spelling(&group_registry, GArg::Group(resource)),
            Err(expected)
        );
        assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
        assert_eq!(
            group_registry.validate_durable_value_metadata([GArg::Scalar(ScalarType::Int)]),
            Err(expected)
        );
        assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
        assert!(matches!(
            ValueGraph::build(&group_registry),
            Err(found) if found == expected
        ));
        assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
    }

    #[test]
    fn metadata_rejects_cyclic_ready_arguments_before_display_or_durable_use() {
        let registry = registry(vec![
            template("Inner", vec![("value", name("T"))]),
            template("Outer", vec![("value", name("T"))]),
        ]);
        let mut draft = ImageDraft::new();
        let inner = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(6))
            .expect("Inner row mints ready");
        let TypeInstId::Record(inner_id) = inner else {
            panic!("Inner is a record template")
        };
        let outer = registry
            .mint_type_instance(&mut draft, 1, &[GArg::Struct(inner_id)], site(7))
            .expect("Outer row may depend on an earlier row");
        let TypeInstId::Record(outer_id) = outer else {
            panic!("Outer is a record template")
        };

        assert_eq!(
            registry.inst_spelling(outer).as_deref(),
            Some("Outer<Inner<int>>")
        );
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Struct(outer_id)).as_deref(),
            Ok("Outer[Inner[int]]")
        );
        registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Struct(outer_id)];
        let expected = GenericInvariant::TypeArgumentOrderViolation {
            owner: inner,
            target: outer,
        };
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_eq!(registry.inst_spelling(inner), None);
        let view = registry.metadata_view();
        let mut metadata =
            MetadataScratch::try_new(&view).expect("identity directory remains valid");
        assert_eq!(
            view.validate_args_with(
                std::slice::from_ref(&GArg::Struct(inner_id)),
                None,
                &mut metadata,
            ),
            Err(expected),
        );
        drop(view);
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Struct(inner_id)),
            Err(expected)
        );
        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Struct(inner_id)]),
            Err(expected)
        );
        assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn generic_predecessor_order_survives_collection_expansion() {
        let mut root_template = template("Root", Vec::new());
        root_template.type_params = vec![("T".to_string(), None), ("U".to_string(), None)];
        let mut registry = registry(vec![
            template("Inner", vec![("value", name("T"))]),
            template("Outer", vec![("value", name("T"))]),
            root_template,
        ]);
        let mut draft = ImageDraft::new();
        let inner = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(8))
            .expect("Inner row mints ready");
        let TypeInstId::Record(inner_id) = inner else {
            panic!("Inner is a record template")
        };
        let outer = registry
            .mint_type_instance(&mut draft, 1, &[GArg::Struct(inner_id)], site(9))
            .expect("Outer row may depend on an earlier row");
        let TypeInstId::Record(outer_id) = outer else {
            panic!("Outer is a record template")
        };
        let list = registry
            .instantiate_list(&mut draft, GArg::Struct(outer_id))
            .expect("collection over a valid predecessor graph mints");
        let nested = registry
            .instantiate_list(&mut draft, GArg::Collection(list))
            .expect("a predecessor collection may be nested");
        let root = registry
            .mint_type_instance(
                &mut draft,
                2,
                &[GArg::Collection(nested), GArg::Struct(inner_id)],
                site(10),
            )
            .expect("Root arguments point only to predecessors");
        let TypeInstId::Record(root_id) = root else {
            panic!("Root is a record template")
        };
        let expected_label = "Root<List<List<Outer<Inner<int>>>>, Inner<int>>";
        assert_eq!(
            registry.inst_spelling(root).as_deref(),
            Some(expected_label)
        );
        let graph = ValueGraph::build(&registry).expect("the valid predecessor graph builds");
        let label_for = |id| {
            let node = graph
                .nodes
                .iter()
                .position(|node| *node == ValueNode::Record(id))
                .expect("the generic record has one graph node");
            graph.labels[node].as_str()
        };
        assert_eq!(label_for(inner_id), "Inner<int>");
        assert_eq!(label_for(outer_id), "Outer<Inner<int>>");
        assert_eq!(label_for(root_id), expected_label);
        assert!(graph.labels.iter().all(|label| label != "instantiation"));

        let carrier = add_declared_struct(
            &mut registry,
            &mut draft,
            "Carrier",
            vec![
                ("collection", GArg::Collection(nested)),
                ("inner", GArg::Struct(inner_id)),
            ],
        );
        registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Collection(nested)];
        let expected = GenericInvariant::TypeArgumentOrderViolation {
            owner: inner,
            target: outer,
        };
        let owner_before = metadata_owner_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        for roots in [
            vec![GArg::Collection(nested), GArg::Struct(inner_id)],
            vec![GArg::Struct(inner_id), GArg::Collection(nested)],
        ] {
            let view = registry.metadata_view();
            let mut metadata =
                MetadataScratch::try_new(&view).expect("identity directory remains valid");
            assert_eq!(
                view.validate_args_with(&roots, None, &mut metadata),
                Err(expected),
                "root order cannot change predecessor validation",
            );
            drop(view);
            assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
            assert_eq!(
                registry.validate_durable_value_metadata(roots),
                Err(expected),
                "durable root order cannot change predecessor validation",
            );
            assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        }
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(registry.inst_spelling(inner), None);
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(registry.inst_spelling(root), None);
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Struct(inner_id)),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Collection(nested)),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Struct(root_id)),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Struct(root_id)]),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Struct(carrier)]),
            Err(expected)
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    }

    #[test]
    fn collection_predecessor_validation_preserves_missing_target_precedence() {
        for (corrupt, missing) in [(0, 0), (1, 1), (2, 2)] {
            let registry = registry(vec![template("A", vec![("value", name("T"))])]);
            let mut draft = ImageDraft::new();
            let a = registry
                .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(11))
                .expect("A<int> mints ready");
            let first = registry
                .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
                .expect("first collection mints aligned");
            assert_eq!(first, 0);
            if corrupt == 1 {
                let second = registry
                    .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Bool))
                    .expect("forward collection target exists");
                assert_eq!(second, 1);
            }
            registry.collections.borrow_mut()[0] = CollSpec::List {
                elem: GArg::Collection(missing),
            };
            registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Collection(0)];
            let owner_before = metadata_owner_snapshot(&registry);
            let draft_before = draft_snapshot(&draft);
            assert_eq!(
                registry.validate_type_arguments(&[match a {
                    TypeInstId::Record(id) => GArg::Struct(id),
                    TypeInstId::Enum(id) => GArg::Enum(id),
                }]),
                Err(GenericInvariant::TypeArgumentTargetMissing(
                    GArg::Collection(missing),
                )),
            );
            assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
            assert!(matches!(
                registry.clone_for_generic_check(),
                Err(GenericInvariant::TypeArgumentTargetMissing(GArg::Collection(found)))
                    if found == missing
            ));
            assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
            assert_eq!(
                garg_anchor_spelling(&registry, GArg::Collection(0)),
                Err(GenericInvariant::TypeArgumentTargetMissing(
                    GArg::Collection(missing),
                )),
            );
            assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        }

        let registry = registry(Vec::new());
        *registry.collections.borrow_mut() = vec![
            CollSpec::List {
                elem: GArg::Scalar(ScalarType::Int),
            },
            CollSpec::List {
                elem: GArg::Collection(0),
            },
        ];
        let before = stable_snapshot(&registry);
        assert_eq!(
            registry.validate_type_arguments(&[GArg::Collection(1)]),
            Ok(())
        );
        assert_eq!(stable_snapshot(&registry), before);
    }

    #[test]
    fn collection_predecessor_validation_preserves_source_order_on_first_visit_and_revisit() {
        struct Rows {
            registry: TypeRegistry,
            draft: ImageDraft,
            owner: TypeInstId,
            forward: TypeInstId,
            later: TypeInstId,
            orphan: TypeId,
        }

        fn rows() -> Rows {
            let registry = registry(vec![
                template("Earlier", vec![("value", name("T"))]),
                template("Owner", vec![("value", name("T"))]),
                template("Forward", vec![("value", name("T"))]),
                template("Later", vec![("value", name("T"))]),
            ]);
            let mut draft = ImageDraft::new();
            let mut ids = Vec::new();
            for template in 0..4 {
                ids.push(
                    registry
                        .mint_type_instance(
                            &mut draft,
                            template,
                            &[GArg::Scalar(ScalarType::Int)],
                            site(template as u32 + 20),
                        )
                        .expect("ordered seed row mints Ready"),
                );
            }
            let orphan_name = draft.intern_string("Orphan");
            let orphan = draft.add_record_type(RecordTypeDef {
                name: orphan_name,
                fields: Vec::new(),
            });
            Rows {
                registry,
                draft,
                owner: ids[1],
                forward: ids[2],
                later: ids[3],
                orphan,
            }
        }

        let missing = rows();
        let TypeInstId::Record(forward) = missing.forward else {
            panic!("Forward is record-shaped")
        };
        missing
            .registry
            .collections
            .borrow_mut()
            .push(CollSpec::Map {
                key: GArg::Struct(missing.orphan),
                value: GArg::Struct(forward),
            });
        missing.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(0)];
        let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(missing.orphan));
        let owner_before = metadata_owner_snapshot(&missing.registry);
        let draft_before = draft_snapshot(&missing.draft);
        let (observed, builds) = count_metadata_directory_builds(|| {
            missing
                .registry
                .validate_type_arguments(&[match missing.owner {
                    TypeInstId::Record(id) => GArg::Struct(id),
                    TypeInstId::Enum(id) => GArg::Enum(id),
                }])
        });
        assert_eq!(observed, Err(expected));
        assert_eq!(builds, 1);
        assert_metadata_unchanged(
            &missing.registry,
            &missing.draft,
            &owner_before,
            &draft_before,
        );

        let ordered = rows();
        let TypeInstId::Record(forward) = ordered.forward else {
            panic!("Forward is record-shaped")
        };
        let TypeInstId::Record(later) = ordered.later else {
            panic!("Later is record-shaped")
        };
        ordered
            .registry
            .collections
            .borrow_mut()
            .push(CollSpec::Map {
                key: GArg::Struct(forward),
                value: GArg::Struct(later),
            });
        ordered.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(0)];
        let expected = GenericInvariant::TypeArgumentOrderViolation {
            owner: ordered.owner,
            target: ordered.forward,
        };
        let owner_before = metadata_owner_snapshot(&ordered.registry);
        let draft_before = draft_snapshot(&ordered.draft);
        assert_eq!(
            ordered
                .registry
                .validate_type_arguments(&[match ordered.owner {
                    TypeInstId::Record(id) => GArg::Struct(id),
                    TypeInstId::Enum(id) => GArg::Enum(id),
                }]),
            Err(expected),
            "Map key order wins over a later value violation"
        );
        assert_metadata_unchanged(
            &ordered.registry,
            &ordered.draft,
            &owner_before,
            &draft_before,
        );

        let nested = rows();
        let TypeInstId::Record(forward) = nested.forward else {
            panic!("Forward is record-shaped")
        };
        *nested.registry.collections.borrow_mut() = vec![
            CollSpec::List {
                elem: GArg::Struct(nested.orphan),
            },
            CollSpec::Map {
                key: GArg::Collection(0),
                value: GArg::Struct(forward),
            },
        ];
        nested.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(1)];
        let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(nested.orphan));
        assert_eq!(
            nested
                .registry
                .validate_type_arguments(&[match nested.owner {
                    TypeInstId::Record(id) => GArg::Struct(id),
                    TypeInstId::Enum(id) => GArg::Enum(id),
                }]),
            Err(expected),
            "nested key traversal precedes the Map value"
        );

        let revisit = rows();
        let TypeInstId::Record(forward) = revisit.forward else {
            panic!("Forward is record-shaped")
        };
        revisit
            .registry
            .collections
            .borrow_mut()
            .push(CollSpec::List {
                elem: GArg::Struct(forward),
            });
        revisit.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(0)];
        let owner_arg = match revisit.owner {
            TypeInstId::Record(id) => GArg::Struct(id),
            TypeInstId::Enum(id) => GArg::Enum(id),
        };
        let view = revisit.registry.metadata_view();
        let mut metadata = MetadataScratch::try_new(&view).expect("directory builds");
        assert_eq!(
            view.validate_args_with(&[GArg::Collection(0), owner_arg], None, &mut metadata,),
            Err(GenericInvariant::TypeArgumentOrderViolation {
                owner: revisit.owner,
                target: revisit.forward,
            }),
            "a root previsit cannot hide the later parent-context violation"
        );
    }

    #[test]
    fn proof_clone_refuses_every_unstable_fill_or_diagnostic_owner_state() {
        let mut registry = registry(vec![template("Good", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
            .expect("stable seed mints");

        let id = registry.generics.borrow().type_insts[0].id;
        registry.generics.get_mut().build_invariant = Some(GenericInvariant::ReadyBodyMissing(id));
        let before = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
        registry.generics.get_mut().build_invariant = None;

        registry.generics.borrow_mut().fill_batch_start = Some(0);
        let before = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
        registry.generics.borrow_mut().fill_batch_start = None;

        let key = TypeInstKey::from(registry.generics.borrow().type_insts[0].id);
        registry.generics.borrow_mut().fill_rows.insert(key, 0);
        let before = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
        registry.generics.borrow_mut().fill_rows.clear();

        registry.generics.borrow_mut().fill_stack.push(0);
        let before = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
        registry.generics.borrow_mut().fill_stack.clear();

        registry
            .generics
            .borrow_mut()
            .fill_failures
            .push((0, ResolveRefusal::Unsupported));
        let before = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
        registry.generics.borrow_mut().fill_failures.clear();

        registry.generics.borrow_mut().type_insts[0]
            .dependents
            .push(0);
        let before = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
        registry.generics.borrow_mut().type_insts[0]
            .dependents
            .clear();

        registry.record_limit(site(9), "the real owner is no longer open");
        let pending = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::LimitOwnerNotOpen
            ))
        ));
        assert_eq!(stable_snapshot(&registry), pending);
        let _ = registry.take_generic_diagnostics();
        let reported = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::LimitOwnerNotOpen
            ))
        ));
        assert!(matches!(pending.limit, StableLimit::Pending(_)));
        assert!(matches!(reported.limit, StableLimit::Reported));
        assert_eq!(stable_snapshot(&registry), reported);
        registry.generics.borrow_mut().limit = LimitState::Open;

        let body = {
            let mut generics = registry.generics.borrow_mut();
            let prior = std::mem::replace(
                &mut generics.type_insts[0].state,
                TypeInstState::Rejected(ResolveRefusal::Unsupported),
            );
            let TypeInstState::Ready(body) = prior else {
                panic!("seed row is ready")
            };
            body
        };
        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Filling { staged: Some(body) };
        let before = stable_snapshot(&registry);
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
    }

    /// An active mutable borrow of the generic owner is a private
    /// coherence failure, not a RefCell unwind.
    #[test]
    fn proof_clone_generics_borrow_conflict_fails_without_unwinding() {
        let registry = registry(Vec::new());
        let before = stable_snapshot(&registry);
        let guard = registry.generics.borrow_mut();
        let result = registry.clone_for_generic_check();
        drop(guard);

        assert!(matches!(
            result,
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
    }

    /// Collection-owner contention is classified independently from
    /// the generic owner and cannot unwind through RefCell.
    #[test]
    fn proof_clone_collections_borrow_conflict_fails_without_unwinding() {
        let registry = registry(Vec::new());
        let before = stable_snapshot(&registry);
        let guard = registry.collections.borrow_mut();
        let result = registry.clone_for_generic_check();
        drop(guard);

        assert!(matches!(
            result,
            Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(stable_snapshot(&registry), before);
    }

    /// Committed reserved rows expose their exact arguments through
    /// the dedicated Option/Result readers.
    #[test]
    fn ready_reserved_option_and_result_readers_preserve_arguments() {
        let registry = registry(reserved_templates());
        let mut draft = ImageDraft::new();
        let option = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
            .expect("ready Option mints");
        let result_template = registry.reserved_template(Reserved::Result);
        let result = registry
            .mint_type_instance(
                &mut draft,
                result_template,
                &[
                    GArg::Scalar(ScalarType::Text),
                    GArg::Scalar(ScalarType::Bool),
                ],
                site(3),
            )
            .expect("ready Result mints");
        let TypeInstId::Enum(result) = result else {
            panic!("the reserved Result template is enum-shaped")
        };

        assert_eq!(
            registry.as_option(option),
            Ok(Some(GArg::Scalar(ScalarType::Int)))
        );
        assert_eq!(
            registry.as_result(result),
            Ok(Some((
                GArg::Scalar(ScalarType::Text),
                GArg::Scalar(ScalarType::Bool)
            )))
        );
        assert!(matches!(
            registry.type_inst_body(TypeInstId::Enum(option)),
            Ok(Some(InstBody::Enum(ref variants)))
                if variants.len() == 2
                    && variants[0].name == "none"
                    && variants[0].payload.is_empty()
                    && variants[1].name == "some"
                    && variants[1].payload
                        == vec![("value".to_string(), GArg::Scalar(ScalarType::Int))]
        ));
        assert!(matches!(
            registry.type_inst_body(TypeInstId::Enum(result)),
            Ok(Some(InstBody::Enum(ref variants)))
                if variants.len() == 2
                    && variants[0].name == "ok"
                    && variants[0].payload
                        == vec![("value".to_string(), GArg::Scalar(ScalarType::Text))]
                    && variants[1].name == "err"
                    && variants[1].payload
                        == vec![("value".to_string(), GArg::Scalar(ScalarType::Bool))]
        ));
        assert_eq!(
            registry.enum_anchor_spelling(option).unwrap().as_deref(),
            Some("Option[int]")
        );
        assert_eq!(
            registry.enum_anchor_spelling(result).unwrap().as_deref(),
            Some("Result[string,bool]")
        );
    }

    #[test]
    fn reserved_readers_require_the_fixed_member_contract_not_only_template_agreement() {
        let mut registry = registry(reserved_templates());
        let mut draft = ImageDraft::new();
        let option = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(4))
            .expect("ready Option mints");
        let option_template = registry.reserved_template(Reserved::Option);
        let option_row = registry
            .generics
            .borrow()
            .type_insts
            .iter()
            .position(|inst| inst.id == TypeInstId::Enum(option))
            .expect("Option row exists");
        let TemplateBody::Enum(template_variants) =
            &mut registry.type_templates[option_template].body
        else {
            panic!("Option template is enum-shaped")
        };
        template_variants[OPTION_NONE as usize].name = "nil".to_string();
        let mut generics = registry.generics.borrow_mut();
        let TypeInstState::Ready(InstBody::Enum(variants)) =
            &mut generics.type_insts[option_row].state
        else {
            panic!("Option row is Ready and enum-shaped")
        };
        variants[OPTION_NONE as usize].name = "nil".to_string();
        drop(generics);
        let expected = GenericInvariant::ReadyBodyShapeMismatch(TypeInstId::Enum(option));
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert_eq!(registry.as_option(option), Err(expected));
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn metadata_directory_builds_follow_immutable_operation_boundaries() {
        let registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let (fresh, fresh_builds) = count_metadata_directory_builds(|| {
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        });
        let id = fresh.expect("the cold Box row mints");
        assert_eq!(fresh_builds, 2, "preflight plus post-settlement proof");

        let (replayed, replay_builds) = count_metadata_directory_builds(|| {
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(3))
        });
        assert_eq!(replayed, Ok(id));
        assert_eq!(replay_builds, 1, "a Ready cache hit uses its one proof");

        let list = registry
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("the List metadata mints");
        let (spelling, spelling_builds) =
            count_metadata_directory_builds(|| registry.collection_spelling(list));
        assert_eq!(spelling, "List<int>");
        assert_eq!(
            spelling_builds, 0,
            "best-effort presentation does not build the semantic directory"
        );

        let (blocked, session_builds) = count_metadata_directory_builds(|| {
            registry
                .with_metadata_session(|_| {
                    Ok::<_, GenericInvariant>((
                        registry.generics.try_borrow_mut().is_err(),
                        registry.collections.try_borrow_mut().is_err(),
                    ))
                })
                .expect("the immutable metadata session opens")
        });
        assert_eq!(blocked, (true, true));
        assert_eq!(session_builds, 1);
        assert!(registry.generics.try_borrow_mut().is_ok());
        assert!(registry.collections.try_borrow_mut().is_ok());

        let map = registry
            .instantiate_map(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                GArg::Scalar(ScalarType::Text),
            )
            .expect("dropping the session permits a later metadata mutation");
        let (observed, post_mutation_builds) = count_metadata_directory_builds(|| {
            registry.with_metadata_session(|metadata| metadata.collection_spec(map))
        });
        assert_eq!(
            observed,
            Ok(CollSpec::Map {
                key: GArg::Scalar(ScalarType::Int),
                value: GArg::Scalar(ScalarType::Text),
            })
        );
        assert_eq!(
            post_mutation_builds, 1,
            "a later read builds one fresh directory after mutation"
        );
    }

    #[test]
    fn validated_nested_collection_spelling_reuses_one_metadata_session() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        let inner = registry
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("the inner List metadata mints");
        let outer = registry
            .instantiate_map(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                GArg::Collection(inner),
            )
            .expect("the outer Map metadata mints");
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        let (spellings, builds) = count_metadata_directory_builds(|| {
            registry.with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>((
                    metadata.garg_spelling(GArg::Collection(outer))?,
                    metadata.garg_spelling(GArg::Collection(outer))?,
                ))
            })
        });

        assert_eq!(
            spellings,
            Ok((
                "Map<int, List<int>>".to_string(),
                "Map<int, List<int>>".to_string(),
            ))
        );
        assert_eq!(
            builds, 1,
            "repeated nested collection spelling reuses one validated directory"
        );
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn deep_collection_spelling_and_anchor_use_iterative_activity_owners() {
        let make_registry = registry;
        let registry = make_registry(Vec::new());
        let depth = MAX_INSTANTIATIONS;
        {
            let mut collections = registry.collections.borrow_mut();
            for index in 0..depth {
                let elem = if index == 0 {
                    GArg::Scalar(ScalarType::Int)
                } else {
                    GArg::Collection((index - 1) as u16)
                };
                collections.push(CollSpec::List { elem });
            }
        }
        let root = (depth - 1) as u16;
        let owner_before = stable_snapshot(&registry);
        let expected_display = format!("{}int{}", "List<".repeat(depth), ">".repeat(depth));
        let expected_anchor = format!("{}int{}", "List[".repeat(depth), "]".repeat(depth));

        let (display, display_builds) =
            count_metadata_directory_builds(|| registry.collection_spelling(root));
        assert_eq!(display, expected_display);
        assert_eq!(display_builds, 0);
        let (anchor, anchor_builds) = count_metadata_directory_builds(|| {
            garg_anchor_spelling(&registry, GArg::Collection(root))
        });
        assert_eq!(anchor, Ok(expected_anchor));
        assert_eq!(anchor_builds, 1);
        assert_eq!(stable_snapshot(&registry), owner_before);

        let cyclic = make_registry(Vec::new());
        cyclic.collections.borrow_mut().push(CollSpec::List {
            elem: GArg::Collection(0),
        });
        let cyclic_before = stable_snapshot(&cyclic);
        assert_eq!(cyclic.collection_spelling(0), "collection");
        assert_eq!(
            garg_anchor_spelling(&cyclic, GArg::Collection(0)),
            Err(GenericInvariant::TypeArgumentTargetMissing(
                GArg::Collection(0)
            ))
        );
        assert_eq!(stable_snapshot(&cyclic), cyclic_before);
    }

    #[test]
    fn metadata_directory_construction_failure_never_enters_a_session() {
        let registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let first = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
            .expect("first Box row mints ready");
        let duplicate = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Bool)], site(3))
            .expect("second Box row mints ready");
        {
            let mut generics = registry.generics.borrow_mut();
            let first_args = generics.type_insts[0].args.clone();
            let first_state = generics.type_insts[0].state.clone();
            generics.type_insts[1].args = first_args;
            generics.type_insts[1].state = first_state;
        }
        let expected = GenericInvariant::TypeInstantiationKeyCollision { first, duplicate };
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);
        let entered = Cell::new(false);

        let (observed, builds) = count_metadata_directory_builds(|| {
            registry.with_metadata_session(|_| {
                entered.set(true);
                Ok::<(), GenericInvariant>(())
            })
        });

        assert_eq!(observed, Err(expected));
        assert_eq!(builds, 1, "directory construction fails on its first pass");
        assert!(!entered.get(), "a failed directory never yields a session");
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn one_metadata_session_classifies_both_reserved_families() {
        let registry = registry(reserved_templates());
        let mut draft = ImageDraft::new();
        let option = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
            .expect("Ready Option mints");
        let result_template = registry.reserved_template(Reserved::Result);
        let TypeInstId::Enum(result) = registry
            .mint_type_instance(
                &mut draft,
                result_template,
                &[
                    GArg::Scalar(ScalarType::Text),
                    GArg::Scalar(ScalarType::Bool),
                ],
                site(3),
            )
            .expect("Ready Result mints")
        else {
            panic!("Result is an enum template")
        };

        let (classified, builds) = count_metadata_directory_builds(|| {
            registry.with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>((
                    metadata.reserved_instantiation(option)?,
                    metadata.reserved_instantiation(result)?,
                ))
            })
        });
        assert_eq!(
            classified,
            Ok((
                Some(ReservedEnumArgs::Option(GArg::Scalar(ScalarType::Int))),
                Some(ReservedEnumArgs::Result(
                    GArg::Scalar(ScalarType::Text),
                    GArg::Scalar(ScalarType::Bool),
                )),
            ))
        );
        assert_eq!(builds, 1);
    }

    #[test]
    fn metadata_session_replays_its_first_failure_without_reusing_scratch() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        let list = registry
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("aligned owners publish List<int>");
        let orphan_name = draft.intern_string("Orphan");
        let orphan = draft.add_record_type(RecordTypeDef {
            name: orphan_name,
            fields: Vec::new(),
        });
        registry.collections.borrow_mut()[list as usize] = CollSpec::List {
            elem: GArg::Struct(orphan),
        };
        let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(orphan));

        let (observed, builds) = count_metadata_directory_builds(|| {
            registry.with_metadata_session(|metadata| {
                let first = metadata.validate_type_arguments(&[GArg::Collection(list)]);
                let collection_replay = metadata.collection_spec(list);
                let unrelated_replay = metadata.validate_type_arguments(&[GArg::Param(7)]);
                Ok::<_, GenericInvariant>((first, collection_replay, unrelated_replay))
            })
        });

        assert_eq!(observed, Ok((Err(expected), Err(expected), Err(expected))));
        assert_eq!(builds, 1, "a poisoned session never rebuilds or resumes");
    }

    /// Provisional and rejected reserved rows expose neither arguments nor body
    /// shape through any semantic reader, for both Option and Result.
    #[test]
    fn filling_and_rejected_reserved_option_and_result_rows_are_hidden() {
        let registry = registry(reserved_templates());
        let mut draft = ImageDraft::new();
        let option = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
            .expect("ready Option mints");
        let result_template = registry.reserved_template(Reserved::Result);
        let result = registry
            .mint_type_instance(
                &mut draft,
                result_template,
                &[
                    GArg::Scalar(ScalarType::Text),
                    GArg::Scalar(ScalarType::Bool),
                ],
                site(3),
            )
            .expect("ready Result mints");
        let TypeInstId::Enum(result) = result else {
            panic!("the reserved Result template is enum-shaped")
        };

        {
            let mut generics = registry.generics.borrow_mut();
            for id in [option, result] {
                let inst = generics
                    .type_insts
                    .iter_mut()
                    .find(|inst| inst.id == TypeInstId::Enum(id))
                    .expect("minted reserved row exists");
                let prior = std::mem::replace(
                    &mut inst.state,
                    TypeInstState::Rejected(ResolveRefusal::Unsupported),
                );
                let TypeInstState::Ready(body) = prior else {
                    panic!("minted reserved row is ready")
                };
                inst.state = TypeInstState::Filling { staged: Some(body) };
            }
        }
        assert_reserved_rows_hidden(&registry, option, result);

        {
            let mut generics = registry.generics.borrow_mut();
            for id in [option, result] {
                let inst = generics
                    .type_insts
                    .iter_mut()
                    .find(|inst| inst.id == TypeInstId::Enum(id))
                    .expect("minted reserved row exists");
                inst.state = TypeInstState::Rejected(ResolveRefusal::Unsupported);
            }
        }
        assert_reserved_rows_hidden(&registry, option, result);
    }

    fn assert_reserved_rows_hidden(registry: &TypeRegistry, option: EnumId, result: EnumId) {
        assert!(registry.as_option(option).unwrap().is_none());
        assert!(registry.as_result(result).unwrap().is_none());
        for id in [option, result] {
            let id = TypeInstId::Enum(id);
            assert!(registry.instantiation_of(id).unwrap().is_none());
            assert!(registry.type_inst_body(id).unwrap().is_none());
        }
        assert!(registry.enum_variants(option).unwrap().is_none());
        assert!(registry.enum_variants(result).unwrap().is_none());
        assert!(registry.enum_anchor_spelling(option).unwrap().is_none());
        assert!(registry.enum_anchor_spelling(result).unwrap().is_none());
    }

    /// Absence of the reserved template is a typed owner failure, not
    /// an expectation unwind.
    #[test]
    fn missing_reserved_template_fails_without_unwinding() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        let registry_before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("empty draft encodes");
        let invariant = take_generic_invariant(registry.instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            site(2),
        ));
        let draft_after = draft.encode().expect("failed draft still encodes");

        assert_eq!(
            invariant,
            GenericInvariant::ReservedTemplateMissing(Reserved::Option)
        );
        assert_eq!(stable_snapshot(&registry), registry_before);
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    /// A corrupted reserved-template kind fails before it can
    /// unwind or expose the record id as an Option enum id.
    #[test]
    fn reserved_option_wrong_kind_fails_without_unwinding() {
        let mut registry = registry(reserved_templates());
        registry.type_templates[0].body =
            TemplateBody::Struct(vec![("value".to_string(), name("T"))]);
        let mut draft = ImageDraft::new();
        let registry_before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("empty draft encodes");
        let invariant = take_generic_invariant(registry.instantiate_reserved_option(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            site(2),
        ));
        let draft_after = draft.encode().expect("failed draft still encodes");

        assert_eq!(
            invariant,
            GenericInvariant::TemplateKindMismatch {
                template: 0,
                expected: TypeInstKind::Enum,
                actual: TypeInstKind::Struct,
            }
        );
        assert_eq!(stable_snapshot(&registry), registry_before);
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn resolve_garg_reports_exact_missing_option_and_result_templates() {
        for (reserved, head) in [(Reserved::Option, "Option"), (Reserved::Result, "Result")] {
            let templates = reserved_templates()
                .into_iter()
                .filter(|template| template.reserved != Some(reserved))
                .collect();
            let registry = registry(templates);
            let args = match reserved {
                Reserved::Option => vec![name("int")],
                Reserved::Result => vec![name("int"), name("string")],
            };
            let annotation = apply(head, args);
            let mut draft = ImageDraft::new();
            let before = stable_snapshot(&registry);
            let draft_before = draft.encode().expect("empty draft encodes");
            assert_eq!(
                registry.resolve_garg(&mut draft, &annotation, site(2)),
                Err(ResolveError::Invariant(
                    GenericInvariant::ReservedTemplateMissing(reserved)
                ))
            );
            assert_eq!(stable_snapshot(&registry), before);
            let draft_after = draft.encode().expect("failed draft still encodes");
            assert_eq!(draft_after.bytes, draft_before.bytes);
            assert_eq!(draft_after.image_id, draft_before.image_id);
        }
    }

    #[test]
    fn resolve_garg_reports_exact_wrong_option_and_result_template_kinds() {
        for (reserved, head) in [(Reserved::Option, "Option"), (Reserved::Result, "Result")] {
            let mut templates = reserved_templates();
            let template = templates
                .iter()
                .position(|candidate| candidate.reserved == Some(reserved))
                .expect("reserved template exists");
            templates[template].body = TemplateBody::Struct(Vec::new());
            let registry = registry(templates);
            let args = match reserved {
                Reserved::Option => vec![name("int")],
                Reserved::Result => vec![name("int"), name("string")],
            };
            let annotation = apply(head, args);
            let expected = ResolveError::Invariant(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Enum,
                actual: TypeInstKind::Struct,
            });

            let mut draft = ImageDraft::new();
            let before = stable_snapshot(&registry);
            let draft_before = draft.encode().expect("empty draft encodes");
            assert_eq!(
                registry.resolve_garg(&mut draft, &annotation, site(2)),
                Err(expected)
            );
            assert_eq!(stable_snapshot(&registry), before);
            let draft_after = draft.encode().expect("failed draft still encodes");
            assert_eq!(draft_after.bytes, draft_before.bytes);
            assert_eq!(draft_after.image_id, draft_before.image_id);
        }
    }

    #[test]
    fn map_key_resolution_validates_metadata_before_semantic_refusal() {
        let annotation = apply("Map", vec![name("K"), name("int")]);
        for family in ["struct", "enum", "collection"] {
            let registry = registry(Vec::new());
            let mut draft = ImageDraft::new();
            let arg = match family {
                "struct" => {
                    let name = draft.intern_string("OrphanStruct");
                    GArg::Struct(draft.add_record_type(RecordTypeDef {
                        name,
                        fields: Vec::new(),
                    }))
                }
                "enum" => {
                    let name = draft.intern_string("OrphanEnum");
                    GArg::Enum(draft.add_enum_type(EnumTypeDef {
                        name,
                        variants: Vec::new(),
                    }))
                }
                "collection" => GArg::Collection(0),
                _ => unreachable!("the hostile family table is closed"),
            };
            let expected = GenericInvariant::TypeArgumentTargetMissing(arg);
            let subst = vec![("K".to_string(), arg)];
            let owner_before = stable_snapshot(&registry);
            let draft_before = draft_snapshot(&draft);

            let (resolved, builds) = count_metadata_directory_builds(|| {
                registry.resolve_garg_env(&mut draft, &annotation, &subst, site(2))
            });
            assert!(matches!(
                resolved,
                Err(ResolveError::Invariant(found)) if found == expected
            ));
            assert_eq!(builds, 1, "{family} key uses one metadata proof");
            assert_eq!(stable_snapshot(&registry), owner_before);
            assert_eq!(draft_snapshot(&draft), draft_before);

            let (direct, builds) = count_metadata_directory_builds(|| {
                registry.instantiate_map(&mut draft, arg, GArg::Scalar(ScalarType::Int))
            });
            assert!(matches!(
                direct,
                Err(ResolveError::Invariant(found)) if found == expected
            ));
            assert_eq!(
                builds, 1,
                "the direct {family} key path cannot bypass the owner"
            );
            assert_eq!(stable_snapshot(&registry), owner_before);
            assert_eq!(draft_snapshot(&draft), draft_before);
        }

        let mut declared_registry = registry(Vec::new());
        let mut declared_draft = ImageDraft::new();
        let declared = add_declared_struct(
            &mut declared_registry,
            &mut declared_draft,
            "Plain",
            Vec::new(),
        );
        let subst = vec![("K".to_string(), GArg::Struct(declared))];
        let owner_before = stable_snapshot(&declared_registry);
        let draft_before = draft_snapshot(&declared_draft);
        let (refused, builds) = count_metadata_directory_builds(|| {
            declared_registry.resolve_garg_env(&mut declared_draft, &annotation, &subst, site(3))
        });
        assert_eq!(
            refused,
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
        );
        assert_eq!(builds, 1, "a coherent non-key is refused after one proof");
        assert_eq!(stable_snapshot(&declared_registry), owner_before);
        assert_eq!(draft_snapshot(&declared_draft), draft_before);

        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        let valid = apply("Map", vec![name("int"), name("string")]);
        let (resolved, builds) =
            count_metadata_directory_builds(|| registry.resolve_garg(&mut draft, &valid, site(4)));
        assert_eq!(resolved, Ok(GArg::Collection(0)));
        assert_eq!(builds, 1, "an admitted Map retains one metadata proof");
    }

    #[test]
    fn missing_nominal_map_key_stops_before_resolving_a_fresh_value() {
        let make_registry = registry;
        let annotation = apply("Map", vec![name("K"), apply("List", vec![name("int")])]);
        let missing = GArg::Nominal(NominalId(0));
        let subst = vec![("K".to_string(), missing)];
        let registry = make_registry(Vec::new());
        let mut draft = ImageDraft::new();
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        let (resolved, builds) = count_metadata_directory_builds(|| {
            registry.resolve_garg_env(&mut draft, &annotation, &subst, site(5))
        });
        assert_eq!(
            resolved,
            Err(ResolveError::Invariant(
                GenericInvariant::TypeArgumentTargetMissing(missing)
            ))
        );
        assert_eq!(
            builds, 0,
            "a missing nominal is rejected by its direct owner"
        );
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);

        let (direct, builds) = count_metadata_directory_builds(|| {
            registry.instantiate_map(&mut draft, missing, GArg::Collection(0))
        });
        assert_eq!(
            direct,
            Err(ResolveError::Invariant(
                GenericInvariant::TypeArgumentTargetMissing(missing)
            ))
        );
        assert_eq!(builds, 0);
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);

        let mut declared = make_registry(Vec::new());
        declared.nominals.push(NominalInfo {
            name: "Key".to_string(),
            lo: 0,
            hi: 10,
            supports: SupportSet::default(),
        });
        let mut declared_draft = ImageDraft::new();
        assert_eq!(
            declared.resolve_garg_env(
                &mut declared_draft,
                &annotation,
                &[("K".to_string(), GArg::Nominal(NominalId(0)))],
                site(6),
            ),
            Ok(GArg::Collection(1)),
            "a declared nominal remains an admissible Map key"
        );
    }

    /// A struct template paired with an enum image id fails
    /// before interning field names or mutating either draft table.
    #[test]
    fn struct_body_with_enum_id_fails_before_draft_mutation() {
        let registry = registry(vec![template("Good", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let wrong_name = draft.intern_string("Wrong");
        let wrong_id = draft.add_enum_type(EnumTypeDef {
            name: wrong_name,
            variants: Vec::new(),
        });
        let registry_before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let result = registry.fill_type_body(
            &mut draft,
            0,
            TypeInstId::Enum(wrong_id),
            &[GArg::Scalar(ScalarType::Int)],
            site(2),
        );
        let draft_after = draft.encode().expect("failed draft still encodes");

        assert_eq!(
            take_generic_invariant(result),
            GenericInvariant::TypeBodyKindMismatch {
                id: TypeInstId::Enum(wrong_id),
                body: TypeInstKind::Struct,
            }
        );
        assert_eq!(stable_snapshot(&registry), registry_before);
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    /// An enum template paired with a record image id fails
    /// before interning member names or mutating either draft table.
    #[test]
    fn enum_body_with_record_id_fails_before_draft_mutation() {
        let registry = registry(vec![enum_template("Good", name("T"))]);
        let mut draft = ImageDraft::new();
        let wrong_name = draft.intern_string("Wrong");
        let wrong_id = draft.add_record_type(RecordTypeDef {
            name: wrong_name,
            fields: Vec::new(),
        });
        let registry_before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let result = registry.fill_type_body(
            &mut draft,
            0,
            TypeInstId::Record(wrong_id),
            &[GArg::Scalar(ScalarType::Int)],
            site(2),
        );
        let draft_after = draft.encode().expect("failed draft still encodes");

        assert_eq!(
            take_generic_invariant(result),
            GenericInvariant::TypeBodyKindMismatch {
                id: TypeInstId::Record(wrong_id),
                body: TypeInstKind::Enum,
            }
        );
        assert_eq!(stable_snapshot(&registry), registry_before);
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    /// A List cache/draft index mismatch fails at the resolving
    /// owner, before a usable collection index can escape.
    #[test]
    fn list_draft_ahead_misalignment_fails_without_exposing_an_index() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        draft.add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        });
        let before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let result = registry.instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int));
        let draft_after = draft.encode().expect("failed draft still encodes");

        assert_eq!(
            take_generic_invariant(result),
            GenericInvariant::CollectionIndexMismatch {
                kind: CollectionKind::List,
                cache_index: 0,
                draft_index: 1,
            }
        );
        assert_eq!(stable_snapshot(&registry), before);
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    /// The Map owner has the same no-index-on-misalignment boundary.
    #[test]
    fn map_cache_ahead_misalignment_fails_without_exposing_an_index() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        registry.collections.borrow_mut().push(CollSpec::Map {
            key: GArg::Scalar(ScalarType::Int),
            value: GArg::Scalar(ScalarType::Text),
        });
        let before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let result = registry.instantiate_map(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Text),
        );
        let draft_after = draft.encode().expect("failed draft still encodes");

        assert_eq!(
            take_generic_invariant(result),
            GenericInvariant::CollectionIndexMismatch {
                kind: CollectionKind::Map,
                cache_index: 1,
                draft_index: 0,
            }
        );
        assert_eq!(stable_snapshot(&registry), before);
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    /// Owner alignment is checked even on a cache hit, before the prior index can
    /// escape or reach collection_spec.
    #[test]
    fn collection_drift_blocks_a_cache_hit_without_exposing_the_prior_index() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        let list = registry
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("aligned owners mint the first List");
        assert_eq!(list, 0);
        draft.add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Text),
            value: ImageType::scalar(Scalar::Bool),
        });
        let before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("drifted draft encodes");

        let result = registry.instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int));
        let draft_after = draft.encode().expect("failed draft still encodes");

        assert_eq!(
            take_generic_invariant(result),
            GenericInvariant::CollectionIndexMismatch {
                kind: CollectionKind::List,
                cache_index: 1,
                draft_index: 2,
            }
        );
        assert_eq!(stable_snapshot(&registry), before);
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn published_collection_metadata_is_revalidated_without_a_watermark() {
        let registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let list = registry
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("aligned owners publish List<int>");
        let orphan_name = draft.intern_string("Orphan");
        let orphan = draft.add_record_type(RecordTypeDef {
            name: orphan_name,
            fields: Vec::new(),
        });
        registry.collections.borrow_mut()[list as usize] = CollSpec::List {
            elem: GArg::Struct(orphan),
        };
        let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(orphan));
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert_eq!(
            registry.validate_type_arguments(&[GArg::Collection(list)]),
            Err(expected)
        );
        assert_eq!(
            registry.mint_type_instance(&mut draft, 0, &[GArg::Collection(list)], site(2)),
            Err(ResolveError::Invariant(expected))
        );
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn aligned_collection_wrappers_publish_consecutive_indices() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        let list = registry
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("aligned List owners mint");
        let map = registry
            .instantiate_map(
                &mut draft,
                GArg::Scalar(ScalarType::Text),
                GArg::Scalar(ScalarType::Bool),
            )
            .expect("aligned Map owners mint");

        assert_eq!((list, map), (0, 1));
        assert_eq!(
            stable_snapshot(&registry).collections,
            vec![
                CollSpec::List {
                    elem: GArg::Scalar(ScalarType::Int),
                },
                CollSpec::Map {
                    key: GArg::Scalar(ScalarType::Text),
                    value: GArg::Scalar(ScalarType::Bool),
                },
            ]
        );
    }

    fn take_generic_invariant<T>(result: Result<T, ResolveError>) -> GenericInvariant {
        match result {
            Err(ResolveError::Invariant(invariant)) => invariant,
            Err(ResolveError::Refusal(_)) => {
                panic!("compiler bookkeeping must not become a semantic refusal")
            }
            Ok(_) => panic!("the incoherent generic state must fail closed"),
        }
    }

    #[test]
    fn record_id_with_enum_body_fails_every_ready_boundary_exactly() {
        let registry = registry(vec![template("Box", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let id = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
            .expect("Box row mints ready");
        let TypeInstId::Record(record_id) = id else {
            panic!("Box is record-shaped")
        };
        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Ready(InstBody::Enum(Vec::new()));
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id,
            body: TypeInstKind::Enum,
        };
        let before = stable_snapshot(&registry);

        assert_eq!(
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(3),),
            Err(ResolveError::Invariant(expected)),
            "a cache hit validates Ready before reusing its ID"
        );
        assert_eq!(
            registry.settled_type_result(0, id, AnyReadyInstance),
            Err(ResolveError::Invariant(expected))
        );
        assert_eq!(registry.instantiation_of(id), Err(expected));
        assert!(matches!(registry.type_inst_body(id), Err(found) if found == expected));
        assert_eq!(registry.inst_anchor_spelling(id), Err(expected));
        assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Struct(record_id)]),
            Err(expected)
        );
        assert_eq!(stable_snapshot(&registry), before);
    }

    #[test]
    fn enum_id_with_struct_body_fails_every_ready_boundary_exactly() {
        let registry = registry(reserved_templates());
        let mut draft = ImageDraft::new();
        let enum_id = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
            .expect("Option row mints ready");
        let id = TypeInstId::Enum(enum_id);
        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Ready(InstBody::Struct(Vec::new()));
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id,
            body: TypeInstKind::Struct,
        };
        let before = stable_snapshot(&registry);

        assert_eq!(
            registry.instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                site(3),
            ),
            Err(ResolveError::Invariant(expected))
        );
        assert_eq!(
            registry.settled_type_result(0, id, AnyReadyInstance),
            Err(ResolveError::Invariant(expected))
        );
        assert_eq!(registry.instantiation_of(id), Err(expected));
        assert!(matches!(registry.type_inst_body(id), Err(found) if found == expected));
        assert_eq!(registry.as_option(enum_id), Err(expected));
        assert_eq!(registry.as_result(enum_id), Err(expected));
        assert_eq!(registry.enum_variants(enum_id), Err(expected));
        assert_eq!(registry.enum_anchor_spelling(enum_id), Err(expected));
        assert_eq!(registry.inst_anchor_spelling(id), Err(expected));
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Enum(enum_id)),
            Err(expected)
        );
        assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Enum(enum_id)]),
            Err(expected)
        );
        assert_eq!(stable_snapshot(&registry), before);
    }

    #[test]
    fn ready_body_proof_is_exact_selective_and_allows_ready_back_edges() {
        let make_registry = registry;
        let registry = make_registry(vec![
            template(
                "Outer",
                vec![
                    ("safe", name("T")),
                    ("child", apply("Inner", vec![name("T")])),
                ],
            ),
            template("Inner", vec![("value", name("T"))]),
        ]);
        let mut draft = ImageDraft::new();
        let outer = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
            .expect("Outer<int> and its later Inner<int> row mint Ready");
        let TypeInstId::Record(outer_id) = outer else {
            panic!("Outer is record-shaped")
        };
        let (outer_row, inner_row, inner) = {
            let generics = registry.generics.borrow();
            let outer_row = generics
                .type_insts
                .iter()
                .position(|inst| inst.id == outer)
                .expect("Outer row exists");
            let inner_row = generics
                .type_insts
                .iter()
                .position(|inst| registry.type_templates[inst.template].name == "Inner")
                .expect("Inner row exists");
            (outer_row, inner_row, generics.type_insts[inner_row].id)
        };
        assert!(
            inner_row > outer_row,
            "body targets may be minted after their owner"
        );
        assert!(registry.type_inst_body(outer).unwrap().is_some());
        assert_eq!(
            registry.struct_field_projection(outer_id, "safe"),
            Ok(StructFieldProjection::Field {
                index: 0,
                ty: GArg::Scalar(ScalarType::Int),
            })
        );

        let inner_ready = {
            let mut generics = registry.generics.borrow_mut();
            std::mem::replace(
                &mut generics.type_insts[inner_row].state,
                TypeInstState::Rejected(ResolveRefusal::Unsupported),
            )
        };
        let expected = GenericInvariant::ReadyBodyMissing(inner);
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);
        assert_eq!(
            registry.struct_field_projection(outer_id, "safe"),
            Ok(StructFieldProjection::Field {
                index: 0,
                ty: GArg::Scalar(ScalarType::Int),
            }),
            "an unrelated unavailable field target does not block a selected safe field"
        );
        assert_eq!(
            registry.struct_field_projection(outer_id, "child"),
            Err(expected)
        );
        assert!(matches!(
            registry.type_inst_body(outer),
            Err(found) if found == expected
        ));
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert!(matches!(
            ValueGraph::build(&registry),
            Err(found) if found == expected
        ));
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
        registry.generics.borrow_mut().type_insts[inner_row].state = inner_ready;

        let original_outer = registry.generics.borrow().type_insts[outer_row]
            .state
            .clone();
        let TypeInstState::Ready(InstBody::Struct(original_fields)) = &original_outer else {
            panic!("Outer row is Ready and struct-shaped")
        };
        let mut malformed = Vec::new();
        malformed.push(InstBody::Struct(Vec::new()));
        let mut renamed = original_fields.clone();
        renamed[0].0 = "renamed".to_string();
        malformed.push(InstBody::Struct(renamed));
        let mut wrong_type = original_fields.clone();
        wrong_type[0].1 = GArg::Scalar(ScalarType::Bool);
        malformed.push(InstBody::Struct(wrong_type));

        for body in malformed {
            registry.generics.borrow_mut().type_insts[outer_row].state = TypeInstState::Ready(body);
            let expected = GenericInvariant::ReadyBodyShapeMismatch(outer);
            let owner_before = stable_snapshot(&registry);
            let draft_before = draft_snapshot(&draft);
            assert!(matches!(
                registry.type_inst_body(outer),
                Err(found) if found == expected
            ));
            assert_eq!(
                registry.struct_field_projection(outer_id, "safe"),
                Err(expected)
            );
            assert_eq!(stable_snapshot(&registry), owner_before);
            assert_eq!(draft_snapshot(&draft), draft_before);
        }
        registry.generics.borrow_mut().type_insts[outer_row].state = original_outer;

        for templates in [
            vec![template(
                "Node",
                vec![("next", apply("Node", vec![name("T")]))],
            )],
            vec![
                template("Left", vec![("right", apply("Right", vec![name("T")]))]),
                template("Right", vec![("left", apply("Left", vec![name("T")]))]),
            ],
        ] {
            let recursive = make_registry(templates);
            let mut recursive_draft = ImageDraft::new();
            let root = recursive
                .mint_type_instance(
                    &mut recursive_draft,
                    0,
                    &[GArg::Scalar(ScalarType::Int)],
                    site(9),
                )
                .expect("Ready body back edges are admitted after settlement");
            assert!(recursive.type_inst_body(root).unwrap().is_some());
            assert!(recursive.clone_for_generic_check().is_ok());
        }
    }

    #[test]
    fn ready_body_nested_template_contract_fails_every_selected_boundary_exactly() {
        #[derive(Clone, Copy)]
        enum Fault {
            TemplateKind,
            ArgumentCount,
        }

        for fault in [Fault::TemplateKind, Fault::ArgumentCount] {
            let mut registry = registry(vec![
                template(
                    "Outer",
                    vec![
                        ("safe", name("T")),
                        ("child", apply("Inner", vec![name("T")])),
                    ],
                ),
                template("Inner", vec![("value", name("T"))]),
            ]);
            let mut draft = ImageDraft::new();
            let outer = registry
                .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(20))
                .expect("Outer<int> and Inner<int> mint Ready");
            let TypeInstId::Record(outer_id) = outer else {
                panic!("Outer is record-shaped")
            };
            let inner_row = registry
                .generics
                .borrow()
                .type_insts
                .iter()
                .position(|inst| registry.type_templates[inst.template].name == "Inner")
                .expect("Inner row exists");
            let inner_template = registry.generics.borrow().type_insts[inner_row].template;
            let expected = match fault {
                Fault::TemplateKind => {
                    registry.type_templates[inner_template].body = TemplateBody::Enum(Vec::new());
                    GenericInvariant::TemplateKindMismatch {
                        template: inner_template,
                        expected: TypeInstKind::Enum,
                        actual: TypeInstKind::Struct,
                    }
                }
                Fault::ArgumentCount => {
                    registry.generics.borrow_mut().type_insts[inner_row]
                        .args
                        .clear();
                    GenericInvariant::TypeArgumentCountMismatch {
                        template: inner_template,
                        expected: 1,
                        actual: 0,
                    }
                }
            };
            let owner_before = stable_snapshot(&registry);
            let draft_before = draft_snapshot(&draft);

            let (selected, builds) = count_metadata_directory_builds(|| {
                registry.struct_field_projection(outer_id, "safe")
            });
            assert_eq!(selected, Err(expected));
            assert_eq!(builds, 1);

            let (body, builds) = count_metadata_directory_builds(|| registry.type_inst_body(outer));
            assert!(matches!(body, Err(found) if found == expected));
            assert_eq!(builds, 1);

            let (replayed, builds) = count_metadata_directory_builds(|| {
                registry.mint_type_instance(
                    &mut draft,
                    0,
                    &[GArg::Scalar(ScalarType::Int)],
                    site(21),
                )
            });
            assert_eq!(replayed, Err(ResolveError::Invariant(expected)));
            assert_eq!(builds, 1);

            let (cloned, builds) =
                count_metadata_directory_builds(|| registry.clone_for_generic_check());
            assert!(matches!(cloned, Err(found) if found == expected));
            assert_eq!(builds, 1);

            let (graph, builds) = count_metadata_directory_builds(|| ValueGraph::build(&registry));
            assert!(matches!(graph, Err(found) if found == expected));
            assert_eq!(builds, 1);
            assert_eq!(stable_snapshot(&registry), owner_before);
            assert_eq!(draft_snapshot(&draft), draft_before);
        }
    }

    #[test]
    fn ready_body_matcher_visits_deep_borrowed_template_once_per_node() {
        let mut registry = registry(vec![template("Deep", vec![("value", name("T"))])]);
        let mut draft = ImageDraft::new();
        let id = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(22))
            .expect("the shallow seed row mints Ready");
        let depth = MAX_INSTANTIATIONS;
        let mut expected = name("T");
        let mut actual = GArg::Scalar(ScalarType::Int);
        {
            let mut collections = registry.collections.borrow_mut();
            collections.reserve(depth);
            for index in 0..depth {
                collections.push(CollSpec::List { elem: actual });
                actual = GArg::Collection(index as u16);
                expected = apply("List", vec![expected]);
            }
        }
        registry.type_templates[0].body =
            TemplateBody::Struct(vec![("value".to_string(), expected)]);
        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Ready(InstBody::Struct(vec![("value".to_string(), actual)]));
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        let ((body, visits), builds) = count_metadata_directory_builds(|| {
            count_ready_body_match_visits(|| registry.type_inst_body(id).map(|body| body.is_some()))
        });
        assert_eq!(body, Ok(true));
        assert_eq!(builds, 1);
        assert_eq!(
            visits,
            depth + 1,
            "each List node and the terminal parameter is visited exactly once",
        );
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);

        // Remove the hostile deep template iteratively so the test also avoids a
        // recursive destructor after proving the production matcher is iterative.
        let body = std::mem::replace(
            &mut registry.type_templates[0].body,
            TemplateBody::Struct(vec![("value".to_string(), name("T"))]),
        );
        let TemplateBody::Struct(mut fields) = body else {
            panic!("Deep remains struct-shaped")
        };
        let (_, mut current) = fields.pop().expect("Deep has one field");
        loop {
            match current {
                TypeExpr::Apply { mut args, .. } => {
                    current = args.pop().expect("each List has one argument");
                }
                TypeExpr::Name { .. } => break,
                TypeExpr::Optional { .. } | TypeExpr::Identity(_) => {
                    panic!("the deep matcher fixture contains only List and T")
                }
            }
        }
    }

    #[test]
    fn ready_body_matcher_preserves_alias_precedence_over_template_parameters() {
        let mut alias = BTreeMap::new();
        alias.insert("Alias".to_string(), name("int"));
        let mut alias_template = template("AliasBox", vec![("value", name("Alias"))]);
        alias_template.type_params = vec![("Alias".to_string(), None)];
        let mut registry = registry(vec![alias_template]);
        registry.aliases = alias;
        let mut draft = ImageDraft::new();
        let id = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Text)], site(23))
            .expect("alias expansion wins before template substitution while minting");
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        let ((body, visits), builds) = count_metadata_directory_builds(|| {
            count_ready_body_match_visits(|| registry.type_inst_body(id))
        });
        assert!(matches!(
            body,
            Ok(Some(InstBody::Struct(ref fields)))
                if fields
                    == &vec![("value".to_string(), GArg::Scalar(ScalarType::Int))]
        ));
        assert_eq!(
            visits, 2,
            "the alias name and expanded scalar are visited once"
        );
        assert_eq!(builds, 1);
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn ready_enum_payload_targets_are_checked_before_shape_or_durable_projection() {
        let registry = registry(vec![
            enum_template("Outer", apply("Inner", vec![name("T")])),
            template("Inner", vec![("value", name("T"))]),
        ]);
        let mut draft = ImageDraft::new();
        let outer = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(12))
            .expect("Outer<int> and its payload target mint Ready");
        let TypeInstId::Enum(outer_id) = outer else {
            panic!("Outer is enum-shaped")
        };
        let (inner_row, inner) = {
            let generics = registry.generics.borrow();
            let row = generics
                .type_insts
                .iter()
                .position(|inst| registry.type_templates[inst.template].name == "Inner")
                .expect("Inner row exists");
            (row, generics.type_insts[row].id)
        };
        registry.generics.borrow_mut().type_insts[inner_row].state =
            TypeInstState::Rejected(ResolveRefusal::Unsupported);
        let expected = GenericInvariant::ReadyBodyMissing(inner);
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert_eq!(registry.enum_variants(outer_id), Err(expected));
        assert!(matches!(
            registry.with_metadata_session(|session| {
                session.durable_enum_shape_and_anchor(outer_id)
            }),
            Err(found) if found == expected
        ));
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert!(matches!(
            ValueGraph::build(&registry),
            Err(found) if found == expected
        ));
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn ready_template_id_mismatch_is_typed_after_body_id_validation() {
        let mut registry = registry(reserved_templates());
        let mut draft = ImageDraft::new();
        let enum_id = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
            .expect("Option row mints ready");
        registry.type_templates[0].body = TemplateBody::Struct(Vec::new());
        let expected = GenericInvariant::TemplateKindMismatch {
            template: 0,
            expected: TypeInstKind::Struct,
            actual: TypeInstKind::Enum,
        };
        let before = stable_snapshot(&registry);

        assert_eq!(registry.as_option(enum_id), Err(expected));
        assert!(matches!(
            registry.clone_for_generic_check(),
            Err(found) if found == expected
        ));
        assert_eq!(stable_snapshot(&registry), before);
    }

    #[test]
    fn commit_ready_state_hostile_branches_are_exact_and_read_only() {
        let stable = active_registry();
        {
            let mut generics = stable.generics.borrow_mut();
            let TypeInstState::Filling { staged } = &mut generics.type_insts[0].state else {
                panic!("active helper produces a Filling row")
            };
            let body = staged.take().expect("active helper stages a body");
            generics.type_insts[0].state = TypeInstState::Ready(body);
        }
        let before = stable_snapshot(&stable);
        let result = {
            let mut generics = stable.generics.borrow_mut();
            stable.commit_ready_state(&mut generics.type_insts[0])
        };
        assert_eq!(
            result,
            Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::StableRowInActiveBatch,
            )))
        );
        assert_eq!(stable_snapshot(&stable), before);

        let missing = active_registry();
        missing.generics.borrow_mut().type_insts[0].state = TypeInstState::Filling { staged: None };
        let before = stable_snapshot(&missing);
        let result = {
            let mut generics = missing.generics.borrow_mut();
            missing.commit_ready_state(&mut generics.type_insts[0])
        };
        assert_eq!(
            result,
            Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant::IncompleteRowWithoutRefusal,
            )))
        );
        assert_eq!(stable_snapshot(&missing), before);
    }

    #[test]
    fn durable_anchor_reports_every_missing_target_without_fallback_tokens() {
        let registry = registry(Vec::new());
        let mut draft = ImageDraft::new();
        let record_name = draft.intern_string("OrphanRecord");
        let record = draft.add_record_type(RecordTypeDef {
            name: record_name,
            fields: Vec::new(),
        });
        let enum_name = draft.intern_string("OrphanEnum");
        let enum_id = draft.add_enum_type(EnumTypeDef {
            name: enum_name,
            variants: Vec::new(),
        });
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        for (arg, expected) in [
            (
                GArg::Nominal(NominalId(0)),
                GenericInvariant::TypeArgumentTargetMissing(GArg::Nominal(NominalId(0))),
            ),
            (
                GArg::Struct(record),
                GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(record)),
            ),
            (
                GArg::Group(record),
                GenericInvariant::TypeArgumentTargetMissing(GArg::Group(record)),
            ),
            (
                GArg::Enum(enum_id),
                GenericInvariant::TypeArgumentTargetMissing(GArg::Enum(enum_id)),
            ),
            (
                GArg::Collection(0),
                GenericInvariant::TypeArgumentTargetMissing(GArg::Collection(0)),
            ),
            (GArg::Param(7), GenericInvariant::TypeArgumentParameter(7)),
        ] {
            assert_eq!(garg_anchor_spelling(&registry, arg), Err(expected));
            assert_eq!(stable_snapshot(&registry), owner_before);
            assert_eq!(draft_snapshot(&draft), draft_before);
        }
    }

    #[test]
    fn durable_metadata_expands_a_shared_value_at_its_shortest_depth() {
        let mut registry = registry(vec![enum_template("Bad", name("T"))]);
        let mut draft = ImageDraft::new();
        let bad = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
            .expect("Bad<int> mints ready");
        let TypeInstId::Enum(bad) = bad else {
            panic!("Bad is enum-shaped")
        };
        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Ready(InstBody::Struct(Vec::new()));

        let shared = add_declared_struct(
            &mut registry,
            &mut draft,
            "Shared",
            vec![("bad", GArg::Enum(bad))],
        );
        let mut deep = shared;
        for index in 0..31 {
            deep = add_declared_struct(
                &mut registry,
                &mut draft,
                &format!("Wrap{index}"),
                vec![("value", GArg::Struct(deep))],
            );
        }
        let root = add_declared_struct(
            &mut registry,
            &mut draft,
            "Root",
            vec![
                ("deep", GArg::Struct(deep)),
                ("direct", GArg::Struct(shared)),
            ],
        );
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(bad),
            body: TypeInstKind::Struct,
        };
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Struct(root)]),
            Err(expected)
        );
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    #[test]
    fn durable_prevalidation_reaches_nested_and_phantom_generic_arguments() {
        for fields in [vec![("value", name("T"))], Vec::new()] {
            let mut templates = reserved_templates();
            templates.push(template("Outer", fields));
            let registry = registry(templates);
            let mut draft = ImageDraft::new();
            let inner = registry
                .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
                .expect("inner Option mints ready");
            let outer = registry
                .mint_type_instance(&mut draft, 2, &[GArg::Enum(inner)], site(3))
                .expect("outer generic mints ready");
            let TypeInstId::Record(outer) = outer else {
                panic!("Outer is record-shaped")
            };
            registry.generics.borrow_mut().type_insts[0].state =
                TypeInstState::Ready(InstBody::Struct(Vec::new()));
            let expected = GenericInvariant::TypeBodyKindMismatch {
                id: TypeInstId::Enum(inner),
                body: TypeInstKind::Struct,
            };

            assert_eq!(
                registry.validate_durable_value_metadata([GArg::Struct(outer)]),
                Err(expected),
                "phantom arguments are prevalidated as well as body-reachable ones"
            );
            assert_eq!(
                garg_anchor_spelling(&registry, GArg::Struct(outer)),
                Err(expected),
                "durable anchor recursion cannot launder the nested Ready invariant"
            );
        }
    }

    #[test]
    fn durable_build_rejects_nested_and_phantom_ready_corruption_before_effects() {
        for source in [
            r#"struct Outer<T> {
    value: T
}

resource Holder {
    required value: Outer<Option<int>>
}

store ^holders[id: int]: Holder
"#,
            r#"struct Outer<T> {}

resource Holder {
    required value: Outer<Option<int>>
}

store ^holders[id: int]: Holder
"#,
        ] {
            let parsed = parse_source(source);
            assert!(parsed.diagnostics.is_empty());
            let generic_struct = parsed
                .file
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Struct(declaration) => Some(declaration),
                    _ => None,
                })
                .expect("generic struct parses");
            let resource = parsed
                .file
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Resource(declaration) => Some(declaration),
                    _ => None,
                })
                .expect("resource parses");
            let store = parsed
                .file
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Store(declaration) => Some(declaration),
                    _ => None,
                })
                .expect("store parses");
            let structs = vec![("src/main.mw".to_string(), generic_struct)];
            let resources = vec![("src/main.mw".to_string(), resource)];
            let stores = vec![("src/main.mw".to_string(), store)];
            let mut draft = ImageDraft::new();
            let mut diagnostics = Vec::new();
            let registry = TypeRegistry::build(
                &mut draft,
                &[],
                &[],
                &structs,
                &[],
                &resources,
                &mut diagnostics,
            );
            assert!(diagnostics.is_empty());
            let (option_index, option_id) = {
                let generics = registry.generics.borrow();
                generics
                    .type_insts
                    .iter()
                    .enumerate()
                    .find_map(|(index, inst)| {
                        (registry.type_templates[inst.template].reserved == Some(Reserved::Option))
                            .then_some((index, inst.id))
                    })
                    .expect("resource field mints Option")
            };
            let TypeInstId::Enum(option_id) = option_id else {
                panic!("Option is enum-shaped")
            };
            registry.generics.borrow_mut().type_insts[option_index].state =
                TypeInstState::Ready(InstBody::Struct(Vec::new()));
            let expected = GenericInvariant::TypeBodyKindMismatch {
                id: TypeInstId::Enum(option_id),
                body: TypeInstKind::Struct,
            };
            let before = draft.encode().expect("seeded draft encodes");

            assert!(matches!(
                crate::durable::DurableRegistry::build(
                    &mut draft,
                    &registry,
                    &resources,
                    &stores,
                    None,
                    &mut diagnostics,
                ),
                Err(found) if found == expected
            ));
            assert!(diagnostics.is_empty());
            let after = draft.encode().expect("rejected draft still encodes");
            assert_eq!(after.bytes, before.bytes);
            assert_eq!(after.image_id, before.image_id);
        }
    }

    #[test]
    fn value_cycle_invariant_precedes_and_preserves_source_diagnostics() {
        let registry = registry(reserved_templates());
        let mut draft = ImageDraft::new();
        let enum_id = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
            .expect("Option row mints ready");
        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Ready(InstBody::Struct(Vec::new()));
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(enum_id),
            body: TypeInstKind::Struct,
        };
        let mut diagnostics = vec![SourceDiagnostic::at(
            Code::CheckType.as_str(),
            "src/main.mw",
            SourceSpan::default(),
            "earlier source failure".to_string(),
        )];
        let before = diagnostics.clone();

        assert_eq!(
            reject_value_cycles(&registry, &[], &[], &mut diagnostics),
            Err(expected)
        );
        assert_eq!(diagnostics, before);
    }
}
