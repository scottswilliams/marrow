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
    CollTypeId, CollectionTypeDef, DraftTxn, EnumId, FieldDef, ImageType, RecordTypeDef, Scalar,
    TypeId, VariantDef,
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

mod build;
mod metadata;
mod owner_txn;
mod render;

use build::{
    build_alias_table, build_nominals, declare_enums, declare_records, declare_structs, fill_enums,
    fill_records, fill_structs, register_type_templates, reserved_templates,
    validate_alias_targets,
};
use metadata::{collection_generic_target, place_generic_row};
use owner_txn::ProofIsolation;
pub(crate) use owner_txn::{GenericOwnerTxn, RegistryInverse};
#[cfg(test)]
use render::garg_anchor_spelling;
use render::{
    collection_spelling_for_display, garg_spelling_validated, inst_spelling_for_display,
    render_validated_anchor_arg, render_validated_display_arg,
};

/// The identity of a nominal type in [`TypeRegistry`] order, carried by the
/// lowered type so classification never re-reads the source spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NominalId(pub(crate) u32);

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
    Collection(CollTypeId),
    /// An abstract generic type parameter by its declaration index, present only
    /// during the once-checked template pass of a generic function. A monomorphized
    /// instantiation carries no `Param`: every parameter is substituted by its
    /// concrete argument first. `image()` returns a sentinel that only ever reaches
    /// the throwaway draft the template pass discards.
    Param(TypeParamIndex),
}

/// The declaration position of one generic type parameter: a wide checked ordinal
/// over a private `u32`, never a wire value — a `Param` exists only during the
/// once-checked template pass, and no monomorphized instantiation carries one.
///
/// The domain is proven by the admitted source envelope: a declared parameter costs
/// at least two source bytes, and the capture ceiling admits at most 64 MiB of
/// source (`CaptureLimits::DEFAULT`), so a declaration position is bounded well
/// under 2^25 and the `u32` carrier cannot be exceeded by any admissible input.
/// Narrowing this carrier back to `u16` is what silently aliased parameter 65,536
/// onto parameter 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TypeParamIndex(u32);

impl TypeParamIndex {
    /// Mint from a declaration position (see the type's domain proof).
    pub(crate) fn from_position(position: usize) -> Self {
        #[expect(
            clippy::expect_used,
            reason = "domain proof: a declared parameter costs at least two source bytes and the \
                      64 MiB capture ceiling bounds every position far inside u32"
        )]
        Self(u32::try_from(position).expect("a type-parameter position fits the proved u32 domain"))
    }

    /// The declaration position, for environment lookups.
    pub(crate) fn position(self) -> usize {
        self.0 as usize
    }
}

/// Renders the declaration position, so diagnostic and hover spellings read exactly
/// as the narrow carrier's did.
impl std::fmt::Display for TypeParamIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
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
                idx: ty,
                optional: false,
            },
            GArg::Enum(id) => ImageType::Enum {
                idx: id,
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
            DeclareError::BuilderDomain(refusal) => {
                Self::Invariant(GenericInvariant::BuilderDomain(refusal))
            }
        }
    }
}

/// A draft mint refused at the builder surface's carrier domain is a compiler
/// coherence failure everywhere in the production compiler: the admitted source
/// envelope cannot reach the `u32` carrier boundary.
impl From<marrow_image::DraftStateError> for GenericInvariant {
    fn from(refusal: marrow_image::DraftStateError) -> Self {
        Self::BuilderDomain(refusal)
    }
}

impl From<marrow_image::DraftStateError> for BuildError {
    fn from(refusal: marrow_image::DraftStateError) -> Self {
        Self::Invariant(GenericInvariant::BuilderDomain(refusal))
    }
}

/// A generic-resolution failure is either a source-semantic refusal or a compiler
/// coherence failure. Only the refusal arm may enter a rejected cache row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveError {
    Refusal(ResolveRefusal),
    Invariant(GenericInvariant),
}

impl From<marrow_image::DraftStateError> for ResolveError {
    fn from(refusal: marrow_image::DraftStateError) -> Self {
        Self::Invariant(GenericInvariant::BuilderDomain(refusal))
    }
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
    TypeArgumentParameter(TypeParamIndex),
    /// A checked value-shape append refused at the image builder surface. The
    /// compiler's own width pre-guards and in-draft leaf minting make the refusal
    /// unreachable, so an occurrence is a compiler coherence failure, never a
    /// source refusal.
    BuilderDomain(marrow_image::DraftStateError),
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
    Record(u32),
    Enum(u32),
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
    collection_index: RefCell<HashMap<CollSpec, CollTypeId>>,
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
    // drop-path audit sentinel: end of RowDirectory::rewind_to

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
        collection_parent: Option<CollTypeId>,
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
    Collection(CollTypeId),
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

