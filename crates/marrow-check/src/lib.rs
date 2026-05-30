//! Resolve and check a Marrow project's source.
//!
//! Discover the project's `.mw` files, parse each one, and report parse
//! diagnostics together with module/path resolution, type, and schema problems,
//! producing a resolved [`CheckedProgram`] alongside the diagnostics.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_modules, discover_test_modules};
use marrow_schema::stdlib::{self, ParamType, ReturnType};
use marrow_schema::{MemberPathResolution, Type};
use marrow_store::value::ScalarType;
use marrow_syntax::{Severity, SourceSpan, parse_source};

pub mod analysis;
pub mod binding;
mod checks;
mod enums;
mod infer;
pub mod program;
pub mod resolve;
mod rules;
mod typerules;

pub use analysis::{AnalysisSnapshot, AnalyzedFile, analyze_project, scope_at, type_at};
pub use binding::{BindingIndex, RenameSafety, SymbolKind, SymbolRef, build_binding_index};
pub use enums::resolve_match_enums;
// The type machinery is carved into sibling modules but stays one flat internal
// namespace: each module reaches the others (and the driver remainder here)
// through `use super::*;`, so these glob re-exports keep every `pub(crate)` helper
// resolvable across the boundary without per-call qualification.
pub(crate) use checks::*;
pub(crate) use enums::{
    check_is, check_match, collect_enum_names, join_or, normalize_program_enum_types,
    normalize_program_enum_types_against, resolve_enum_member_path, resolve_type,
};
pub(crate) use infer::*;
pub use marrow_schema::{IndexSchema, ResourceSchema};
use program::TypeNames;
pub use program::{
    CheckedConst, CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, FileId, MarrowType,
};
pub use resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};
pub(crate) use typerules::*;

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
/// A bare call names a `pub` function reachable in two or more modules, so the
/// bare name cannot pick one — it must be qualified (`module::fn`). Distinct from
/// [`CHECK_UNRESOLVED_CALL`]: candidates exist, the bare spelling is ambiguous.
pub const CHECK_AMBIGUOUS_CALL: &str = "check.ambiguous_call";
/// `nextId(^root)` names a root with no default integer allocation policy: a
/// composite identity, a single non-integer identity key, or a keyless singleton.
/// The default per-root policy is only available for a resource with one `int`
/// identity key. The runtime backstops
/// this with `write.next_id_unsupported`; the checker catches it before a run.
pub const CHECK_NEXT_ID_REQUIRES_SINGLE_INT: &str = "check.next_id_requires_single_int";
/// `next`/`prev` is applied to a shape it cannot navigate: a composite
/// multi-key identity record (its identity spans several key levels, not the one
/// `next`/`prev` step over) or an index branch (it inspects identities, with no
/// single key position to seek). The runtime would reject these with an
/// uncatchable `run.unsupported` fault; the checker catches it before a run.
pub const CHECK_NEIGHBOR_UNSUPPORTED: &str = "check.neighbor_unsupported";
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
/// Two resources in the project claim the same saved root. A saved root has one
/// managed owner. This is a schema-model rule, but it is cross-resource, so the
/// project checker reports it rather than per-resource schema compilation.
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
    analyze_project(project_root, config, sources)
        .map(|snapshot| (snapshot.report, snapshot.program))
}

/// The schema of the resource that owns saved root `^root`, if any. Saved roots
/// are project-wide (a `^books` write addresses the one `books` resource from any
/// module), so this is a thin shim over the resolver's project-wide root lookup;
/// the runtime's `find_resource` routes through the same helper.
pub(crate) fn find_resource_schema<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<&'p marrow_schema::ResourceSchema> {
    resolve::resolve_resource_by_root(program, root)
}

