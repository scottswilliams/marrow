//! The Marrow diagnostic code registry: the single owner of every dotted error
//! code string, its family, documented meaning, and static classification.
//!
//! A [`Code`] variant is the one place a diagnostic code exists. Every crate that
//! emits a code names the variant and renders the wire string through
//! [`Code::as_str`], so a code string is spelled exactly once in the whole
//! toolchain. The reference page `docs/error-codes.md` is generated from this
//! registry by [`generate`]; a drift test keeps the two byte-identical, so the
//! meaning prose lives here as the single source and the page cannot diverge.

mod docs;
pub use docs::generate;

/// The family a code belongs to, named by the first dotted segment of its string.
/// The family fixes the tooling [`Family::kind`] a code reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Family {
    Parse,
    Fmt,
    Check,
    Schema,
    Catalog,
    Doctor,
    Run,
    Value,
    Write,
    Store,
    Io,
    Config,
    Project,
    Data,
    Evolve,
    Test,
    Backup,
    Restore,
    Surface,
}

impl Family {
    /// The first dotted segment codes in this family carry.
    pub const fn segment(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Fmt => "fmt",
            Self::Check => "check",
            Self::Schema => "schema",
            Self::Catalog => "catalog",
            Self::Doctor => "doctor",
            Self::Run => "run",
            Self::Value => "value",
            Self::Write => "write",
            Self::Store => "store",
            Self::Io => "io",
            Self::Config => "config",
            Self::Project => "project",
            Self::Data => "data",
            Self::Evolve => "evolve",
            Self::Test => "test",
            Self::Backup => "backup",
            Self::Restore => "restore",
            Self::Surface => "surface",
        }
    }

    /// The broad `kind` a tooling envelope reports for codes in this family. The
    /// first segment is not always the kind name (`run.*`/`value.*` are
    /// `runtime`), so the mapping is explicit.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Check | Self::Schema => "check",
            Self::Run | Self::Value => "runtime",
            Self::Store => "storage",
            Self::Surface => "surface",
            Self::Io => "io",
            Self::Fmt
            | Self::Catalog
            | Self::Doctor
            | Self::Write
            | Self::Config
            | Self::Project
            | Self::Data
            | Self::Evolve
            | Self::Test
            | Self::Backup
            | Self::Restore => "tooling",
        }
    }
}

/// The severity a code renders under. Most codes are hard failures; a handful of
/// advisories are warnings that leave the command passing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeverityClass {
    Error,
    Warning,
}

/// Whether a code can be caught as an `Error` value inside a running `.mw`
/// program. Recoverable builtin, arithmetic, write, and I/O faults are
/// `Catchable`; fatal runtime backstops are `Fatal`; static, storage, and
/// tooling codes never reach a running program as an `Error` and are
/// `NotApplicable`. Exactly two codes are `Conditional` today, each constructed
/// both ways in the runtime: `run.type` (a recoverable builtin type fault is
/// catchable, an internal type backstop is fatal) and `run.absent_element` (a
/// missing required host value such as an absent environment variable is
/// catchable, while a required stored field or index target missing once a
/// saved address is fixed is fatal invalid attached data). That split is the
/// distinction a later lane separates into per-behavior codes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Catchability {
    Catchable,
    Fatal,
    Conditional,
    NotApplicable,
}

/// Whether a code is emitted by the current build, and how it reaches a user. An
/// `Active` code is emitted and has a public product surface: a CLI or tooling
/// path a developer can reach. An `Internal` code is emitted too, but only as a
/// defense-in-depth fail-closed guard over an invariant the surrounding layers
/// already close, so it has no public product repro — a lower layer classifies
/// every reachable case first. The reference renders internal codes separately
/// from ordinary user-facing diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lifecycle {
    Active,
    Internal,
}

macro_rules! codes {
    ($($variant:ident => $string:expr, $family:ident, $severity:ident, $catch:ident, $life:ident, $meaning:expr);* $(;)?) => {
        /// A diagnostic code: the single typed identity for one dotted error-code
        /// string. Construct the wire string with [`Code::as_str`].
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        pub enum Code {
            $($variant),*
        }

        impl Code {
            /// Every registered code, in `docs/error-codes.md` order.
            pub const ALL: &'static [Code] = &[$(Code::$variant),*];

            /// The canonical dotted string, spelled once here for the whole toolchain.
            pub const fn as_str(self) -> &'static str {
                match self { $(Code::$variant => $string),* }
            }

            /// The family this code belongs to.
            pub const fn family(self) -> Family {
                match self { $(Code::$variant => Family::$family),* }
            }

            /// The severity this code renders under.
            pub const fn severity_class(self) -> SeverityClass {
                match self { $(Code::$variant => SeverityClass::$severity),* }
            }

            /// Whether this code can be caught inside a running program.
            pub const fn catchability(self) -> Catchability {
                match self { $(Code::$variant => Catchability::$catch),* }
            }

            /// Whether the current build emits this code or reserves it.
            pub const fn lifecycle(self) -> Lifecycle {
                match self { $(Code::$variant => Lifecycle::$life),* }
            }

            /// The documented meaning, the single source of the code's reference prose.
            pub const fn meaning(self) -> &'static str {
                match self { $(Code::$variant => $meaning),* }
            }

            /// The registered code for a wire string, if any.
            pub fn from_code(string: &str) -> Option<Code> {
                match string { $($string => Some(Code::$variant),)* _ => None }
            }
        }
    };
}