    fn enter_collection(&mut self, index: CollTypeId) -> bool {
        let Some(active) = self.active_collections.get_mut(index.index() as usize) else {
            return false;
        };
        std::mem::replace(active, 1) == 0
    }

    fn leave_collection(&mut self, index: CollTypeId) {
        let active = &mut self.active_collections[index.index() as usize];
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
#[path = "test_probes.rs"]
mod test_probes;
#[cfg(test)]
pub(crate) use test_probes::*;

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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
        ty: &TypeExpr,
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        self.resolve_garg_expanded(draft, &self.expand(ty), subst, site)
    }

    fn resolve_garg_expanded(
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
        template: usize,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<TypeInstId, ResolveError> {
        self.mint_type_instance_with_requirement(draft, template, args, site, AnyReadyInstance)
    }

    #[inline(never)]
    fn mint_type_instance_with_requirement<R: ReadyInstanceRequirement>(
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        let name_id = draft.intern_string(&template_info.name)?;
        let id = if template_info.is_enum() {
            let enum_id = draft.reserve_enum_type(name_id)?;
            TypeInstId::Enum(enum_id)
        } else {
            let type_id = draft.reserve_record_type(name_id)?;
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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
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
        &mut self,
        draft: &mut DraftTxn<'_>,
        template: usize,
        id: TypeInstId,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<InstBody, ResolveError> {
        let (subst, fields) = {
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
            // The declaration list is copied out rather than held: resolving a field
            // mints through the exclusively held registry, and no read of a template may
            // stay live across that. The copy is exact — `type_templates` is fixed after
            // build — and costs the same order per instantiation as the resolved and
            // definition vectors this fill already builds from it.
            //
            // Resource bound: one copy per instantiation of the template's declared
            // fields, so the whole cost is at most `MAX_INSTANTIATIONS` copies of the
            // widest admissible body. That is measured, not asserted: the issuance RSS
            // gate's type-amplification corpus is a maximum-width generic struct driven
            // to the instantiation ceiling, which is exactly this term, and its peak sits
            // an order of magnitude under the declared owned-heap ceiling.
            (subst, fields.clone())
        };
        let mut resolved = Vec::with_capacity(fields.len());
        let mut defs = Vec::with_capacity(fields.len());
        for (fname, fty) in &fields {
            let arg = self.resolve_garg_env(draft, fty, &subst, site)?;
            defs.push(FieldDef {
                name: draft.intern_string(fname)?,
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
        #[expect(
            clippy::expect_used,
            reason = "reserve-then-fill law: the row was reserved in this batch and fills exactly once"
        )]
        draft
            .set_record_fields(ty, defs)
            .expect("a reserved row fills once");
        Ok(InstBody::Struct(resolved))
    }

    fn fill_enum_type_body(
        &mut self,
        draft: &mut DraftTxn<'_>,
        template: usize,
        id: TypeInstId,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<InstBody, ResolveError> {
        let (subst, variants, enum_name) = {
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
            // Copied out for the same reason as a struct fill: a payload resolution
            // mints through the exclusively held registry, so no template read may stay
            // live across it. Bounded and measured exactly as the struct copy above.
            (subst, variants.clone(), template_info.name.clone())
        };
        let enum_name = enum_name.as_str();
        let mut reported = false;
        let mut resolved = Vec::with_capacity(variants.len());
        let mut defs = Vec::with_capacity(variants.len());
        for variant in &variants {
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
                name: draft.intern_string(&variant.name)?,
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
        #[expect(
            clippy::expect_used,
            reason = "reserve-then-fill law: the row was reserved in this batch and fills exactly once"
        )]
        draft
            .set_enum_variants(enum_id, defs)
            .expect("a reserved row fills once");
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
        coll: CollTypeId,
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
    pub(crate) fn set_fn_base(&mut self, base: u16) {
        self.generics.get_mut().fn_base = base;
    }

    /// Reserve the image function index for `(fn template, args)`, minting and
    /// enqueuing a fresh instance on first request and reusing it thereafter. A shared
    /// bound refusal records the first coherent mint site and returns `Err(Limit)`.
    pub(crate) fn reserve_fn_instance(
        &mut self,
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
    ///
    /// This *reads* the front entry and leaves the queue alone. Removing it is
    /// [`Self::consume_fn_pending`], which the drain driver calls only once the batch that
    /// lowered the entry has settled. The split is what makes the queue invertible: an
    /// inverse that captures a length can undo the batch's appends, but it cannot put
    /// back a front entry the driver removed before the batch was even admitted, and
    /// reinstating one would mean an allocating call on the restore path.
    pub(crate) fn peek_fn_pending(&self) -> Option<(usize, Vec<GArg>, u16)> {
        self.generics
            .borrow()
            .fn_queue
            .front()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
    }

    /// Remove the entry [`Self::peek_fn_pending`] reported, after its batch settled.
    ///
    /// A batch only ever appends to the back, so the front entry after settlement is
    /// still the one that was lowered.
    pub(crate) fn consume_fn_pending(&mut self) {
        self.generics.get_mut().fn_queue.pop_front();
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
        &mut self,
        draft: &mut DraftTxn<'_>,
        elem: GArg,
    ) -> Result<CollTypeId, ResolveError> {
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
        &mut self,
        draft: &mut DraftTxn<'_>,
        key: GArg,
        value: GArg,
    ) -> Result<CollTypeId, ResolveError> {
        self.check_map_key_admissibility(key)?;
        self.instantiate_collection(draft, CollSpec::Map { key, value })
    }

    fn instantiate_collection(
        &mut self,
        draft: &mut DraftTxn<'_>,
        spec: CollSpec,
    ) -> Result<CollTypeId, ResolveError> {
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
            if collections.get(index.index() as usize) != Some(&spec) {
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
        let id = draft.add_collection_type(spec.definition())?;
        debug_assert_eq!(id.index() as usize, cache_index);
        let mut collections = self.collections.borrow_mut();
        debug_assert_eq!(collections.len(), cache_index);
        collections.push(spec);
        self.collection_index.borrow_mut().insert(spec, id);
        Ok(id)
    }

    /// The source element/key/value spec of a minted collection instantiation.
    pub(crate) fn collection_spec(&self, idx: CollTypeId) -> CollSpec {
        self.collections.borrow()[idx.index() as usize]
    }

    /// The source spelling of a collection instantiation (`List<T>` / `Map<K, V>`),
    /// used in diagnostics and cycle labels. The canonical angle-form display owner.
    pub(crate) fn collection_spelling(&self, idx: CollTypeId) -> String {
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
            .map(|index| (NominalId(index as u32), &self.nominals[index]))
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
        draft: &mut DraftTxn<'_>,
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
    /// [`Self::restore_generic_owners`] truncates the appended rows and re-seats the swapped
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
    ) -> Result<RegistryInverse, GenericInvariant> {
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
        let savepoint = RegistryInverse {
            type_insts: generics.type_insts.len(),
            collections,
            fn_insts: generics.fn_insts.len(),
            fn_queue: generics.fn_queue.len(),
            fn_base: generics.fn_base,
            build_invariant: generics.build_invariant,
            prior_argument_domain: generics.argument_domain,
            entry_records,
            entry_enums,
            row_directory_present: self.row_directory.borrow().is_some(),
            isolation: Some(ProofIsolation {
                // Whole-owner swap: the proof pass gets a fresh live collector and
                // the prior owner is saved intact for exit to re-seat.
                prior_payloads: std::mem::replace(
                    &mut generics.collection_payloads,
                    DiagnosticCollector::new(),
                ),
            }),
        };
        generics.argument_domain = ArgumentDomain::TemplateProof;
        Ok(savepoint)
    }

    /// Admit an ordinary generic-owner batch and capture its inverse.
    ///
    /// Admission proves the registry is between fills — no open fill batch, no active
    /// row cache, no fill stack, no unsettled failure list. That is what makes the
    /// captured lengths a complete description of the batch: every row the batch can
    /// append or fill lies at or above them, and no settled prefix row can gain a
    /// dependency edge while the batch runs. Unlike a template proof this takes no
    /// isolating swap — an ordinary batch shares the live instantiation-limit owner and
    /// ordered diagnostic buffer, whose custody is the diagnostic substrate's.
    ///
    /// `entry_records`/`entry_enums` are the draft's record/enum id ceilings at
    /// admission, which the reused metadata directory rolls back to.
    pub(crate) fn admit_generic_owners(
        &self,
        entry_records: usize,
        entry_enums: usize,
    ) -> Result<RegistryInverse, GenericInvariant> {
        let generics = self
            .generics
            .try_borrow()
            .map_err(|_| GenericInvariant::ProofClone(ProofCloneError::UnstableFillState))?;
        if generics.fill_batch_start.is_some()
            || !generics.fill_rows.is_empty()
            || !generics.fill_stack.is_empty()
            || !generics.fill_failures.is_empty()
        {
            return Err(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState,
            ));
        }
        // Read before any mutation, so contention on the collection owner refuses with
        // the registry untouched.
        let collections = self
            .collections
            .try_borrow()
            .map_err(|_| GenericInvariant::ProofClone(ProofCloneError::UnstableFillState))?
            .len();
        // Destructured exhaustively: a new generic owner stops this compiling until it is
        // captured here or deliberately excluded beside the two the inverse names in
        // `UNRESTORED_DIAGNOSTIC_OWNERS`. The fill owners are bound to `_` because
        // admission above has just proved every one of them empty, which is what makes
        // the captured lengths a complete description of the batch.
        let Monomorph {
            type_insts,
            type_index: _,
            fn_base,
            fn_insts,
            fn_index: _,
            fn_queue,
            fill_batch_start: _,
            fill_rows: _,
            fill_stack: _,
            fill_failures: _,
            limit: _,
            collection_payloads: _,
            build_invariant,
            argument_domain,
        } = &*generics;
        Ok(RegistryInverse {
            type_insts: type_insts.len(),
            collections,
            fn_insts: fn_insts.len(),
            fn_queue: fn_queue.len(),
            fn_base: *fn_base,
            build_invariant: *build_invariant,
            prior_argument_domain: *argument_domain,
            entry_records,
            entry_enums,
            row_directory_present: self.row_directory.borrow().is_some(),
            isolation: None,
        })
    }

    /// Restore the registry to the exact state captured by `savepoint`, erasing every effect
    /// of the proof pass. Appended type instantiations and collections are truncated and
    /// their lockstep secondary-index keys removed (a purge proportional to the appended
    /// rows, never the settled population); the transient fill state — empty around a settled
    /// batch, but possibly dirty after a proof that failed mid-fill — is reset; and the
    /// argument domain, ordered-diagnostic buffer, and instantiation-limit owner are
    /// re-seated. The reused metadata directory is rolled back to the pre-proof image.
    pub(crate) fn restore_generic_owners(&mut self, inverse: RegistryInverse) {
        let RegistryInverse {
            type_insts,
            collections,
            fn_insts,
            fn_queue,
            fn_base,
            build_invariant,
            prior_argument_domain,
            entry_records,
            entry_enums,
            row_directory_present,
            isolation,
        } = inverse;
        {
            let generics = self.generics.get_mut();
            while generics.type_insts.len() > type_insts {
                if let Some(inst) = generics.type_insts.pop() {
                    generics.type_index.remove(&(inst.template, inst.args));
                }
            }
            while generics.fn_insts.len() > fn_insts {
                if let Some(inst) = generics.fn_insts.pop() {
                    generics.fn_index.remove(&(inst.template, inst.args));
                }
            }
            generics.fn_queue.truncate(fn_queue);
            generics.fn_base = fn_base;
            generics.fill_batch_start = None;
            generics.fill_rows.clear();
            generics.fill_stack.clear();
            generics.fill_failures.clear();
            generics.build_invariant = build_invariant;
            generics.argument_domain = prior_argument_domain;
            if let Some(ProofIsolation { prior_payloads }) = isolation {
                // Only an isolated proof re-seats these: its swapped-in owners are
                // throwaway, so the limit returns to the open state admission proved
                // and the live payload owner is put back whole.
                generics.limit = LimitState::Open;
                generics.collection_payloads = prior_payloads;
            }
        }
        {
            let colls = self.collections.get_mut();
            let index = self.collection_index.get_mut();
            while colls.len() > collections {
                if let Some(spec) = colls.pop() {
                    index.remove(&spec);
                }
            }
        }
        if row_directory_present {
            if let Some(directory) = self.row_directory.get_mut().as_mut() {
                directory.rewind_to(entry_records, entry_enums, type_insts, collections);
            }
        } else {
            // The batch opened the first directory. Rewinding it to the captured ceilings
            // would leave a directory the registry did not have; taking it is the inverse.
            *self.row_directory.get_mut() = None;
        }
    }
    // drop-path audit sentinel: end of TypeRegistry::restore_generic_owners
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
mod types_metadata_successor_tests;

#[cfg(test)]
mod generic_scaling_counts_tests;

#[cfg(test)]
mod alias_cycle_scaling_tests;

#[cfg(test)]
mod refusal_join_tests;

#[cfg(test)]
mod instantiation_state_tests;