/// The qualified name of the module that declares `resource`. A saved enum field
/// names an enum declared in the same module (the saved-field rule forbids any
/// other named field type), so this module owns the enum a bare `Named` field
/// type refers to. Defaults to the empty module for a resource not found in any
/// module's list (a module-less script's resource), matching how a script's enums
/// carry the empty owner.
fn resource_module<'p>(program: &'p CheckedProgram, name: &str) -> &'p str {
    program
        .modules
        .iter()
        .find(|module| {
            module
                .resources
                .iter()
                .any(|resource| resource.name == name)
        })
        .map_or("", |module| module.name.as_str())
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
/// `use a::b::c`) names the imported module (`a::b::c`). Reuses [`expand_alias`]
/// by appending a sentinel leaf, so a single-segment alias expands the same way a
/// call's leading segment does; a non-alias leading segment is left untouched.
/// Public so the runtime resolves an aliased enum literal to the same module the
/// checker did.
pub fn expand_module_alias(module: &str, aliases: &HashMap<String, Vec<String>>) -> String {
    let mut segments: Vec<String> = module.split("::").map(str::to_string).collect();
    // `expand_alias` only expands a leading alias when a trailing segment follows
    // (short-form requires the qualifier); append a sentinel so a bare alias
    // module (`c`) expands, then drop it.
    segments.push(String::new());
    let mut expanded = expand_alias(&segments, aliases);
    expanded.pop();
    expanded.join("::")
}

/// Resolve a call's `segments` to a function, also yielding the [`CheckedModule`]
/// that owns it so the binding index can locate the definition's source file. A
/// thin shim over the unified [`resolve`]: a bare name resolves in `from_module`,
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
        [name] => {
            matches!(
                name.as_str(),
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
        // A `std::module::op` builtin must name a real std module, mirroring
        // import resolution (`is_std_module`/`std_modules`); an unknown submodule
        // is not a builtin, so it is reported like a rejected `use std::bogus`.
        [first, module, _] => first == "std" && std_modules().contains(&module.as_str()),
        _ => false,
    }
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
    let files = discover_test_modules(project_root, config)?;
    let mut report = CheckReport::default();
    let mut modules = Vec::new();
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();

    for file in &files {
        let Some(CheckedFile {
            parsed,
            resources,
            enums,
            functions,
            constants,
        }) = check_file(&file.path, &mut report.diagnostics)
        else {
            continue;
        };

        // A test file is a script: it is named from its path, never from a
        // declared `module`, so it can never shadow or duplicate a project
        // module. Skip a file carrying a parse error.
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
                enums,
            });
        }
        parsed_files.push((file, parsed));
    }

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
    // `nextId` gate finds the project's resource schemas. Resource annotations are
    // recognized project-wide (project plus test resources).
    let project_count = project.modules.len();
    let mut combined = CheckedProgram {
        modules: project.modules.iter().cloned().chain(modules).collect(),
    };
    // Stamp cross-module enum signature slots in the test modules with their true
    // owner — resolved against the combined program — before the type pass reads
    // them, so a test's argument against a qualified or foreign enum parameter is
    // gated on the parameter's real identity rather than a per-file `Unknown`. The
    // project modules carry the project's own normalized signatures already.
    let resolver = combined.clone();
    normalize_program_enum_types_against(&mut combined, &resolver, &parsed_files);
    let project_resources: HashSet<String> = combined
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .map(|resource| resource.name.clone())
        .collect();
    let project_enums: HashSet<String> = combined
        .modules
        .iter()
        .flat_map(|module| &module.enums)
        .map(|enum_schema| enum_schema.name.clone())
        .collect();

    // Passes 2-3 plus unresolved-call suppression are shared with check_project.
    // A read failure drops a file from `parsed_files` so a call into it would look
    // unresolved; the suppression there handles that exactly as check_project does.
    check_resolved_files(
        files.len(),
        &parsed_files,
        &resolvable,
        &combined,
        &project_resources,
        &project_enums,
        &mut report,
    );

    // Resolve `match` scrutinee enums on the test bodies, inferring against the whole
    // combined program so a test's `match` over a project enum dispatches correctly.
    // The bodies belong to the already-normalized test modules — the tail of the
    // combined program past the project's own modules.
    let mut test_program = CheckedProgram {
        modules: combined.modules[project_count..].to_vec(),
    };
    resolve_match_enums(&mut test_program, &combined);

    Ok((report, test_program.modules))
}

