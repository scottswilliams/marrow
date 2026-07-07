//! The checker's diagnostic vocabulary: stable diagnostic codes, the typed
//! payloads that travel beside a rendered message, and the [`CheckDiagnostic`] /
//! [`CheckReport`] result types.

use marrow_codes::Code;
use std::path::{Path, PathBuf};

use marrow_syntax::{Severity, SourceSpan};

use crate::ScalarType;
use crate::program::MarrowType;
use crate::{CatalogEntryKind, CatalogLifecycle};

/// A library or test file declares a module name that does not match its
/// path-derived name.
pub const CHECK_MODULE_PATH: &str = Code::CheckModulePath.as_str();
/// The project's `run.defaultEntry` does not name a runnable zero-argument entry:
/// it is missing, private, ambiguous, or declares parameters. A default entry runs
/// with no arguments, so any of these can only fail at run time; the check rejects
/// them up front.
pub const CHECK_DEFAULT_ENTRY: &str = Code::CheckDefaultEntry.as_str();
/// Two library files declare the same module name.
pub const CHECK_DUPLICATE_MODULE: &str = Code::CheckDuplicateModule.as_str();
/// A project holds more than one module-less file. A project may have at most one
/// single-file script (its entrypoint); every other file must declare a `module`.
pub const CHECK_MULTIPLE_SCRIPTS: &str = Code::CheckMultipleScripts.as_str();
/// A name is declared more than once within one scope: a top-level name declared
/// or imported twice in a file, or a local `const`/`var` redeclared in the same
/// block. Shadowing in an inner block is allowed.
pub const CHECK_DUPLICATE_DECLARATION: &str = Code::CheckDuplicateDeclaration.as_str();
/// A module-level declaration reuses a builtin name. Distinct from a
/// redeclaration: a single declaration that shadows a builtin is rejected on
/// its own, not because the name appears twice.
pub const CHECK_BUILTIN_COLLISION: &str = Code::CheckBuiltinCollision.as_str();
/// A surface declaration name collides with a module-level name, a collection
/// alias collides with another alias or generated operation, or a payload list
/// repeats a name.
pub const CHECK_SURFACE_COLLISION: &str = Code::CheckSurfaceCollision.as_str();
/// A surface's backing store or collection target does not resolve to the
/// declared store/index shape the surface contract admits.
pub const CHECK_SURFACE_TARGET: &str = Code::CheckSurfaceTarget.as_str();
/// A surface payload name does not resolve to an admitted top-level field on the
/// backing store resource.
pub const CHECK_SURFACE_FIELD: &str = Code::CheckSurfaceField.as_str();
/// A surface action does not resolve to a public declared function.
pub const CHECK_SURFACE_ACTION: &str = Code::CheckSurfaceAction.as_str();
/// A surface computed read does not resolve to an admitted public read function.
pub const CHECK_SURFACE_COMPUTED_READ: &str = Code::CheckSurfaceComputedRead.as_str();
/// A `use` names a module that is neither a project module nor a standard
/// library module.
pub const CHECK_UNRESOLVED_IMPORT: &str = Code::CheckUnresolvedImport.as_str();
/// A type annotation names a type the checker does not recognize.
pub const CHECK_UNKNOWN_TYPE: &str = Code::CheckUnknownType.as_str();
/// A typed keyed-entry layer recursively names its own resource shape.
pub const CHECK_RECURSIVE_KEYED_ENTRY: &str = Code::CheckRecursiveKeyedEntry.as_str();
/// A `return` carries a value in a function with no return type, or omits one in a
/// value-returning function.
pub const CHECK_RETURN_VALUE: &str = Code::CheckReturnValue.as_str();
/// A value-returning function can reach the end of its body without returning.
pub const CHECK_MISSING_RETURN: &str = Code::CheckMissingReturn.as_str();
/// An operator is applied to operands whose types it does not accept.
pub const CHECK_OPERATOR_TYPE: &str = Code::CheckOperatorType.as_str();
/// A condition (`if`/`while`) is not a `bool`.
pub const CHECK_CONDITION_TYPE: &str = Code::CheckConditionType.as_str();
/// A call passes the wrong number of arguments, or names a parameter that does
/// not exist, for the function it resolves to.
pub const CHECK_CALL_ARGUMENT: &str = Code::CheckCallArgument.as_str();
/// A `return` value's type does not match the function's declared return type.
pub const CHECK_RETURN_TYPE: &str = Code::CheckReturnType.as_str();
/// A value's type does not match the binding or place it is stored into (a typed
/// `const`/`var` initializer, or an assignment target).
pub const CHECK_ASSIGNMENT_TYPE: &str = Code::CheckAssignmentType.as_str();
/// A whole saved-record replacement can clear keyed child layers that a
/// whole-resource or keyed-entry read does not materialize.
pub const CHECK_LOSSY_ROUND_TRIP: &str = Code::CheckLossyRoundTrip.as_str();
/// A straight-line local resource value is missing a required field when written
/// as a whole saved root.
pub const CHECK_REQUIRED_ABSENT: &str = Code::CheckRequiredAbsent.as_str();
/// A `var` of a type with no buildable initial form — an enum or a store identity —
/// is declared without an initializer. A scalar var defaults, a resource var builds
/// field by field, and a sequence or keyed-tree var starts empty, but an enum and an
/// identity have no default member and no incremental construction, so they must be
/// given an initial value at the declaration.
pub const CHECK_UNINITIALIZED_VAR: &str = Code::CheckUninitializedVar.as_str();
/// A loop condition or body contains a saved-data write outside an explicit transaction.
pub const CHECK_COMMIT_AMPLIFICATION: &str = Code::CheckCommitAmplification.as_str();
/// A value whose type cannot be resolved is stored into a concrete typed place.
/// Under strict typing, dynamic data must be converted before typed use.
pub const CHECK_UNTYPED_VALUE: &str = Code::CheckUntypedValue.as_str();
/// A saved key or identity argument's type does not match the key it addresses: a
/// scalar of the wrong type in a keyed lookup, or an identity of a foreign store
/// root spliced into a keyspace. Saved keys are nominally typed, so a
/// key-compatible foreign identity is still rejected. The static counterpart of a
/// key-type fault at lowering.
pub const CHECK_KEY_TYPE: &str = Code::CheckKeyType.as_str();
/// A write to a sequence position the spec proves addresses no node: a
/// statically-known zero or negative position in a 1-based single int-keyed layer.
/// The static counterpart of the absent fault a dynamic non-positive position
/// raises at lowering.
pub const CHECK_SEQUENCE_POSITION: &str = Code::CheckSequencePosition.as_str();
/// A bare name used as a value does not resolve to any binding in scope (a
/// parameter, local, loop or catch binding, or module constant). Under strict
/// typing every value name must be defined.
pub const CHECK_UNRESOLVED_NAME: &str = Code::CheckUnresolvedName.as_str();
/// A dotted or optional (`?.`) field read names no field on a resolved value: a
/// resource-shaped value with no such member, or a value with no fields at all.
pub const CHECK_UNKNOWN_FIELD: &str = Code::CheckUnknownField.as_str();
/// A `^root` names no declared store. A saved root is the only way a saved address
/// exists, so an undeclared or misspelled root is a static resolution error at its
/// span, not a silently dropped function body the runtime later refuses.
pub const CHECK_UNKNOWN_ROOT: &str = Code::CheckUnknownRoot.as_str();
/// A field read names a keyed child layer on a materialized record value. A whole
/// read materializes scalars and unkeyed groups but not keyed child layers, which
/// are reached only through their saved addresses (`^books(id).versions(v)`). The
/// field is declared, so this is distinct from [`CHECK_UNKNOWN_FIELD`].
pub const CHECK_LAYER_NOT_VALUE: &str = Code::CheckLayerNotValue.as_str();
/// A call names a function that is neither a builtin nor a declared function. Only
/// reported for calls in files that are part of a fully parsed project — a library
/// module or a module-less script — so a module excluded by a parse error never
/// false-positives.
pub const CHECK_UNRESOLVED_CALL: &str = Code::CheckUnresolvedCall.as_str();
/// A qualified call (`module::fn`) names a function that exists but is not `pub`,
/// so it is not callable from another module. Distinct from
/// [`CHECK_UNRESOLVED_CALL`]: the name resolves, the visibility does not.
pub const CHECK_PRIVATE_FUNCTION: &str = Code::CheckPrivateFunction.as_str();
/// A cross-module enum reference names an enum that exists but is not `pub`.
/// Distinct from [`CHECK_UNKNOWN_TYPE`] and [`CHECK_UNKNOWN_ENUM_MEMBER`]: the
/// enum resolves, the visibility does not.
pub const CHECK_PRIVATE_ENUM: &str = Code::CheckPrivateEnum.as_str();
/// A `pub fn` names a non-`pub` enum from its own module in a parameter or return
/// type. The enum's values escape through a public signature even though other
/// modules cannot name the type. A warning, not an error: the program is sound,
/// but the API leaks an unnameable type. Distinct from [`CHECK_PRIVATE_ENUM`],
/// which rejects a foreign reference to a private enum.
pub const CHECK_EXPOSED_PRIVATE_ENUM: &str = Code::CheckExposedPrivateEnum.as_str();
/// A bare call names a `pub` function reachable in two or more modules, so the
/// bare name cannot pick one — it must be qualified (`module::fn`). Distinct from
/// [`CHECK_UNRESOLVED_CALL`]: candidates exist, the bare spelling is ambiguous.
pub const CHECK_AMBIGUOUS_CALL: &str = Code::CheckAmbiguousCall.as_str();
/// `nextId(^root)` names a root with no default integer allocation policy: a
/// composite identity, a single non-integer identity key, or a keyless singleton.
/// The default per-root policy is only available for a store with one `int`
/// identity key. The runtime backstops this with `write.next_id_unsupported`; the
/// checker catches it before a run.
pub const CHECK_NEXT_ID_REQUIRES_SINGLE_INT: &str = Code::CheckNextIdRequiresSingleInt.as_str();
/// Two `nextId(^root)` results for the same store are both written as record keys
/// with no intervening write to that store between the two allocations. `nextId`
/// does not advance until a record is written, so both allocations return the same
/// value (`max + 1`) and the second write silently overwrites the first. A warning,
/// not an error: the program is well-typed, but the duplicate-key write is almost
/// never intended. Interleaving the writes (`allocate, write, allocate, write`)
/// makes each id distinct.
pub const CHECK_NEXT_ID_COLLISION: &str = Code::CheckNextIdCollision.as_str();
/// `next`/`prev` is applied to a shape it cannot navigate: a composite
/// multi-key identity record (its identity spans several key levels, not the one
/// `next`/`prev` step over) or an index branch (it inspects identities, with no
/// single key position to seek). The runtime would reject these with an
/// uncatchable `run.unsupported` fault; the checker catches it before a run.
pub const CHECK_NEIGHBOR_UNSUPPORTED: &str = Code::CheckNeighborUnsupported.as_str();
/// `key(id)` is applied to a composite multi-key identity, which has no single
/// scalar key to project. A composite identity is reconstructed as a whole value,
/// never exposed as a tuple of raw key components, so the misuse is rejected
/// rather than leaking a partial key.
pub const CHECK_KEY_REQUIRES_SINGLE_KEY: &str = Code::CheckKeyRequiresSingleKey.as_str();
/// `values`/`entries` is applied to an address-only collection such as a
/// non-unique index branch. These shapes are valid for key traversal, but they do
/// not have materialized values distinct from their keys.
pub const CHECK_COLLECTION_UNSUPPORTED: &str = Code::CheckCollectionUnsupported.as_str();
/// A parsed construct is outside the accepted v0.1 source surface.
pub const CHECK_REJECTED_SURFACE: &str = Code::CheckRejectedSurface.as_str();
/// Accepted catalog metadata is missing, invalid, or lacks an accepted durable
/// identity binding for a source declaration.
pub const CHECK_CATALOG_INTENT: &str = Code::CheckCatalogIntent.as_str();
/// A committed `marrow.lock` cannot be adopted as first-run durable identity: a
/// source declaration would adopt a stable id the lock's append-only ledger has
/// tombstoned. Adoption fails closed — the binding records no adopting proposal — so
/// a retired id is never reissued over a fresh empty store. This is the check-layer
/// surface of the lock-corruption contract; the wire/codec constant
/// [`marrow_catalog::LOCK_CORRUPT`] names the same condition at the projection
/// boundary, coordinated by name across the two layers rather than shared.
pub const CHECK_LOCK_CORRUPT: &str = Code::CheckLockCorrupt.as_str();
/// The program declares a durable surface — a store, enum, or resource that needs committed
/// catalog identity — but the configured store backend has no durable identity. The runtime would
/// reject the program as `run.durable_store_required`; the checker rejects it earlier because the
/// backend is statically known.
pub const CHECK_DURABLE_STORE_REQUIRED: &str = Code::CheckDurableStoreRequired.as_str();
/// An `evolve` step names a target that does not resolve to a catalog-addressable
/// entity: a resource, a resource member, a saved root, a store index, an enum, or
/// an enum member that the current source declares (or, for a rename's source side,
/// an entry the accepted catalog records).
pub const CHECK_EVOLVE_TARGET: &str = Code::CheckEvolveTarget.as_str();
/// An `evolve default` value does not match its target member's type, or an
/// `evolve transform` body does not type-check.
pub const CHECK_EVOLVE_TYPE: &str = Code::CheckEvolveType.as_str();
/// An `evolve transform` violates the transform contract: a non-top-level target, an
/// impure body (a saved read or write, host effect, transaction, or user-function
/// call), or a body that reads its own target or any member another `default` or
/// `transform` rewrites in the same block. A transform must compute a top-level member
/// as a pure function of `old`'s other, decodable members.
pub const CHECK_EVOLVE_TRANSFORM: &str = Code::CheckEvolveTransform.as_str();
/// A `T?` value is used where a `T` is required without one of the resolution
/// forms (`?? default`, `if const`, `exists`, or `?.`). Optionality lives in the
/// value's type, so this fires whenever an optional reaches a non-optional slot.
pub const CHECK_UNRESOLVED_OPTIONAL: &str = Code::CheckUnresolvedOptional.as_str();
/// A `const`/`var` whose sole initializer is the bare `absent` (the empty optional)
/// carries no element type to infer, so the binding must name its optional type
/// (`var v: string? = absent`). `absent` is a concrete empty optional, not an
/// `unknown` deferral, so this is rejected at the binding site rather than silently
/// bound at a type with no element.
pub const CHECK_UNANNOTATED_ABSENT: &str = Code::CheckUnannotatedAbsent.as_str();
/// A numeric literal is provably outside its type's range: an integer literal
/// beyond `i64`, or a decimal literal outside the 34-significant-digit /
/// 34-fractional-place envelope. The runtime would reject it as `run.overflow`.
pub const CHECK_LITERAL_RANGE: &str = Code::CheckLiteralRange.as_str();
/// A string literal or interpolation text segment carries a backslash escape
/// outside the recognized set (`\\`, `\"`, `\n`, `\r`, `\t`), or a trailing lone
/// backslash. The escape text is static, so the checker rejects it before the
/// runtime would.
pub const CHECK_STRING_ESCAPE: &str = Code::CheckStringEscape.as_str();
/// A bytes literal carries a backslash escape outside the recognized set (`\\`,
/// `\"`, `\n`, `\r`, `\t`, `\xNN`), a trailing lone backslash, or a malformed or
/// truncated `\xNN`. The escape text is static, so the checker rejects it before
/// the runtime would.
pub const CHECK_BYTES_ESCAPE: &str = Code::CheckBytesEscape.as_str();
/// A range-for header is malformed: its endpoints are not the same steppable type
/// (int, date, instant), its `by` step does not match the endpoints (a number
/// for int, a duration for date/instant), an instant range omits its required
/// `by` step, the step is a zero or a literal wrong-direction step that would
/// never run, or a step appears on a non-range iterable.
pub const CHECK_RANGE: &str = Code::CheckRange.as_str();
/// A range expression is used as an ordinary value. Ranges only exist as `for`
/// iterables.
pub const CHECK_RANGE_VALUE: &str = Code::CheckRangeValue.as_str();
/// A `throw` operand is known not to be an `Error` value.
pub const CHECK_THROW_TYPE: &str = Code::CheckThrowType.as_str();
/// A qualified name `Enum::member` names a known enum but not one of its members.
pub const CHECK_UNKNOWN_ENUM_MEMBER: &str = Code::CheckUnknownEnumMember.as_str();
/// A bare `Enum::member` literal cannot pick a single enum member. Either the enum
/// owner is exposed by several visible foreign modules, or the member exists under
/// more than one parent in the enum tree (a blessed duplicate, e.g.
/// `Cat::tiger::paw` and `Cat::lion::paw`). Qualifying the enum owner or member path
/// disambiguates it.
pub const CHECK_AMBIGUOUS_MEMBER: &str = Code::CheckAmbiguousMember.as_str();
/// A `match` scrutinee is not an enum value. `match` dispatches on an enum's
/// members, so it requires an enum-typed scrutinee.
pub const CHECK_MATCH_REQUIRES_ENUM: &str = Code::CheckMatchRequiresEnum.as_str();
/// A `match` does not cover every member of its enum. A `match` over an enum is
/// exhaustive over its selectable leaves: each needs an arm (a category arm covers
/// its whole subtree), and there is no wildcard.
pub const CHECK_NONEXHAUSTIVE_MATCH: &str = Code::CheckNonexhaustiveMatch.as_str();
/// A `match` has two arms covering the same member — either a repeated arm or a
/// leaf already covered by an enclosing category arm.
pub const CHECK_DUPLICATE_MATCH_ARM: &str = Code::CheckDuplicateMatchArm.as_str();
/// A category enum member is named in value position. A category groups its
/// descendants and is not selectable; only a concrete member under it can be a
/// value.
pub const CHECK_CATEGORY_NOT_SELECTABLE: &str = Code::CheckCategoryNotSelectable.as_str();
/// A `match` arm names a bare member that exists at more than one level of the
/// enum tree. The arm must resolve to a single member.
pub const CHECK_AMBIGUOUS_MATCH_ARM: &str = Code::CheckAmbiguousMatchArm.as_str();
/// A `match` arm is qualified with the scrutinee enum's own name. Arms are member
/// paths relative to the scrutinee enum, so the enum name is implicit and writing
/// it as a prefix is redundant; the arm is the path with that prefix dropped.
pub const CHECK_SCRUTINEE_QUALIFIED_MATCH_ARM: &str =
    Code::CheckScrutineeQualifiedMatchArm.as_str();
