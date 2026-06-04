//! Resolve and check a Marrow project's source.
//!
//! Discover the project's `.mw` files, parse each one, and report parse
//! diagnostics together with module/path resolution, type, and schema problems,
//! producing a resolved [`CheckedProgram`] alongside the diagnostics.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, discover_test_modules};
use marrow_schema::stdlib::{self, ParamType, ReturnType};
pub use marrow_store::value::ScalarType;
use marrow_syntax::{Severity, SourceSpan, parse_source};

pub mod analysis;
pub mod binding;
mod catalog;
mod checks;
pub mod durable_path;
mod enums;
pub mod evolution;
pub mod executable;
pub mod facts;
mod infer;
mod presence;
pub mod program;
mod rejected_surface;
pub mod resolve;
mod rules;
mod typerules;

pub use analysis::{AnalysisSnapshot, AnalyzedFile, analyze_project, scope_at, type_at};
pub use binding::{BindingIndex, RenameSafety, SymbolKind, SymbolRef, build_binding_index};
pub use catalog::program_with_activation_proposal;
pub use durable_path::{
    PathParseError, PathSegment, StoreLeafKind, StorePathClass, classify_store_path, display_path,
    identity_leaf_key_mismatch, parse_path,
};
pub use executable::{
    CheckedArg, CheckedArgMode, CheckedBinaryOp, CheckedBody, CheckedBuiltinCall,
    CheckedCallTarget, CheckedCatchClause, CheckedElseIf, CheckedEnumMemberRef, CheckedEnumRef,
    CheckedExpr, CheckedForBinding, CheckedFunctionRef, CheckedInterpolationPart,
    CheckedLiteralKind, CheckedMatchArm, CheckedParamMode, CheckedResourceConstructor,
    CheckedResourceConstructorField, CheckedResourceRef, CheckedRuntimeValueType,
    CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedLayer,
    CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, CheckedSavedTerminal,
    CheckedStdCall, CheckedStmt, CheckedUnaryOp, checked_activation_root_places,
    checked_saved_root_place,
};
pub use facts::PresenceProofRead;
pub use facts::{
    CheckedFacts, CheckedType, DirectEffectFacts, EnumFact, EnumId, EnumMemberFact, EnumMemberId,
    FunctionFact, FunctionId, FutureEphemeralRootEffect, FutureEphemeralRootEffects, HostEffect,
    LocalFact, LocalId, ModuleFact, ModuleId, ResourceFact, ResourceId, ResourceMemberFact,
    ResourceMemberId, ResourceMemberKind, SavedPlaceEffect, StoreFact, StoreId,
    StoreIdentityKeyFact, StoreIndexFact, StoreIndexId, StoreIndexKeyFact, StoreIndexKeySource,
    StoredValueMeaning,
};
pub use facts::{
    PresenceProofFact, PresenceProofId, PresenceProofPlace, PresenceProofSource,
    PresenceProofStatus,
};
pub use marrow_project::ProjectConfig;
pub use marrow_project::{CatalogEntryKind, CatalogLifecycle};
pub use marrow_schema::{IndexSchema, ResourceSchema, StoreSchema, Type};
use program::TypeNames;
pub use program::{
    CheckedConst, CheckedEntryFunction, CheckedFunction, CheckedModule, CheckedParam,
    CheckedProgram, CheckedRuntimeConst, CheckedRuntimeFunction, CheckedRuntimeModule,
    CheckedRuntimeProgram, EvolveTransform, FileId, MarrowType, ProgramCatalog,
};
pub(crate) use rejected_surface::check_rejected_surface;
pub use resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};

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
/// identity key. The runtime backstops
/// this with `write.next_id_unsupported`; the checker catches it before a run.
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

/// A problem found while checking a project, located in a specific file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDiagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub file: PathBuf,
    pub message: String,
    pub span: SourceSpan,
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

/// An overlay of in-memory source text keyed by file path. Editor tooling fills
/// it with unsaved buffer contents so analysis sees what the user is typing; a
/// path with no overlay entry is read from disk as usual. An empty overlay means
/// "read everything from disk", which is exactly [`check_project`]'s behavior.
#[derive(Debug, Clone, Default)]
pub struct ProjectSources {
    overlays: HashMap<PathBuf, String>,
}

impl ProjectSources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Overlay `source` for `path`, replacing any existing entry, and return the
    /// overlay so calls can be chained when building one up.
    pub fn with(mut self, path: impl Into<PathBuf>, source: impl Into<String>) -> Self {
        self.insert(path, source);
        self
    }

    /// Overlay `source` for `path`, replacing any existing entry.
    pub fn insert(&mut self, path: impl Into<PathBuf>, source: impl Into<String>) {
        self.overlays.insert(path.into(), source.into());
    }

    /// The overlaid text for `path`, or `None` when the path should be read from
    /// disk.
    pub fn get(&self, path: &Path) -> Option<&str> {
        self.overlays.get(path).map(String::as_str)
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.overlays.keys().map(PathBuf::as_path)
    }
}

/// Discover, read, and parse every `.mw` file in the project, collecting parse
/// diagnostics and module/path resolution problems. Fails only when a source
/// root cannot be walked; per-file read errors become diagnostics.
pub fn check_project(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    check_project_with_sources(project_root, config, &ProjectSources::new())
}

/// Like [`check_project`], but analyzing `sources` over disk: any path with an
/// overlay entry is checked from that text instead of being re-read. An empty
/// overlay is identical to [`check_project`].
pub fn check_project_with_sources(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    analysis::analyze_source_project(project_root, config, sources)
        .map(|snapshot| (snapshot.report, snapshot.program))
}

