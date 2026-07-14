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
use marrow_syntax::{Declaration, ParsedSource, SourceSpan, parse_source};

use crate::diag::SourceDiagnostic;
use crate::lower::FnLowerer;

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

    // Export = project-unique `pub fn` name (interim mapping; `ExportId` lands at
    // C00). A duplicate is a name conflict, detected before lowering.
    reject_duplicate_exports(&parsed, &mut diagnostics);

    // Lower each declaration. Only functions are admitted at this slice.
    let mut draft = ImageDraft::new();
    for (path, module) in &parsed {
        for declaration in &module.file.declarations {
            match declaration {
                Declaration::Function(function) => {
                    FnLowerer::lower(&mut draft, &mut diagnostics, path, function);
                }
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

/// Report a `check.name_conflict` for every `pub fn` name declared more than once
/// across the project.
fn reject_duplicate_exports(
    parsed: &[(String, ParsedSource)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for (path, module) in parsed {
        for declaration in &module.file.declarations {
            if let Declaration::Function(function) = declaration
                && function.public
            {
                if seen.contains(&function.name.as_str()) {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckNameConflict.as_str(),
                        path,
                        function.span,
                        format!(
                            "a public function named `{}` is already declared",
                            function.name
                        ),
                    ));
                } else {
                    seen.push(&function.name);
                }
            }
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