/// The left operand of `is` is not an enum value. `is` tests enum-subtree
/// membership, so it requires an enum-typed left operand.
pub const CHECK_IS_REQUIRES_ENUM: &str = Code::CheckIsRequiresEnum.as_str();
/// The right operand of `is` is not a member of the left operand's enum. `is`
/// tests membership within one enum, so both sides must name the same enum.
pub const CHECK_IS_TYPE: &str = Code::CheckIsType.as_str();
/// A discovered source file could not be read.
pub const IO_READ: &str = Code::IoRead.as_str();
/// The checked read-only expression request was asked to evaluate in a module or
/// program context that does not exist.
pub const CHECK_READ_ONLY_EXPRESSION_CONTEXT: &str = Code::CheckReadOnlyExpressionContext.as_str();
/// A checked read-only expression attempts to write or allocate saved data.
pub const CHECK_READ_ONLY_EXPRESSION_WRITE: &str = Code::CheckReadOnlyExpressionWrite.as_str();
/// A checked read-only expression calls a host-effecting operation.
pub const CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT: &str =
    Code::CheckReadOnlyExpressionHostEffect.as_str();
/// A checked read-only expression would traverse a saved collection without a
/// declared index.
pub const CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP: &str =
    Code::CheckReadOnlyExpressionUnindexedLookup.as_str();
