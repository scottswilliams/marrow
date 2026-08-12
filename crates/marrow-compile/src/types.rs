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
    TemplateProofDraftGuard, TypeId, VariantDef,
};
use marrow_project::FileIdentity;
use marrow_syntax::{
    AliasDecl, EnumDecl, EnumMember, Expression, FieldDecl, GroupDecl, LiteralKind, NominalDecl,
    ResourceDecl, ResourceMember, SourceSpan, StructDecl, TypeExpr, UnaryOp, range_expr,
};

use crate::analysis::FileRef;
use crate::decl::{
    Binding, DeclarationBudget, DeclarationIndexDrift, DeclarationLedger, DeclarationLedgerFull,
    DeclarationNamespace, DeclarationOccurrence, DeclarationRefusalId, DeclarationRefusalSummary,
    DeclarationSite, DeclareError, declaration_refused, refuse, refuse_covered, refuse_first,
    refuse_row,
};
use crate::diag::{BoundedDiagnostics, DiagnosticCollector, SourceDiagnostic};
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

/// Why resolution of a value type could not produce a usable type.
///
/// `Limit` is diagnosed once by the shared monomorphization owner; `Unsupported`
/// is contextualized by each declaration or lowering consumer at its current site;
/// `RefusedDeclaration` names a declaration this project wrote and the compiler
/// refused, so the use is steered to that cause instead of being told the name was
/// never declared. The handle keeps the variant `Copy`, so a rejected
/// instantiation caches a cause without taking owned bytes into the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveRefusal {
    Limit,
    /// Genuinely outside the admitted subset — the only refusal a "not yet
    /// supported on the beta line" report may describe.
    Unsupported,
    RefusedDeclaration(DeclarationRefusalId),
}

impl ResolveRefusal {
    /// Combine refusals for one provisional row, or for sub-parts of one
    /// annotation.
    ///
    /// A terminal shared limit dominates everything regardless of discovery or edge
    /// order. A genuine absence dominates a refused declaration: a real gap must
    /// never be hidden behind a refused sibling's steer, which would report the
    /// project's own refusal in place of the name that is actually missing. Two
    /// refused declarations survive as one cause only when they are the same
    /// declaration; otherwise the merge would have to pick a winner, and picking
    /// either would steer the reader to a cause the other part does not have.
    ///
    /// The collapse loses a steer, never a cause — every refused declaration is
    /// reported at its own declaration site — and it is bounded to sub-parts of a
    /// single annotation, because argument and parameter lists reject per element
    /// at each element's own span rather than folding across them.
    ///
    /// **Known limit.** A generic *argument list* still folds through one join, so
    /// `Pair<Bad, AlsoMissing>` reports the first argument and says nothing about
    /// the second: the reader fixes one, recompiles, and meets the other. The
    /// collapse loses a steer, never a cause — every refused declaration was already
    /// reported at its own declaration — and it is strictly narrower than the
    /// whole-annotation fold it replaced. Splitting the report per argument is a
    /// separate change to the diagnostic surface.
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Limit, _) | (_, Self::Limit) => Self::Limit,
            (Self::RefusedDeclaration(one), Self::RefusedDeclaration(two)) if one == two => {
                Self::RefusedDeclaration(one)
            }
            (Self::RefusedDeclaration(_), Self::RefusedDeclaration(_))
            | (Self::Unsupported, _)
            | (_, Self::Unsupported) => Self::Unsupported,
        }
    }
}

/// Why a declaration pass could not complete at all — as distinct from a single
/// declaration whose own refusal the pass records and carries on past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildError {
    /// A compiler-owned coherence failure, which aborts at the invariant boundary
    /// rather than reporting against the source.
    Invariant(GenericInvariant),
    /// The declaration ledgers' shared retention ceiling is spent.
    LedgerFull(DeclarationLedgerFull),
}

impl From<GenericInvariant> for BuildError {
    fn from(invariant: GenericInvariant) -> Self {
        Self::Invariant(invariant)
    }
}

impl From<DeclarationLedgerFull> for BuildError {
    fn from(full: DeclarationLedgerFull) -> Self {
        Self::LedgerFull(full)
    }
}

impl From<DeclarationIndexDrift> for BuildError {
    fn from(drift: DeclarationIndexDrift) -> Self {
        Self::Invariant(drift.into())
    }
}

/// A ledger's two ways of refusing to record an occurrence, routed to the same two
/// arms every other build failure takes.
impl From<DeclareError> for BuildError {
    fn from(error: DeclareError) -> Self {
        match error {
            DeclareError::LedgerFull(full) => full.into(),
            DeclareError::IndexDrift(drift) => drift.into(),
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

impl From<DeclarationIndexDrift> for ResolveError {
    fn from(drift: DeclarationIndexDrift) -> Self {
        Self::Invariant(drift.into())
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
    /// A mint or reserve appended a `(template, args)` key that `type_index` or
    /// `fn_index` already held. It reaches the append only on a dedup miss, so a
    /// displaced key means the dedup probe and the index disagree — the same divergence
    /// `MintIndexDrift` names, observed at the write. Rejected so a duplicate
    /// instantiation row can never be admitted.
    MintKeyAlreadyPresent,
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
    /// A `store` names a resource the type registry admitted, but the resource
    /// declaration it was built from is not in the declaration set the durable build
    /// walks. The two owners disagree about one name; it is not a fact about the
    /// source, so it is neither reported against the declaration nor allowed to drop
    /// the root silently.
    DurableResourceMissing(marrow_image::TypeId),
    /// The image draft refused a Product declaration, a root occurrence, a site binding,
    /// a site request, or a function body's site operands. Every one of those is a
    /// producer-side invariant: the compiler names a place the draft itself published, so
    /// a refusal means the compiler and the image owner disagree about the graph the
    /// compiler just built. It is named for the fault, not for the image owner's error
    /// type: a malformed Product command vector and a root occurrence over an undeclared
    /// Product are refusals of durable construction, not states of a site plan. The image
    /// owner's error is opaque by construction, so no cause is carried and none is
    /// rendered.
    DurableConstructionRefused,
    /// A declaration ledger's lookup index and its occurrence list disagree: a
    /// refusal handle addresses a position that holds no refusal, or one namespace's
    /// handle was presented to another's ledger. The two layers name one declaration
    /// and must agree about it; a wrong summary would steer a reader to a cause that
    /// is not the one their code hit.
    DeclarationIndexDrift,
}

impl From<DeclarationIndexDrift> for GenericInvariant {
    fn from(_: DeclarationIndexDrift) -> Self {
        Self::DeclarationIndexDrift
    }
}

impl From<marrow_image::SitePlanStateError> for GenericInvariant {
    fn from(_: marrow_image::SitePlanStateError) -> Self {
        Self::DurableConstructionRefused
    }
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

/// The reserved generic type names the user cannot redeclare, in a stable order. The
/// toolchain owns `Option`/`Result` (as generic enums) and `List`/`Map` (as compiler
/// collections). This is the single source both the redeclaration gate
/// ([`is_reserved_type_name`]) and the editor type-completion namespace derive from, so
/// the two cannot drift.
pub(crate) const RESERVED_GENERIC_TYPE_NAMES: [&str; 4] = ["Option", "Result", "List", "Map"];

/// Whether `name` is a reserved generic type name the user cannot redeclare.
pub(crate) fn is_reserved_type_name(name: &str) -> bool {
    RESERVED_GENERIC_TYPE_NAMES.contains(&name)
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
    /// The record owns no member of this name and never declared one.
    MissingRecordField,
    /// The group owns no leaf of this name and never declared one.
    MissingGroupField,
    /// The owner declared this member and the compiler refused the declaration, so
    /// the member is not projectable and the use is steered to that cause. A
    /// separate variant from the missing ones because reporting a refused member as
    /// absent is a false statement about the source.
    RefusedMember(DeclarationRefusalId),
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
    /// The captured file this template was declared in, or `None` for a reserved
    /// toolchain generic (`Option`, `Result`) that has no source file. A template
    /// with a source file always carries a real identity; the absence is
    /// structural, so no diagnostic can ever name an empty or sentinel file.
    file: Option<FileIdentity>,
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
/// The source anchor for a generic instantiation: the file and span a mint-time
/// diagnostic (an instantiation limit or a rejected payload) points at. Always a
/// real captured file — a mint is triggered by a use site, never by a fileless
/// synthetic construct.
#[derive(Clone, Copy)]
pub(crate) struct MintSite<'a> {
    pub(crate) file: &'a FileIdentity,
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
/// carries an abstract parameter into a published image; only an isolated template-proof
/// pass (entered through `enter_template_proof`) may use `Param` while checking one generic
/// template body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ArgumentDomain {
    #[default]
    Concrete,
    TemplateProof,
}

/// State captured before an isolated generic-template proof pass runs directly on the
/// registry, consumed by [`TypeRegistry::exit_template_proof`] to erase every effect the
/// pass leaves. It records the append-only lengths of the instantiation owners (whose
/// suffixes the proof appends and truncation drops), the swapped argument domain and
/// ordered-diagnostic buffer, and the draft record/enum id ceilings the reused metadata
/// directory rolls back to. The settled prefix is immutable across a fill batch, so these
/// lengths and swapped owners are a complete description of what the pass can change.
#[must_use = "a template-proof savepoint must be restored through exit_template_proof"]
pub(crate) struct RegistryProofSavepoint {
    type_insts: usize,
    collections: usize,
    fn_insts: usize,
    fn_queue: usize,
    prior_argument_domain: ArgumentDomain,
    /// The live pre-proof diagnostic owner, swapped whole out of the registry
    /// at proof entry and re-seated whole at exit.
    prior_payloads: DiagnosticCollector,
    entry_records: usize,
    entry_enums: usize,
}

/// A live isolated generic-template proof pass over the real registry and draft. Entering it
/// admits the pass and records the savepoint; the guard restores both owners on **every**
/// exit — the caller's normal return, an early lowering invariant, or an unwind — so a proof
/// that fails or panics part-way leaks nothing. The proof body borrows the real draft through
/// [`Self::draft`]; the registry is restored through its interior mutability. Only the
/// diagnostics the caller takes before the guard drops cross back.
pub(crate) struct TemplateProofScope<'r, 'd> {
    registry: &'r TypeRegistry,
    savepoint: Option<RegistryProofSavepoint>,
    /// The draft's own rollback. It is a field rather than a value the scope applies,
    /// because the guard *is* the borrow: the draft is unreachable except through it, and
    /// dropping this field is the whole draft restoration. The scope's own `Drop` body runs
    /// before any field drops, so the registry inverse is always restored first.
    draft: TemplateProofDraftGuard<'d>,
}

impl<'r, 'd> TemplateProofScope<'r, 'd> {
    /// Admit a proof pass on a settled registry, taking the registry savepoint and the
    /// draft's proof guard. Fails with the same admission errors as
    /// [`TypeRegistry::enter_template_proof`] (an unstable fill owner, a non-open limit
    /// owner, or owner contention), leaving both owners untouched.
    pub(crate) fn enter(
        registry: &'r TypeRegistry,
        draft: &'d mut ImageDraft,
    ) -> Result<Self, GenericInvariant> {
        let savepoint =
            registry.enter_template_proof(draft.record_type_count(), draft.enum_type_count())?;
        Ok(Self {
            registry,
            savepoint: Some(savepoint),
            draft: draft.template_proof(),
        })
    }

    /// The real in-progress draft the proof body appends its throwaway image to.
    pub(crate) fn draft(&mut self) -> &mut ImageDraft {
        self.draft.proof_draft()
    }
}

impl Drop for TemplateProofScope<'_, '_> {
    /// Restore the compiler registry's inverse, then let the draft guard drop and discard
    /// everything the proof appended.
    fn drop(&mut self) {
        if let Some(savepoint) = self.savepoint.take() {
            self.registry.exit_template_proof(savepoint);
        }
    }
}

/// One owner-ordered finished transfer from the generic owner: the optional
/// terminal limit row first, followed by the finished collection-payload
/// terminal gathered before it. The names rename the live `{limit, payloads}`
/// pair (A4); the live/terminal distinction is the point — a transfer is never
/// reopened, only merged or adopted whole.
#[must_use = "generic diagnostics must be adopted or reported as one ordered outcome"]
pub(crate) struct GenericDiagnostics {
    first_limit: Option<SourceDiagnostic>,
    collection_payloads: BoundedDiagnostics,
}

impl GenericDiagnostics {
    /// Merge this transfer into the stage's live owner in canonical order:
    /// the one-row limit exception is pushed first — charged exactly once,
    /// here — then the finished collection payloads are absorbed.
    pub(crate) fn merge_into(self, collector: &mut DiagnosticCollector) {
        if let Some(limit) = self.first_limit {
            collector.push(limit);
        }
        collector.absorb(self.collection_payloads);
    }
}

/// The single owner of generic instantiation identity across functions and value
/// types. Interior-mutable so a shared `&TypeRegistry` mints instances during field
/// resolution and body lowering. Type instantiations mint their image record/enum
/// eagerly (declare-then-fill, so a self-referential instantiation terminates and
/// the containment-cycle check rejects it); function instantiations reserve an image
/// index and enqueue their body for the driver to drain in mint order. An isolated
/// generic-template proof pass runs directly on this owner inside a
/// [`TypeRegistry::enter_template_proof`]/[`TypeRegistry::exit_template_proof`] savepoint,
/// which truncates the rows the pass appends; a fill batch never mutates the settled prefix,
/// so that suffix truncation restores the exact pre-proof state.
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
    /// The live bounded owner of ordered collection-payload diagnostics.
    collection_payloads: DiagnosticCollector,
    /// A declare/fill coherence failure discovered while building concrete source
    /// types. Kept inside the defaulted generic owner so private test fixtures cannot
    /// accidentally bypass a newly added top-level registry field.
    build_invariant: Option<GenericInvariant>,
    argument_domain: ArgumentDomain,
}

/// Manual `Default`: the diagnostic owner is deliberately non-`Default` (one
/// live collector per owner, never conjured incidentally), so the generic
/// owner spells its construction while every other field keeps its default.
impl Default for Monomorph {
    fn default() -> Self {
        Self {
            type_insts: Vec::new(),
            type_index: HashMap::new(),
            fn_base: 0,
            fn_insts: Vec::new(),
            fn_index: HashMap::new(),
            fn_queue: VecDeque::new(),
            fill_batch_start: None,
            fill_rows: BTreeMap::new(),
            fill_stack: Vec::new(),
            fill_failures: Vec::new(),
            limit: LimitState::Open,
            collection_payloads: DiagnosticCollector::new(),
            build_invariant: None,
            argument_domain: ArgumentDomain::Concrete,
        }
    }
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
    pub(crate) verdict: DeclarationVerdict,
}

impl StructInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }
}

/// What pass two decided about a value type whose image index pass one already
/// reserved.
///
/// Pass one reserves an id before any body is resolved, so a reference minted by an
/// earlier fill pass binds that reservation — the verdict the later pass will reach
/// does not exist yet. Dropping a refused declaration's row would leave every such
/// reference addressing nothing, and a dangling type argument is a
/// [`GenericInvariant`], which outranks the diagnostics: the cause reported at the
/// declaration never reaches the reader. So the refused row stays in place and
/// records its verdict instead. `Refused` means exactly *not in the accepted set*:
/// no name resolves to it, no construction or match binds it, and its body is
/// empty — but its reserved id still addresses a declaration this project wrote and
/// the compiler refused, whose cause the named-type ledger holds under its name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeclarationVerdict {
    Accepted,
    Refused,
}

impl DeclarationVerdict {
    pub(crate) fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
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
    pub(crate) verdict: DeclarationVerdict,
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

/// Which resource member one ledger entry is: the record or unkeyed group that
/// writes it, and the member's own name.
///
/// The key lives on [`TypeRegistry`] rather than inside [`RecordInfo`] because a
/// record projection is cloned to cross a borrow, and a cloned refusal summary
/// would carry its own report-once flag — one refused member would then steer at
/// every use instead of once.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MemberKey {
    owner: String,
    member: String,
}

impl MemberKey {
    /// A top-level member of the resource record `record`.
    pub(crate) fn field(record: &str, member: &str) -> Self {
        Self {
            owner: record.to_string(),
            member: member.to_string(),
        }
    }

