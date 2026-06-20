//! The resolved-file pass: import resolution over the full module set, the file
//! prelude (import aliases plus module constants), per-declaration type-annotation
//! checks, and the missing-return analysis.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marrow_schema::{ReturnPresence, Type};
use marrow_syntax::SourceSpan;

use crate::enums::{
    EnumAnnotationResolution, ambiguous_enum_annotation_diagnostic, annotation_type_known,
    annotation_unknown_identity_name, resolve_diagnosed_annotation_type, resolve_enum_annotation,
    resolve_type, same_module_private_enum,
};
use crate::infer::infer_type;
use crate::{
    CHECK_EXPOSED_PRIVATE_ENUM, CHECK_MISSING_RETURN, CHECK_PRIVATE_ENUM, CHECK_UNKNOWN_TYPE,
    CHECK_UNRESOLVED_IMPORT, CheckDiagnostic, CheckReport, CheckedProgram, DiagnosticPayload,
    MarrowType, build_alias_map, check_rejected_surface, has_duplicate_error, is_resolved_import,
    push_schema_error,
};

use super::operators::check_assignment;
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
    pub(crate) backing_invalidations:
        Option<&'a mut crate::backing_validity::PendingBackingInvalidations>,
}

pub(crate) fn check_resolved_files(
    input: ResolvedFileCheck<'_>,
    report: &mut CheckReport,
) -> HashSet<String> {
    let ResolvedFileCheck {
        files,
        parsed_files,
        module_name_policy,
        resolvable,
        program,
        mut backing_invalidations,
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
        check_file_types(
            program,
            &file.path,
            parsed,
            backing_invalidations.as_deref_mut(),
            &mut report.diagnostics,
        );
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
                | DiagnosticPayload::AmbiguousType { .. }
                | DiagnosticPayload::Schema(_)
                | DiagnosticPayload::DuplicateDeclaration { .. }
                | DiagnosticPayload::SurfaceCollision { .. }
                | DiagnosticPayload::SurfaceTarget(_)
                | DiagnosticPayload::SurfaceField(_)
                | DiagnosticPayload::SurfaceAction(_)
                | DiagnosticPayload::SurfaceComputedRead(_)
                | DiagnosticPayload::DuplicateModule { .. }
                | DiagnosticPayload::ModulePath { .. }
                | DiagnosticPayload::DefaultEntry { .. }
                | DiagnosticPayload::ReservedTestModulePathSegment { .. }
                | DiagnosticPayload::DuplicateRootOwner { .. }
                | DiagnosticPayload::RejectedSurface(_)
                | DiagnosticPayload::Enum(_)
                | DiagnosticPayload::PrivateEnum(_)
                | DiagnosticPayload::ExposedPrivateEnum { .. }
                | DiagnosticPayload::DuplicateNamedArgument(_)
                | DiagnosticPayload::AppendTarget(_)
                | DiagnosticPayload::ConversionUnsupportedSource(_)
                | DiagnosticPayload::RenderUnsupportedSource { .. }
                | DiagnosticPayload::ReservedCatalogPathReuse { .. }
                | DiagnosticPayload::CatalogIntent(_)
                | DiagnosticPayload::SuggestedIndex { .. }
                | DiagnosticPayload::RequiredAbsent { .. }
                | DiagnosticPayload::TypeMismatch { .. }
                | DiagnosticPayload::SavedCollectionByValue { .. }
                | DiagnosticPayload::LayerNotValue { .. }
                | DiagnosticPayload::None => true,
            });
    }
    incomplete_modules
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
/// scope for a later one. Both the type pass and editor lookups start here, so
/// the bindings a function body sees are derived in exactly one place.
pub(crate) fn file_prelude(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
) -> FilePrelude {
    build_file_prelude(program, file, parsed, None)
}

