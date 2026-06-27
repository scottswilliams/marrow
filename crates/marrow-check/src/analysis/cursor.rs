//! Cursor type and scope lookups over a checked program: the read-only surface
//! editor tooling consumes for hover and completion. These walk the parse the
//! pipeline already built, reconstructing the cursor's lexical scope exactly as
//! the checker does, and record no diagnostics.

use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::SourceSpan;

use crate::checks::{catch_frame, file_prelude, for_frame};
use crate::enums::resolve_type;
use crate::infer::{bind, infer_only, infer_type, local_binding};
use crate::walk::for_each_child_expr;
use crate::{CheckedProgram, MarrowType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeCompletionBinding {
    pub(crate) name: String,
    pub(crate) kind: ScopeCompletionBindingKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScopeCompletionBindingKind {
    Value { ty: MarrowType },
    ModuleAlias { module: Vec<String> },
}

/// The type of the expression at byte `offset` in `parsed` (a file of `program`),
/// or `None` when no expression covers the offset. Editor tooling uses this for
/// hover and type-aware actions. It reconstructs the cursor's lexical scope
/// exactly as the checker does — module constants and imports, the enclosing
/// function's parameters, the `const`/`var` bindings that precede the cursor, and
/// any loop or catch binding whose body the cursor sits in — then infers the
/// smallest expression covering the offset. It records no diagnostics.
pub fn type_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Option<MarrowType> {
    let prelude = file_prelude(program, file, parsed);
    let function = enclosing_function(parsed, offset)?;
    let mut scope = function_base_scope(
        program,
        function,
        &prelude.module_constants,
        &prelude.aliases,
        file,
    );
    walk_block_to_offset(
        program,
        &function.body,
        offset,
        &prelude.aliases,
        file,
        &mut scope,
    );
    let expr = smallest_expression_at(&function.body, offset)?;
    Some(infer_type(
        program,
        expr,
        &scope,
        &prelude.aliases,
        file,
        &mut Vec::new(),
    ))
}

/// The visible typed bindings at byte `offset` in `parsed`, as `(name, type)`
/// pairs. The reconstructed scope is the same one [`type_at`] infers against:
/// module constants and imports, then — when the offset is inside a function —
/// that function's parameters, the `const`/`var` locals declared before the
/// cursor, and any loop or catch binding in scope. Import aliases are surfaced
/// with [`MarrowType::Unknown`] (they name modules, not values). Inner bindings
/// shadow outer ones. It records no diagnostics.
pub fn scope_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Vec<(String, MarrowType)> {
    let prelude = file_prelude(program, file, parsed);
    let function = enclosing_function(parsed, offset);
    let module_constants = if function.is_some() {
        prelude.module_constants.clone()
    } else {
        visible_module_constants_before(parsed, &prelude.module_constants, offset)
    };
    let mut scope: Vec<HashMap<String, MarrowType>> = vec![
        prelude
            .aliases
            .keys()
            .map(|alias| (alias.clone(), MarrowType::Unknown))
            .collect(),
        module_constants,
    ];
    if let Some(function) = function {
        scope.extend(function_base_scope(
            program,
            function,
            &prelude.module_constants,
            &prelude.aliases,
            file,
        ));
        walk_block_to_offset(
            program,
            &function.body,
            offset,
            &prelude.aliases,
            file,
            &mut scope,
        );
    }
    let mut visible: HashMap<String, MarrowType> = HashMap::new();
    for frame in scope {
        visible.extend(frame);
    }
    let mut bindings: Vec<(String, MarrowType)> = visible.into_iter().collect();
    bindings.sort_by(|(left, _), (right, _)| left.cmp(right));
    bindings
}

pub(crate) fn scope_completion_bindings_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Vec<ScopeCompletionBinding> {
    let prelude = file_prelude(program, file, parsed);
    let function = enclosing_function(parsed, offset);
    let module_constants = if function.is_some() {
        prelude.module_constants.clone()
    } else {
        visible_module_constants_before(parsed, &prelude.module_constants, offset)
    };
    let mut value_scope: Vec<HashMap<String, MarrowType>> = vec![
        prelude
            .aliases
            .keys()
            .map(|alias| (alias.clone(), MarrowType::Unknown))
            .collect(),
        module_constants.clone(),
    ];
    let mut completion_scope: Vec<HashMap<String, ScopeCompletionBindingKind>> = vec![
        import_alias_completion_frame(&parsed.file),
        completion_frame_from_values(&module_constants),
    ];
    if let Some(function) = function {
        let base = function_base_scope(
            program,
            function,
            &prelude.module_constants,
            &prelude.aliases,
            file,
        );
        push_value_frames(&mut value_scope, &mut completion_scope, base);
        walk_block_to_offset_with_completion(
            program,
            &function.body,
            offset,
            &prelude.aliases,
            file,
            &mut value_scope,
            Some(&mut completion_scope),
        );
    }
    let mut visible: HashMap<String, ScopeCompletionBindingKind> = HashMap::new();
    for frame in completion_scope {
        visible.extend(frame);
    }
    let mut bindings: Vec<ScopeCompletionBinding> = visible
        .into_iter()
        .map(|(name, kind)| ScopeCompletionBinding { name, kind })
        .collect();
    bindings.sort_by(|a, b| a.name.cmp(&b.name));
    bindings
}

pub(crate) fn debug_expression_scope_before(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    span: SourceSpan,
) -> Vec<HashMap<String, MarrowType>> {
    let prelude = file_prelude(program, file, parsed);
    let Some(function) = enclosing_function(parsed, span.start_byte) else {
        return vec![visible_module_constants_before(
            parsed,
            &prelude.module_constants,
            span.start_byte,
        )];
    };
    let mut scope = function_base_scope(
        program,
        function,
        &prelude.module_constants,
        &prelude.aliases,
        file,
    );
    walk_block_to_offset(
        program,
        &function.body,
        span.start_byte,
        &prelude.aliases,
        file,
        &mut scope,
    );
    scope
}

fn visible_module_constants_before(
    parsed: &marrow_syntax::ParsedSource,
    module_constants: &HashMap<String, MarrowType>,
    offset: usize,
) -> HashMap<String, MarrowType> {
    let mut visible = module_constants.clone();
    for declaration in &parsed.file.declarations {
        let marrow_syntax::Declaration::Const(constant) = declaration else {
            continue;
        };
        if constant.span.start_byte >= offset || span_covers(constant.span, offset) {
            visible.remove(&constant.name);
        }
    }
    visible
}

/// The function declaration whose body span covers `offset`, if any. A cursor in a
/// function signature or at module level has no enclosing body and yields `None`.
fn enclosing_function(
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Option<&marrow_syntax::FunctionDecl> {
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            marrow_syntax::Declaration::Function(function)
                if span_covers(function.body.span, offset) =>
            {
                Some(function)
            }
            _ => None,
        })
}

/// The base scope frame for a function body: the module's constants overlaid with
/// the parameter list, mirroring [`check_function_types`] (a parameter shadows a
/// like-named constant).
fn function_base_scope(
    program: &CheckedProgram,
    function: &marrow_syntax::FunctionDecl,
    module_constants: &HashMap<String, MarrowType>,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Vec<HashMap<String, MarrowType>> {
    let mut base = module_constants.clone();
    for param in &function.params {
        base.insert(
            param.name.clone(),
            MarrowType::keyed(
                param
                    .keys
                    .iter()
                    .map(|key| resolve_type(&key.ty, program, aliases, file)),
                resolve_type(&param.ty, program, aliases, file),
            ),
        );
    }
    vec![base]
}

/// Replay the binding behavior of [`check_block_types`]/[`check_statement_types`]
/// up to `offset`: push a frame for `block`, record each `const`/`var` binding the
/// block introduces before the cursor, and descend into the one nested block (and
/// its loop or catch frame) that covers the cursor. Statements after the cursor
/// are not visible and are skipped. This shares the checker's binding primitives
/// (`local_binding`, the loop/catch frames) so the reconstructed scope cannot
/// drift from the one the checker builds.
fn walk_block_to_offset(
    program: &CheckedProgram,
    block: &marrow_syntax::Block,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    scope: &mut Vec<HashMap<String, MarrowType>>,
) {
    walk_block_to_offset_with_completion(program, block, offset, aliases, file, scope, None);
}

fn walk_block_to_offset_with_completion(
    program: &CheckedProgram,
    block: &marrow_syntax::Block,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    mut completion_scope: Option<&mut Vec<HashMap<String, ScopeCompletionBindingKind>>>,
) {
    scope.push(HashMap::new());
    if let Some(completion_scope) = completion_scope.as_deref_mut() {
        completion_scope.push(HashMap::new());
    }
    for statement in &block.statements {
        // A binding declared at or after the cursor is not yet in scope. Compared
        // against the statement's start so the cursor on a `const`'s own line does
        // not see that `const` (its initializer cannot reference itself).
        if statement.span().start_byte >= offset {
            break;
        }
        let statement_covers_offset = span_covers(statement.span(), offset);
        let in_initializer = statement_covers_offset
            && binding_initializer_span(statement).is_some_and(|span| span_covers(span, offset));
        // Record the binding this statement introduces after its initializer,
        // exactly as the checker does, before deciding whether to descend into it.
        if !in_initializer
            && let Some((name, ty)) = local_binding(program, statement, scope, aliases, file)
        {
            bind(scope, &name, ty.clone());
            if let Some(completion_scope) = completion_scope.as_deref_mut() {
                bind_completion_value(completion_scope, name, ty);
            }
        }
        // Descend into the nested block (and its loop/catch frame) that the cursor
        // sits in. Only one statement can cover the cursor, so the walk stops here.
        if statement_covers_offset {
            if let Some(body) = descend_target(
                program,
                statement,
                offset,
                aliases,
                file,
                scope,
                completion_scope.as_deref_mut(),
            ) {
                walk_block_to_offset_with_completion(
                    program,
                    body,
                    offset,
                    aliases,
                    file,
                    scope,
                    completion_scope,
                );
            }
            return;
        }
    }
}

fn binding_initializer_span(statement: &marrow_syntax::Statement) -> Option<SourceSpan> {
    match statement {
        marrow_syntax::Statement::Const { value, .. } => Some(value.span()),
        marrow_syntax::Statement::Var { value, .. } => value.as_ref().map(|value| value.span()),
        _ => None,
    }
}

/// The nested block of `statement` that covers `offset`, pushing the loop or catch
/// frame that block runs under (a `for` binding, a `catch` error value) onto
/// `scope` first, mirroring [`check_statement_types`]. Returns `None` when the
/// cursor is in the statement but not in one of its bodies (for example in an `if`
/// condition), leaving `scope` untouched.
fn descend_target<'b>(
    program: &CheckedProgram,
    statement: &'b marrow_syntax::Statement,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    mut completion_scope: Option<&mut Vec<HashMap<String, ScopeCompletionBindingKind>>>,
) -> Option<&'b marrow_syntax::Block> {
    use marrow_syntax::Statement;
    match statement {
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if span_covers(then_block.span, offset) {
                return Some(then_block);
            }
            for else_if in else_ifs {
                if span_covers(else_if.block.span, offset) {
                    return Some(&else_if.block);
                }
            }
            else_block
                .as_ref()
                .filter(|block| span_covers(block.span, offset))
        }
        Statement::IfConst {
            name,
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if span_covers(then_block.span, offset) {
                let ty = infer_only(program, value, scope, aliases, file);
                let mut frame = HashMap::new();
                frame.insert(name.clone(), ty.clone());
                scope.push(frame);
                if let Some(completion_scope) = completion_scope.as_deref_mut() {
                    let mut frame = HashMap::new();
                    frame.insert(name.clone(), ScopeCompletionBindingKind::Value { ty });
                    completion_scope.push(frame);
                }
                return Some(then_block);
            }
            for else_if in else_ifs {
                if span_covers(else_if.block.span, offset) {
                    return Some(&else_if.block);
                }
            }
            else_block
                .as_ref()
                .filter(|block| span_covers(block.span, offset))
        }
        Statement::While { body, .. } | Statement::Transaction { body, .. } => {
            span_covers(body.span, offset).then_some(body)
        }
        Statement::For {
            binding,
            iterable,
            body,
            ..
        } => {
            if !span_covers(body.span, offset) {
                return None;
            }
            let frame = for_frame(program, binding, iterable, scope, aliases, file);
            scope.push(frame.clone());
            if let Some(completion_scope) = completion_scope.as_deref_mut() {
                completion_scope.push(completion_frame_from_values(&frame));
            }
            Some(body)
        }
        Statement::Try { body, catch, .. } => {
            if span_covers(body.span, offset) {
                return Some(body);
            }
            if let Some(clause) = catch
                && span_covers(clause.block.span, offset)
            {
                let frame = catch_frame(clause);
                scope.push(frame.clone());
                if let Some(completion_scope) = completion_scope {
                    completion_scope.push(completion_frame_from_values(&frame));
                }
                return Some(&clause.block);
            }
            None
        }
        Statement::Match { arms, .. } => arms
            .iter()
            .find(|arm| span_covers(arm.block.span, offset))
            .map(|arm| &arm.block),
        _ => None,
    }
}