/// Two stores in the project declare the same root. A saved root has one managed
/// owner. This is a schema-model rule, but it is cross-declaration, so the
/// project checker reports it rather than per-store schema compilation.
pub const SCHEMA_DUPLICATE_ROOT_OWNER: &str = Code::SchemaDuplicateRootOwner.as_str();

/// The rejected v0.1 source surface named by a `check.rejected_surface`
/// diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedSurface {
    /// An old saved traversal method shaper was called.
    SavedTraversalMethod { method: String },
}

/// Structured facts for `append` target diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendTargetDiagnostic {
    /// The target path names a keyed group layer instead of a leaf layer.
    GroupLayer,
    /// The target layer's key is not the integer position `append` allocates.
    NonIntKeyedLayer { key_type: MarrowType },
    /// The target is a composite (multi-column) keyed layer, which is a chain of
    /// sub-layers with no single column for `append` to allocate a position in.
    CompositeLayer,
}

/// The target of a scalar-conversion builtin. The language spellings `string` and
/// `ErrorCode` both store as [`ScalarType::Str`], so the source spelling is the
/// conversion identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionTarget {
    Bool,
    Int,
    Str,
    ErrorCode,
    Bytes,
    Date,
    Instant,
    Duration,
    Decimal,
}

/// The source spelling of each conversion builtin, paired with the variant it
/// names. This is the single owner of the spelling vocabulary that `from_name`
/// searches; `spelling` matches the same vocabulary exhaustively, and the
/// round-trip test pins the two directions together.
const CONVERSION_SPELLINGS: &[(&str, ConversionTarget)] = &[
    ("bool", ConversionTarget::Bool),
    ("int", ConversionTarget::Int),
    ("string", ConversionTarget::Str),
    ("ErrorCode", ConversionTarget::ErrorCode),
    ("bytes", ConversionTarget::Bytes),
    ("date", ConversionTarget::Date),
    ("instant", ConversionTarget::Instant),
    ("duration", ConversionTarget::Duration),
    ("decimal", ConversionTarget::Decimal),
];