fn checked_file_prelude(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> FilePrelude {
    build_file_prelude(program, file, parsed, Some(diagnostics))
}

fn build_file_prelude(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    mut diagnostics: Option<&mut Vec<CheckDiagnostic>>,
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
    // `check.untyped_value`.
    let mut module_constants: HashMap<String, MarrowType> = HashMap::new();
    for declaration in &parsed.file.declarations {
        if let marrow_syntax::Declaration::Const(constant) = declaration {
            let annotation_type = constant
                .ty
                .as_ref()
                .map(|ty| resolve_diagnosed_annotation_type(ty, program, &aliases, file));
            let value_type = constant.value.as_ref().map(|value| {
                infer_module_const_value(
                    program,
                    value,
                    &module_constants,
                    &aliases,
                    file,
                    diagnostics.as_deref_mut(),
                )
            });
            if let (Some(expected), Some(found), Some(diagnostics)) =
                (&annotation_type, &value_type, diagnostics.as_deref_mut())
            {
                check_assignment(file, constant.span, expected, found, diagnostics);
            }
            let ty = match (annotation_type, value_type) {
                (Some(ty), _) => ty,
                (None, Some(value_type)) => value_type,
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

fn infer_module_const_value(
    program: &CheckedProgram,
    value: &marrow_syntax::Expression,
    module_constants: &HashMap<String, MarrowType>,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    diagnostics: Option<&mut Vec<CheckDiagnostic>>,
) -> MarrowType {
    let scope = std::slice::from_ref(module_constants);
    let Some(diagnostics) = diagnostics else {
        return infer_type(program, value, scope, aliases, file, &mut Vec::new());
    };
    let mut emitted = Vec::new();
    let ty = infer_type(program, value, scope, aliases, file, &mut emitted);
    for diagnostic in emitted {
        if !has_duplicate_error(diagnostics, &diagnostic) {
            diagnostics.push(diagnostic);
        }
    }
    ty
}

/// Run the type pass over one parsed file: unknown-type annotations, return-value
/// placement, the expression/statement type checks, and missing-return analysis.
/// Shared by library files and test scripts.
pub(crate) fn check_file_types(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    backing_invalidations: Option<&mut crate::backing_validity::PendingBackingInvalidations>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let has_parse_errors = parsed.has_errors();
    let FilePrelude {
        aliases,
        module_constants,
    } = checked_file_prelude(program, file, parsed, diagnostics);
    check_rejected_surface(program, file, parsed, diagnostics);
    let annotation_context = TypeAnnotationContext {
        program,
        aliases: &aliases,
        file,
    };
    let mut backing_invalidations = backing_invalidations;
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
                    for key in &param.keys {
                        check_type_annotation(
                            &key.ty,
                            function.span,
                            &annotation_context,
                            diagnostics,
                        );
                    }
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
                check_exposed_private_enums(function, &annotation_context, diagnostics);
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
                        resource.name.as_str(),
                        &resource.members,
                        &annotation_context,
                        backing_invalidations.as_deref_mut(),
                        diagnostics,
                    );
                    check_saved_named_field_annotations(
                        resource.name.as_str(),
                        &resource.members,
                        &annotation_context,
                        backing_invalidations.as_deref_mut(),
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

fn check_saved_named_field_annotations(
    resource_name: &str,
    members: &[marrow_syntax::ResourceMember],
    context: &TypeAnnotationContext<'_>,
    mut backing_invalidations: Option<&mut crate::backing_validity::PendingBackingInvalidations>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    marrow_schema::walk_saved_named_member_fields(members, |field, name| {
        check_saved_named_field_annotation(
            resource_name,
            field,
            name,
            context,
            backing_invalidations.as_deref_mut(),
            diagnostics,
        );
    });
}

fn check_saved_named_field_annotation(
    resource_name: &str,
    field: &marrow_syntax::FieldDecl,
    name: &str,
    context: &TypeAnnotationContext<'_>,
    backing_invalidations: Option<&mut crate::backing_validity::PendingBackingInvalidations>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let schema_type = Type::resolve(&field.ty);
    match resolve_enum_annotation(&field.ty, context.program, context.aliases, context.file) {
        EnumAnnotationResolution::Visible(_) => {}
        EnumAnnotationResolution::Private(private) => {
            if let Some(backing_invalidations) = backing_invalidations {
                backing_invalidations.record_resource_error(context.file, resource_name);
            }
            diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_PRIVATE_ENUM,
                    context.file,
                    field.ty.span,
                    format!(
                        "enum `{private}` is private to its module; mark it `pub` to use it from another module"
                    ),
                )
                .with_payload(DiagnosticPayload::PrivateEnum(private)),
            );
        }
        EnumAnnotationResolution::AmbiguousBareForeign(name) => {
            if let Some(backing_invalidations) = backing_invalidations {
                backing_invalidations.record_resource_error(context.file, resource_name);
            }
            diagnostics.push(ambiguous_enum_annotation_diagnostic(
                context.file,
                field.ty.span,
                name,
                schema_type,
            ));
        }
        EnumAnnotationResolution::MissingOrNonEnum => {
            if let Some(backing_invalidations) = backing_invalidations {
                backing_invalidations.record_resource_error(context.file, resource_name);
            }
            push_schema_error(
                context.file,
                diagnostics,
                marrow_schema::non_enum_named_field_error(field, name),
            );
        }
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
    match resolve_enum_annotation(ty, context.program, context.aliases, context.file) {
        EnumAnnotationResolution::Private(private) => {
            push_enum_annotation_diagnostic(
                diagnostics,
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
        EnumAnnotationResolution::AmbiguousBareForeign(name) => {
            push_enum_annotation_diagnostic(
                diagnostics,
                ambiguous_enum_annotation_diagnostic(context.file, span, name, schema_type),
            );
            return;
        }
        EnumAnnotationResolution::Visible(_) | EnumAnnotationResolution::MissingOrNonEnum => {}
    }
    let resolved_type = resolve_type(ty, context.program, context.aliases, context.file);
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

/// Warn when a `pub fn` names a non-`pub` enum from its own module in a parameter
/// or return type: the enum's values escape through a public API even though other
/// modules cannot name the type. Private functions encapsulate the enum, and a
/// foreign private enum is already a hard `check.private_enum` error.
fn check_exposed_private_enums(
    function: &marrow_syntax::FunctionDecl,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !function.public {
        return;
    }
    let signature_types = function
        .params
        .iter()
        .map(|param| &param.ty)
        .chain(function.return_type.as_ref());
    for ty in signature_types {
        let Some(enum_name) =
            same_module_private_enum(ty, context.program, context.aliases, context.file)
        else {
            continue;
        };
        diagnostics.push(
            CheckDiagnostic::warning(
                CHECK_EXPOSED_PRIVATE_ENUM,
                context.file,
                function.span,
                format!(
                    "public function `{}` exposes private enum `{enum_name}`; mark the enum `pub` to make it nameable",
                    function.name
                ),
            )
            .with_payload(DiagnosticPayload::ExposedPrivateEnum {
                enum_name,
                function: function.name.clone(),
            }),
        );
    }
}

fn push_enum_annotation_diagnostic(
    diagnostics: &mut Vec<CheckDiagnostic>,
    diagnostic: CheckDiagnostic,
) {
    if has_duplicate_error(diagnostics, &diagnostic) {
        return;
    }
    diagnostics.push(diagnostic);
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
    resource_name: &str,
    members: &[marrow_syntax::ResourceMember],
    context: &TypeAnnotationContext<'_>,
    mut backing_invalidations: Option<&mut crate::backing_validity::PendingBackingInvalidations>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for member in members {
        match member {
            marrow_syntax::ResourceMember::Field(field) => {
                if let Some(name) = unknown_identity_type_ref(&field.ty, context) {
                    if let Some(backing_invalidations) = backing_invalidations.as_deref_mut() {
                        backing_invalidations.record_invalid_resource(context.file, resource_name);
                    }
                    push_unknown_identity_type_diagnostic(
                        name,
                        Type::resolve(&field.ty),
                        field.ty.span,
                        context,
                        diagnostics,
                    );
                }
            }
            marrow_syntax::ResourceMember::Group(group) => {
                check_resource_identity_annotations(
                    resource_name,
                    &group.members,
                    context,
                    backing_invalidations.as_deref_mut(),
                    diagnostics,
                );
            }
        }
    }
}

fn push_unknown_identity_type_diagnostic(
    name: String,
    resolved: Type,
    span: SourceSpan,
    context: &TypeAnnotationContext<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(
            CHECK_UNKNOWN_TYPE,
            context.file,
            span,
            format!("unknown type `{name}`"),
        )
        .with_payload(DiagnosticPayload::UnknownType(resolved)),
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
    crate::annotation_refs::walk_block_type_refs(block, &mut |ty| {
        check_type_annotation(ty, ty.span, context, diagnostics);
    });
}