/// The per-file result of [`check_file`]: the parsed source and the declaration
/// lists collected for a [`CheckedModule`].
struct CheckedFile {
    parsed: marrow_syntax::ParsedSource,
    resources: Vec<marrow_schema::ResourceSchema>,
    enums: Vec<marrow_schema::EnumSchema>,
    functions: Vec<CheckedFunction>,
    constants: Vec<CheckedConst>,
}

/// Read one source file from disk, then parse and structurally check it. Returns
/// `None` if the file cannot be read (an `io.read` diagnostic is recorded). This
/// is the disk path used by [`check_tests`]; [`analyze_project`] reads through an
/// overlay instead.
fn check_file(file_path: &Path, diagnostics: &mut Vec<CheckDiagnostic>) -> Option<CheckedFile> {
    let source = read_source(file_path, &ProjectSources::new(), diagnostics)?;
    Some(check_file_source(file_path, &source, diagnostics))
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
/// stable-id uniqueness, and import resolution — belong to the caller, since they
/// span files and differ between library modules and test scripts.
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

    // Named types resolve first so a module's function and constant types can
    // refer to its own resources and enums.
    let module_resources: Vec<String> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Resource(resource) => Some(resource.name.clone()),
            _ => None,
        })
        .collect();
    let module_enums: Vec<String> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Enum(decl) => Some(decl.name.clone()),
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
        resources: &module_resources,
        enums: &module_enums,
    };
    let mut resources = Vec::new();
    let mut enums = Vec::new();
    let mut functions = Vec::new();
    let mut constants = Vec::new();
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Function(function) => {
                rules::check_function_body(file_path, &function.body, diagnostics);
                functions.push(checked_function(function, names));
            }
            marrow_syntax::Declaration::Resource(resource) => {
                let (schema, mut errors) = marrow_schema::compile_resource(resource);
                // A bare-named saved field must be a declared enum. Schema
                // compilation cannot see other declarations, so the enum names are
                // resolved here and the rule applied alongside the other saved-data
                // checks.
                errors.extend(marrow_schema::check_saved_named_fields(
                    resource,
                    &module_enums,
                ));
                for error in errors {
                    diagnostics.push(CheckDiagnostic {
                        code: error.code,
                        severity: Severity::Error,
                        file: file_path.to_path_buf(),
                        message: error.message,
                        span: error.span,
                    });
                }
                resources.push(schema);
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
        }
    }

    CheckedFile {
        parsed,
        resources,
        enums,
        functions,
        constants,
    }
}

/// Resolve a function declaration for the checked-program artifact: its
/// signature (parameter and return types resolve against the module's own named
/// types) plus its body, which the runtime evaluates.
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
                mode: param.mode,
                ty: MarrowType::resolve(&param.ty, names),
            })
            .collect(),
        return_type: function
            .return_type
            .as_ref()
            .map(|ty| MarrowType::resolve(ty, names)),
        span: function.span,
        touches_saved_data: block_touches_saved_data(&function.body),
        body: function.body.clone(),
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
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
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
        Statement::Lock { path, body, .. } => {
            path.as_ref().is_some_and(expr_touches_saved_data) || block_touches_saved_data(body)
        }
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
/// call resolution; the runtime builds the identical map from
/// `CheckedModule::imports`.
pub fn build_alias_map(import_paths: &[String]) -> std::collections::HashMap<String, Vec<String>> {
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
/// project `mod::fn` pass through unchanged). This is the shared semantics both the
/// checker and the runtime apply, so short-form resolves symmetrically.
pub fn expand_alias(
    segments: &[String],
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    if segments.len() >= 2
        && let Some(full) = aliases.get(&segments[0])
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
            Declaration::Function(decl) => (decl.name.as_str(), decl.span),
            Declaration::Enum(decl) => (decl.name.as_str(), decl.span),
        };
        introduced.push((name, span, "declaration"));
    }
    introduced.sort_by_key(|(_, span, _)| (span.line, span.start_byte));

    let mut first_seen: HashMap<&str, SourceSpan> = HashMap::new();
    for (name, span, _kind) in &introduced {
        // The parser leaves a failed declaration with an empty name; do not
        // treat those as colliding with each other.
        if name.is_empty() {
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