impl ConversionTarget {
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        CONVERSION_SPELLINGS.iter().map(|(_, target)| *target)
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        CONVERSION_SPELLINGS
            .iter()
            .find(|(spelling, _)| *spelling == name)
            .map(|(_, target)| *target)
    }

    pub(crate) fn spelling(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Str => "string",
            Self::ErrorCode => "ErrorCode",
            Self::Bytes => "bytes",
            Self::Date => "date",
            Self::Instant => "instant",
            Self::Duration => "duration",
            Self::Decimal => "decimal",
        }
    }

    /// The stored scalar this conversion yields. `string` and `ErrorCode` both
    /// store as [`ScalarType::Str`]; their distinct conversion identity lives in
    /// the variant, not the stored scalar.
    pub(crate) fn scalar(self) -> ScalarType {
        match self {
            Self::Bool => ScalarType::Bool,
            Self::Int => ScalarType::Int,
            Self::Str | Self::ErrorCode => ScalarType::Str,
            Self::Bytes => ScalarType::Bytes,
            Self::Date => ScalarType::Date,
            Self::Instant => ScalarType::Instant,
            Self::Duration => ScalarType::Duration,
            Self::Decimal => ScalarType::Decimal,
        }
    }

    pub(crate) fn return_type(self) -> MarrowType {
        MarrowType::Primitive(self.scalar())
    }

    pub(crate) fn accepted_sources(self) -> &'static [ScalarType] {
        use ScalarType::{Bool, Bytes, Date, Decimal, Duration, Instant, Int, Str};
        match self {
            Self::Bool => &[Bool, Int],
            Self::Int => &[Int, Str, Decimal],
            Self::Str => &[Str, Int, Decimal, Bool, Bytes, Date, Instant, Duration],
            Self::ErrorCode => &[Str],
            Self::Bytes => &[Bytes, Str],
            Self::Date => &[Date, Str],
            Self::Instant => &[Instant, Str],
            Self::Duration => &[Duration, Str],
            Self::Decimal => &[Decimal, Int, Str],
        }
    }

    /// Whether this conversion accepts an enum source. Only `string` does: it
    /// renders the member's `Enum::member` spelling.
    pub(crate) fn accepts_enum(self) -> bool {
        matches!(self, Self::Str)
    }

    /// The source types this conversion accepts statically, plus unknown.
    pub fn accepted_source_types(self) -> Vec<MarrowType> {
        self.accepted_sources()
            .iter()
            .copied()
            .map(MarrowType::Primitive)
            .chain([MarrowType::Unknown])
            .collect()
    }

    pub(crate) fn supported_sources_message(self) -> String {
        let mut parts: Vec<String> = self
            .accepted_sources()
            .iter()
            .map(|scalar| format!("`{}`", scalar.name()))
            .collect();
        if self.accepts_enum() {
            parts.push("an enum".to_string());
        }
        parts.push("`unknown`".to_string());
        join_or_list(&parts)
    }

    pub(crate) fn accepts(self, source: &MarrowType) -> bool {
        match source {
            MarrowType::Unknown | MarrowType::Invalid => true,
            MarrowType::Primitive(scalar) => self.accepted_sources().contains(scalar),
            MarrowType::Enum { .. } => self.accepts_enum(),
            MarrowType::Optional(_)
            | MarrowType::Absent
            | MarrowType::Error
            | MarrowType::GroupEntry { .. }
            | MarrowType::Identity(_)
            | MarrowType::LocalTree { .. }
            | MarrowType::Resource(_)
            | MarrowType::Sequence(_) => false,
        }
    }
}

fn join_or_list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Structured facts for a scalar conversion call whose known source type is not
/// one of that conversion target's accepted sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionUnsupportedSourceDiagnostic {
    pub target: ConversionTarget,
    pub source: MarrowType,
    pub accepted_sources: Vec<MarrowType>,
}

/// Which argument a per-argument `check.call_argument` type mismatch is about: a
/// named parameter when one is known (user functions, constructor fields) or a
/// 1-based position otherwise (positional std helpers). The rendered forms
/// (`parameter \`name\`` / `argument N`) distinguish two failures on one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallArgumentSlot {
    Named(String),
    /// The zero-based argument index; rendered 1-based.
    Position(usize),
}

