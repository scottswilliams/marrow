//! Source-native evolution intent: the durable rename/retire/default/transform
//! declarations an `evolve` block states about catalog-addressable entities.
//!
//! A bare source diff never implies identity preservation or destructive intent,
//! so this module turns each `evolve` step into an explicit intent the catalog
//! binding consults. Target spellings are mapped to the module-qualified catalog
//! path they name, in the same form the catalog path helpers produce, rather than
//! through a second semantic classifier; the catalog binding resolves each path to
//! the accepted or source entry that carries its kind and stable identity.

use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::{
    Block, Declaration, EvolveStep, Expression, ParsedSource, Severity, SourceSpan,
};

use crate::catalog::{SourceCatalogEntry, source_catalog_entries};
use crate::checks::{FilePrelude, check_block_types, file_prelude};
use crate::infer::infer_type;
use crate::program::TypeNames;
use crate::typerules::{marrow_type_name, type_compatible};
use crate::{
    CHECK_EVOLVE_TARGET, CHECK_EVOLVE_TYPE, CheckDiagnostic, CheckedModule, CheckedProgram,
    MarrowType,
};

/// One declared rename: the entity is now spelled `to_path` and was formerly
/// `from_path` (both module-qualified catalog paths), reported at `span` if either
/// side does not resolve.
#[derive(Debug, Clone)]
pub(crate) struct RenameIntent {
    pub(crate) from_path: String,
    pub(crate) to_path: String,
    pub(crate) file: std::path::PathBuf,
    pub(crate) span: SourceSpan,
}

/// One declared retirement: the module-qualified catalog path of an entity to
/// remove destructively, reported at `span` if it does not resolve.
#[derive(Debug, Clone)]
pub(crate) struct RetireIntent {
    pub(crate) path: String,
    pub(crate) file: std::path::PathBuf,
    pub(crate) span: SourceSpan,
}

/// The rename and retire intents an evolve block declares. The catalog binding
/// consults these to carry stable identity forward across a rename and to mark a
/// retired entity removed; a path that matches neither is a target diagnostic.
#[derive(Debug, Default, Clone)]
pub(crate) struct EvolveIntents {
    pub(crate) renames: Vec<RenameIntent>,
    pub(crate) retires: Vec<RetireIntent>,
}

/// Extract the rename and retire intents every `evolve` block declares, mapping
/// each target spelling to its module-qualified catalog path. Pure surface
/// extraction: the catalog binding validates the paths against the accepted and
/// source catalogs. A rename or retire target that is not a catalog-addressable
/// reference shape is reported here, since no catalog lookup can recover it.
pub(crate) fn collect_evolve_intents<'a, I>(
    parsed_files: I,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> EvolveIntents
where
    I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
{
    let mut intents = EvolveIntents::default();
    for (file, parsed) in parsed_files {
        let module = module_name(parsed);
        for declaration in &parsed.file.declarations {
            let Declaration::Evolve(evolve) = declaration else {
                continue;
            };
            for step in &evolve.steps {
                match step {
                    EvolveStep::Rename { from, to, span } => {
                        match (target_path(module, from), target_path(module, to)) {
                            (Some(from_path), Some(to_path)) => {
                                intents.renames.push(RenameIntent {
                                    from_path,
                                    to_path,
                                    file: file.to_path_buf(),
                                    span: *span,
                                });
                            }
                            _ => report_target(file, *span, diagnostics),
                        }
                    }
                    EvolveStep::Retire { target, span } => match target_path(module, target) {
                        Some(path) => intents.retires.push(RetireIntent {
                            path,
                            file: file.to_path_buf(),
                            span: *span,
                        }),
                        None => report_target(file, *span, diagnostics),
                    },
                    // Default and transform do not move catalog identity; they are
                    // type-checked against current source separately.
                    EvolveStep::Default { .. } | EvolveStep::Transform { .. } => {}
                }
            }
        }
    }
    intents
}

