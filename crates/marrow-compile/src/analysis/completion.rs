//! The per-query read-only completion re-resolution.
//!
//! This is a distinct read-only pass over the query-local parse. It never drives the
//! compile-path lowerer or resolver — whose arms assume post-`has_errors` input and can
//! raise resolution invariants — so it runs safely on a broken file and leaks no
//! diagnostic. A partial or unresolvable base yields an empty candidate set; the position
//! class is derived purely positionally.

use marrow_syntax::{
    Block, CheckedBind, Declaration, ElseIf, EnumDecl, EnumMember, Expression, ForBinding,
    FunctionDecl, IfConstBinding, MatchArm, NameSegment, Recovery, ResourceMember, SourceSpan,
    Statement, TraversalBound, TypeExpr,
};

use crate::lower::builtin_value_names;
use crate::scalar::ScalarType;

use super::{
    AnalysisResourceLimit, Candidate, CandidateKind, CompletionOutcome, Completions, Fact,
    MAX_COMPLETION_CANDIDATES, MAX_COMPLETION_RENDER_BYTES, PositionClass, QueryFile,
};

/// One in-scope binding: its spelling and, when annotated, its declared type node
/// borrowed from that parse. The node is what the fail-soft type probe reads.
struct Binding<'a> {
    name: String,
    ty: Option<&'a TypeExpr>,
}

/// The lexical scope accumulated while descending to the offset: the enclosing
/// declaration's generic type parameters, its parameters, and the locals introduced
/// before the offset. Only bindings that precede the offset on the path to it are
/// added, so this is never a superset.
#[derive(Default)]
struct Scope<'a> {
    type_params: Vec<String>,
    params: Vec<Binding<'a>>,
    locals: Vec<Binding<'a>>,
}

/// The positional classification of the offset, with the base receiver borrowed for a
/// member or enum-path position.
enum Located<'a> {
    ExprName,
    Member(&'a Expression),
    EnumPath(&'a Expression),
    TypeAnnotation,
}

/// Classify the offset over the queried file's tree and enumerate the class namespace.
pub(super) fn resolve(file: &QueryFile<'_>) -> CompletionOutcome {
    let offset = file.offset;
    let mut scope = Scope::default();
    let Some(located) = locate_file(file, offset, &mut scope) else {
        return CompletionOutcome::Ready(Fact::Absent);
    };
    let (class, candidates) = match located {
        Located::ExprName => (
            PositionClass::ExpressionName,
            expression_name_candidates(file, &scope),
        ),
        Located::Member(base) => (PositionClass::Member, member_candidates(file, &scope, base)),
        Located::EnumPath(base) => (PositionClass::EnumPath, enum_path_candidates(file, base)),
        Located::TypeAnnotation => (
            PositionClass::TypeAnnotation,
            type_annotation_candidates(file, &scope),
        ),
    };
    finish(class, candidates)
}

/// Apply the per-query candidate-count and render-byte caps, then package the fact. An
/// over-cap namespace is a query-local refusal, never a truncated prefix.
fn finish(class: PositionClass, candidates: Vec<Candidate>) -> CompletionOutcome {
    if candidates.len() as u64 > MAX_COMPLETION_CANDIDATES {
        return CompletionOutcome::Refused(AnalysisResourceLimit::CompletionCandidateCount {
            limit: MAX_COMPLETION_CANDIDATES,
        });
    }
    let bytes: u64 = candidates
        .iter()
        .map(|candidate| (candidate.label.len() + candidate.detail.len()) as u64)
        .sum();
    if bytes > MAX_COMPLETION_RENDER_BYTES {
        return CompletionOutcome::Refused(AnalysisResourceLimit::CompletionRenderBytes {
            limit: MAX_COMPLETION_RENDER_BYTES,
        });
    }
    CompletionOutcome::Ready(Fact::Present(Completions { class, candidates }))
}

pub(super) fn contains(span: SourceSpan, offset: u32) -> bool {
    span.start_byte as u32 <= offset && offset <= span.end_byte as u32
}

fn ends_before(span: SourceSpan, offset: u32) -> bool {
    (span.end_byte as u32) < offset
}

/// The byte extent of a declaration, including its body. A `fn`/`test` declaration's
/// own `span` covers only the header through the opening brace; the body block is a
/// separate span, so the extent unions the two. Every other declaration's `span`
/// already covers its whole construct.
pub(super) fn declaration_contains(declaration: &Declaration, offset: u32) -> bool {
    let (start, end) = match declaration {
        Declaration::Function(function) => (function.span.start_byte, function.body.span.end_byte),
        Declaration::Test(test) => (test.span.start_byte, test.body.span.end_byte),
        Declaration::Alias(alias) => (alias.span.start_byte, alias.span.end_byte),
        Declaration::Nominal(nominal) => (nominal.span.start_byte, nominal.span.end_byte),
        Declaration::Const(konst) => (konst.span.start_byte, konst.span.end_byte),
        Declaration::Resource(resource) => (resource.span.start_byte, resource.span.end_byte),
        Declaration::Struct(item) => (item.span.start_byte, item.span.end_byte),
        Declaration::Store(store) => (store.span.start_byte, store.span.end_byte),
        Declaration::Enum(item) => (item.span.start_byte, item.span.end_byte),
    };
    start as u32 <= offset && offset <= end as u32
}

fn locate_file<'a>(
    file: &'a QueryFile<'_>,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    let declaration = file
        .declarations
        .iter()
        .find(|declaration| declaration_contains(declaration, offset))?;
    locate_declaration(declaration, offset, scope)
}