    /// A leaf of one unkeyed group. The owner is the group's anchor
    /// `Record.group` — the same spelling the group's image record type carries —
    /// so a leaf and a top-level member of the same name never share a key.
    pub(crate) fn leaf(record: &str, group: &str, member: &str) -> Self {
        Self {
            owner: format!("{record}.{group}"),
            member: member.to_string(),
        }
    }

    fn owns(&self, owner: &str) -> bool {
        self.owner == owner
    }

    fn member(&self) -> &str {
        &self.member
    }
}

/// What kind of named type a declared name binds. The ledger's accepted payload:
/// enough to say what a name already is when a second declaration takes it, and
/// nothing the kind-specific tables already own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NamedTypeKind {
    Alias,
    Nominal,
    Struct,
    Enum,
    Resource,
    /// A generic `struct`/`enum`: a template monomorphized on use rather than a
    /// concrete image type, but the same declared type name. Templates and
    /// concrete types share one namespace — the registry's own conflict predicate
    /// scans both — so they share one ledger.
    Template,
}

/// The project named-type registry: the transparent aliases, the nominal int
/// types, the dense struct value types, and the durable-capable record types.
pub(crate) struct TypeRegistry {
    /// Every declared type name in one namespace, accepted or refused.
    ///
    /// The kind-specific tables below stay the authority for what an *accepted*
    /// name resolves to and for image order; this ledger is the authority for
    /// whether a name was declared at all. A refused declaration is dropped from
    /// its table exactly as before — so no construction or match resolves against
    /// a broken type — and is retained here, so the use that can no longer resolve
    /// is steered to the cause instead of being told the name was never written.
    named: DeclarationLedger<String, NamedTypeKind>,
    /// Every member declared by a resource record or one of its unkeyed groups,
    /// accepted or refused, in declaration order.
    ///
    /// This is the authority for which members survived and in what order:
    /// `RecordInfo::fields` and `GroupInfo::fields` are read out of `accepted()`,
    /// so the record cannot hold a member the ledger does not, and a member the
    /// compiler refused answers `Refused` at the lookups that would otherwise
    /// report the record as having no such field.
    members: DeclarationLedger<MemberKey, FieldInfo>,
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
    /// Lookup-only secondary index `CollSpec -> row in collections`, appended in lockstep
    /// with `collections` and carrying the same authority discipline as the type/function
    /// instantiation indexes: `collections` stays the sole COLLTYPES-order authority, and
    /// the reused row's index is always read from the vector, never invented from this map.
    /// It only accelerates the mint-dedup reuse probe from a linear spec scan to a keyed
    /// lookup; a row that does not carry the looked-up spec is index/authority drift,
    /// reported as the shared `MintIndexDrift` coherence failure rather than trusted.
    collection_index: RefCell<HashMap<CollSpec, u16>>,
    /// A metadata directory reused across every probe of one monomorphization pass — the
    /// mint/dedup probes and the presentation projections (field access, spelling, durable
    /// walks) alike. Type instantiations and collections are appended in strict image
    /// order, so the directory maps image identity to row and is extended for the newly
    /// appended rows rather than rebuilt over every prior row on each probe. It is a
    /// projection of the append-only owners, never a mint/dedup authority; a caller that
    /// mutates an already-classified row out of the append order must invalidate it.
    row_directory: RefCell<Option<RowDirectory>>,
}

impl TypeRegistry {
    /// A registry with no declared type, charging its retentions against the pass's
    /// `budget`. There is no `Default`: a ledger that retains off the pass's books
    /// would let the declared ceiling be crossed without reporting it.
    ///
    /// Production builds the registry through [`Self::build`]; this exists for the
    /// lowering tests that need a registry holding only the reserved templates.
    #[cfg(test)]
    pub(crate) fn empty(budget: DeclarationBudget) -> Self {
        Self {
            named: DeclarationLedger::new(DeclarationNamespace::NamedType, budget.clone()),
            members: DeclarationLedger::new(DeclarationNamespace::ResourceMember, budget),
            aliases: BTreeMap::new(),
            nominals: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            records: Vec::new(),
            type_templates: Vec::new(),
            generics: RefCell::default(),
            collections: RefCell::default(),
            collection_index: RefCell::default(),
            row_directory: RefCell::default(),
        }
    }
}

/// The image-identity directory of the monomorphization pass, plus the image-order
/// watermarks it has already classified. Extended in place as rows and collections are
/// appended; it holds identity mapping and per-walk marks, not argument keys.
struct RowDirectory {
    scratch: MetadataScratch,
    declared: DeclaredCounts,
    built_type_insts: usize,
    built_collections: usize,
}

/// The declared-type population a directory classified. DeclarationSite records (with their
/// groups), structs, and enums, and the groups of each record, are all fixed once
/// monomorphization begins — the declare phase completes before the first mint — so in
/// the production pipeline incremental extension only appends generic rows and
/// collections, and this length triple is a complete change-detector: a differing count
/// forces a rebuild. It is kept O(1) rather than summing group counts per probe so the
/// reuse check adds no per-mint factor in the declared-type count. A test that mutates a
/// committed declared type out of that append order reclassifies via
/// `invalidate_row_directory`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct DeclaredCounts {
    records: usize,
    structs: usize,
    enums: usize,
}

impl DeclaredCounts {
    fn of(registry: &TypeRegistry) -> Self {
        Self {
            records: registry.records.len(),
            structs: registry.structs.len(),
            enums: registry.enums.len(),
        }
    }
}

impl RowDirectory {
    /// A directory classifying every currently declared and instantiated row, with its
    /// watermarks set to the current image lengths. Used to seed the cache.
    fn build_full(view: &TypeMetadataView<'_>) -> Result<Self, GenericInvariant> {
        Ok(Self {
            scratch: MetadataScratch::try_new(view)?,
            declared: DeclaredCounts::of(view.registry),
            built_type_insts: view.generics.type_insts.len(),
            built_collections: view.collections.len(),
        })
    }

    /// Classify the type instantiations and collections appended since the last build,
    /// extending the directory in image order. Rows below the watermark were classified
    /// on a prior probe and are not revisited. A `(template, args)` semantic-key collision
    /// cannot arise on an appended row (mint dedup admits only a fresh key), so extension
    /// checks only image-identity placement; the full `try_new` semantic-key scan still
    /// runs on every cold or invalidated build and on every unrouted projection path.
    fn extend(&mut self, view: &TypeMetadataView<'_>) -> Result<(), GenericInvariant> {
        let type_insts = view.generics.type_insts.len();
        for row in self.built_type_insts..type_insts {
            // During an isolated template proof the reused directory already classifies the
            // whole settled population, so extension only reaches the rows the proof body
            // itself mints. Counting them here is the proof's per-template row cost — the
            // owner-decoupled successor to the discarded clone's whole-population replay.
            #[cfg(test)]
            if view.generics.argument_domain == ArgumentDomain::TemplateProof {
                bump_scaling(|counts| counts.proof_clone_rows += 1);
            }
            let id = view.generics.type_insts[row].id;
            place_generic_row(&mut self.scratch.records, &mut self.scratch.enums, row, id)?;
        }
        self.built_type_insts = type_insts;
        let collections = view.collections.len();
        for index in self.built_collections..collections {
            let target = collection_generic_target(
                &self.scratch.records,
                &self.scratch.enums,
                &self.scratch.collection_generic_targets,
                index,
                view.collections[index],
            );
            self.scratch.collection_generic_targets.push(target);
        }
        self.built_collections = collections;
        Ok(())
    }

    /// Discard the classification of every row and collection appended during a
    /// generic-template proof pass, restoring the directory to the pre-proof image so the
    /// cache stays reusable without a full rebuild and holds no truncated-row identity a
    /// later real mint would collide with. The image record/enum id ceilings shrink to the
    /// pre-proof draft counts (`records`/`enums`) — every proof row reserved an id at or
    /// above them — and the watermarks return to the pre-proof instantiation and collection
    /// counts. The per-walk marks are re-sized on the next probe by `reset_marks`.
    fn rewind_to(&mut self, records: usize, enums: usize, type_insts: usize, collections: usize) {
        self.scratch.records.truncate(records);
        self.scratch.enums.truncate(enums);
        self.scratch
            .collection_generic_targets
            .truncate(collections);
        self.built_type_insts = type_insts;
        self.built_collections = collections;
    }

    /// Reset the per-walk visitation marks to cover every current row and collection.
    /// The directory content persists; only the traversal state is cleared for the next
    /// probe.
    fn reset_marks(&mut self, view: &TypeMetadataView<'_>) {
        let type_insts = view.generics.type_insts.len();
        let collections = view.collections.len();
        self.scratch.seen_rows.clear();
        self.scratch.seen_rows.resize(type_insts, false);
        self.scratch.seen_collections.clear();
        self.scratch.seen_collections.resize(collections, false);
        self.scratch.tasks.clear();
    }
}

/// A borrowed row directory. On drop it is returned to the registry cache so the next
/// mint probe extends it rather than rebuilding over every prior row.
struct RowDirectoryGuard<'r> {
    registry: &'r TypeRegistry,
    directory: Option<RowDirectory>,
}

impl RowDirectoryGuard<'_> {
    #[expect(
        clippy::expect_used,
        reason = "the directory is Some from construction until Drop takes it; no other \
                  path clears it, so this guard cannot observe None"
    )]
    fn scratch(&mut self) -> &mut MetadataScratch {
        &mut self
            .directory
            .as_mut()
            .expect("directory is present until drop")
            .scratch
    }
}