impl CallArgumentSlot {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Named(name) => format!("parameter `{name}`"),
            Self::Position(index) => format!("argument {}", index + 1),
        }
    }
}

/// Why a `check.unresolved_call` was raised. The two forms share their carried
/// name but render distinct prose, so the form is a stored fact rather than one
/// recovered from the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedCallKind {
    /// A bare or qualified call names no builtin and no declared function.
    Function,
    /// A `std::…` path names no standard-library operation.
    StdOperation,
}

/// The single owner of every `check.call_argument` shape. The code is emitted for
/// many distinct argument faults — arity, named-argument, constructor-field,
/// per-argument type, builtin-shape, and saved-collection faults — so each shape is
/// a typed variant here and the renderer selects its template, rather than a stringly
/// payload reconstructing the fault from prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallArgumentFault {
    /// A fixed-arity callee named by `label` (a builtin, std helper, or `assert`)
    /// received the wrong number of arguments.
    Arity {
        label: String,
        expected: usize,
        given: usize,
    },
    /// A user function received the wrong number of arguments.
    FunctionArity {
        callee: String,
        expected: usize,
        given: usize,
    },
    /// `std::assert::equal` compared two scalars of different type.
    AssertEqualMismatch {
        label: String,
        first: MarrowType,
        second: MarrowType,
    },
    /// `std::assert::equal` received a non-scalar operand; `found` is the offending type.
    AssertEqualNonScalar { label: String, found: MarrowType },
    /// A named argument does not match any parameter of the user function.
    UnknownParameter { callee: String, parameter: String },
    /// A user function parameter is supplied more than once.
    DuplicateParameter { callee: String, name: String },
    /// A named-field constructor received a positional argument.
    ConstructorNeedsNamedFields { label: String },
    /// A named-field constructor argument does not match any field.
    UnknownField { label: String, field: String },
    /// A named-field constructor field is supplied more than once.
    DuplicateField { name: String },
    /// A required named-field constructor field is missing.
    RequiredField { label: String, field: String },
    /// A supplied argument's type does not match its parameter.
    ArgumentType {
        label: String,
        slot: CallArgumentSlot,
        expected: MarrowType,
        found: MarrowType,
    },
    /// An `append` value does not match the target element type.
    AppendValue {
        expected: MarrowType,
        found: MarrowType,
    },
    /// An `append` target is not an int-keyed leaf layer.
    AppendTarget(AppendTargetDiagnostic),
    /// A conversion call rejects the known source type.
    ConversionUnsupportedSource(ConversionUnsupportedSourceDiagnostic),
    /// A conversion call's sole argument was named.
    ConversionArgumentNamed { label: String, name: String },
    /// A saved collection was passed to a by-value local-collection parameter of
    /// the callee named `label`.
    SavedCollectionByValue {
        label: String,
        parameter: MarrowType,
    },
    /// `std::assert::isAbsent` received a non-optional value.
    AssertAbsentRequiresOptional,
    /// `exists` was asked to guard a read whose key carries an effect.
    ExistsEffectInKey,
    /// `exists` was applied to an always-present value.
    ExistsAlwaysPresent,
    /// `exists` was applied to something that is not a saved path.
    ExistsRequiresSavedPath,
    /// A string literal in an error-code position is not a dotted lowercase code.
    /// `label` names the offending place, pre-quoted.
    ErrorCodeLiteral { label: String },
    /// `Id` received a named argument.
    IdArgumentsPositional,
    /// `Id` received no arguments.
    IdExpectsRoot,
    /// `Id`'s first argument is not a saved root.
    IdExpectsRootFirst,
    /// `nextId` received a saved path that is not a bare store root.
    NextIdRequiresBareRoot,
    /// `nextId` received a concrete non-saved argument; `found` is its type.
    NextIdRequiresRoot { found: MarrowType },
    /// `key` received a concrete non-identity argument; `found` is its type.
    KeyRequiresIdentity { found: MarrowType },
    /// A saved key lookup or identity constructor received a named argument.
    SavedKeyArgumentsPositional,
}

/// Structured facts for enum-member and enum-match diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumDiagnostic {
    UnknownMember {
        enum_name: String,
        member: String,
        /// Valid full-path forms the named segment could have meant — the qualified
        /// member path through its real parent and/or the bare leaf — when the segment
        /// is a category or a leaf reached at the wrong level. Empty when no concrete
        /// member matches the written tail.
        suggestions: Vec<String>,
    },
    AmbiguousMember {
        enum_name: String,
        label: String,
        candidates: Vec<String>,
        form: AmbiguousMemberForm,
    },
    AmbiguousMatchArm {
        enum_name: String,
        label: String,
        candidates: Vec<String>,
    },
    /// A `match` arm written as `Enum::member` where `Enum` is the scrutinee enum's
    /// own name.
    ScrutineeQualifiedMatchArm {
        enum_name: String,
        /// The arm exactly as written, including the redundant scrutinee-enum prefix.
        written: String,
        /// The corrected arm with that prefix dropped.
        relative: String,
    },
    NonexhaustiveMatch {
        enum_name: String,
        missing: Vec<String>,
    },
    DuplicateMatchArm {
        label: String,
    },
    CategoryNotSelectable {
        /// The full `Enum::category` path the source named, as written.
        path: String,
    },
    /// A `match` scrutinee is not a usable enum value.
    MatchRequiresEnum(MatchScrutinee),
    /// The left operand of `is` is a known non-enum value.
    IsRequiresEnum {
        found: MarrowType,
    },
    /// The right operand of `is` is not a valid member of the left operand's enum.
    IsType(IsTypeFault),
}

/// Which construction produced a `check.ambiguous_member`. The two forms render
/// distinct messages from the same facts, so the form is stored rather than
/// recovered from the rendered prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbiguousMemberForm {
    /// A bare `Enum::member` whose enum owner is exposed by several foreign modules.
    BareForeignOwner,
    /// A duplicated member name reached where a single concrete member is required.
    ValuePosition,
}

/// Why a `match` scrutinee is not a usable enum value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchScrutinee {
    /// The scrutinee's enum is named, but the project declares no such enum.
    UndeclaredEnum { enum_name: String },
    /// The scrutinee has a known non-enum type.
    NonEnum { found: MarrowType },
}

/// Why the right operand of `is` is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsTypeFault {
    /// The right operand does not name a member of the left operand's enum.
    RequiresMember { left_name: String },
    /// The right operand names a member of a different enum than the left operand.
    DifferentEnum {
        left_name: String,
        right_name: String,
    },
}