codes! {
    ParseSyntax => r#"parse.syntax"#, Parse, Error, NotApplicable, Active, r#"The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the `message` says what was expected."#;
    FmtCommentLoss => r#"fmt.comment_loss"#, Fmt, Error, NotApplicable, Active, r#"`marrow fmt` would drop a retained comment while rewriting the source, so the command refuses instead of publishing lossy formatted output."#;
    CheckFailed => r#"check.failed"#, Check, Error, NotApplicable, Active, r#"A project check completed with one or more parse, schema, or check diagnostics. Command boundaries may use this summary code while the detailed diagnostics carry their own codes."#;
    CheckModulePath => r#"check.module_path"#, Check, Error, NotApplicable, Active, r#"A library or test file declares a module name that does not match its path-derived name. A test file may omit the `module` declaration."#;
    CheckDefaultEntry => r#"check.default_entry"#, Check, Error, NotApplicable, Active, r#"The project's `run.defaultEntry` does not name a runnable zero-argument entry: it is missing, private, ambiguous (a bare name in two modules), or declares parameters. A default entry runs with no arguments, so the check rejects it rather than letting it fault at run time."#;
    CheckDuplicateModule => r#"check.duplicate_module"#, Check, Error, NotApplicable, Active, r#"Two library files declare the same module name."#;
    CheckMultipleScripts => r#"check.multiple_scripts"#, Check, Error, NotApplicable, Active, r#"A project holds more than one file without a `module` declaration. A project may have at most one single-file script (its entrypoint); every other file must declare a `module`."#;
    CheckDuplicateDeclaration => r#"check.duplicate_declaration"#, Check, Error, NotApplicable, Active, r#"A name is declared more than once within one scope: a top-level name declared or imported twice in a file, or a local `const`/`var` redeclared in the same block. Shadowing the name in an inner block is allowed. Duplicate names in one `for`-loop head are the same violation."#;
    CheckBuiltinCollision => r#"check.builtin_collision"#, Check, Error, NotApplicable, Active, r#"A module-level declaration reuses a builtin name such as `exists`, `keys`, `print`, or `int`. A single such declaration is rejected on its own, distinct from a redeclaration. A surface declaration that shadows a builtin reports `check.surface_collision` instead."#;
    CheckSurfaceCollision => r#"check.surface_collision"#, Check, Error, NotApplicable, Active, r#"A surface declaration name collides with a module-level or builtin name; a collection, action, or computed-read alias collides with another operation alias or generated `id`, `get`, `create`, `update`, or `delete`; a surface repeats `delete`; or a `fields`, `create`, or `update` payload list repeats a name."#;
    CheckSurfaceTarget => r#"check.surface_target"#, Check, Error, NotApplicable, Active, r#"A surface declaration targets an unknown, ambiguous, or invalid store root; a store whose normalized resource shape is ambiguous or invalid; a foreign store root; a keyless singleton root as a collection; or an unknown, ambiguous, or schema-invalid index on the surface's backing store."#;
    CheckSurfaceField => r#"check.surface_field"#, Check, Error, NotApplicable, Active, r#"A surface `fields`, `create`, or `update` item names an unknown, ambiguous, or schema-invalid field, a non-top-level/non-plain member, or a generated write field that is not part of the declared read projection."#;
    CheckSurfaceAction => r#"check.surface_action"#, Check, Error, NotApplicable, Active, r#"A surface `action` item targets an unknown, ambiguous, or non-public function, or the target function has a parameter or return type outside the active action JSON surface. Bare action targets resolve only in the declaring module; cross-module targets must be qualified and use ordinary import-alias expansion."#;
    CheckSurfaceComputedRead => r#"check.surface_computed_read"#, Check, Error, NotApplicable, Active, r#"A surface `read` item targets an unknown, ambiguous, or non-public function, has a parameter or return type outside the active computed-read JSON surface, or its checked effect closure writes saved data, opens a transaction, performs host effects, throws, or uses an unindexed collection read. Bare read targets resolve only in the declaring module; cross-module targets must be qualified and use ordinary import-alias expansion."#;
    CheckUnresolvedImport => r#"check.unresolved_import"#, Check, Error, NotApplicable, Active, r#"A `use` names a module that is neither a project module nor a standard-library module."#;
    CheckUnknownType => r#"check.unknown_type"#, Check, Error, NotApplicable, Active, r#"A type annotation names a type the checker does not recognize."#;
    CheckRecursiveKeyedEntry => r#"check.recursive_keyed_entry"#, Check, Error, NotApplicable, Active, r#"A typed keyed-entry layer names a resource whose typed keyed-entry layers recursively name the original resource. The current language expands typed entries to a finite saved member shape, so recursive entry shapes fail closed."#;
    CheckReturnValue => r#"check.return_value"#, Check, Error, NotApplicable, Active, r#"A `return` carries a value in a function with no return type, or omits one in a value-returning function."#;
    CheckMissingReturn => r#"check.missing_return"#, Check, Error, NotApplicable, Active, r#"A value-returning function can reach the end of its body without returning."#;
    CheckOperatorType => r#"check.operator_type"#, Check, Error, NotApplicable, Active, r#"An operator is applied to operands whose types it does not accept."#;
    CheckConditionType => r#"check.condition_type"#, Check, Error, NotApplicable, Active, r#"An `if`/`while` condition is not a `bool`, or an `if const` guard is not a saved value read that can be presence-bound."#;
    CheckCallArgument => r#"check.call_argument"#, Check, Error, NotApplicable, Active, r#"A call or constructor passes the wrong number of arguments, names a parameter or key that does not exist, omits a required key, or supplies one more than once."#;
    CheckReturnType => r#"check.return_type"#, Check, Error, NotApplicable, Active, r#"A `return` value's type does not match the function's declared return type."#;
    CheckAssignmentType => r#"check.assignment_type"#, Check, Error, NotApplicable, Active, r#"A value's type does not match the typed binding or assignment target it is stored into."#;
    CheckLossyRoundTrip => r#"check.lossy_round_trip"#, Check, Warning, NotApplicable, Active, r#"Warning: a whole saved-record replacement targets a record shape with keyed child layers, so omitted keyed children will be cleared."#;
    CheckRequiredAbsent => r#"check.required_absent"#, Check, Error, NotApplicable, Active, r#"A straight-line whole saved-root write stores a local resource variable whose required field path was never assigned. Inconclusive paths remain runtime `write.required_absent` checks."#;
    CheckUninitializedVar => r#"check.uninitialized_var"#, Check, Error, NotApplicable, Active, r#"A `var` of a type with no buildable initial form — an enum or a store identity — is declared without an initializer. A scalar var defaults, a resource var builds field by field, and a sequence or keyed-tree var starts empty, but an enum and an identity have no default member and no incremental construction, so they must be given an initial value at the declaration."#;
    CheckCommitAmplification => r#"check.commit_amplification"#, Check, Warning, NotApplicable, Active, r#"Warning: a loop condition or body contains a saved-data write outside an enclosing `transaction`."#;
    CheckUntypedValue => r#"check.untyped_value"#, Check, Error, NotApplicable, Active, r#"A value whose type cannot be resolved (`unknown`) is stored into a concrete typed place."#;
    CheckKeyType => r#"check.key_type"#, Check, Error, NotApplicable, Active, r#"A saved key or identity argument's type does not match the key it addresses: a scalar of the wrong type in a keyed lookup, or an identity of a foreign store root spliced into a keyspace."#;
    CheckSequencePosition => r#"check.sequence_position"#, Check, Error, NotApplicable, Active, r#"A statically-known zero or negative position in a single integer-keyed layer, which is 1-based and so addresses no node: a write to such a position, or an `Id(^store, key)` identity naming a record by such a key. The position folds in the const-int environment, so a literal, a `const` binding, or integer arithmetic over either is caught at check. A guarded read of such a position resolves to absent at run time and is not flagged. The static counterpart of the absent fault a dynamic non-positive position raises."#;
    CheckUnresolvedName => r#"check.unresolved_name"#, Check, Error, NotApplicable, Active, r#"A bare name used as a value resolves to no binding in scope."#;
    CheckUnknownField => r#"check.unknown_field"#, Check, Error, NotApplicable, Active, r#"A dotted or optional (`?.`) field read names no field on a resolved value: a resource-shaped value with no such member, or a value with no fields at all (a scalar, enum, identity, sequence, or keyed map)."#;
    CheckUnknownRoot => r#"check.unknown_root"#, Check, Error, NotApplicable, Active, r#"A `^root` names no declared store. A saved root is the only spelling of a saved address, so an undeclared or misspelled root (`^shelves` for `^books`) is a static resolution error at its span, not a silently dropped function body."#;
    CheckLayerNotValue => r#"check.layer_not_value"#, Check, Error, NotApplicable, Active, r#"A `.field` or child-layer access descends off a base that names a keyed sub-layer rather than a record value. A whole read materializes scalars and unkeyed groups but not keyed child layers, which are reached only through their saved addresses (`^books(id).versions(v)`); likewise a partially keyed composite layer (`^boards(id).cells(row)` of `cells(row, col)`) names an iterable inner sub-layer, so descending a field off it would address durable data with the unfilled column elided. The field is declared, so this is distinct from `check.unknown_field`."#;
    CheckUnresolvedCall => r#"check.unresolved_call"#, Check, Error, NotApplicable, Active, r#"A call names a function that is neither a builtin nor a declared function."#;
    CheckPrivateFunction => r#"check.private_function"#, Check, Error, NotApplicable, Active, r#"A qualified call (`module::fn`) names a function that exists but is not `pub`, so it is not callable from another module. The name resolves; the visibility does not."#;
    CheckAmbiguousCall => r#"check.ambiguous_call"#, Check, Error, NotApplicable, Active, r#"A bare call names a `pub` function reachable in two or more modules, so the bare name cannot pick one — it must be qualified (`module::fn`)."#;
    CheckNextIdRequiresSingleInt => r#"check.next_id_requires_single_int"#, Check, Error, NotApplicable, Active, r#"`nextId(^root)` names a root with no default integer allocation policy (composite identity, a non-integer key, or a keyless singleton). The static counterpart of `write.next_id_unsupported`."#;
    CheckNextIdCollision => r#"check.next_id_collision"#, Check, Warning, NotApplicable, Active, r#"A warning: two `nextId(^root)` results for the same store are both written as record keys with no write to that store between the two allocations. `nextId` returns `max + 1` and does not advance until a record is written, so both calls return the same value and the second write silently overwrites the first. Interleave the writes (`allocate, write, allocate, write`) for distinct ids."#;
    CheckRejectedSurface => r#"check.rejected_surface"#, Check, Error, NotApplicable, Active, r#"Source uses a parsed construct outside the current language, such as old saved traversal method shapers including `.take(...)`, `.window(...)`, and `.resume(...)`. Reserved syntax forms such as `merge`, `lock`, and `~` are parser diagnostics instead."#;
    CheckCatalogIntent => r#"check.catalog_intent"#, Check, Error, NotApplicable, Active, r#"Binding source against the accepted saved-data identity cannot resolve it soundly: proposed declarations whose identities collide, a reserved spelling reused without an `evolve` intent, or an `evolve` intent that cannot carry identity forward — a rename without an accepted entry holding the new canonical path and old alias. A source declaration not yet recorded as accepted identity is informational, not an error: it reports that durable identity is not yet frozen. An additive declaration — a sparse field, a new resource, store, enum, or group — is recorded by the next `marrow run` or `marrow evolve apply`; a newly `required` field added over an established store needs `marrow evolve preview` then `marrow evolve apply` to backfill existing records, since a plain run fences `run.schema_drift`."#;
    CheckLockCorrupt => r#"check.lock_corrupt"#, Check, Error, NotApplicable, Internal, r#"A defense-in-depth adoption guard: the committed `marrow.lock` cannot seed first-run identity for a fresh empty store because a source declaration would adopt a stable id the lock's append-only ledger has retired. Adoption fails closed so a retired id is never reissued. The catalog lock decoder rejects every publicly reachable malformed lock as `catalog.lock_corrupt` first, so this checker guard has no public product repro; it stands as an independent fail-closed gate. Restore or regenerate `marrow.lock` from a valid live store."#;
    CheckLockMissing => r#"check.lock_missing"#, Check, Error, NotApplicable, Active, r#"A `marrow check --locked` failure for CI: the committed `marrow.lock` is absent over a project that has durable shape to lock — any present native store, whether its accepted catalog reads back cleanly or the store is recovery-required after an unclean shutdown — so the gate fails closed rather than passing a project whose lock was never committed or was deleted. Distinct from `check.stale_lock`, which reports a present-but-behind lock. A legitimate first run, which has no durable store yet, raises no condition: an absent lock there is expected and `--locked` still passes."#;
    CheckStaleLock => r#"check.stale_lock"#, Check, Warning, NotApplicable, Active, r#"A non-fatal advisory: the committed `marrow.lock` records a different producing source shape than the current source, so the lock is behind the project. `marrow check` is read-only and cannot regenerate it, so it reports the staleness and still passes; a `run` or `evolve apply` regenerates the lock. `marrow check --locked` treats this condition as a failure for CI."#;
    CheckStaleClient => r#"check.stale_client"#, Check, Warning, NotApplicable, Active, r#"A non-fatal advisory: the project declares a callable `surface` and a `client` output path, but the declared TypeScript client is absent or carries a different `marrow-client-digest` than the current TypeScript client profile and surface. `marrow check` is read-only and cannot rewrite it, so it reports the staleness and still passes; a `run`, `serve` startup, or `evolve apply` rewrites it. `marrow check --locked` treats this condition as a failure for CI. Stale and absent are one condition here, unlike the lock's split `check.stale_lock`/`check.lock_missing` pair."#;
    CheckDurableStoreRequired => r#"check.durable_store_required"#, Check, Error, NotApplicable, Active, r#"The program declares a durable surface (a `resource`, a saved `store`, or an `enum`) but no native durable store is configured. The static counterpart of `run.durable_store_required`."#;
    CheckUnresolvedOptional => r#"check.unresolved_optional"#, Check, Error, NotApplicable, Active, r#"A `T?` value is used where a `T` is required without one of the resolution forms (`?? default`, `if const name = place`, `exists(...)`, or `?.`). Optionality lives in the value's type, so this fires wherever an optional reaches a non-optional slot. A `required` declaration is a validity rule for populated records; it is not a proof that arbitrary saved data is present at a read site."#;
    CheckUnannotatedAbsent => r#"check.unannotated_absent"#, Check, Error, NotApplicable, Active, r#"A `const`/`var` whose sole initializer is the bare `absent` (the empty optional) carries no element type to infer, so the binding must name its optional type (`var v: string? = absent`). `absent` is a concrete empty optional, not an `unknown` deferral, so it is rejected at the binding site rather than silently bound with no element type."#;
    CheckLiteralRange => r#"check.literal_range"#, Check, Error, NotApplicable, Active, r#"A numeric literal, or a `const` value whose constant integer arithmetic overflows, is provably outside its type's range (an integer beyond `i64`, or a decimal outside the 34-digit / 34-place envelope). The static counterpart of the runtime numeric range faults."#;
    CheckStringEscape => r#"check.string_escape"#, Check, Error, NotApplicable, Active, r#"A string literal or interpolation text segment carries a backslash escape outside the recognized set (`\\`, `\"`, `\n`, `\r`, `\t`), or a trailing lone backslash."#;
    CheckBytesEscape => r#"check.bytes_escape"#, Check, Error, NotApplicable, Active, r#"A bytes literal carries a backslash escape outside the recognized set (`\\`, `\"`, `\n`, `\r`, `\t`, `\xNN`), a trailing lone backslash, or a malformed or truncated `\xNN` hex escape."#;
    CheckLoopControlFlow => r#"check.loop_control_flow"#, Check, Error, NotApplicable, Active, r#"A `break`/`continue` is outside any loop."#;
    CheckCatchType => r#"check.catch_type"#, Check, Error, NotApplicable, Active, r#"A `catch` annotation is not `Error`."#;
    CheckThrowType => r#"check.throw_type"#, Check, Error, NotApplicable, Active, r#"A `throw` operand is known not to be an `Error` value."#;
    CheckMatchRequiresEnum => r#"check.match_requires_enum"#, Check, Error, NotApplicable, Active, r#"A `match` scrutinee is not an enum value, or names an enum the project does not declare."#;
    CheckUnknownEnumMember => r#"check.unknown_enum_member"#, Check, Error, NotApplicable, Active, r#"A `match` arm path, or an `Enum::member` reference, walks to no member the enum declares."#;
    CheckDuplicateMatchArm => r#"check.duplicate_match_arm"#, Check, Error, NotApplicable, Active, r#"Two `match` arms cover the same member — a repeated arm, or a leaf already covered by an enclosing category arm."#;
    CheckNonexhaustiveMatch => r#"check.nonexhaustive_match"#, Check, Error, NotApplicable, Active, r#"A `match` over an enum does not cover every selectable leaf; the message names each uncovered leaf by its full path."#;
    CheckAmbiguousMatchArm => r#"check.ambiguous_match_arm"#, Check, Error, NotApplicable, Active, r#"A `match` arm is a bare member name that appears under more than one parent of the enum tree; the message names the qualifying paths to disambiguate."#;
    CheckScrutineeQualifiedMatchArm => r#"check.scrutinee_qualified_match_arm"#, Check, Error, NotApplicable, Active, r#"A `match` arm is qualified with the scrutinee enum's own name (`Status::active`); arms are relative to the scrutinee, so the message names the corrected arm with that prefix dropped (`active`)."#;
    CheckAmbiguousMember => r#"check.ambiguous_member"#, Check, Error, NotApplicable, Active, r#"A bare `Enum::member` literal (in value or `is` position) names a member that appears under more than one parent; the full path (`Enum::parent::member`) disambiguates."#;
    CheckCategoryNotSelectable => r#"check.category_not_selectable"#, Check, Error, NotApplicable, Active, r#"A category enum member is named in value position; only a concrete member under it is selectable."#;
    CheckIsRequiresEnum => r#"check.is_requires_enum"#, Check, Error, NotApplicable, Active, r#"The left operand of `is` is not an enum value."#;
    CheckIsType => r#"check.is_type"#, Check, Error, NotApplicable, Active, r#"The right operand of `is` is not a member of the left operand's enum."#;
    CheckInvalidAssignTarget => r#"check.invalid_assign_target"#, Check, Error, NotApplicable, Active, r#"An assignment target is not a writable place: a non-place expression, a read-only parameter, an immutable binding (a `const`, a loop variable, or an `if const` binding), or a nested write on a local resource whose path does not descend its declared groups — a keyed layer (whose layers are keyed only after the resource is saved), a scalar field, or an undeclared member. An unkeyed nested group field path on a local resource is writable and is not flagged."#;
    CheckNonConstantConst => r#"check.non_constant_const"#, Check, Error, NotApplicable, Active, r#"A `const` initializer is not a constant expression."#;
    CheckLoopMutatesTraversedLayer => r#"check.loop_mutates_traversed_layer"#, Check, Error, NotApplicable, Active, r#"A loop over a saved layer mutates that same layer: a whole keyed-entry write, delete, or append that changes its key set, or a field write at a key that is not provably the loop's key (which may insert or rewrite a sibling). Collect the keys into a local sequence first. The static counterpart of `run.traversal`."#;
    CheckNeighborUnsupported => r#"check.neighbor_unsupported"#, Check, Error, NotApplicable, Active, r#"`next`/`prev` targets a shape with no single key level to seek: a composite-identity store root (traversed whole — iterate it with `for id in ^root` or `reversed ^root`), a composite-identity record, or an index branch."#;
    CheckKeyRequiresSingleKey => r#"check.key_requires_single_key"#, Check, Error, NotApplicable, Active, r#"`key(id)` targets a composite multi-key identity, which has no single scalar key to project. A composite identity is reconstructed as a whole value, never exposed as a tuple of raw key components."#;
    CheckRange => r#"check.range"#, Check, Error, NotApplicable, Active, r#"A range-for header is ill-formed: an endpoint is missing (`0..`, `..10`, `..`), the endpoints are not the same steppable type, or the `by` step does not match them (an `int` for `int`, a positive duration for `date`/`instant`). `instant` requires an explicit step; a zero step, a literal step pointing away from literal endpoints (a dead loop), a negated duration on a temporal range, or a `by` on a non-range iterable is rejected."#;
    CheckRangeValue => r#"check.range_value"#, Check, Error, NotApplicable, Active, r#"A range expression appears outside a `for` iterable. Ranges are loop shapes, not values."#;
    CheckCollectionUnsupported => r#"check.collection_unsupported"#, Check, Error, NotApplicable, Active, r#"A collection operation uses a shape the current language does not support: a `for` or `count` over a value that is a scalar rather than a collection; a `for` over a saved path that names a single stored value; a `reversed` traversal of a range (spell a descending range with its endpoints and `by`); a unique index lookup used as a stream; a generated index branch as a resource member/call chain; or a hidden lookup with no matching declared index. Missing-index diagnostics may render an `add: index ...` remedy."#;
    CheckLoopHeadArity => r#"check.loop_head_arity"#, Check, Error, NotApplicable, Active, r#"A `for` head binds the wrong number of names for its iterable. A saved layer with N key columns accepts 1 name (the outer key) or N+1 names (every key column outermost-first plus the leaf value); a range or scalar accepts 1; a local collection or index branch accepts 1 or 2. Any intermediate count is rejected."#;
    CheckLoopHeadViewCall => r#"check.loop_head_view_call"#, Check, Error, NotApplicable, Active, r#"A `for` head iterable is a direct `keys(...)` or `values(...)` call. Iterate the collection directly: `for k in xs` streams the keys and `for k, v in xs` pairs each key with its value; `keys`/`values` are for building a local sequence in value position."#;
    CheckReadOnlyExpressionContext => r#"check.read_only_expression_context"#, Check, Error, NotApplicable, Active, r#"A checked read-only expression request names a module or program context that does not exist."#;
    CheckReadOnlyExpressionWrite => r#"check.read_only_expression_write"#, Check, Error, NotApplicable, Active, r#"A checked read-only expression would write or allocate saved data, or open a transaction."#;
    CheckReadOnlyExpressionHostEffect => r#"check.read_only_expression_host_effect"#, Check, Error, NotApplicable, Active, r#"A checked read-only expression would call a host-effecting operation."#;
    CheckReadOnlyExpressionUnindexedLookup => r#"check.read_only_expression_unindexed_lookup"#, Check, Error, NotApplicable, Active, r#"A checked read-only expression would traverse a saved collection without a declared index."#;
    CheckPrivateEnum => r#"check.private_enum"#, Check, Error, NotApplicable, Active, r#"A cross-module enum reference names an enum that exists but is not `pub`; the enum resolves, the visibility does not."#;
    CheckExposedPrivateEnum => r#"check.exposed_private_enum"#, Check, Warning, NotApplicable, Active, r#"A warning: a `pub fn` names a non-`pub` enum from its own module in a parameter or return type, so the enum's values escape through a public signature even though other modules cannot name the type. Mark the enum `pub`."#;
    CheckNestingLimit => r#"check.nesting_limit"#, Check, Error, NotApplicable, Active, r#"Source nests expressions or statement blocks deeper than the fixed parser limit (256). Raised by the parser at the offending span so pathologically nested source fails closed rather than overflowing the stack; see [execution limits](language/execution-limits.md)."#;
    CheckEvolveTarget => r#"check.evolve_target"#, Check, Error, NotApplicable, Active, r#"An `evolve` intent names an entity — a resource, a resource member, a saved root, a store index, an enum, or an enum member — that the current source does not declare (or, for a rename's source side, that the accepted catalog does not record)."#;
    CheckEvolveType => r#"check.evolve_type"#, Check, Error, NotApplicable, Active, r#"An `evolve default` value does not match its target member's type, or an `evolve transform` body does not type-check."#;
    CheckEvolveTransform => r#"check.evolve_transform"#, Check, Error, NotApplicable, Active, r#"An `evolve transform` body is ill-formed: it is impure, reads its own target or a member another `default`/`transform` rewrites in the same block, or does not compute a top-level member as a pure function of `old`'s other decodable members."#;
    SchemaDuplicateMember => r#"schema.duplicate_member"#, Schema, Error, NotApplicable, Active, r#"A resource or enum member name collides with another member at the same level."#;
    SchemaCategoryLeaf => r#"schema.category_leaf"#, Schema, Error, NotApplicable, Active, r#"A `category` enum member has no nested members, so it can never be selected or matched."#;
    SchemaParentNotCategory => r#"schema.parent_not_category"#, Schema, Error, NotApplicable, Active, r#"An enum member has nested members but is not a `category`; a grouping node must be marked `category`, since a value selects a concrete member under it."#;
    SchemaDuplicateRootOwner => r#"schema.duplicate_root_owner"#, Schema, Error, NotApplicable, Active, r#"Two stores declare the same saved root (a cross-declaration rule the project checker reports)."#;
    SchemaUnknownInSaved => r#"schema.unknown_in_saved"#, Schema, Error, NotApplicable, Active, r#"A managed saved field or key is typed `unknown`; saved schemas use concrete types."#;
    SchemaOptionalInStoredShape => r#"schema.optional_in_stored_shape"#, Schema, Error, NotApplicable, Active, r#"A stored-shape position — a key, saved field, keyed leaf, or sequence element — is declared optional (`T?`), in a local or saved tree alike. Saved fields and keyed leaves are sparse by default, so their types drop the `?`; a sequence element, which is always present, is rejected the same way."#;
    SchemaKeyMemberCollision => r#"schema.key_member_collision"#, Schema, Error, NotApplicable, Active, r#"Two store members collide in the store namespace: a top-level field or layer shares a name with an identity key, or a declared field shares a name with an index."#;
    SchemaUnknownIndexArg => r#"schema.unknown_index_arg"#, Schema, Error, NotApplicable, Active, r#"An index argument names neither an identity key nor a top-level member."#;
    SchemaUnorderableKey => r#"schema.unorderable_key"#, Schema, Error, NotApplicable, Active, r#"A key (saved or local keyed-collection) has a type with no order-preserving key encoding (currently `decimal`)."#;
    SchemaNonscalarKey => r#"schema.nonscalar_key"#, Schema, Error, NotApplicable, Active, r#"A key (a saved identity key or keyed-layer key parameter, or a local keyed-`var`/keyed-parameter key column) is typed as an identity, a name, or a sequence; keys must be orderable scalars. A local keyed tree follows the same key-type contract as a saved keyed layer. Index arguments also reject sequences, keyed-layer members, and resource-name fields, while top-level enum and `Id(^store)` fields are valid index components."#;
    SchemaNonEnumNamedField => r#"schema.non_enum_named_field"#, Schema, Error, NotApplicable, Active, r#"A saved field or explicit keyed leaf has a named value type that is not a declared enum; these members store scalars, identities, or declared enum values. Direct resource names on keyed fields are typed keyed entries instead."#;
    SchemaIndexMissingIdentityKeys => r#"schema.index_missing_identity_keys"#, Schema, Error, NotApplicable, Active, r#"A non-unique index does not end with all identity keys in declaration order."#;
    SchemaIndexRequiresKeyedRoot => r#"schema.index_requires_keyed_root"#, Schema, Error, NotApplicable, Active, r#"An index is declared on a store with no keyed root."#;
    SchemaNestedIndexArg => r#"schema.nested_index_arg"#, Schema, Error, NotApplicable, Active, r#"An index argument names a field nested through an unkeyed group (not yet resolved by the write planner)."#;
    CatalogInvalid => r#"catalog.invalid"#, Catalog, Error, NotApplicable, Active, r#"An accepted catalog snapshot is malformed, has an unsupported format version, fails digest validation, or carries catalog data that cannot be decoded."#;
    CatalogLockCorrupt => r#"catalog.lock_corrupt"#, Catalog, Error, NotApplicable, Active, r#"The committed `marrow.lock` projection is malformed or fails its structural validation. A corrupt lock refuses the command; it is never silently regenerated, and Marrow never mints fresh identity around it. Regenerate `marrow.lock` from a valid live store, or restore the committed file."#;
    DoctorConfigInvalid => r#"doctor.config_invalid"#, Doctor, Error, NotApplicable, Active, r#"`doctor` could not load `marrow.json`. `data.underlying_code` is usually `config.missing` (no `marrow.json` at the target: not a Marrow project), `config.not_a_project` (the target is a bare file, not a project directory), `io.read`, or `config.invalid`. The printed remedy is derived from the underlying code and names a working next action (initialize the project, point at a project directory, or fix the named field), not a self-referential `marrow doctor` rerun."#;
    DoctorLockCorrupt => r#"doctor.lock_corrupt"#, Doctor, Error, NotApplicable, Active, r#"The committed `marrow.lock` projection exists but is malformed. `data.underlying_code` carries `catalog.lock_corrupt`; delete the corrupt `marrow.lock` so the next run or `evolve apply` re-projects it from the authoritative store (a run over a corrupt lock fails closed without regenerating it), then run the printed `marrow check` command."#;
    DoctorCheckFailed => r#"doctor.check_failed"#, Doctor, Error, NotApplicable, Active, r#"The project check summary reported diagnostics or could not load source. Run the printed `marrow check` command for the full diagnostic report."#;
    DoctorStoreLocked => r#"doctor.store_locked"#, Doctor, Error, NotApplicable, Active, r#"The configured native store exists but a read-only open reported `store.locked`. Close the process holding the store, then rerun the printed `marrow doctor` command."#;
    DoctorStoreRecoveryRequired => r#"doctor.store_recovery_required"#, Doctor, Error, NotApplicable, Active, r#"The configured native store needs a write-capable recovery open before read-only inspection. Run the printed `marrow data recover` command."#;
    DoctorStoreUnavailable => r#"doctor.store_unavailable"#, Doctor, Error, NotApplicable, Active, r#"A read-only store open or metadata read failed with another `store.*` code such as corruption, format-version mismatch, or I/O failure. The finding data carries the underlying store code."#;
    DoctorPopulatedUnstamped => r#"doctor.populated_unstamped"#, Doctor, Error, NotApplicable, Active, r#"The native store holds saved records but carries no catalog commit stamp, so the run path would fence it. Run the printed `marrow evolve apply` command to attach the accepted shape."#;
    DoctorCatalogCollision => r#"doctor.catalog_collision"#, Doctor, Error, NotApplicable, Active, r#"The store and the committed `marrow.lock` record the same epoch but different shape digests, so the lock no longer matches the live store at that epoch. The store wins; regenerate `marrow.lock` by running the project, then commit it."#;
    DoctorStoreLockEpochMismatch => r#"doctor.store_lock_epoch_mismatch"#, Doctor, Error, NotApplicable, Active, r#"The store's accepted epoch and the committed `marrow.lock` epoch differ. The store wins; the finding data carries both epochs so an operator can confirm the store is current and regenerate the lock."#;
    DoctorStaleLock => r#"doctor.stale_lock"#, Doctor, Error, NotApplicable, Active, r#"The committed `marrow.lock` records a different producing source shape digest than the current source, so the lock is stale against the project. The store remains authoritative; regenerate `marrow.lock` by running the project."#;
    DoctorLockMissing => r#"doctor.lock_missing"#, Doctor, Error, NotApplicable, Active, r#"The live store carries accepted saved shape but no committed `marrow.lock` is present, so a CI gate would pass a project whose lock was never committed or was deleted. Regenerate `marrow.lock` with a run or `evolve apply`, then commit it. Mirrors `check.lock_missing`. A uid-only store with no accepted catalog, like an absent store, has nothing to lock and is not flagged."#;
    DoctorFenceMismatch => r#"doctor.fence_mismatch"#, Doctor, Error, NotApplicable, Active, r#"The source/store fence classification does not match the checked project. `data.underlying_code` carries the `run.*` or `store.*` fence code, and `next_command` names the evolve, recovery, or rerun command to use next."#;
    DoctorIntegritySampleFailed => r#"doctor.integrity_sample_failed"#, Doctor, Error, NotApplicable, Active, r#"The bounded saved-data integrity sample found problems or could not complete. Run the printed `marrow data integrity` command for the full read-only report."#;
    RunType => r#"run.type"#, Run, Error, Conditional, Active, r#"A value was used where another type was required. Recoverable builtin/evaluator type faults are catchable; unchecked internal type backstops can be fatal."#;
    RunUnboundName => r#"run.unbound_name"#, Run, Error, Fatal, Active, r#"A name was read or assigned that is not bound in scope. Fatal runtime backstop for unchecked programs."#;
    RunOverflow => r#"run.overflow"#, Run, Error, Catchable, Active, r#"Integer arithmetic overflowed the 64-bit range."#;
    RunDecimalOverflow => r#"run.decimal_overflow"#, Run, Error, Catchable, Active, r#"Decimal arithmetic exceeded the 34-digit / 34-place envelope."#;
    RunTemporalOverflow => r#"run.temporal_overflow"#, Run, Error, Catchable, Active, r#"Temporal arithmetic exceeded the saved RFC3339 instant envelope or the `duration` nanosecond range."#;
    RunDivideByZero => r#"run.divide_by_zero"#, Run, Error, Catchable, Active, r#"Division or remainder by zero."#;
    RunNoEnclosingLoop => r#"run.no_enclosing_loop"#, Run, Error, Fatal, Active, r#"A `break`/`continue` reached the top of a function with no loop to target. Fatal runtime control-flow backstop."#;
    RunUnknownFunction => r#"run.unknown_function"#, Run, Error, Fatal, Active, r#"A call named a function the program does not declare. Fatal runtime backstop for unchecked programs."#;
    RunAmbiguousFunction => r#"run.ambiguous_function"#, Run, Error, Fatal, Active, r#"A bare run entry name matched more than one public function. Qualify the entry as `module::function`."#;
    RunPrivateFunction => r#"run.private_function"#, Run, Error, Fatal, Active, r#"A qualified call or run entry reached a function that exists but is not `pub` to the caller. The runtime backstop for `check.private_function`."#;
    RunEntryArgument => r#"run.entry_argument"#, Run, Error, Fatal, Active, r#"A `marrow run --arg` value or linked-Rust `EntryInvocation` value could not be decoded from the checked entry signature, the descriptor identity no longer matches the current callable ABI, or the parameter surface is outside the supported entry argument surface. Fatal runtime boundary error; exit code `1`."#;
    RunEntrySurface => r#"run.entry_surface"#, Run, Error, Fatal, Active, r#"A run entry parameter or JSON return value is outside the supported entry surface, such as a resource-shaped JSON return. Fatal runtime boundary error; exit code `1`. If a JSON return-surface failure occurs after durable writes commit, the fault JSON also carries `store_stamp` and `committed: true`."#;
    RunNoValue => r#"run.no_value"#, Run, Error, Fatal, Active, r#"A call to a function that returns no value was used where a value is needed. Fatal runtime backstop for unchecked programs."#;
    RunAbsentElement => r#"run.absent_element"#, Run, Error, Conditional, Active, r#"Ordinary maybe-present saved reads must be resolved at the read site (`??` / `if exists` / `if const` / `?.`) or are compile errors; those forms treat ordinary absence as control flow rather than catching a runtime fault. Once a saved address is fixed, missing required data is fatal invalid attached data and is not hidden by `??` or `catch`. Non-saved host APIs may still use this code for catchable absence, such as a missing required environment variable."#;
    RunStore => r#"run.store"#, Run, Error, Fatal, Active, r#"The store reported an error (e.g. corrupt tree-cell payload) during a read. Fatal storage/backend failure while evaluating a read."#;
    RunUnsupported => r#"run.unsupported"#, Run, Error, Fatal, Active, r#"A construct the runtime does not evaluate. Fatal runtime backstop."#;
    RunCapability => r#"run.capability"#, Run, Error, Fatal, Active, r#"A host capability a builtin needs (e.g. the clock for `std::clock::now`) was not provided to this run. Fatal host/tooling failure."#;
    RunTransactionHostEffect => r#"run.transaction_host_effect"#, Run, Error, Fatal, Active, r#"A rollback-sensitive host effect (`print`, `std::log::*`, `std::io::writeText`, `std::io::writeBytes`) was attempted inside a `transaction`. Host effects cannot be rolled back, so the effect is rejected before it runs; move it outside the transaction. A structural program error: uncatchable."#;
    RunAssertion => r#"run.assertion"#, Run, Error, Catchable, Active, r#"A `std::assert::*` assertion did not hold. `marrow test` reports these as located test failures."#;
    RunUncaughtError => r#"run.uncaught_error"#, Run, Error, Fatal, Active, r#"An `Error` raised by `throw` reached the top of a function with no `catch`. The original code travels in text messages (e.g. `[io.read]`) and in run JSON envelopes as `diagnostics[0].data.code`."#;
    RunTraversal => r#"run.traversal"#, Run, Error, Fatal, Active, r#"A write, delete, or append changed the saved layer a loop was actively traversing. Fatal dynamic counterpart of `check.loop_mutates_traversed_layer`."#;
    RunDepth => r#"run.depth"#, Run, Error, Fatal, Active, r#"Function-call nesting exceeded the fixed call-depth budget (256). Located at the offending call site and reports the callee name, budget, and observed attempted depth, so runaway or unbounded recursion fails closed rather than overflowing the stack; see [execution limits](language/execution-limits.md)."#;
    RunNoEntry => r#"run.no_entry"#, Run, Error, Fatal, Active, r#"`marrow run` found no entry: no `--entry` was given and `marrow.json` sets no `run.defaultEntry`."#;
    RunDurableStoreRequired => r#"run.durable_store_required"#, Run, Error, Fatal, Active, r#"A command needs a native durable store to establish accepted durable identity, but no native durable store is configured."#;
    RunDryRunIsolation => r#"run.dry_run_isolation"#, Run, Error, Fatal, Active, r#"Dry-run execution exhausted attempts to allocate a unique temporary store directory."#;
    RunStoreEvolved => r#"run.store_evolved"#, Run, Error, Fatal, Active, r#"An already-bound program is fenced because the store advanced past the catalog epoch that program accepted: a concurrent run or `marrow evolve apply` stamped a newer epoch under a long-running binding. Recompile or upgrade against the current accepted catalog. A fresh command instead rebinds against the store's current snapshot and reports same-epoch `run.schema_drift`, so this fence surfaces through a linked, long-running runtime rather than a fresh CLI over old source. Fenced before any execution; the store is unchanged."#;
    RunStoreBehind => r#"run.store_behind"#, Run, Error, Fatal, Active, r#"The store is older than the accepted catalog. On a plain `run`, the store predates this program's catalog: run `marrow evolve apply` first. On an `evolve apply`, the local store is behind the committed `marrow.lock` by more than a single catch-up step, so applying would regress the committed lock: reconcile the local store with the team's up-to-date store (pull or rebuild it to match the committed lock) instead of re-running apply. Fenced before any execution; the store is unchanged."#;
    RunSchemaDrift => r#"run.schema_drift"#, Run, Error, Fatal, Active, r#"The store was stamped under a different schema at the same catalog epoch: its recorded source digest does not match the durable shape this binary expects. Run `marrow evolve preview` to inspect the required repair or `marrow evolve apply` to commit it. Fenced before any execution; the store is unchanged."#;
    RunEngineProfile => r#"run.engine_profile"#, Run, Error, Fatal, Active, r#"The store's engine profile does not match this binary's storage layout. Fenced before any execution; the store is unchanged."#;
    RunStoreUnstamped => r#"run.store_unstamped"#, Run, Error, Fatal, Active, r#"The store holds saved records but carries no catalog commit stamp. Run `marrow evolve preview` to inspect the required work and `marrow evolve apply` to attach the accepted catalog before running. Fenced before any execution; the store is unchanged."#;
    ValueRange => r#"value.range"#, Value, Error, Catchable, Active, r#"A `date` or `instant` reaching the store codec lies outside Marrow's supported calendar range, years 0001-9999. This is a store-boundary integrity guard, not a source-arithmetic fault: every `.mw` temporal path (the `date`/`instant` constructors, `std::clock` parse and `addDays` helpers, and `+`/`-` arithmetic) shares the same 0001-9999 envelope and already raises `run.temporal_overflow` before an out-of-range value can be produced, so no ordinary checked program reaches this code. It fires only if a value that bypasses those bounds reaches the canonical encoder or key projection."#;
    WriteRequiredAbsent => r#"write.required_absent"#, Write, Error, Catchable, Active, r#"A required field was absent in a whole-resource or whole-entry write."#;
    WriteTypeMismatch => r#"write.type_mismatch"#, Write, Error, Catchable, Active, r#"A field value's type does not match the resource schema."#;
    WriteIdentityMismatch => r#"write.identity_mismatch"#, Write, Error, Catchable, Active, r#"The supplied identity keys do not match the store root's identity shape."#;
    WriteInvalidData => r#"write.invalid_data"#, Write, Error, Catchable, Active, r#"Existing stored data needed to plan or maintain a managed write cannot be decoded under the checked schema, such as a malformed value in a generated-index key source."#;
    WriteStore => r#"write.store"#, Write, Error, Catchable, Active, r#"The store reported an error during a write."#;
    WriteUnknownField => r#"write.unknown_field"#, Write, Error, Catchable, Active, r#"A field write names a field the resource does not declare."#;
    WriteUniqueConflict => r#"write.unique_conflict"#, Write, Error, Catchable, Active, r#"A unique index already maps the supplied key(s) to a different identity."#;
    WriteUnknownLayer => r#"write.unknown_layer"#, Write, Error, Catchable, Active, r#"A keyed-layer write names a layer the resource does not declare."#;
    WriteNotALeafLayer => r#"write.not_a_leaf_layer"#, Write, Error, Catchable, Active, r#"A keyed-leaf write targets a group layer."#;
    WriteNotAGroupLayer => r#"write.not_a_group_layer"#, Write, Error, Catchable, Active, r#"A group-entry field write targets a leaf layer."#;
    WriteLayerKeyArity => r#"write.layer_key_arity"#, Write, Error, Catchable, Active, r#"A keyed-layer write supplies the wrong number of layer keys."#;
    WriteIdOverflow => r#"write.id_overflow"#, Write, Error, Catchable, Active, r#"The integer key space is exhausted (`i64::MAX`), so no next identity or position can be allocated."#;
    WriteNextIdUnsupported => r#"write.next_id_unsupported"#, Write, Error, Catchable, Active, r#"`nextId` was asked for a root whose identity shape has no default integer allocation policy. The runtime backstop for `check.next_id_requires_single_int`."#;
    WriteRequiredField => r#"write.required_field"#, Write, Error, Catchable, Active, r#"Deleting a `required` field on its own is rejected outside maintenance."#;
    WriteRequiresMaintenance => r#"write.requires_maintenance"#, Write, Error, Catchable, Active, r#"A whole managed-root delete (`delete ^books`) was attempted without the maintenance capability."#;
    WriteTransactionTooLarge => r#"write.transaction_too_large"#, Write, Error, Catchable, Active, r#"A `transaction` buffered more than 64 MiB of pending write payload. A transaction holds its whole write set in memory until commit, so this fails closed before the buffer exhausts memory. Located at the write that crossed the budget; the aborted transaction commits nothing. Split the atomic write into smaller transactions. See [execution limits](language/execution-limits.md)."#;
    StoreIo => r#"store.io"#, Store, Error, NotApplicable, Active, r#"An I/O operation on a persistent backend failed."#;
    StorePermissionDenied => r#"store.permission_denied"#, Store, Error, NotApplicable, Active, r#"The process lacks read/write access to the store directory or file. The message names the store path; grant access to that directory, then retry."#;
    StoreLocked => r#"store.locked"#, Store, Error, NotApplicable, Active, r#"The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry."#;
    StoreFormatVersion => r#"store.format_version"#, Store, Error, NotApplicable, Active, r#"The store's recorded format version is not the one this build supports."#;
    StoreCorruption => r#"store.corruption"#, Store, Error, NotApplicable, Active, r#"The store file, tree-cell metadata, tree-cell index cell, or accepted catalog table is corrupt and could not be opened or decoded — including a truncated or torn store body and a catalog snapshot whose recomputed digest does not match its stored header."#;
    StoreRecoveryRequired => r#"store.recovery_required"#, Store, Error, NotApplicable, Active, r#"The store was not shut down cleanly, so a read-only open is refused until a write-capable open replays the interrupted commit. Run `marrow data recover` to attempt that open. The recovery is attempted, not guaranteed: the command reports whether the store opened, and a store damaged beyond replay surfaces `store.corruption`."#;
    StoreLimit => r#"store.limit"#, Store, Error, NotApplicable, Active, r#"A Marrow framing layer could not encode a tree-cell metadata or value-codec length above a `u32` field. Backends enforce no key/value size limit."#;
    StoreCursor => r#"store.cursor"#, Store, Error, NotApplicable, Active, r#"A bounded scan cursor does not belong to the scan being resumed."#;
    StoreTransaction => r#"store.transaction"#, Store, Error, NotApplicable, Active, r#"A transaction or snapshot operation was requested in an invalid store state."#;
    StoreReadOnly => r#"store.read_only"#, Store, Error, NotApplicable, Active, r#"A write-capability operation was requested through a read-only store handle."#;
    IoRead => r#"io.read"#, Io, Error, Catchable, Active, r#"A read failed: a project source file or `marrow.json` could not be read, or `std::io::readText`/`readBytes` failed."#;
    IoListen => r#"io.listen"#, Io, Error, NotApplicable, Active, r#"A local listener could not bind, report its bound address, or accept a connection."#;
    IoThread => r#"io.thread"#, Io, Error, NotApplicable, Active, r#"The CLI could not spawn the worker thread it uses for parsing, checking, and running."#;
    IoSignal => r#"io.signal"#, Io, Error, NotApplicable, Active, r#"`marrow serve` could not install its OS shutdown-signal handler, so it refuses to start rather than serve without graceful shutdown."#;
    IoEntropy => r#"io.entropy"#, Io, Error, NotApplicable, Active, r#"The OS entropy source needed to mint a durable store's physical identity (its store UID) was unavailable, so the session fails closed rather than stamp the store with weak identity."#;
    IoWrite => r#"io.write"#, Io, Error, Catchable, Active, r#"`std::io::writeText`/`writeBytes` failed."#;
    ConfigMissing => r#"config.missing"#, Config, Error, NotApplicable, Active, r#"Emitted by `check`/`run`/`doctor`/`fmt` when no `marrow.json` exists at the target directory: the path is not a Marrow project. Run `marrow init <dir>`, or run from a directory that has a `marrow.json`."#;
    ConfigNotAProject => r#"config.not_a_project"#, Config, Error, NotApplicable, Active, r#"The project path is a bare file, not a directory containing `marrow.json`. Pass the project directory, or run from a directory that has a `marrow.json`. Unlike `config.missing`, `marrow init` does not apply: a file cannot be turned into a project in place."#;
    ConfigInvalid => r#"config.invalid"#, Config, Error, NotApplicable, Active, r#"`marrow.json` is malformed JSON, has an unknown key, is missing a required field, or names an unknown backend. A malformed-JSON or unknown-field fault carries its `marrow.json` line and column in `source_span`; validation faults with no single source point carry none."#;
    ConfigDataDir => r#"config.data_dir"#, Config, Error, NotApplicable, Active, r#"The native store `dataDir` directory could not be created: the path is occupied by a non-directory file, a parent denies access, or the filesystem is read-only. Point `dataDir` at a writable directory or remove the file occupying it."#;
    ConfigClientWithoutSurface => r#"config.client_without_surface"#, Config, Warning, NotApplicable, Active, r#"A non-fatal warning: `marrow.json` sets a `client` output path, but the project declares no callable `surface`, so there is nothing to generate. `run`, `serve` startup, and `evolve apply` report it and write no client; either add a `surface` or remove the `client` line."#;
    ProjectSourceRoot => r#"project.source_root"#, Project, Error, NotApplicable, Active, r#"A configured source root could not be walked (e.g. the directory does not exist)."#;
    DataDecode => r#"data.decode"#, Data, Error, NotApplicable, Active, r#"A stored value is not a canonical form of its declared type."#;
    DataKeyType => r#"data.key_type"#, Data, Error, NotApplicable, Active, r#"A stored record key, keyed-layer key, or identity payload key has a scalar type the schema does not declare for that key position (e.g. a string key under an `int` identity)."#;
    DataDanglingRef => r#"data.dangling_ref"#, Data, Error, NotApplicable, Active, r#"A canonical stored `Id(^root)` leaf points to no saved record node in the referenced root. JSON and JSONL include `containing_identity`, `field_catalog_id`, `referenced_root`, and `referenced_identity`; `source_span.path` is display-only."#;
    DataIncomplete => r#"data.incomplete"#, Data, Error, NotApplicable, Active, r#"An existing record or keyed-layer entry is missing an accepted required field. JSON and JSONL include `store_catalog_id`, `record_identity`, `parent_path`, and `missing_member_catalog_id`; `source_span.path` is display-only."#;
    DataOrphan => r#"data.orphan"#, Data, Error, NotApplicable, Active, r#"A stored data cell is under a saved root or member the schema no longer declares; integrity reports repair guidance for source-native evolution or maintenance repair. Derived index cells are never flagged. An actual stored cell whose key does not decode under the tree-cell key grammar is reported as `store.corruption`."#;
    DataUnknownPath => r#"data.unknown_path"#, Data, Error, NotApplicable, Active, r#"A `data get` path parses but the checked schema cannot resolve it to a declared address: it names a saved root or member the schema does not declare, or an identity or member key whose scalar type or arity the schema does not declare for that position. The path is well-formed input the schema cannot resolve, so it is a typed resolution failure rather than a command-line usage error; `source_span.path` echoes the offending path (display-only). A path that does not parse remains a usage error (exit `2`)."#;
    EvolveNoAcceptedCatalog => r#"evolve.no_accepted_catalog"#, Evolve, Error, NotApplicable, Active, r#"Apply was run on a project that declares no saved data, so there is no baseline catalog epoch to advance from."#;
    EvolveRepairRequired => r#"evolve.repair_required"#, Evolve, Error, NotApplicable, Active, r#"The attached data snapshot cannot discharge a required obligation. Repair the data through explicit maintenance/admin code, then run `marrow evolve preview` again."#;
    EvolveDrift => r#"evolve.drift"#, Evolve, Error, NotApplicable, Active, r#"The live source, catalog, store snapshot, engine metadata, affected IDs, store commit, or planned effect counts no longer match the preview witness. JSON envelopes carry `data.drift_kind`: `{"kind":"witness"}`, `{"kind":"store_commit","pinned":...,"found":...}`, or `{"kind":"plan_mismatch","expected":...,"staged":...}`. Rerun `marrow evolve preview`, then rerun `marrow evolve apply`."#;
    EvolveCatalogDrift => r#"evolve.catalog_drift"#, Evolve, Error, NotApplicable, Active, r#"The store's accepted catalog snapshot changed after preview, so the witness was discharged against a catalog the store no longer holds. Apply refuses before writing; rerun `marrow evolve preview`, then rerun `marrow evolve apply`."#;
    EvolveMaintenanceRequired => r#"evolve.maintenance_required"#, Evolve, Error, NotApplicable, Active, r#"A destructive retire was reached without the maintenance gate."#;
    EvolveApprovalRequired => r#"evolve.approval_required"#, Evolve, Error, NotApplicable, Active, r#"A destructive retire needs `--approve-retire <field-path>:<count>` naming the field path and populated count from preview."#;
    EvolveApprovalMismatch => r#"evolve.approval_mismatch"#, Evolve, Error, NotApplicable, Active, r#"The `--approve-retire` counts did not match what the evolution retires. The message names the exact path and count to approve."#;
    EvolveApprovalTargetUnknown => r#"evolve.approval_target_unknown"#, Evolve, Error, NotApplicable, Active, r#"A `--approve-retire` target is not a field path or catalog id in the project. Run `marrow evolve preview <projectdir>` to see the exact path to approve."#;
    EvolveRequiresBackup => r#"evolve.requires_backup"#, Evolve, Error, NotApplicable, Active, r#"A Retire-bearing apply did not name `--backup <path>` or explicit `--no-backup`. Apply refuses before approval checks or evolution work."#;
    EvolveBackupPathManaged => r#"evolve.backup_path_managed"#, Evolve, Error, NotApplicable, Active, r#"`evolve apply --backup` named a managed project artifact or subtree: `marrow.json`, `marrow.lock`, source roots, test paths, or the native data directory/store file. Apply refuses before backup creation or evolution work."#;
    EvolveTransformFaulted => r#"evolve.transform_faulted"#, Evolve, Error, NotApplicable, Active, r#"A checked transform body faulted while running against real data, so apply rolled back."#;
    TestNone => r#"test.none"#, Test, Error, NotApplicable, Active, r#"`marrow test` found no tests; check the `tests` paths in `marrow.json`. Exit code `1`. (Failing tests are reported per test with their own `run.assertion` or other `run.*` code, not a `test.*` code.)"#;
    BackupCatalogSerialization => r#"backup.catalog_serialization"#, Backup, Error, NotApplicable, Active, r#"The accepted catalog section could not be serialized into the backup artifact."#;
    BackupCellTooLarge => r#"backup.cell_too_large"#, Backup, Error, NotApplicable, Active, r#"A data cell frame exceeded the backup format's per-cell size bound."#;
    BackupManifestSerialization => r#"backup.manifest_serialization"#, Backup, Error, NotApplicable, Active, r#"The backup manifest could not be serialized."#;
    BackupStoreUidMissing => r#"backup.store_uid_missing"#, Backup, Error, NotApplicable, Active, r#"The existing store predates the physical store UID stamp. Run or evolve apply with this build to stamp the store before backup."#;
    RestoreFormatVersion => r#"restore.format_version"#, Restore, Error, NotApplicable, Active, r#"The file is not a Marrow backup, or its format version is not the one this build restores."#;
    RestoreCorruptChunk => r#"restore.corrupt_chunk"#, Restore, Error, NotApplicable, Active, r#"The backup's cell stream is truncated or its data checksum does not match the manifest."#;
    RestoreNotEmpty => r#"restore.not_empty"#, Restore, Error, NotApplicable, Active, r#"The target store already holds saved data, generated indexes, or an accepted catalog and the command did not provide a matching `--replace --count N` confirmation. `N` is the live record (entity) count `data stats records:` reports; a count mismatch uses this code, reports the expected and found record counts, and leaves the target unchanged."#;
    RestoreEngineRecompileRequired => r#"restore.engine_recompile_required"#, Restore, Error, NotApplicable, Active, r#"The backup was written under a different engine, layout, or value codec. A cross-engine restore is a future engine recompile."#;
    RestoreSourceMismatch => r#"restore.source_mismatch"#, Restore, Error, NotApplicable, Active, r#"The backup was written from a program whose schema does not match this project. The message prints backup source digest and project source digest."#;
    RestoreCatalogMismatch => r#"restore.catalog_mismatch"#, Restore, Error, NotApplicable, Active, r#"The backup's catalog does not match this project's accepted catalog. The message prints backup catalog epoch/digest and project catalog epoch/digest."#;
    RestoreDataInvalid => r#"restore.data_invalid"#, Restore, Error, NotApplicable, Active, r#"The replayed data does not validate against the project schema, including orphaned managed cells; restore rolls back, and backup-backed read targets refuse the mount."#;
    SurfaceRequest => r#"surface.request"#, Surface, Error, NotApplicable, Active, r#"A request parameter, identity, index argument, generated write field catalog ID, generated write value, empty update patch, action/computed-read argument, or limit cannot decode to the checked surface operation input shape; cursor tokens use `surface.cursor`."#;
    SurfaceAuth => r#"surface.auth"#, Surface, Error, NotApplicable, Active, r#"Remote HTTP authorization failed, or a known write route was requested from a read-only remote serve. The server returns this before reading the request body."#;
    SurfaceAbsent => r#"surface.absent"#, Surface, Error, NotApplicable, Active, r#"A requested record identity is well-formed but no record node exists, or a requested singleton node is absent."#;
    SurfaceCursor => r#"surface.cursor"#, Surface, Error, NotApplicable, Active, r#"A typed cursor boundary or cursor token is malformed, does not decode under its codec, or is well-formed but bound to normalized parameters that do not match the current request."#;
    SurfaceStaleCursor => r#"surface.stale_cursor"#, Surface, Error, NotApplicable, Active, r#"A typed cursor boundary or cursor token is well-formed, but its operation equality tag, profile tag, or store lineage no longer matches the active surface operation facts."#;
    SurfaceAbiMismatch => r#"surface.abi_mismatch"#, Surface, Error, NotApplicable, Active, r#"A generated client or transport request targets a surface ABI or profile slice that is no longer active."#;
    SurfaceInvalidData => r#"surface.invalid_data"#, Surface, Error, NotApplicable, Active, r#"Backing saved data reached by a surface read, create result validation, or update validation cannot be decoded under the checked footprint, including required backing-field absence, malformed materialized values, malformed stored values needed to maintain generated indexes, corrupt traversed identity/key bytes, or an index row whose identity points at no record. Public envelopes are sanitized service faults; repair details stay in operator tooling."#;
    SurfaceLimit => r#"surface.limit"#, Surface, Error, NotApplicable, Active, r#"A well-formed surface operation would exceed its materialization, row, or decoded-byte budget."#;
    SurfaceConflict => r#"surface.conflict"#, Surface, Error, NotApplicable, Active, r#"A generated write conflicts with existing saved data, such as a create targeting an existing record or a unique-index conflict."#;
    SurfaceWrite => r#"surface.write"#, Surface, Error, NotApplicable, Active, r#"A generated write could not be applied after successful request decoding and before commit, excluding conflicts and store/backend faults."#;
    SurfaceAction => r#"surface.action"#, Surface, Error, NotApplicable, Active, r#"A surface action was admitted by operation tag, but entry execution or return rendering failed after request decoding. Public envelopes intentionally hide the underlying `run.*`, source, and store details. Action argument decode failures use `surface.request`."#;
    SurfaceComputed => r#"surface.computed"#, Surface, Error, NotApplicable, Active, r#"A surface computed read was admitted by operation tag, but entry execution or result rendering failed after request decoding. Public envelopes intentionally hide the underlying `run.*`, source, and store details. Computed-read argument decode failures use `surface.request`."#;
    SurfaceStore => r#"surface.store"#, Surface, Error, NotApplicable, Active, r#"The store reported a fault while executing a surface operation."#;
}