fn locate_declaration<'a>(
    declaration: &'a Declaration,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    match declaration {
        Declaration::Function(function) => locate_function(function, offset, scope),
        Declaration::Test(test) => locate_block(&test.body, offset, scope),
        Declaration::Const(konst) => {
            if let Some(ty) = &konst.ty
                && contains(ty.span(), offset)
            {
                return Some(Located::TypeAnnotation);
            }
            konst
                .value
                .as_ref()
                .and_then(|value| locate_expression(value, offset))
        }
        Declaration::Alias(alias) => type_position(alias.ty.as_ref(), offset),
        Declaration::Nominal(nominal) => type_position(nominal.base.as_ref(), offset),
        Declaration::Struct(item) => {
            scope.type_params = item.type_params.iter().map(|p| p.name.clone()).collect();
            members_type_position(&item.members, offset)
        }
        Declaration::Resource(resource) => members_type_position(&resource.members, offset),
        Declaration::Enum(item) => {
            scope.type_params = item.type_params.iter().map(|p| p.name.clone()).collect();
            locate_enum_payload_type(&item.members, offset)
        }
        Declaration::Store(_) => None,
    }
}

fn type_position(ty: Option<&TypeExpr>, offset: u32) -> Option<Located<'static>> {
    match ty {
        Some(ty) if contains(ty.span(), offset) => Some(Located::TypeAnnotation),
        _ => None,
    }
}

fn members_type_position(members: &[ResourceMember], offset: u32) -> Option<Located<'static>> {
    for member in members {
        match member {
            ResourceMember::Field(field) => {
                if contains(field.ty.span(), offset) {
                    return Some(Located::TypeAnnotation);
                }
            }
            ResourceMember::Group(group) => {
                if let Some(located) = members_type_position(&group.members, offset) {
                    return Some(located);
                }
            }
        }
    }
    None
}

fn locate_enum_payload_type(members: &[EnumMember], offset: u32) -> Option<Located<'static>> {
    for member in members {
        for field in &member.payload {
            if contains(field.ty.span(), offset) {
                return Some(Located::TypeAnnotation);
            }
        }
        if let Some(located) = locate_enum_payload_type(&member.members, offset) {
            return Some(located);
        }
    }
    None
}

fn locate_function<'a>(
    function: &'a FunctionDecl,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    scope.type_params = function
        .type_params
        .iter()
        .map(|param| param.name.clone())
        .collect();
    for param in &function.params {
        if contains(param.ty.span(), offset) {
            return Some(Located::TypeAnnotation);
        }
        scope.params.push(Binding {
            name: param.name.clone(),
            ty: Some(&param.ty),
        });
    }
    if let Some(return_type) = &function.return_type
        && contains(return_type.span(), offset)
    {
        return Some(Located::TypeAnnotation);
    }
    locate_block(&function.body, offset, scope)
}

