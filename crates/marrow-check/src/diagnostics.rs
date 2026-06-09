//! The checker's diagnostic vocabulary: stable diagnostic codes, the typed
//! payloads that travel beside a rendered message, and the [`CheckDiagnostic`] /
//! [`CheckReport`] result types.

use std::path::{Path, PathBuf};

use marrow_syntax::{Severity, SourceSpan};

use crate::CatalogEntryKind;
use crate::ScalarType;
use crate::program::MarrowType;

/// A library file declares a module name that does not match its path.
pub const CHECK_MODULE_PATH: &str = "check.module_path";
/// Two library files declare the same module name.
pub const CHECK_DUPLICATE_MODULE: &str = "check.duplicate_module";
/// A project holds more than one module-less file. A project may have at most one
/// single-file script (its entrypoint); every other file must declare a `module`.
pub const CHECK_MULTIPLE_SCRIPTS: &str = "check.multiple_scripts";
/// A name is declared or imported more than once within a single file.
pub const CHECK_DUPLICATE_DECLARATION: &str = "check.duplicate_declaration";
/// A `use` names a module that is neither a project module nor a standard
/// library module.
pub const CHECK_UNRESOLVED_IMPORT: &str = "check.unresolved_import";
/// A type annotation names a type the checker does not recognize.
pub const CHECK_UNKNOWN_TYPE: &str = "check.unknown_type";
/// A `return` carries a value in a function with no return type, or omits one in a
/// value-returning function.
pub const CHECK_RETURN_VALUE: &str = "check.return_value";
/// A value-returning function can reach the end of its body without returning.
pub const CHECK_MISSING_RETURN: &str = "check.missing_return";
/// An operator is applied to operands whose types it does not accept.
pub const CHECK_OPERATOR_TYPE: &str = "check.operator_type";
/// A condition (`if`/`while`) is not a `bool`.
pub const CHECK_CONDITION_TYPE: &str = "check.condition_type";
/// A call passes the wrong number of arguments, or names a parameter that does
/// not exist, for the function it resolves to.
pub const CHECK_CALL_ARGUMENT: &str = "check.call_argument";
/// A `return` value's type does not match the function's declared return type.
pub const CHECK_RETURN_TYPE: &str = "check.return_type";
/// A value's type does not match the binding or place it is stored into (a typed
/// `const`/`var` initializer, or an assignment target).
pub const CHECK_ASSIGNMENT_TYPE: &str = "check.assignment_type";
/// A value whose type cannot be resolved is stored into a concrete typed place.
/// Under strict typing, dynamic data must be converted before typed use.
pub const CHECK_UNTYPED_VALUE: &str = "check.untyped_value";
/// A saved key or identity argument's type does not match the key it addresses: a
/// scalar of the wrong type in a keyed lookup, or an identity of a foreign
/// resource spliced into a keyspace. Saved keys are nominally typed, so a
/// key-compatible foreign identity is still rejected. The static counterpart of a
/// key-type fault at lowering.
pub const CHECK_KEY_TYPE: &str = "check.key_type";
/// A bare name used as a value does not resolve to any binding in scope (a
/// parameter, local, loop or catch binding, or module constant). Under strict
/// typing every value name must be defined.
pub const CHECK_UNRESOLVED_NAME: &str = "check.unresolved_name";
/// A call names a function that is neither a builtin nor a declared function. Only
/// reported for calls in files that are part of a fully parsed project — a library
/// module or a module-less script — so a module excluded by a parse error never
/// false-positives.
pub const CHECK_UNRESOLVED_CALL: &str = "check.unresolved_call";
/// A qualified call (`module::fn`) names a function that exists but is not `pub`,
/// so it is not callable from another module. Distinct from
/// [`CHECK_UNRESOLVED_CALL`]: the name resolves, the visibility does not.
pub const CHECK_PRIVATE_FUNCTION: &str = "check.private_function";
/// A cross-module enum reference names an enum that exists but is not `pub`.
/// Distinct from [`CHECK_UNKNOWN_TYPE`] and [`CHECK_UNKNOWN_ENUM_MEMBER`]: the
/// enum resolves, the visibility does not.
pub const CHECK_PRIVATE_ENUM: &str = "check.private_enum";
/// A bare call names a `pub` function reachable in two or more modules, so the
/// bare name cannot pick one — it must be qualified (`module::fn`). Distinct from
/// [`CHECK_UNRESOLVED_CALL`]: candidates exist, the bare spelling is ambiguous.
pub const CHECK_AMBIGUOUS_CALL: &str = "check.ambiguous_call";
/// `nextId(^root)` names a root with no default integer allocation policy: a
/// composite identity, a single non-integer identity key, or a keyless singleton.
/// The default per-root policy is only available for a store with one `int`
/// identity key. The runtime backstops this with `write.next_id_unsupported`; the
/// checker catches it before a run.
pub const CHECK_NEXT_ID_REQUIRES_SINGLE_INT: &str = "check.next_id_requires_single_int";
/// `next`/`prev` is applied to a shape it cannot navigate: a composite
/// multi-key identity record (its identity spans several key levels, not the one
/// `next`/`prev` step over) or an index branch (it inspects identities, with no
/// single key position to seek). The runtime would reject these with an
/// uncatchable `run.unsupported` fault; the checker catches it before a run.
pub const CHECK_NEIGHBOR_UNSUPPORTED: &str = "check.neighbor_unsupported";
/// `values`/`entries` is applied to an address-only collection such as a
/// non-unique index branch. These shapes are valid for key traversal, but they do
/// not have materialized values distinct from their keys.
pub const CHECK_COLLECTION_UNSUPPORTED: &str = "check.collection_unsupported";
/// A parsed construct is outside the accepted v0.1 source surface.
pub const CHECK_REJECTED_SURFACE: &str = "check.rejected_surface";
/// Accepted catalog metadata is missing, invalid, or lacks an accepted durable
/// identity binding for a source declaration.
pub const CHECK_CATALOG_INTENT: &str = "check.catalog_intent";
/// An `evolve` step names a target that does not resolve to a catalog-addressable
/// entity: a resource member, saved root, store index, enum, or enum member that
/// the current source declares (or, for a rename's source side, an entry the
/// accepted catalog records).
pub const CHECK_EVOLVE_TARGET: &str = "check.evolve_target";
/// An `evolve default` value does not match its target member's type, or an
/// `evolve transform` body does not type-check.
pub const CHECK_EVOLVE_TYPE: &str = "check.evolve_type";
/// An `evolve transform` violates the transform contract: a non-top-level target, an
/// impure body (a saved read or write, host effect, transaction, or user-function
/// call), or a body that reads its own target or any member another `default` or
/// `transform` rewrites in the same block. A transform must compute a top-level member
/// as a pure function of `old`'s other, decodable members.
pub const CHECK_EVOLVE_TRANSFORM: &str = "check.evolve_transform";
/// A maybe-present saved read appears in value position without a read-site
/// resolution form such as `??`, `exists(...)`, or optional chaining.
pub const CHECK_BARE_MAYBE_PRESENT_READ: &str = "check.bare_maybe_present_read";
/// A numeric literal is provably outside its type's range: an integer literal
/// beyond `i64`, or a decimal literal outside the 34-significant-digit /
/// 34-fractional-place envelope. The runtime would reject it as `run.overflow`.
pub const CHECK_LITERAL_RANGE: &str = "check.literal_range";
/// A range-for header is malformed: its endpoints are not the same steppable type
/// (int, decimal, date, instant), its `by` step does not match the endpoints
/// (a number for int/decimal, a duration for date/instant), a decimal or instant
/// range omits its required `by` step, the step is a zero or a literal
/// wrong-direction step that would never run, or a step appears on a non-range
/// iterable.
pub const CHECK_RANGE: &str = "check.range";
/// A range expression is used as an ordinary value. Ranges only exist as `for`
/// iterables.
pub const CHECK_RANGE_VALUE: &str = "check.range_value";
/// A `throw` operand is known not to be an `Error` value.
pub const CHECK_THROW_TYPE: &str = "check.throw_type";
/// A `try` block has neither a `catch` nor a `finally` clause.
pub const CHECK_TRY_HANDLER: &str = "check.try_handler";
/// A qualified name `Enum::member` names a known enum but not one of its members.
pub const CHECK_UNKNOWN_ENUM_MEMBER: &str = "check.unknown_enum_member";
/// A bare `Enum::member` literal names a member that exists under more than one
/// parent in the enum tree (a blessed duplicate, e.g. `Cat::tiger::paw` and
/// `Cat::lion::paw`). The bare name cannot pick one, so it is rejected in value and
/// `is` positions; the full path always disambiguates.
pub const CHECK_AMBIGUOUS_MEMBER: &str = "check.ambiguous_member";
/// A `match` scrutinee is not an enum value. `match` dispatches on an enum's
/// members, so it requires an enum-typed scrutinee.
pub const CHECK_MATCH_REQUIRES_ENUM: &str = "check.match_requires_enum";
/// A `match` does not cover every member of its enum. A `match` over an enum is
/// exhaustive over its selectable leaves: each needs an arm (a category arm covers
/// its whole subtree), and there is no wildcard.
pub const CHECK_NONEXHAUSTIVE_MATCH: &str = "check.nonexhaustive_match";
/// A `match` has two arms covering the same member — either a repeated arm or a
/// leaf already covered by an enclosing category arm.
pub const CHECK_DUPLICATE_MATCH_ARM: &str = "check.duplicate_match_arm";
/// A category enum member is named in value position. A category groups its
/// descendants and is not selectable; only a concrete member under it can be a
/// value.
pub const CHECK_CATEGORY_NOT_SELECTABLE: &str = "check.category_not_selectable";
/// A `match` arm names a bare member that exists at more than one level of the
/// enum tree. The arm must resolve to a single member.
pub const CHECK_AMBIGUOUS_MATCH_ARM: &str = "check.ambiguous_match_arm";
/// The left operand of `is` is not an enum value. `is` tests enum-subtree
/// membership, so it requires an enum-typed left operand.
pub const CHECK_IS_REQUIRES_ENUM: &str = "check.is_requires_enum";
/// The right operand of `is` is not a member of the left operand's enum. `is`
/// tests membership within one enum, so both sides must name the same enum.
pub const CHECK_IS_TYPE: &str = "check.is_type";
/// A discovered source file could not be read.
pub const IO_READ: &str = "io.read";
/// Two stores in the project declare the same root. A saved root has one managed
/// owner. This is a schema-model rule, but it is cross-declaration, so the
/// project checker reports it rather than per-store schema compilation.
pub const SCHEMA_DUPLICATE_ROOT_OWNER: &str = "schema.duplicate_root_owner";