/// Type-check the `default` and `transform` steps of every `evolve` block against
/// current source: a default value must match its target member's type, and a
/// transform body must satisfy the structural body rules. Targets that name no
/// current source entity are reported.
pub(crate) fn check_evolve_types<'a, I>(
    program: &CheckedProgram,
    parsed_files: I,
    diagnostics: &mut Vec<CheckDiagnostic>,
) where
    I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
{
    let source_entries = source_catalog_entries(program);
    for (file, parsed) in parsed_files {
        let module = module_name(parsed);
        if !parsed
            .file
            .declarations
            .iter()
            .any(|declaration| matches!(declaration, Declaration::Evolve(_)))
        {
            continue;
        }
        let prelude = file_prelude(program, file, parsed);
        for declaration in &parsed.file.declarations {
            let Declaration::Evolve(evolve) = declaration else {
                continue;
            };
            let context = TypeContext {
                program,
                file,
                module,
                source_entries: &source_entries,
                prelude: &prelude,
            };
            for step in &evolve.steps {
                match step {
                    EvolveStep::Default {
                        target,
                        value,
                        span,
                    } => context.check_default(target, value, *span, diagnostics),
                    EvolveStep::Transform { target, body, span } => {
                        context.check_transform(target, body, *span, diagnostics)
                    }
                    EvolveStep::Rename { .. } | EvolveStep::Retire { .. } => {}
                }
            }
        }
    }
}

/// The per-file context default and transform steps resolve against: the bound
/// program, the evolve block's file and module, and the catalog entries current
/// source declares.
struct TypeContext<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    module: &'a str,
    source_entries: &'a [SourceCatalogEntry],
    prelude: &'a FilePrelude,
}

impl TypeContext<'_> {
    fn check_default(
        &self,
        target: &Expression,
        value: &Expression,
        span: SourceSpan,
        diagnostics: &mut Vec<CheckDiagnostic>,
    ) {
        if !self.resolves_in_source(target) {
            report_target(self.file, target_span(target, span), diagnostics);
            return;
        }
        // Only a populated member target has a leaf type to check the value
        // against; a default on a store root, index, or enum has no leaf type, and
        // its applicability is the apply phase's concern.
        let Some(member_type) = self.member_type(target) else {
            return;
        };
        let actual = infer_type(
            self.program,
            value,
            &[HashMap::new()],
            &HashMap::new(),
            self.file,
            diagnostics,
        );
        if type_compatible(&member_type, &actual) == Some(false) {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_EVOLVE_TYPE,
                severity: Severity::Error,
                file: self.file.to_path_buf(),
                message: format!(
                    "default value is `{}` but `{}` is `{}`",
                    marrow_type_name(&actual),
                    marrow_syntax::format_expression(target),
                    marrow_type_name(&member_type)
                ),
                span: value.span(),
            });
        }
    }

    fn check_transform(
        &self,
        target: &Expression,
        body: &Block,
        span: SourceSpan,
        diagnostics: &mut Vec<CheckDiagnostic>,
    ) {
        if !self.resolves_in_source(target) {
            report_target(self.file, target_span(target, span), diagnostics);
            return;
        }
        // A transform body is a function body in everything but the typed old/new
        // views discharge supplies in Phase C. It is held to the same structural
        // rules and run through the same name-resolution and type pass, so an
        // undefined identifier or unknown call is caught at check time rather than
        // surviving as unchecked free text. Only the old/new mapping is deferred.
        crate::rules::check_transform_body(self.file, body, diagnostics);
        let mut scope = vec![self.prelude.module_constants.clone()];
        check_block_types(
            self.program,
            self.file,
            &MarrowType::Unknown,
            body,
            &mut scope,
            &self.prelude.aliases,
            diagnostics,
        );
    }

    fn resolves_in_source(&self, target: &Expression) -> bool {
        target_path(self.module, target)
            .is_some_and(|path| self.source_entries.iter().any(|entry| entry.path == path))
    }

    /// The declared type of a resource-member target, if it names one in a resource
    /// still declared in this block's module and file.
    fn member_type(&self, target: &Expression) -> Option<MarrowType> {
        let (resource_name, member_chain) = member_chain(target)?;
        let module = self
            .program
            .modules
            .iter()
            .find(|module| module.name == self.module && module.source_file == self.file)?;
        let resource = module
            .resources
            .iter()
            .find(|resource| resource.name == resource_name)?;
        let chain: Vec<&str> = member_chain.iter().map(String::as_str).collect();
        let field = resource.field_type(&chain)?;
        let names = TypeNames {
            module: self.module,
            enums: &enum_names(module),
        };
        Some(MarrowType::from_resolved(field.clone(), names))
    }
}

fn module_name(parsed: &ParsedSource) -> &str {
    parsed
        .file
        .module
        .as_ref()
        .map(|module| module.name.as_str())
        .unwrap_or_default()
}

fn enum_names(module: &CheckedModule) -> Vec<String> {
    module
        .enums
        .iter()
        .map(|enum_schema| enum_schema.name.clone())
        .collect()
}

