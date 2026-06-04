//! Source-native evolution intent: the durable rename/retire/default/transform
//! declarations an `evolve` block states about catalog-addressable entities.
//!
//! A bare source diff never implies identity preservation or destructive intent,
//! so this module turns each `evolve` step into an explicit intent the catalog
//! binding consults. Target spellings are mapped to the module-qualified catalog
//! path they name, in the same form the catalog path helpers produce, rather than
//! through a second semantic classifier; the catalog binding resolves each path to
//! the accepted or source entry that carries its kind and stable identity.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_syntax::{
    Argument, Block, Declaration, EvolveStep, Expression, InterpolationPart, ParsedSource,
    Severity, SourceSpan, Statement,
};

use crate::catalog::{SourceCatalogEntry, source_catalog_entries};
use crate::checks::{FilePrelude, check_block_types, file_prelude};
use crate::infer::infer_type;
use crate::program::TypeNames;
use crate::typerules::{marrow_type_name, type_compatible};
use crate::{
    CHECK_EVOLVE_TARGET, CHECK_EVOLVE_TRANSFORM, CHECK_EVOLVE_TYPE, CheckDiagnostic, CheckedBody,
    CheckedModule, CheckedProgram, MarrowType,
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

/// One declared default: the module-qualified catalog path of the member an
/// `evolve default` step targets and the constant value expression to backfill.
/// Discharge evaluates the value to a typed fill and classifies the newly-required
/// member as defaultable; the source digest binds the normalized value so a changed
/// default drifts the witness.
#[derive(Debug, Clone)]
pub(crate) struct DefaultIntent {
    pub(crate) path: String,
    pub(crate) value: Expression,
}

/// One declared transform: the module-qualified catalog path an `evolve transform`
/// step reshapes (the target), the catalog paths of the members its body reads via
/// `old.<member>`, and the body itself. The catalog binding resolves the target and
/// read paths to stable ids; discharge classifies the member as an applyable transform
/// once the body passed the checker.
#[derive(Debug, Clone)]
pub(crate) struct TransformIntent {
    pub(crate) path: String,
    pub(crate) read_paths: Vec<String>,
    pub(crate) file: std::path::PathBuf,
    pub(crate) body_span: SourceSpan,
}

/// The rename, retire, default, and transform intents an evolve block declares. The
/// catalog binding consults the renames and retires to carry stable identity forward
/// and to mark a retired entity removed; the default and transform intents flow to
/// discharge. A rename or retire path that matches neither side is a target
/// diagnostic.
#[derive(Debug, Default, Clone)]
pub(crate) struct EvolveIntents {
    pub(crate) renames: Vec<RenameIntent>,
    pub(crate) retires: Vec<RetireIntent>,
    pub(crate) defaults: Vec<DefaultIntent>,
    pub(crate) transforms: Vec<TransformIntent>,
}

/// Extract the rename, retire, default, and transform intents every `evolve` block
/// declares, mapping each target spelling to its module-qualified catalog path. Pure
/// surface extraction: the catalog binding validates the rename/retire paths against
/// the accepted and source catalogs. A rename or retire target that is not a
/// catalog-addressable reference shape is reported here, since no catalog lookup can
/// recover it.
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
                    // A default does not move catalog identity, but discharge needs
                    // its target and value to evaluate the constant fill; record both
                    // here and let the catalog binding resolve the path to a stable
                    // id. A non-reference target is reported by the type pass.
                    EvolveStep::Default { target, value, .. } => {
                        if let Some(path) = target_path(module, target) {
                            intents.defaults.push(DefaultIntent {
                                path,
                                value: value.clone(),
                            });
                        }
                    }
                    // A transform recomputes its target from the members its body reads
                    // via `old.<member>`. The target and read paths flow to the catalog
                    // binding for stable-id resolution and to discharge; the body flows
                    // through for apply to execute. It carries no catalog identity move.
                    EvolveStep::Transform { target, body, .. } => {
                        if let Some(path) = target_path(module, target) {
                            let read_paths = transform_read_paths(&path, body);
                            intents.transforms.push(TransformIntent {
                                path,
                                read_paths,
                                file: file.to_path_buf(),
                                body_span: body.span,
                            });
                        }
                    }
                }
            }
        }
    }
    intents
}