/// Why a `.field`/child-layer access or a bare value read names a keyed sub-layer
/// rather than a value. The three cases render distinct prose, so the reason is a
/// stored fact rather than one recovered from the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerNotValueReason {
    /// The base is a materialized record value; keyed child layers are not pulled
    /// into it and are reached only through their saved address.
    MaterializedValue,
    /// The base is a saved address whose innermost composite layer is only partially
    /// keyed, so it names an iterable inner sub-layer with key columns still to fill.
    PartialKeyLayer,
    /// A value-read position reads a partially keyed composite layer directly. The
    /// address names an iterable inner sub-layer, so reading it as a scalar would
    /// check clean and fault `run.absent_element`.
    PartialKeyValue,
}

/// The source of a name that participates in a `check.surface_collision`
/// diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCollisionNameKind {
    Builtin,
    Import,
    Const,
    Resource,
    Function,
    Enum,
    Surface,
    GeneratedOperation,
    FieldItem,
    CollectionAlias,
    ActionAlias,
    ComputedReadAlias,
    CreateItem,
    UpdateItem,
    DeleteItem,
}

impl SurfaceCollisionNameKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Import => "import",
            Self::Const => "const",
            Self::Resource => "resource",
            Self::Function => "function",
            Self::Enum => "enum",
            Self::Surface => "surface",
            Self::GeneratedOperation => "generated operation",
            Self::FieldItem => "surface field",
            Self::CollectionAlias => "surface collection alias",
            Self::ActionAlias => "surface action alias",
            Self::ComputedReadAlias => "surface computed read alias",
            Self::CreateItem => "surface create item",
            Self::UpdateItem => "surface update item",
            Self::DeleteItem => "surface delete item",
        }
    }
}

/// Structured facts for a `check.surface_action` diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceActionDiagnostic {
    UnknownFunction { path: String },
    PrivateFunction { path: String },
    AmbiguousFunction { path: String },
    UnsupportedParameter { path: String, parameter: String },
    UnsupportedReturn { path: String },
}

/// Structured facts for a `check.surface_computed_read` diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceComputedReadDiagnostic {
    UnknownFunction { path: String },
    PrivateFunction { path: String },
    AmbiguousFunction { path: String },
    UnsupportedParameter { path: String, parameter: String },
    UnsupportedReturn { path: String },
    Writes { path: String },
    Transactions { path: String },
    HostEffects { path: String },
    Throws { path: String },
    UnindexedCollectionRead { path: String },
}

/// Which surface construct names the rejected store root in a
/// `check.surface_target` diagnostic. A surface's own backing store names the
/// surface in its message; a collection target describes itself generically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceRootOrigin {
    Surface { name: String },
    Collection,
}

/// Structured facts for a `check.surface_target` diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceTargetDiagnostic {
    UnknownStore {
        origin: SurfaceRootOrigin,
        root: String,
    },
    AmbiguousStore {
        origin: SurfaceRootOrigin,
        root: String,
    },
    InvalidStore {
        surface: String,
        root: String,
    },
    InvalidStoreResource {
        surface: String,
        root: String,
        resource: String,
    },
    AmbiguousStoreResource {
        surface: String,
        root: String,
        resource: String,
    },
    ForeignCollectionRoot {
        surface_root: String,
        target_root: String,
    },
    KeylessCollectionRoot {
        root: String,
    },
    UnknownCollectionIndex {
        root: String,
        index: String,
    },
    AmbiguousCollectionIndex {
        root: String,
        index: String,
    },
    InvalidCollectionIndex {
        root: String,
        index: String,
    },
    RangeCollectionUniqueIndex {
        root: String,
        index: String,
    },
    RangeCollectionMissingIdentitySuffix {
        root: String,
        index: String,
    },
    RangeCollectionMissingRangeKey {
        root: String,
        index: String,
    },
    RangeCollectionUnsupportedRangeKey {
        root: String,
        index: String,
        key: String,
    },
}

/// The payload list that produced a `check.surface_field` diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceFieldList {
    Fields,
    Create,
    Update,
}

impl SurfaceFieldList {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Fields => "fields",
            Self::Create => "create",
            Self::Update => "update",
        }
    }
}

/// Why a surface payload field is not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceFieldProblem {
    Unknown,
    Unsupported,
    Invalid,
    Ambiguous,
    NotProjected,
    RequiredNotCreateAddressable,
    /// The item names a store identity key, which every read and page response already
    /// returns automatically under `identity`. Listing it in `fields` is redundant and
    /// rejected.
    IdentityKey,
}

/// Structured facts for a `check.surface_field` diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceFieldDiagnostic {
    pub list: SurfaceFieldList,
    pub name: String,
    pub problem: SurfaceFieldProblem,
}

