//! The IDE-grade analysis pipeline: discover, read, parse, and check a project's
//! source into the snapshot editor tooling consumes. The cursor type and scope
//! queries that read that snapshot live in [`cursor`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use marrow_project::{DiscoverError, ProjectConfig, discover_modules};
use marrow_syntax::SourceSpan;

use crate::checks::{ModuleNamePolicy, ResolvedFileCheck, check_resolved_files};
use crate::enums::normalize_program_named_types;
use crate::{
    CHECK_DUPLICATE_MODULE, CHECK_MULTIPLE_SCRIPTS, CheckDiagnostic, CheckReport, CheckedFile,
    CheckedModule, CheckedProgram, DiagnosticPayload, IO_READ, ProjectSources,
    SCHEMA_DUPLICATE_ROOT_OWNER, TestResolutionSuppression, check_file_source, enum_visibility,
    module_path_error, read_source,
};

mod cursor;

pub(crate) use cursor::span_covers;
pub use cursor::{scope_at, type_at};

/// An IDE-grade view of a checked project: the diagnostics and best-effort
/// program [`check_project`] produces, plus every parsed file — including files
/// with parse errors, which contribute no [`CheckedModule`] but are retained
/// here so editor tooling can still work on them.
#[derive(Debug, Clone)]
pub struct AnalysisSnapshot {
    pub report: CheckReport,
    pub program: CheckedProgram,
    pub files: Vec<AnalyzedFile>,
}

/// One parsed file in an [`AnalysisSnapshot`]: its path, the module name its
/// path implies (`None` for a path that cannot name a module), and the parse —
/// retained whether or not it carries errors.
#[derive(Debug, Clone)]
pub struct AnalyzedFile {
    pub path: PathBuf,
    pub module_name: Option<String>,
    pub source: String,
    pub parsed: marrow_syntax::ParsedSource,
}

/// The IDE-grade analysis core: discover, read (from `sources` overlay or disk),
/// parse, and check source-root files plus configured test files, returning the
/// diagnostics and best-effort source program plus every parsed source file
/// (error files included). The accepted catalog is a caller-supplied input —
/// `None` checks the source as a first run — against which durable identity binds.
/// Fails only when a configured source or test directory cannot be walked.
pub fn analyze_project(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
) -> Result<AnalysisSnapshot, DiscoverError> {
    let mut snapshot = analyze_source_project(project_root, config, sources, accepted)?;
    let resolution_suppression = source_resolution_suppression(&snapshot, project_root, config);
    let tests = crate::check_tests_with_sources_analysis(
        project_root,
        config,
        &snapshot.program,
        sources,
        resolution_suppression,
    )?;
    snapshot.report.diagnostics.extend(tests.report.diagnostics);
    snapshot.files.extend(tests.files);
    snapshot.files.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.files.dedup_by(|a, b| a.path == b.path);
    Ok(snapshot)
}

fn source_resolution_suppression(
    snapshot: &AnalysisSnapshot,
    project_root: &Path,
    config: &ProjectConfig,
) -> TestResolutionSuppression {
    let mut suppression = TestResolutionSuppression::default();
    let mut declared_modules: HashMap<String, usize> = HashMap::new();
    for file in &snapshot.files {
        if let Some(module) = &file.parsed.file.module {
            *declared_modules.entry(module.name.clone()).or_default() += 1;
        }
    }

    for file in &snapshot.files {
        let declared = file.parsed.file.module.as_ref().map(|module| &module.name);
        let path_matches = match (declared, file.module_name.as_ref()) {
            (Some(declared), Some(expected)) => declared == expected,
            (Some(_), None) => false,
            _ => true,
        };
        let duplicate_module = declared.is_some_and(|name| declared_modules[name] > 1);
        if file.parsed.has_errors() || !path_matches || duplicate_module {
            let mut hidden_module_names = Vec::new();
            if let Some(name) = declared {
                hidden_module_names.push(name.clone());
            }
            if let Some(name) = &file.module_name
                && !hidden_module_names.iter().any(|module| module == name)
            {
                hidden_module_names.push(name.clone());
            }
            for name in &hidden_module_names {
                suppression.hide_module(name.clone());
            }
            suppression.hide_declared_types(&file.parsed, &hidden_module_names);
        }
    }

    for diagnostic in &snapshot.report.diagnostics {
        if diagnostic.code == IO_READ
            && let Some(file) = overlay_module_file(project_root, config, &diagnostic.file)
            && let Some(name) = file.module_name
        {
            suppression.hide_module(name);
        }
    }

    suppression
}

