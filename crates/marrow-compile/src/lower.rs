//! Function-body lowering (design §B/§D).
//!
//! [`FnLowerer`] type-checks the compiled subset and lowers one function body to
//! a draft instruction stream. Locals are allocated one fresh slot per `const`/
//! `var`/param/`if const` binding — slots are never reused — so every read is
//! dominated by the slot's single write and the independent verifier's
//! definite-init dataflow is satisfied. Jumps are emitted with placeholder targets
//! and patched to instruction indices once the target position is known; the
//! encoder rewrites indices to byte offsets.

use marrow_codes::Code;
use marrow_image::{FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry, TypeId};
use marrow_syntax::{
    Argument, BinaryOp, Block, ElseIf, Expression, FunctionDecl, LiteralKind, SourceSpan,
    Statement, TypeExpr, UnaryOp, decode_string_literal,
};

use crate::diag::SourceDiagnostic;
use crate::record::RecordRegistry;
use crate::scalar::ScalarType;

/// A lowered value type: a scalar or the project record, each bare or optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LTy {
    Scalar { scalar: ScalarType, optional: bool },
    Record { ty: TypeId, optional: bool },
}

impl LTy {
    fn bare_scalar(scalar: ScalarType) -> Self {
        LTy::Scalar {
            scalar,
            optional: false,
        }
    }

    fn is_optional(self) -> bool {
        match self {
            LTy::Scalar { optional, .. } | LTy::Record { optional, .. } => optional,
        }
    }

    fn to_optional(self) -> Self {
        match self {
            LTy::Scalar { scalar, .. } => LTy::Scalar {
                scalar,
                optional: true,
            },
            LTy::Record { ty, .. } => LTy::Record { ty, optional: true },
        }
    }

    fn to_bare(self) -> Self {
        match self {
            LTy::Scalar { scalar, .. } => LTy::bare_scalar(scalar),
            LTy::Record { ty, .. } => LTy::Record {
                ty,
                optional: false,
            },
        }
    }

    fn bare_scalar_type(self) -> Option<ScalarType> {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => Some(scalar),
            _ => None,
        }
    }

    fn spelling(self) -> String {
        let (base, optional) = match self {
            LTy::Scalar { scalar, optional } => (scalar.spelling().to_string(), optional),
            LTy::Record { optional, .. } => ("record".to_string(), optional),
        };
        if optional { format!("{base}?") } else { base }
    }

    fn image(self) -> ImageType {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => ImageType::scalar(scalar.image()),
            LTy::Scalar {
                scalar,
                optional: true,
            } => ImageType::opt_scalar(scalar.image()),
            LTy::Record { ty, optional } => ImageType::Record {
                idx: ty.index(),
                optional,
            },
        }
    }
}

/// Whether control continues past a statement or block, or leaves it (via `return`,
/// `break`, or `continue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Fallthrough,
    Terminates,
}

/// The declared return shape of the function being lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetType {
    Unit,
    Value(LTy),
}

/// One in-scope local binding.
struct Local {
    name: String,
    ty: LTy,
    mutable: bool,
    slot: u16,
}

/// A loop's patch targets: where `continue` jumps, and the jumps `break` emits that
/// must be patched to the loop's exit once it is known.
struct LoopCtx {
    continue_target: usize,
    break_jumps: Vec<usize>,
}

pub(crate) struct FnLowerer<'a> {
    draft: &'a mut ImageDraft,
    records: &'a RecordRegistry,
    diagnostics: &'a mut Vec<SourceDiagnostic>,
    file: &'a str,
    code: Vec<Instr>,
    spans: Vec<SpanEntry>,
    locals: Vec<Local>,
    loops: Vec<LoopCtx>,
    /// Monotonic slot allocator; never decreases, so slots are never reused.
    slot_count: u16,
    ret: RetType,
    failed: bool,
}