fn locate_block<'a>(block: &'a Block, offset: u32, scope: &mut Scope<'a>) -> Option<Located<'a>> {
    for statement in &block.statements {
        let span = statement.span();
        if contains(span, offset) {
            return locate_statement(statement, offset, scope);
        }
        if ends_before(span, offset)
            && let Some(binding) = following_binding(statement)
        {
            scope.locals.push(binding);
        }
    }
    None
}

/// The binding a statement introduces into the *following* scope (a `const`/`var`
/// declaration and the like). Control-flow statements bind only inside their own
/// blocks and introduce nothing here.
fn following_binding(statement: &Statement) -> Option<Binding<'_>> {
    match statement {
        Statement::Const { name, ty, .. } | Statement::Var { name, ty, .. } => Some(Binding {
            name: name.clone(),
            ty: ty.as_deref(),
        }),
        Statement::EntryBinding { name, .. } => Some(Binding {
            name: name.clone(),
            ty: None,
        }),
        Statement::LetElse { name, ty, .. } => Some(Binding {
            name: name.clone(),
            ty: ty.as_deref(),
        }),
        Statement::Checked { bind, .. } => match bind {
            CheckedBind::Const { name, ty, .. } | CheckedBind::Var { name, ty, .. } => {
                Some(Binding {
                    name: name.clone(),
                    ty: ty.as_deref(),
                })
            }
            CheckedBind::Return => None,
        },
        _ => None,
    }
}

/// The block structure shared by the three `if` forms: the `then` block and the
/// `else if` / `else` tail that follows it.
struct Branches<'a> {
    then_block: &'a Block,
    else_ifs: &'a [ElseIf],
    else_block: Option<&'a Block>,
}

fn locate_statement<'a>(
    statement: &'a Statement,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    match statement {
        Statement::Const { ty, value, .. } => {
            if in_annotation(ty.as_deref(), offset) {
                return Some(Located::TypeAnnotation);
            }
            locate_expression(value, offset)
        }
        Statement::Var { ty, value, .. } => {
            if in_annotation(ty.as_deref(), offset) {
                return Some(Located::TypeAnnotation);
            }
            value
                .as_ref()
                .and_then(|value| locate_expression(value, offset))
        }
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            locate_expression(target, offset).or_else(|| locate_expression(value, offset))
        }
        Statement::Require {
            condition, value, ..
        } => locate_expression(condition, offset).or_else(|| locate_expression(value, offset)),
        Statement::Delete { path: value, .. }
        | Statement::Unset { target: value, .. }
        | Statement::Assert { value, .. }
        | Statement::Expr { value, .. } => locate_expression(value, offset),
        Statement::Return { value, .. } => value
            .as_ref()
            .and_then(|value| locate_expression(value, offset)),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => locate_if(
            condition,
            Branches {
                then_block,
                else_ifs,
                else_block: else_block.as_ref(),
            },
            offset,
            scope,
        ),
        Statement::IfConst {
            name,
            ty,
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => locate_if_const(
            name,
            ty.as_deref(),
            value,
            Branches {
                then_block,
                else_ifs,
                else_block: else_block.as_ref(),
            },
            offset,
            scope,
        ),
        Statement::IfConstChain {
            bindings,
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => locate_if_const_chain(
            bindings,
            condition.as_ref(),
            Branches {
                then_block,
                else_ifs,
                else_block: else_block.as_ref(),
            },
            offset,
            scope,
        ),
        Statement::While {
            condition, body, ..
        } => locate_expression(condition, offset)
            .or_else(|| locate_contained_block(body, offset, scope)),
        Statement::For {
            binding,
            iterable,
            step,
            bound,
            body,
            ..
        } => locate_for(
            binding,
            iterable,
            step.as_ref(),
            bound.as_deref(),
            body,
            offset,
            scope,
        ),
        Statement::Transaction { body, .. } => locate_contained_block(body, offset, scope),
        Statement::Match {
            scrutinee, arms, ..
        } => locate_match(scrutinee, arms, offset, scope),
        Statement::Checked {
            bind,
            op,
            out_of_range,
            zero_divisor,
            ..
        } => locate_checked(
            bind,
            op,
            out_of_range.as_ref(),
            zero_divisor.as_ref(),
            offset,
            scope,
        ),
        Statement::LetElse {
            ty,
            value,
            else_block,
            ..
        } => locate_let_else(ty.as_deref(), value, else_block, offset, scope),
        Statement::EntryBinding {
            address,
            else_block,
            ..
        } => locate_let_else(None, address, else_block, offset, scope),
        Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => None,
    }
}