/// Writing the catalog file failed, or the project could not be re-discovered after
/// the write. The caller surfaces the path and the underlying cause.
#[derive(Debug)]
pub enum CommitIdentityError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Discover(DiscoverError),
}

/// Establish a project's baseline durable identity: write its first catalog proposal
/// to the project's catalog file and re-check the project against it. Returns
/// `Ok(None)` when there is nothing to establish — the project already has an accepted
/// catalog, or proposes no catalog at all — so a project past its baseline never
/// churns the file. On success the re-checked report and program reflect the
/// now-committed baseline catalog.
///
/// This is the one production path that writes the catalog. The authorized
/// state-establishing flows — running the program and `evolve apply` — call it when
/// the source checks clean; `check` never does, so it stays read-only.
///
/// It deliberately commits only the baseline. Once a catalog is accepted, every later
/// change to durable identity is an evolution that must flow through `evolve apply`'s
/// witness — its renames, retires, and backfills are stamped into the store under the
/// apply transaction, never silently advanced here. Auto-writing an evolution proposal
/// would reserve retired entries before the witness consumed them, dropping the
/// very entries a retire relies on.
pub fn commit_pending_identity(
    project_root: &Path,
    config: &ProjectConfig,
    program: &CheckedProgram,
) -> Result<Option<(CheckReport, CheckedProgram)>, CommitIdentityError> {
    if program.catalog.accepted_epoch.is_some() {
        return Ok(None);
    }
    let Some(proposal) = &program.catalog.proposal else {
        return Ok(None);
    };
    // A project with no durable surface — a plain script, no resources, stores, or
    // enums — has no identity to freeze. Writing an empty baseline catalog would be
    // pure noise, so leave the project without one.
    if proposal.entries.is_empty() {
        return Ok(None);
    }
    write_accepted_catalog(project_root, config, proposal)?;
    check_project(project_root, config)
        .map(Some)
        .map_err(CommitIdentityError::Discover)
}

/// Write `catalog` to the project's accepted-catalog file, creating its parent
/// directory. This is the single production catalog writer: [`commit_pending_identity`]
/// freezes a baseline through it, and an authorized `evolve apply` advances the
/// accepted file to the activated proposal through it once the store transaction has
/// committed. The byte form is the same pretty JSON both the baseline and an evolution
/// proposal already serialize to.
///
/// The accepted catalog is the project's durable ABI: every binding's stable identity is
/// resolved against it, so a torn write would brick the project. The write is therefore
/// all-or-nothing. The bytes land in a temp file in the same directory and are flushed to
/// disk, then an atomic rename swaps the complete file over the target so a reader sees
/// either the old catalog or the whole new one, never a prefix. The parent directory is
/// flushed last so the rename itself survives a crash. A failure before the rename leaves
/// the prior catalog intact and removes the temp file.
pub fn write_accepted_catalog(
    project_root: &Path,
    config: &ProjectConfig,
    catalog: &marrow_project::CatalogMetadata,
) -> Result<(), CommitIdentityError> {
    let path = project_root.join(&config.accepted_catalog);
    let parent = path.parent().unwrap_or(project_root);
    std::fs::create_dir_all(parent).map_err(|error| CommitIdentityError::Io {
        path: parent.to_path_buf(),
        error,
    })?;

    // The temp name must be unique per write so two writers in this process — concurrent
    // threads sharing the same pid — never target the same temp file and rename a half-written
    // one over the other. The process-wide counter makes every call in this process distinct;
    // the pid keeps it distinct from other processes writing the same project.
    static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temp = path.clone();
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp.{}.{seq}", std::process::id()));
    temp.set_file_name(name);

    let io = |path: &Path, error| CommitIdentityError::Io {
        path: path.to_path_buf(),
        error,
    };
    let write_temp = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(catalog.to_json_pretty().as_bytes())?;
        file.sync_all()
    };
    if let Err(error) = write_temp() {
        let _ = std::fs::remove_file(&temp);
        return Err(io(&temp, error));
    }
    if let Err(error) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(io(&path, error));
    }
    // Flushing the directory persists the rename. A failure here is non-fatal: the bytes
    // and the rename are already durable on common platforms, and the next write will
    // re-establish the entry, so the catalog is never left torn.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The schema of the resource stored at saved root `^root`, if any. Saved roots
/// are project-wide (a `^books` write addresses the one `books` store from any
/// module), so this resolves through the store table and returns only the
/// resource shape for callers that do not need identity keys or indexes.
pub(crate) fn find_resource_schema<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<&'p marrow_schema::ResourceSchema> {
    resolve::resolve_store_by_root(program, root).map(|store| store.resource)
}

pub(crate) fn identity_type_for_store(store: &marrow_schema::StoreSchema) -> MarrowType {
    MarrowType::Identity(store.root.clone())
}

pub(crate) fn resource_type_name(module: &str, resource: &str) -> String {
    if module.is_empty() {
        resource.to_string()
    } else {
        format!("{module}::{resource}")
    }
}

