//! The storeless subset checker and lowering to an [`ImageDraft`].
//!
//! The compiler opens no store and mints no verified image: it parses source,
//! checks the current subset, and lowers to canonical image bytes the independent
//! verifier rechecks. Coverage grows one slice at a time; a well-formed construct
//! outside the current subset is a typed `check.unsupported` diagnostic, never a
//! silent drop.

use marrow_codes::Code;
use marrow_image::{EncodedImage, FunctionDef, ImageDraft, ImageType, Instr, SpanEntry};
use marrow_project::ProjectInput;
use marrow_syntax::{
    Declaration, Expression, FunctionDecl, LiteralKind, ParsedSource, SourceSpan, Statement,
    TypeExpr, decode_string_literal, parse_source,
};

use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;

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

    // Lower each declaration. Only functions are admitted at this slice.
    let mut draft = ImageDraft::new();
    let mut exports: Vec<(String, SourceSpan, String)> = Vec::new(); // (name, span, file)
    for (path, module) in &parsed {
        for declaration in &module.file.declarations {
            match declaration {
                Declaration::Function(function) => {
                    lower_function(&mut draft, path, function, &mut exports, &mut diagnostics);
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

    // Reject duplicate export names (interim mapping; ExportId lands at C00).
    for i in 0..exports.len() {
        for j in (i + 1)..exports.len() {
            if exports[i].0 == exports[j].0 {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckNameConflict.as_str(),
                    &exports[j].2,
                    exports[j].1,
                    format!(
                        "a public function named `{}` is already declared",
                        exports[j].0
                    ),
                ));
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

/// Lower one function of the current subset: no params, a scalar return type, and a
/// body that is exactly `return <matching scalar literal>`.
fn lower_function(
    draft: &mut ImageDraft,
    file: &str,
    function: &FunctionDecl,
    exports: &mut Vec<(String, SourceSpan, String)>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let before = diagnostics.len();

    if !function.params.is_empty() {
        diagnostics.push(unsupported(file, function.span, "function parameters"));
    }

    let return_type = match &function.return_type {
        Some(TypeExpr::Name { text, span }) => match ScalarType::from_spelling(text) {
            Some(scalar) => Some((scalar, *span)),
            None => {
                diagnostics.push(unsupported(file, *span, "this return type"));
                None
            }
        },
        Some(other) => {
            diagnostics.push(unsupported(file, other.span(), "this return type"));
            None
        }
        None => {
            diagnostics.push(unsupported(
                file,
                function.span,
                "a function without a return type",
            ));
            None
        }
    };

    let literal = match function.body.statements.as_slice() {
        [
            Statement::Return {
                value: Some(expr), ..
            },
        ] => Some(expr),
        _ => {
            diagnostics.push(unsupported(
                file,
                function.span,
                "a function body other than a single `return <literal>`",
            ));
            None
        }
    };

    let (Some((return_scalar, _)), Some(expr)) = (return_type, literal) else {
        return;
    };

    let Some(instr) = lower_scalar_literal(draft, file, expr, return_scalar, diagnostics) else {
        return;
    };

    if diagnostics.len() != before {
        return;
    }

    let name_id = draft.intern_string(&function.name);
    let source_id = draft.intern_string(file);
    let return_ref = ImageType::scalar(return_scalar.image());
    let return_span = expr.span();
    let func_id = draft.add_function(FunctionDef {
        name: name_id,
        source: source_id,
        params: Vec::new(),
        ret: return_ref,
        local_count: 0,
        code: vec![instr, Instr::Return],
        spans: vec![SpanEntry {
            instr_index: 0,
            line: return_span.line,
            column: return_span.column,
        }],
    });

    if function.public {
        exports.push((function.name.clone(), function.span, file.to_string()));
        let export_name = draft.intern_string(&function.name);
        draft.add_export(export_name, func_id);
    }
}

/// Lower a scalar literal whose type must match `expected`, returning the
/// `ConstLoad` that pushes it.
fn lower_scalar_literal(
    draft: &mut ImageDraft,
    file: &str,
    expr: &Expression,
    expected: ScalarType,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Instr> {
    let Expression::Literal { kind, text, span } = expr else {
        diagnostics.push(unsupported(file, expr.span(), "this expression"));
        return None;
    };
    let (scalar, const_id) = match kind {
        LiteralKind::Integer => {
            let Some(value) = parse_int(text) else {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    *span,
                    "integer literal is out of the 64-bit range".to_string(),
                ));
                return None;
            };
            (ScalarType::Int, draft.intern_int(value))
        }
        LiteralKind::Bool => {
            let value = text == "true";
            (ScalarType::Bool, draft.intern_bool(value))
        }
        LiteralKind::String => {
            let Ok(decoded) = decode_string_literal(text) else {
                diagnostics.push(unsupported(file, *span, "this string literal"));
                return None;
            };
            (ScalarType::Text, draft.intern_text(&decoded))
        }
        _ => {
            diagnostics.push(unsupported(file, *span, "this literal"));
            return None;
        }
    };
    if scalar != expected {
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            file,
            *span,
            format!(
                "returned {} where {} is required",
                scalar.spelling(),
                expected.spelling()
            ),
        ));
        return None;
    }
    Some(Instr::ConstLoad(const_id.index()))
}

fn parse_int(text: &str) -> Option<i64> {
    text.replace('_', "").parse().ok()
}

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}