impl std::ops::Deref for RowDirectoryGuard<'_> {
    type Target = MetadataScratch;

    #[expect(
        clippy::expect_used,
        reason = "the directory is Some from construction until Drop takes it; no other \
                  path clears it, so this guard cannot observe None"
    )]
    fn deref(&self) -> &MetadataScratch {
        &self
            .directory
            .as_ref()
            .expect("directory is present until drop")
            .scratch
    }
}

impl std::ops::DerefMut for RowDirectoryGuard<'_> {
    fn deref_mut(&mut self) -> &mut MetadataScratch {
        self.scratch()
    }
}

impl Drop for RowDirectoryGuard<'_> {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take() {
            *self.registry.row_directory.borrow_mut() = Some(directory);
        }
    }
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

/// Dense, validation-local lookup and visitation state. The directory is classified
/// from immutable registry rows, reused across the probes of one pass and extended for
/// newly appended rows, and invalidated before any out-of-order owner mutation; it is a
/// projection of the append-only owners, not a mint/dedup authority.
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
    metadata: RowDirectoryGuard<'a>,
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
        // Profiles cannot disagree: the write is idempotent. Only a caller whose
        // `enter_row` returned true leaves, and clearing an already-clear slot is the
        // same state either way.
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
        // Idempotent on the same terms as `leave_row`.
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
    /// Generic instantiation rows classified into directories, counting a full build's
    /// whole population and an incremental extension's newly appended rows alike. When a
    /// directory is rebuilt per probe this is `directory_builds * type_insts.len()`; when
    /// the reused row directory is extended it is one visit per appended row, so it grows
    /// linearly with the instantiation count rather than quadratically.
    pub(crate) directory_row_visits: usize,
    /// Elements examined by the `(template, args)` primary-key scan in
    /// `existing_type_instance` (type-mint reuse).
    pub(crate) type_inst_scan_steps: usize,
    /// Elements examined by the `(template, args)` primary-key scan in
    /// `reserve_fn_instance` (function-mint reuse).
    pub(crate) fn_inst_scan_steps: usize,
    /// Reuse probes performed by `instantiate_collection` (collection-mint dedup). With
    /// the keyed index this is one keyed lookup per instantiation attempt, so the count is
    /// the mint-attempt count and grows linearly with the collection population — the
    /// former linear spec scan made per-attempt work O(collections), i.e. O(collections²).
    pub(crate) coll_inst_probe_steps: usize,
    /// Value-graph edges traversed across every `cycle_through` start.
    pub(crate) cycle_walk_steps: usize,
    /// `enter_template_proof` admissions (one isolated proof pass per generic template).
    pub(crate) proof_clones: usize,
    /// Type-inst rows the proof pass classifies into the shared metadata directory — the
    /// rows its own body mints, extended onto the already-built population directory rather
    /// than replayed over the whole settled population. Constant per template, decoupled from
    /// the instantiation count.
    pub(crate) proof_clone_rows: usize,
    /// Characters rendered into editor hover displays across the whole compile
    /// (`ty.spelling`/signature displays). A monomorphized instance body's facts are
    /// discarded — an instance's use-site spans duplicate its template's — so its
    /// spelling is never rendered; only monomorphic function and test bodies contribute.
    /// On a divergent-monomorphization program the pre-repair per-instance render made
    /// this Σ O(depth) = O(instances²); the repair holds it to the monomorphic baseline.
    pub(crate) hover_spelling_chars: usize,
}

#[cfg(test)]
thread_local! {
    static SCALING_COUNTS: Cell<ScalingCounts> = const { Cell::new(ScalingCounts {
        directory_builds: 0,
        directory_row_visits: 0,
        type_inst_scan_steps: 0,
        fn_inst_scan_steps: 0,
        coll_inst_probe_steps: 0,
        cycle_walk_steps: 0,
        proof_clones: 0,
        proof_clone_rows: 0,
        hover_spelling_chars: 0,
    }) };
}

/// Observe editor hover-display rendering work: the character length of one rendered
/// hover display. A no-op outside the scaling-count test window.
#[cfg(test)]
pub(crate) fn bump_hover_spelling_chars(chars: usize) {
    bump_scaling(|counts| counts.hover_spelling_chars += chars);
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

/// Deterministic work counts for alias-cycle classification. These observe the
/// real alias-table owner only in ordinary test builds and cannot affect graph
/// state, diagnostics, or accepted programs.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AliasCycleCounts {
    target_visits: usize,
    resolved_edges: usize,
    node_entries: usize,
    edge_inspections: usize,
    cyclic_aliases: usize,
}

#[cfg(test)]
thread_local! {
    static ALIAS_CYCLE_COUNTS: Cell<AliasCycleCounts> = const {
        Cell::new(AliasCycleCounts {
            target_visits: 0,
            resolved_edges: 0,
            node_entries: 0,
            edge_inspections: 0,
            cyclic_aliases: 0,
        })
    };
}

#[cfg(test)]
fn bump_alias_cycle(update: impl FnOnce(&mut AliasCycleCounts)) {
    ALIAS_CYCLE_COUNTS.with(|cell| {
        let mut counts = cell.get();
        update(&mut counts);
        cell.set(counts);
    });
}

#[cfg(test)]
fn capture_alias_cycle_counts<T>(run: impl FnOnce() -> T) -> (T, AliasCycleCounts) {
    let previous = ALIAS_CYCLE_COUNTS.with(|cell| cell.replace(AliasCycleCounts::default()));
    let result = run();
    let counts = ALIAS_CYCLE_COUNTS.with(Cell::get);
    ALIAS_CYCLE_COUNTS.with(|cell| cell.set(previous));
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

/// Place one generic instantiation row into the record/enum directory, rejecting a
/// second owner of the same image type identity. Shared by the full directory build
/// and the batch-scoped incremental extension so both classify identity once.
fn place_generic_row(
    records: &mut Vec<Option<RecordMetadataOwner>>,
    enums: &mut Vec<Option<EnumMetadataOwner>>,
    row: usize,
    id: TypeInstId,
) -> Result<(), GenericInvariant> {
    #[cfg(test)]
    bump_scaling(|counts| counts.directory_row_visits += 1);
    match id {
        TypeInstId::Record(record_id) => {
            let index = record_id.index() as usize;
            if records.len() <= index {
                records.resize(index + 1, None);
            }
            let slot = &mut records[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(id));
            }
            *slot = Some(RecordMetadataOwner::GenericRow(row));
        }
        TypeInstId::Enum(enum_id) => {
            let index = enum_id.index() as usize;
            if enums.len() <= index {
                enums.resize(index + 1, None);
            }
            let slot = &mut enums[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(id));
            }
            *slot = Some(EnumMetadataOwner::GenericRow(row));
        }
    }
    Ok(())
}