impl Code {
    /// The tooling `kind` for this code, derived from its family.
    pub const fn kind(self) -> &'static str {
        self.family().kind()
    }
}

/// The tooling `kind` for any dotted code string, including ones the registry
/// does not name (reserved look-alikes or codes minted outside the toolchain).
/// A registered code resolves through its typed family; an unknown string falls
/// back to first-segment classification so the mapping stays total. Generic
/// string consumers, such as the language server, call this.
pub fn kind_for_code(code: &str) -> &'static str {
    if let Some(code) = Code::from_code(code) {
        return code.kind();
    }
    match code.split('.').next().unwrap_or("") {
        "parse" => "parse",
        "check" | "schema" => "check",
        "run" | "value" => "runtime",
        "store" => "storage",
        "surface" => "surface",
        "io" => "io",
        _ => "tooling",
    }
}

#[cfg(test)]
mod tests {
    use super::{Catchability, Code, Family, Lifecycle, SeverityClass, kind_for_code};

    #[test]
    fn strings_are_unique_and_round_trip() {
        let mut seen = std::collections::BTreeSet::new();
        for &code in Code::ALL {
            assert!(
                seen.insert(code.as_str()),
                "duplicate code string {}",
                code.as_str()
            );
            assert_eq!(Code::from_code(code.as_str()), Some(code));
        }
    }