/// Structured data attached to diagnostics whose consumers need more than the
/// rendered message. Resolution-suppression branches on typed identities: an
/// import names the module it failed to resolve, an unresolved call names the
/// function, and an unknown type names the type spelling. Schema diagnostics carry
/// the schema compiler's structured error kind. Duplicate declarations carry the
/// duplicated name and first declaration span. Surface collisions carry the
/// repeated surface-related name plus the first and later name kinds. Duplicate
/// modules carry the duplicated name and first source file. Module-path diagnostics carry the
/// declared module name and expected path-derived name when one exists.
/// Reserved test-module path diagnostics carry the path-derived module name and
/// reserved segment.
/// Duplicate root ownership names the saved root and first owning file.
/// Rejected-source-surface diagnostics name the rejected surface. Enum diagnostics
/// carry the member or coverage fact. Private enum diagnostics name the
/// inaccessible enum. Call-argument diagnostics carry the typed argument fault
/// (arity, named-argument, constructor-field, per-argument type, builtin-shape, or
/// saved-collection). Interpolation unsupported-source diagnostics
/// carry the source type that interpolation cannot render directly. Reserved
/// catalog path reuse diagnostics carry the reused source identity and reserved
/// stable id. Catalog-intent diagnostics carry structured intent facts.
/// Suggested-index diagnostics carry the source declaration that admits a hidden
/// lookup. Required-absent diagnostics carry the local, resource, root, and
/// missing required field paths. Type mismatch diagnostics carry the expected and
/// found types.
/// Other diagnostics carry [`DiagnosticPayload::None`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DiagnosticPayload {
    /// No resolution identity is attached.
    #[default]
    None,
    /// `cannot resolve import`: the `use` path that named no module.
    UnresolvedImport(String),
    /// `check.unresolved_call`: the call's (possibly qualified) name and which
    /// resolution form it failed. Resolution-suppression keys on the name.
    UnresolvedCall {
        name: String,
        kind: UnresolvedCallKind,
    },
    /// `unknown type`: the structured type the checker did not recognize. Carries
    /// the resolved [`marrow_schema::Type`] (sequence wrappers and named segments
    /// already classified) so resolution-suppression compares it against hidden
    /// type identities through the type model instead of re-parsing source text.
    UnknownType(marrow_schema::Type),
    /// `unknown type`: the named enum annotation has more than one foreign owner.
    /// The unresolved type and ambiguous leaf stay structured instead of being
    /// recovered from rendered diagnostic prose.
    AmbiguousType {
        ty: marrow_schema::Type,
        name: String,
    },
    /// Schema compiler facts for schema diagnostics.
    Schema(marrow_schema::SchemaErrorKind),
    /// `check.duplicate_declaration`: duplicated name and first declaration span.
    DuplicateDeclaration {
        name: String,
        first_span: SourceSpan,
    },
    /// `check.surface_collision`: repeated surface-related name, first occurrence,
    /// and the two namespace sources.
    SurfaceCollision {
        name: String,
        first_kind: SurfaceCollisionNameKind,
        first_span: SourceSpan,
        duplicate_kind: SurfaceCollisionNameKind,
    },
    /// `check.surface_target`: rejected surface store or collection target.
    SurfaceTarget(SurfaceTargetDiagnostic),
    /// `check.surface_field`: rejected surface payload field.
    SurfaceField(SurfaceFieldDiagnostic),
    /// `check.surface_action`: rejected surface action target.
    SurfaceAction(SurfaceActionDiagnostic),
    /// `check.surface_computed_read`: rejected surface computed read target.
    SurfaceComputedRead(SurfaceComputedReadDiagnostic),
    /// `check.duplicate_module`: duplicated module name and first source file.
    DuplicateModule { name: String, first_file: PathBuf },
    /// `check.module_path`: declared name and expected path-derived name.
    ModulePath {
        declared: String,
        expected: Option<String>,
    },
    /// `check.default_entry`: the configured `run.defaultEntry` and why it cannot
    /// run with no arguments.
    DefaultEntry {
        entry: String,
        problem: DefaultEntryProblem,
    },
    /// `check.module_path`: a path-derived test module name contains a reserved segment.
    ReservedTestModulePathSegment {
        module_name: String,
        reserved_segment: String,
    },
    /// `schema.duplicate_root_owner`: saved root and first owning source file.
    DuplicateRootOwner { root: String, first_owner: PathBuf },
    /// `check.rejected_surface`: the rejected source surface.
    RejectedSurface(RejectedSurface),
    /// Enum-member and enum-match diagnostic facts.
    Enum(EnumDiagnostic),
    /// `check.private_enum`: the private enum's fully-qualified name.
    PrivateEnum(String),
    /// `check.exposed_private_enum`: the leaked enum's fully-qualified name and the
    /// public function whose signature exposes it.
    ExposedPrivateEnum { enum_name: String, function: String },
    /// `check.call_argument`: the typed argument fault.
    CallArgument(CallArgumentFault),
    /// `check.private_function`: the inaccessible function's (possibly qualified) name.
    PrivateFunction(String),
    /// `check.ambiguous_call`: the bare call leaf and the modules that expose it.
    AmbiguousCall {
        leaf: String,
        candidates: Vec<String>,
    },
    /// `check.operator_type`: a render surface (print/interpolation) rejects a
    /// known source type.
    RenderUnsupportedSource { source: MarrowType },
    /// `check.catalog_intent`: a source declaration reused a reserved catalog path.
    ReservedCatalogPathReuse {
        source_kind: CatalogEntryKind,
        source_path: String,
        reserved_stable_id: String,
    },
    /// `check.catalog_intent`: a path-only evolve intent names more than one
    /// catalog/source entity and cannot pick a semantic target, or a source
    /// declaration has no durable identity yet and reports how it is recorded.
    CatalogIntent(CatalogIntentDiagnostic),
    /// `check.lock_corrupt`: the adopted catalog id whose reissue the committed ledger
    /// forbids.
    LockCorrupt { reissued_id: String },
    /// `check.evolve_target`: why an `evolve` intent's target does not resolve.
    EvolveTarget(EvolveTargetFault),
    /// `check.evolve_type`: an `evolve default` value's type does not match its target
    /// member. `target` is the member expression as written.
    EvolveDefaultType {
        value: MarrowType,
        target: String,
        member: MarrowType,
    },
    /// `check.evolve_transform`: why an `evolve transform` body is rejected.
    EvolveTransform(EvolveTransformFault),
    /// `check.collection_unsupported`: a lookup names no declared index.
    SuggestedIndex { declaration: String },
    /// `check.unresolved_name`: the bare name that resolved to no binding. Carries the
    /// name so repeated uses of one undeclared name collapse to a single root cause.
    UnresolvedName { name: String },
    /// `check.unknown_field`: a dotted or optional field read names no field on the
    /// resolved value's type.
    UnknownField { field: String },
    /// `check.layer_not_value`: a `.field`/child-layer descends off a base that
    /// names a keyed sub-layer rather than a record value — a keyed child layer read
    /// off a materialized value, or a descent off a partially keyed composite layer.
    LayerNotValue {
        field: String,
        reason: LayerNotValueReason,
    },
    /// `check.required_absent`: a sparse local resource is written to a saved root.
    RequiredAbsent {
        local: String,
        resource: String,
        store_root: String,
        missing_field_paths: Vec<String>,
    },
    /// `check.return_type` or `check.assignment_type`: incompatible known types.
    TypeMismatch {
        expected: MarrowType,
        found: MarrowType,
    },
    /// `check.unknown_root`: a `^root` that names no declared store.
    UnknownRoot { root: String },
    /// `check.uninitialized_var`: a `var` of a no-default type (an enum or a store
    /// identity) declared without an initializer, and the annotation as written.
    UninitializedVar {
        kind: UninitializedVarKind,
        annotation: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogIntentKind {
    RetireTarget,
    RenameSource,
    RenameTarget,
}

/// Which uninitialized-declaration shape a `check.uninitialized_var` is about. An
/// enum and a store identity are the two types with no buildable initial form, and
/// they render distinct prose, so the shape is a stored discriminant rather than one
/// recovered from the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninitializedVarKind {
    Enum,
    Identity,
}

impl UninitializedVarKind {
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Enum => "an enum",
            Self::Identity => "a store identity",
        }
    }
}

/// Why a configured `run.defaultEntry` cannot run with no arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultEntryProblem {
    /// The name resolves to no public entry (including an empty entry name).
    Missing,
    /// The name resolves only to a non-`pub` function.
    Private,
    /// A bare name names a `pub` entry in two or more modules.
    Ambiguous,
    /// The entry resolves but declares parameters, so it cannot run argument-free.
    HasParameters,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPathCandidate {
    pub kind: CatalogEntryKind,
    pub lifecycle: CatalogLifecycle,
    pub stable_id: String,
}

