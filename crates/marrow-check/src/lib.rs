//! Resolve and check a Marrow project's source.
//!
//! This is the start of the checked-program pipeline: discover the project's
//! `.mw` files, parse each one, and report parse diagnostics together with
//! module/path resolution problems. Type, effect, and schema facts build on
//! top of this in later work.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_modules};
use marrow_syntax::{Severity, SourceSpan, parse_source};

mod rules;

/// A library file declares a module name that does not match its path.
pub const CHECK_MODULE_PATH: &str = "check.module_path";
/// Two library files declare the same module name.
pub const CHECK_DUPLICATE_MODULE: &str = "check.duplicate_module";
/// A name is declared or imported more than once within a single file.
pub const CHECK_DUPLICATE_DECLARATION: &str = "check.duplicate_declaration";
/// A `use` names a module that is neither a project module nor a standard
/// library module.
pub const CHECK_UNRESOLVED_IMPORT: &str = "check.unresolved_import";
/// A discovered source file could not be read.
pub const IO_READ: &str = "io.read";

/// A problem found while checking a project, located in a specific file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDiagnostic {
    pub code: String,
    pub severity: Severity,
    pub file: PathBuf,
    pub message: String,
    pub line: u32,
    pub column: u32,
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

/// Discover, read, and parse every `.mw` file in the project, collecting parse
/// diagnostics and module/path resolution problems. Fails only when a source
/// root cannot be walked; per-file read errors become diagnostics.
pub fn check_project(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<CheckReport, DiscoverError> {
    let files = discover_modules(project_root, config)?;
    let mut report = CheckReport::default();
    // The first valid library file (in path order) to declare each module owns
    // that name; later files declaring it are duplicates. This is also the set
    // of resolvable project module names for `use` resolution.
    let mut declared: HashMap<String, PathBuf> = HashMap::new();
    // Parsed sources kept from pass 1 so pass 2 can resolve imports against the
    // full project module set without re-reading files.
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();

    // Pass 1: parse each file and collect per-file findings plus the project's
    // module set.
    for file in &files {
        let source = match std::fs::read_to_string(&file.path) {
            Ok(source) => source,
            Err(error) => {
                report.diagnostics.push(CheckDiagnostic {
                    code: IO_READ.to_string(),
                    severity: Severity::Error,
                    file: file.path.clone(),
                    message: format!("failed to read source: {error}"),
                    line: 0,
                    column: 0,
                });
                continue;
            }
        };

        let parsed = parse_source(&source);
        for diagnostic in &parsed.diagnostics {
            report.diagnostics.push(CheckDiagnostic {
                code: diagnostic.code.to_string(),
                severity: diagnostic.severity,
                file: file.path.clone(),
                message: diagnostic.message.clone(),
                line: diagnostic.line,
                column: diagnostic.column,
            });
        }

        check_duplicate_declarations(&file.path, &parsed.file, &mut report.diagnostics);

        // Structural statement rules apply to every function body; resources are
        // compiled to schemas so structural schema problems surface here too.
        for declaration in &parsed.file.declarations {
            match declaration {
                marrow_syntax::Declaration::Function(function) => {
                    rules::check_function_body(&file.path, &function.body, &mut report.diagnostics);
                }
                marrow_syntax::Declaration::Resource(resource) => {
                    let (_schema, errors) = marrow_schema::compile_resource(resource);
                    for error in errors {
                        report.diagnostics.push(CheckDiagnostic {
                            code: error.code.to_string(),
                            severity: Severity::Error,
                            file: file.path.clone(),
                            message: error.message,
                            line: error.span.line,
                            column: error.span.column,
                        });
                    }
                }
                marrow_syntax::Declaration::Const(_) => {}
            }
        }

        // A library file (one that declares a `module`) must declare the name
        // its path implies. A module-less file is a script or entrypoint and is
        // not bound to a path.
        if let Some(module) = &parsed.file.module {
            match &file.module_name {
                // A valid library module: enforce uniqueness of the name.
                Some(expected) if expected == &module.name => {
                    if let Some(first) = declared.get(expected) {
                        report.diagnostics.push(CheckDiagnostic {
                            code: CHECK_DUPLICATE_MODULE.to_string(),
                            severity: Severity::Error,
                            file: file.path.clone(),
                            message: format!(
                                "module `{expected}` is already declared by `{}`",
                                first.display()
                            ),
                            line: module.span.line,
                            column: module.span.column,
                        });
                    } else {
                        declared.insert(expected.clone(), file.path.clone());
                    }
                }
                Some(expected) => report.diagnostics.push(module_path_error(
                    file,
                    module,
                    format!(
                        "module `{}` does not match its path; expected `{expected}`",
                        module.name
                    ),
                )),
                // `discover_modules` only yields `.mw` files with clean relative
                // paths, so it always carries an expected name; this arm is
                // defensive for any other source of `ModuleFile`.
                None => report.diagnostics.push(module_path_error(
                    file,
                    module,
                    format!(
                        "a file at this path cannot declare module `{}`",
                        module.name
                    ),
                )),
            }
        }

        parsed_files.push((file, parsed));
    }

    // Pass 2: every `use` must name a project module or a standard-library
    // module, now that the full project module set is known.
    for (file, parsed) in &parsed_files {
        for use_decl in &parsed.file.uses {
            if !is_resolved_import(&use_decl.name, &declared) {
                report.diagnostics.push(CheckDiagnostic {
                    code: CHECK_UNRESOLVED_IMPORT.to_string(),
                    severity: Severity::Error,
                    file: file.path.clone(),
                    message: format!("cannot resolve import `{}`", use_decl.name),
                    line: use_decl.span.line,
                    column: use_decl.span.column,
                });
            }
        }
    }

    Ok(report)
}

/// The standard-library modules. Host modules resolve at check time even when a
/// host would not provide them at run time.
const STD_MODULES: &[&str] = &[
    "clock", "io", "env", "text", "bytes", "math", "assert", "log",
];

/// Is `name` a standard-library module path? Accepts `std::<module>` and any
/// deeper path under a valid `std` module.
fn is_std_module(name: &str) -> bool {
    name.strip_prefix("std::")
        .and_then(|rest| rest.split("::").next())
        .is_some_and(|module| STD_MODULES.contains(&module))
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
        code: CHECK_MODULE_PATH.to_string(),
        severity: Severity::Error,
        file: file.path.clone(),
        message,
        line: module.span.line,
        column: module.span.column,
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
                code: CHECK_DUPLICATE_DECLARATION.to_string(),
                severity: Severity::Error,
                file: file.to_path_buf(),
                message: format!("`{name}` is already declared on line {}", first.line),
                line: span.line,
                column: span.column,
            }),
            None => {
                first_seen.insert(name, *span);
            }
        }
    }
}