/// Resolve resource references inside a schema type through the checked,
/// module-aware resolver and return the canonical checker type.
pub(crate) fn resolve_resource_schema_type(
    program: &CheckedProgram,
    from_module: &str,
    ty: &marrow_schema::Type,
) -> Option<MarrowType> {
    match ty {
        marrow_schema::Type::Sequence(element) => {
            resolve_resource_schema_type(program, from_module, element)
                .map(|element_type| MarrowType::Sequence(Box::new(element_type)))
        }
        marrow_schema::Type::Named(name) => {
            let segments: Vec<String> = name.split("::").map(str::to_string).collect();
            match resolve(program, from_module, &segments, ResolvableKind::Resource) {
                Resolution::Found(Def {
                    module,
                    item: DefItem::Resource(resource),
                    ..
                }) => Some(MarrowType::Resource(resource_type_name(
                    &module.name,
                    &resource.name,
                ))),
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn resolve_resource_type<'p>(
    program: &'p CheckedProgram,
    name: &str,
) -> Option<(&'p marrow_schema::ResourceSchema, &'p str)> {
    if let Some((module_name, resource_name)) = name.rsplit_once("::") {
        return program
            .modules
            .iter()
            .find(|module| module.name == module_name)
            .and_then(|module| {
                module
                    .resources
                    .iter()
                    .find(|resource| resource.name == resource_name)
                    .map(|resource| (resource, module.name.as_str()))
            });
    }
    program
        .modules
        .iter()
        .find(|module| module.name.is_empty())
        .and_then(|module| {
            module
                .resources
                .iter()
                .find(|resource| resource.name == name)
                .map(|resource| (resource, module.name.as_str()))
        })
}

fn enum_visibility(file: &marrow_syntax::SourceFile) -> HashMap<String, bool> {
    file.declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Enum(decl) => Some((decl.name.clone(), decl.public)),
            _ => None,
        })
        .collect()
}

/// The qualified name of the program module whose source is `file`, if any. The
/// referencing module for resolving a bare enum name in that file's expressions.
fn module_of_file<'p>(program: &'p CheckedProgram, file: &Path) -> Option<&'p str> {
    program
        .modules
        .iter()
        .find(|module| module.source_file == file)
        .map(|module| module.name.as_str())
}

/// Expand a dotted `module` path's leading segment through the file's import
/// aliases, so an enum spelling qualified by a short alias (`c` under
/// `use a::b::c`) names the imported module (`a::b::c`).
pub(crate) fn expand_module_alias(module: &str, aliases: &HashMap<String, Vec<String>>) -> String {
    let segments: Vec<String> = module.split("::").map(str::to_string).collect();
    expand_leading_alias(&segments, aliases).join("::")
}

/// Resolve a call's `segments` to a function, also yielding the [`CheckedModule`]
/// that owns it so the binding index can locate the definition's source file. A
/// small wrapper over the unified [`resolve`]: a bare name resolves in `from_module`,
/// a qualified name in the named module — so a bare cross-module call no longer
/// first-matches a foreign function. Used by the LSP binding index, which carries
/// the referencing module.
pub(crate) fn resolve_function_in_module<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    segments: &[String],
) -> Option<(&'p CheckedModule, &'p CheckedFunction)> {
    match resolve(program, from_module, segments, ResolvableKind::Function) {
        Resolution::Found(Def {
            module,
            item: DefItem::Function(function),
            ..
        }) => Some((module, function)),
        _ => None,
    }
}

/// Whether a name callee is a builtin, std helper, or the `Error` constructor —
/// each dispatched before user functions at runtime, so never a program function.
fn is_builtin_call(segments: &[String]) -> bool {
    match segments {
        // The single-name builtins, grouped by purpose. Each
        // dispatches before user-function resolution at runtime, so none is ever a
        // declared program function.
        [name] => is_builtin_name(name),
        // A `std::module::op` builtin must name a real std module, mirroring
        // import resolution (`is_std_module`/`std_modules`); an unknown submodule
        // is not a builtin, so it is reported like a rejected `use std::bogus`.
        [first, module, _] => first == "std" && std_modules().contains(&module.as_str()),
        _ => false,
    }
}

fn is_builtin_name(name: &str) -> bool {
    matches!(
        name,
        // presence and reads
        "exists"
        // tree traversal
        | "keys" | "values" | "entries" | "count"
        // ordered navigation
        | "reversed" | "next" | "prev"
        // sequence updates and id allocation
        | "append" | "nextId"
        // write and print
        | "write" | "print"
        // error constructor
        | "Error" // conversions: the nine storable scalars, by canonical name.
    ) || ScalarType::from_scalar_name(name).is_some()
}

/// The return type of a single-name data builtin: `exists(path): bool` and
/// `append(layer, value): int`. `nextId` is handled in [`check_next_id`], which
/// has the `^root` argument it needs to type the identity. The absence-default
/// `??` is an operator, not a builtin, and is typed in [`check_coalesce`].
fn builtin_return_type(segments: &[String], _arg_types: &[MarrowType]) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    match name.as_str() {
        "exists" => Some(MarrowType::Primitive(ScalarType::Bool)),
        "append" => Some(MarrowType::Primitive(ScalarType::Int)),
        _ => None,
    }
}

/// The return type of a scalar conversion builtin (`int(x): int`, `string(x):
/// string`, …). The conversion builtins are exactly the nine storable scalars;
/// each validates a dynamically-typed value and yields its named type.
fn conversion_return_type(segments: &[String]) -> Option<MarrowType> {
    let [name] = segments else {
        return None;
    };
    ScalarType::from_scalar_name(name).map(MarrowType::Primitive)
}

/// The descriptor for a `std::module::op` helper, looked up in the shared table.
fn std_op(segments: &[String]) -> Option<&'static stdlib::StdOp> {
    let [first, module, op] = segments else {
        return None;
    };
    (first == "std")
        .then(|| stdlib::lookup(module, op))
        .flatten()
}

/// The declared return type of a value-returning `std::module::op` helper, derived
/// from its descriptor. Void helpers (`std::log`, `std::assert`,
/// `std::io::write*`) and single-name builtins return `None`, leaving the call
/// `Unknown` for the surrounding checks.
fn std_call_return_type(segments: &[String]) -> Option<MarrowType> {
    match std_op(segments)?.ret {
        ReturnType::Scalar(scalar) => Some(MarrowType::Primitive(scalar)),
        ReturnType::Sequence(element) => Some(MarrowType::Sequence(Box::new(
            MarrowType::Primitive(element),
        ))),
        ReturnType::Void => None,
    }
}