/// How a source entity the accepted catalog does not yet record can be saved. A plain
/// `marrow run` records an additive change — a sparse field, a new resource, store, enum,
/// or group — by auto-applying it; a newly `required` field cannot be auto-applied over an
/// established store, since backfilling existing records is data work an explicit
/// `evolve apply` must drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingRecord {
    RunOrEvolveApply,
    EvolveApply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogIntentDiagnostic {
    AmbiguousPath {
        intent: CatalogIntentKind,
        path: String,
        accepted: Vec<CatalogPathCandidate>,
        source: Vec<CatalogEntryKind>,
    },
    /// A source entity with no durable identity yet, and how a state-establishing flow
    /// records it.
    PendingIdentity {
        path: String,
        records: PendingRecord,
    },
}

/// Why a `check.evolve_target` was raised. The forms render distinct prose from one code,
/// so the form is a stored fact rather than one recovered from the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolveTargetFault {
    /// The target names no catalog-addressable entity kind.
    Unaddressable,
    /// The target names no accepted entry whose identity a carry-forward could preserve.
    UnacceptedCarryForward,
    /// A rename's destination is not declared by the current source.
    RenameTargetUndeclared { to_path: String },
}

/// Why an `evolve transform` body is rejected. The forms render distinct prose from one
/// code, so the form is stored rather than recovered from the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolveTransformFault {
    /// The body performs an effect a pure transform may not.
    Impure { reason: TransformImpurity },
    /// The target is not a top-level saved resource member.
    NonTopLevelMember,
    /// The body reads its own target through `old`.
    ReadsOwnTarget { field: String },
    /// The body reads a member the same evolve block also rewrites.
    ReadsRewrittenMember { field: String },
}

/// The effect that makes an `evolve transform` body impure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformImpurity {
    ReadsSavedData,
    WritesSavedData,
    HostEffect,
    Transaction,
    CallsFunction,
}

/// A problem found while checking a project, located in a specific file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDiagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub file: PathBuf,
    pub message: String,
    pub span: SourceSpan,
    /// Typed facts for diagnostics whose consumers need structured data. Set at
    /// the emit site so consumers do not read the rendered message.
    pub payload: DiagnosticPayload,
}

/// Where a diagnostic points. The only way to give a [`CheckDiagnostic`] built
/// through [`CheckDiagnostic::new`] a location, so a diagnostic can never carry a
/// silently-zeroed span: [`at`](Self::at) takes a real span, and a finding about a
/// whole file with no meaningful position within it is the explicit
/// [`whole_file`](Self::whole_file) constructor, which resolves to the file's start
/// rather than the unplaceable `0:0`. The location shape is a private inner enum, so
/// these two constructors are the sole way to build an anchor — a caller cannot
/// assemble a zeroed span directly and bypass `at`'s guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticAnchor(Anchor);

/// The private location shape behind [`DiagnosticAnchor`]. Module-private so no code
/// outside constructs a variant directly.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Anchor {
    /// A specific span within a source file.
    At { file: PathBuf, span: SourceSpan },
    /// A whole file, when the finding is about the file as a unit and no position
    /// within it is meaningful.
    WholeFile { file: PathBuf },
}

impl DiagnosticAnchor {
    /// Anchor at a real span within `file`.
    pub fn at(file: &Path, span: SourceSpan) -> Self {
        debug_assert!(
            span != SourceSpan::default(),
            "a zeroed span is not a location; use DiagnosticAnchor::whole_file for a \
             finding with no position within the file",
        );
        Self(Anchor::At {
            file: file.to_path_buf(),
            span,
        })
    }

    /// Anchor at a whole file with no meaningful span.
    pub fn whole_file(file: &Path) -> Self {
        Self(Anchor::WholeFile {
            file: file.to_path_buf(),
        })
    }

    /// The single place an anchor becomes the `(file, span)` a [`CheckDiagnostic`]
    /// stores. A whole-file finding points at the file's start so an editor can
    /// place it, never the unplaceable `0:0`.
    fn resolve(self) -> (PathBuf, SourceSpan) {
        match self.0 {
            Anchor::At { file, span } => (file, span),
            Anchor::WholeFile { file } => (file, crate::source_spans::start_of_file()),
        }
    }
}

impl CheckDiagnostic {
    /// Construct a diagnostic from typed inputs: a registry [`Code`] (which owns the
    /// wire string and severity), a typed [`DiagnosticAnchor`], and a typed payload.
    /// The human message is derived from `(code, payload)` by the single renderer, so
    /// no prose is built at the construction site.
    pub fn new(code: Code, anchor: DiagnosticAnchor, payload: DiagnosticPayload) -> Self {
        let (file, span) = anchor.resolve();
        Self {
            code: code.as_str(),
            severity: severity_of(code),
            file,
            message: crate::diagnostic_render::render_message(code, &payload),
            span,
            payload,
        }
    }

    /// An error diagnostic with no typed payload. The single owner of the
    /// `Severity::Error` and owned-file defaults; attach structured facts with
    /// [`with_payload`](Self::with_payload).
    pub fn error(
        code: &'static str,
        file: &Path,
        span: SourceSpan,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: message.into(),
            span,
            payload: DiagnosticPayload::None,
        }
    }

    /// A warning diagnostic with no typed payload. Counterpart to
    /// [`error`](Self::error) for non-fatal findings.
    pub fn warning(
        code: &'static str,
        file: &Path,
        span: SourceSpan,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            file: file.to_path_buf(),
            message: message.into(),
            span,
            payload: DiagnosticPayload::None,
        }
    }

    /// Attach typed facts for consumers that read structured data instead of the
    /// rendered message.
    pub fn with_payload(mut self, payload: DiagnosticPayload) -> Self {
        self.payload = payload;
        self
    }
}

/// The checker severity a registry code renders under. The registry is the single
/// owner of the severity class, so a diagnostic built through
/// [`CheckDiagnostic::new`] never restates whether it is an error or a warning.
fn severity_of(code: Code) -> Severity {
    match code.severity_class() {
        marrow_codes::SeverityClass::Error => Severity::Error,
        marrow_codes::SeverityClass::Warning => Severity::Warning,
    }
}

impl marrow_syntax::Diagnose for CheckDiagnostic {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn severity(&self) -> Severity {
        self.severity
    }
}

/// The result of checking a project: every diagnostic across its files, in
/// file then source order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    pub diagnostics: Vec<CheckDiagnostic>,
}

impl CheckReport {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}