    #[test]
    fn string_starts_with_family_segment() {
        for &code in Code::ALL {
            let prefix = format!("{}.", code.family().segment());
            assert!(
                code.as_str().starts_with(&prefix),
                "code {} does not start with family segment {}",
                code.as_str(),
                code.family().segment()
            );
        }
    }

    #[test]
    fn kind_for_code_matches_family() {
        for &code in Code::ALL {
            assert_eq!(kind_for_code(code.as_str()), code.kind());
        }
        assert_eq!(kind_for_code("unknown.family"), "tooling");
        assert_eq!(kind_for_code("value.range"), "runtime");
    }

    #[test]
    fn catchability_is_runtime_only() {
        for &code in Code::ALL {
            let runtime = matches!(code.family(), Family::Run | Family::Value | Family::Write)
                || matches!(code, Code::IoRead | Code::IoWrite);
            match code.catchability() {
                Catchability::NotApplicable => {}
                _ => assert!(
                    runtime,
                    "non-runtime code {} recorded as catchable",
                    code.as_str()
                ),
            }
        }
        let conditional: Vec<Code> = Code::ALL
            .iter()
            .copied()
            .filter(|c| c.catchability() == Catchability::Conditional)
            .collect();
        assert_eq!(
            conditional,
            [Code::RunType, Code::RunAbsentElement],
            "the dual-constructed codes are exactly run.type and run.absent_element"
        );
    }