/// The positional parameter types of a `std::module::op` helper, in order, derived
/// from its descriptor. `None` for an unknown op, leaving its arguments to the
/// runtime; a `None` slot inside the list marks a non-checked path argument
/// (`assert::absent`).
fn std_call_params(segments: &[String]) -> Option<Vec<Option<MarrowType>>> {
    let op = std_op(segments)?;
    Some(
        op.params
            .iter()
            .map(|param| match param {
                ParamType::Scalar(scalar) => Some(MarrowType::Primitive(*scalar)),
                ParamType::Error => Some(MarrowType::Error),
                ParamType::Path => None,
            })
            .collect(),
    )
}

/// Discover, read, parse, and check a project's test files (the `tests`
/// patterns), producing one checked module per clean test file plus any
/// diagnostics. Test files are scripts outside the source roots, so each is
/// checked module-less and named from its project-relative path
/// (`tests/books_test.mw` → `tests::books_test`). Imports resolve against the
/// already-checked `project` modules, the test modules, and the standard library.
/// Saved-root ownership is not re-checked here: test scripts exercise the
/// project's resources, they do not own saved roots.
pub fn check_tests(
    project_root: &Path,
    config: &ProjectConfig,
    project: &CheckedProgram,
) -> Result<(CheckReport, Vec<CheckedModule>), DiscoverError> {
    check_tests_with_sources(project_root, config, project, &ProjectSources::new())
}

/// Like [`check_tests`], but uses overlaid source text for selected test files and
/// includes overlaid test files that match the configured `tests` patterns even
/// when they are not on disk yet.
pub fn check_tests_with_sources(
    project_root: &Path,
    config: &ProjectConfig,
    project: &CheckedProgram,
    sources: &ProjectSources,
) -> Result<(CheckReport, Vec<CheckedModule>), DiscoverError> {
    let checked = check_tests_with_sources_analysis(
        project_root,
        config,
        project,
        sources,
        TestResolutionSuppression::default(),
    )?;
    Ok((
        checked.report,
        checked.program.modules[checked.test_module_start..].to_vec(),
    ))
}

pub fn check_tests_program(
    project_root: &Path,
    config: &ProjectConfig,
    project: &CheckedProgram,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    check_tests_with_sources_program(project_root, config, project, &ProjectSources::new())
}

pub fn check_tests_with_sources_program(
    project_root: &Path,
    config: &ProjectConfig,
    project: &CheckedProgram,
    sources: &ProjectSources,
) -> Result<(CheckReport, CheckedProgram), DiscoverError> {
    let checked = check_tests_with_sources_analysis(
        project_root,
        config,
        project,
        sources,
        TestResolutionSuppression::default(),
    )?;
    Ok((checked.report, checked.program))
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TestResolutionSuppression {
    hidden_modules: HashSet<String>,
    hidden_types: HashSet<String>,
    hidden_qualified_types: HashSet<String>,
}

impl TestResolutionSuppression {
    pub(crate) fn hide_module(&mut self, name: String) {
        self.hidden_modules.insert(name);
    }

    pub(crate) fn hide_declared_types(
        &mut self,
        parsed: &marrow_syntax::ParsedSource,
        modules: &[String],
    ) {
        for declaration in &parsed.file.declarations {
            let name = match declaration {
                marrow_syntax::Declaration::Resource(resource) => &resource.name,
                marrow_syntax::Declaration::Enum(enum_decl) => &enum_decl.name,
                _ => continue,
            };
            self.hidden_types.insert(name.clone());
            for module in modules {
                self.hidden_qualified_types
                    .insert(format!("{module}::{name}"));
            }
        }
    }

    pub(crate) fn reveal_complete_modules(&mut self, program: &CheckedProgram) {
        for module in &program.modules {
            self.hidden_modules.remove(&module.name);
        }
    }

    fn should_suppress(&self, diagnostic: &CheckDiagnostic) -> bool {
        match diagnostic.code {
            CHECK_UNRESOLVED_IMPORT => {
                diagnostic_message_name(&diagnostic.message, "cannot resolve import `", "`")
                    .is_some_and(|name| self.references_hidden_module_exactly(name))
            }
            CHECK_UNRESOLVED_CALL => {
                diagnostic_message_name(&diagnostic.message, "function `", "` is not defined")
                    .is_some_and(|name| self.references_hidden_module_member(name))
            }
            CHECK_UNKNOWN_TYPE => {
                diagnostic_message_name(&diagnostic.message, "unknown type `", "`")
                    .is_some_and(|name| self.references_hidden_type(name))
            }
            _ => false,
        }
    }

    fn references_hidden_module_exactly(&self, name: &str) -> bool {
        self.hidden_modules.contains(name)
    }

    fn references_hidden_module_member(&self, name: &str) -> bool {
        self.hidden_modules
            .iter()
            .any(|module| name.starts_with(&format!("{module}::")))
    }

    fn references_hidden_type(&self, name: &str) -> bool {
        self.references_hidden_type_text(name.trim())
    }

    fn references_hidden_type_text(&self, name: &str) -> bool {
        if let Some(element) = sequence_type_element(name) {
            return self.references_hidden_type_text(element);
        }
        self.references_hidden_named_type(name)
    }

    fn references_hidden_named_type(&self, name: &str) -> bool {
        if !is_type_name_like(name) {
            return false;
        }
        if name.contains("::") {
            self.hidden_qualified_types.contains(name)
        } else {
            self.hidden_types.contains(name)
        }
    }
}

fn sequence_type_element(text: &str) -> Option<&str> {
    let inner = text.strip_prefix("sequence[")?.strip_suffix(']')?;
    (!inner.is_empty()).then_some(inner)
}

fn is_type_name_like(text: &str) -> bool {
    let mut saw_segment = false;
    for segment in text.split("::") {
        if segment.is_empty()
            || !segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return false;
        }
        saw_segment = true;
    }
    saw_segment
}

