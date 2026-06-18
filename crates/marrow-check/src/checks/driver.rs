//! The resolved-file pass: import resolution over the full module set, the file
//! prelude (import aliases plus module constants), per-declaration type-annotation
//! checks, and the missing-return analysis.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_schema::ReturnPresence;
use marrow_schema::Type;
use marrow_syntax::SourceSpan;

use crate::enums::{
    annotation_type_known, annotation_unknown_identity_name, private_enum_type_reference,
    resolve_diagnosed_annotation_type, resolve_type,
};
use crate::infer::infer_type;
use crate::{
    CHECK_MISSING_RETURN, CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_TYPE, CHECK_UNRESOLVED_IMPORT,
    CheckDiagnostic, CheckReport, CheckedProgram, DiagnosticPayload, MarrowType, build_alias_map,
    check_rejected_surface, is_resolved_import, push_schema_error,
};

use super::returns::{block_returns, check_return_values};
use super::statements::check_function_types;

/// The shared tail of library and test checking: once the resolvable module set
/// and `program` are known, import resolution and the type pass run identically;
/// only pass 1 differs and stays in the caller.
pub(crate) struct ResolvedFileCheck<'a> {
    pub(crate) files: &'a [marrow_project::ModuleFile],
    pub(crate) parsed_files: &'a [(&'a marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    pub(crate) module_name_policy: ModuleNamePolicy,
    pub(crate) resolvable: &'a HashMap<String, PathBuf>,
    pub(crate) program: &'a CheckedProgram,
}