fn push_value_frames(
    scope: &mut Vec<HashMap<String, MarrowType>>,
    completion_scope: &mut Vec<HashMap<String, ScopeCompletionBindingKind>>,
    frames: Vec<HashMap<String, MarrowType>>,
) {
    for frame in frames {
        completion_scope.push(completion_frame_from_values(&frame));
        scope.push(frame);
    }
}

fn completion_frame_from_values(
    frame: &HashMap<String, MarrowType>,
) -> HashMap<String, ScopeCompletionBindingKind> {
    frame
        .iter()
        .map(|(name, ty)| {
            (
                name.clone(),
                ScopeCompletionBindingKind::Value { ty: ty.clone() },
            )
        })
        .collect()
}

fn import_alias_completion_frame(
    source_file: &marrow_syntax::SourceFile,
) -> HashMap<String, ScopeCompletionBindingKind> {
    let mut frame = HashMap::new();
    for use_decl in &source_file.uses {
        let alias = crate::short_name(&use_decl.name);
        if frame.contains_key(alias) {
            continue;
        }
        if let Ok(Some(module)) = crate::driver::unique_import_module_alias_path(source_file, alias)
        {
            frame.insert(
                alias.to_string(),
                ScopeCompletionBindingKind::ModuleAlias { module },
            );
        }
    }
    frame
}