fn diagnostic_message_name<'a>(message: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    message.strip_prefix(prefix)?.strip_suffix(suffix)
}

pub(crate) struct CheckedTests {
    pub(crate) report: CheckReport,
    pub(crate) program: CheckedProgram,
    pub(crate) test_module_start: usize,
    pub(crate) files: Vec<analysis::AnalyzedFile>,
}

pub(crate) fn check_tests_with_sources_analysis(
    project_root: &Path,
    config: &ProjectConfig,
    project: &CheckedProgram,
    sources: &ProjectSources,
    mut resolution_suppression: TestResolutionSuppression,
) -> Result<CheckedTests, DiscoverError> {
    let mut files = discover_test_modules(project_root, config)?;
    for path in sources.paths() {
        if let Some(file) = marrow_project::test_module_file(project_root, config, path) {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    let mut report = CheckReport::default();
    let mut modules = Vec::new();
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();
    let mut parsed_sources: HashMap<PathBuf, String> = HashMap::new();

    for file in &files {
        let Some(source) = read_source(&file.path, sources, &mut report.diagnostics) else {
            continue;
        };
        let CheckedFile {
            parsed,
            resources,
            stores,
            enums,
            functions,
            constants,
        } = check_file_source(&file.path, &source, &mut report.diagnostics);

        // A test file is a script: it is named from its path, never from a
        // declared `module`. Duplicate source/test module names are rejected
        // after the clean test module set is known.
        if !parsed.has_errors() {
            modules.push(CheckedModule {
                name: file.module_name.clone().unwrap_or_default(),
                source_file: file.path.clone(),
                span: SourceSpan::default(),
                imports: parsed
                    .file
                    .uses
                    .iter()
                    .map(|use_decl| use_decl.name.clone())
                    .collect(),
                constants,
                functions,
                resources,
                stores,
                enums,
                enum_public: enum_visibility(&parsed.file),
            });
        }
        parsed_sources.insert(file.path.clone(), source);
        parsed_files.push((file, parsed));
    }

    for (file, parsed) in &parsed_files {
        if parsed.has_errors() {
            if let Some(module) = &file.module_name {
                resolution_suppression.hide_declared_types(parsed, std::slice::from_ref(module));
            } else {
                resolution_suppression.hide_declared_types(parsed, &[]);
            }
        }
    }

    // A clean test file is named from its project-root path. If that name
    // collides with a source module, keeping both would make one qualified name
    // point at two files depending on which resolver table was used.
    let project_module_sources: HashMap<&str, &PathBuf> = project
        .modules
        .iter()
        .filter(|module| !module.name.is_empty())
        .map(|module| (module.name.as_str(), &module.source_file))
        .collect();
    let mut duplicate_test_module_names = HashSet::new();
    let mut unique_modules = Vec::new();
    for module in modules {
        if let Some(first) = project_module_sources.get(module.name.as_str()) {
            report.diagnostics.push(CheckDiagnostic {
                code: CHECK_DUPLICATE_MODULE,
                severity: Severity::Error,
                file: module.source_file.clone(),
                message: format!(
                    "module `{}` is already declared by `{}`",
                    module.name,
                    first.display()
                ),
                span: SourceSpan::default(),
            });
            duplicate_test_module_names.insert(module.name);
        } else {
            unique_modules.push(module);
        }
    }
    let modules = unique_modules;

    // Imports in a test file resolve against the project's modules, the other
    // test modules, and the standard library. The project's module-less script
    // carries the empty name; a `use ""` is not spellable, so it is filtered out
    // here — the import-safety invariant (a script is never importable) is kept
    // local to this map rather than resting on a distant construction guard.
    let mut resolvable: HashMap<String, PathBuf> = project
        .modules
        .iter()
        .filter(|module| !module.name.is_empty())
        .map(|module| (module.name.clone(), module.source_file.clone()))
        .collect();
    for module in &modules {
        resolvable.insert(module.name.clone(), module.source_file.clone());
    }

    // Run the same type-inference pass library files get, so a test file's std
    // argument/arity errors, `nextId` misuse, and ordinary type mismatches are
    // reported at check time rather than only at run time. Types resolve against a
    // program holding both the already-checked project modules and the clean test
    // modules — cross-module calls and resource constructors resolve, and the
    // `nextId` gate finds the project's resource schemas.
    let project_count = project.modules.len();
    let mut combined =
        CheckedProgram::from_modules(project.modules.iter().cloned().chain(modules).collect());
    resolution_suppression.reveal_complete_modules(&combined);
    // The duplicate diagnostic is the useful error; downstream references into
    // that invalid namespace should not also pretend the function is just absent.
    for module in &duplicate_test_module_names {
        resolution_suppression.hide_module(module.clone());
    }
    // Stamp cross-module named-type signature slots in the test modules with their
    // true owner — resolved against the combined program — before the type pass
    // reads them. The project modules carry the project's own normalized
    // signatures already.
    let resolver = combined.clone();
    enums::normalize_program_named_types_against(&mut combined, &resolver, &parsed_files);
    // Passes 2-3 plus targeted unresolved-call suppression are shared with check_project.
    // A read failure drops a file from `parsed_files` so a call into it would look
    // unresolved; the shared suppression handles that qualified-call case.
    checks::check_resolved_files(
        checks::ResolvedFileCheck {
            files: &files,
            parsed_files: &parsed_files,
            module_name_policy: checks::ModuleNamePolicy::PathOnly,
            resolvable: &resolvable,
            program: &combined,
        },
        &mut report,
    );
    // When the source program is partial, a configured test may name a module,
    // function, resource, or enum that exists in a source file which could not
    // join `project`.
    // Keep test-local parse/type diagnostics, but avoid reporting resolution
    // noise whose truth depends on the incomplete source module set.
    report
        .diagnostics
        .retain(|diagnostic| !resolution_suppression.should_suppress(diagnostic));

    combined.rebuild_facts_with_sources_preserving_prefix(
        project,
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );
    combined.lower_runtime_bodies(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );
    let analyzed = parsed_files
        .into_iter()
        .map(|(file, parsed)| analysis::AnalyzedFile {
            path: file.path.clone(),
            module_name: file.module_name.clone(),
            source: parsed_sources.remove(&file.path).unwrap_or_default(),
            parsed,
        })
        .collect();

    Ok(CheckedTests {
        report,
        program: combined,
        test_module_start: project_count,
        files: analyzed,
    })
}

/// The per-file result of [`check_file`]: the parsed source and the declaration
/// lists collected for a [`CheckedModule`].
struct CheckedFile {
    parsed: marrow_syntax::ParsedSource,
    resources: Vec<marrow_schema::ResourceSchema>,
    stores: Vec<marrow_schema::StoreSchema>,
    enums: Vec<marrow_schema::EnumSchema>,
    functions: Vec<CheckedFunction>,
    constants: Vec<CheckedConst>,
}

/// Resolve a file's text: the overlay entry for `file_path` if `sources` carries
/// one, otherwise the on-disk contents. A read failure records an `io.read`
/// diagnostic and returns `None`, preserving the read-failure-drops-a-file
/// invariant the project and test checkers rely on.
fn read_source(
    file_path: &Path,
    sources: &ProjectSources,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<String> {
    if let Some(overlay) = sources.get(file_path) {
        return Some(overlay.to_string());
    }
    match std::fs::read_to_string(file_path) {
        Ok(source) => Some(source),
        Err(error) => {
            diagnostics.push(CheckDiagnostic {
                code: IO_READ,
                severity: Severity::Error,
                file: file_path.to_path_buf(),
                message: format!("failed to read source: {error}"),
                span: SourceSpan::default(),
            });
            None
        }
    }
}

/// Parse and structurally check one source file's `source`: record its parse,
/// duplicate-name, function-body, const-value, and resource-schema diagnostics,
/// and collect the declaration lists for a checked module. Always returns a
/// [`CheckedFile`] — a parse error yields one whose `parsed` carries the
/// diagnostics. Cross-file checks — module-path matching, saved-root ownership,
/// and import resolution — belong to the caller, since they span files and differ
/// between library modules and test scripts.
fn check_file_source(
    file_path: &Path,
    source: &str,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> CheckedFile {
    let parsed = parse_source(source);
    for diagnostic in &parsed.diagnostics {
        diagnostics.push(CheckDiagnostic {
            code: diagnostic.code,
            severity: diagnostic.severity,
            file: file_path.to_path_buf(),
            message: diagnostic.message.clone(),
            span: diagnostic.span,
        });
    }

    check_duplicate_declarations(file_path, &parsed.file, diagnostics);

    let module_enums: Vec<String> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Enum(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect();
    let module_stored_resources: HashSet<String> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Store(store) => Some(store.resource.clone()),
            _ => None,
        })
        .collect();
    // A bare enum annotation in a signature names this module's enum, so the
    // resolved type carries the module's qualified name as the enum's owner. A
    // module-less script has no declared name (its enums are project-unique).
    let module_name = parsed
        .file
        .module
        .as_ref()
        .map_or("", |module| module.name.as_str());
    let names = TypeNames {
        module: module_name,
        enums: &module_enums,
    };
    let resources = checked_resources(
        file_path,
        &parsed,
        &module_stored_resources,
        &module_enums,
        diagnostics,
    );
    let mut stores = Vec::new();
    let mut enums = Vec::new();
    let mut functions = Vec::new();
    let mut constants = Vec::new();
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Function(function) => {
                rules::check_function_body(file_path, function, diagnostics);
                functions.push(checked_function(function, names));
            }
            marrow_syntax::Declaration::Resource(_) => {}
            marrow_syntax::Declaration::Store(store) => {
                if let Some(resource) = resources
                    .iter()
                    .find(|resource| resource.name == store.resource)
                {
                    let (schema, errors) = marrow_schema::compile_store(store, resource);
                    for error in errors {
                        let diagnostic = CheckDiagnostic {
                            code: error.code,
                            severity: Severity::Error,
                            file: file_path.to_path_buf(),
                            message: error.message,
                            span: error.span,
                        };
                        if !diagnostics.iter().any(|existing| {
                            existing.code == diagnostic.code
                                && existing.file == diagnostic.file
                                && existing.message == diagnostic.message
                                && existing.span == diagnostic.span
                        }) {
                            diagnostics.push(diagnostic);
                        }
                    }
                    stores.push(schema);
                } else {
                    diagnostics.push(CheckDiagnostic {
                        code: CHECK_UNKNOWN_TYPE,
                        severity: Severity::Error,
                        file: file_path.to_path_buf(),
                        message: format!(
                            "unknown resource `{}` for store `^{}`",
                            store.resource, store.root.root
                        ),
                        span: store.span,
                    });
                }
            }
            marrow_syntax::Declaration::Enum(decl) => {
                let (schema, errors) = marrow_schema::compile_enum(decl);
                for error in errors {
                    diagnostics.push(CheckDiagnostic {
                        code: error.code,
                        severity: Severity::Error,
                        file: file_path.to_path_buf(),
                        message: error.message,
                        span: error.span,
                    });
                }
                enums.push(schema);
            }
            marrow_syntax::Declaration::Const(constant) => {
                if let Some(value) = &constant.value {
                    rules::check_const_value(file_path, value, diagnostics);
                }
                constants.push(CheckedConst {
                    name: constant.name.clone(),
                    ty: constant
                        .ty
                        .as_ref()
                        .map(|ty| MarrowType::resolve(ty, names)),
                    value: constant.value.clone(),
                    span: constant.span,
                });
            }
            // An evolve block compiles to no resource, store, enum, function, or
            // constant; its catalog intent is resolved against the bound program.
            marrow_syntax::Declaration::Evolve(_) => {}
        }
    }

    CheckedFile {
        parsed,
        resources,
        stores,
        enums,
        functions,
        constants,
    }
}

fn checked_resources(
    file_path: &Path,
    parsed: &marrow_syntax::ParsedSource,
    module_stored_resources: &HashSet<String>,
    module_enums: &[String],
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<marrow_schema::ResourceSchema> {
    parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Resource(resource) => Some(resource),
            _ => None,
        })
        .map(|resource| {
            let has_store = module_stored_resources.contains(&resource.name);
            let (schema, errors) = if has_store {
                marrow_schema::compile_stored_resource(resource)
            } else {
                marrow_schema::compile_resource(resource)
            };
            for error in errors {
                push_schema_error(file_path, diagnostics, error);
            }
            if has_store {
                for error in marrow_schema::check_saved_member_rules(&resource.members) {
                    push_schema_error(file_path, diagnostics, error);
                }
                for error in
                    marrow_schema::check_saved_named_member_fields(&resource.members, module_enums)
                {
                    push_schema_error(file_path, diagnostics, error);
                }
            }
            schema
        })
        .collect()
}

