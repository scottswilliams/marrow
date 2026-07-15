//! The storeless subset checker and lowering to an [`ImageDraft`].
//!
//! The compiler opens no store and mints no verified image: it parses source,
//! checks the current subset, and lowers to canonical image bytes the independent
//! verifier rechecks. Coverage grows one slice at a time; a well-formed construct
//! outside the current subset is a typed `check.unsupported` diagnostic, never a
//! silent drop.

use std::collections::{BTreeMap, BTreeSet};

use marrow_codes::Code;
use marrow_image::{EncodedImage, ExportId, ImageDraft};
use marrow_project::ProjectInput;
use marrow_syntax::{
    AliasDecl, ConstDecl, Declaration, FunctionDecl, ParsedSource, ResourceDecl, SourceSpan,
    StoreDecl, parse_source,
};

use crate::diag::SourceDiagnostic;
use crate::durable::DurableRegistry;
use crate::konst::ConstRegistry;
use crate::lower::{FnLowerer, FunctionRegistry};
use crate::types::TypeRegistry;

/// One resolved public export: its dotted module, its item name, and the stable
/// [`ExportId`] the image carries. This directory is the only place a human export
/// name is paired with its id; the CLI resolves a caller-supplied path to an id
/// here, then dispatches into the image by that verified id.
#[derive(Debug, Clone)]
pub struct ExportEntry {
    pub module: String,
    pub item: String,
    pub id: ExportId,
}

/// The result of compiling a project: the canonical image bytes and the export
/// directory that maps declaration paths to their ids.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub image: EncodedImage,
    pub exports: Vec<ExportEntry>,
}

/// One discovered `test "name"` declaration: its report title, the module and
/// source file it lives in, and the source position of its header. The image
/// carries the title in its closed non-wire TEST-ENTRY table; this directory pairs
/// it with its location for reporting.
#[derive(Debug, Clone)]
pub struct TestEntry {
    pub name: String,
    pub module: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// The result of compiling a project *with* its tests: the image (carrying the
/// test functions and the TEST-ENTRY table), the export directory, and the test
/// directory `marrow test` reports against.
#[derive(Debug, Clone)]
pub struct CompiledTests {
    pub image: EncodedImage,
    pub exports: Vec<ExportEntry>,
    pub tests: Vec<TestEntry>,
}

/// Whether a compilation includes the project's `test` declarations. A production
/// `run` image excludes them (tests are not shipped); `marrow test` includes them,
/// adding the test functions and the TEST-ENTRY table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestMode {
    Exclude,
    Include,
}

/// A parsed module: its file identity (for spans and diagnostics), its dotted
/// module name (for export identity), and the parse tree.
struct Module {
    file: String,
    name: String,
    parsed: ParsedSource,
}

/// A lowered function's identity for recursion detection: its image index, the
/// functions it calls directly, and where to report a cycle.
struct LoweredFn {
    index: u16,
    file: String,
    name: String,
    span: SourceSpan,
    callees: Vec<u16>,
}

/// Compile a captured project into canonical program-image bytes and its export
/// directory, or return the typed source diagnostics that block it. The production
/// path: `test` declarations are not lowered and the TEST-ENTRY table is empty.
pub fn compile(project: &ProjectInput) -> Result<Compiled, Vec<SourceDiagnostic>> {
    let built = build(project, TestMode::Exclude)?;
    Ok(Compiled {
        image: built.image,
        exports: built.exports,
    })
}

/// Compile a captured project *with* its tests: the image additionally carries the
/// test functions and the closed TEST-ENTRY table, and the returned directory pairs
/// each test's title with its location for `marrow test`.
pub fn compile_with_tests(project: &ProjectInput) -> Result<CompiledTests, Vec<SourceDiagnostic>> {
    let built = build(project, TestMode::Include)?;
    Ok(CompiledTests {
        image: built.image,
        exports: built.exports,
        tests: built.tests,
    })
}

/// The image, export directory, and (when included) test directory a compilation
/// produced.
struct Built {
    image: EncodedImage,
    exports: Vec<ExportEntry>,
    tests: Vec<TestEntry>,
}