/// Type-check the `default` and `transform` steps of every `evolve` block against
/// current source: a default value must match its target member's type, and a
/// transform body must satisfy the structural body rules, type its result as its
/// target, and obey the read restrictions. Targets that name no current source entity
/// are reported. The catalog path of every member a default or transform rewrites is
/// collected first so a transform can reject a read of a member the same evolve surface
/// changes. The purity check runs after lowering, in [`check_transform_effects`], where
/// the body's effects are available.
pub(crate) fn check_evolve_types<'a, I>(
    program: &CheckedProgram,
    parsed_files: I,
    diagnostics: &mut Vec<CheckDiagnostic>,
) where
    I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
{
    let source_entries = source_catalog_entries(program);
    let parsed_files: Vec<(&Path, &ParsedSource)> = parsed_files.into_iter().collect();
    let rewritten_targets = rewritten_target_paths(&parsed_files);
    for (file, parsed) in &parsed_files {
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
                rewritten_targets: &rewritten_targets,
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

/// Enforce that every `evolve transform` body is pure and effect-free. This runs after
/// the bodies are lowered, where the canonical effect classifier sees each body's
/// direct saved writes, host effects, and transactions, and a call-target walk catches
/// a call into a user function (whose own effects this narrow model does not propagate
/// into a transform). A transform must be a pure, total function of `old`, so any of
/// these is a fail-closed check error.
pub(crate) fn check_transform_effects(
    program: &CheckedProgram,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for transform in &program.catalog.evolve_transforms {
        let Some(body) = &transform.runtime_body else {
            continue;
        };
        let reason = impurity_reason(program, body);
        if let Some(reason) = reason {
            diagnostics.push(CheckDiagnostic {
                code: CHECK_EVOLVE_TRANSFORM,
                severity: Severity::Error,
                file: transform.file.clone(),
                message: format!("an evolve transform body must be pure: {reason}"),
                span: transform.body_span,
            });
        }
    }
}

/// Why a transform body is impure, or `None` when it is pure. A saved read, a saved
/// write, a host effect, a transaction, and a read or write of a future ephemeral root
/// are read from the canonical direct-effect facts; a call into a user function is its
/// own reason, since this model evaluates a transform as a self-contained pure
/// expression rather than propagating callee effects.
///
/// Reading saved data is the soundness-critical case: a transform body is a per-record
/// function of `old` only, so a direct `^root(key).field` read would let one record's
/// stored value flow into every record's recomputed cell, bypassing both the `old`
/// binding and the decodability proof discharge builds for each read member.
fn impurity_reason(program: &CheckedProgram, body: &CheckedBody) -> Option<&'static str> {
    let effects = crate::presence::direct_effects_for_block(&program.facts, body);
    if !effects.saved_reads.is_empty()
        || !effects.future_ephemeral_roots.reads.is_empty()
        || !effects.future_ephemeral_roots.writes.is_empty()
    {
        return Some("it reads saved data; a transform body may only read `old`");
    }
    if !effects.saved_writes.is_empty() {
        return Some("it writes saved data");
    }
    if !effects.host_calls.is_empty() {
        return Some("it performs a host effect");
    }
    if effects.transactions {
        return Some("it opens a transaction");
    }
    block_calls_function(body).then_some("it calls a function; inline the computation over `old`")
}

/// Whether a lowered block calls any user-defined function. A pure transform computes
/// its target from `old` with operators and pure builtins; a call into a user function
/// is rejected because its effects are not propagated into the transform.
fn block_calls_function(body: &CheckedBody) -> bool {
    body.statements().iter().any(statement_calls_function)
}

fn statement_calls_function(statement: &crate::CheckedStmt) -> bool {
    use crate::CheckedStmt;
    match statement {
        CheckedStmt::Const { value, .. }
        | CheckedStmt::Throw { value, .. }
        | CheckedStmt::Expr { value, .. } => expr_calls_function(value),
        CheckedStmt::Var { value, .. } | CheckedStmt::Return { value, .. } => {
            value.as_ref().is_some_and(expr_calls_function)
        }
        CheckedStmt::Assign { target, value, .. } => {
            expr_calls_function(target) || expr_calls_function(value)
        }
        CheckedStmt::Delete { path, .. } => expr_calls_function(path),
        CheckedStmt::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            condition.as_ref().is_some_and(expr_calls_function)
                || block_calls_function(then_block)
                || else_ifs.iter().any(|else_if| {
                    else_if.condition.as_ref().is_some_and(expr_calls_function)
                        || block_calls_function(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_calls_function)
        }
        CheckedStmt::While {
            condition, body, ..
        } => condition.as_ref().is_some_and(expr_calls_function) || block_calls_function(body),
        CheckedStmt::For {
            iterable,
            step,
            body,
            ..
        } => {
            expr_calls_function(iterable)
                || step.as_ref().is_some_and(expr_calls_function)
                || block_calls_function(body)
        }
        CheckedStmt::Transaction { body, .. } => block_calls_function(body),
        CheckedStmt::Try {
            body,
            catch,
            finally,
            ..
        } => {
            block_calls_function(body)
                || catch
                    .as_ref()
                    .is_some_and(|catch| block_calls_function(&catch.block))
                || finally.as_ref().is_some_and(block_calls_function)
        }
        CheckedStmt::Match {
            scrutinee, arms, ..
        } => {
            scrutinee.as_ref().is_some_and(expr_calls_function)
                || arms.iter().any(|arm| block_calls_function(&arm.block))
        }
        CheckedStmt::Break { .. } | CheckedStmt::Continue { .. } => false,
    }
}

fn expr_calls_function(expr: &crate::CheckedExpr) -> bool {
    use crate::{CheckedCallTarget, CheckedExpr, CheckedInterpolationPart};
    match expr {
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            matches!(target, CheckedCallTarget::Function(_))
                || expr_calls_function(callee)
                || args.iter().any(|arg| expr_calls_function(&arg.value))
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            expr_calls_function(base)
        }
        CheckedExpr::Unary { operand, .. } => expr_calls_function(operand),
        CheckedExpr::Binary { left, right, .. } => {
            expr_calls_function(left) || expr_calls_function(right)
        }
        CheckedExpr::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            CheckedInterpolationPart::Expr(expr) => expr_calls_function(expr),
            CheckedInterpolationPart::Text { .. } => false,
        }),
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {
            false
        }
    }
}