/// Whether the offset falls inside a written type annotation.
fn in_annotation(ty: Option<&TypeExpr>, offset: u32) -> bool {
    ty.is_some_and(|ty| contains(ty.span(), offset))
}

/// Descend into a block only when it contains the offset.
fn locate_contained_block<'a>(
    block: &'a Block,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    contains(block.span, offset)
        .then(|| locate_block(block, offset, scope))
        .flatten()
}

/// The `else if` / `else` tail of `if` and `if const`: each clause condition, then the
/// clause block that contains the offset.
fn locate_else_chain<'a>(
    branches: &Branches<'a>,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    for else_if in branches.else_ifs {
        if let Some(located) = locate_expression(&else_if.condition, offset) {
            return Some(located);
        }
        if contains(else_if.block.span, offset) {
            return locate_block(&else_if.block, offset, scope);
        }
    }
    branches
        .else_block
        .and_then(|block| locate_contained_block(block, offset, scope))
}

fn locate_if<'a>(
    condition: &'a Expression,
    branches: Branches<'a>,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    if let Some(located) = locate_expression(condition, offset) {
        return Some(located);
    }
    if contains(branches.then_block.span, offset) {
        return locate_block(branches.then_block, offset, scope);
    }
    locate_else_chain(&branches, offset, scope)
}

/// `if const`: the binding is in scope only inside the `then` block, so it is pushed
/// only on the descent into that block.
fn locate_if_const<'a>(
    name: &str,
    ty: Option<&'a TypeExpr>,
    value: &'a Expression,
    branches: Branches<'a>,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    if in_annotation(ty, offset) {
        return Some(Located::TypeAnnotation);
    }
    if let Some(located) = locate_expression(value, offset) {
        return Some(located);
    }
    if contains(branches.then_block.span, offset) {
        scope.locals.push(Binding {
            name: name.to_owned(),
            ty,
        });
        return locate_block(branches.then_block, offset, scope);
    }
    locate_else_chain(&branches, offset, scope)
}

/// The chained `if const` head. Its `else if` conditions are not descended into: the
/// form is parse-only and `marrow-compile` rejects it, so the clause conditions carry
/// no resolvable scope.
fn locate_if_const_chain<'a>(
    bindings: &'a [IfConstBinding],
    condition: Option<&'a Expression>,
    branches: Branches<'a>,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    for binding in bindings {
        if let Some(located) = locate_expression(&binding.value, offset) {
            return Some(located);
        }
    }
    if let Some(condition) = condition
        && let Some(located) = locate_expression(condition, offset)
    {
        return Some(located);
    }
    if contains(branches.then_block.span, offset) {
        for binding in bindings {
            scope.locals.push(Binding {
                name: binding.name.clone(),
                ty: binding.ty.as_ref(),
            });
        }
        return locate_block(branches.then_block, offset, scope);
    }
    for else_if in branches.else_ifs {
        if contains(else_if.block.span, offset) {
            return locate_block(&else_if.block, offset, scope);
        }
    }
    branches
        .else_block
        .and_then(|block| locate_contained_block(block, offset, scope))
}