/// Compile a captured project, including or excluding its `test` declarations per
/// `mode`, or return the typed source diagnostics that block it.
fn build(project: &ProjectInput, mode: TestMode) -> Result<Built, Vec<SourceDiagnostic>> {
    let mut diagnostics = Vec::new();

    // Parse every module first. A parse error blocks semantic processing, mirroring
    // the total-parser contract: semantics run only on `!has_errors`.
    let mut parsed: Vec<Module> = Vec::new();
    for module in project.modules() {
        let file = module.identity().as_str().to_string();
        let name = module.module().as_str().to_string();
        match std::str::from_utf8(module.source()) {
            Ok(source) => parsed.push(Module {
                file,
                name,
                parsed: parse_source(source),
            }),
            Err(_) => diagnostics.push(SourceDiagnostic {
                code: Code::CheckUnsupported.as_str(),
                file,
                line: 1,
                column: 1,
                message: "source file is not valid UTF-8".to_string(),
            }),
        }
    }
    for module in &parsed {
        for diagnostic in &module.parsed.diagnostics {
            if diagnostic.severity == marrow_syntax::Severity::Error {
                diagnostics.push(SourceDiagnostic::at(
                    diagnostic.code,
                    &module.file,
                    diagnostic.span,
                    diagnostic.message.clone(),
                ));
            }
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // The source-root-relative path is the authority for module identity. A file
    // that declares a `module` header is an importable module and must spell the
    // path-derived name (with `::` as the dotted separator). A file with no header
    // is a single-file script: it keeps a path-derived identity for its own scope
    // and its exports, but is not importable by module path.
    let mut module_names: BTreeSet<String> = BTreeSet::new();
    for module in &parsed {
        if let Some(header) = &module.parsed.file.module {
            let declared = header.name.replace("::", ".");
            if declared == module.name {
                module_names.insert(module.name.clone());
            } else {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckModulePath.as_str(),
                    &module.file,
                    header.span,
                    format!(
                        "module header `{}` does not match its path; expected `module {}`",
                        header.name,
                        module.name.replace('.', "::")
                    ),
                ));
            }
        }
    }

    // Each module's `use` bindings (final segment -> dotted target). A `use` must
    // name an importable project module; two imports binding the same final segment
    // in one module are ambiguous.
    let mut imports: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for module in &parsed {
        let bindings = imports.entry(module.name.clone()).or_default();
        for use_decl in &module.parsed.file.uses {
            let target = use_decl.name.replace("::", ".");
            let segment = target
                .rsplit('.')
                .next()
                .unwrap_or(target.as_str())
                .to_string();
            if !module_names.contains(&target) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckImport.as_str(),
                    &module.file,
                    use_decl.span,
                    format!("no module `{}` in this project", use_decl.name),
                ));
                continue;
            }
            if bindings.iter().any(|(seg, _)| seg == &segment) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckImport.as_str(),
                    &module.file,
                    use_decl.span,
                    format!("import `{segment}` is already bound by another `use` in this module"),
                ));
                continue;
            }
            bindings.push((segment, target));
        }
    }

    // A module has at most one function with a given name, so an unqualified or
    // qualified call resolves to one target.
    reject_duplicate_functions(&parsed, &mut diagnostics);

    // The function signatures paired with their dotted module, in declaration order
    // (the order lowering assigns image indices).
    let functions: Vec<(String, &FunctionDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Function(function) = decl {
                    Some((module.name.clone(), function))
                } else {
                    None
                }
            })
        })
        .collect();

    // Build the named types — transparent aliases plus the single project record
    // type — and the function signatures before body lowering, so annotations,
    // constructors, field reads, and forward calls resolve.
    let mut draft = ImageDraft::new();
    let aliases: Vec<(String, &AliasDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Alias(alias) = decl {
                    Some((module.file.clone(), alias))
                } else {
                    None
                }
            })
        })
        .collect();
    let resources: Vec<(String, &ResourceDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Resource(resource) = decl {
                    Some((module.file.clone(), resource))
                } else {
                    None
                }
            })
        })
        .collect();
    let records = TypeRegistry::build(&mut draft, &aliases, &resources, &mut diagnostics);
    let stores: Vec<(String, &StoreDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Store(store) = decl {
                    Some((module.file.clone(), store))
                } else {
                    None
                }
            })
        })
        .collect();
    let durable = DurableRegistry::build(&mut draft, &records, &stores, &mut diagnostics);
    let signatures = FunctionRegistry::build(&records, &functions, module_names, imports);

    // Module-private constants, evaluated before body lowering so a reference folds
    // to its value.
    let const_decls: Vec<(String, String, &ConstDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Const(konst) = decl {
                    Some((module.name.clone(), module.file.clone(), konst))
                } else {
                    None
                }
            })
        })
        .collect();
    let constants = ConstRegistry::build(&const_decls, &records, &mut diagnostics);

    // Lower each function, in the same order the registry assigned indices, minting
    // an export for each public function from its declaration path and recording its
    // direct-call edges for recursion detection. Other declarations are handled
    // above or not yet admitted.
    let mut exports: Vec<ExportEntry> = Vec::new();
    let mut lowered: Vec<LoweredFn> = Vec::new();
    for module in &parsed {
        for declaration in &module.parsed.file.declarations {
            match declaration {
                Declaration::Function(function) => {
                    let Some(result) = FnLowerer::lower(
                        &mut draft,
                        &records,
                        &durable,
                        &signatures,
                        &constants,
                        &mut diagnostics,
                        &module.file,
                        &module.name,
                        function,
                    ) else {
                        continue;
                    };
                    lowered.push(LoweredFn {
                        index: result.func.index(),
                        file: module.file.clone(),
                        name: function.name.clone(),
                        span: function.span,
                        callees: result.callees,
                    });
                    if function.public {
                        // The injectivity owner's own guard: every dotted module
                        // segment and the item must be ASCII identifiers before an
                        // ExportId is minted over them (see marrow-image::export_id).
                        // Unreachable through the current capture path, which already
                        // constrains both; kept so the id payload's injectivity never
                        // silently rests on an upstream layer alone.
                        if !valid_export_path(&module.name, &function.name) {
                            diagnostics.push(SourceDiagnostic::at(
                                Code::CheckModulePath.as_str(),
                                &module.file,
                                function.span,
                                format!(
                                    "export `{}` in module `{}` is not an ASCII \
                                     identifier path, so it cannot be exported",
                                    function.name, module.name
                                ),
                            ));
                            continue;
                        }
                        let id = ExportId::of_local(&module.name, &function.name);
                        draft.add_export(id, result.func);
                        exports.push(ExportEntry {
                            module: module.name.clone(),
                            item: function.name.clone(),
                            id,
                        });
                    }
                }
                // Constants are evaluated into the const registry above; aliases,
                // resources, and stores are handled by their own registries; test
                // declarations are lowered separately below, after every function
                // has an index.
                Declaration::Alias(_)
                | Declaration::Const(_)
                | Declaration::Resource(_)
                | Declaration::Store(_)
                | Declaration::Test(_) => {}
                other => diagnostics.push(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    &module.file,
                    declaration_span(other),
                    "this declaration is not yet supported on the beta line".to_string(),
                )),
            }
        }
    }

    // Lower each `test "name"` body into a storeless, zero-argument function and
    // bind its title into the TEST-ENTRY table (only when tests are included). Tests
    // are lowered after every function so their bodies' calls resolve and their own
    // indices follow the functions'. Titles are unique across the project.
    let mut tests: Vec<TestEntry> = Vec::new();
    if mode == TestMode::Include {
        for module in &parsed {
            for declaration in &module.parsed.file.declarations {
                let Declaration::Test(test) = declaration else {
                    continue;
                };
                if tests.iter().any(|existing| existing.name == test.name) {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckNameConflict.as_str(),
                        &module.file,
                        test.name_span,
                        format!("a test named `{}` is already declared", test.name),
                    ));
                    continue;
                }
                let Some(result) = FnLowerer::lower_test(
                    &mut draft,
                    &records,
                    &durable,
                    &signatures,
                    &constants,
                    &mut diagnostics,
                    &module.file,
                    &module.name,
                    &test.name,
                    &test.body,
                ) else {
                    continue;
                };
                lowered.push(LoweredFn {
                    index: result.func.index(),
                    file: module.file.clone(),
                    name: test.name.clone(),
                    span: test.span,
                    callees: result.callees,
                });
                let name_id = draft.intern_string(&test.name);
                draft.add_test_entry(name_id, result.func);
                tests.push(TestEntry {
                    name: test.name.clone(),
                    module: module.name.clone(),
                    file: module.file.clone(),
                    line: test.name_span.line,
                    column: test.name_span.column,
                });
            }
        }
    }

    // The compiled subset does not admit recursion: the direct-call graph must be
    // acyclic. Reported at check time so the source carries the diagnostic. The
    // verifier independently rejects any cycle that still reaches it (image.closure),
    // so this is a source-facing check, not the trust boundary. Only run it once
    // every function and test lowered, so the indices are aligned.
    if diagnostics.is_empty() {
        reject_recursion(&lowered, &mut diagnostics);
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let image = draft.encode().map_err(|error| {
        vec![SourceDiagnostic {
            code: Code::CheckUnsupported.as_str(),
            file: String::new(),
            line: 1,
            column: 1,
            message: format!("program exceeds a representational bound: {error}"),
        }]
    })?;
    Ok(Built {
        image,
        exports,
        tests,
    })
}