/// The catalog paths every `evolve default` and `evolve transform` step targets, across
/// all evolve blocks. A transform may not read a member the same evolve surface rewrites
/// (its `old` value is the pre-evolution one, not the value the default or transform
/// produces), so the read check needs the whole set, not just the current step.
fn rewritten_target_paths(parsed_files: &[(&Path, &ParsedSource)]) -> HashSet<String> {
    let mut targets = HashSet::new();
    for (_, parsed) in parsed_files {
        let module = module_name(parsed);
        for declaration in &parsed.file.declarations {
            let Declaration::Evolve(evolve) = declaration else {
                continue;
            };
            for step in &evolve.steps {
                let target = match step {
                    EvolveStep::Transform { target, .. } | EvolveStep::Default { target, .. } => {
                        target
                    }
                    EvolveStep::Rename { .. } | EvolveStep::Retire { .. } => continue,
                };
                if let Some(path) = target_path(module, target) {
                    targets.insert(path);
                }
            }
        }
    }
    targets
}

/// The per-file context default and transform steps resolve against: the bound
/// program, the evolve block's file and module, the catalog entries current source
/// declares, and the catalog paths every default or transform rewrites.
struct TypeContext<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    module: &'a str,
    source_entries: &'a [SourceCatalogEntry],
    prelude: &'a FilePrelude,
    rewritten_targets: &'a HashSet<String>,
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
        // A transform must target a top-level saved resource member: only a member has a
        // leaf type its result encodes to and old bytes apply reads, and read resolution
        // and the per-record write address handle only a plain field directly under the
        // record node. A store root, index, or enum target, or a nested member under a
        // group or keyed layer, is rejected.
        if !self.target_is_top_level_member(target) {
            diagnostics.push(self.transform_error(
                target_span(target, span),
                "an evolve transform must target a top-level saved resource member".to_string(),
            ));
            return;
        }
        let (Some(member_type), Some(resource)) =
            (self.member_type(target), self.resource_of_target(target))
        else {
            diagnostics.push(self.transform_error(
                target_span(target, span),
                "an evolve transform must target a top-level saved resource member".to_string(),
            ));
            return;
        };
        // The body is a pure function from `old` (the record's current-typed values
        // before this evolution) to the target's new value. Holding it to the same
        // structural rules and the same name-resolution and type pass catches an
        // undefined identifier or unknown call at check time; binding `old` as the
        // resource type makes `old.<member>` reads type against the current schema, and
        // the target member type as the return type makes the result type-check.
        crate::rules::check_transform_body(self.file, body, diagnostics);
        let mut scope = vec![self.prelude.module_constants.clone()];
        scope.push(HashMap::from([(
            "old".to_string(),
            MarrowType::Resource(resource),
        )]));
        check_block_types(
            self.program,
            self.file,
            &member_type,
            body,
            &mut scope,
            &self.prelude.aliases,
            diagnostics,
        );
        self.check_read_restrictions(target, body, diagnostics);
    }

    /// Reject reading, through `old.<member>`, the transform's own target or any member
    /// the same evolve surface rewrites with a default or another transform. `old`
    /// exposes each member's pre-evolution value, so reading a member this evolution
    /// changes computes from a value the developer is replacing, not the intended
    /// post-evolution one. The read member's catalog path is compared against the
    /// rewritten-target paths.
    fn check_read_restrictions(
        &self,
        target: &Expression,
        body: &Block,
        diagnostics: &mut Vec<CheckDiagnostic>,
    ) {
        let Some(resource_prefix) = self
            .target_member_path(target)
            .and_then(|path| path.rsplit_once("::").map(|(head, _)| head.to_string()))
        else {
            return;
        };
        let target_path = self.target_member_path(target);
        for read in old_field_reads(body) {
            let read_path = format!("{resource_prefix}::{}", read.field);
            let message = if Some(&read_path) == target_path.as_ref() {
                Some(format!(
                    "a transform cannot read its own target `old.{}`; compute it from other members",
                    read.field
                ))
            } else if self.rewritten_targets.contains(&read_path) {
                Some(format!(
                    "a transform cannot read `old.{}`, which the same evolve block changes; read a member no default or transform rewrites",
                    read.field
                ))
            } else {
                None
            };
            if let Some(message) = message {
                diagnostics.push(self.transform_error(read.span, message));
            }
        }
    }

    fn transform_error(&self, span: SourceSpan, message: String) -> CheckDiagnostic {
        CheckDiagnostic {
            code: crate::CHECK_EVOLVE_TRANSFORM,
            severity: Severity::Error,
            file: self.file.to_path_buf(),
            message,
            span,
        }
    }

    /// The module-qualified catalog path the target names, used to compare reads
    /// against transform targets.
    fn target_member_path(&self, target: &Expression) -> Option<String> {
        target_path(self.module, target)
    }

    /// Whether the target names a top-level member of a resource: a bare resource name
    /// followed by exactly one member segment (`Book.priceCents`). A deeper chain
    /// (`Book.name.first`) names a nested member the transform model does not support.
    fn target_is_top_level_member(&self, target: &Expression) -> bool {
        member_chain(target).is_some_and(|(_, chain)| chain.len() == 1)
    }

    /// The qualified resource type name a member target belongs to, when it names a
    /// member of a resource declared in this block's module and file.
    fn resource_of_target(&self, target: &Expression) -> Option<String> {
        let (resource_name, _) = member_chain(target)?;
        let module = self
            .program
            .modules
            .iter()
            .find(|module| module.name == self.module && module.source_file == self.file)?;
        module
            .resources
            .iter()
            .find(|resource| resource.name == resource_name)
            .map(|resource| crate::resource_type_name(self.module, &resource.name))
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

/// The `evolve transform` body in `parsed` whose target resolves to `target_path`, used
/// by runtime-body lowering to find the syntax for a bound transform. The checked
/// program carries no syntax body, so lowering reads the body back from the parse the
/// same way function lowering does.
pub(crate) fn transform_body_in_source<'a>(
    parsed: &'a ParsedSource,
    module: &str,
    wanted_path: &str,
) -> Option<&'a Block> {
    for declaration in &parsed.file.declarations {
        let Declaration::Evolve(evolve) = declaration else {
            continue;
        };
        for step in &evolve.steps {
            if let EvolveStep::Transform { target, body, .. } = step
                && target_path(module, target).as_deref() == Some(wanted_path)
            {
                return Some(body);
            }
        }
    }
    None
}