/// The generic row a collection instantiation resolves to for ordering: the latest
/// (highest-row) generic target among the collection's element/key/value arguments and
/// any nested collection already resolved. `index` is the collection's own position, so
/// only strictly earlier child collections are consulted. Shared by the full directory
/// build and the batch-scoped incremental extension.
fn collection_generic_target(
    records: &[Option<RecordMetadataOwner>],
    enums: &[Option<EnumMetadataOwner>],
    resolved_targets: &[Option<GenericRowRef>],
    index: usize,
    spec: CollSpec,
) -> Option<GenericRowRef> {
    let direct = |arg: GArg| -> Option<GenericRowRef> {
        match arg {
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
        }
    };
    let mut latest: Option<GenericRowRef> = None;
    let mut consider = |candidate: Option<GenericRowRef>| {
        if candidate.is_some_and(|candidate| {
            latest.is_none_or(|current: GenericRowRef| candidate.row > current.row)
        }) {
            latest = candidate;
        }
    };
    let mut consider_arg = |arg: GArg| {
        consider(direct(arg));
        if let GArg::Collection(child) = arg
            && (child as usize) < index
        {
            consider(resolved_targets.get(child as usize).copied().flatten());
        }
    };
    match spec {
        CollSpec::List { elem } => consider_arg(elem),
        CollSpec::Map { key, value } => {
            consider_arg(key);
            consider_arg(value);
        }
    }
    latest
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
            place_generic_row(&mut records, &mut enums, row, inst.id)?;
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
        let mut collection_generic_targets = Vec::with_capacity(view.collections.len());
        for (index, spec) in view.collections.iter().copied().enumerate() {
            let latest = collection_generic_target(
                &records,
                &enums,
                &collection_generic_targets,
                index,
                spec,
            );
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
        // Profiles cannot disagree: the drain loop below empties `tasks` on every path
        // that returns `Ok`, and a `?` leaves entries behind only after an invariant that
        // has already ended the compile.
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
        // Profiles cannot disagree: nothing here branches on the flag. The
        // `validate_args_with` call above visits this row, and this restates that
        // postcondition beside the `Ok` it returns either way.
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
                TypeExpr::Optional { .. } | TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => {
                    return Ok(false);
                }
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
            let Some(info) = self.view.registry.struct_by_name(name) else {
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
        let result = Ok(self.view.registry.enum_by_name(name).cloned());
        self.remember(result)
    }

    pub(crate) fn static_named_type(
        &mut self,
        name: &str,
    ) -> Result<Option<StaticNamedType>, GenericInvariant> {
        self.ensure_healthy()?;
        let registry = self.view.registry;
        let result = Ok(if let Some(info) = registry.struct_by_name(name) {
            Some(StaticNamedType::Struct(info.type_id))
        } else if let Some(info) = registry.enum_by_name(name) {
            Some(StaticNamedType::Enum(info.enum_id))
        } else {
            registry
                .by_name(name)
                .map(|info| StaticNamedType::Record(info.type_id))
        });
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
                    Ok(match self.view.registry.member(&info.name, name)? {
                        Binding::Refused(id, _) => ProductFieldProjection::RefusedMember(id),
                        Binding::Accepted(_) | Binding::Absent => {
                            ProductFieldProjection::MissingRecordField
                        }
                    })
                }
                RecordMetadataOwner::Group(record, group) => {
                    let owner = &self.view.registry.records[record];
                    let info = &owner.groups[group];
                    let Some((index, field)) = info.field(name) else {
                        let anchor = format!("{}.{}", owner.name, info.name);
                        return Ok(match self.view.registry.member(&anchor, name)? {
                            Binding::Refused(id, _) => ProductFieldProjection::RefusedMember(id),
                            Binding::Accepted(_) | Binding::Absent => {
                                ProductFieldProjection::MissingGroupField
                            }
                        });
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
        let metadata = self.row_directory(&view).map_err(E::from)?;
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

    /// A metadata directory for one mint/dedup probe or presentation projection. The
    /// directory is reused across the pass's probes and extended for the rows appended
    /// since the previous probe, so minting a deeply nested type — or projecting a field
    /// over a growing instantiation population — classifies each row once instead of
    /// rescanning every prior row per probe. A row appended after the previous probe is
    /// classified now; rows below the watermark were classified before. A metadata session
    /// borrows this same directory, so an out-of-line projection reuses the pass
    /// classification rather than rebuilding a fresh one.
    fn row_directory(
        &self,
        view: &TypeMetadataView<'_>,
    ) -> Result<RowDirectoryGuard<'_>, GenericInvariant> {
        let cached = self.row_directory.borrow_mut().take();
        let reusable = cached.filter(|directory| {
            directory.declared == DeclaredCounts::of(self)
                && directory.built_type_insts <= view.generics.type_insts.len()
                && directory.built_collections <= view.collections.len()
        });
        let mut directory = match reusable {
            Some(directory) => directory,
            None => RowDirectory::build_full(view)?,
        };
        directory.extend(view)?;
        directory.reset_marks(view);
        Ok(RowDirectoryGuard {
            registry: self,
            directory: Some(directory),
        })
    }

    /// Discard the reused row directory so the next probe rebuilds and re-classifies
    /// identity from the owners. The production append path keeps the directory current
    /// without this; only a test that mutates an already-classified row out of the append
    /// order needs to reclassify, so this affordance is test-only.
    #[cfg(test)]
    fn invalidate_row_directory(&self) {
        *self.row_directory.borrow_mut() = None;
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
            // A head no template answers is either genuinely undeclared or a
            // template this project declared and the compiler refused.
            match self.type_template_by_name(head) {
                Some(template) => template,
                None => {
                    return Err(ResolveError::Refusal(self.unresolved_named_type(head)?));
                }
            }
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
        } else if let Some(info) = self.enum_by_name(text) {
            Ok(GArg::Enum(info.enum_id))
        } else {
            // A name no table answers is either genuinely undeclared or a declaration
            // this project refused; the ledger tells them apart. Answering
            // `Unsupported` for both is what let a member position describe a refused
            // sibling as a language form the beta line does not admit — the one
            // statement the subset-gap phrase must never make.
            Err(ResolveError::Refusal(self.unresolved_named_type(text)?))
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
            let mut metadata = view.registry.row_directory(&view)?;
            view.validate_args_with(args, None, metadata.scratch())?;
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
                                .ready_inst_header_with(inst, metadata.scratch())?
                                .ok_or(GenericInvariant::ReadyBodyMissing(inst.id))?;
                            self.validate_ready_requirement(inst, body, requirement)?;
                            view.validate_ready_body_with(inst, body, metadata.scratch())?;
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
            // only appends on a dedup miss, so this key is new; a pre-existing entry is a
            // mint/dedup coherence failure. Reject it as a typed invariant rather than
            // trusting the append: the batch-directory extension classifies an appended
            // row without rescanning `(template, args)`, so a duplicate key must never be
            // admitted here.
            let displaced = generics.type_index.insert((template, args.to_vec()), index);
            if displaced.is_some() {
                return Err(GenericInvariant::CacheState(
                    GenericCacheInvariant::MintKeyAlreadyPresent,
                )
                .into());
            }
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
        let mut metadata = view.registry.row_directory(&view)?;
        let body = view
            .ready_inst_header_with(inst, metadata.scratch())?
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        self.validate_ready_requirement(inst, body, requirement)?;
        view.validate_ready_body_with(inst, body, metadata.scratch())?;
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
    /// result is a valid `.marrow/ids` anchor path (printable ASCII, no spaces). The
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
        // Keep the lookup-only reuse index in lockstep with its authority. A reserve
        // only appends on a dedup miss, so this key is new; a pre-existing entry means
        // the dedup probe and the index disagree. Reject it as a typed invariant on the
        // same terms as the type mint: the append below reserves an image function index
        // and queues a body for it, so a duplicate key would mint a second reservation
        // and a second lowering for one instantiation.
        let displaced = generics
            .fn_index
            .insert((inst.template, inst.args.clone()), row);
        if displaced.is_some() {
            return Err(
                GenericInvariant::CacheState(GenericCacheInvariant::MintKeyAlreadyPresent).into(),
            );
        }
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

    /// Drain the one owner-ordered generic outcome: replace the active live
    /// owner with a fresh collector and finish the removed owner exactly once.
    /// Taking a pending limit advances its owner to `Reported`, so cached
    /// `Rejected(Limit)` rows replay silently.
    pub(crate) fn take_generic_diagnostics(&self) -> GenericDiagnostics {
        let mut generics = self.generics.borrow_mut();
        let first_limit = match std::mem::replace(&mut generics.limit, LimitState::Reported) {
            LimitState::Open => {
                generics.limit = LimitState::Open;
                None
            }
            LimitState::Pending(diagnostic) => Some(diagnostic),
            LimitState::Reported => None,
        };
        let collector = std::mem::replace(
            &mut generics.collection_payloads,
            DiagnosticCollector::new(),
        );
        GenericDiagnostics {
            first_limit,
            collection_payloads: collector.finish(),
        }
    }

    pub(crate) fn has_instantiation_limit(&self) -> bool {
        !matches!(self.generics.borrow().limit, LimitState::Open)
    }

    /// Adopt a proof pass's transfer back into this owner: the limit state is
    /// restored first exactly as taken (an already non-open owner keeps its
    /// state — the transferred row is dropped, never double-charged), then the
    /// finished collection payloads are consumed through the persistent live
    /// owner's `absorb`. A terminal is never reopened.
    pub(crate) fn adopt_generic_diagnostics(&self, outcome: GenericDiagnostics) {
        let GenericDiagnostics {
            first_limit,
            collection_payloads,
        } = outcome;
        let mut generics = self.generics.borrow_mut();
        if matches!(generics.limit, LimitState::Open)
            && let Some(diagnostic) = first_limit
        {
            generics.limit = LimitState::Pending(diagnostic);
        }
        generics.collection_payloads.absorb(collection_payloads);
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
        // Mint-dedup reuse probe: a keyed lookup into the append-only secondary index.
        // The reused row's index is read from `collections` (the authority); a row that
        // does not carry the looked-up spec is drift.
        #[cfg(test)]
        bump_scaling(|counts| counts.coll_inst_probe_steps += 1);
        if let Some(&index) = self.collection_index.borrow().get(&spec) {
            if collections.get(index as usize) != Some(&spec) {
                return Err(
                    GenericInvariant::CacheState(GenericCacheInvariant::MintIndexDrift).into(),
                );
            }
            return Ok(index);
        }
        drop(collections);

        // Profiles cannot disagree: the drift these two restate is already a typed
        // release outcome. The reuse probe above compares the looked-up row against the
        // spec it carries and rejects a mismatch as `MintIndexDrift`, so an index that
        // fell out of step with the draft is refused at the next read in either profile.
        let id = draft.add_collection_type(spec.definition());
        debug_assert_eq!(id.index() as usize, cache_index);
        let mut collections = self.collections.borrow_mut();
        debug_assert_eq!(collections.len(), cache_index);
        collections.push(spec);
        self.collection_index.borrow_mut().insert(spec, id.index());
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

    /// The resource record whose image record type is `ty`, if `ty` is one — the
    /// name a durable lookup keyed on the resource takes.
    pub(crate) fn record_by_type(&self, ty: TypeId) -> Option<&RecordInfo> {
        self.records.iter().find(|info| info.type_id == ty)
    }

    /// The accepted struct declared as `name`.
    ///
    /// A refused row keeps its reserved id addressable but leaves the accepted set,
    /// so it never answers a name: an annotation naming it falls through to
    /// [`Self::unresolved_named_type`], which reads the cause out of the named-type
    /// ledger and steers the use to it.
    ///
    /// This is the only scan of `structs` keyed on a source spelling. The static
    /// projections that annotation resolution, signature building, and body lowering
    /// read delegate here rather than scanning again, because a second scan is a
    /// second place to forget the verdict — and a name answered by a reserved,
    /// unfilled row resolves to a *live empty struct*, against which every later
    /// question fabricates an answer.
    pub(crate) fn struct_by_name(&self, name: &str) -> Option<&StructInfo> {
        self.structs
            .iter()
            .find(|info| info.name == name && info.verdict.is_accepted())
    }

    pub(crate) fn struct_by_type(&self, ty: TypeId) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.type_id == ty)
    }

    /// The accepted enum declared as `name`. A refused row answers no name, for the
    /// reason given at [`Self::struct_by_name`].
    pub(crate) fn enum_by_name(&self, name: &str) -> Option<&EnumInfo> {
        self.enums
            .iter()
            .find(|info| info.name == name && info.verdict.is_accepted())
    }

    pub(crate) fn enum_by_id(&self, id: EnumId) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.enum_id == id)
    }

    /// Why an annotation naming `name` could not resolve.
    ///
    /// The one conversion from a named-type ledger lookup to a resolution refusal,
    /// so `Unsupported` keeps meaning *genuinely outside the admitted subset* and
    /// is never the answer for a type this project declared. A name the ledger
    /// never saw is a real absence; a name it refused carries the cause forward as
    /// a `Copy` handle.
    pub(crate) fn unresolved_named_type(
        &self,
        name: &str,
    ) -> Result<ResolveRefusal, DeclarationIndexDrift> {
        Ok(match self.named.lookup(name)? {
            Binding::Refused(id, _) => ResolveRefusal::RefusedDeclaration(id),
            Binding::Accepted(_) | Binding::Absent => ResolveRefusal::Unsupported,
        })
    }

    /// What the declared type name `name` binds: its kind, the refusal that stands
    /// in its place, or a genuine absence.
    pub(crate) fn named_type(
        &self,
        name: &str,
    ) -> Result<Binding<'_, NamedTypeKind>, DeclarationIndexDrift> {
        self.named.lookup(name)
    }

    /// The row a member position reports when its declared type does not resolve
    /// to an admitted shape: the causal steer when the annotation names a
    /// declaration this project refused, and the subset-gap phrase otherwise.
    ///
    /// The summary is read out of the same lookup that classified the name, so
    /// there is no handle to mis-address and no drift arm to swallow.
    pub(crate) fn unresolved_member_row(
        &self,
        ty: &TypeExpr,
        file: &FileIdentity,
        subject: &str,
    ) -> Result<SourceDiagnostic, DeclarationIndexDrift> {
        if let TypeExpr::Name { text, .. } = ty
            && let Binding::Refused(_, summary) = self.named.lookup(text.as_str())?
        {
            return Ok(declaration_refused(file, ty.span(), summary));
        }
        Ok(unsupported(file, ty.span(), subject))
    }

    /// The row a member whose type could not resolve is reported with: the causal
    /// steer when the type names a declaration this project refused, the
    /// subset-gap phrase when the name is genuinely outside the admitted set, and
    /// `None` for the shared instantiation limit, which the monomorphization owner
    /// reports once on its own.
    ///
    /// The one place a member-position resolution failure becomes a report, so a
    /// refused sibling declaration can never be described as an unsupported
    /// language form.
    pub(crate) fn member_refusal_row(
        &self,
        refusal: ResolveRefusal,
        file: &FileIdentity,
        span: SourceSpan,
        subject: &str,
    ) -> Result<Option<SourceDiagnostic>, GenericInvariant> {
        match refusal {
            ResolveRefusal::Limit => Ok(None),
            ResolveRefusal::Unsupported => Ok(Some(unsupported(file, span, subject))),
            ResolveRefusal::RefusedDeclaration(id) => {
                let summary = self.refusal(id)?;
                Ok(Some(declaration_refused(file, span, summary)))
            }
        }
    }

    /// The accepted members of `owner`, in declaration order.
    ///
    /// `owner` is a resource record's name, or the `Record.group` anchor of one of
    /// its unkeyed groups. This is what a record's field list is built from, so
    /// the record and the ledger cannot disagree about which members survived.
    pub(crate) fn accepted_members(&self, owner: &str) -> Vec<FieldInfo> {
        self.members
            .accepted()
            .filter(|(key, _)| key.owns(owner))
            .map(|(_, info)| info.clone())
            .collect()
    }

    /// The members `owner` declared and the compiler refused, in declaration order.
    ///
    /// A refused member is still a member the source wrote, so a derivation over a
    /// resource's declared members — the durable identity anchors, above all —
    /// reads this beside `accepted_members` rather than narrowing to the accepted
    /// set alone.
    pub(crate) fn refused_members(&self, owner: &str) -> Vec<&str> {
        self.members
            .refused()
            .filter(|(key, _)| key.owns(owner))
            .map(|(key, _)| key.member())
            .collect()
    }

    /// What the member `member` of `owner` binds: an accepted member, the refusal
    /// its declaration reported, or a genuine absence.
    ///
    /// A lookup that would report "has no field" reads this first, so the one
    /// namespace that refuses a member without refusing what contains it cannot
    /// make a false statement about the source.
    pub(crate) fn member(
        &self,
        owner: &str,
        member: &str,
    ) -> Result<Binding<'_, FieldInfo>, DeclarationIndexDrift> {
        self.members.lookup(&MemberKey::field(owner, member))
    }

    /// The same steer for a member a projection already resolved to a refusal
    /// handle, so the owner's name is not spelled a second time at the use site.
    pub(crate) fn refused_member_steer(
        &self,
        id: DeclarationRefusalId,
        file: &FileIdentity,
        span: SourceSpan,
    ) -> Result<Option<SourceDiagnostic>, DeclarationIndexDrift> {
        let summary = self.members.refusal(id)?;
        Ok(summary
            .steer_once()
            .then(|| declaration_refused(file, span, summary)))
    }

    /// The refusal a named-type or template handle addresses. Every other
    /// namespace's handle is drift here, checked by the ledger's own tag.
    pub(crate) fn refusal(
        &self,
        id: DeclarationRefusalId,
    ) -> Result<&DeclarationRefusalSummary, DeclarationIndexDrift> {
        self.named.refusal(id)
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
            TypeExpr::Apply {
                head,
                head_span,
                args,
                span,
            } => TypeExpr::Apply {
                head: head.clone(),
                head_span: *head_span,
                args: args.iter().map(|arg| self.expand(arg)).collect(),
                span: *span,
            },
            TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => ty.clone(),
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
        aliases: &[(FileRef, FileIdentity, &AliasDecl)],
        nominals: &[(FileRef, FileIdentity, &NominalDecl)],
        structs: &[(FileRef, FileIdentity, &StructDecl)],
        enums: &[(FileRef, FileIdentity, &EnumDecl)],
        resources: &[(FileRef, FileIdentity, &ResourceDecl)],
        diagnostics: &mut DiagnosticCollector,
        budget: DeclarationBudget,
    ) -> Result<Self, BuildError> {
        let mut named = DeclarationLedger::new(DeclarationNamespace::NamedType, budget.clone());
        let aliases_table =
            build_alias_table(&mut named, aliases, resources, structs, enums, diagnostics)?;
        let mut registry = Self {
            named,
            members: DeclarationLedger::new(DeclarationNamespace::ResourceMember, budget),
            aliases: aliases_table,
            nominals: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            records: Vec::new(),
            type_templates: reserved_templates(),
            generics: RefCell::default(),
            collections: RefCell::default(),
            collection_index: RefCell::default(),
            row_directory: RefCell::default(),
        };
        registry.nominals = build_nominals(
            &mut registry,
            nominals,
            resources,
            structs,
            enums,
            diagnostics,
        )?;

        // A generic `struct`/`enum` (one carrying type parameters) is a template
        // monomorphized on use, not a concrete image type; the concrete declarations
        // are declared-then-filled below, the templates registered aside.
        let concrete_structs: Vec<(FileRef, FileIdentity, &StructDecl)> = structs
            .iter()
            .filter(|(_, _, decl)| decl.type_params.is_empty())
            .map(|(at, file, decl)| (*at, file.clone(), *decl))
            .collect();
        let concrete_enums: Vec<(FileRef, FileIdentity, &EnumDecl)> = enums
            .iter()
            .filter(|(_, _, decl)| decl.type_params.is_empty())
            .map(|(at, file, decl)| (*at, file.clone(), *decl))
            .collect();
        register_type_templates(&mut registry, structs, enums, resources, diagnostics)?;

        // Pass one: reserve every value type's image index with empty members and
        // decide name conflicts. The records reserve first (image indices `0..n`),
        // so a project's durable root and sites keep the same record index whether
        // or not dense structs are also declared.
        let record_decls = declare_records(draft, &mut registry, resources, diagnostics)?;
        let struct_decls = declare_structs(
            draft,
            &mut registry,
            &concrete_structs,
            resources,
            diagnostics,
        )?;
        let enum_decls = declare_enums(
            draft,
            &mut registry,
            &concrete_enums,
            resources,
            diagnostics,
        )?;

        // Pass two: resolve and fill each definition's members against the full
        // registry, monomorphizing any generic field type on first use. Each pass
        // records its verdict — accepted, or refused with the cause it reported —
        // in the named-type ledger, so pass one's reservation never stands as the
        // answer for a name pass two went on to refuse.
        let result = fill_records(draft, &mut registry, &record_decls, diagnostics)
            .and_then(|()| fill_structs(draft, &mut registry, &struct_decls, diagnostics));
        match result {
            Ok(()) => {
                fill_enums(draft, &mut registry, &enum_decls, diagnostics)?;
                validate_alias_targets(&mut registry, aliases, diagnostics)?;
            }
            // A coherence failure is recorded on the registry rather than
            // returned: the remaining fills are skipped and `build_invariant` is
            // what fences the pass off from the artifacts.
            Err(BuildError::Invariant(invariant)) => {
                registry.generics.get_mut().build_invariant = Some(invariant);
            }
            Err(full @ BuildError::LedgerFull(_)) => return Err(full),
        }
        Ok(registry)
    }

    pub(crate) fn build_invariant(&self) -> Option<GenericInvariant> {
        self.generics.borrow().build_invariant
    }

    /// Admit an isolated generic-template proof pass to run directly on this registry, and
    /// capture the state needed to erase its effects. The pass mints type instantiations and
    /// collections and reports diagnostics against the abstract type parameters; on exit
    /// [`Self::exit_template_proof`] truncates the appended rows and re-seats the swapped
    /// owners, so nothing the proof appended survives and only the diagnostics the caller
    /// takes cross back.
    ///
    /// A fill batch mutates only `type_insts[start..]` — settlement, staging, and dependency
    /// edges never touch a settled prefix row (a dependent is recorded only for a `Filling`
    /// row, and settlement clears and commits only the active suffix). The proof's batches
    /// all open at or above the length captured here, so the settled prefix is immutable
    /// across the pass and truncation is its exact inverse. Admission requires that settled
    /// state: no fill in progress, no provisional or still-referenced row, no recorded build
    /// fault, and the shared instantiation-limit owner open.
    ///
    /// `entry_records`/`entry_enums` are the draft's record/enum id ceilings at entry, used
    /// to roll the reused metadata directory back to the pre-proof image.
    pub(crate) fn enter_template_proof(
        &self,
        entry_records: usize,
        entry_enums: usize,
    ) -> Result<RegistryProofSavepoint, GenericInvariant> {
        let mut generics = self
            .generics
            .try_borrow_mut()
            .map_err(|_| GenericInvariant::ProofClone(ProofCloneError::UnstableFillState))?;
        let has_unstable_row = generics.type_insts.iter().any(|inst| {
            matches!(inst.state, TypeInstState::Filling { .. }) || !inst.dependents.is_empty()
        });
        // A proof pass is non-reentrant: it swaps the argument domain to `TemplateProof` for
        // its duration and restores it on exit, so finding it already `TemplateProof` on entry
        // means a prior pass never exited (or the owner is otherwise unsettled). Reject rather
        // than nest, keeping the swap a clean save/restore pair.
        if generics.fill_batch_start.is_some()
            || !generics.fill_rows.is_empty()
            || !generics.fill_stack.is_empty()
            || !generics.fill_failures.is_empty()
            || has_unstable_row
            || generics.build_invariant.is_some()
            || !matches!(generics.argument_domain, ArgumentDomain::Concrete)
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
        // Contention on the collection owner is a coherence failure, not a RefCell unwind;
        // read its length before any mutation so a conflict leaves the registry untouched.
        let collections = self
            .collections
            .try_borrow()
            .map_err(|_| GenericInvariant::ProofClone(ProofCloneError::UnstableFillState))?
            .len();
        #[cfg(test)]
        bump_scaling(|counts| counts.proof_clones += 1);
        let savepoint = RegistryProofSavepoint {
            type_insts: generics.type_insts.len(),
            collections,
            fn_insts: generics.fn_insts.len(),
            fn_queue: generics.fn_queue.len(),
            prior_argument_domain: generics.argument_domain,
            // Whole-owner swap: the proof pass gets a fresh live collector and
            // the prior owner is saved intact for exit to re-seat.
            prior_payloads: std::mem::replace(
                &mut generics.collection_payloads,
                DiagnosticCollector::new(),
            ),
            entry_records,
            entry_enums,
        };
        generics.argument_domain = ArgumentDomain::TemplateProof;
        Ok(savepoint)
    }

    /// Restore the registry to the exact state captured by `savepoint`, erasing every effect
    /// of the proof pass. Appended type instantiations and collections are truncated and
    /// their lockstep secondary-index keys removed (a purge proportional to the appended
    /// rows, never the settled population); the transient fill state — empty around a settled
    /// batch, but possibly dirty after a proof that failed mid-fill — is reset; and the
    /// argument domain, ordered-diagnostic buffer, and instantiation-limit owner are
    /// re-seated. The reused metadata directory is rolled back to the pre-proof image.
    pub(crate) fn exit_template_proof(&self, savepoint: RegistryProofSavepoint) {
        let RegistryProofSavepoint {
            type_insts,
            collections,
            fn_insts,
            fn_queue,
            prior_argument_domain,
            prior_payloads,
            entry_records,
            entry_enums,
        } = savepoint;
        {
            let mut generics = self.generics.borrow_mut();
            for inst in generics.type_insts.split_off(type_insts) {
                generics.type_index.remove(&(inst.template, inst.args));
            }
            for inst in generics.fn_insts.split_off(fn_insts) {
                generics.fn_index.remove(&(inst.template, inst.args));
            }
            generics.fn_queue.truncate(fn_queue);
            generics.fill_batch_start = None;
            generics.fill_rows.clear();
            generics.fill_stack.clear();
            generics.fill_failures.clear();
            generics.limit = LimitState::Open;
            generics.build_invariant = None;
            generics.argument_domain = prior_argument_domain;
            generics.collection_payloads = prior_payloads;
        }
        {
            let mut colls = self.collections.borrow_mut();
            let mut index = self.collection_index.borrow_mut();
            for spec in colls.split_off(collections) {
                index.remove(&spec);
            }
        }
        if let Some(directory) = self.row_directory.borrow_mut().as_mut() {
            directory.rewind_to(entry_records, entry_enums, type_insts, collections);
        }
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
        segment_spans: Vec::new(),
        span: SourceSpan::default(),
    };
    let payload = |ty: TypeExpr| TemplatePayload {
        name: "value".to_string(),
        ty,
    };
    vec![
        TypeTemplate {
            name: "Option".to_string(),
            file: None,
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
            file: None,
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
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), DeclareError> {
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
            || resources.iter().any(|(_, _, r)| r.name == name)
            || structs
                .iter()
                .filter(|(_, _, d)| d.type_params.is_empty())
                .any(|(_, _, d)| d.name == name)
            || enums
                .iter()
                .filter(|(_, _, d)| d.type_params.is_empty())
                .any(|(_, _, d)| d.name == name)
            || registry
                .type_templates
                .iter()
                .any(|template| template.name == name)
    };
    for (at, file, decl) in structs {
        if decl.type_params.is_empty() {
            continue;
        }
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
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
        let mut refusal = None;
        let fields = template_struct_fields(file, decl, diagnostics, declared, &mut refusal);
        if let Some(fields) = fields.as_ref() {
            for (_, ty) in fields {
                if let Some(row) = unknown_template_member(
                    registry,
                    structs,
                    enums,
                    resources,
                    &decl.type_params,
                    ty,
                    file,
                ) {
                    refuse_first(&mut refusal, diagnostics, declared, row);
                }
            }
        }
        let fields = match (fields, refusal) {
            (Some(fields), None) => fields,
            // Every arm that drops the members, and every member type that names
            // nothing declared, reported through the accumulator, so a refused
            // template always carries the cause a use is steered to.
            (_, Some(refusal)) => {
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
            (None, None) => continue,
        };
        registry.named.declare(
            decl.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Template),
        )?;
        registry.type_templates.push(TypeTemplate {
            name: decl.name.clone(),
            file: Some(file.clone()),
            name_span: decl.name_span,
            reserved: None,
            type_params: type_param_names(&decl.type_params),
            body: TemplateBody::Struct(fields),
        });
    }
    for (at, file, decl) in enums {
        if decl.type_params.is_empty() {
            continue;
        }
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
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
        let mut refusal = None;
        let variants = template_enum_variants(file, decl, diagnostics, declared, &mut refusal);
        if let Some(variants) = variants.as_ref() {
            for variant in variants {
                for payload in &variant.payload {
                    if let Some(row) = unknown_template_member(
                        registry,
                        structs,
                        enums,
                        resources,
                        &decl.type_params,
                        &payload.ty,
                        file,
                    ) {
                        refuse_first(&mut refusal, diagnostics, declared, row);
                    }
                }
            }
        }
        let variants = match (variants, refusal) {
            (Some(variants), None) => variants,
            // Every arm that drops the members, and every member type that names
            // nothing declared, reported through the accumulator, so a refused
            // template always carries the cause a use is steered to.
            (_, Some(refusal)) => {
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
            (None, None) => continue,
        };
        registry.named.declare(
            decl.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Template),
        )?;
        registry.type_templates.push(TypeTemplate {
            name: decl.name.clone(),
            file: Some(file.clone()),
            name_span: decl.name_span,
            reserved: None,
            type_params: type_param_names(&decl.type_params),
            body: TemplateBody::Enum(variants),
        });
    }
    Ok(())
}