/// A `for` head and body. The loop names bind only inside the body, so the bounded
/// traversal clause is searched before they are pushed.
fn locate_for<'a>(
    binding: &'a ForBinding,
    iterable: &'a Expression,
    step: Option<&'a Expression>,
    bound: Option<&'a TraversalBound>,
    body: &'a Block,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    if let Some(located) = locate_expression(iterable, offset) {
        return Some(located);
    }
    if let Some(step) = step
        && let Some(located) = locate_expression(step, offset)
    {
        return Some(located);
    }
    if let Some(bound) = bound {
        if let Some(located) = locate_expression(&bound.limit, offset) {
            return Some(located);
        }
        if let Some(from) = &bound.from
            && let Some(located) = locate_expression(from, offset)
        {
            return Some(located);
        }
        if let Some(on_more) = &bound.on_more
            && contains(on_more.span, offset)
        {
            return locate_block(on_more, offset, scope);
        }
    }
    if contains(body.span, offset) {
        for name in &binding.names {
            scope.locals.push(Binding {
                name: name.name.clone(),
                ty: None,
            });
        }
        return locate_block(body, offset, scope);
    }
    None
}

/// A `match`: the selected arm's payload bindings enter scope with its block.
fn locate_match<'a>(
    scrutinee: &'a Expression,
    arms: &'a [MatchArm],
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    if let Some(located) = locate_expression(scrutinee, offset) {
        return Some(located);
    }
    for arm in arms {
        if contains(arm.block.span, offset) {
            for arm_binding in &arm.bindings {
                scope.locals.push(Binding {
                    name: arm_binding.name.clone(),
                    ty: None,
                });
            }
            return locate_block(&arm.block, offset, scope);
        }
    }
    None
}

/// A `checked` form: its binding is not in scope in the operation or the `on` arms.
fn locate_checked<'a>(
    bind: &'a CheckedBind,
    op: &'a Expression,
    out_of_range: Option<&'a Block>,
    zero_divisor: Option<&'a Block>,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    if let CheckedBind::Const { ty: Some(ty), .. } | CheckedBind::Var { ty: Some(ty), .. } = bind
        && contains(ty.span(), offset)
    {
        return Some(Located::TypeAnnotation);
    }
    if let Some(located) = locate_expression(op, offset) {
        return Some(located);
    }
    for block in [out_of_range, zero_divisor].into_iter().flatten() {
        if contains(block.span, offset) {
            return locate_block(block, offset, scope);
        }
    }
    None
}

fn locate_let_else<'a>(
    ty: Option<&'a TypeExpr>,
    value: &'a Expression,
    else_block: &'a Block,
    offset: u32,
    scope: &mut Scope<'a>,
) -> Option<Located<'a>> {
    if in_annotation(ty, offset) {
        return Some(Located::TypeAnnotation);
    }
    if let Some(located) = locate_expression(value, offset) {
        return Some(located);
    }
    locate_contained_block(else_block, offset, scope)
}

/// The immediate expression children to recurse into for the compositional forms. The
/// forms that carry a completion class of their own (`Name`, `Field`, and the recovery
/// nodes) are matched before this helper is reached.
fn expression_children(expression: &Expression) -> Vec<&Expression> {
    match expression {
        Expression::Call { callee, args, .. } => {
            let mut children = vec![callee.as_ref()];
            children.extend(args.iter().map(|argument| &argument.value));
            children
        }
        Expression::Keyed { base, keys, .. } => {
            let mut children = vec![base.as_ref()];
            children.extend(keys.iter());
            children
        }
        Expression::Unary { operand, .. } => vec![operand.as_ref()],
        Expression::Binary { operands, .. } => vec![&operands.left, &operands.right],
        Expression::Membership { value, range, .. } => vec![value.as_ref(), range.as_ref()],
        Expression::Range {
            start, end, step, ..
        } => [start, end, step]
            .into_iter()
            .flatten()
            .map(|boxed| boxed.as_ref())
            .collect(),
        Expression::Interpolation { parts, .. } => parts
            .iter()
            .filter_map(|part| match part {
                marrow_syntax::InterpolationPart::Expr(expression) => Some(expression),
                marrow_syntax::InterpolationPart::Text { .. } => None,
            })
            .collect(),
        Expression::Try { inner, .. } => vec![inner.as_ref()],
        // Leaves carry no sub-expression. The match stays exhaustive so a new
        // child-bearing `Expression` variant is a compile error here, not a silent gap.
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. }
        | Expression::Field { .. }
        | Expression::OptionalField { .. }
        | Expression::Error { .. } => Vec::new(),
    }
}