pub(crate) fn check_resolved_files(input: ResolvedFileCheck<'_>, report: &mut CheckReport) {
    let ResolvedFileCheck {
        files,
        parsed_files,
        module_name_policy,
        resolvable,
        program,
    } = input;

    // Every `use` must name a resolvable module now that the full set is known.
    for (file, parsed) in parsed_files {
        for use_decl in &parsed.file.uses {
            if !is_resolved_import(&use_decl.name, resolvable) {
                report.diagnostics.push(
                    CheckDiagnostic::error(
                        CHECK_UNRESOLVED_IMPORT,
                        &file.path,
                        use_decl.span,
                        format!("cannot resolve import `{}`", use_decl.name),
                    )
                    .with_payload(DiagnosticPayload::UnresolvedImport(use_decl.name.clone())),
                );
            }
        }
    }

    for (file, parsed) in parsed_files {
        check_file_types(program, &file.path, parsed, &mut report.diagnostics);
    }

    // A file that failed to parse or read is excluded from the program, so exact
    // imports of its module and qualified calls into it would look unresolved
    // even though the source may define them. Suppress only those reports; other
    // clean files' local resolution diagnostics remain trustworthy.
    let incomplete_modules =
        incomplete_module_names(files, parsed_files, module_name_policy, program);
    if !incomplete_modules.is_empty() {
        report
            .diagnostics
            .retain(|diagnostic| match &diagnostic.payload {
                DiagnosticPayload::UnresolvedImport(name) => !incomplete_modules.contains(name),
                DiagnosticPayload::UnresolvedCall(name) => {
                    !references_incomplete_module_member(name, &incomplete_modules)
                }
                DiagnosticPayload::UnknownType(_)
                | DiagnosticPayload::Schema(_)
                | DiagnosticPayload::DuplicateDeclaration { .. }
                | DiagnosticPayload::SurfaceCollision { .. }
                | DiagnosticPayload::DuplicateModule { .. }
                | DiagnosticPayload::ModulePath { .. }
                | DiagnosticPayload::ReservedTestModulePathSegment { .. }
                | DiagnosticPayload::DuplicateRootOwner { .. }
                | DiagnosticPayload::RejectedSurface(_)
                | DiagnosticPayload::Enum(_)
                | DiagnosticPayload::PrivateEnum(_)
                | DiagnosticPayload::DuplicateNamedArgument(_)
                | DiagnosticPayload::AppendTarget(_)
                | DiagnosticPayload::ConversionUnsupportedSource(_)
                | DiagnosticPayload::InterpolationUnsupportedSource { .. }
                | DiagnosticPayload::ReservedCatalogPathReuse { .. }
                | DiagnosticPayload::CatalogIntent(_)
                | DiagnosticPayload::SuggestedIndex { .. }
                | DiagnosticPayload::RequiredAbsent { .. }
                | DiagnosticPayload::TypeMismatch { .. }
                | DiagnosticPayload::None => true,
            });
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ModuleNamePolicy {
    DeclaredOrPath,
    PathOnly,
}

fn incomplete_module_names(
    files: &[marrow_project::ModuleFile],
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    module_name_policy: ModuleNamePolicy,
    program: &CheckedProgram,
) -> HashSet<String> {
    let complete_modules: HashSet<&str> = program
        .modules
        .iter()
        .map(|module| module.name.as_str())
        .collect();
    let parsed_paths: HashSet<&Path> = parsed_files
        .iter()
        .map(|(file, _)| file.path.as_path())
        .collect();
    let mut modules = HashSet::new();
    for file in files {
        if !parsed_paths.contains(file.path.as_path())
            && let Some(module) = &file.module_name
        {
            insert_incomplete_module(&mut modules, &complete_modules, module);
        }
    }
    for (file, parsed) in parsed_files {
        if parsed.has_errors() {
            if matches!(module_name_policy, ModuleNamePolicy::DeclaredOrPath)
                && let Some(module) = &parsed.file.module
            {
                insert_incomplete_module(&mut modules, &complete_modules, &module.name);
            }
            if let Some(module) = &file.module_name {
                insert_incomplete_module(&mut modules, &complete_modules, module);
            }
        }
    }
    modules
}

fn insert_incomplete_module(
    modules: &mut HashSet<String>,
    complete_modules: &HashSet<&str>,
    name: &str,
) {
    if !complete_modules.contains(name) {
        modules.insert(name.to_string());
    }
}

fn references_incomplete_module_member(name: &str, modules: &HashSet<String>) -> bool {
    modules.iter().any(|module| {
        name.strip_prefix(module)
            .is_some_and(|rest| rest.starts_with("::"))
    })
}

/// The file-level prelude every function body in a file shares: its short→full
/// import aliases and its module-level constants, both of which are in scope
/// before any function's own parameters and locals.
pub(crate) struct FilePrelude {
    pub(crate) aliases: HashMap<String, Vec<String>>,
    pub(crate) module_constants: HashMap<String, MarrowType>,
}

/// Build a file's [`FilePrelude`], in source order so an earlier constant is in
/// scope for a later one. Both the type pass and editor queries start here, so
/// the bindings a function body sees are derived in exactly one place.
pub(crate) fn file_prelude(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
) -> FilePrelude {
    let import_paths: Vec<String> = parsed
        .file
        .uses
        .iter()
        .map(|use_decl| use_decl.name.clone())
        .collect();
    let aliases = build_alias_map(&import_paths);
    // Top-level constants are in scope (bare) for the file's functions, so a typed
    // use like `var x: int = M` resolves rather than false-positiving
    // `check.untyped_value`. Initializer validity is owned by the const-value pass,
    // so the inference diagnostics raised here are discarded.
    let mut module_constants: HashMap<String, MarrowType> = HashMap::new();
    for declaration in &parsed.file.declarations {
        if let marrow_syntax::Declaration::Const(constant) = declaration {
            let ty = match (&constant.ty, &constant.value) {
                (Some(ty), _) => resolve_diagnosed_annotation_type(ty, program, &aliases, file),
                (None, Some(value)) => infer_type(
                    program,
                    value,
                    std::slice::from_ref(&module_constants),
                    &aliases,
                    file,
                    &mut Vec::new(),
                ),
                // The value did not parse; the parser already reported the error.
                (None, None) => MarrowType::Unknown,
            };
            module_constants.insert(constant.name.clone(), ty);
        }
    }
    FilePrelude {
        aliases,
        module_constants,
    }
}

/// Run the type pass over one parsed file: unknown-type annotations, return-value
/// placement, the expression/statement type checks, and missing-return analysis.
/// Shared by library files and test scripts.
pub(crate) fn check_file_types(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let has_parse_errors = parsed.has_errors();
    let FilePrelude {
        aliases,
        module_constants,
    } = file_prelude(program, file, parsed);
    check_rejected_surface(program, file, parsed, diagnostics);
    let annotation_context = TypeAnnotationContext {
        program,
        aliases: &aliases,
        file,
    };
    let stored_resources: HashSet<&str> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            marrow_syntax::Declaration::Store(store) => Some(store.resource.as_str()),
            _ => None,
        })
        .collect();
    for declaration in &parsed.file.declarations {
        match declaration {
            marrow_syntax::Declaration::Function(function) => {
                for param in &function.params {
                    check_type_annotation(
                        &param.ty,
                        function.span,
                        &annotation_context,
                        diagnostics,
                    );
                }
                if let Some(return_type) = &function.return_type {
                    check_type_annotation(
                        return_type,
                        function.span,
                        &annotation_context,
                        diagnostics,
                    );
                }
                if has_parse_errors {
                    continue;
                }
                check_return_values(
                    file,
                    &function.body,
                    function.return_type.is_some(),
                    match function.return_presence {
                        marrow_syntax::FunctionReturnPresence::Always => ReturnPresence::Always,
                        marrow_syntax::FunctionReturnPresence::MaybePresent => {
                            ReturnPresence::MaybePresent
                        }
                    },
                    diagnostics,
                );
                check_block_type_annotations(&function.body, &annotation_context, diagnostics);
                check_function_types(
                    program,
                    file,
                    function,
                    &module_constants,
                    &aliases,
                    diagnostics,
                );
                if function.return_type.is_some() && !block_returns(&function.body) {
                    diagnostics.push(CheckDiagnostic::error(
                        CHECK_MISSING_RETURN,
                        file,
                        function.span,
                        format!(
                            "function `{}` may reach its end without returning a value",
                            function.name
                        ),
                    ));
                }
            }
            marrow_syntax::Declaration::Const(constant) => {
                if let Some(ty) = &constant.ty {
                    check_type_annotation(ty, constant.span, &annotation_context, diagnostics);
                }
            }
            marrow_syntax::Declaration::Resource(resource) => {
                if stored_resources.contains(resource.name.as_str()) {
                    check_resource_identity_annotations(
                        &resource.members,
                        &annotation_context,
                        diagnostics,
                    );
                    check_qualified_saved_named_field_annotations(
                        &resource.members,
                        &annotation_context,
                        diagnostics,
                    );
                } else {
                    check_resource_type_annotations(
                        &resource.members,
                        &annotation_context,
                        diagnostics,
                    );
                }
            }
            // These declarations do not expose checker-owned type annotations here.
            marrow_syntax::Declaration::Store(_)
            | marrow_syntax::Declaration::Enum(_)
            | marrow_syntax::Declaration::Evolve(_)
            | marrow_syntax::Declaration::Surface(_) => {}
        }
    }
}

