//! Function-body lowering (design §B/§D).
//!
//! [`FnLowerer`] type-checks the compiled subset and lowers one function body to
//! a draft instruction stream. Locals are allocated one fresh slot per `const`/
//! `var`/param — slots are never reused — so every read is dominated by the slot's
//! single write and the independent verifier's definite-init dataflow is satisfied.
//! Jumps are emitted with placeholder targets and patched to instruction indices
//! once the target position is known; the encoder rewrites indices to byte offsets.

use marrow_codes::Code;
use marrow_image::{FunctionDef, ImageDraft, ImageType, Instr, SpanEntry};
use marrow_syntax::{
    BinaryOp, Block, ElseIf, Expression, FunctionDecl, LiteralKind, SourceSpan, Statement,
    TypeExpr, UnaryOp, decode_string_literal,
};

use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;

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
    Scalar(ScalarType),
}

/// One in-scope local binding.
struct Local {
    name: String,
    ty: ScalarType,
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
    diagnostics: &'a mut Vec<SourceDiagnostic>,
    file: &'a str,
    code: Vec<Instr>,
    spans: Vec<SpanEntry>,
    locals: Vec<Local>,
    /// `locals.len()` at each open scope's entry, for popping visibility on exit.
    scope_marks: Vec<usize>,
    loops: Vec<LoopCtx>,
    /// Monotonic slot allocator; never decreases, so slots are never reused.
    slot_count: u16,
    ret: RetType,
    /// Set when an unrecoverable check error was reported, so the caller skips
    /// adding a half-built function to the draft.
    failed: bool,
}