fn locate_expression<'a>(expression: &'a Expression, offset: u32) -> Option<Located<'a>> {
    if !contains(expression.span(), offset) {
        return None;
    }
    match expression {
        Expression::Error {
            recovery: Some(Recovery::Member { base } | Recovery::OptionalMember { base }),
            ..
        } => {
            return if contains(base.span(), offset) {
                locate_expression(base, offset)
            } else {
                Some(Located::Member(base))
            };
        }
        Expression::Error {
            recovery: Some(Recovery::Path { base }),
            ..
        } => {
            return if contains(base.span(), offset) {
                locate_expression(base, offset)
            } else {
                Some(Located::EnumPath(base))
            };
        }
        Expression::Error { recovery: None, .. } => return None,
        Expression::Name { .. } => return Some(Located::ExprName),
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            return if contains(base.span(), offset) {
                locate_expression(base, offset)
            } else {
                Some(Located::Member(base))
            };
        }
        _ => {}
    }
    for child in expression_children(expression) {
        if let Some(located) = locate_expression(child, offset) {
            return Some(located);
        }
    }
    None
}

fn expression_name_candidates(file: &QueryFile<'_>, scope: &Scope<'_>) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for local in &scope.locals {
        candidates.push(Candidate {
            label: local.name.clone(),
            kind: CandidateKind::Local,
            detail: local.ty.map(TypeExpr::to_string).unwrap_or_default(),
        });
    }
    for param in &scope.params {
        candidates.push(Candidate {
            label: param.name.clone(),
            kind: CandidateKind::Param,
            detail: param.ty.map(TypeExpr::to_string).unwrap_or_default(),
        });
    }
    for declaration in file.declarations {
        match declaration {
            Declaration::Function(function) => candidates.push(Candidate {
                label: function.name.clone(),
                kind: CandidateKind::Function,
                detail: function_signature(function),
            }),
            Declaration::Const(konst) => candidates.push(Candidate {
                label: konst.name.clone(),
                kind: CandidateKind::Const,
                detail: konst
                    .ty
                    .as_ref()
                    .map(TypeExpr::to_string)
                    .unwrap_or_default(),
            }),
            Declaration::Enum(item) => candidates.push(Candidate {
                label: item.name.clone(),
                kind: CandidateKind::Type,
                detail: String::new(),
            }),
            _ => {}
        }
    }
    for name in builtin_value_names() {
        candidates.push(Candidate {
            label: (*name).to_string(),
            kind: CandidateKind::Builtin,
            detail: String::new(),
        });
    }
    for use_decl in file.uses {
        let Some(segment) = use_decl.segments.last() else {
            continue;
        };
        candidates.push(Candidate {
            label: segment.text().to_string(),
            kind: CandidateKind::Module,
            detail: String::new(),
        });
    }
    candidates
}

fn member_candidates(file: &QueryFile<'_>, scope: &Scope<'_>, base: &Expression) -> Vec<Candidate> {
    let Some(type_name) = base_type_name(scope, base) else {
        return Vec::new();
    };
    let Some(item) = file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Struct(item) if item.name == type_name => Some(item),
            _ => None,
        })
    else {
        return Vec::new();
    };
    struct_field_candidates(&item.members)
}

fn struct_field_candidates(members: &[ResourceMember]) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for member in members {
        if let ResourceMember::Field(field) = member {
            candidates.push(Candidate {
                label: field.name.clone(),
                kind: CandidateKind::Field,
                detail: field.ty.to_string(),
            });
        }
    }
    candidates
}