/// The row refusing a generic template's member type that names nothing this
/// project declares, or `None` when the spelling is resolvable.
///
/// A template's member types are resolved per instantiation, so without this check
/// a template whose member names an undeclared type is registered whole and its
/// defect is first reported at a *construction* site — blaming the construction for
/// a declaration's error, and never reporting the declaration at all. The
/// declaration set is read raw because templates register before the concrete types
/// reserve, which is also what lets one template name another declared later.
fn unknown_template_member(
    registry: &TypeRegistry,
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    params: &[marrow_syntax::TypeParamDecl],
    ty: &TypeExpr,
    file: &FileIdentity,
) -> Option<SourceDiagnostic> {
    let declares = |name: &str| {
        params.iter().any(|param| param.name == name)
            || ScalarType::from_spelling(name).is_some()
            || registry.aliases.contains_key(name)
            || registry.nominal_by_name(name).is_some()
            || resources.iter().any(|(_, _, decl)| decl.name == name)
            || structs.iter().any(|(_, _, decl)| decl.name == name)
            || enums.iter().any(|(_, _, decl)| decl.name == name)
            || registry
                .type_templates
                .iter()
                .any(|template| template.name == name)
            || matches!(name, "List" | "Map")
    };
    match ty {
        TypeExpr::Name { text, span, .. } => (!declares(text)).then(|| {
            SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                *span,
                format!("`{text}` does not name a known type"),
            )
        }),
        TypeExpr::Optional { inner, .. } => {
            unknown_template_member(registry, structs, enums, resources, params, inner, file)
        }
        TypeExpr::Apply {
            head,
            head_span,
            args,
            ..
        } => {
            if !declares(head) {
                return Some(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    *head_span,
                    format!("`{head}` does not name a known type"),
                ));
            }
            args.iter().find_map(|arg| {
                unknown_template_member(registry, structs, enums, resources, params, arg, file)
            })
        }
        // An entry identity names a store root, resolved by the durable owner, and a
        // parse-recovery leaf never reaches a `!has_errors` tree.
        TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => None,
    }
}