fn bind_completion_value(
    scope: &mut [HashMap<String, ScopeCompletionBindingKind>],
    name: String,
    ty: MarrowType,
) {
    if let Some(frame) = scope.last_mut() {
        frame.insert(name, ScopeCompletionBindingKind::Value { ty });
    }
}

/// Whether `span` covers `offset`, inclusive of the end byte so a cursor at the
/// closing edge of an expression still resolves.
pub(crate) fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

/// The smallest expression in a function `body` whose span covers `offset`, the
/// expression the cursor sits on. Walks every expression the type pass would
/// visit, keeping the tightest span. Statement-level structure (conditions,
/// initializers, call arguments, nested blocks) is traversed so the cursor lands
/// on the leaf expression rather than an enclosing one.
fn smallest_expression_at(
    body: &marrow_syntax::Block,
    offset: usize,
) -> Option<&marrow_syntax::Expression> {
    let mut best: Option<&marrow_syntax::Expression> = None;
    collect_block_expression(body, offset, &mut best);
    best
}

fn collect_block_expression<'b>(
    block: &'b marrow_syntax::Block,
    offset: usize,
    best: &mut Option<&'b marrow_syntax::Expression>,
) {
    use marrow_syntax::Statement;
    for statement in &block.statements {
        match statement {
            Statement::Const { value, .. } | Statement::Throw { value, .. } => {
                collect_expression(value, offset, best);
            }
            Statement::Expr { value, .. } => collect_expression(value, offset, best),
            Statement::Var { value, .. } => {
                if let Some(value) = value {
                    collect_expression(value, offset, best);
                }
            }
            Statement::Assign { target, value, .. } => {
                collect_expression(target, offset, best);
                collect_expression(value, offset, best);
            }
            Statement::Delete { path, .. } => collect_expression(path, offset, best),
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    collect_expression(value, offset, best);
                }
            }
            Statement::ReturnAbsent { .. } => {}
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                if let Some(condition) = condition {
                    collect_expression(condition, offset, best);
                }
                collect_block_expression(then_block, offset, best);
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        collect_expression(condition, offset, best);
                    }
                    collect_block_expression(&else_if.block, offset, best);
                }
                if let Some(block) = else_block {
                    collect_block_expression(block, offset, best);
                }
            }
            Statement::IfConst {
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                collect_expression(value, offset, best);
                collect_block_expression(then_block, offset, best);
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        collect_expression(condition, offset, best);
                    }
                    collect_block_expression(&else_if.block, offset, best);
                }
                if let Some(block) = else_block {
                    collect_block_expression(block, offset, best);
                }
            }
            Statement::While {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_expression(condition, offset, best);
                }
                collect_block_expression(body, offset, best);
            }
            Statement::For { iterable, body, .. } => {
                collect_expression(iterable, offset, best);
                collect_block_expression(body, offset, best);
            }
            Statement::Transaction { body, .. } => collect_block_expression(body, offset, best),
            Statement::Try { body, catch, .. } => {
                collect_block_expression(body, offset, best);
                if let Some(clause) = catch {
                    collect_block_expression(&clause.block, offset, best);
                }
            }
            Statement::Match {
                scrutinee, arms, ..
            } => {
                if let Some(scrutinee) = scrutinee {
                    collect_expression(scrutinee, offset, best);
                }
                for arm in arms {
                    collect_block_expression(&arm.block, offset, best);
                }
            }
            Statement::Break { .. } | Statement::Continue { .. } => {}
        }
    }
}

