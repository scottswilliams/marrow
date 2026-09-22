//! The project named-type registry: transparent aliases and record types.
//!
//! This is the single owner of what a source type name denotes. A transparent
//! `alias Name = Type` shares one globally bound terminal and optionality and mints no
//! identity or constructor. A nominal `type Name: int in lo..hi` mints a distinct type
//! whose identity — name, inclusive interval, and `supports` capability set — lives
//! here, while the image records only its base scalar: the interval is carried by the
//! guard instructions the compiler emits, not by an image type table.
//!
//! Two product kinds lower into image [`RecordTypeDef`]s, the single canonical
//! product-leaf order owner: `resource` types and dense `struct` value types. Keyed
//! resource children belong to the durable graph rather than this record. Value types
//! are built declare-then-fill so a field may name any other value type regardless of
//! order; the sole nesting restriction is acyclicity.

use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::Hash;
use std::rc::Rc;

use crate::source::{CapturedOrigins, ProjectFile, ScopedName};
use marrow_codes::Code;
use marrow_image::{
    CollTypeId, CollectionTypeDef, DraftTxn, EnumId, FieldDef, ImageType, RecordTypeDef, Scalar,
    TypeId, VariantDef,
};
use marrow_project::SourceOrigin;
use marrow_syntax::{
    AliasDecl, EnumDecl, EnumMember, Expression, FieldDecl, GroupDecl, LiteralKind, NominalDecl,
    ResourceDecl, ResourceMember, SourceSpan, StructDecl, TypeExpr, UnaryOp, range_expr,
};

use crate::analysis::FileRef;
use crate::decl::{
    Binding, DeclarationBudget, DeclarationIndexDrift, DeclarationLedger, DeclarationLedgerFull,
    DeclarationNamespace, DeclarationOccurrence, DeclarationRefusalId, DeclarationRefusalSummary,
    DeclarationSite, DeclareError, MemberNamespace, declaration_refused, placeholder_declared,
    refuse, refuse_covered, refuse_first, refuse_row,
};
use crate::diag::{BoundedDiagnostics, DiagnosticCollector, SourceDiagnostic, unsupported};
use crate::scalar::ScalarType;

mod aliases;
use aliases::{AliasInput, AliasTable};
pub(crate) use aliases::{AliasPresence, GlobalAliasTarget};
mod build;
mod decl_coords;
mod function_index;
mod metadata;
mod owner_txn;
mod render;

use build::{
    build_alias_table, build_nominals, declare_enums, declare_records, declare_structs,
    fill_records, fill_rows, register_type_templates, reserved_templates, validate_alias_targets,
};
use decl_coords::{AdmittedRecords, DeclarationCoordinates};
use metadata::{DeclaredCounts, RowDirectory, RowDirectoryGuard};
pub(crate) use owner_txn::GenericOwnerTxn;
use owner_txn::ProofIsolation;
use owner_txn::RegistryInverse;
use render::{ANCHOR, DISPLAY, render_validated_arg};

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

/// A boundary value keeps resource identity distinct from generic value arguments.
#[derive(Clone, Copy)]
pub(crate) enum NominalBoundaryValue {
    Value(GArg),
    Resource(TypeId),
}

pub(crate) enum NominalBoundaryKind {
    Input,
    Durable,
}

/// Resolved during binding/signature construction; retained only until the shared
/// containment walk, without copying source identities or type bodies.
pub(crate) struct NominalBoundaryRoot<'a> {
    pub(crate) value: NominalBoundaryValue,
    pub(crate) kind: NominalBoundaryKind,
    pub(crate) file: &'a ProjectFile,
    pub(crate) span: SourceSpan,
}