fn push_schema_error(
    file_path: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
    error: marrow_schema::SchemaError,
) {
    diagnostics.push(CheckDiagnostic {
        code: error.code,
        severity: Severity::Error,
        file: file_path.to_path_buf(),
        message: error.message,
        span: error.span,
    });
}

/// Resolve a function declaration for the checked-program artifact: its
/// signature plus the checked executable body runtime evaluates.
fn checked_function(
    function: &marrow_syntax::FunctionDecl,
    names: TypeNames<'_>,
) -> CheckedFunction {
    CheckedFunction {
        name: function.name.clone(),
        public: function.public,
        params: function
            .params
            .iter()
            .map(|param| CheckedParam {
                name: param.name.clone(),
                mode: param.mode.map(CheckedParamMode::lower),
                ty: MarrowType::resolve(&param.ty, names),
            })
            .collect(),
        return_type: function
            .return_type
            .as_ref()
            .map(|ty| MarrowType::resolve(ty, names)),
        span: function.span,
        touches_saved_data: block_touches_saved_data(&function.body),
        runtime_body: None,
    }
}

/// Does a block read or write any saved root (`^...`)? Walks every statement and
/// nested expression, recursing through nested blocks.
fn block_touches_saved_data(block: &marrow_syntax::Block) -> bool {
    block.statements.iter().any(statement_touches_saved_data)
}

