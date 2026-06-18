//! Project-aware normalization for explicit keyed fields whose entry type is a resource.

use std::path::{Path, PathBuf};

use marrow_syntax::{FieldDecl, ResourceDecl};

use crate::enums::{annotation_type_known, private_enum_type_reference};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};
use crate::{
    CHECK_PRIVATE_ENUM, CHECK_RECURSIVE_KEYED_ENTRY, CHECK_UNKNOWN_TYPE, CheckDiagnostic,
    CheckedModule, CheckedProgram, DiagnosticPayload, MarrowType, build_alias_map, expand_alias,
    has_duplicate_error, resource_type_name, split_type_path,
};

pub(crate) fn normalize_resource_layers(
    program: &mut CheckedProgram,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    backing_invalidations: Option<&mut crate::backing_validity::PendingBackingInvalidations>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let plan = plan_resource_layers(program, parsed_files, backing_invalidations, diagnostics);
    for item in plan {
        let Some(resource) = program
            .modules
            .get_mut(item.module_index)
            .and_then(|module| module.resources.get_mut(item.resource_index))
        else {
            continue;
        };
        resource.members = item.members;
    }
}

struct ResourceLayerNormalization {
    module_index: usize,
    resource_index: usize,
    members: Vec<marrow_schema::Node>,
}