/// The named field-type expressions of a generic struct template, or `None` if any
/// member is not the bare `name: Type` form (matching the concrete-struct rule; the
/// field types themselves are resolved per instantiation).
fn template_struct_fields(
    file: &FileIdentity,
    decl: &StructDecl,
    diagnostics: &mut DiagnosticCollector,
    declared: DeclarationSite<'_>,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Option<Vec<(String, TypeExpr)>> {
    let mut fields = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(file, member.span(), "a struct group"),
            );
            ok = false;
            continue;
        };
        if !field.keys.is_empty() {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(file, field.span, "a keyed struct field"),
            );
            ok = false;
            continue;
        }
        if field.required {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    field.span,
                    "the `required` keyword on a struct field (struct fields are always required)",
                ),
            );
            ok = false;
            continue;
        }
        if matches!(field.ty, TypeExpr::Optional { .. }) {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(file, field.ty.span(), "an optional struct field type"),
            );
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
    file: &FileIdentity,
    decl: &EnumDecl,
    diagnostics: &mut DiagnosticCollector,
    declared: DeclarationSite<'_>,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Option<Vec<TemplateVariant>> {
    let mut variants = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        if member.category || !member.members.is_empty() {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    member.span,
                    "a category or nested member on a generic enum",
                ),
            );
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
    named: &mut DeclarationLedger<String, NamedTypeKind>,
    aliases: &[(FileRef, FileIdentity, &AliasDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<BTreeMap<String, TypeExpr>, DeclareError> {
    let mut raw: BTreeMap<String, TypeExpr> = BTreeMap::new();
    for (at, file, decl) in aliases {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        // A parse error blocks compilation before this runs, so a missing target
        // only means the declaration itself was reported; skip it quietly.
        let Some(ty) = &decl.ty else { continue };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            named.declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
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
            .any(|(_, _, resource)| resource.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a resource", decl.name),
            ));
            continue;
        }
        if structs.iter().any(|(_, _, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a struct", decl.name),
            ));
            continue;
        }
        if enums.iter().any(|(_, _, item)| item.name == decl.name) {
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

    // Report every member of a cyclic component in the existing sorted alias
    // order, at its declaration.
    let cyclic_membership = alias_cycle_membership(&raw);
    let cyclic: Vec<String> = raw
        .keys()
        .zip(cyclic_membership)
        .filter_map(|(name, cyclic)| cyclic.then_some(name.clone()))
        .collect();
    #[cfg(test)]
    bump_alias_cycle(|counts| counts.cyclic_aliases += cyclic.len());
    for name in &cyclic {
        #[expect(
            clippy::expect_used,
            reason = "lowering bookkeeping: `name` was collected from the alias declaration map being searched, so the lookup finds its declaration"
        )]
        let (at, file, decl) = aliases
            .iter()
            .find(|(_, _, decl)| &decl.name == name)
            .expect("cyclic alias came from the declaration list");
        let refusal = refuse(
            diagnostics,
            DeclarationSite {
                name,
                file,
                at: *at,
                span: decl.name_span,
            },
            Code::CheckRecursion.as_str(),
            format!("alias `{name}` is part of a cyclic alias chain"),
        );
        named.declare(name.clone(), DeclarationOccurrence::Refused(refusal))?;
        raw.remove(name);
    }

    // The survivors are acyclic; expand each target to alias-free form. Whether
    // each one is accepted or refused is settled by `validate_alias_targets`, which
    // is where an alias over an unknown target is reported.
    let expanded: BTreeMap<String, TypeExpr> = raw
        .keys()
        .map(|name| (name.clone(), expand_in(&raw, &raw[name])))
        .collect();
    Ok(expanded)
}

/// Classify cyclic aliases with two iterative graph passes. Node indices follow
/// the alias table's sorted key order; graph order never owns diagnostics.
fn alias_cycle_membership(table: &BTreeMap<String, TypeExpr>) -> Vec<bool> {
    let node_count = table.len();
    let node_by_name: BTreeMap<&str, usize> = table
        .keys()
        .enumerate()
        .map(|(node, name)| (name.as_str(), node))
        .collect();
    let mut adjacency = vec![Vec::new(); node_count];
    let mut reverse = vec![Vec::new(); node_count];
    let mut self_edges = vec![false; node_count];

    for (source, target) in table.values().enumerate() {
        referenced_names(target, &mut |name| {
            let Some(&destination) = node_by_name.get(name) else {
                return;
            };
            adjacency[source].push(destination);
            reverse[destination].push(source);
            self_edges[source] |= source == destination;
            #[cfg(test)]
            bump_alias_cycle(|counts| counts.resolved_edges += 1);
        });
    }

    let finishing_order = alias_graph_finishing_order(&adjacency);
    let mut assigned = vec![false; node_count];
    let mut cyclic = vec![false; node_count];
    let mut stack = Vec::new();
    let mut component = Vec::new();

    for root in finishing_order.into_iter().rev() {
        if assigned[root] {
            continue;
        }
        assigned[root] = true;
        stack.push(root);
        #[cfg(test)]
        bump_alias_cycle(|counts| counts.node_entries += 1);

        while let Some(node) = stack.pop() {
            component.push(node);
            for &next in &reverse[node] {
                #[cfg(test)]
                bump_alias_cycle(|counts| counts.edge_inspections += 1);
                if !assigned[next] {
                    assigned[next] = true;
                    stack.push(next);
                    #[cfg(test)]
                    bump_alias_cycle(|counts| counts.node_entries += 1);
                }
            }
        }

        let component_is_cyclic = component.len() > 1 || self_edges[component[0]];
        if component_is_cyclic {
            for node in component.drain(..) {
                cyclic[node] = true;
            }
        } else {
            component.clear();
        }
    }

    cyclic
}

/// Iterative depth-first postorder for the first Kosaraju pass.
fn alias_graph_finishing_order(adjacency: &[Vec<usize>]) -> Vec<usize> {
    let mut entered = vec![false; adjacency.len()];
    let mut order = Vec::with_capacity(adjacency.len());
    let mut stack: Vec<(usize, usize)> = Vec::new();

    for root in 0..adjacency.len() {
        if entered[root] {
            continue;
        }
        entered[root] = true;
        stack.push((root, 0));
        #[cfg(test)]
        bump_alias_cycle(|counts| counts.node_entries += 1);

        while let Some((node, next_edge)) = stack.last_mut() {
            let Some(&next) = adjacency[*node].get(*next_edge) else {
                order.push(*node);
                stack.pop();
                continue;
            };
            *next_edge += 1;
            #[cfg(test)]
            bump_alias_cycle(|counts| counts.edge_inspections += 1);
            if !entered[next] {
                entered[next] = true;
                stack.push((next, 0));
                #[cfg(test)]
                bump_alias_cycle(|counts| counts.node_entries += 1);
            }
        }
    }

    order
}

/// Visit every type name a target mentions. `referenced_names` and `expand_in`
/// walk the same structure; keeping the traversal here keeps them aligned.
fn referenced_names<'t>(ty: &'t TypeExpr, visit: &mut impl FnMut(&'t str)) {
    #[cfg(test)]
    bump_alias_cycle(|counts| counts.target_visits += 1);
    match ty {
        TypeExpr::Name { text, .. } => visit(text),
        TypeExpr::Optional { inner, .. } => referenced_names(inner, visit),
        TypeExpr::Apply { args, .. } => {
            for arg in args {
                referenced_names(arg, visit);
            }
        }
        TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => {}
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
        TypeExpr::Apply {
            head,
            head_span,
            args,
            span,
        } => TypeExpr::Apply {
            head: head.clone(),
            head_span: *head_span,
            args: args.iter().map(|arg| expand_in(table, arg)).collect(),
            span: *span,
        },
        TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => ty.clone(),
    }
}