/// The rejected v0.1 source surface named by a `check.rejected_surface`
/// diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedSurface {
    /// A saved place was passed through an `inout` argument.
    SavedInout,
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

    pub(crate) fn return_type(self) -> MarrowType {
        let scalar = match self {
            Self::Bool => ScalarType::Bool,
            Self::Int => ScalarType::Int,
            Self::Str | Self::ErrorCode => ScalarType::Str,
            Self::Bytes => ScalarType::Bytes,
            Self::Date => ScalarType::Date,
            Self::Instant => ScalarType::Instant,
            Self::Duration => ScalarType::Duration,
            Self::Decimal => ScalarType::Decimal,
        };
        MarrowType::Primitive(scalar)
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
        parts.push("`unknown`".to_string());
        join_or_list(&parts)
    }

    pub(crate) fn accepts(self, source: &MarrowType) -> bool {
        match source {
            MarrowType::Unknown | MarrowType::Invalid => true,
            MarrowType::Primitive(scalar) => self.accepted_sources().contains(scalar),
            MarrowType::Enum { .. }
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

/// Structured facts for enum-member and enum-match diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumDiagnostic {
    UnknownMember {
        enum_name: String,
        member: String,
    },
    AmbiguousMember {
        enum_name: String,
        label: String,
        candidates: Vec<String>,
    },
    AmbiguousMatchArm {
        enum_name: String,
        label: String,
        candidates: Vec<String>,
    },
    NonexhaustiveMatch {
        enum_name: String,
        missing: Vec<String>,
    },
    DuplicateMatchArm {
        label: String,
    },
    CategoryNotSelectable {
        label: String,
    },
}

/// Structured data attached to diagnostics whose consumers need more than the
/// rendered message. Resolution-suppression branches on typed identities: an
/// import names the module it failed to resolve, an unresolved call names the
/// function, and an unknown type names the type spelling. Schema diagnostics carry
/// the schema compiler's structured error kind. Duplicate declarations carry the
/// duplicated name and first declaration span. Duplicate modules carry the
/// duplicated name and first source file. Module-path diagnostics carry the
/// declared module name and expected path-derived name when one exists.
/// Duplicate root ownership names the saved root and first owning file.
/// Rejected-source-surface diagnostics name the rejected surface. Enum diagnostics
/// carry the member or coverage fact. Private enum diagnostics name the
/// inaccessible enum. Duplicate named arguments carry the repeated argument or
/// field name. Append target diagnostics carry the rejected target shape.
/// Conversion unsupported-source diagnostics carry the target, rejected source,
/// and accepted static sources. Interpolation unsupported-source diagnostics
/// carry the source type that interpolation cannot render directly. Reserved
/// catalog path reuse diagnostics carry the reused source identity and reserved
/// stable id. Type mismatch diagnostics carry the expected and found types.
/// Other diagnostics carry [`DiagnosticPayload::None`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DiagnosticPayload {
    /// No resolution identity is attached.
    #[default]
    None,
    /// `cannot resolve import`: the `use` path that named no module.
    UnresolvedImport(String),
    /// `function … is not defined`: the call's (possibly qualified) name.
    UnresolvedCall(String),
    /// `unknown type`: the structured type the checker did not recognize. Carries
    /// the resolved [`marrow_schema::Type`] (sequence wrappers and named segments
    /// already classified) so resolution-suppression compares it against hidden
    /// type identities through the type model instead of re-parsing source text.
    UnknownType(marrow_schema::Type),
    /// Schema compiler facts for schema diagnostics.
    Schema(marrow_schema::SchemaErrorKind),
    /// `check.duplicate_declaration`: duplicated name and first declaration span.
    DuplicateDeclaration {
        name: String,
        first_span: SourceSpan,
    },
    /// `check.duplicate_module`: duplicated module name and first source file.
    DuplicateModule { name: String, first_file: PathBuf },
    /// `check.module_path`: declared name and expected path-derived name.
    ModulePath {
        declared: String,
        expected: Option<String>,
    },
    /// `schema.duplicate_root_owner`: saved root and first owning source file.
    DuplicateRootOwner { root: String, first_owner: PathBuf },
    /// `check.rejected_surface`: the rejected source surface.
    RejectedSurface(RejectedSurface),
    /// Enum-member and enum-match diagnostic facts.
    Enum(EnumDiagnostic),
    /// `check.private_enum`: the private enum's fully-qualified name.
    PrivateEnum(String),
    /// `check.call_argument`: a named argument or constructor field repeated.
    DuplicateNamedArgument(String),
    /// `check.call_argument`: an `append` target is not an int-keyed leaf layer.
    AppendTarget(AppendTargetDiagnostic),
    /// `check.call_argument`: a conversion call rejects the known source type.
    ConversionUnsupportedSource(ConversionUnsupportedSourceDiagnostic),
    /// `check.operator_type`: interpolation rejects a known source type.
    InterpolationUnsupportedSource { source: MarrowType },
    /// `check.catalog_intent`: a source declaration reused a reserved catalog path.
    ReservedCatalogPathReuse {
        source_kind: CatalogEntryKind,
        source_path: String,
        reserved_stable_id: String,
    },
    /// `check.return_type` or `check.assignment_type`: incompatible known types.
    TypeMismatch {
        expected: MarrowType,
        found: MarrowType,
    },
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

impl CheckDiagnostic {
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