fn check_qualified_saved_named_field_annotations(
    members: &[marrow_syntax::ResourceMember],
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for error in marrow_schema::check_saved_named_member_fields_with(members, |name| {
        if !name.contains("::") {
            return true;
        }
        matches!(
            crate::enums::resolve_enum_type(
                &Type::Named(name.to_string()),
                context.program,
                context.aliases,
                context.file,
            ),
            Some(MarrowType::Enum { .. })
        )
    }) {
        push_schema_error(context.file, diagnostics, error);
    }
}

struct TypeAnnotationContext<'a> {
    program: &'a CheckedProgram,
    aliases: &'a HashMap<String, Vec<String>>,
    file: &'a Path,
}

fn check_type_annotation(
    ty: &marrow_syntax::TypeRef,
    span: SourceSpan,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let schema_type = Type::resolve(ty);
    let resolved_type = resolve_type(ty, context.program, context.aliases, context.file);
    if !contains_resource_type(&resolved_type)
        && let Some(private) =
            private_enum_type_reference(ty, context.program, context.aliases, context.file)
    {
        diagnostics.push(
            CheckDiagnostic::error(
                CHECK_PRIVATE_ENUM,
                context.file,
                span,
                format!(
                    "enum `{private}` is private to its module; mark it `pub` to use it from another module"
                ),
            )
            .with_payload(DiagnosticPayload::PrivateEnum(private)),
        );
        return;
    }
    let unknown_identity = annotation_unknown_identity_name(&schema_type, context.program);
    if unknown_identity.is_some() || !annotation_type_known(&schema_type, &resolved_type) {
        let name = unknown_identity.unwrap_or_else(|| ty.text.trim().to_string());
        diagnostics.push(
            CheckDiagnostic::error(
                CHECK_UNKNOWN_TYPE,
                context.file,
                span,
                format!("unknown type `{name}`"),
            )
            .with_payload(DiagnosticPayload::UnknownType(schema_type)),
        );
    }
}