/// Report a `check.name_conflict` for every function name declared more than once
/// within a single module (a `Call` must resolve to a unique target). Functions of
/// the same name in different modules are distinct and do not conflict.
fn reject_duplicate_functions(parsed: &[Module], diagnostics: &mut Vec<SourceDiagnostic>) {
    for module in parsed {
        let mut seen: Vec<&str> = Vec::new();
        for declaration in &module.parsed.file.declarations {
            let Declaration::Function(function) = declaration else {
                continue;
            };
            if seen.contains(&function.name.as_str()) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckNameConflict.as_str(),
                    &module.file,
                    function.span,
                    format!(
                        "a function named `{}` is already declared in this module",
                        function.name
                    ),
                ));
            } else {
                seen.push(&function.name);
            }
        }
    }
}

/// Report `check.recursion` for every function that participates in a direct or
/// mutual recursion cycle. A function is on a cycle exactly when it can reach
/// itself by following direct calls, so each function is checked for reachability
/// back to itself over the edge set.
fn reject_recursion(lowered: &[LoweredFn], diagnostics: &mut Vec<SourceDiagnostic>) {
    // Adjacency by image index. Indices are dense (0..lowered.len()) and each
    // function appears once, so a plain vec keyed by index is exact.
    let mut callees: Vec<&[u16]> = vec![&[]; lowered.len()];
    for function in lowered {
        if (function.index as usize) < callees.len() {
            callees[function.index as usize] = &function.callees;
        }
    }
    for function in lowered {
        if reaches_self(function.index, &callees) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckRecursion.as_str(),
                &function.file,
                function.span,
                format!("`{}` is part of a recursive call cycle", function.name),
            ));
        }
    }
}

