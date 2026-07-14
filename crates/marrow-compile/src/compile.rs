//! The storeless subset checker and lowering to an [`ImageDraft`].
//!
//! The compiler opens no store and mints no verified image: it parses source,
//! checks the current subset, and lowers to canonical image bytes the independent
//! verifier rechecks. Coverage grows one slice at a time; a well-formed construct
//! outside the current subset is a typed `check.unsupported` diagnostic, never a
//! silent drop.

use marrow_codes::Code;
use marrow_image::{EncodedImage, ImageDraft};
use marrow_project::ProjectInput;
use marrow_syntax::{
    Declaration, FunctionDecl, ParsedSource, ResourceDecl, SourceSpan, parse_source,
};

use crate::diag::SourceDiagnostic;
use crate::lower::{FnLowerer, FunctionRegistry};
use crate::record::RecordRegistry;

/// Compile a captured project into canonical program-image bytes, or return the
/// typed source diagnostics that block it.
pub fn compile(project: &ProjectInput) -> Result<EncodedImage, Vec<SourceDiagnostic>> {
    let mut diagnostics = Vec::new();

    // Parse every module first. A parse error blocks semantic processing, mirroring
    // the total-parser contract: semantics run only on `!has_errors`.
    let mut parsed: Vec<(String, ParsedSource)> = Vec::new();
    for module in project.modules() {
        let path = module.identity().as_str().to_string();
        match std::str::from_utf8(module.source()) {
            Ok(source) => parsed.push((path, parse_source(source))),
            Err(_) => diagnostics.push(SourceDiagnostic {
                code: Code::CheckUnsupported.as_str(),
                file: path,
                line: 1,
                column: 1,
                message: "source file is not valid UTF-8".to_string(),
            }),
        }
    }
    for (path, module) in &parsed {
        for diagnostic in &module.diagnostics {
            if diagnostic.severity == marrow_syntax::Severity::Error {
                diagnostics.push(SourceDiagnostic::at(
                    diagnostic.code,
                    path,
                    diagnostic.span,
                    diagnostic.message.clone(),
                ));
            }
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // Function names must be unique project-wide so a `Call` resolves to one target
    // (interim export mapping; `ExportId` lands at C00). A duplicate is a conflict.
    let functions: Vec<(String, &FunctionDecl)> = parsed
        .iter()
        .flat_map(|(path, module)| {
            module.file.declarations.iter().filter_map(move |decl| {
                if let Declaration::Function(function) = decl {
                    Some((path.clone(), function))
                } else {
                    None
                }
            })
        })
        .collect();
    reject_duplicate_functions(&functions, &mut diagnostics);

    // Build the single project record type and the function signatures before body
    // lowering, so constructors, field reads, and forward calls resolve.
    let mut draft = ImageDraft::new();
    let resources: Vec<(String, &ResourceDecl)> = parsed
        .iter()
        .flat_map(|(path, module)| {
            module.file.declarations.iter().filter_map(move |decl| {
                if let Declaration::Resource(resource) = decl {
                    Some((path.clone(), resource))
                } else {
                    None
                }
            })
        })
        .collect();
    let records = RecordRegistry::build(&mut draft, &resources, &mut diagnostics);
    let signatures = FunctionRegistry::build(&records, &functions);

    // Lower each function, in the same order the registry assigned indices. Other
    // declarations are handled above or not yet admitted.
    for (path, module) in &parsed {
        for declaration in &module.file.declarations {
            match declaration {
                Declaration::Function(function) => {
                    FnLowerer::lower(
                        &mut draft,
                        &records,
                        &signatures,
                        &mut diagnostics,
                        path,
                        function,
                    );
                }
                Declaration::Resource(_) => {}
                other => diagnostics.push(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    path,
                    declaration_span(other),
                    "this declaration is not yet supported on the beta line".to_string(),
                )),
            }
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    draft.encode().map_err(|error| {
        vec![SourceDiagnostic {
            code: Code::CheckUnsupported.as_str(),
            file: String::new(),
            line: 1,
            column: 1,
            message: format!("program exceeds a representational bound: {error}"),
        }]
    })
}

/// Report a `check.name_conflict` for every function name declared more than once
/// across the project (a `Call`, and the interim export mapping, must resolve to a
/// unique target).
fn reject_duplicate_functions(
    functions: &[(String, &FunctionDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for (path, function) in functions {
        if seen.contains(&function.name.as_str()) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                path,
                function.span,
                format!("a function named `{}` is already declared", function.name),
            ));
        } else {
            seen.push(&function.name);
        }
    }
}

fn declaration_span(declaration: &Declaration) -> SourceSpan {
    match declaration {
        Declaration::Const(decl) => decl.span,
        Declaration::Resource(decl) => decl.span,
        Declaration::Store(decl) => decl.span,
        Declaration::Function(decl) => decl.span,
        Declaration::Enum(decl) => decl.span,
        Declaration::Evolve(decl) => decl.span,
    }
}