impl<'a> FnLowerer<'a> {
    /// Lower `function` and add it (and its export, when public) to the draft.
    pub(crate) fn lower(
        draft: &'a mut ImageDraft,
        records: &'a RecordRegistry,
        diagnostics: &'a mut Vec<SourceDiagnostic>,
        file: &'a str,
        function: &FunctionDecl,
    ) {
        let ret = match &function.return_type {
            None => RetType::Unit,
            Some(annotation) => match resolve_type(records, annotation) {
                Some(LTy::Record { .. }) => {
                    diagnostics.push(unsupported(file, annotation.span(), "a record return type"));
                    return;
                }
                Some(ty) => RetType::Value(ty),
                None => {
                    diagnostics.push(unsupported(file, annotation.span(), "this return type"));
                    return;
                }
            },
        };

        let mut lowerer = FnLowerer {
            draft,
            records,
            diagnostics,
            file,
            code: Vec::new(),
            spans: Vec::new(),
            locals: Vec::new(),
            loops: Vec::new(),
            slot_count: 0,
            ret,
            failed: false,
        };

        // Params occupy the first slots as bare scalars (design §C: params are bare
        // scalar type refs), pre-initialized to their type.
        for param in &function.params {
            if !param.keys.is_empty() {
                lowerer.fail(unsupported(file, function.span, "a keyed parameter"));
            }
            let Some(scalar) = lowerer.param_scalar(&param.ty) else {
                continue;
            };
            let slot = lowerer.alloc_slot();
            lowerer.locals.push(Local {
                name: param.name.clone(),
                ty: LTy::bare_scalar(scalar),
                mutable: false,
                slot,
            });
        }

        let body_flow = lowerer.lower_block(&function.body);
        match (body_flow, lowerer.ret) {
            (Flow::Terminates, _) => {}
            (Flow::Fallthrough, RetType::Unit) => {
                lowerer.push(Instr::Return, function.body.span);
            }
            (Flow::Fallthrough, RetType::Value(_)) => {
                lowerer.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    function.span,
                    "not all paths return a value".to_string(),
                ));
            }
        }

        if lowerer.failed {
            return;
        }

        let params: Vec<Scalar> = function
            .params
            .iter()
            .zip(&lowerer.locals)
            .map(|(_, local)| match local.ty {
                LTy::Scalar { scalar, .. } => scalar.image(),
                LTy::Record { .. } => unreachable!("params are scalars"),
            })
            .collect();
        let ret_ref = match ret {
            RetType::Unit => ImageType::Unit,
            RetType::Value(ty) => ty.image(),
        };
        let name_id = lowerer.draft.intern_string(&function.name);
        let source_id = lowerer.draft.intern_string(file);
        let code = std::mem::take(&mut lowerer.code);
        let spans = std::mem::take(&mut lowerer.spans);
        let func_id = lowerer.draft.add_function(FunctionDef {
            name: name_id,
            source: source_id,
            params,
            ret: ret_ref,
            local_count: lowerer.slot_count,
            code,
            spans,
        });

        if function.public {
            let export_name = lowerer.draft.intern_string(&function.name);
            lowerer.draft.add_export(export_name, func_id);
        }
    }

    // --- emission helpers ---

    fn here(&self) -> usize {
        self.code.len()
    }

    fn push(&mut self, instr: Instr, span: SourceSpan) {
        let index = self.code.len() as u32;
        self.code.push(instr);
        self.spans.push(SpanEntry {
            instr_index: index,
            line: span.line.max(1),
            column: span.column.max(1),
        });
    }

    fn push_jump(&mut self, span: SourceSpan) -> usize {
        let at = self.here();
        self.push(Instr::Jump(0), span);
        at
    }

    fn push_jif(&mut self, span: SourceSpan) -> usize {
        let at = self.here();
        self.push(Instr::JumpIfFalse(0), span);
        at
    }

    fn push_branch_present(&mut self, span: SourceSpan) -> usize {
        let at = self.here();
        self.push(Instr::BranchPresent(0), span);
        at
    }

    fn patch(&mut self, at: usize, target: usize) {
        match &mut self.code[at] {
            Instr::Jump(t) | Instr::JumpIfFalse(t) | Instr::BranchPresent(t) => *t = target as u32,
            other => unreachable!("patch target is not a jump: {other:?}"),
        }
    }

    fn patch_all(&mut self, jumps: Vec<usize>, target: usize) {
        for jump in jumps {
            self.patch(jump, target);
        }
    }

    fn alloc_slot(&mut self) -> u16 {
        let slot = self.slot_count;
        self.slot_count += 1;
        slot
    }

    fn fail(&mut self, diagnostic: SourceDiagnostic) {
        self.diagnostics.push(diagnostic);
        self.failed = true;
    }

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.locals.iter().rev().find(|local| local.name == name)
    }

    // --- statements ---

    fn lower_block(&mut self, block: &Block) -> Flow {
        let mark = self.locals.len();
        let mut flow = Flow::Fallthrough;
        for statement in &block.statements {
            if flow == Flow::Terminates {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    statement.span(),
                    "this statement is unreachable".to_string(),
                ));
                break;
            }
            flow = self.lower_statement(statement);
        }
        self.locals.truncate(mark);
        flow
    }

    fn lower_statement(&mut self, statement: &Statement) -> Flow {
        match statement {
            Statement::Const {
                name, ty, value, ..
            } => {
                self.lower_binding(name, ty.as_ref(), value, false);
                Flow::Fallthrough
            }
            Statement::Var {
                name,
                keys,
                ty,
                value,
                span,
            } => {
                if !keys.is_empty() {
                    self.fail(unsupported(self.file, *span, "a keyed local"));
                    return Flow::Fallthrough;
                }
                let Some(value) = value else {
                    self.fail(unsupported(
                        self.file,
                        *span,
                        "a `var` without an initializer",
                    ));
                    return Flow::Fallthrough;
                };
                self.lower_binding(name, ty.as_ref(), value, true);
                Flow::Fallthrough
            }
            Statement::Assign { target, value, .. } => {
                self.lower_assign(target, value);
                Flow::Fallthrough
            }
            Statement::CompoundAssign {
                target, op, value, ..
            } => {
                self.lower_compound_assign(target, op.binary(), value);
                Flow::Fallthrough
            }
            Statement::Return { value, span } => self.lower_return(value.as_ref(), *span),
            Statement::Break { span } => self.lower_break(*span),
            Statement::Continue { span } => self.lower_continue(*span),
            Statement::Expr { value, .. } => {
                if self.lower_expr(value).is_some() {
                    self.push(Instr::Pop, value.span());
                }
                Flow::Fallthrough
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                let mut branches: Vec<(&Expression, &Block)> = vec![(condition, then_block)];
                for else_if in else_ifs {
                    branches.push((&else_if.condition, &else_if.block));
                }
                self.lower_cond_chain(&branches, else_block.as_ref())
            }
            Statement::IfConst {
                name,
                ty,
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => self.lower_if_const(
                name,
                ty.as_ref(),
                value,
                then_block,
                else_ifs,
                else_block.as_ref(),
            ),
            Statement::While {
                condition, body, ..
            } => self.lower_while(condition, body),
            other => {
                self.fail(unsupported(self.file, other.span(), "this statement"));
                Flow::Fallthrough
            }
        }
    }

    fn lower_binding(
        &mut self,
        name: &str,
        annotation: Option<&TypeExpr>,
        value: &Expression,
        mutable: bool,
    ) {
        let ty = if let Some(annotation) = annotation {
            let Some(expected) = self.resolve(annotation) else {
                self.fail(unsupported(
                    self.file,
                    annotation.span(),
                    "this type annotation",
                ));
                return;
            };
            if self.lower_as(value, expected).is_none() {
                return;
            }
            expected
        } else {
            let Some(ty) = self.lower_expr(value) else {
                return;
            };
            ty
        };
        let slot = self.alloc_slot();
        self.push(Instr::LocalSet(slot), value.span());
        self.locals.push(Local {
            name: name.to_string(),
            ty,
            mutable,
            slot,
        });
    }

    fn lower_assign(&mut self, target: &Expression, value: &Expression) {
        let Some((slot, ty, mutable, span, name)) = self.resolve_place(target) else {
            return;
        };
        if !mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` is a `const` and cannot be reassigned"),
            ));
            return;
        }
        if self.lower_as(value, ty).is_none() {
            return;
        }
        self.push(Instr::LocalSet(slot), value.span());
    }

    fn lower_compound_assign(&mut self, target: &Expression, op: BinaryOp, value: &Expression) {
        let Some((slot, ty, mutable, span, name)) = self.resolve_place(target) else {
            return;
        };
        if !mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` is a `const` and cannot be reassigned"),
            ));
            return;
        }
        let Some(scalar) = ty.bare_scalar_type() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("cannot apply a compound assignment to {}", ty.spelling()),
            ));
            return;
        };
        self.push(Instr::LocalGet(slot), span);
        let Some(result) = self.lower_binary_op(op, LTy::bare_scalar(scalar), value) else {
            return;
        };
        if result != ty {
            self.fail(type_mismatch(self.file, value.span(), result, ty));
            return;
        }
        self.push(Instr::LocalSet(slot), value.span());
    }

    /// Resolve an assignment target to a mutable-checked local place.
    fn resolve_place(
        &mut self,
        target: &Expression,
    ) -> Option<(u16, LTy, bool, SourceSpan, String)> {
        let Expression::Name { segments, span, .. } = target else {
            self.fail(unsupported(
                self.file,
                target.span(),
                "this assignment target",
            ));
            return None;
        };
        let [name] = segments.as_slice() else {
            self.fail(unsupported(self.file, *span, "this assignment target"));
            return None;
        };
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *span, name));
            return None;
        };
        Some((local.slot, local.ty, local.mutable, *span, name.clone()))
    }

    fn lower_return(&mut self, value: Option<&Expression>, span: SourceSpan) -> Flow {
        match (value, self.ret) {
            (None, RetType::Unit) => {
                self.push(Instr::Return, span);
            }
            (None, RetType::Value(_)) => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    "this function must return a value".to_string(),
                ));
            }
            (Some(expr), RetType::Unit) => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    expr.span(),
                    "this function returns nothing".to_string(),
                ));
            }
            (Some(expr), RetType::Value(want)) => {
                if self.lower_as(expr, want).is_some() {
                    self.push(Instr::Return, span);
                }
            }
        }
        Flow::Terminates
    }

    fn lower_break(&mut self, span: SourceSpan) -> Flow {
        if self.loops.is_empty() {
            self.fail(loop_error(self.file, span, "break"));
            return Flow::Terminates;
        }
        let at = self.push_jump(span);
        self.loops
            .last_mut()
            .expect("loop present")
            .break_jumps
            .push(at);
        Flow::Terminates
    }

    fn lower_continue(&mut self, span: SourceSpan) -> Flow {
        let Some(ctx) = self.loops.last() else {
            self.fail(loop_error(self.file, span, "continue"));
            return Flow::Terminates;
        };
        let target = ctx.continue_target;
        self.push(Instr::Jump(target as u32), span);
        Flow::Terminates
    }

    /// Lower a chain of conditional branches followed by an optional `else`. Used for
    /// `if`/`else if`/`else` and for the absent tail of `if const`.
    fn lower_cond_chain(
        &mut self,
        branches: &[(&Expression, &Block)],
        else_block: Option<&Block>,
    ) -> Flow {
        let mut end_jumps: Vec<usize> = Vec::new();
        let mut all_terminate = else_block.is_some();

        for (cond, block) in branches {
            if self.lower_condition(cond).is_none() {
                return Flow::Fallthrough;
            }
            let jif = self.push_jif(cond.span());
            let flow = self.lower_block(block);
            all_terminate &= flow == Flow::Terminates;
            if flow == Flow::Fallthrough {
                end_jumps.push(self.push_jump(block.span));
            }
            let next = self.here();
            self.patch(jif, next);
        }

        if let Some(else_block) = else_block {
            let flow = self.lower_block(else_block);
            all_terminate &= flow == Flow::Terminates;
        }

        let end = self.here();
        self.patch_all(end_jumps, end);
        if all_terminate {
            Flow::Terminates
        } else {
            Flow::Fallthrough
        }
    }

    fn lower_if_const(
        &mut self,
        name: &str,
        annotation: Option<&TypeExpr>,
        value: &Expression,
        then_block: &Block,
        else_ifs: &[ElseIf],
        else_block: Option<&Block>,
    ) -> Flow {
        let Some(optional) = self.lower_expr(value) else {
            return Flow::Fallthrough;
        };
        if !optional.is_optional() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                value.span(),
                format!(
                    "`if const` needs an optional, found {}",
                    optional.spelling()
                ),
            ));
            return Flow::Fallthrough;
        }
        let bound = optional.to_bare();
        if let Some(annotation) = annotation
            && let Some(declared) = self.resolve(annotation)
            && declared != bound
        {
            self.fail(type_mismatch(self.file, annotation.span(), bound, declared));
            return Flow::Fallthrough;
        }

        // Present edge falls through with the unwrapped bare value; absent edge jumps.
        let bp = self.push_branch_present(value.span());
        let mark = self.locals.len();
        let slot = self.alloc_slot();
        self.push(Instr::LocalSet(slot), value.span());
        self.locals.push(Local {
            name: name.to_string(),
            ty: bound,
            mutable: false,
            slot,
        });
        let then_flow = self.lower_block(then_block);
        self.locals.truncate(mark);

        let mut end_jumps = Vec::new();
        if then_flow == Flow::Fallthrough {
            end_jumps.push(self.push_jump(then_block.span));
        }

        // Absent tail: the `else if`/`else` chain.
        let absent = self.here();
        self.patch(bp, absent);
        let branches: Vec<(&Expression, &Block)> = else_ifs
            .iter()
            .map(|else_if| (&else_if.condition, &else_if.block))
            .collect();
        let else_flow = self.lower_cond_chain(&branches, else_block);

        let end = self.here();
        self.patch_all(end_jumps, end);

        if then_flow == Flow::Terminates && else_flow == Flow::Terminates {
            Flow::Terminates
        } else {
            Flow::Fallthrough
        }
    }

    fn lower_while(&mut self, condition: &Expression, body: &Block) -> Flow {
        let top = self.here();
        if self.lower_condition(condition).is_none() {
            return Flow::Fallthrough;
        }
        let exit = self.push_jif(condition.span());
        self.loops.push(LoopCtx {
            continue_target: top,
            break_jumps: Vec::new(),
        });
        let body_flow = self.lower_block(body);
        if body_flow == Flow::Fallthrough {
            self.push(Instr::Jump(top as u32), body.span);
        }
        let ctx = self.loops.pop().expect("loop was pushed");
        let end = self.here();
        self.patch(exit, end);
        self.patch_all(ctx.break_jumps, end);
        Flow::Fallthrough
    }

    fn lower_condition(&mut self, expr: &Expression) -> Option<()> {
        let ty = self.lower_expr(expr)?;
        if ty != LTy::bare_scalar(ScalarType::Bool) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                expr.span(),
                format!("condition must be bool, found {}", ty.spelling()),
            ));
            return None;
        }
        Some(())
    }

    // --- expressions ---

    /// Lower `expr`, emitting code that pushes its value, then coerce that value to
    /// exactly `expected` (bare-to-optional via `SomeWrap`; `absent` becomes a vacant
    /// optional). Reports a diagnostic and returns `None` on mismatch.
    fn lower_as(&mut self, expr: &Expression, expected: LTy) -> Option<()> {
        if let Expression::Absent { span } = expr {
            let LTy::Scalar {
                scalar,
                optional: true,
            } = expected
            else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *span,
                    format!(
                        "`absent` needs an optional type, found {}",
                        expected.spelling()
                    ),
                ));
                return None;
            };
            self.push(
                Instr::VacantLoad(ImageType::opt_scalar(scalar.image())),
                *span,
            );
            return Some(());
        }
        let got = self.lower_expr(expr)?;
        if got == expected {
            return Some(());
        }
        if !got.is_optional() && expected.is_optional() && got.to_optional() == expected {
            self.push(Instr::SomeWrap, expr.span());
            return Some(());
        }
        self.fail(type_mismatch(self.file, expr.span(), got, expected));
        None
    }

    /// Lower `expr`, emitting code that pushes its value and returning its type.
    fn lower_expr(&mut self, expr: &Expression) -> Option<LTy> {
        match expr {
            Expression::Literal { kind, text, span } => self.lower_literal(*kind, text, *span),
            Expression::Name { segments, span, .. } => {
                let [name] = segments.as_slice() else {
                    self.fail(unsupported(self.file, *span, "a qualified name"));
                    return None;
                };
                let Some(local) = self.lookup(name) else {
                    self.fail(name_error(self.file, *span, name));
                    return None;
                };
                let (slot, ty) = (local.slot, local.ty);
                self.push(Instr::LocalGet(slot), *span);
                Some(ty)
            }
            Expression::Absent { span } => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *span,
                    "the type of `absent` cannot be inferred here".to_string(),
                ));
                None
            }
            Expression::Unary { op, operand, span } => self.lower_unary(*op, operand, *span),
            Expression::Binary {
                op, left, right, ..
            } => self.lower_binary(*op, left, right),
            Expression::Call {
                callee, args, span, ..
            } => self.lower_call(callee, args, *span),
            Expression::Field {
                base, name, span, ..
            } => self.lower_field(base, name, *span),
            other => {
                self.fail(unsupported(self.file, other.span(), "this expression"));
                None
            }
        }
    }

    fn lower_literal(&mut self, kind: LiteralKind, text: &str, span: SourceSpan) -> Option<LTy> {
        let (scalar, const_id) = match kind {
            LiteralKind::Integer => {
                let Some(value) = parse_int(text) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "integer literal is out of the 64-bit range".to_string(),
                    ));
                    return None;
                };
                (ScalarType::Int, self.draft.intern_int(value))
            }
            LiteralKind::Bool => (ScalarType::Bool, self.draft.intern_bool(text == "true")),
            LiteralKind::String => {
                let Ok(decoded) = decode_string_literal(text) else {
                    self.fail(unsupported(self.file, span, "this string literal"));
                    return None;
                };
                (ScalarType::Text, self.draft.intern_text(&decoded))
            }
            _ => {
                self.fail(unsupported(self.file, span, "this literal"));
                return None;
            }
        };
        self.push(Instr::ConstLoad(const_id.index()), span);
        Some(LTy::bare_scalar(scalar))
    }

    fn lower_unary(&mut self, op: UnaryOp, operand: &Expression, span: SourceSpan) -> Option<LTy> {
        let ty = self.lower_expr(operand)?;
        match op {
            UnaryOp::Neg => {
                if ty != LTy::bare_scalar(ScalarType::Int) {
                    self.fail(unary_error(self.file, span, "negate", ty));
                    return None;
                }
                self.push(Instr::IntNeg, span);
                Some(LTy::bare_scalar(ScalarType::Int))
            }
            UnaryOp::Not => {
                if ty != LTy::bare_scalar(ScalarType::Bool) {
                    self.fail(unary_error(self.file, span, "apply `not` to", ty));
                    return None;
                }
                self.push(Instr::BoolNot, span);
                Some(LTy::bare_scalar(ScalarType::Bool))
            }
        }
    }

    fn lower_binary(&mut self, op: BinaryOp, left: &Expression, right: &Expression) -> Option<LTy> {
        match op {
            BinaryOp::And | BinaryOp::Or => self.lower_short_circuit(op, left, right),
            BinaryOp::Coalesce => self.lower_coalesce(left, right),
            _ => {
                let left_ty = self.lower_expr(left)?;
                self.lower_binary_op(op, left_ty, right)
            }
        }
    }

    /// Lower the right operand and the arithmetic/comparison operator, given the left
    /// operand's already-pushed type. Both operands must be bare scalars.
    fn lower_binary_op(&mut self, op: BinaryOp, left_ty: LTy, right: &Expression) -> Option<LTy> {
        let right_ty = self.lower_expr(right)?;
        let span = right.span();
        let (Some(left), Some(right_scalar)) =
            (left_ty.bare_scalar_type(), right_ty.bare_scalar_type())
        else {
            self.fail(binary_error(self.file, span, op, left_ty, right_ty));
            return None;
        };
        use ScalarType::{Bool, Int, Text};
        let (instr, result): (Instr, ScalarType) = match (op, left, right_scalar) {
            (BinaryOp::Add, Int, Int) => (Instr::IntAdd, Int),
            (BinaryOp::Add, Text, Text) => (Instr::TextConcat, Text),
            (BinaryOp::Subtract, Int, Int) => (Instr::IntSub, Int),
            (BinaryOp::Multiply, Int, Int) => (Instr::IntMul, Int),
            (BinaryOp::Remainder, Int, Int) => (Instr::IntRem, Int),
            (BinaryOp::Divide, _, _) => {
                self.fail(unsupported(self.file, span, "integer division `/`"));
                return None;
            }
            (BinaryOp::Less, Int, Int) => (Instr::IntLt, Bool),
            (BinaryOp::LessEqual, Int, Int) => (Instr::IntLe, Bool),
            (BinaryOp::Greater, Int, Int) => (Instr::IntGt, Bool),
            (BinaryOp::GreaterEqual, Int, Int) => (Instr::IntGe, Bool),
            (BinaryOp::Equal, a, b) if a == b => (eq_instr(a), Bool),
            (BinaryOp::NotEqual, a, b) if a == b => {
                self.push(eq_instr(a), span);
                self.push(Instr::BoolNot, span);
                return Some(LTy::bare_scalar(Bool));
            }
            _ => {
                self.fail(binary_error(self.file, span, op, left_ty, right_ty));
                return None;
            }
        };
        self.push(instr, span);
        Some(LTy::bare_scalar(result))
    }

    /// `left ?? right`: yield the present value of the optional `left`, else `right`.
    /// Lowered to the atomic present-branch (design §D), so no unchecked unwrap.
    fn lower_coalesce(&mut self, left: &Expression, right: &Expression) -> Option<LTy> {
        let left_ty = self.lower_expr(left)?;
        if !left_ty.is_optional() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                left.span(),
                format!(
                    "`??` needs an optional on the left, found {}",
                    left_ty.spelling()
                ),
            ));
            return None;
        }
        let bare = left_ty.to_bare();
        let bp = self.push_branch_present(left.span());
        let to_end = self.push_jump(left.span());
        let absent = self.here();
        self.patch(bp, absent);
        self.lower_as(right, bare)?;
        let end = self.here();
        self.patch(to_end, end);
        Some(bare)
    }

    fn lower_short_circuit(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let left_ty = self.lower_expr(left)?;
        if left_ty != bool_ty {
            self.fail(logic_operand(self.file, left.span(), op, left_ty));
            return None;
        }
        match op {
            BinaryOp::And => {
                let jif = self.push_jif(left.span());
                let right_ty = self.lower_expr(right)?;
                if right_ty != bool_ty {
                    self.fail(logic_operand(self.file, right.span(), op, right_ty));
                    return None;
                }
                let to_end = self.push_jump(right.span());
                let false_at = self.here();
                self.patch(jif, false_at);
                let konst = self.draft.intern_bool(false);
                self.push(Instr::ConstLoad(konst.index()), left.span());
                let end = self.here();
                self.patch(to_end, end);
            }
            BinaryOp::Or => {
                let jif = self.push_jif(left.span());
                let konst = self.draft.intern_bool(true);
                self.push(Instr::ConstLoad(konst.index()), left.span());
                let to_end = self.push_jump(left.span());
                let rhs_at = self.here();
                self.patch(jif, rhs_at);
                let right_ty = self.lower_expr(right)?;
                if right_ty != bool_ty {
                    self.fail(logic_operand(self.file, right.span(), op, right_ty));
                    return None;
                }
                let end = self.here();
                self.patch(to_end, end);
            }
            _ => unreachable!("only and/or reach short-circuit lowering"),
        }
        Some(bool_ty)
    }

    /// A parenthesized application at this slice is a record constructor
    /// (`Note(title: t, ...)`); function calls land with the call slice.
    fn lower_call(
        &mut self,
        callee: &Expression,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let Expression::Name { segments, .. } = callee else {
            self.fail(unsupported(self.file, span, "this call"));
            return None;
        };
        let [name] = segments.as_slice() else {
            self.fail(unsupported(self.file, span, "this call"));
            return None;
        };
        let Some(record) = self.records.by_name(name) else {
            self.fail(unsupported(self.file, span, "a function call"));
            return None;
        };
        let type_id = record.type_id;

        // Resolve each named argument against a field before emitting, so evaluation
        // order is the field declaration order (f0 pushed first).
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "constructor arguments must be named".to_string(),
                ));
                return None;
            };
            if self.records.by_name(name)?.field(arg_name).is_none() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{name}` has no field `{arg_name}`"),
                ));
                return None;
            }
        }

        let field_plan: Vec<(String, ScalarType, bool)> = self
            .records
            .by_name(name)?
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.scalar, field.required))
            .collect();
        for (field_name, scalar, required) in field_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()));
            let expected = if required {
                LTy::bare_scalar(scalar)
            } else {
                LTy::bare_scalar(scalar).to_optional()
            };
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, expected)?;
                }
                None if required => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing required field `{field_name}`"),
                    ));
                    return None;
                }
                None => {
                    self.push(
                        Instr::VacantLoad(ImageType::opt_scalar(scalar.image())),
                        span,
                    );
                }
            }
        }
        self.push(Instr::RecordNew(type_id.index()), span);
        Some(LTy::Record {
            ty: type_id,
            optional: false,
        })
    }

    fn lower_field(&mut self, base: &Expression, name: &str, span: SourceSpan) -> Option<LTy> {
        let base_ty = self.lower_expr(base)?;
        let LTy::Record {
            ty,
            optional: false,
        } = base_ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                base.span(),
                format!("field access needs a record, found {}", base_ty.spelling()),
            ));
            return None;
        };
        let Some(record) = self.records.by_name_for_type(ty) else {
            self.fail(unsupported(self.file, span, "this field access"));
            return None;
        };
        let Some((index, field)) = record.field(name) else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("record has no field `{name}`"),
            ));
            return None;
        };
        let (scalar, required) = (field.scalar, field.required);
        self.push(Instr::FieldGet(index), span);
        Some(if required {
            LTy::bare_scalar(scalar)
        } else {
            LTy::bare_scalar(scalar).to_optional()
        })
    }

    // --- type resolution ---

    fn resolve(&self, annotation: &TypeExpr) -> Option<LTy> {
        resolve_type(self.records, annotation)
    }

    fn param_scalar(&mut self, ty: &TypeExpr) -> Option<ScalarType> {
        match ty {
            TypeExpr::Name { text, span } => match ScalarType::from_spelling(text) {
                Some(scalar) => Some(scalar),
                None => {
                    self.fail(unsupported(self.file, *span, "this parameter type"));
                    None
                }
            },
            other => {
                self.fail(unsupported(self.file, other.span(), "this parameter type"));
                None
            }
        }
    }
}

/// Resolve a type annotation into a lowered type, or `None` for an unsupported
/// spelling.
fn resolve_type(records: &RecordRegistry, annotation: &TypeExpr) -> Option<LTy> {
    match annotation {
        TypeExpr::Name { text, .. } => {
            if let Some(scalar) = ScalarType::from_spelling(text) {
                Some(LTy::bare_scalar(scalar))
            } else {
                records.by_name(text).map(|record| LTy::Record {
                    ty: record.type_id,
                    optional: false,
                })
            }
        }
        TypeExpr::Optional { inner, .. } => {
            let inner = resolve_type(records, inner)?;
            if inner.is_optional() {
                None
            } else {
                Some(inner.to_optional())
            }
        }
        _ => None,
    }
}

fn eq_instr(scalar: ScalarType) -> Instr {
    match scalar {
        ScalarType::Int => Instr::EqInt,
        ScalarType::Bool => Instr::EqBool,
        ScalarType::Text => Instr::EqText,
    }
}

fn operator_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        _ => "operator",
    }
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

fn name_error(file: &str, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{name}` is not in scope"),
    )
}

fn loop_error(file: &str, span: SourceSpan, keyword: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{keyword}` is not inside a loop"),
    )
}

fn type_mismatch(file: &str, span: SourceSpan, found: LTy, want: LTy) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "found {} where {} is required",
            found.spelling(),
            want.spelling()
        ),
    )
}

fn unary_error(file: &str, span: SourceSpan, verb: &str, ty: LTy) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("cannot {verb} {}", ty.spelling()),
    )
}

fn binary_error(
    file: &str,
    span: SourceSpan,
    op: BinaryOp,
    left: LTy,
    right: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{}` is not defined for {} and {}",
            operator_symbol(op),
            left.spelling(),
            right.spelling()
        ),
    )
}

fn logic_operand(file: &str, span: SourceSpan, op: BinaryOp, ty: LTy) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{}` operand must be bool, found {}",
            operator_symbol(op),
            ty.spelling()
        ),
    )
}