fn plan_resource_layers(
    program: &CheckedProgram,
    parsed_files: &[(&marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    backing_invalidations: Option<&mut crate::backing_validity::PendingBackingInvalidations>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Vec<ResourceLayerNormalization> {
    let mut plan = Vec::new();
    let mut normalizer = Normalizer {
        resolver: program,
        parsed_files,
        backing_invalidations,
        diagnostics,
    };
    for (module_index, module) in program.modules.iter().enumerate() {
        let Some((_, parsed)) = parsed_files
            .iter()
            .find(|(file, _)| file.path == module.source_file)
        else {
            continue;
        };
        for (resource_index, resource) in module.resources.iter().enumerate() {
            let Some(decl) = parsed_resource_decl(parsed, &resource.name) else {
                continue;
            };
            let mut members = resource.members.clone();
            let mut stack = vec![(module.name.clone(), resource.name.clone())];
            normalizer.normalize_members(
                MemberScope {
                    file: &module.source_file,
                    module_name: &module.name,
                    owner_file: &module.source_file,
                    owner_resource: &resource.name,
                },
                &decl.members,
                &mut members,
                &mut stack,
            );
            plan.push(ResourceLayerNormalization {
                module_index,
                resource_index,
                members,
            });
        }
    }
    plan
}

struct Normalizer<'a, 'd> {
    resolver: &'a CheckedProgram,
    parsed_files: &'a [(&'a marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    backing_invalidations: Option<&'d mut crate::backing_validity::PendingBackingInvalidations>,
    diagnostics: &'d mut Vec<CheckDiagnostic>,
}

#[derive(Clone, Copy)]
struct MemberScope<'a> {
    file: &'a Path,
    module_name: &'a str,
    owner_file: &'a Path,
    owner_resource: &'a str,
}

impl Normalizer<'_, '_> {
    fn normalize_members(
        &mut self,
        scope: MemberScope<'_>,
        syntax_members: &[marrow_syntax::ResourceMember],
        nodes: &mut [marrow_schema::Node],
        stack: &mut Vec<(String, String)>,
    ) {
        for (syntax_member, node) in syntax_members.iter().zip(nodes) {
            match syntax_member {
                marrow_syntax::ResourceMember::Field(field) if !field.keys.is_empty() => {
                    self.normalize_keyed_field(scope, field, node, stack);
                }
                marrow_syntax::ResourceMember::Group(group) => {
                    self.normalize_members(scope, &group.members, &mut node.members, stack);
                }
                _ => {}
            }
        }
    }

    fn normalize_keyed_field(
        &mut self,
        scope: MemberScope<'_>,
        field: &FieldDecl,
        node: &mut marrow_schema::Node,
        stack: &mut Vec<(String, String)>,
    ) {
        match self.resolve_keyed_field_type(scope, field) {
            KeyedFieldType::Resource(target) => {
                if let Some(decl) = &target.decl {
                    self.validate_entry_resource(scope, &target, decl);
                }
                let stack_key = (target.module_name.clone(), target.resource_name.clone());
                if stack.contains(&stack_key) {
                    self.record_owner_resource_invalid(scope);
                    self.push_recursive_keyed_entry(scope.file, field, &target.resource_name);
                    return;
                }
                let mut members = target.members;
                if let Some(decl) = &target.decl {
                    stack.push(stack_key);
                    self.normalize_members(
                        MemberScope {
                            file: &target.source_file,
                            module_name: &target.module_name,
                            owner_file: scope.owner_file,
                            owner_resource: scope.owner_resource,
                        },
                        &decl.members,
                        &mut members,
                        stack,
                    );
                    stack.pop();
                }
                node.kind = marrow_schema::NodeKind::Group;
                node.entry_type = Some(marrow_schema::Type::Named(resource_type_name(
                    &target.module_name,
                    &target.resource_name,
                )));
                node.members = members;
            }
            KeyedFieldType::PrivateEnum(name) => {
                self.record_owner_resource_invalid(scope);
                self.push_private_enum(scope.file, field.span, name);
            }
            KeyedFieldType::Unknown(ty) => {
                self.record_owner_resource_invalid(scope);
                self.push_unknown_type(scope.file, field, ty);
            }
            KeyedFieldType::SavedLeaf => {
                self.check_keyed_leaf_named_type(scope, field);
            }
        }
    }

    /// Apply the saved named-type rule to a keyed entry's value type. Schema
    /// validation skips keyed fields because the keyed entry is checked here,
    /// where the project-aware enum resolver is available, so this routes the
    /// field's value through the schema owner as an unkeyed saved field to keep
    /// one owner for the rule and its diagnostic.
    fn check_keyed_leaf_named_type(&mut self, scope: MemberScope<'_>, field: &FieldDecl) {
        let aliases = module_aliases(self.resolver, scope.module_name);
        let leaf = marrow_syntax::ResourceMember::Field(FieldDecl {
            keys: Vec::new(),
            ..field.clone()
        });
        for error in marrow_schema::check_saved_named_member_fields_with(&[leaf], |name| {
            matches!(
                crate::enums::resolve_enum_type(
                    &marrow_schema::Type::Named(name.to_string()),
                    self.resolver,
                    &aliases,
                    scope.file,
                ),
                Some(MarrowType::Enum { .. })
            )
        }) {
            self.record_owner_resource_invalid(scope);
            self.push_schema_error(scope.file, error);
        }
    }

    fn resolve_keyed_field_type(
        &self,
        scope: MemberScope<'_>,
        field: &FieldDecl,
    ) -> KeyedFieldType {
        let schema_type = marrow_schema::Type::resolve(&field.ty);
        let aliases = module_aliases(self.resolver, scope.module_name);
        if let marrow_schema::Type::Named(name) = &schema_type {
            let segments = expand_alias(&split_type_path(name), &aliases);
            if let Resolution::Found(Def {
                module,
                item: DefItem::Resource(resource),
                ..
            }) = resolve(
                self.resolver,
                scope.module_name,
                &segments,
                ResolvableKind::Resource,
            ) {
                return KeyedFieldType::Resource(Box::new(ResourceTarget {
                    module_name: module.name.clone(),
                    source_file: module.source_file.clone(),
                    imports: module.imports.clone(),
                    enum_names: module
                        .enums
                        .iter()
                        .map(|enum_| enum_.name.clone())
                        .collect(),
                    resource_name: resource.name.clone(),
                    members: resource.members.clone(),
                    decl: resource_decl(self.parsed_files, module, &resource.name).cloned(),
                }));
            }
        }

        if let Some(private) =
            private_enum_type_reference(&field.ty, self.resolver, &aliases, scope.file)
        {
            return KeyedFieldType::PrivateEnum(private);
        }
        let resolved = crate::enums::resolve_type(&field.ty, self.resolver, &aliases, scope.file);
        if !annotation_type_known(&schema_type, &resolved) {
            return KeyedFieldType::Unknown(schema_type);
        }
        KeyedFieldType::SavedLeaf
    }

    fn validate_entry_resource(
        &mut self,
        scope: MemberScope<'_>,
        target: &ResourceTarget,
        decl: &ResourceDecl,
    ) {
        for error in marrow_schema::check_saved_member_rules(&decl.members) {
            self.record_owner_resource_invalid(scope);
            self.push_schema_error(&target.source_file, error);
        }

        let aliases = build_alias_map(&target.imports);
        for error in marrow_schema::check_saved_named_member_fields_with(&decl.members, |name| {
            if !name.contains("::") {
                return target.enum_names.iter().any(|enum_name| enum_name == name);
            }
            matches!(
                crate::enums::resolve_enum_type(
                    &marrow_schema::Type::Named(name.to_string()),
                    self.resolver,
                    &aliases,
                    &target.source_file,
                ),
                Some(MarrowType::Enum { .. })
            )
        }) {
            self.record_owner_resource_invalid(scope);
            self.push_schema_error(&target.source_file, error);
        }
    }

    fn push_schema_error(&mut self, file: &Path, error: marrow_schema::SchemaError) {
        let marrow_schema::SchemaError {
            kind,
            code,
            message,
            span,
        } = error;
        self.push_diagnostic(
            CheckDiagnostic::error(code, file, span, message)
                .with_payload(DiagnosticPayload::Schema(kind)),
        );
    }

    fn record_owner_resource_invalid(&mut self, scope: MemberScope<'_>) {
        if let Some(backing_invalidations) = self.backing_invalidations.as_deref_mut() {
            backing_invalidations.record_invalid_resource(scope.owner_file, scope.owner_resource);
        }
    }

    fn push_recursive_keyed_entry(&mut self, file: &Path, field: &FieldDecl, resource_name: &str) {
        self.push_diagnostic(CheckDiagnostic::error(
            CHECK_RECURSIVE_KEYED_ENTRY,
            file,
            field.span,
            format!(
                "typed keyed-entry layer `{}` recursively names resource `{}`",
                field.name, resource_name
            ),
        ));
    }

    fn push_private_enum(&mut self, file: &Path, span: marrow_syntax::SourceSpan, name: String) {
        self.push_diagnostic(
            CheckDiagnostic::error(
                CHECK_PRIVATE_ENUM,
                file,
                span,
                format!("enum `{name}` is private to its module; mark it `pub` to use it from another module"),
            )
            .with_payload(DiagnosticPayload::PrivateEnum(name)),
        );
    }

    fn push_unknown_type(&mut self, file: &Path, field: &FieldDecl, ty: marrow_schema::Type) {
        self.push_diagnostic(
            CheckDiagnostic::error(
                CHECK_UNKNOWN_TYPE,
                file,
                field.span,
                format!("unknown type `{}`", field.ty.text.trim()),
            )
            .with_payload(DiagnosticPayload::UnknownType(ty)),
        );
    }

    fn push_diagnostic(&mut self, diagnostic: CheckDiagnostic) {
        if has_duplicate_error(self.diagnostics, &diagnostic) {
            return;
        }
        self.diagnostics.push(diagnostic);
    }
}

enum KeyedFieldType {
    Resource(Box<ResourceTarget>),
    PrivateEnum(String),
    Unknown(marrow_schema::Type),
    /// A value type that is not a resource layer: its saved named-type rule is
    /// checked by the schema owner, which is a no-op for scalar and identity types.
    SavedLeaf,
}

struct ResourceTarget {
    module_name: String,
    source_file: PathBuf,
    imports: Vec<String>,
    enum_names: Vec<String>,
    resource_name: String,
    members: Vec<marrow_schema::Node>,
    decl: Option<ResourceDecl>,
}

fn parsed_resource_decl<'a>(
    parsed: &'a marrow_syntax::ParsedSource,
    resource_name: &str,
) -> Option<&'a ResourceDecl> {
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            marrow_syntax::Declaration::Resource(decl) if decl.name == resource_name => Some(decl),
            _ => None,
        })
}

fn resource_decl<'a>(
    parsed_files: &'a [(&'a marrow_project::ModuleFile, marrow_syntax::ParsedSource)],
    module: &CheckedModule,
    resource_name: &str,
) -> Option<&'a ResourceDecl> {
    let (_, parsed) = parsed_files
        .iter()
        .find(|(file, _)| file.path == module.source_file)?;
    parsed_resource_decl(parsed, resource_name)
}

fn module_aliases(
    program: &CheckedProgram,
    module_name: &str,
) -> std::collections::HashMap<String, Vec<String>> {
    program
        .modules
        .iter()
        .find(|module| module.name == module_name)
        .map(|module| build_alias_map(&module.imports))
        .unwrap_or_default()
}