/// Source-root-only analysis shared by [`check_project`]. Runtime entry points use
/// this so configured test files do not block running the checked source program. The
/// accepted catalog is the caller-supplied snapshot durable identity binds against.
pub(crate) fn analyze_source_project(
    project_root: &Path,
    config: &ProjectConfig,
    sources: &ProjectSources,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
) -> Result<AnalysisSnapshot, DiscoverError> {
    let mut files = discover_modules(project_root, config)?;
    for path in sources.paths() {
        if let Some(file) = overlay_module_file(project_root, config, path) {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    let mut report = CheckReport::default();
    let mut program = CheckedProgram::default();
    // The first valid library file (in path order) to declare each module owns
    // that name; later files declaring it are duplicates. This is also the set
    // of resolvable project module names for `use` resolution.
    let mut declared: HashMap<String, PathBuf> = HashMap::new();
    // The first store (in file then source order) to declare each saved root owns
    // it; a later store on the same root is a duplicate owner.
    let mut root_owners: HashMap<String, PathBuf> = HashMap::new();
    // Parsed sources kept from pass 1 so pass 2 can resolve imports against the
    // full project module set without re-reading files.
    let mut parsed_files: Vec<(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)> =
        Vec::new();
    let mut parsed_sources: HashMap<PathBuf, String> = HashMap::new();
    // Module-less parse-clean files, deferred until the whole project is seen. A
    // project may hold at most one such single-file script; it joins `program`
    // under the empty module name. Two or more would share that name, so a bare
    // reference in one could alias another's declarations — that is rejected, not
    // assembled. Holding the candidates until the loop ends lets the decision rest
    // on the project-wide count rather than first-seen order.
    let mut scripts: Vec<CheckedModule> = Vec::new();

    // Pass 1: parse each file and collect per-file findings plus the project's
    // module set.
    for file in &files {
        // Editor buffer text wins over disk for an overlaid path; a path with no
        // overlay is read from disk, and a read failure drops the file (its
        // `io.read` diagnostic is recorded).
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

        // Saved roots are owned project-wide by stores.
        for declaration in &parsed.file.declarations {
            let marrow_syntax::Declaration::Store(store) = declaration else {
                continue;
            };
            match root_owners.get(&store.root.root) {
                Some(first) => report.diagnostics.push(
                    CheckDiagnostic::error(
                        SCHEMA_DUPLICATE_ROOT_OWNER,
                        &file.path,
                        store.span,
                        format!(
                            "saved root `^{}` is already owned by a store in `{}`",
                            store.root.root,
                            first.display()
                        ),
                    )
                    .with_payload(DiagnosticPayload::DuplicateRootOwner {
                        root: store.root.root.clone(),
                        first_owner: first.clone(),
                    }),
                ),
                None => {
                    root_owners.insert(store.root.root.clone(), file.path.clone());
                }
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
                        report.diagnostics.push(
                            CheckDiagnostic::error(
                                CHECK_DUPLICATE_MODULE,
                                &file.path,
                                module.span,
                                format!(
                                    "module `{expected}` is already declared by `{}`",
                                    first.display()
                                ),
                            )
                            .with_payload(
                                DiagnosticPayload::DuplicateModule {
                                    name: expected.clone(),
                                    first_file: first.clone(),
                                },
                            ),
                        );
                    } else {
                        declared.insert(expected.clone(), file.path.clone());
                        // The artifact takes a clean, path-matched, first-seen
                        // library module; a file carrying a parse error contributes none.
                        if !parsed.has_errors() {
                            program.modules.push(CheckedModule {
                                name: module.name.clone(),
                                source_file: file.path.clone(),
                                span: module.span,
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
                    }
                }
                Some(expected) => report.diagnostics.push(module_path_error(
                    file,
                    module,
                    format!(
                        "module `{}` does not match its path; expected `{expected}`",
                        module.name
                    ),
                    Some(expected.clone()),
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
                    None,
                )),
            }
        } else if !parsed.has_errors() {
            // A module-less file is a single-file script: its declarations are
            // nominally self-resolvable within the file, but it is never bound to
            // a path and no other module can `use` it. A single script joins
            // `program` under the empty module name it always carries — so its own
            // resource, enum, and function references resolve and are checked — but
            // never `declared`, the import-resolvable map, so it stays
            // un-importable. The empty-name module is deferred until the project's
            // script count is known. A file carrying a parse error contributes none.
            scripts.push(CheckedModule {
                name: String::new(),
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

    // A project may have at most one module-less file: its single entrypoint
    // script. Exactly one joins the program under the empty module name. Two or
    // more share that name, so the empty-named module would be ambiguous — a bare
    // reference in one script could resolve against another's declarations, a
    // `var o: Order` could bind to a foreign resource of the same name, and an
    // entry in any but the first would be unreachable at run time. Rather than
    // assemble that aliasing module, reject every script: a project's library files
    // must declare a `module`. The single-file `check`/`run` paths see one script
    // and are unaffected.
    if scripts.len() <= 1 {
        program.modules.append(&mut scripts);
    } else {
        for script in &scripts {
            report.diagnostics.push(CheckDiagnostic::error(
                CHECK_MULTIPLE_SCRIPTS,
                &script.source_file,
                SourceSpan::default(),
                "a project may have at most one file without a `module` \
                    declaration (its single-file script); declare a `module` for this file",
            ));
        }
    }

    // Stamp each cross-module named-type signature slot with its true owner, now
    // that the whole program is known, before any pass reads parameter types.
    normalize_program_named_types(&mut program, &parsed_files);

    // Passes 2-3 plus unresolved-call suppression are shared with check_tests.
    check_resolved_files(
        ResolvedFileCheck {
            files: &files,
            parsed_files: &parsed_files,
            module_name_policy: ModuleNamePolicy::DeclaredOrPath,
            resolvable: &declared,
            program: &program,
        },
        &mut report,
    );

    program.rebuild_facts_with_sources(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );

    let evolve_intents = crate::evolution::collect_evolve_intents(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
        &mut report.diagnostics,
    );
    crate::catalog::bind_catalog(
        accepted,
        &mut program,
        &evolve_intents,
        &mut report.diagnostics,
    );
    crate::evolution::check_evolve_types(
        &program,
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
        &mut report.diagnostics,
    );
    program.lower_runtime_bodies(
        parsed_files
            .iter()
            .map(|(file, parsed)| (file.path.as_path(), parsed)),
    );
    crate::evolution::check_transform_effects(&program, &mut report.diagnostics);
    crate::presence::check_presence(&mut program, &mut report.diagnostics);

    // Move every parse — error files included — into the snapshot now that the
    // shared tail has finished borrowing them. The path and module name are
    // copied from each `ModuleFile`; the parse itself moves.
    let analyzed = parsed_files
        .into_iter()
        .map(|(file, parsed)| AnalyzedFile {
            path: file.path.clone(),
            module_name: file.module_name.clone(),
            source: parsed_sources.remove(&file.path).unwrap_or_default(),
            parsed,
        })
        .collect();

    Ok(AnalysisSnapshot {
        report,
        program,
        files: analyzed,
    })
}

fn overlay_module_file(
    project_root: &Path,
    config: &ProjectConfig,
    path: &Path,
) -> Option<marrow_project::ModuleFile> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("mw") {
        return None;
    }
    for source_root in &config.source_roots {
        let root = project_root.join(source_root);
        let Ok(relative_path) = path.strip_prefix(&root) else {
            continue;
        };
        let relative_path = relative_path.to_path_buf();
        return Some(marrow_project::ModuleFile {
            path: path.to_path_buf(),
            module_name: marrow_project::expected_module_name(&relative_path),
            relative_path,
        });
    }
    None
}