    /// Every registered code renders into the generated reference, in the section
    /// its lifecycle names. Without this, a variant added to the table but dropped
    /// from the generator's layout would vanish from the page while the byte-exact
    /// drift gate stayed green.
    #[test]
    fn generated_reference_covers_every_code_in_its_section() {
        let generated = crate::generate();
        let (active_part, internal_part) = generated
            .split_once(crate::docs::INTERNAL_HEADING)
            .expect("generated reference has the internal-codes section");
        for &code in Code::ALL {
            let row_prefix = format!("| `{}` |", code.as_str());
            let (section, name) = match code.lifecycle() {
                Lifecycle::Active => (active_part, "active"),
                Lifecycle::Internal => (internal_part, "internal"),
            };
            assert!(
                section.contains(&row_prefix),
                "{} is missing from the {name} section of the generated reference",
                code.as_str()
            );
        }
    }

    #[test]
    fn warnings_are_advisories() {
        let warnings: std::collections::BTreeSet<&str> = Code::ALL
            .iter()
            .filter(|c| c.severity_class() == SeverityClass::Warning)
            .map(|c| c.as_str())
            .collect();
        let expected: std::collections::BTreeSet<&str> = [
            "check.lossy_round_trip",
            "check.commit_amplification",
            "check.next_id_collision",
            "check.exposed_private_enum",
            "check.stale_lock",
            "check.stale_client",
            "config.client_without_surface",
        ]
        .into_iter()
        .collect();
        assert_eq!(warnings, expected);
    }
}