/// The fail-soft type probe: the struct-type name of a single-segment base that resolves
/// to a local or parameter annotated with a bare struct name (a single-segment
/// [`TypeExpr::Name`]). Any partial, unannotated, generic, optional, identity, or
/// otherwise non-bare annotation yields `None` — never a resolver failure. The name is
/// read from the type node structurally, not from a rendered display string.
fn base_type_name<'a>(scope: &Scope<'a>, base: &Expression) -> Option<&'a str> {
    let Expression::Name { segments, .. } = base else {
        return None;
    };
    let [name] = &segments[..] else {
        return None;
    };
    let binding = scope
        .locals
        .iter()
        .rev()
        .chain(scope.params.iter())
        .find(|binding| binding.name == name.text())?;
    match binding.ty? {
        TypeExpr::Name { text, .. } => Some(text.as_str()),
        _ => None,
    }
}

fn enum_path_candidates(file: &QueryFile<'_>, base: &Expression) -> Vec<Candidate> {
    let Expression::Name { segments, .. } = base else {
        return Vec::new();
    };
    let Some((enum_name, rest)) = segments.split_first() else {
        return Vec::new();
    };
    let Some(item) = file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Enum(item) if item.name == enum_name.text() => Some(item),
            _ => None,
        })
    else {
        return Vec::new();
    };
    match resolve_enum_members(item, rest) {
        Some(members) => members
            .iter()
            .map(|member| Candidate {
                label: member.name.clone(),
                kind: CandidateKind::EnumMember {
                    selectable: !member.category,
                },
                detail: String::new(),
            })
            .collect(),
        None => Vec::new(),
    }
}

/// Walk the qualified segments after the enum name into the member tree, returning the
/// reached node's immediate members. An unresolvable segment yields `None`.
fn resolve_enum_members<'a>(item: &'a EnumDecl, rest: &[NameSegment]) -> Option<&'a [EnumMember]> {
    let mut members = item.members.as_slice();
    for segment in rest {
        let member = members
            .iter()
            .find(|member| member.name == segment.text())?;
        members = member.members.as_slice();
    }
    Some(members)
}

fn type_annotation_candidates(file: &QueryFile<'_>, scope: &Scope<'_>) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for declaration in file.declarations {
        let name = match declaration {
            Declaration::Alias(item) => &item.name,
            Declaration::Nominal(item) => &item.name,
            Declaration::Struct(item) => &item.name,
            Declaration::Enum(item) => &item.name,
            Declaration::Resource(item) => &item.name,
            _ => continue,
        };
        candidates.push(Candidate {
            label: name.clone(),
            kind: CandidateKind::Type,
            detail: String::new(),
        });
    }
    for name in builtin_type_names() {
        candidates.push(Candidate {
            label: name.to_string(),
            kind: CandidateKind::Type,
            detail: String::new(),
        });
    }
    for type_param in &scope.type_params {
        candidates.push(Candidate {
            label: type_param.clone(),
            kind: CandidateKind::TypeParam,
            detail: String::new(),
        });
    }
    candidates
}

/// The built-in type-name namespace: the language scalar spellings (routed through the
/// scalar owner), the reserved toolchain generics (routed through their type-system
/// owner so the completion set cannot drift from the redeclaration gate), and the `Id`
/// identity-type keyword.
fn builtin_type_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = [
        ScalarType::Int,
        ScalarType::Bool,
        ScalarType::Text,
        ScalarType::Bytes,
        ScalarType::Date,
        ScalarType::Instant,
        ScalarType::Duration,
    ]
    .into_iter()
    .map(ScalarType::spelling)
    .collect();
    names.extend(crate::types::RESERVED_GENERIC_TYPE_NAMES);
    names.push("Id");
    names
}

fn function_signature(function: &FunctionDecl) -> String {
    let mut signature = String::from("(");
    for (index, param) in function.params.iter().enumerate() {
        if index > 0 {
            signature.push_str(", ");
        }
        signature.push_str(&param.name);
        signature.push_str(": ");
        signature.push_str(&param.ty.to_string());
    }
    signature.push(')');
    if let Some(return_type) = &function.return_type {
        signature.push_str(": ");
        signature.push_str(&return_type.to_string());
    }
    signature
}