/// Whether `start` can reach itself by following direct calls.
fn reaches_self(start: u16, callees: &[&[u16]]) -> bool {
    let mut stack: Vec<u16> = callees
        .get(start as usize)
        .map(|targets| targets.to_vec())
        .unwrap_or_default();
    let mut visited = vec![false; callees.len()];
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if (node as usize) >= visited.len() || visited[node as usize] {
            continue;
        }
        visited[node as usize] = true;
        if let Some(targets) = callees.get(node as usize) {
            stack.extend_from_slice(targets);
        }
    }
    false
}

fn declaration_span(declaration: &Declaration) -> SourceSpan {
    match declaration {
        Declaration::Alias(decl) => decl.span,
        Declaration::Const(decl) => decl.span,
        Declaration::Resource(decl) => decl.span,
        Declaration::Store(decl) => decl.span,
        Declaration::Function(decl) => decl.span,
        Declaration::Enum(decl) => decl.span,
        Declaration::Evolve(decl) => decl.span,
        Declaration::Test(decl) => decl.span,
    }
}

/// Whether an export declaration path is valid to mint an [`ExportId`] over:
/// every dotted module segment and the item must be non-empty ASCII identifiers
/// (a letter or `_`, then letters, digits, or `_`; never a `.`). This is what
/// keeps the id payload's dotted `module` join injective over segments, so it is
/// checked here — immediately before minting — rather than assumed from capture.
fn valid_export_path(module: &str, item: &str) -> bool {
    module.split('.').all(is_ascii_identifier) && is_ascii_identifier(item)
}

/// Whether `text` is a non-empty ASCII identifier.
fn is_ascii_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::valid_export_path;

    /// The minting guard rejects every input class whose dotted join would break
    /// the ExportId payload's injectivity, even though the current capture path
    /// cannot produce them.
    #[test]
    fn export_path_validation_guards_the_id_payload() {
        // Ordinary declaration paths mint.
        assert!(valid_export_path("main", "run"));
        assert!(valid_export_path("shelf.books", "add"));
        assert!(valid_export_path("a_b", "_x1"));

        // Empty or dotted components would let two distinct declaration paths
        // collide on one payload.
        assert!(!valid_export_path("", "run"));
        assert!(!valid_export_path("a", ""));
        assert!(!valid_export_path("a..b", "run"));
        assert!(!valid_export_path("a.", "run"));
        assert!(!valid_export_path(".a", "run"));
        assert!(!valid_export_path("a", "b.c"));

        // Non-ASCII and non-identifier characters are outside the frozen payload
        // domain.
        assert!(!valid_export_path("caf\u{e9}", "run"));
        assert!(!valid_export_path("a", "r\u{e9}sum\u{e9}"));
        assert!(!valid_export_path("a-b", "run"));
        assert!(!valid_export_path("1a", "run"));
        assert!(!valid_export_path("a", "1run"));
        assert!(!valid_export_path("a b", "run"));
    }
}