/// Every alias must denote a known type, used or not: its expansion is a scalar,
/// the record type, or one optional over either. An unknown name is a
/// `check.type` at the alias; a well-formed but unadmitted shape is a
/// `check.unsupported`.
fn validate_alias_targets(
    registry: &mut TypeRegistry,
    aliases: &[(FileRef, FileIdentity, &AliasDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), DeclareError> {
    let mut refused: Vec<String> = Vec::new();
    for (at, file, decl) in aliases {
        let Some(expanded) = registry.aliases.get(&decl.name) else {
            continue; // duplicate or cyclic: already reported
        };
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.span,
        };
        let head = match expanded {
            TypeExpr::Optional { inner, .. } => inner.as_ref(),
            other => other,
        };
        let refusal = match head {
            TypeExpr::Name { text, .. } => {
                if ScalarType::from_spelling(text).is_none()
                    && registry.by_name(text).is_none()
                    && registry.nominal_by_name(text).is_none()
                    && registry.struct_by_name(text).is_none()
                    && registry.enum_by_name(text).is_none()
                {
                    // The target names nothing this alias can expand to. Whether that
                    // is a genuine absence or a declaration this project wrote and the
                    // compiler refused is the ledger's answer, never this pass's: a
                    // refused target is steered to its own cause, because telling the
                    // reader the name is unknown when it is declared two lines above
                    // is the fabricated absence the declaration ledger exists to kill.
                    Some(match registry.named.lookup(text.as_str())? {
                        Binding::Refused(_, summary) => refuse_row(
                            diagnostics,
                            declared,
                            declaration_refused(file, decl.span, summary),
                        ),
                        Binding::Accepted(_) | Binding::Absent => refuse(
                            diagnostics,
                            declared,
                            Code::CheckType.as_str(),
                            format!("alias `{}` does not name a known type: `{text}`", decl.name),
                        ),
                    })
                } else {
                    None
                }
            }
            _ => Some(refuse_row(
                diagnostics,
                declared,
                unsupported(
                    file,
                    decl.span,
                    &format!("the target type of alias `{}`", decl.name),
                ),
            )),
        };
        let occurrence = match refusal {
            Some(refusal) => {
                refused.push(decl.name.clone());
                DeclarationOccurrence::Refused(refusal)
            }
            None => DeclarationOccurrence::Accepted(NamedTypeKind::Alias),
        };
        registry.named.declare(decl.name.clone(), occurrence)?;
    }
    // A refused alias leaves the expansion table, so a use of its name stops
    // expanding to the target that could not be resolved and reaches the ledger
    // instead. Without this the use would resolve the unknown *target* spelling and
    // be told that name is missing, blaming a name the source never wrote.
    for name in refused {
        registry.aliases.remove(&name);
    }
    Ok(())
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
    registry: &mut TypeRegistry,
    nominals: &[(FileRef, FileIdentity, &NominalDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    enums: &[(FileRef, FileIdentity, &EnumDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<NominalInfo>, DeclareError> {
    let mut built: Vec<NominalInfo> = Vec::new();
    for (at, file, decl) in nominals {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        // A parse error blocks compilation before this runs; a missing piece
        // only means the declaration itself was reported, so skip it quietly.
        let (Some(base), Some(interval)) = (&decl.base, &decl.interval) else {
            continue;
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        // Scalar spellings are keywords the parser already rejects as names;
        // owning them here keeps the conflict predicate self-contained. A nominal
        // this pass already refused holds its name too, so the repeat conflicts
        // whichever of the two the compiler could admit.
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || resources
                .iter()
                .any(|(_, _, resource)| resource.name == decl.name)
            || structs.iter().any(|(_, _, item)| item.name == decl.name)
            || enums.iter().any(|(_, _, item)| item.name == decl.name)
            || registry.named.declared(&decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let refused = match scalar_of(&registry.expand(base)) {
            Some(ScalarType::Int) => None,
            Some(other) => Some(refuse_row(
                diagnostics,
                declared,
                unsupported(
                    file,
                    base.span(),
                    &format!("a nominal type over `{}`", other.spelling()),
                ),
            )),
            None => Some(refuse_row(
                diagnostics,
                declared,
                unsupported(file, base.span(), "this nominal base type"),
            )),
        };
        if let Some(refusal) = refused {
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        let interval = match nominal_interval(file, interval) {
            Ok(bounds) => Ok(bounds),
            Err(row) => Err(refuse_row(diagnostics, declared, *row)),
        };
        let (lo, hi) = match interval {
            Ok(bounds) => bounds,
            Err(refusal) => {
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        let supports = match support_set(file, decl) {
            Ok(supports) => supports,
            Err(row) => {
                let refusal = refuse_row(diagnostics, declared, *row);
                registry
                    .named
                    .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        registry.named.declare(
            decl.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Nominal),
        )?;
        built.push(NominalInfo {
            name: decl.name.clone(),
            lo,
            hi,
            supports,
        });
    }
    Ok(built)
}

/// Evaluate a nominal `in` range to its inclusive `[lo, hi]` bounds. The range
/// follows the language's range operators — `lo..hi` excludes the end, `lo..=hi`
/// includes it — with int-literal bounds (a leading `-` allowed), no step, and
/// at least one admitted value.
/// The interval's inclusive bounds, or the row that refuses it. The row is
/// returned rather than pushed so the caller can retain it as the declaration's
/// cause in the same statement that reports it.
fn nominal_interval(
    file: &FileIdentity,
    interval: &Expression,
) -> Result<(i64, i64), Box<SourceDiagnostic>> {
    let error = |span, message: &str| {
        Err(Box::new(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            file,
            span,
            message.to_string(),
        )))
    };
    let Some(range) = range_expr(interval) else {
        return error(
            interval.span(),
            "a nominal interval is a range of int literals, such as `0..150`",
        );
    };
    if range.step.is_some() {
        return error(range.span, "a nominal interval takes no step");
    }
    let (Some(start), Some(end)) = (range.start, range.end) else {
        return error(range.span, "a nominal interval needs both bounds");
    };
    let (Some(lo), Some(end_value)) = (literal_int(start), literal_int(end)) else {
        return error(range.span, "a nominal interval's bounds are int literals");
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
        Some(hi) if lo <= hi => Ok((lo, hi)),
        _ => error(range.span, "this interval admits no values"),
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
    file: &FileIdentity,
    decl: &NominalDecl,
) -> Result<SupportSet, Box<SourceDiagnostic>> {
    let mut supports = SupportSet::default();
    for spelling in &decl.supports {
        let flag = match spelling.name.as_str() {
            "add" => &mut supports.add,
            "subtract" => &mut supports.subtract,
            "step" => &mut supports.step,
            "scale" => &mut supports.scale,
            other => {
                return Err(Box::new(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    spelling.span,
                    format!(
                        "unknown capability `{other}`; the capabilities are add, subtract, step, scale"
                    ),
                )));
            }
        };
        if *flag {
            return Err(Box::new(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                spelling.span,
                format!("capability `{}` is repeated", spelling.name),
            )));
        }
        *flag = true;
    }
    Ok(supports)
}

/// One struct reserved in pass one: the file it was declared in, its declaration,
/// and the image record index it will fill in pass two.
struct ReservedStruct<'a> {
    file: FileIdentity,
    at: FileRef,
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
    structs: &'a [(FileRef, FileIdentity, &StructDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<ReservedStruct<'a>>, DeclareError> {
    let mut reserved: Vec<ReservedStruct<'a>> = Vec::new();
    for (at, file, decl) in structs {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, _, resource)| resource.name == decl.name)
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
            verdict: DeclarationVerdict::Accepted,
        });
        reserved.push(ReservedStruct {
            file: file.clone(),
            at: *at,
            decl,
            type_id,
        });
    }
    Ok(reserved)
}

/// Pass two for the dense struct types: resolve each reserved struct's fields
/// against the full registry and fill both the registry info and the image record.
/// A struct field is the bare `name: Type` form over any value type — a scalar,
/// nominal, another struct, or a closed enum (`Option`/`Result`/a user `enum`);
/// a group, keyed field, the `required` keyword, an optional type, or an unknown
/// type is `check.unsupported`. A declaration with a member defect is refused whole
/// (its reserved image record stays empty and its name leaves the accepted set) so
/// a later construction or match cannot resolve against a broken struct. Its
/// reserved row stays in place carrying [`DeclarationVerdict::Refused`], so a
/// reference an earlier fill pass minted against the reservation addresses a
/// refused declaration rather than dangling.
fn fill_structs(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    reserved: &[ReservedStruct<'_>],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    for item in reserved {
        let declared = DeclarationSite {
            name: &item.decl.name,
            file: &item.file,
            at: item.at,
            span: item.decl.name_span,
        };
        let occurrence = struct_fields(draft, registry, declared, item.decl, diagnostics)?
            .map_accepted(|(fields, field_defs)| {
                draft.set_record_fields(item.type_id, field_defs);
                if let Some(info) = registry
                    .structs
                    .iter_mut()
                    .find(|info| info.type_id == item.type_id)
                {
                    info.fields = fields;
                }
                NamedTypeKind::Struct
            });
        if matches!(occurrence, DeclarationOccurrence::Refused(_))
            && let Some(info) = registry
                .structs
                .iter_mut()
                .find(|info| info.type_id == item.type_id)
        {
            info.verdict = DeclarationVerdict::Refused;
        }
        registry.named.declare(item.decl.name.clone(), occurrence)?;
    }
    Ok(())
}

/// Resolve a struct's members to its required value fields and their image
/// definitions, or `None` if any member is not the bare `name: Type` form over a
/// value type.
type ResolvedStructFields = (Vec<FieldInfo>, Vec<FieldDef>);

fn struct_fields(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    declared: DeclarationSite<'_>,
    decl: &StructDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DeclarationOccurrence<ResolvedStructFields>, GenericInvariant> {
    let file = declared.file;
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    let mut refusal = None;
    let mut limited = false;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(file, member.span(), "a struct group"),
            );
            continue;
        };
        if !field.keys.is_empty() {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(file, field.span, "a keyed struct field"),
            );
            continue;
        }
        if field.required {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    field.span,
                    "the `required` keyword on a struct field (struct fields are always required)",
                ),
            );
            continue;
        }
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(file, field.ty.span(), "an optional struct field type"),
            );
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
            Err(ResolveError::Refusal(refused)) => {
                match registry.member_refusal_row(
                    refused,
                    file,
                    field.ty.span(),
                    "this struct field type",
                )? {
                    Some(row) => refuse_first(&mut refusal, diagnostics, declared, row),
                    None => limited = true,
                }
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
    Ok(match (refusal, limited) {
        (Some(refusal), _) => DeclarationOccurrence::Refused(refusal),
        // The shared instantiation limit reports once, at the monomorphization
        // owner; this declaration is refused for a cause that pass owns.
        (None, true) => DeclarationOccurrence::Refused(refuse_covered(
            declared,
            Code::CheckInstantiationLimit.as_str(),
        )),
        (None, false) => DeclarationOccurrence::Accepted((fields, field_defs)),
    })
}

/// One enum reserved in pass one: the file it was declared in, its declaration,
/// and the image ENUMS index it will fill in pass two.
struct ReservedEnum<'a> {
    file: FileIdentity,
    at: FileRef,
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
    enums: &'a [(FileRef, FileIdentity, &EnumDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<ReservedEnum<'a>>, DeclareError> {
    let mut reserved: Vec<ReservedEnum<'a>> = Vec::new();
    for (at, file, decl) in enums {
        let declared = DeclarationSite {
            name: &decl.name,
            file,
            at: *at,
            span: decl.name_span,
        };
        if is_reserved_type_name(&decl.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, decl.name_span, &decl.name),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || registry.struct_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, _, resource)| resource.name == decl.name)
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
            let refusal = refuse(
                diagnostics,
                declared,
                Code::CheckResourceLimit.as_str(),
                format!(
                    "an enum declares {} members; the fixed limit is {}",
                    decl.members.len(),
                    marrow_image::bounds::MAX_VARIANTS
                ),
            );
            registry
                .named
                .declare(decl.name.clone(), DeclarationOccurrence::Refused(refusal))?;
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
            verdict: DeclarationVerdict::Accepted,
        });
        reserved.push(ReservedEnum {
            file: file.clone(),
            at: *at,
            decl,
            enum_id,
        });
    }
    Ok(reserved)
}

/// Pass two for the closed flat enum types: resolve each reserved enum's variants
/// and fill both the registry info and the image ENUMS entry. Hierarchy is
/// deferred: a `category` member or a member with nested members is
/// `check.unsupported`. A member's payload is the dense `name: Type` form over bare
/// scalars; an optional or non-scalar payload type is `check.unsupported`. A
/// declaration with a defect is refused whole (its reserved image entry stays empty
/// and its name leaves the accepted set) so a later match cannot resolve against a
/// broken enum. Its reserved row stays in place carrying
/// [`DeclarationVerdict::Refused`], for the reason given at [`fill_structs`].
fn fill_enums(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    reserved: &[ReservedEnum<'_>],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    for item in reserved {
        let declared = DeclarationSite {
            name: &item.decl.name,
            file: &item.file,
            at: item.at,
            span: item.decl.name_span,
        };
        let occurrence = enum_variants(draft, registry, declared, item.decl, diagnostics)?
            .map_accepted(|(variants, variant_defs)| {
                draft.set_enum_variants(item.enum_id, variant_defs);
                if let Some(info) = registry
                    .enums
                    .iter_mut()
                    .find(|info| info.enum_id == item.enum_id)
                {
                    info.variants = variants;
                }
                NamedTypeKind::Enum
            });
        if matches!(occurrence, DeclarationOccurrence::Refused(_))
            && let Some(info) = registry
                .enums
                .iter_mut()
                .find(|info| info.enum_id == item.enum_id)
        {
            info.verdict = DeclarationVerdict::Refused;
        }
        registry.named.declare(item.decl.name.clone(), occurrence)?;
    }
    Ok(())
}

/// One enum's selectable variants and the image definitions that carry them.
type EnumVariants = (Vec<VariantInfo>, Vec<VariantDef>);

/// One enum member's payload fields, as info and as the scalars the image holds.
type EnumPayload = (Vec<EnumPayloadInfo>, Vec<ScalarType>);

/// Resolve an enum's members to its selectable variants and their image
/// definitions, or `None` if any member is unsupported. On the flat line every
/// member is a leaf: a `category` member or one with nested members is deferred.
fn enum_variants(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    declared: DeclarationSite<'_>,
    decl: &EnumDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DeclarationOccurrence<EnumVariants>, DeclarationIndexDrift> {
    let file = declared.file;
    let mut variants = Vec::new();
    let mut variant_defs = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut refusal = None;
    for member in &decl.members {
        if member.category {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    member.span,
                    "a `category` enum member (hierarchical enums are deferred)",
                ),
            );
            continue;
        }
        if !member.members.is_empty() {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                unsupported(
                    file,
                    member.span,
                    "a nested enum member (hierarchical enums are deferred)",
                ),
            );
            continue;
        }
        if seen.contains(&member.name) {
            refuse_first(
                &mut refusal,
                diagnostics,
                declared,
                SourceDiagnostic::at(
                    Code::CheckNameConflict.as_str(),
                    file,
                    member.name_span,
                    format!("enum member `{}` is already declared", member.name),
                ),
            );
            continue;
        }
        seen.push(member.name.clone());
        let Some((payload, payload_scalars)) =
            enum_payload(registry, declared, member, diagnostics, &mut refusal)?
        else {
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
    Ok(match refusal {
        Some(refusal) => DeclarationOccurrence::Refused(refusal),
        None => DeclarationOccurrence::Accepted((variants, variant_defs)),
    })
}