impl<'a> FnLowerer<'a> {
    /// Lower `function` and add it (and its export, when public) to the draft.
    pub(crate) fn lower(
        draft: &'a mut ImageDraft,
        diagnostics: &'a mut Vec<SourceDiagnostic>,
        file: &'a str,
        function: &FunctionDecl,
    ) {
        let ret = match &function.return_type {
            None => RetType::Unit,
            Some(TypeExpr::Name { text, span }) => match ScalarType::from_spelling(text) {
                Some(scalar) => RetType::Scalar(scalar),
                None => {
                    diagnostics.push(unsupported(file, *span, "this return type"));
                    return;
                }
            },
            Some(other) => {
                diagnostics.push(unsupported(file, other.span(), "this return type"));
                return;
            }
        };

        let mut lowerer = FnLowerer {
            draft,
            diagnostics,
            file,
            code: Vec::new(),
            spans: Vec::new(),
            locals: Vec::new(),
            scope_marks: Vec::new(),
            loops: Vec::new(),
            slot_count: 0,
            ret,
            failed: false,
        };

        // Params occupy the first slots, pre-initialized to their scalar type.
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
                ty: scalar,
                mutable: false,
                slot,
            });
        }

        let body_flow = lowerer.lower_block(&function.body);

        // Every reachable path must end in a return. A Unit function gets an implicit
        // one when it can fall through; a value function that can fall through is a
        // control-flow error.
        match (body_flow, lowerer.ret) {
            (Flow::Terminates, _) => {}
            (Flow::Fallthrough, RetType::Unit) => {
                lowerer.push(Instr::Return, function.body.span);
            }
            (Flow::Fallthrough, RetType::Scalar(_)) => {
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

        // After the body scope closes, `locals[0..params]` are exactly the params in
        // declaration order, so their image types are read back here.
        let params: Vec<_> = lowerer
            .locals
            .iter()
            .take(function.params.len())
            .map(|local| local.ty.image())
            .collect();
        let ret_ref = match ret {
            RetType::Unit => ImageType::Unit,
            RetType::Scalar(scalar) => ImageType::scalar(scalar.image()),
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

    /// Emit a jump with a placeholder target and return its instruction index for
    /// later patching.
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

    fn patch(&mut self, at: usize, target: usize) {
        match &mut self.code[at] {
            Instr::Jump(t) | Instr::JumpIfFalse(t) => *t = target as u32,
            other => unreachable!("patch target is not a jump: {other:?}"),
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
        self.scope_marks.push(self.locals.len());
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
        let mark = self.scope_marks.pop().expect("scope was opened");
        self.locals.truncate(mark);
        flow
    }

    fn lower_statement(&mut self, statement: &Statement) -> Flow {
        match statement {
            Statement::Const {
                name, ty, value, ..
            } => {
                self.lower_binding(name, ty.as_ref(), Some(value), false);
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
                self.lower_binding(name, ty.as_ref(), Some(value), true);
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
                if let Some(ty) = self.lower_expr(value) {
                    // Discard the produced value to keep the stack balanced.
                    let _ = ty;
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
            } => self.lower_if(condition, then_block, else_ifs, else_block.as_ref()),
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
        value: Option<&Expression>,
        mutable: bool,
    ) {
        let value = value.expect("bindings carry an initializer at this slice");
        let Some(ty) = self.lower_expr(value) else {
            return;
        };
        if let Some(annotation) = annotation
            && let Some(declared) = self.annotation_scalar(annotation)
            && declared != ty
        {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                annotation.span(),
                format!(
                    "initializer is {} but the annotation is {}",
                    ty.spelling(),
                    declared.spelling()
                ),
            ));
            return;
        }
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
        let Expression::Name { segments, span, .. } = target else {
            self.fail(unsupported(
                self.file,
                target.span(),
                "this assignment target",
            ));
            return;
        };
        let [name] = segments.as_slice() else {
            self.fail(unsupported(self.file, *span, "this assignment target"));
            return;
        };
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *span, name));
            return;
        };
        let (slot, local_ty, mutable) = (local.slot, local.ty, local.mutable);
        if !mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *span,
                format!("`{name}` is a `const` and cannot be reassigned"),
            ));
            return;
        }
        let Some(value_ty) = self.lower_expr(value) else {
            return;
        };
        if value_ty != local_ty {
            self.fail(type_mismatch(self.file, value.span(), value_ty, local_ty));
            return;
        }
        self.push(Instr::LocalSet(slot), value.span());
    }

    fn lower_compound_assign(&mut self, target: &Expression, op: BinaryOp, value: &Expression) {
        let Expression::Name { segments, span, .. } = target else {
            self.fail(unsupported(
                self.file,
                target.span(),
                "this assignment target",
            ));
            return;
        };
        let [name] = segments.as_slice() else {
            self.fail(unsupported(self.file, *span, "this assignment target"));
            return;
        };
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *span, name));
            return;
        };
        let (slot, local_ty, mutable) = (local.slot, local.ty, local.mutable);
        if !mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *span,
                format!("`{name}` is a `const` and cannot be reassigned"),
            ));
            return;
        }
        self.push(Instr::LocalGet(slot), *span);
        let Some(result_ty) = self.lower_binary_op(op, local_ty, value) else {
            return;
        };
        if result_ty != local_ty {
            self.fail(type_mismatch(self.file, value.span(), result_ty, local_ty));
            return;
        }
        self.push(Instr::LocalSet(slot), value.span());
    }

    fn lower_return(&mut self, value: Option<&Expression>, span: SourceSpan) -> Flow {
        match (value, self.ret) {
            (None, RetType::Unit) => {
                self.push(Instr::Return, span);
            }
            (None, RetType::Scalar(_)) => {
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
            (Some(expr), RetType::Scalar(want)) => {
                if let Some(ty) = self.lower_expr(expr) {
                    if ty != want {
                        self.fail(type_mismatch(self.file, expr.span(), ty, want));
                    } else {
                        self.push(Instr::Return, span);
                    }
                }
            }
        }
        Flow::Terminates
    }

    fn lower_break(&mut self, span: SourceSpan) -> Flow {
        if self.loops.is_empty() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`break` is not inside a loop".to_string(),
            ));
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
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`continue` is not inside a loop".to_string(),
            ));
            return Flow::Terminates;
        };
        let target = ctx.continue_target;
        self.push(Instr::Jump(target as u32), span);
        Flow::Terminates
    }

    fn lower_if(
        &mut self,
        condition: &Expression,
        then_block: &Block,
        else_ifs: &[ElseIf],
        else_block: Option<&Block>,
    ) -> Flow {
        let mut end_jumps: Vec<usize> = Vec::new();
        let mut all_terminate = else_block.is_some();

        let mut branches: Vec<(&Expression, &Block)> = vec![(condition, then_block)];
        for else_if in else_ifs {
            branches.push((&else_if.condition, &else_if.block));
        }

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
        for jump in end_jumps {
            self.patch(jump, end);
        }

        if all_terminate {
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
        for jump in ctx.break_jumps {
            self.patch(jump, end);
        }
        Flow::Fallthrough
    }

    /// Lower a condition expression, requiring it to be `bool`.
    fn lower_condition(&mut self, expr: &Expression) -> Option<()> {
        let ty = self.lower_expr(expr)?;
        if ty != ScalarType::Bool {
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

    /// Lower `expr`, emitting code that pushes its value and returning its type, or
    /// `None` after reporting a diagnostic.
    fn lower_expr(&mut self, expr: &Expression) -> Option<ScalarType> {
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
            Expression::Unary { op, operand, span } => self.lower_unary(*op, operand, *span),
            Expression::Binary {
                op, left, right, ..
            } => self.lower_binary(*op, left, right),
            other => {
                self.fail(unsupported(self.file, other.span(), "this expression"));
                None
            }
        }
    }

    fn lower_literal(
        &mut self,
        kind: LiteralKind,
        text: &str,
        span: SourceSpan,
    ) -> Option<ScalarType> {
        match kind {
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
                let konst = self.draft.intern_int(value);
                self.push(Instr::ConstLoad(konst.index()), span);
                Some(ScalarType::Int)
            }
            LiteralKind::Bool => {
                let konst = self.draft.intern_bool(text == "true");
                self.push(Instr::ConstLoad(konst.index()), span);
                Some(ScalarType::Bool)
            }
            LiteralKind::String => {
                let Ok(decoded) = decode_string_literal(text) else {
                    self.fail(unsupported(self.file, span, "this string literal"));
                    return None;
                };
                let konst = self.draft.intern_text(&decoded);
                self.push(Instr::ConstLoad(konst.index()), span);
                Some(ScalarType::Text)
            }
            _ => {
                self.fail(unsupported(self.file, span, "this literal"));
                None
            }
        }
    }

    fn lower_unary(
        &mut self,
        op: UnaryOp,
        operand: &Expression,
        span: SourceSpan,
    ) -> Option<ScalarType> {
        let ty = self.lower_expr(operand)?;
        match op {
            UnaryOp::Neg => {
                if ty != ScalarType::Int {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("cannot negate {}", ty.spelling()),
                    ));
                    return None;
                }
                self.push(Instr::IntNeg, span);
                Some(ScalarType::Int)
            }
            UnaryOp::Not => {
                if ty != ScalarType::Bool {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("cannot apply `not` to {}", ty.spelling()),
                    ));
                    return None;
                }
                self.push(Instr::BoolNot, span);
                Some(ScalarType::Bool)
            }
        }
    }

    /// Lower a binary expression whose left operand's type is not yet known.
    fn lower_binary(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
    ) -> Option<ScalarType> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return self.lower_short_circuit(op, left, right);
        }
        let left_ty = self.lower_expr(left)?;
        self.lower_binary_op(op, left_ty, right)
    }

    /// Lower the right operand and the operator, given the left operand's type and
    /// its value already on the stack.
    fn lower_binary_op(
        &mut self,
        op: BinaryOp,
        left_ty: ScalarType,
        right: &Expression,
    ) -> Option<ScalarType> {
        let right_ty = self.lower_expr(right)?;
        let span = right.span();
        use ScalarType::{Bool, Int, Text};
        let (instr, result): (Instr, ScalarType) = match (op, left_ty, right_ty) {
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
                return Some(Bool);
            }
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{}` is not defined for {} and {}",
                        operator_symbol(op),
                        left_ty.spelling(),
                        right_ty.spelling()
                    ),
                ));
                return None;
            }
        };
        self.push(instr, span);
        Some(result)
    }

    /// Lower `and`/`or` with short-circuit evaluation using conditional jumps.
    fn lower_short_circuit(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
    ) -> Option<ScalarType> {
        let left_ty = self.lower_expr(left)?;
        if left_ty != ScalarType::Bool {
            self.fail(logic_operand(self.file, left.span(), op, left_ty));
            return None;
        }
        // `a and b`: if a is false, the result is false (skip b); else the result is
        // b. `a or b`: if a is false, the result is b; else the result is true.
        match op {
            BinaryOp::And => {
                let jif = self.push_jif(left.span());
                let right_ty = self.lower_expr(right)?;
                if right_ty != ScalarType::Bool {
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
                if right_ty != ScalarType::Bool {
                    self.fail(logic_operand(self.file, right.span(), op, right_ty));
                    return None;
                }
                let end = self.here();
                self.patch(to_end, end);
            }
            _ => unreachable!("only and/or reach short-circuit lowering"),
        }
        Some(ScalarType::Bool)
    }

    fn annotation_scalar(&mut self, annotation: &TypeExpr) -> Option<ScalarType> {
        match annotation {
            TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
            _ => None,
        }
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

fn type_mismatch(
    file: &str,
    span: SourceSpan,
    found: ScalarType,
    want: ScalarType,
) -> SourceDiagnostic {
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

fn logic_operand(file: &str, span: SourceSpan, op: BinaryOp, ty: ScalarType) -> SourceDiagnostic {
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