fn statement_touches_saved_data(statement: &marrow_syntax::Statement) -> bool {
    use marrow_syntax::Statement;
    match statement {
        Statement::Const { value, .. } | Statement::Throw { value, .. } => {
            expr_touches_saved_data(value)
        }
        Statement::Var { value, .. } => value.as_ref().is_some_and(expr_touches_saved_data),
        Statement::Assign { target, value, .. } => {
            expr_touches_saved_data(target) || expr_touches_saved_data(value)
        }
        Statement::Delete { path, .. } => expr_touches_saved_data(path),
        Statement::Return { value, .. } => value.as_ref().is_some_and(expr_touches_saved_data),
        Statement::Expr { value, .. } => expr_touches_saved_data(value),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            condition.as_ref().is_some_and(expr_touches_saved_data)
                || block_touches_saved_data(then_block)
                || else_ifs.iter().any(|else_if| {
                    else_if
                        .condition
                        .as_ref()
                        .is_some_and(expr_touches_saved_data)
                        || block_touches_saved_data(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_touches_saved_data)
        }
        Statement::While {
            condition, body, ..
        } => {
            condition.as_ref().is_some_and(expr_touches_saved_data)
                || block_touches_saved_data(body)
        }
        Statement::For { iterable, body, .. } => {
            expr_touches_saved_data(iterable) || block_touches_saved_data(body)
        }
        Statement::Transaction { body, .. } => block_touches_saved_data(body),
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            block_touches_saved_data(body)
                || catch
                    .as_ref()
                    .is_some_and(|catch| block_touches_saved_data(&catch.block))
                || finally.as_ref().is_some_and(block_touches_saved_data)
        }
        Statement::Match {
            scrutinee, arms, ..
        } => {
            scrutinee.as_ref().is_some_and(expr_touches_saved_data)
                || arms.iter().any(|arm| block_touches_saved_data(&arm.block))
        }
        Statement::Break { .. } | Statement::Continue { .. } => false,
    }
}