/// Resolve one member's payload fields to their scalars and info, or `None` when
/// a field is not the bare `name: scalar` form. A defect refuses the whole
/// declaration, so it is recorded in the enum's shared refusal rather than
/// returned separately.
fn enum_payload(
    registry: &TypeRegistry,
    declared: DeclarationSite<'_>,
    member: &EnumMember,
    diagnostics: &mut DiagnosticCollector,
    refusal: &mut Option<DeclarationRefusalSummary>,
) -> Result<Option<EnumPayload>, DeclarationIndexDrift> {
    let file = declared.file;
    if member.payload.len() > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
        refuse_first(
            refusal,
            diagnostics,
            declared,
            SourceDiagnostic::at(
                Code::CheckResourceLimit.as_str(),
                file,
                member.span,
                format!(
                    "an enum member carries {} payload fields; the fixed limit is {}",
                    member.payload.len(),
                    marrow_image::bounds::MAX_PAYLOAD_FIELDS
                ),
            ),
        );
        return Ok(None);
    }
    let mut payload = Vec::new();
    let mut scalars = Vec::new();
    let mut ok = true;
    for field in &member.payload {
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            refuse_first(
                refusal,
                diagnostics,
                declared,
                unsupported(file, field.ty.span(), "an optional enum payload field type"),
            );
            ok = false;
            continue;
        }
        let Some(scalar) = scalar_of(&registry.expand(&field.ty)) else {
            // A payload naming a declaration this project refused is steered to
            // that cause; only a genuinely unknown or unadmitted spelling is
            // described as an unsupported payload type.
            let row =
                registry.unresolved_member_row(&field.ty, file, "this enum payload field type")?;
            refuse_first(refusal, diagnostics, declared, row);
            ok = false;
            continue;
        };
        payload.push(EnumPayloadInfo {
            name: field.name.clone(),
            scalar,
        });
        scalars.push(scalar);
    }
    Ok(ok.then_some((payload, scalars)))
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
    resources: &'a [(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<Vec<(FileRef, FileIdentity, &'a ResourceDecl)>, DeclareError> {
    let mut survivors = Vec::new();
    for (at, file, resource) in resources {
        let declared = DeclarationSite {
            name: &resource.name,
            file,
            at: *at,
            span: resource.name_span,
        };
        if is_reserved_type_name(&resource.name) {
            let refusal = refuse_row(
                diagnostics,
                declared,
                reserved_name(file, resource.name_span, &resource.name),
            );
            registry.named.declare(
                resource.name.clone(),
                DeclarationOccurrence::Refused(refusal),
            )?;
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
        registry.named.declare(
            resource.name.clone(),
            DeclarationOccurrence::Accepted(NamedTypeKind::Resource),
        )?;
        survivors.push((*at, file.clone(), *resource));
    }
    Ok(survivors)
}

/// Pass two for the record types: fill each reserved record from its surviving
/// declaration, in the reserved order.
fn fill_records(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    record_decls: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    // The survivors are in the same order as the reserved records, so record `index`
    // is the one this declaration reserved.
    for (index, (at, file, resource)) in record_decls.iter().enumerate() {
        let declared = DeclarationSite {
            name: &resource.name,
            file,
            at: *at,
            span: resource.name_span,
        };
        fill_record(draft, registry, index, declared, resource, diagnostics)?;
    }
    Ok(())
}

/// Fill one reserved record (`registry.records[index]`) from its resource
/// declaration: declare each member into the registry's member ledger and fill both
/// the registry info and the image record from what the ledger accepted. A resource
/// field is a scalar, nominal scalar, dense struct, or closed enum value
/// (`Option`/`Result`/a user `enum`). A collection, keyed field, or unknown spelling
/// is not admitted; an unkeyed group is materialized separately below.
///
/// A refused member is `check.unsupported` at its own span and only that member
/// leaves the accepted set — the record keeps its other members. The refusal stays
/// in the ledger, so a later use of that member is steered to the cause rather than
/// told the record has no such field.
fn fill_record(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    index: usize,
    declared: DeclarationSite<'_>,
    resource: &ResourceDecl,
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), BuildError> {
    let file = declared.file;
    let type_id = registry.records[index].type_id;
    let mut groups = Vec::new();
    let mut group_slot_defs = Vec::new();
    for member in &resource.members {
        match member {
            ResourceMember::Field(field) => {
                let at = DeclarationSite {
                    name: &field.name,
                    file,
                    at: declared.at,
                    span: field.span,
                };
                let occurrence = if registry
                    .members
                    .declared(&MemberKey::field(&resource.name, &field.name))
                {
                    // Two members of one name have no unambiguous slot in the
                    // record; the first declaration stands and the repeat is a
                    // precise rejection rather than a silently dropped member.
                    DeclarationOccurrence::Refused(refuse_row(
                        diagnostics,
                        at,
                        member_conflict(file, field.span, &resource.name, &field.name),
                    ))
                } else if field.keys.is_empty() {
                    resource_member(draft, registry, at, field, "this field type", diagnostics)?
                } else {
                    // A keyed scalar leaf (`tags(pos: int): string`) is a keyed
                    // positional layer, not yet part of the beta durable graph. It is
                    // refused so the shape is a precise rejection, not a silent drop.
                    DeclarationOccurrence::Refused(refuse_row(
                        diagnostics,
                        at,
                        unsupported(file, field.span, "a keyed field"),
                    ))
                };
                registry
                    .members
                    .declare(MemberKey::field(&resource.name, &field.name), occurrence)?;
            }
            ResourceMember::Group(group) if group.keys.is_empty() => {
                // An unkeyed `group` is a nested sub-record value: its scalar/enum
                // leaves become a group record type, and the containing value gains one
                // required slot holding that record. Its durable identity is owned
                // separately by `durable.rs`; this is the materialized-value side only.
                let (leaf_fields, leaf_defs) = build_group_leaves(
                    draft,
                    registry,
                    &resource.name,
                    group,
                    declared,
                    diagnostics,
                )?;
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
    // The ledger is the authority for which members survived and in what order, so
    // the record's fields and the image slots are read out of it rather than
    // accumulated beside it.
    let fields = registry.accepted_members(&resource.name);
    let mut field_defs: Vec<FieldDef> = fields
        .iter()
        .map(|field| FieldDef {
            name: draft.intern_string(&field.name),
            ty: field.ty.image(),
            required: field.required,
        })
        .collect();
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

/// The row rejecting a second member of one name in `owner`, which has no
/// unambiguous slot in the record the owner materializes.
fn member_conflict(
    file: &FileIdentity,
    span: SourceSpan,
    owner: &str,
    member: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict.as_str(),
        file,
        span,
        format!("`{owner}` already declares a member `{member}`"),
    )
}

/// Resolve one resource member's declared type to the value it binds, or to the
/// refusal the member ledger retains.
///
/// A resource member is a value drawn from the closed acyclic durable value set: a
/// scalar, a nominal scalar, a dense struct, or a closed enum (`Option`/`Result`/a
/// user `enum`). A collection is not a durable member value; an abstract parameter
/// never reaches a concrete record.
fn resource_member(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    at: DeclarationSite<'_>,
    field: &FieldDecl,
    subject: &str,
    diagnostics: &mut DiagnosticCollector,
) -> Result<DeclarationOccurrence<FieldInfo>, GenericInvariant> {
    let file = at.file;
    Ok(
        match registry.resolve_garg(
            draft,
            &field.ty,
            MintSite {
                file,
                span: field.ty.span(),
            },
        ) {
            Ok(ty @ (GArg::Scalar(_) | GArg::Nominal(_) | GArg::Struct(_) | GArg::Enum(_))) => {
                DeclarationOccurrence::Accepted(FieldInfo {
                    name: field.name.clone(),
                    ty,
                    required: field.required,
                })
            }
            // A member type that resolves but is outside the durable value set is a
            // genuine subset gap; one that names a refused declaration is steered to
            // that declaration's own cause.
            Ok(_) => DeclarationOccurrence::Refused(refuse_row(
                diagnostics,
                at,
                unsupported(file, field.ty.span(), subject),
            )),
            Err(ResolveError::Refusal(refused)) => {
                match registry.member_refusal_row(refused, file, field.ty.span(), subject)? {
                    Some(row) => DeclarationOccurrence::Refused(refuse_row(diagnostics, at, row)),
                    // The shared instantiation limit reports once, at the
                    // monomorphization owner; this member is refused for a cause that
                    // pass owns.
                    None => DeclarationOccurrence::Refused(refuse_covered(
                        at,
                        Code::CheckInstantiationLimit.as_str(),
                    )),
                }
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant),
        },
    )
}

/// The direct scalar/enum leaves of an unkeyed group, in declaration order,
/// returning both the registry field infos and the image field defs. A keyed leaf,
/// a nested group or keyed branch inside the group, or a non-value leaf type is a
/// precise `check.unsupported` that refuses only that leaf. Nested groups and
/// group-scoped branches are deferred; refusing them keeps them from silently
/// dropping, and keeps the leaf name answerable at its uses.
fn build_group_leaves(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    record: &str,
    group: &GroupDecl,
    declared: DeclarationSite<'_>,
    diagnostics: &mut DiagnosticCollector,
) -> Result<(Vec<FieldInfo>, Vec<FieldDef>), BuildError> {
    let file = declared.file;
    let anchor = format!("{record}.{}", group.name);
    for member in &group.members {
        let field = match member {
            ResourceMember::Field(field) => field,
            ResourceMember::Group(inner) => {
                let at = DeclarationSite {
                    name: &inner.name,
                    file,
                    at: declared.at,
                    span: inner.span,
                };
                let key = MemberKey::leaf(record, &group.name, &inner.name);
                // A member occupies its name whether or not it was accepted, so a
                // repeat here is a name conflict exactly as it is at a leaf below.
                // The nested group is refused either way; the repeat is the thing
                // the reader has to fix first.
                let row = if registry.members.declared(&key) {
                    member_conflict(file, inner.span, &anchor, &inner.name)
                } else {
                    let what = if inner.keys.is_empty() {
                        "a nested group"
                    } else {
                        "a keyed branch inside a group"
                    };
                    unsupported(file, inner.span, what)
                };
                let refusal = refuse_row(diagnostics, at, row);
                registry
                    .members
                    .declare(key, DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
        };
        let at = DeclarationSite {
            name: &field.name,
            file,
            at: declared.at,
            span: field.span,
        };
        let occurrence =
            if registry
                .members
                .declared(&MemberKey::leaf(record, &group.name, &field.name))
            {
                DeclarationOccurrence::Refused(refuse_row(
                    diagnostics,
                    at,
                    member_conflict(file, field.span, &anchor, &field.name),
                ))
            } else if field.keys.is_empty() {
                resource_member(
                    draft,
                    registry,
                    at,
                    field,
                    "this group field type",
                    diagnostics,
                )?
            } else {
                DeclarationOccurrence::Refused(refuse_row(
                    diagnostics,
                    at,
                    unsupported(file, field.span, "a keyed field"),
                ))
            };
        registry.members.declare(
            MemberKey::leaf(record, &group.name, &field.name),
            occurrence,
        )?;
    }
    let fields = registry.accepted_members(&anchor);
    let field_defs = fields
        .iter()
        .map(|leaf| FieldDef {
            name: draft.intern_string(&leaf.name),
            ty: leaf.ty.image(),
            required: leaf.required,
        })
        .collect();
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
    structs: &[(FileRef, FileIdentity, &StructDecl)],
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), GenericInvariant> {
    let view = registry.metadata_view();
    let mut metadata = MetadataScratch::try_new(&view)?;
    let graph = ValueGraph::build_validated(registry, &view, &mut metadata)?;
    for info in &registry.structs {
        // A refused struct has an empty body and so lies on no cycle, but it is also
        // not a declaration this pass speaks for: its own cause was already reported
        // at its declaration.
        if !info.verdict.is_accepted() {
            continue;
        }
        if let Some(path) = graph.cycle_through(ValueNode::Record(info.type_id)) {
            #[expect(
                clippy::expect_used,
                reason = "lowering bookkeeping: every registered struct was reserved from this declaration list, so its declaration survives to be found"
            )]
            let (file, span) = structs
                .iter()
                .find(|(_, _, decl)| decl.name == info.name)
                .map(|(_, file, decl)| (file.clone(), decl.name_span))
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
                .find(|(_, _, decl)| decl.name == record.name)
                .map(|(_, file, decl)| (file.clone(), decl.name_span))
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
            // A reserved toolchain generic (`Option`, `Result`) is payloaded by a
            // type parameter and never defines a value cycle itself: any cycle it
            // sits on closes through a user type (`struct A { me: Option<A> }`
            // cycles through `A`), which is reported at its own real declaration by
            // the struct/resource loops above or by a user-template instance. Such a
            // reserved instance carries no source file, so it is skipped here rather
            // than attributed to an empty file.
            let Some(file) = &template.file else {
                continue;
            };
            diagnostics.push(value_cycle_diagnostic(
                file,
                template.name_span,
                &template.name,
                &path,
            ));
        }
    }
    Ok(())
}

fn value_cycle_diagnostic(
    file: &FileIdentity,
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
                    // Profiles cannot disagree: `leave_row` takes the frame's own row,
                    // not the popped one, so nothing here reads what this compares. The
                    // pop keeps `entered` in step for the unwind path below.
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(DisplayNode::Row(row)));
                    display.leave_row(row);
                }
                BestEffortDisplayFrame::LeaveCollection(index) => {
                    // Unread on the same terms as the row arm above.
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
                    // Profiles cannot disagree: `leave` takes the frame's own node, so
                    // nothing here reads what this compares; the pop keeps `entered` in
                    // step for the unwind path below.
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
                    // Unread on the same terms as the validated-display walker above.
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
fn reserved_name(file: &FileIdentity, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict.as_str(),
        file,
        span,
        format!("`{name}` is a built-in generic type and cannot be redeclared"),
    )
}

fn unsupported(file: &FileIdentity, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}

#[cfg(test)]
#[path = "types/types_metadata_successor_tests.rs"]
mod types_metadata_successor_tests;

#[cfg(test)]
#[path = "types/generic_scaling_counts_tests.rs"]
mod generic_scaling_counts_tests;

#[cfg(test)]
#[path = "types/alias_cycle_scaling_tests.rs"]
mod alias_cycle_scaling_tests;

#[cfg(test)]
#[path = "types/refusal_join_tests.rs"]
mod refusal_join_tests;

#[cfg(test)]
#[path = "types/instantiation_state_tests.rs"]
mod instantiation_state_tests;