/// The catalog paths of the members a transform body reads via `old.<member>`, derived
/// from the body and the target's own catalog path. The resource prefix is the target
/// path with its member segment dropped, so each read `old.<field>` maps to
/// `<resource>::<field>`. The paths are deduplicated in source order; the catalog
/// binding resolves them to stable ids and discharge proves their decodability.
fn transform_read_paths(target_path: &str, body: &Block) -> Vec<String> {
    let Some((resource_prefix, _)) = target_path.rsplit_once("::") else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for read in old_field_reads(body) {
        let path = format!("{resource_prefix}::{}", read.field);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// One `old.<field>` read inside a transform body: the immediate field name read off
/// `old` and the span of the read. The transform model reads top-level members, so
/// only an immediate field of the `old` binding is a read; a deeper access is rejected
/// by the type pass when it does not name a member.
struct OldFieldRead {
    field: String,
    span: SourceSpan,
}

/// Collect every `old.<field>` read in a transform body, in source order. This walks
/// the whole body — every statement, condition, and sub-expression — so a read buried
/// in a branch or interpolation is still found.
fn old_field_reads(body: &Block) -> Vec<OldFieldRead> {
    let mut reads = Vec::new();
    walk_block_reads(body, &mut reads);
    reads
}

fn walk_block_reads(block: &Block, reads: &mut Vec<OldFieldRead>) {
    for statement in &block.statements {
        walk_statement_reads(statement, reads);
    }
}

fn walk_statement_reads(statement: &Statement, reads: &mut Vec<OldFieldRead>) {
    match statement {
        Statement::Const { value, .. }
        | Statement::Throw { value, .. }
        | Statement::Expr { value, .. } => walk_expr_reads(value, reads),
        Statement::Var { value, .. } | Statement::Return { value, .. } => {
            if let Some(value) = value {
                walk_expr_reads(value, reads);
            }
        }
        Statement::Assign { target, value, .. } => {
            walk_expr_reads(target, reads);
            walk_expr_reads(value, reads);
        }
        Statement::Delete { path, .. } => walk_expr_reads(path, reads),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                walk_expr_reads(condition, reads);
            }
            walk_block_reads(then_block, reads);
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    walk_expr_reads(condition, reads);
                }
                walk_block_reads(&else_if.block, reads);
            }
            if let Some(block) = else_block {
                walk_block_reads(block, reads);
            }
        }
        Statement::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                walk_expr_reads(condition, reads);
            }
            walk_block_reads(body, reads);
        }
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => {
            walk_expr_reads(iterable, reads);
            if let Some(step) = step {
                walk_expr_reads(step, reads);
            }
            walk_block_reads(body, reads);
        }
        Statement::Transaction { body, .. } => walk_block_reads(body, reads),
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            walk_block_reads(body, reads);
            if let Some(catch) = catch {
                walk_block_reads(&catch.block, reads);
            }
            if let Some(finally) = finally {
                walk_block_reads(finally, reads);
            }
        }
        Statement::Match {
            scrutinee, arms, ..
        } => {
            if let Some(scrutinee) = scrutinee {
                walk_expr_reads(scrutinee, reads);
            }
            for arm in arms {
                walk_block_reads(&arm.block, reads);
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } => {}
    }
}

fn walk_expr_reads(expr: &Expression, reads: &mut Vec<OldFieldRead>) {
    match expr {
        Expression::Field {
            base, name, span, ..
        }
        | Expression::OptionalField {
            base, name, span, ..
        } => {
            if is_old_name(base) {
                reads.push(OldFieldRead {
                    field: name.clone(),
                    span: *span,
                });
            }
            walk_expr_reads(base, reads);
        }
        Expression::Call { callee, args, .. } => {
            walk_expr_reads(callee, reads);
            for Argument { value, .. } in args {
                walk_expr_reads(value, reads);
            }
        }
        Expression::Unary { operand, .. } => walk_expr_reads(operand, reads),
        Expression::Binary { left, right, .. } => {
            walk_expr_reads(left, reads);
            walk_expr_reads(right, reads);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    walk_expr_reads(expr, reads);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn is_old_name(expr: &Expression) -> bool {
    matches!(expr, Expression::Name { segments, .. } if segments == &["old".to_string()])
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
