//! Resolve and check a Marrow project's source.
//!
//! This is the start of the checked-program pipeline: discover the project's
//! `.mw` files, parse each one, and report parse diagnostics together with
//! module/path resolution problems. Type, effect, and schema facts build on
//! top of this in later work.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_modules};
use marrow_syntax::{Severity, parse_source};

/// A library file declares a module name that does not match its path.
pub const CHECK_MODULE_PATH: &str = "check.module_path";
/// Two library files declare the same module name.
pub const CHECK_DUPLICATE_MODULE: &str = "check.duplicate_module";
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
    // that name; later files declaring it are duplicates.
    let mut declared: HashMap<String, PathBuf> = HashMap::new();

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
    }

    Ok(report)
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