fn expr_touches_saved_data(expr: &marrow_syntax::Expression) -> bool {
    use marrow_syntax::{Expression, InterpolationPart};
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Literal { .. } | Expression::Name { .. } => false,
        Expression::Call { callee, args, .. } => {
            expr_touches_saved_data(callee)
                || args.iter().any(|arg| expr_touches_saved_data(&arg.value))
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            expr_touches_saved_data(base)
        }
        Expression::Unary { operand, .. } => expr_touches_saved_data(operand),
        Expression::Binary { left, right, .. } => {
            expr_touches_saved_data(left) || expr_touches_saved_data(right)
        }
        Expression::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            InterpolationPart::Text { .. } => false,
            InterpolationPart::Expr(expr) => expr_touches_saved_data(expr),
        }),
    }
}

/// Build the per-file short→full alias map from a file's full import paths
/// (`["std::clock", "shelf::books"]`): key = the short trailing segment
/// (`"clock"`, `"books"`), value = the full path split into segments
/// (`["std","clock"]`). Same short-name derivation as
/// [`check_duplicate_declarations`], so the two stay consistent. Drives short-form
/// call resolution during checking and executable lowering.
pub(crate) fn build_alias_map(
    import_paths: &[String],
) -> std::collections::HashMap<String, Vec<String>> {
    import_paths
        .iter()
        .map(|path| {
            let short = path.rsplit("::").next().unwrap_or(path).to_string();
            let full = path.split("::").map(str::to_string).collect();
            (short, full)
        })
        .collect()
}

/// Expand a call/name's leading segment through the file's import aliases, applied
/// once up front before any builtin/std/function resolution. `clock::now` with
/// `{clock → [std,clock]}` becomes `[std,clock,now]`. A single-segment name is
/// never expanded (short-form requires the module qualifier), and a leading
/// segment that is not an alias is left untouched (so `std::clock::now` and a
/// project `mod::fn` pass through unchanged).
pub(crate) fn expand_alias(
    segments: &[String],
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    if segments.len() >= 2 {
        return expand_leading_alias(segments, aliases);
    }
    segments.to_vec()
}

fn expand_leading_alias(
    segments: &[String],
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    if let Some(first) = segments.first()
        && let Some(full) = aliases.get(first)
    {
        return full
            .iter()
            .cloned()
            .chain(segments[1..].iter().cloned())
            .collect();
    }
    segments.to_vec()
}

/// The standard-library module names, derived once from the shared stdlib table
/// ([`marrow_schema::stdlib::all`]) so import resolution and the op table share a
/// single source of truth — a new `std::<module>::op` row adds its module here
/// automatically, with no hardcoded list to drift. Host modules resolve at check
/// time even when a host would not provide them at run time.
fn std_modules() -> &'static std::collections::HashSet<&'static str> {
    static STD_MODULES: std::sync::OnceLock<std::collections::HashSet<&'static str>> =
        std::sync::OnceLock::new();
    STD_MODULES.get_or_init(|| stdlib::all().iter().map(|op| op.module).collect())
}

/// Is `name` a standard-library module path? Accepts `std::<module>` and any
/// deeper path under a valid `std` module.
fn is_std_module(name: &str) -> bool {
    name.strip_prefix("std::")
        .and_then(|rest| rest.split("::").next())
        .is_some_and(|module| std_modules().contains(module))
}

/// An import resolves when it names a project module or a standard-library
/// module.
fn is_resolved_import(name: &str, project_modules: &HashMap<String, PathBuf>) -> bool {
    project_modules.contains_key(name) || is_std_module(name)
}

fn module_path_error(
    file: &marrow_project::ModuleFile,
    module: &marrow_syntax::ModuleDecl,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_MODULE_PATH,
        severity: Severity::Error,
        file: file.path.clone(),
        message,
        span: module.span,
    }
}

/// Top-level declaration names (const, resource, function) and imported short
/// module names share one namespace within a file. Flag any name introduced
/// more than once, reporting the later occurrence and referencing the first.
fn check_duplicate_declarations(
    file: &Path,
    source: &marrow_syntax::SourceFile,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Declaration;

    // Every name this file introduces, in source order. A `use shelf::books`
    // introduces the short name `books`.
    let mut introduced: Vec<(&str, SourceSpan, &'static str)> = Vec::new();
    for use_decl in &source.uses {
        let short = use_decl.name.rsplit("::").next().unwrap_or(&use_decl.name);
        introduced.push((short, use_decl.span, "import"));
    }
    for declaration in &source.declarations {
        let (name, span) = match declaration {
            Declaration::Const(decl) => (decl.name.as_str(), decl.span),
            Declaration::Resource(decl) => (decl.name.as_str(), decl.span),
            Declaration::Store(_) | Declaration::Evolve(_) => continue,
            Declaration::Function(decl) => (decl.name.as_str(), decl.span),
            Declaration::Enum(decl) => (decl.name.as_str(), decl.span),
        };
        introduced.push((name, span, "declaration"));
    }
    introduced.sort_by_key(|(_, span, _)| (span.line, span.start_byte));

    let mut first_seen: HashMap<&str, SourceSpan> = HashMap::new();
    for (name, span, kind) in &introduced {
        // The parser leaves a failed declaration with an empty name; do not
        // treat those as colliding with each other.
        if name.is_empty() {
            continue;
        }
        if *kind != "import" && is_builtin_name(name) {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_DUPLICATE_DECLARATION,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!(
                    "`{name}` is a builtin name and cannot be used as a module-level {kind}"
                ),
                span: *span,
            });
            continue;
        }
        match first_seen.get(name) {
            Some(first) => diagnostics.push(CheckDiagnostic {
                code: CHECK_DUPLICATE_DECLARATION,
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("`{name}` is already declared on line {}", first.line),
                span: *span,
            }),
            None => {
                first_seen.insert(name, *span);
            }
        }
    }
}