fn contains_resource_type(ty: &MarrowType) -> bool {
    match ty {
        MarrowType::Resource(_) => true,
        MarrowType::Sequence(element) => contains_resource_type(element),
        MarrowType::LocalTree { keys, value } => {
            keys.iter().any(contains_resource_type) || contains_resource_type(value)
        }
        _ => false,
    }
}

fn check_resource_type_annotations(
    members: &[marrow_syntax::ResourceMember],
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for member in members {
        match member {
            marrow_syntax::ResourceMember::Field(field) => {
                for key in &field.keys {
                    check_type_annotation(&key.ty, key.ty.span, context, diagnostics);
                }
                check_type_annotation(&field.ty, field.ty.span, context, diagnostics);
            }
            marrow_syntax::ResourceMember::Group(group) => {
                for key in &group.keys {
                    check_type_annotation(&key.ty, key.ty.span, context, diagnostics);
                }
                check_resource_type_annotations(&group.members, context, diagnostics);
            }
        }
    }
}

fn check_resource_identity_annotations(
    members: &[marrow_syntax::ResourceMember],
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for member in members {
        match member {
            marrow_syntax::ResourceMember::Field(field) => {
                check_unknown_identity_type_annotation(
                    &field.ty,
                    field.ty.span,
                    context,
                    diagnostics,
                );
            }
            marrow_syntax::ResourceMember::Group(group) => {
                check_resource_identity_annotations(&group.members, context, diagnostics);
            }
        }
    }
}

fn check_unknown_identity_type_annotation(
    ty: &marrow_syntax::TypeRef,
    span: SourceSpan,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(name) = unknown_identity_type_ref(ty, context) else {
        return;
    };
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_UNKNOWN_TYPE,
            context.file,
            span,
            format!("unknown type `{name}`"),
        )
        .with_payload(DiagnosticPayload::UnknownType(Type::resolve(ty))),
    );
}

fn unknown_identity_type_ref(
    ty: &marrow_syntax::TypeRef,
    context: &TypeAnnotationContext<'_>,
) -> Option<String> {
    annotation_unknown_identity_name(&Type::resolve(ty), context.program)
}

fn check_block_type_annotations(
    block: &marrow_syntax::Block,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        check_statement_type_annotations(statement, context, diagnostics);
    }
}

fn check_statement_type_annotations(
    statement: &marrow_syntax::Statement,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Statement;
    match statement {
        Statement::Const {
            ty: Some(ty), span, ..
        } => {
            check_type_annotation(ty, *span, context, diagnostics);
        }
        Statement::Var { keys, ty, span, .. } => {
            for key in keys {
                check_type_annotation(&key.ty, *span, context, diagnostics);
            }
            if let Some(ty) = ty {
                check_type_annotation(ty, *span, context, diagnostics);
            }
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        }
        | Statement::IfConst {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            check_block_type_annotations(then_block, context, diagnostics);
            for else_if in else_ifs {
                check_block_type_annotations(&else_if.block, context, diagnostics);
            }
            if let Some(block) = else_block {
                check_block_type_annotations(block, context, diagnostics);
            }
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => {
            check_block_type_annotations(body, context, diagnostics);
        }
        Statement::Try { body, catch, .. } => {
            check_block_type_annotations(body, context, diagnostics);
            if let Some(catch) = catch {
                if let Some(ty) = &catch.ty {
                    check_type_annotation(ty, catch.block.span, context, diagnostics);
                }
                check_block_type_annotations(&catch.block, context, diagnostics);
            }
        }
        Statement::Match { arms, .. } => {
            for arm in arms {
                check_block_type_annotations(&arm.block, context, diagnostics);
            }
        }
        Statement::Const { ty: None, .. }
        | Statement::Assign { .. }
        | Statement::Delete { .. }
        | Statement::Return { .. }
        | Statement::ReturnAbsent { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::Expr { .. } => {}
    }
}