/// The declaration position of one generic type parameter: a wide checked ordinal
/// over a private `u32`, never a wire value — a `Param` exists only during the
/// once-checked template pass, and no monomorphized instantiation carries one.
///
/// The domain is proven by the admitted source envelope: a declared parameter costs
/// at least two source bytes, and the capture ceiling admits at most 64 MiB of
/// source (`CaptureLimits::DEFAULT`), so a declaration position is bounded well
/// under 2^25 and the `u32` carrier cannot be exceeded by any admissible input. A
/// narrower carrier would silently alias one parameter position onto another.
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

    pub(crate) fn position(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for TypeParamIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<TypeInstId> for GArg {
    fn from(id: TypeInstId) -> Self {
        match id {
            TypeInstId::Record(ty) => Self::Struct(ty),
            TypeInstId::Enum(id) => Self::Enum(id),
        }
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
    /// at every application of a constrained generic. The admission is the operator
    /// table's: every scalar, nominal int, and enum has `==`, and order holds for the
    /// scalars [`scalar_order`](crate::lower::scalar_order) orders and for a nominal
    /// int through its `int` row. A struct or collection supports neither. `Param`
    /// never reaches a concrete revalidation.
    pub(crate) fn satisfies(self, constraint: TypeConstraint) -> bool {
        match (self, constraint) {
            (GArg::Scalar(_) | GArg::Nominal(_) | GArg::Enum(_), TypeConstraint::Equality) => true,
            (GArg::Scalar(scalar), TypeConstraint::Order) => {
                crate::lower::scalar_order(marrow_syntax::BinaryOp::Less, scalar).is_some()
            }
            (GArg::Nominal(_), TypeConstraint::Order) => {
                GArg::Scalar(ScalarType::Int).satisfies(constraint)
            }
            _ => false,
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
/// and containment graph produces a finite set; this bound fails a divergent
/// monomorphization — a generic that recurses into itself over an ever-growing type —
/// with a typed `check.instantiation_limit` before the worklist allocates unboundedly.
pub(crate) const MAX_INSTANTIATIONS: usize = 4096;

/// The maximum nesting depth of generic type instantiation minting. A member of a
/// minted type may itself mint a type; this bound (at the parser's type-nesting
/// limit, so any finite source-shaped nesting fits) stops a divergent chain — a
/// generic type whose field grows the argument at every level — with
/// `check.instantiation_limit` rather than letting it mint until the count bound.
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
    /// order. A genuine absence dominates a refused declaration: a real gap must never
    /// be hidden behind a refused sibling's steer. Two refused declarations survive as
    /// one cause only when they are the same declaration; otherwise the merge would
    /// steer the reader to a cause the other part does not have.
    ///
    /// The collapse loses a steer, never a cause — every refused declaration is reported
    /// at its own declaration site.
    ///
    /// **Known limit.** A generic *argument list* folds through one join, so
    /// `Pair<Bad, AlsoMissing>` reports the first argument and says nothing about the
    /// second: the reader fixes one, recompiles, and meets the other.
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

/// Why a template-proof savepoint could not admit a coherent isolated pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateProofError {
    UnstableFillState,
    LimitOwnerNotOpen,
}

/// The generic instantiation cache disagreed with itself, tagged with the site that
/// observed it.
///
/// Every one of these is a compiler coherence failure, never a fact about the
/// source, and every one is redacted behind the opaque `CompileInvariant` before a
/// user sees it. The tag names the check that failed so a bug report and a test can
/// say which; it is not a stable code and nothing dispatches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GenericCacheInvariant(pub(crate) &'static str);

/// Detailed compiler-private causes that cross the build boundary only through the
/// redacted public `CompileInvariant` wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericInvariant {
    TemplateProof(TemplateProofError),
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
    /// The type registry admitted a resource the declaration set the durable build
    /// received does not answer for: the ordinal the record cites holds no declaration,
    /// the record carries no declaration coordinate, or the declaration at that ordinal
    /// sits at a different module position or name span than the declare pass recorded.
    /// The two inputs are joined exactly once, when the resource directory is taken, so
    /// this is produced at that single join and nowhere else. It is a compiler coherence
    /// failure, never a fact about the source: reporting it as a refusal would charge the
    /// user for the drift, and staying silent would bind the store to whatever sits at
    /// the cited ordinal — nothing, or another resource's declaration.
    DurableResourceMissing(marrow_image::TypeId),
    /// A store's executable derivation read a branch key tuple the directory had
    /// already refused — for its declared width, or for a column outside the
    /// durable-key scalar set. The graph build consumes either refusal as the
    /// branch's own diagnostic and a refused branch refuses its store, so a store
    /// that reaches the executable derivation proved every branch admitted — this arm
    /// is the compiler disagreeing with its own admission ordering, not the source.
    DurableBranchKeyUnresolved,
    /// A branch field no longer has the scalar recorded by its builder. Capture reads
    /// it before deciding whether the containing root is executable or parked.
    DurableBranchFieldUnresolved,
    /// Scalar annotation lookup cannot spend a generic instantiation budget.
    ScalarResolutionLimit,
    /// A declared value type on a containment cycle has no declaration coordinate.
    ///
    /// The declare pass mints the coordinate in the same statement sequence that
    /// pushes the registry row, so a miss is those two owners disagreeing about one
    /// declaration — a compiler coherence failure, never a fact about the source.
    /// Reporting it as a refusal would charge the user for that disagreement, and
    /// dropping the row would lose a real cycle report with no cause at all.
    DeclarationCoordinateMissing(TypeId),
    /// The same coherence failure for a declared `enum`.
    EnumCoordinateMissing(EnumId),
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

/// Drop one `(template, args)` key from a nested mint-dedup index, and the template's
/// bucket with it when that was its last key, so a rolled-back proof leaves the index
/// exactly as it found it.
fn remove_index_key(
    index: &mut HashMap<usize, HashMap<Vec<GArg>, usize>>,
    template: usize,
    args: &[GArg],
) {
    if let Some(rows) = index.get_mut(&template) {
        rows.remove(args);
        if rows.is_empty() {
            index.remove(&template);
        }
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
fn is_reserved_type_name(name: &str) -> bool {
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
    /// This type id owns no record the registry declared, so the question was asked of
    /// the wrong owner: the caller falls through to the durable branch-entry layout.
    /// Not a statement that the member is missing — no owner was found to ask.
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructFieldProjection {
    Field {
        index: u16,
        ty: GArg,
    },
    /// The struct owns no field of this name — a report about the source.
    Missing,
    /// This type id names no Ready struct body, so there was nothing to ask. A
    /// compiler coherence failure at the caller, not a missing field.
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
///
/// The entries are shared rather than owned outright. A template body is fixed once the
/// registry is built, and every fill of it must read the declared entries while minting
/// through the exclusively held registry — a borrow the registry's own `&mut` methods
/// cannot admit. A shared handle releases that borrow without copying the entries, so the
/// per-instantiation cost of reaching a body is a refcount, not a body.
#[derive(Clone)]
enum TemplateBody {
    Struct(Rc<[(String, TypeExpr)]>),
    Enum(Rc<[TemplateVariant]>),
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
    file: Option<ProjectFile>,
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

/// A Ready enum body's members as resolved variants, dropping the payload leaf names
/// a variant reader does not address.
pub(crate) fn ready_enum_variants(variants: &[InstVariant]) -> ResolvedEnumVariants {
    variants
        .iter()
        .map(|variant| {
            (
                variant.name.clone(),
                variant.payload.iter().map(|(_, arg)| *arg).collect(),
            )
        })
        .collect()
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

#[derive(Clone, Copy)]
pub(crate) struct EnumVariantSelection<'a> {
    pub(crate) index: usize,
    pub(crate) name: &'a str,
}

/// What a caller minting or reusing a generic type instantiation needs the settled
/// row to be.
#[derive(Clone, Copy)]
enum ReadyRequirement<'a> {
    /// Any settled body. Only this caller may reuse a row still filling: it asks
    /// for identity, not for a readable body.
    Any,
    Struct,
    Variant(EnumVariantSelection<'a>),
}

impl ReadyRequirement<'_> {
    fn allows_provisional(self) -> bool {
        matches!(self, Self::Any)
    }

    fn validate(self, inst: &TypeInst, body: &InstBody) -> Result<(), GenericInvariant> {
        let kind_mismatch = |body| GenericInvariant::TypeBodyKindMismatch { id: inst.id, body };
        match (self, body) {
            (Self::Any, _) | (Self::Struct, InstBody::Struct(_)) => Ok(()),
            (Self::Struct, InstBody::Enum(_)) => Err(kind_mismatch(TypeInstKind::Enum)),
            (Self::Variant(_), InstBody::Struct(_)) => Err(kind_mismatch(TypeInstKind::Struct)),
            (Self::Variant(selection), InstBody::Enum(variants)) => {
                if variants
                    .get(selection.index)
                    .is_some_and(|member| member.name == selection.name)
                {
                    return Ok(());
                }
                let TypeInstId::Enum(id) = inst.id else {
                    return Err(kind_mismatch(TypeInstKind::Enum));
                };
                Err(GenericInvariant::ReadyEnumVariantMissing {
                    id,
                    template: inst.template,
                    variant: selection.index,
                })
            }
        }
    }
}

/// One reserved instantiation whose body is not filled yet, carrying the nesting
/// depth of the row itself: one more than the depth of the fill that reserved it,
/// and the quantity [`MINT_DEPTH_LIMIT`] bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingFill {
    index: usize,
    depth: usize,
}

/// A sortable active-batch key for a generic type row. Image IDs are insertion
/// ordered within their own record/enum tables; the variant keeps those domains
/// disjoint without searching the stable cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TypeInstKey {
    Record(u32),
    Enum(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TypeInstSemanticKey<'a> {
    template: usize,
    args: &'a [GArg],
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

/// A reserved type row is visible to the rest of its fill batch before its body is
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
    func: marrow_image::FuncId,
}

/// The source anchor for a generic instantiation: the file and span a mint-time
/// diagnostic (an instantiation limit or a rejected payload) points at. Always a
/// real captured file — a mint is triggered by a use site, never by a fileless
/// synthetic construct.
#[derive(Clone, Copy)]
pub(crate) struct MintSite<'a> {
    pub(crate) file: &'a ProjectFile,
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

enum InstantiationLimit {
    Count,
    TypeDepth,
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
/// terminal limit row first, followed by the finished collection-payload terminal
/// gathered before it. A transfer is never reopened, only merged or adopted whole.
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
    /// does not carry the looked-up key is index/authority drift, reported as a typed
    /// coherence failure rather than silently trusted.
    ///
    /// Nested by template so the probe borrows the caller's argument slice: a flat
    /// tuple key would allocate a `Vec` on every dedup probe, including the misses.
    type_index: HashMap<usize, HashMap<Vec<GArg>, usize>>,
    fn_insts: Vec<FnInst>,
    /// Lookup-only secondary index `(template, args) -> row in fn_insts`, with the
    /// same authority discipline and drift detection as `type_index`. `fn_insts`
    /// stays the sole reservation-order authority; the reserved image function index
    /// is always read from the row, never from this index.
    fn_index: HashMap<usize, HashMap<Vec<GArg>, usize>>,
    fn_queue: VecDeque<FnInst>,
    /// The first row appended by the active outermost fill. Settlement
    /// touches only this contiguous suffix, never the stable prefix.
    fill_batch_start: Option<usize>,
    /// Direct image-id lookup for rows in that active suffix. Cleared atomically at
    /// settlement, so semantic dependency discovery never scans the stable cache.
    fill_rows: BTreeMap<TypeInstKey, usize>,
    /// The row whose body is being filled. At most one fill runs at a time — the
    /// `Option` is what makes that unrepresentable otherwise — because a member
    /// needing a nested instantiation queues it below instead of descending into it.
    filling: Option<PendingFill>,
    /// Reserved rows awaiting their bodies, in reservation order. One nesting level
    /// costs one entry; every entry names a distinct row of the active batch, so
    /// [`MAX_INSTANTIATIONS`] bounds the queue's length as well as the population.
    pending_fills: VecDeque<PendingFill>,
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
            fn_insts: Vec::new(),
            fn_index: HashMap::new(),
            fn_queue: VecDeque::new(),
            fill_batch_start: None,
            fill_rows: BTreeMap::new(),
            filling: None,
            pending_fills: VecDeque::new(),
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
}

/// One nominal type: a distinct int-based type whose every value lies in the
/// inclusive interval `[lo, hi]`.
#[derive(Clone)]
pub(crate) struct NominalInfo {
    pub(crate) origin: SourceOrigin,
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

/// One resource's record type. `type_id` is the group-inclusive materialized
/// record: its top-level scalar/enum field slots followed by one slot per unkeyed group
/// (a nested group sub-record). The verifier ties the field slots to the durable member
/// tree's fields and each trailing group slot to a `Group` member, so one record type
/// serves both the durable graph and the storeless value model.
#[derive(Clone)]
pub(crate) struct RecordInfo {
    pub(crate) type_id: TypeId,
    pub(crate) origin: SourceOrigin,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
    pub(crate) groups: Vec<GroupInfo>,
}

impl RecordInfo {
    /// This record's name in the tree that declared it: the member ledger's owner
    /// key, and the one place the pair is put back together.
    pub(crate) fn scoped_name(&self) -> ScopedName {
        ScopedName::new(&self.origin, &self.name)
    }

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
    pub(crate) origin: SourceOrigin,
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
    fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

/// One enum-variant payload leaf: a named bare value carried by that variant, in
/// declaration order. The name is used for named construction; the image records
/// the leaf's erased type.
#[derive(Clone)]
pub(crate) struct EnumPayloadInfo {
    pub(crate) name: String,
    pub(crate) ty: GArg,
}

/// One selectable enum variant: its member name and dense payload.
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
    pub(crate) origin: SourceOrigin,
    pub(crate) name: String,
    pub(crate) variants: Vec<VariantInfo>,
    pub(crate) verdict: DeclarationVerdict,
}

impl EnumInfo {
    /// This declared enum's members as resolved variants. A declared enum's payload
    /// leaves are resolved when the declaration fills, so no instantiation cache is
    /// consulted.
    pub(crate) fn resolved_variants(&self) -> ResolvedEnumVariants {
        self.variants
            .iter()
            .map(|variant| {
                (
                    variant.name.clone(),
                    variant.payload.iter().map(|field| field.ty).collect(),
                )
            })
            .collect()
    }

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
/// The owner is scoped to the tree that declared it: two trees may each declare a
/// resource of one name, and a bare-name key would merge their members into one
/// record and steer a refused member to the wrong declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberKey {
    owner: ScopedName,
    member: String,
}

/// Every member of one record shares that record's owner, so the member name is the
/// discriminating half of the key. Ordering on it first keeps a ledger probe from
/// comparing the same origin and record spelling at every step of its search — a
/// resource declaring thousands of fields is the shape this ledger is sized for.
impl Ord for MemberKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.member
            .cmp(&other.member)
            .then_with(|| self.owner.cmp(&other.owner))
    }
}

impl PartialOrd for MemberKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl MemberKey {
    /// A member of the resource record `owner`, or a leaf of one of its unkeyed
    /// groups when `owner` is that group's [anchor](ScopedName::below).
    pub(crate) fn new(owner: &ScopedName, member: &str) -> Self {
        Self {
            owner: owner.clone(),
            member: member.to_string(),
        }
    }

    fn owns(&self, owner: &ScopedName) -> bool {
        self.owner == *owner
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
    /// A builtin scalar spelling. No declaration binds one — the parser rejects a
    /// scalar keyword in name position — so this kind never enters the ledger; it
    /// exists so [`TypeRegistry::name_conflict`] is total over the namespace.
    Scalar,
}

impl NamedTypeKind {
    /// How a name-conflict report spells this kind, with its article.
    fn spelling(self) -> &'static str {
        match self {
            Self::Alias => "an alias",
            Self::Nominal => "a nominal type",
            Self::Struct => "a struct",
            Self::Enum => "an enum",
            Self::Resource => "a resource",
            Self::Template => "a generic type",
            Self::Scalar => "a builtin type",
        }
    }
}

/// What already holds a declared type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameHolder {
    /// An accepted declaration of this kind, or a builtin scalar spelling.
    Kind(NamedTypeKind),
    /// A declaration this build refused. It occupies its name from where it is
    /// written, and the ledger retains its cause rather than its kind.
    Refused,
}

impl NameHolder {
    /// How a name-conflict report spells the holder, with its article.
    pub(super) fn spelling(self) -> &'static str {
        match self {
            Self::Kind(kind) => kind.spelling(),
            Self::Refused => "a type",
        }
    }
}

/// The project named-type registry: the transparent aliases, the nominal int
/// types, the dense struct value types, and the durable-capable record types.
pub(crate) struct TypeRegistry {
    /// Every declared type name in one namespace, accepted or refused.
    ///
    /// The kind-specific tables below stay the authority for what an *accepted*
    /// name resolves to and for image order; this ledger is the authority for
    /// whether a name was declared at all. A refused declaration is dropped from its
    /// table — so no construction or match resolves against a broken type — and retained
    /// here, so the use that can no longer resolve is steered to the cause instead of
    /// being told the name was never written.
    named: DeclarationLedger<ScopedName, NamedTypeKind>,
    /// The trees this compilation captured, so a qualified annotation's first
    /// segment is resolved to the dependency that declares it.
    origins: CapturedOrigins,
    /// Every member declared by a resource record or one of its unkeyed groups,
    /// accepted or refused, in declaration order.
    ///
    /// This is the authority for which members survived and in what order:
    /// `RecordInfo::fields` and `GroupInfo::fields` are read out of `accepted()`,
    /// so the record cannot hold a member the ledger does not, and a member the
    /// compiler refused answers `Refused` at the lookups that would otherwise
    /// report the record as having no such field.
    members: DeclarationLedger<MemberKey, FieldInfo>,
    /// Supported aliases share globally bound terminal names and optionality.
    aliases: AliasTable,
    nominals: Vec<NominalInfo>,
    structs: Vec<StructInfo>,
    enums: Vec<EnumInfo>,
    /// The project's `resource` record types, in source order, each with the position
    /// of the declaration it was built from. Each is a value record type, and any
    /// number of `store` declarations may bind one. Names are unique (a duplicate is
    /// rejected at declare), so a name selects at most one.
    ///
    /// The ordinal travels with the record so the durable build reads the pairing
    /// settled at declaration time rather than re-deriving it from resource name
    /// strings. [`AdmittedRecords`] is what makes the two answer for one another.
    records: AdmittedRecords,
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
    /// Where each declared `struct` and `resource` was written.
    ///
    /// Owned here rather than beside the pass that reports at a declaration:
    /// declaration admission is one-shot, so this table lives and dies with the
    /// registry it is a field of, and a failed admission drops it whole. See
    /// [`decl_coords`] for the ownership argument and the condition under which
    /// these rows would owe a transaction inverse.
    coordinates: DeclarationCoordinates,
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

/// Active-path marks for a spelling walk. Compiler-owned metadata validation rejects
/// cycles before semantic or durable use; these marks keep rendering total even when
/// it is asked to spell a hostile cache.
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
        name: &ScopedName,
    ) -> Result<Option<RecordInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_record_by_name(name))
    }

    pub(crate) fn static_group_projection(
        &self,
        record: &ScopedName,
        group: &str,
    ) -> Result<Option<GroupInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_group_by_name(record, group))
    }

    pub(crate) fn static_struct_projection(
        &self,
        name: &ScopedName,
    ) -> Result<Option<StructInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_struct_by_name(name))
    }

    pub(crate) fn static_enum_projection(
        &self,
        name: &ScopedName,
    ) -> Result<Option<EnumInfo>, GenericInvariant> {
        self.with_metadata_session(|session| session.static_enum_by_name(name))
    }

    pub(crate) fn static_named_type_projection(
        &self,
        name: &ScopedName,
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

    /// A metadata directory for one mint/dedup probe or presentation projection. It is
    /// reused across the pass's probes and extended for the rows appended since the
    /// previous probe, so each row is classified once rather than rescanned per probe.
    /// A metadata session borrows this same directory.
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
        // An admitted directory is not lost to a failed probe: `extend` restores it to
        // the state this scope received it in, so putting it back is the whole inverse of
        // having taken it.
        if let Err(invariant) = directory.extend(view) {
            *self.row_directory.borrow_mut() = Some(directory);
            return Err(invariant);
        }
        directory.reset_marks(view);
        Ok(RowDirectoryGuard::seat(self, directory))
    }

    /// Reclassify owned identities on the next metadata query. Resource filling
    /// publishes groups after generic field types may have warmed the directory;
    /// generic rows and collections normally extend it through their append owners.
    fn invalidate_row_directory(&mut self) {
        *self.row_directory.get_mut() = None;
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

    /// Validate value-type arguments against the cached row directory, the same
    /// directory every mint probe and session reads, so a validation never rebuilds
    /// the classification of rows an earlier probe already made.
    pub(crate) fn validate_type_arguments(&self, args: &[GArg]) -> Result<(), GenericInvariant> {
        let view = self.metadata_view();
        let mut metadata = self.row_directory(&view)?;
        view.validate_args_with(args, None, metadata.scratch())
    }

    /// The image enum index of the reserved `Option[inner]`, minting it on first use.
    pub(crate) fn instantiate_reserved_option(
        &mut self,
        draft: &mut DraftTxn<'_>,
        inner: GArg,
        site: MintSite<'_>,
    ) -> Result<EnumId, ResolveError> {
        let template = self.reserved_template(Reserved::Option)?;
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
    pub(crate) fn application_template(
        &self,
        origin: &SourceOrigin,
        head: &str,
    ) -> Result<usize, ResolveError> {
        match head {
            "Option" => return self.reserved_template(Reserved::Option),
            "Result" => return self.reserved_template(Reserved::Result),
            _ => {}
        }
        // A head that names no tree, or one no template answers, is either genuinely
        // undeclared or a template this project declared and the compiler refused.
        let Some(scope) = self.scoped(origin, head) else {
            return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
        };
        match self.type_template_by_name(&scope) {
            Some(template) => Ok(template),
            None => Err(ResolveError::Refusal(self.unresolved_named_type(&scope)?)),
        }
    }

    /// The compiler-owned template for one reserved toolchain generic, which
    /// belongs to no tree and must remain an enum.
    fn reserved_template(&self, reserved: Reserved) -> Result<usize, ResolveError> {
        let template = self
            .type_templates
            .iter()
            .position(|template| template.reserved == Some(reserved))
            .ok_or(ResolveError::Invariant(
                GenericInvariant::ReservedTemplateMissing(reserved),
            ))?;
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
        Ok(template)
    }

    /// The tree a generic type template was declared in. A reserved toolchain
    /// generic belongs to none, so its parameter annotations resolve in the root.
    pub(crate) fn template_origin(&self, template: usize) -> &SourceOrigin {
        self.type_templates
            .get(template)
            .and_then(|template| template.file.as_ref())
            .map_or(&SourceOrigin::Root, ProjectFile::origin)
    }

    /// The template index of a generic value type named `head` (a reserved
    /// `Option`/`Result` or a user `struct`/`enum` template), if one exists.
    pub(crate) fn type_template_by_name(&self, scope: &ScopedName) -> Option<usize> {
        self.type_templates.iter().position(|template| {
            template.name == scope.name()
                // A reserved toolchain generic belongs to no tree and answers every
                // origin; a user template answers only the tree that declared it.
                && template
                    .file
                    .as_ref()
                    .is_none_or(|file| file.origin() == scope.origin())
        })
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
            TemplateBody::Struct(fields) => Ok(fields.to_vec()),
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

    /// Admit one enum-variant payload leaf: the bare value type the leaf carries,
    /// and the collection that disqualifies it when it is not a payload type.
    ///
    /// A declared `enum` member and a generic enum instantiation share this one
    /// rule, so both admit exactly a bare scalar, a nominal int, a struct, or
    /// another enum (a generic application of those resolves to one of them). The
    /// image admits a bare scalar, record, or enum as a payload leaf, so a
    /// collection is refused here — at the declaration or at the mint — and a
    /// checker-clean program can never emit an image the verifier rejects at the
    /// Table phase. The leaf is returned either way: the generic line still fills
    /// its body so the shared instance cache stays consistent, and each caller
    /// renders the refusal into its own ledger.
    fn enum_payload_leaf(
        &mut self,
        draft: &mut DraftTxn<'_>,
        origin: &SourceOrigin,
        ty: &TypeExpr,
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<(GArg, Option<CollTypeId>), ResolveError> {
        let arg = self.resolve_garg_annotation(draft, origin, ty, subst, site)?;
        Ok(match arg {
            GArg::Collection(coll) => (arg, Some(coll)),
            _ => (arg, None),
        })
    }

    /// The refusal a collection payload leaf earns, wherever it was written.
    fn collection_payload_refusal(
        &self,
        site: MintSite<'_>,
        enum_name: &str,
        variant_name: &str,
        coll: CollTypeId,
    ) -> SourceDiagnostic {
        let kind = match self.collection_spec(coll) {
            CollSpec::List { .. } => "List",
            CollSpec::Map { .. } => "Map",
        };
        SourceDiagnostic::at(
            Code::CheckUnsupported,
            site.file,
            site.span,
            format!(
                "the `{variant_name}` payload of `{enum_name}` is a `{kind}` value. An enum \
                 member payload is a bare scalar, a struct, or another enum; a collection is \
                 not a payload type. Declare a struct that holds the collection and use that \
                 struct as the payload."
            ),
        )
    }

    /// Resolve a type annotation to a bare value type (a [`GArg`]), monomorphizing
    /// any `Option`/`Result`/user generic application into `draft` on first use.
    /// `None` for an optional, the resource record, or a name not yet
    /// declared as a value type.
    fn resolve_garg(
        &mut self,
        draft: &mut DraftTxn<'_>,
        annotation: &TypeExpr,
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        self.resolve_garg_annotation(draft, site.file.origin(), annotation, &[], site)
    }

    /// Resolve a type expression under a substitution environment (`param name ->
    /// concrete argument`), used when a generic template body is monomorphized. The
    /// expression is the template's written syntax, so its names resolve in the tree
    /// that declared the template — never in the tree of the use site that mints
    /// this instance.
    fn resolve_garg_env(
        &mut self,
        draft: &mut DraftTxn<'_>,
        origin: &SourceOrigin,
        ty: &TypeExpr,
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        self.resolve_garg_annotation(draft, origin, ty, subst, site)
    }

    fn resolve_garg_annotation(
        &mut self,
        draft: &mut DraftTxn<'_>,
        origin: &SourceOrigin,
        ty: &TypeExpr,
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        match ty {
            TypeExpr::Name { text, .. } => self.resolve_garg_name(origin, text, subst),
            TypeExpr::Apply { head, args, .. } if head == "List" => {
                self.resolve_list_garg(draft, origin, args, subst, site)
            }
            TypeExpr::Apply { head, args, .. } if head == "Map" => {
                self.resolve_map_garg(draft, origin, args, subst, site)
            }
            TypeExpr::Apply { head, args, .. } => {
                self.resolve_template_garg(draft, origin, head, args, subst, site)
            }
            _ => Err(ResolveError::Refusal(ResolveRefusal::Unsupported)),
        }
    }

    fn resolve_garg_name(
        &self,
        origin: &SourceOrigin,
        text: &str,
        subst: &[(String, GArg)],
    ) -> Result<GArg, ResolveError> {
        if let Some((_, arg)) = subst.iter().find(|(name, _)| name == text) {
            return Ok(*arg);
        }
        let Some(written) = self.scoped(origin, text) else {
            return Err(ResolveRefusal::Unsupported.into());
        };
        if let Some(target) = self.alias_target(&written) {
            let arg = self.resolve_global_garg(target.terminal)?;
            if target.presence == AliasPresence::Optional {
                return Err(ResolveRefusal::Unsupported.into());
            }
            Ok(arg)
        } else {
            self.resolve_global_garg(&written)
        }
    }

    fn resolve_global_garg(&self, scope: &ScopedName) -> Result<GArg, ResolveError> {
        if let Some(scalar) = ScalarType::from_spelling(scope.name()) {
            Ok(GArg::Scalar(scalar))
        } else if let Some((id, _)) = self.nominal_by_name(scope) {
            Ok(GArg::Nominal(id))
        } else if let Some(info) = self.struct_by_name(scope) {
            Ok(GArg::Struct(info.type_id))
        } else if let Some(info) = self.enum_by_name(scope) {
            Ok(GArg::Enum(info.enum_id))
        } else {
            // A name no table answers is either genuinely undeclared or a declaration
            // this project refused; the ledger tells them apart. Answering `Unsupported`
            // for both would let a member position describe a refused sibling as a
            // language form the beta line does not admit.
            Err(ResolveError::Refusal(self.unresolved_named_type(scope)?))
        }
    }

    fn resolve_list_garg(
        &mut self,
        draft: &mut DraftTxn<'_>,
        origin: &SourceOrigin,
        args: &[TypeExpr],
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        let [elem] = args else {
            return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
        };
        let elem = self.resolve_garg_annotation(draft, origin, elem, subst, site)?;
        Ok(GArg::Collection(self.instantiate_list(draft, elem)?))
    }

    fn resolve_map_garg(
        &mut self,
        draft: &mut DraftTxn<'_>,
        origin: &SourceOrigin,
        args: &[TypeExpr],
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        let [key, value] = args else {
            return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
        };
        let key = self.resolve_garg_annotation(draft, origin, key, subst, site)?;
        self.check_map_key_admissibility(key)?;
        let value = self.resolve_garg_annotation(draft, origin, value, subst, site)?;
        Ok(GArg::Collection(self.instantiate_map(draft, key, value)?))
    }

    fn resolve_template_garg(
        &mut self,
        draft: &mut DraftTxn<'_>,
        origin: &SourceOrigin,
        head: &str,
        args: &[TypeExpr],
        subst: &[(String, GArg)],
        site: MintSite<'_>,
    ) -> Result<GArg, ResolveError> {
        let template = self.application_template(origin, head)?;
        let mut resolved = Vec::with_capacity(args.len());
        for arg in args {
            resolved.push(self.resolve_garg_annotation(draft, origin, arg, subst, site)?);
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
            .map(GArg::from)
    }

    /// Validate one instantiation key and resolve any existing row without keeping
    /// validation scratch in the mint frame. A missing key returns `None` only after
    /// its complete metadata preflight succeeds.
    fn existing_type_instance(
        &self,
        template: usize,
        args: &[GArg],
        requirement: ReadyRequirement<'_>,
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
            let existing = match view
                .generics
                .type_index
                .get(&template)
                .and_then(|rows| rows.get(args))
            {
                Some(&index) => {
                    let drifted = view
                        .generics
                        .type_insts
                        .get(index)
                        .is_none_or(|inst| inst.template != template || inst.args != args);
                    if drifted {
                        return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                            "mint index drift",
                        ))
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
                            requirement.validate(inst, body)?;
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
            return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                "Filling reuse outside batch",
            ))
            .into());
        };
        let Some(dependent) = generics.filling.map(|frame| frame.index) else {
            return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                "Filling reuse outside batch",
            ))
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
            return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                "Filling reuse outside batch",
            ))
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
    pub(crate) fn mint_type_instance(
        &mut self,
        draft: &mut DraftTxn<'_>,
        template: usize,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<TypeInstId, ResolveError> {
        self.mint_type_instance_with_requirement(draft, template, args, site, ReadyRequirement::Any)
    }

    fn mint_type_instance_with_requirement(
        &mut self,
        draft: &mut DraftTxn<'_>,
        template: usize,
        args: &[GArg],
        site: MintSite<'_>,
        requirement: ReadyRequirement<'_>,
    ) -> Result<TypeInstId, ResolveError> {
        if let Some(id) = self.existing_type_instance(template, args, requirement)? {
            return Ok(id);
        }
        let outermost = self.generics.borrow().filling.is_none();
        let (id, inst_index) = self.reserve_type_instance(draft, template, args, site)?;
        if !outermost {
            // A member's mint hands its caller the reserved identity and nothing more:
            // the queued fill runs later in the outermost drain, and settlement
            // publishes the body or rejects the row through the dependency graph.
            return match requirement.allows_provisional() {
                true => Ok(id),
                false => Err(GenericInvariant::ReadyBodyMissing(id).into()),
            };
        }
        self.drain_pending_fills(draft, site)?;
        self.settle_fill_batch()?;
        self.settled_type_result(inst_index, id, requirement)
    }

    /// Reserve the image row, the provisional cache row and the queued fill for one
    /// new instantiation, and record the dependency edges settlement propagates
    /// refusals along. Crossing the shared instantiation count or the nesting depth
    /// returns `Err(Limit)` with the one owned `check.instantiation_limit` recorded.
    fn reserve_type_instance(
        &mut self,
        draft: &mut DraftTxn<'_>,
        template: usize,
        args: &[GArg],
        site: MintSite<'_>,
    ) -> Result<(TypeInstId, usize), ResolveError> {
        let template_info = self.template_for_args(template, args)?;
        let depth = {
            let generics = self.generics.borrow();
            let over_count =
                generics.type_insts.len() + generics.fn_insts.len() >= MAX_INSTANTIATIONS;
            // One more than the depth of the fill that named this row; an outermost
            // mint is depth zero.
            let depth = generics.filling.map_or(0, |frame| frame.depth + 1);
            if over_count || depth >= MINT_DEPTH_LIMIT {
                let limit = if over_count {
                    InstantiationLimit::Count
                } else {
                    InstantiationLimit::TypeDepth
                };
                drop(generics);
                self.record_limit(site, limit);
                return Err(ResolveError::Refusal(ResolveRefusal::Limit));
            }
            depth
        };
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
            if generics.filling.is_none() && generics.fill_batch_start.is_none() {
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
            let displaced = generics
                .type_index
                .entry(template)
                .or_default()
                .insert(args.to_vec(), index);
            if displaced.is_some() {
                return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                    "mint key already present",
                ))
                .into());
            }
            generics.fill_rows.insert(id.into(), index);
            generics
                .pending_fills
                .push_back(PendingFill { index, depth });
            index
        };
        self.record_active_dependency(inst_index);
        self.record_semantic_dependencies(inst_index, args.iter().copied());
        Ok((id, inst_index))
    }

    /// Fill every queued row of the active batch, in reservation order.
    ///
    /// This loop is the whole of the monomorphization recursion: a member needing a
    /// further instantiation reserves it and appends it here instead of descending
    /// into it, so one nesting level costs one queue entry and no machine frame. A
    /// member refusal is recorded against its own row and the drain continues, because
    /// settlement requires every reserved row to carry either a staged body or a
    /// refusal; the recorded dependency edges then carry that refusal to the rows that
    /// named it. An instantiation bound is the exception: it ends the batch, because
    /// filling the rest of the queue could only reserve more rows against a bound that
    /// is already exhausted.
    fn drain_pending_fills(
        &mut self,
        draft: &mut DraftTxn<'_>,
        site: MintSite<'_>,
    ) -> Result<(), ResolveError> {
        loop {
            let next = {
                let mut generics = self.generics.borrow_mut();
                generics.filling = generics.pending_fills.pop_front();
                match generics.filling {
                    Some(pending) => {
                        let Some(inst) = generics.type_insts.get(pending.index) else {
                            return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                                "pending fill row missing",
                            ))
                            .into());
                        };
                        Some((pending, inst.template, inst.id, inst.args.clone()))
                    }
                    None => None,
                }
            };
            let Some((pending, template, id, args)) = next else {
                return Ok(());
            };
            let filled = self.fill_type_body(draft, template, id, &args, site);
            self.finish_fill(pending.index)?;
            match filled {
                Ok(body) => {
                    self.generics.borrow_mut().type_insts[pending.index].state =
                        TypeInstState::Filling { staged: Some(body) };
                }
                Err(ResolveError::Refusal(refusal)) => {
                    let mut generics = self.generics.borrow_mut();
                    generics.fill_failures.push((pending.index, refusal));
                    if matches!(refusal, ResolveRefusal::Limit) {
                        // Refuse the queue where it stands: these rows are reserved but
                        // unresolvable, and settlement takes them and their dependents
                        // down with the same bound.
                        let abandoned = std::mem::take(&mut generics.pending_fills);
                        generics.fill_failures.extend(
                            abandoned
                                .into_iter()
                                .map(|pending| (pending.index, ResolveRefusal::Limit)),
                        );
                        return Ok(());
                    }
                }
                Err(ResolveError::Invariant(invariant)) => {
                    return Err(ResolveError::Invariant(invariant));
                }
            }
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
            ReadyRequirement::Struct,
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
        let id = self.mint_type_instance_with_requirement(
            draft,
            template,
            args,
            site,
            ReadyRequirement::Variant(selection),
        )?;
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

    /// Close the active fill. A mismatch is observed without clearing the actual
    /// frame so the first cache invariant preserves all hostile state.
    fn finish_fill(&self, inst_index: usize) -> Result<(), ResolveError> {
        let mut generics = self.generics.borrow_mut();
        if generics.filling.map(|frame| frame.index) != Some(inst_index) {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant("fill frame mismatch"),
            )));
        }
        generics.filling = None;
        Ok(())
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
        let origin = self.template_origin(template).clone();
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
            // A handle, not a borrow: resolving a field mints through the exclusively
            // held registry, which no live read of a template may cross. Bound: zero
            // declaration entries copied per instantiation, pinned by
            // `a_fill_copies_no_template_body_entries`.
            let fields = Rc::clone(fields);
            (subst, fields)
        };
        let mut pending = Vec::with_capacity(fields.len());
        for (fname, fty) in fields.iter() {
            let arg = self.resolve_garg_env(draft, &origin, fty, &subst, site)?;
            let name = draft.intern_string(fname)?;
            pending.push((name, arg));
        }
        let TypeInstId::Record(ty) = id else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id,
                body: TypeInstKind::Struct,
            }
            .into());
        };
        let (defs, resolved) = fields
            .iter()
            .zip(pending)
            .map(|((fname, _), (name, arg))| {
                (
                    FieldDef {
                        name,
                        ty: arg.image(),
                        required: true,
                    },
                    (fname.clone(), arg),
                )
            })
            .unzip();
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
        let origin = self.template_origin(template).clone();
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
            // Shared for the same reason as a struct fill, and counted the same way. The
            // copy avoided here is the larger of the two bodies: `MAX_VARIANTS` variants
            // each of `MAX_PAYLOAD_FIELDS` leaves.
            let variants = Rc::clone(variants);
            (subst, variants, template_info.name.clone())
        };
        let enum_name = enum_name.as_str();
        let mut reported = false;
        let mut resolved = Vec::with_capacity(variants.len());
        let mut defs = Vec::with_capacity(variants.len());
        for variant in variants.iter() {
            let mut payload = Vec::with_capacity(variant.payload.len());
            let mut leaves = Vec::with_capacity(variant.payload.len());
            for field in &variant.payload {
                let (arg, refused) =
                    self.enum_payload_leaf(draft, &origin, &field.ty, &subst, site)?;
                // The instantiation still fills its body so the shared instance
                // cache stays consistent; the non-empty pending queue makes the
                // driver reject before the image is encoded, so the collection leaf
                // never reaches the verifier.
                if let Some(coll) = refused
                    && !reported
                {
                    let refusal =
                        self.collection_payload_refusal(site, enum_name, &variant.name, coll);
                    self.generics.borrow_mut().collection_payloads.push(refusal);
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
        let Some(dependent) = generics.filling.map(|frame| frame.index) else {
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

    /// Record `dependent` as depending on every provisional row its arguments reach.
    ///
    /// With [`record_active_dependency`](Self::record_active_dependency) and the
    /// reuse edge `existing_type_instance` records, this covers every provisional row
    /// a filled body can name: a member's type is built from this row's arguments,
    /// from a concrete name of the template's own tree, or from a nested mint, and a
    /// collection is walked through to those same leaves.
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
            return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                "stable row in active batch",
            ))
            .into());
        };
        let Some(body) = staged.as_ref() else {
            return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                "incomplete row without refusal",
            ))
            .into());
        };
        self.validate_inst_body_metadata(inst.template, &inst.args, inst.id, body)?;

        let body = match &mut inst.state {
            TypeInstState::Filling { staged } => staged.take().ok_or({
                ResolveError::Invariant(GenericInvariant::CacheState(GenericCacheInvariant(
                    "incomplete row without refusal",
                )))
            })?,
            TypeInstState::Ready(_) | TypeInstState::Rejected(_) => {
                return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                    "stable row in active batch",
                ))
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
                GenericCacheInvariant("active batch missing"),
            )));
        };
        let end = generics.type_insts.len();
        let Some(active_len) = end.checked_sub(start) else {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant("active batch range"),
            )));
        };
        if generics.filling.is_some() || !generics.pending_fills.is_empty() {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant("active fill not finished"),
            )));
        }
        if generics.fill_rows.len() != active_len {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant("active row cardinality"),
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
                GenericCacheInvariant("active row key mismatch"),
            )));
        }
        if generics
            .fill_failures
            .iter()
            .any(|(index, _)| *index < start || *index >= end)
        {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant("failure index out of range"),
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
                    GenericCacheInvariant("stable row in active batch"),
                )));
            };
            if inst
                .dependents
                .iter()
                .any(|dependent| *dependent < start || *dependent >= end)
            {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant("dependent index out of range"),
                )));
            }
            if staged.is_none() && refusals[offset].is_none() {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant("incomplete row without refusal"),
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
                    GenericCacheInvariant("stable row in active batch"),
                )));
            };
            if refusals[offset].is_none() {
                let Some(body) = staged.as_ref() else {
                    return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                        GenericCacheInvariant("incomplete row without refusal"),
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

    fn settled_type_result(
        &self,
        index: usize,
        id: TypeInstId,
        requirement: ReadyRequirement<'_>,
    ) -> Result<TypeInstId, ResolveError> {
        let generics = self.generics.borrow();
        let Some(inst) = generics.type_insts.get(index) else {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant("settled row missing"),
            )));
        };
        match &inst.state {
            TypeInstState::Ready(_) => {}
            TypeInstState::Rejected(refusal) => {
                return Err(ResolveError::Refusal(*refusal));
            }
            TypeInstState::Filling { .. } => {
                return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                    GenericCacheInvariant("settled row still Filling"),
                )));
            }
        }
        drop(generics);
        let view = self.metadata_view();
        let Some(inst) = view.generics.type_insts.get(index) else {
            return Err(ResolveError::Invariant(GenericInvariant::CacheState(
                GenericCacheInvariant("settled row missing"),
            )));
        };
        let mut metadata = view.registry.row_directory(&view)?;
        let body = view
            .ready_inst_header_with(inst, metadata.scratch())?
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        requirement.validate(inst, body)?;
        view.validate_ready_body_with(inst, body, metadata.scratch())?;
        Ok(id)
    }

    fn record_limit(&self, site: MintSite<'_>, limit: InstantiationLimit) {
        let mut generics = self.generics.borrow_mut();
        if matches!(generics.limit, LimitState::Open) {
            let message = match limit {
                InstantiationLimit::Count => format!(
                    "generic instantiation reached the limit of {MAX_INSTANTIATIONS} distinct function and type instances"
                ),
                InstantiationLimit::TypeDepth => format!(
                    "generic type instantiation reached the nesting limit of {MINT_DEPTH_LIMIT}"
                ),
            };
            generics.limit = LimitState::Pending(SourceDiagnostic::at(
                Code::CheckInstantiationLimit,
                site.file,
                site.span,
                message,
            ));
        }
    }

    /// The resolved member shape of a minted type instantiation, if `id` names one.
    fn type_inst_body(&self, id: TypeInstId) -> Result<Option<InstBody>, GenericInvariant> {
        let view = self.metadata_view();
        let mut metadata = self.row_directory(&view)?;
        Ok(view
            .ready_inst_by_id(id, metadata.scratch())?
            .map(|(_, body)| body.clone()))
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
            Some(InstBody::Enum(variants)) => Ok(Some(ready_enum_variants(&variants))),
            Some(InstBody::Struct(_)) => Err(GenericInvariant::TypeBodyKindMismatch {
                id: TypeInstId::Enum(id),
                body: TypeInstKind::Struct,
            }),
            None => Ok(self.enum_by_id(id).map(EnumInfo::resolved_variants)),
        }
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
        render_validated_arg(self, view, metadata, GArg::from(id), display, ANCHOR).map(Some)
    }

    /// The source spelling of a generic type instantiation, `Name<arg, ...>`, if
    /// `id` names one. The canonical angle-form display owner for diagnostics and
    /// cycle labels; durable identity uses [`enum_anchor_spelling`](Self::enum_anchor_spelling).
    pub(crate) fn inst_spelling(&self, id: TypeInstId) -> Option<String> {
        let view = self.metadata_view();
        let metadata = self.row_directory(&view).ok()?;
        let mut display = DisplayScratch::for_view(&view);
        render_validated_arg(
            self,
            &view,
            &metadata,
            GArg::from(id),
            &mut display,
            DISPLAY,
        )
        .ok()
    }

    fn inst_spelling_validated(
        &self,
        view: &TypeMetadataView<'_>,
        metadata: &MetadataScratch,
        id: TypeInstId,
        display: &mut DisplayScratch,
    ) -> Result<Option<String>, GenericInvariant> {
        let arg = GArg::from(id);
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
        render_validated_arg(self, view, metadata, arg, display, DISPLAY).map(Some)
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
        if let Some(&index) = self.collection_index.borrow().get(&spec) {
            if collections.get(index.index() as usize) != Some(&spec) {
                return Err(GenericInvariant::CacheState(GenericCacheInvariant(
                    "mint index drift",
                ))
                .into());
            }
            return Ok(index);
        }
        drop(collections);

        // The cache index and the draft's own index advance together; the reuse probe
        // above turns any later divergence into a typed drift refusal.
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
        let spelling = self.row_directory(&view).ok().and_then(|metadata| {
            let mut display = DisplayScratch::for_view(&view);
            render_validated_arg(
                self,
                &view,
                &metadata,
                GArg::Collection(idx),
                &mut display,
                DISPLAY,
            )
            .ok()
        });
        spelling.unwrap_or_else(|| "collection".to_string())
    }

    /// Every admitted `resource` record, in declaration-admission order. The durable
    /// build's resource directory is taken from this list, so the registry is the one
    /// source deciding which resources exist.
    pub(crate) fn admitted_resources(&self) -> &[RecordInfo] {
        &self.records
    }

    /// For each admitted record, in record order, its position in the resource slice the
    /// declare pass was given. The durable build reads this instead of re-pairing records
    /// to declarations by name.
    pub(crate) fn record_declaration_ordinals(&self) -> &[usize] {
        self.records.ordinals()
    }

    /// The module position and span `type_id` was declared at, for a consumer checking
    /// that a declaration it holds sits where the record's declaration was written.
    pub(crate) fn declaration_module(
        &self,
        type_id: TypeId,
    ) -> Option<(crate::analysis::FileRef, SourceSpan)> {
        self.coordinates.module_of(type_id)
    }

    pub(crate) fn by_name(&self, scope: &ScopedName) -> Option<&RecordInfo> {
        self.records
            .iter()
            .find(|info| info.origin == *scope.origin() && info.name == scope.name())
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
    /// This is the only scan of `structs` keyed on a source spelling; annotation
    /// resolution, signature building, and body lowering delegate here rather than
    /// scanning again. A second scan is a second place to forget the verdict, and a name
    /// answered by a reserved, unfilled row resolves to a live empty struct against
    /// which every later question fabricates an answer.
    pub(crate) fn struct_by_name(&self, scope: &ScopedName) -> Option<&StructInfo> {
        self.structs.iter().find(|info| {
            info.origin == *scope.origin()
                && info.name == scope.name()
                && info.verdict.is_accepted()
        })
    }

    pub(crate) fn struct_by_type(&self, ty: TypeId) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.type_id == ty)
    }

    /// The accepted enum declared as `name`. A refused row answers no name, for the
    /// reason given at [`Self::struct_by_name`].
    pub(crate) fn enum_by_name(&self, scope: &ScopedName) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| {
            info.origin == *scope.origin()
                && info.name == scope.name()
                && info.verdict.is_accepted()
        })
    }

    pub(crate) fn enum_by_id(&self, id: EnumId) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.enum_id == id)
    }

    /// Why an annotation naming `name` could not resolve.
    ///
    /// Where a written type spelling resolves from `origin`.
    ///
    /// A bare name resolves in the tree that wrote it: type namespaces are
    /// origin-scoped, so a dependency's `Pair` and the root's `Pair` are two types.
    /// A two-segment `alias::Name` resolves in the dependency the alias declares.
    /// Any other shape — an unknown first segment, or more than two segments —
    /// names no tree, so it resolves nowhere and the annotation is refused rather
    /// than silently read as a bare name carrying a `::`.
    pub(crate) fn scoped(&self, origin: &SourceOrigin, written: &str) -> Option<ScopedName> {
        ScopedName::written(&self.origins, origin, written)
    }

    /// The one conversion from a named-type ledger lookup to a resolution refusal,
    /// so `Unsupported` keeps meaning *genuinely outside the admitted subset* and
    /// is never the answer for a type this project declared. A name the ledger
    /// never saw is a real absence; a name it refused carries the cause forward as
    /// a `Copy` handle.
    pub(crate) fn unresolved_named_type(
        &self,
        name: &ScopedName,
    ) -> Result<ResolveRefusal, DeclarationIndexDrift> {
        Ok(match self.named.lookup(name)? {
            Binding::Refused(id, _) => ResolveRefusal::RefusedDeclaration(id),
            Binding::Accepted(_) | Binding::Absent => ResolveRefusal::Unsupported,
        })
    }

    /// What already holds the type name `name`, or `None` when the name is free.
    ///
    /// The one conflict predicate every declaration pass runs, so which of two
    /// colliding declarations is refused cannot depend on which pass asks.
    ///
    /// The named-type ledger answers for every name a pass has settled, accepted or
    /// refused. Two kinds reach it later than they take their name, and the tables
    /// that hold them in the meantime are read here rather than at each call site:
    /// an accepted alias is declared only once its target is validated, after every
    /// other pass, and a struct or enum name is declared with its fill verdict in
    /// pass two, after pass one reserved its image index.
    pub(super) fn name_conflict(
        &self,
        scope: &ScopedName,
    ) -> Result<Option<NameHolder>, DeclarationIndexDrift> {
        if ScalarType::from_spelling(scope.name()).is_some() {
            return Ok(Some(NameHolder::Kind(NamedTypeKind::Scalar)));
        }
        Ok(match self.named.lookup(scope)? {
            Binding::Accepted(kind) => Some(NameHolder::Kind(*kind)),
            Binding::Refused(..) => Some(NameHolder::Refused),
            Binding::Absent => {
                if self.aliases.contains_key(scope) {
                    Some(NameHolder::Kind(NamedTypeKind::Alias))
                } else if self.struct_by_name(scope).is_some() {
                    Some(NameHolder::Kind(NamedTypeKind::Struct))
                } else if self.enum_by_name(scope).is_some() {
                    Some(NameHolder::Kind(NamedTypeKind::Enum))
                } else {
                    None
                }
            }
        })
    }

    /// What the declared type name `name` binds: its kind, the refusal that stands
    /// in its place, or a genuine absence.
    pub(crate) fn named_type(
        &self,
        name: &ScopedName,
    ) -> Result<Binding<'_, NamedTypeKind>, DeclarationIndexDrift> {
        self.named.lookup(name)
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
    fn member_refusal_row(
        &self,
        refusal: ResolveRefusal,
        file: &ProjectFile,
        span: SourceSpan,
        subject: &str,
    ) -> Result<Option<SourceDiagnostic>, GenericInvariant> {
        match refusal {
            ResolveRefusal::Limit => Ok(None),
            ResolveRefusal::Unsupported => Ok(Some(unsupported(file, span, subject))),
            ResolveRefusal::RefusedDeclaration(id) => {
                let summary = self.refusal(id)?;
                Ok(Some(declaration_refused(
                    file,
                    span,
                    id.namespace(),
                    summary,
                )))
            }
        }
    }

    /// Scalar lookup never mints: its refusal must name a source cause, not a
    /// generic limit whose diagnostic some other phase might have reported.
    pub(crate) fn scalar_refusal_row(
        &self,
        refusal: ResolveRefusal,
        file: &ProjectFile,
        span: SourceSpan,
        subject: &str,
    ) -> Result<SourceDiagnostic, GenericInvariant> {
        self.member_refusal_row(refusal, file, span, subject)?
            .ok_or(GenericInvariant::ScalarResolutionLimit)
    }

    /// The accepted members of `owner`, in declaration order.
    ///
    /// `owner` is a resource record's name, or the `Record.group` anchor of one of
    /// its unkeyed groups. This is what a record's field list is built from, so
    /// the record and the ledger cannot disagree about which members survived.
    fn accepted_members(&self, owner: &ScopedName) -> Vec<FieldInfo> {
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
    pub(crate) fn refused_members(&self, owner: &ScopedName) -> Vec<&str> {
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
        owner: &ScopedName,
        member: &str,
    ) -> Result<Binding<'_, FieldInfo>, DeclarationIndexDrift> {
        self.members.lookup(&MemberKey::new(owner, member))
    }

    /// The same steer for a member a projection already resolved to a refusal
    /// handle, so the owner's name is not spelled a second time at the use site.
    pub(crate) fn refused_member_steer(
        &self,
        id: DeclarationRefusalId,
        file: &ProjectFile,
        span: SourceSpan,
    ) -> Result<Option<SourceDiagnostic>, DeclarationIndexDrift> {
        let summary = self.members.refusal(id)?;
        Ok(summary
            .steer_once()
            .then(|| declaration_refused(file, span, id.namespace(), summary)))
    }

    /// The refusal a named-type or template handle addresses. Every other
    /// namespace's handle is drift here, checked by the ledger's own tag.
    pub(crate) fn refusal(
        &self,
        id: DeclarationRefusalId,
    ) -> Result<&DeclarationRefusalSummary, DeclarationIndexDrift> {
        self.named.refusal(id)
    }

    pub(crate) fn nominal_by_name(&self, scope: &ScopedName) -> Option<(NominalId, &NominalInfo)> {
        self.nominals
            .iter()
            .position(|info| info.origin == *scope.origin() && info.name == scope.name())
            .map(|index| (NominalId(index as u32), &self.nominals[index]))
    }

    pub(crate) fn nominal(&self, id: NominalId) -> &NominalInfo {
        &self.nominals[id.0 as usize]
    }

    /// An alias terminal is bound once at declaration: it must never re-enter a
    /// caller's parameter environment. The terminal names a type of the tree that
    /// declared the alias, so it is returned already scoped to that tree.
    pub(crate) fn alias_target(&self, scope: &ScopedName) -> Option<GlobalAliasTarget<'_>> {
        self.aliases.get(scope)
    }

    pub(crate) fn scalar_annotation(
        &self,
        origin: &SourceOrigin,
        ty: &TypeExpr,
    ) -> Result<ScalarType, ResolveError> {
        let TypeExpr::Name { text, .. } = ty else {
            return Err(ResolveRefusal::Unsupported.into());
        };
        let Some(written) = self.scoped(origin, text) else {
            return Err(ResolveRefusal::Unsupported.into());
        };
        let target = self.alias_target(&written);
        let scope = target.map_or(&written, |target| target.terminal);
        if let Binding::Refused(id, _) = self.named.lookup(scope)? {
            return Err(ResolveRefusal::RefusedDeclaration(id).into());
        }
        if target.is_some_and(|target| target.presence == AliasPresence::Optional) {
            return Err(ResolveRefusal::Unsupported.into());
        }
        ScalarType::from_spelling(scope.name()).ok_or_else(|| ResolveRefusal::Unsupported.into())
    }

    fn optional_annotation(&self, origin: &SourceOrigin, ty: &TypeExpr) -> bool {
        match ty {
            TypeExpr::Optional { .. } => true,
            TypeExpr::Name { text, .. } => self
                .scoped(origin, text)
                .and_then(|scope| self.alias_target(&scope))
                .is_some_and(|target| target.presence == AliasPresence::Optional),
            _ => false,
        }
    }

    /// Normalize alias targets, build nominal and value types, then validate alias
    /// terminals against the completed declaration set.
    ///
    /// Value types (the resource records, the dense structs, and the closed
    /// enums) are built declare-then-fill: pass one reserves every type's image
    /// index with empty members and decides name conflicts, so pass two can resolve
    /// each field or payload against the full set of declared types regardless of
    /// declaration order — a struct field may name a later struct or enum, two
    /// structs may reference each other, and a resource field may name a user enum.
    /// The only nesting restriction is acyclicity: a value type may not contain
    /// itself directly or transitively, reported at check time and independently
    /// re-rejected by the verifier.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        draft: &mut DraftTxn<'_>,
        origins: CapturedOrigins,
        aliases: &[(FileRef, ProjectFile, &AliasDecl)],
        nominals: &[(FileRef, ProjectFile, &NominalDecl)],
        structs: &[(FileRef, ProjectFile, &StructDecl)],
        enums: &[(FileRef, ProjectFile, &EnumDecl)],
        resources: &[(FileRef, ProjectFile, &ResourceDecl)],
        diagnostics: &mut DiagnosticCollector,
        budget: DeclarationBudget,
    ) -> Result<Self, BuildError> {
        let mut named = DeclarationLedger::new(DeclarationNamespace::NamedType, budget.clone());
        let aliases_table = build_alias_table(
            &mut named,
            &origins,
            aliases,
            resources,
            structs,
            enums,
            diagnostics,
        )?;
        let mut registry = Self {
            named,
            origins,
            members: DeclarationLedger::new(DeclarationNamespace::ResourceMember, budget),
            aliases: aliases_table,
            nominals: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            records: AdmittedRecords::default(),
            type_templates: reserved_templates(),
            generics: RefCell::default(),
            collections: RefCell::default(),
            collection_index: RefCell::default(),
            row_directory: RefCell::default(),
            coordinates: DeclarationCoordinates::default(),
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
        let concrete_structs: Vec<(FileRef, ProjectFile, &StructDecl)> = structs
            .iter()
            .filter(|(_, _, decl)| decl.type_params.is_empty())
            .map(|(at, file, decl)| (*at, file.clone(), *decl))
            .collect();
        let concrete_enums: Vec<(FileRef, ProjectFile, &EnumDecl)> = enums
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
        let struct_decls = declare_structs(draft, &mut registry, &concrete_structs, diagnostics)?;
        let enum_decls = declare_enums(draft, &mut registry, &concrete_enums, diagnostics)?;

        // Pass two: resolve and fill each definition's members against the full
        // registry, monomorphizing any generic field type on first use. Each pass
        // records its verdict — accepted, or refused with the cause it reported —
        // in the named-type ledger, so pass one's reservation never stands as the
        // answer for a name pass two went on to refuse.
        let result = fill_records(draft, &mut registry, &record_decls, diagnostics)
            .and_then(|()| fill_rows(draft, &mut registry, &struct_decls, diagnostics));
        match result {
            Ok(()) => {
                fill_rows(draft, &mut registry, &enum_decls, diagnostics)?;
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
    /// A fill batch mutates only `type_insts[start..]`: settlement, staging, and dependency
    /// edges never touch a settled prefix row. The proof's batches all open at or above the
    /// length captured here, so the settled prefix is immutable across the pass and
    /// truncation is its exact inverse. Admission requires that settled state: no fill in
    /// progress, no provisional or still-referenced row, no recorded build fault, and the
    /// shared instantiation-limit owner open.
    ///
    /// `entry_records`/`entry_enums` are the draft's record/enum id ceilings at entry, used
    /// to roll the reused metadata directory back to the pre-proof image.
    fn enter_template_proof(
        &self,
        entry_records: usize,
        entry_enums: usize,
    ) -> Result<RegistryInverse, GenericInvariant> {
        let mut generics = self
            .generics
            .try_borrow_mut()
            .map_err(|_| GenericInvariant::TemplateProof(TemplateProofError::UnstableFillState))?;
        let has_unstable_row = generics.type_insts.iter().any(|inst| {
            matches!(inst.state, TypeInstState::Filling { .. }) || !inst.dependents.is_empty()
        });
        // A proof pass is non-reentrant: it swaps the argument domain to `TemplateProof` for
        // its duration and restores it on exit, so finding it already `TemplateProof` on entry
        // means a prior pass never exited (or the owner is otherwise unsettled). Reject rather
        // than nest, keeping the swap a clean save/restore pair.
        if generics.fill_batch_start.is_some()
            || !generics.fill_rows.is_empty()
            || generics.filling.is_some()
            || !generics.pending_fills.is_empty()
            || !generics.fill_failures.is_empty()
            || has_unstable_row
            || generics.build_invariant.is_some()
            || !matches!(generics.argument_domain, ArgumentDomain::Concrete)
        {
            return Err(GenericInvariant::TemplateProof(
                TemplateProofError::UnstableFillState,
            ));
        }
        if !matches!(generics.limit, LimitState::Open) {
            return Err(GenericInvariant::TemplateProof(
                TemplateProofError::LimitOwnerNotOpen,
            ));
        }
        // Contention on the collection owner is a coherence failure, not a RefCell unwind;
        // read its length before any mutation so a conflict leaves the registry untouched.
        let collections = self
            .collections
            .try_borrow()
            .map_err(|_| GenericInvariant::TemplateProof(TemplateProofError::UnstableFillState))?
            .len();
        let savepoint = RegistryInverse {
            type_insts: generics.type_insts.len(),
            collections,
            fn_insts: generics.fn_insts.len(),
            fn_queue: generics.fn_queue.len(),
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
    fn admit_generic_owners(
        &self,
        entry_records: usize,
        entry_enums: usize,
    ) -> Result<RegistryInverse, GenericInvariant> {
        let generics = self
            .generics
            .try_borrow()
            .map_err(|_| GenericInvariant::TemplateProof(TemplateProofError::UnstableFillState))?;
        if generics.fill_batch_start.is_some()
            || !generics.fill_rows.is_empty()
            || generics.filling.is_some()
            || !generics.pending_fills.is_empty()
            || !generics.fill_failures.is_empty()
        {
            return Err(GenericInvariant::TemplateProof(
                TemplateProofError::UnstableFillState,
            ));
        }
        // Read before any mutation, so contention on the collection owner refuses with
        // the registry untouched.
        let collections = self
            .collections
            .try_borrow()
            .map_err(|_| GenericInvariant::TemplateProof(TemplateProofError::UnstableFillState))?
            .len();
        // Destructured exhaustively: a new generic owner stops this compiling until it is
        // captured here or deliberately excluded beside the two the inverse names in
        // `UNRESTORED_DIAGNOSTIC_OWNERS`. The fill owners are bound to `_` because
        // admission above has just proved every one of them empty, which is what makes
        // the captured lengths a complete description of the batch.
        let Monomorph {
            type_insts,
            type_index: _,
            fn_insts,
            fn_index: _,
            fn_queue,
            fill_batch_start: _,
            fill_rows: _,
            filling: _,
            pending_fills: _,
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
    fn restore_generic_owners(&mut self, inverse: RegistryInverse) {
        let RegistryInverse {
            type_insts,
            collections,
            fn_insts,
            fn_queue,
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
                    remove_index_key(&mut generics.type_index, inst.template, &inst.args);
                }
            }
            while generics.fn_insts.len() > fn_insts {
                if let Some(inst) = generics.fn_insts.pop() {
                    remove_index_key(&mut generics.fn_index, inst.template, &inst.args);
                }
            }
            generics.fn_queue.truncate(fn_queue);
            generics.fill_batch_start = None;
            generics.fill_rows.clear();
            generics.filling = None;
            generics.pending_fills.clear();
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
    diagnostics: &mut DiagnosticCollector,
) -> Result<(), GenericInvariant> {
    let view = registry.metadata_view();
    let mut metadata = registry.row_directory(&view)?;
    let graph = ValueGraph::build_validated(registry, &view, metadata.scratch())?;
    for info in &registry.structs {
        // A refused struct has an empty body and so lies on no cycle, but it is also
        // not a declaration this pass speaks for: its own cause was already reported
        // at its declaration.
        if !info.verdict.is_accepted() {
            continue;
        }
        if let Some(path) = graph.cycle_through(ValueNode::Record(info.type_id)) {
            let (file, span) = registry
                .coordinates
                .resolve(info.type_id)
                .ok_or(GenericInvariant::DeclarationCoordinateMissing(info.type_id))?;
            diagnostics.push(value_cycle_diagnostic(file, span, &info.name, &path));
        }
    }
    for record in registry.records.iter() {
        if let Some(path) = graph.cycle_through(ValueNode::Record(record.type_id)) {
            let (file, span) = registry.coordinates.resolve(record.type_id).ok_or(
                GenericInvariant::DeclarationCoordinateMissing(record.type_id),
            )?;
            diagnostics.push(value_cycle_diagnostic(file, span, &record.name, &path));
        }
    }
    for info in &registry.enums {
        // A refused enum has no variants and so lies on no cycle; like a refused
        // struct, its own cause was already reported at its declaration.
        if !info.verdict.is_accepted() {
            continue;
        }
        if let Some(path) = graph.cycle_through(ValueNode::Enum(info.enum_id)) {
            let (file, span) = registry
                .coordinates
                .resolve_enum(info.enum_id)
                .ok_or(GenericInvariant::EnumCoordinateMissing(info.enum_id))?;
            diagnostics.push(value_cycle_diagnostic(file, span, &info.name, &path));
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
    file: &ProjectFile,
    span: SourceSpan,
    name: &str,
    path: &[String],
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckRecursion,
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
        for record in registry.records.iter() {
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
            let outgoing = info
                .variants
                .iter()
                .flat_map(|variant| variant.payload.iter().map(|field| field.ty))
                .collect::<Vec<_>>();
            view.validate_args_with(&outgoing, None, metadata)?;
            push(ValueNode::Enum(info.enum_id), info.name.clone(), outgoing);
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
    /// stack (grey) witnesses a cycle. One shared traversal costs O(V + E) total.
    /// Explicit stacks keep the walk iterative, so a deep value graph cannot overflow
    /// the native call stack.
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
    /// answers `None` immediately from the shared build-time verdict; only a graph that
    /// already holds a cycle — a program that fails to compile — walks to recover the
    /// exact path.
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

/// The diagnostic for a declaration that reuses a built-in generic type name.
fn reserved_name(file: &ProjectFile, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict,
        file,
        span,
        format!("`{name}` is a built-in generic type and cannot be redeclared"),
    )
}

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
mod generic_instantiation_tests;
#[cfg(test)]
mod name_spelling_tests;

#[cfg(test)]
mod alias_cycle_tests;

#[cfg(test)]
mod refusal_join_tests;

#[cfg(test)]
mod owner_txn_tests;

#[cfg(test)]
mod value_cycle_coords_tests;