/// The `(resource, member chain)` a target names, when it is a dotted member path
/// off a bare resource name (`Book.title`, `Book.name.first`).
fn member_chain(target: &Expression) -> Option<(String, Vec<String>)> {
    let segments = target_segments(target)?;
    let (head, rest) = segments.split_first()?;
    if rest.is_empty() || head.starts_with('^') {
        return None;
    }
    Some((head.clone(), rest.to_vec()))
}

/// The module-qualified catalog path a target spelling names, in the same form the
/// catalog path helpers produce, or `None` for a non-reference shape.
fn target_path(module: &str, target: &Expression) -> Option<String> {
    let segments = target_segments(target)?;
    let joined = segments.join("::");
    Some(if module.is_empty() {
        joined
    } else {
        format!("{module}::{joined}")
    })
}

/// Decompose an evolve target into its catalog path segments. A saved root carries
/// its leading `^`; a `::` name path, a dotted member path, and a saved index
/// lookup map to their segment lists. Other expression shapes are not catalog
/// references.
fn target_segments(target: &Expression) -> Option<Vec<String>> {
    match target {
        Expression::SavedRoot { name, .. } => Some(vec![format!("^{name}")]),
        Expression::Name { segments, .. } => Some(segments.clone()),
        Expression::Field {
            base, name, quoted, ..
        } if !quoted => {
            let mut segments = target_segments(base)?;
            segments.push(name.clone());
            Some(segments)
        }
        _ => None,
    }
}

/// A target's own span when the parser recorded one, else the step span.
fn target_span(target: &Expression, step: SourceSpan) -> SourceSpan {
    let span = target.span();
    if span == SourceSpan::default() {
        step
    } else {
        span
    }
}

fn report_target(file: &Path, span: SourceSpan, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_EVOLVE_TARGET,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: "evolve target does not name a catalog-addressable entity \
            (a resource member, saved root, store index, enum, or enum member)"
            .to_string(),
        span,
    });
}

#[cfg(test)]
mod tests {
    use marrow_schema::{EnumMemberSchema, EnumSchema};
    use marrow_syntax::{Expression, SourceSpan};

    use super::target_path;
    use crate::catalog::{enum_member_path, resource_member_path, store_index_path, store_path};

    fn name(segments: &[&str]) -> Expression {
        Expression::Name {
            segments: segments.iter().map(|s| s.to_string()).collect(),
            span: SourceSpan::default(),
        }
    }

    fn field(base: Expression, name: &str) -> Expression {
        Expression::Field {
            base: Box::new(base),
            name: name.to_string(),
            quoted: false,
            span: SourceSpan::default(),
        }
    }

    fn saved_root(name: &str) -> Expression {
        Expression::SavedRoot {
            name: name.to_string(),
            span: SourceSpan::default(),
        }
    }

    // An evolve target's path string must match the catalog path helpers byte for
    // byte: the two formats are produced independently and compared by string
    // equality, so a format change in either side must break loudly here.
    #[test]
    fn target_path_matches_resource_member_helper() {
        let target = field(name(&["Book"]), "subtitle");
        assert_eq!(
            target_path("books", &target),
            Some(resource_member_path(
                "books",
                "Book",
                &["subtitle".to_string()]
            ))
        );
    }

    #[test]
    fn target_path_matches_nested_resource_member_helper() {
        let target = field(field(name(&["Book"]), "name"), "first");
        assert_eq!(
            target_path("books", &target),
            Some(resource_member_path(
                "books",
                "Book",
                &["name".to_string(), "first".to_string()]
            ))
        );
    }

    #[test]
    fn target_path_matches_store_root_helper() {
        let target = saved_root("books");
        assert_eq!(
            target_path("books", &target),
            Some(store_path("books", "books"))
        );
    }

    #[test]
    fn target_path_matches_store_index_helper() {
        let target = field(saved_root("books"), "byTitle");
        assert_eq!(
            target_path("books", &target),
            Some(store_index_path("books", "books", "byTitle"))
        );
    }

    #[test]
    fn target_path_matches_enum_member_helper() {
        let target = field(name(&["Status"]), "active");
        let schema = EnumSchema {
            name: "Status".to_string(),
            docs: Vec::new(),
            members: vec![EnumMemberSchema {
                name: "active".to_string(),
                docs: Vec::new(),
                parent: None,
                category: false,
            }],
        };
        assert_eq!(
            target_path("books", &target),
            Some(enum_member_path("books", "Status", 0, &schema))
        );
    }
}