/// Keep `expr` as the best match when its span covers `offset` and is no wider
/// than the current best, then recurse into its subexpressions so the tightest
/// covering leaf wins.
fn collect_expression<'e>(
    expr: &'e marrow_syntax::Expression,
    offset: usize,
    best: &mut Option<&'e marrow_syntax::Expression>,
) {
    let span = expr.span();
    if !span_covers(span, offset) {
        return;
    }
    let width = span.end_byte.saturating_sub(span.start_byte);
    let replace = best.is_none_or(|current| {
        let current = current.span();
        width <= current.end_byte.saturating_sub(current.start_byte)
    });
    if replace {
        *best = Some(expr);
    }
    for_each_child_expr(expr, |child| collect_expression(child, offset, best));
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use marrow_project::parse_config;
    use marrow_syntax::parse_source;

    use super::*;
    use crate::{ProjectSources, analyze_project};

    static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn scope_at_preserves_ambiguous_import_aliases_for_legacy_callers() {
        let project = ScopeProject::new();
        let (source, offset) = source_with_cursor(
            "\
module app::main

use shelf::books
use archive::books

pub fn f()
    return |
",
        );
        let parsed = parse_source(&source);

        assert_eq!(
            type_of(
                scope_at(project.program(), project.app_file(), &parsed, offset),
                "books"
            ),
            Some(MarrowType::Unknown)
        );
        assert!(
            !scope_completion_bindings_at(project.program(), project.app_file(), &parsed, offset)
                .iter()
                .any(|binding| matches!(
                    (&*binding.name, &binding.kind),
                    ("books", ScopeCompletionBindingKind::ModuleAlias { .. })
                ))
        );
    }

    #[test]
    fn scope_at_preserves_top_level_shadowed_import_alias_before_declaration() {
        let project = ScopeProject::new();
        let (source, offset) = source_with_cursor(
            "\
module app::main

use shelf::books

const books = |1
",
        );
        let parsed = parse_source(&source);

        assert_eq!(
            type_of(
                scope_at(project.program(), project.app_file(), &parsed, offset),
                "books"
            ),
            Some(MarrowType::Unknown)
        );
        assert!(
            !scope_completion_bindings_at(project.program(), project.app_file(), &parsed, offset)
                .iter()
                .any(|binding| matches!(
                    (&*binding.name, &binding.kind),
                    ("books", ScopeCompletionBindingKind::ModuleAlias { .. })
                ))
        );
    }

    fn type_of(bindings: Vec<(String, MarrowType)>, name: &str) -> Option<MarrowType> {
        bindings
            .into_iter()
            .find_map(|(binding, ty)| (binding == name).then_some(ty))
    }

    fn source_with_cursor(source: &str) -> (String, usize) {
        let offset = source.find('|').expect("cursor marker");
        let source = source.replacen('|', "", 1);
        (source, offset)
    }

    struct ScopeProject {
        root: PathBuf,
        program: CheckedProgram,
        app: PathBuf,
    }

    impl ScopeProject {
        fn new() -> Self {
            let root = unique_temp_dir();
            std::fs::create_dir_all(root.join("src/app")).expect("create app dir");
            std::fs::create_dir_all(root.join("src/shelf")).expect("create shelf dir");
            std::fs::create_dir_all(root.join("src/archive")).expect("create archive dir");
            std::fs::write(
                root.join("marrow.json"),
                r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
            )
            .expect("write config");
            let shelf = root.join("src/shelf/books.mw");
            let shelf_source = "module shelf::books\n\npub fn shelfOnly(): int\n    return 1\n";
            std::fs::write(&shelf, shelf_source).expect("write shelf module");
            let archive = root.join("src/archive/books.mw");
            let archive_source =
                "module archive::books\n\npub fn archiveOnly(): int\n    return 2\n";
            std::fs::write(&archive, archive_source).expect("write archive module");
            let app = root.join("src/app/main.mw");
            let app_source = "module app::main\n\npub fn f()\n    return\n";
            std::fs::write(&app, app_source).expect("write app module");
            let config =
                parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
                    .expect("parse config");
            let sources = ProjectSources::new()
                .with(&shelf, shelf_source)
                .with(&archive, archive_source)
                .with(&app, app_source);
            let snapshot = analyze_project(&root, &config, &sources, None, None).expect("analyze");
            Self {
                root,
                program: snapshot.program,
                app,
            }
        }

        fn program(&self) -> &CheckedProgram {
            &self.program
        }

        fn app_file(&self) -> &Path {
            &self.app
        }
    }

    impl Drop for ScopeProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let name = format!(
            "marrow-cursor-scope-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            NEXT_TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed)
        );
        std::env::temp_dir().join(name)
    }
}
