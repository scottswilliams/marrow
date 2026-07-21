//! Statement, control-flow, loop, and traversal lowering.

use super::*;

impl<'a> FnLowerer<'a> {
    // --- statements ---

    pub(super) fn lower_block(&mut self, block: &Block) -> Flow {
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        let mark = self.locals.len();
        let place_mark = self.places.len();
        // Presence facts established inside this block (e.g. after an upsert) do not
        // outlive it; facts the caller established for the block (a guard) sit below
        // this mark and are preserved here, dropped by the caller.
        let present_mark = self.present_places.len();
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
            if flow == Flow::Rejected || self.terminal_rejection() {
                flow = Flow::Rejected;
                break;
            }
        }
        self.locals.truncate(mark);
        self.places.truncate(place_mark);
        self.present_places.truncate(present_mark);
        flow
    }

    pub(super) fn lower_statement(&mut self, statement: &Statement) -> Flow {
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
                // A call statement may return nothing (no `Pop`); any other expression
                // statement produces a value that is discarded.
                if let Expression::Call {
                    callee, args, span, ..
                } = value
                {
                    match self.lower_call_core(callee, args, *span) {
                        Some(CallResult::Value(_)) => self.push(Instr::Pop, value.span()),
                        Some(CallResult::Diverges) => return Flow::Terminates,
                        Some(CallResult::Unit) | None => {}
                    }
                } else if self.lower_expr(value).is_some() {
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
            Statement::IfConstChain {
                bindings,
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                let bindings: Vec<(&str, Option<&TypeExpr>, &Expression)> = bindings
                    .iter()
                    .map(|b| (b.name.as_str(), b.ty.as_ref(), &b.value))
                    .collect();
                self.lower_if_const_bindings(
                    &bindings,
                    condition.as_ref(),
                    then_block,
                    else_ifs,
                    else_block.as_ref(),
                )
            }
            Statement::LetElse {
                is_var,
                name,
                ty,
                value,
                else_block,
                ..
            } => self.lower_let_else(*is_var, name, ty.as_ref(), value, else_block),
            Statement::While {
                condition, body, ..
            } => self.lower_while(condition, body),
            Statement::For {
                binding,
                order,
                iterable,
                step,
                bound,
                body,
                span,
            } => self.lower_for(
                binding,
                *order,
                iterable,
                step.as_ref(),
                bound.as_ref(),
                body,
                *span,
            ),
            Statement::Checked {
                bind,
                op,
                out_of_range,
                zero_divisor,
                span,
            } => self.lower_checked(
                bind,
                op,
                out_of_range.as_ref(),
                zero_divisor.as_ref(),
                *span,
            ),
            Statement::Transaction { body, .. } => {
                self.push(Instr::TxnBegin, body.span);
                self.txn_depth += 1;
                let body_flow = self.lower_block(body);
                self.txn_depth -= 1;
                if body_flow == Flow::Rejected {
                    return Flow::Rejected;
                }
                // The closing brace is a commit site only for a path that falls out of
                // the region. When every path returns from inside (each in-region
                // `return` is its own commit site), the region has no fall-through: no
                // closing commit is emitted — it would be unreachable — and the region's
                // divergence propagates so the checker sees the function return on every
                // path.
                if body_flow == Flow::Fallthrough {
                    self.push(Instr::TxnCommit, body.span);
                }
                body_flow
            }
            Statement::Delete { path, span } => {
                self.lower_durable_delete(path, *span);
                Flow::Fallthrough
            }
            Statement::PlaceBinding {
                name,
                name_span,
                place,
                ..
            } => {
                self.lower_place_binding(name, *name_span, place);
                Flow::Fallthrough
            }
            Statement::Unset { place, span } => {
                self.lower_unset(place, *span);
                Flow::Fallthrough
            }
            Statement::Assert { value, span } => {
                self.lower_assert(value, *span);
                Flow::Fallthrough
            }
            Statement::Match {
                scrutinee,
                arms,
                span,
            } => self.lower_match(scrutinee, arms, *span),
            other => {
                self.fail(unsupported(self.file, other.span(), "this statement"));
                Flow::Fallthrough
            }
        }
    }

    /// Lower `assert <expr>`. The condition must be bool; on false the emitted
    /// `Assert` op faults the running test with `run.assert`. Legal only in a test
    /// body — in an ordinary function it is `check.assert_outside_test`.
    fn lower_assert(&mut self, value: &Expression, span: SourceSpan) {
        if self.body_kind != BodyKind::Test {
            self.fail(SourceDiagnostic::at(
                Code::CheckAssertOutsideTest.as_str(),
                self.file,
                span,
                "`assert` is legal only inside a `test` declaration".to_string(),
            ));
            return;
        }
        if self.lower_condition(value).is_some() {
            self.push(Instr::Assert, span);
        }
    }

    fn lower_binding(
        &mut self,
        name: &str,
        annotation: Option<&TypeExpr>,
        value: &Expression,
        mutable: bool,
    ) {
        if is_reserved_builtin_name(name) {
            self.fail(reserved_builtin_name(self.file, value.span(), name));
            return;
        }
        // A `const`/`var` never reuses an in-scope `place` name: the place and its
        // designation stay distinct, so a name resolves to exactly one of them.
        if self.lookup_place(name).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                value.span(),
                format!("`{name}` is already bound as a place in this scope"),
            ));
            return;
        }
        let ty = if let Some(annotation) = annotation {
            let expected = match self.resolve(annotation) {
                Ok(expected) => expected,
                Err(refusal) => {
                    self.reject_resolution(refusal, annotation.span(), "this type annotation");
                    return;
                }
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
        if self.durable_access(target).is_some() {
            if let Some(place) = self.resolve_durable(target) {
                self.lower_durable_assign(place, value);
            }
            return;
        }
        // `local.field = value`: a local product mutation. The base names a mutable
        // local record/struct; the assignment sets the field slot present.
        if let Expression::Field {
            base, name, span, ..
        } = target
        {
            self.lower_local_field_assign(base, name, *span, value);
            return;
        }
        // `m[k] = value`: a keyed write on a local map, create-or-replace at the key
        // (the same sentence as a durable keyed write, differing only by the `^`). A
        // list has no keyed write; `xs[i] = value` is refused with a teaching diagnostic.
        if let Expression::Keyed {
            base, keys, span, ..
        } = target
        {
            self.lower_local_bracket_write(base, keys, *span, value);
            return;
        }
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

    /// Lower `local.…field = value`: read-modify-write a field, possibly nested one
    /// or more group levels deep. Every container on the path — the local and each
    /// intervening group sub-record — is loaded, the leaf is coerced to its bare value
    /// type and stored present, and each container is written back into its parent up
    /// to the local. A required or a sparse leaf alike becomes present. The path root
    /// must be a mutable local and every container above the leaf must be present.
    fn lower_local_field_assign(
        &mut self,
        base: &Expression,
        field_name: &str,
        span: SourceSpan,
        value: &Expression,
    ) {
        let Some(chain) = self.resolve_place_chain(base) else {
            return;
        };
        if !chain.mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                chain.root_span,
                format!(
                    "`{}` is a `const` and cannot be reassigned",
                    chain.root_name
                ),
            ));
            return;
        }
        let Some((leaf_index, leaf_ty, _required)) =
            self.resolve_product_field(chain.ty, field_name, base.span(), span)
        else {
            return;
        };
        self.push_place_containers(chain.slot, &chain.indices, span);
        if self.lower_as(value, garg_to_lty(leaf_ty)).is_none() {
            return;
        }
        self.push(Instr::FieldSet(leaf_index), span);
        self.writeback_place_containers(chain.slot, &chain.indices, span);
    }

    /// Push the container stack for a nested field mutation: the local at `slot` and
    /// each ancestor container reached by descending `indices`, one per depth from the
    /// local (depth 0) through the leaf's own container (depth `indices.len()`). Every
    /// descended field is present (a required group slot), so each `FieldGet` yields a
    /// bare record. Leaves the containers on the stack for the leaf `FieldSet`/
    /// `FieldUnset` and a matching [`Self::writeback_place_containers`].
    fn push_place_containers(&mut self, slot: u16, indices: &[u16], span: SourceSpan) {
        for depth in 0..=indices.len() {
            self.push(Instr::LocalGet(slot), span);
            for index in &indices[..depth] {
                self.push(Instr::FieldGet(*index), span);
            }
        }
    }

    /// Write each mutated container back into its parent (innermost first) and store
    /// the updated local. Pairs with [`Self::push_place_containers`] after the leaf op
    /// has left the innermost container's new value on the stack.
    fn writeback_place_containers(&mut self, slot: u16, indices: &[u16], span: SourceSpan) {
        for index in indices.iter().rev() {
            self.push(Instr::FieldSet(*index), span);
        }
        self.push(Instr::LocalSet(slot), span);
    }

    /// Lower `unset local.field` or `unset m[k]`: clear a sparse field of a local
    /// product to absent, or remove a key from a local map. A required field cannot be
    /// unset (`check.type`); a durable place uses `delete`, not `unset`; a list has no
    /// keyed removal; any other place is unsupported.
    fn lower_unset(&mut self, place: &Expression, span: SourceSpan) {
        if Self::durable_shape(place).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`unset` clears a local field; use `delete` for a durable place".to_string(),
            ));
            return;
        }
        if let Expression::Keyed {
            base, keys, span, ..
        } = place
        {
            self.lower_local_bracket_unset(base, keys, *span);
            return;
        }
        let Expression::Field {
            base,
            name,
            span: field_span,
            ..
        } = place
        else {
            self.fail(unsupported(self.file, span, "this `unset` target"));
            return;
        };
        let Some(chain) = self.resolve_place_chain(base) else {
            return;
        };
        if !chain.mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                chain.root_span,
                format!("`{}` is a `const` and cannot be modified", chain.root_name),
            ));
            return;
        }
        let Some((leaf_index, _field_ty, required)) =
            self.resolve_product_field(chain.ty, name, base.span(), *field_span)
        else {
            return;
        };
        if required {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *field_span,
                format!("`{name}` is a required field and cannot be unset"),
            ));
            return;
        }
        self.push_place_containers(chain.slot, &chain.indices, span);
        self.push(Instr::FieldUnset(leaf_index), span);
        self.writeback_place_containers(chain.slot, &chain.indices, span);
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
        if ty.bare_scalar_type().is_none() && ty.bare_nominal().is_none() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "cannot apply a compound assignment to {}",
                    ty.spelling(self.records)
                ),
            ));
            return;
        }
        self.push(Instr::LocalGet(slot), span);
        let Some(result) = self.lower_binary_op(op, ty, value) else {
            return;
        };
        if result != ty {
            self.fail(type_mismatch(
                self.records,
                self.file,
                value.span(),
                result,
                ty,
            ));
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

    /// Resolve a place expression to its chain of present composite containers rooted
    /// at a local: a bare local, or a local descended through one or more group
    /// members. Each intervening member must be a present (required) composite so a
    /// read-modify-write reaches it; a possibly-absent member is a `check.type`
    /// rejection. Reports name and non-place errors like [`Self::resolve_place`].
    fn resolve_place_chain(&mut self, target: &Expression) -> Option<PlaceChain> {
        match target {
            Expression::Name { segments, span, .. } => {
                let [name] = segments.as_slice() else {
                    self.fail(unsupported(self.file, *span, "this assignment target"));
                    return None;
                };
                let Some(local) = self.lookup(name) else {
                    self.fail(name_error(self.file, *span, name));
                    return None;
                };
                Some(PlaceChain {
                    slot: local.slot,
                    mutable: local.mutable,
                    root_span: *span,
                    root_name: name.clone(),
                    ty: local.ty,
                    indices: Vec::new(),
                })
            }
            Expression::Field {
                base, name, span, ..
            } => {
                let mut chain = self.resolve_place_chain(base)?;
                let (index, field_ty, required) =
                    self.resolve_product_field(chain.ty, name, base.span(), *span)?;
                if !required {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        format!(
                            "cannot assign through the possibly-absent member `{name}`. A member \
                             that is not `required` is absent until it holds a value, and a \
                             read-modify-write cannot begin from an absent place. Assign `{name}` \
                             a present value first."
                        ),
                    ));
                    return None;
                }
                chain.indices.push(index);
                chain.ty = garg_to_lty(field_ty);
                Some(chain)
            }
            _ => {
                self.fail(unsupported(
                    self.file,
                    target.span(),
                    "this assignment target",
                ));
                None
            }
        }
    }

    /// Emit a function-exit `return`. Inside an owned `transaction` region an explicit
    /// `return` is a commit site: the region's staged writes commit before the frame
    /// exits. The return expression is already lowered (its durable reads ran while the
    /// region was open), so the ordering is evaluate → commit → return, with the value
    /// left on the stack across the stack-neutral `TxnCommit`. Only the owning export
    /// runs with `txn_depth > 0`; a helper called inside the region lowers at depth zero,
    /// so its own `return` carries no commit and is not a region exit. `try`'s implicit
    /// `err` exit is emitted separately and never routes here, so it stays barred from
    /// crossing a region. The verifier independently proves a `TxnCommit` precedes the
    /// `Return` on every in-region path.
    fn emit_region_return(&mut self, span: SourceSpan) {
        if self.txn_depth > 0 {
            self.push(Instr::TxnCommit, span);
        }
        self.push(Instr::Return, span);
    }

    fn lower_return(&mut self, value: Option<&Expression>, span: SourceSpan) -> Flow {
        match (value, self.ret) {
            (None, RetType::Unit) => {
                self.emit_region_return(span);
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
                    self.emit_region_return(span);
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
        #[expect(
            clippy::expect_used,
            reason = "lowering bookkeeping: the empty-loop-stack case returned above, so a loop context is present here"
        )]
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
    pub(super) fn lower_cond_chain(
        &mut self,
        branches: &[(&Expression, &Block)],
        else_block: Option<&Block>,
    ) -> Flow {
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        let mut end_jumps: Vec<usize> = Vec::new();
        let mut all_terminate = else_block.is_some();

        for (cond, block) in branches {
            // `exists(p)` over a named place proves the entry present in the guarded
            // block: a sparse-field set through `p` there lowers to the strict form.
            let guard_path = self.exists_guard_path(cond);
            if self.lower_condition(cond).is_none() {
                return if self.terminal_rejection() {
                    Flow::Rejected
                } else {
                    Flow::Fallthrough
                };
            }
            let jif = self.push_jif(cond.span());
            let present_mark = self.present_places.len();
            if let Some(path) = guard_path {
                self.mark_present(path);
            }
            let flow = self.lower_block(block);
            self.present_places.truncate(present_mark);
            if flow == Flow::Rejected {
                return Flow::Rejected;
            }
            all_terminate &= flow == Flow::Terminates;
            if flow == Flow::Fallthrough {
                end_jumps.push(self.push_jump(block.span));
            }
            let next = self.here();
            self.patch(jif, next);
        }

        if let Some(else_block) = else_block {
            let flow = self.lower_block(else_block);
            if flow == Flow::Rejected {
                return Flow::Rejected;
            }
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
        // The single `if const a = e` is the one-binding, no-condition case of the
        // general chained form.
        self.lower_if_const_bindings(
            &[(name, annotation, value)],
            None,
            then_block,
            else_ifs,
            else_block,
        )
    }

    /// Lower the general `if const` form (B5): a left-to-right chain of existence
    /// bindings joined by `and` and an optional trailing bare condition, with the
    /// then and `else if`/`else` tails. Each binding's value is proven present
    /// before the next is evaluated (short-circuit), each binding scopes rightward
    /// into later binding values, the condition, and the then block, and any absent
    /// binding or false condition takes the else tail. This is the one owner of
    /// `if const` lowering; the single form is one binding with no condition.
    pub(super) fn lower_if_const_bindings(
        &mut self,
        bindings: &[(&str, Option<&TypeExpr>, &Expression)],
        condition: Option<&Expression>,
        then_block: &Block,
        else_ifs: &[ElseIf],
        else_block: Option<&Block>,
    ) -> Flow {
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        let mark = self.locals.len();
        let present_mark = self.present_places.len();

        // The present path threads through every binding and the condition into the
        // then block; every failure edge (an absent binding or a false condition)
        // jumps to the shared absent tail. Each `BranchPresent`/`JumpIfFalse` pops its
        // own operand, so all failure edges reach the tail with a balanced stack.
        let Some(fail_jumps) = self.lower_if_const_head(bindings, condition) else {
            self.present_places.truncate(present_mark);
            self.locals.truncate(mark);
            return if self.terminal_rejection() {
                Flow::Rejected
            } else {
                Flow::Fallthrough
            };
        };

        let then_flow = self.lower_block(then_block);
        self.present_places.truncate(present_mark);
        self.locals.truncate(mark);
        if then_flow == Flow::Rejected {
            return Flow::Rejected;
        }

        let mut end_jumps = Vec::new();
        if then_flow == Flow::Fallthrough {
            end_jumps.push(self.push_jump(then_block.span));
        }

        // Absent/false tail: the `else if`/`else` chain.
        let absent = self.here();
        self.patch_all(fail_jumps, absent);
        let branches: Vec<(&Expression, &Block)> = else_ifs
            .iter()
            .map(|else_if| (&else_if.condition, &else_if.block))
            .collect();
        let else_flow = self.lower_cond_chain(&branches, else_block);
        if else_flow == Flow::Rejected {
            return Flow::Rejected;
        }

        let end = self.here();
        self.patch_all(end_jumps, end);

        if then_flow == Flow::Terminates && else_flow == Flow::Terminates {
            Flow::Terminates
        } else {
            Flow::Fallthrough
        }
    }

    /// Emit the present-threading head of an `if const` chain: for each binding,
    /// prove its value present and bind it to a fresh local scoped rightward; then
    /// evaluate the optional trailing condition. Returns the failure jumps to patch
    /// to the absent tail, leaving the bindings' locals in scope for the then block;
    /// `None` on a hard type error (the caller restores the local stack).
    fn lower_if_const_head(
        &mut self,
        bindings: &[(&str, Option<&TypeExpr>, &Expression)],
        condition: Option<&Expression>,
    ) -> Option<Vec<usize>> {
        let mut fail_jumps: Vec<usize> = Vec::new();
        for (name, annotation, value) in bindings.iter().copied() {
            if is_reserved_builtin_name(name) {
                self.fail(reserved_builtin_name(self.file, value.span(), name));
                return None;
            }
            // A whole durable entry address (`if const book = ^books(id)` or the named
            // `place` form) reads the entry here and proves it present on the guarded
            // edge, so a sparse-field set through the same place in the then block
            // lowers strict; a bare place name is otherwise not a value.
            let mut guard_path: Option<Vec<u16>> = None;
            let optional = if matches!(self.durable_access(value), Some(DurShape::Entry)) {
                let place = self.resolve_durable(value)?;
                guard_path = place.bound_key_path();
                self.lower_durable_read(place)?
            } else {
                self.lower_expr(value)?
            };
            if !optional.is_optional() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    value.span(),
                    format!(
                        "`if const` needs an optional, found {}",
                        optional.spelling(self.records)
                    ),
                ));
                return None;
            }
            let bound = optional.to_bare();
            if let Some(annotation) = annotation {
                match self.resolve(annotation) {
                    Ok(declared) if declared != bound => {
                        self.fail(type_mismatch(
                            self.records,
                            self.file,
                            annotation.span(),
                            bound,
                            declared,
                        ));
                        return None;
                    }
                    Ok(_) => {}
                    Err(refusal) => {
                        self.reject_resolution(refusal, annotation.span(), "this type annotation");
                        return None;
                    }
                }
            }

            // Present edge falls through with the unwrapped bare value; absent edge
            // jumps to the shared tail.
            fail_jumps.push(self.push_branch_present(value.span()));
            let slot = self.alloc_slot();
            self.push(Instr::LocalSet(slot), value.span());
            self.locals.push(Local {
                name: name.to_string(),
                ty: bound,
                mutable: false,
                slot,
            });
            if let Some(path) = guard_path {
                self.mark_present(path);
            }
        }

        if let Some(cond) = condition {
            let cond_ty = self.lower_expr(cond)?;
            if cond_ty != LTy::bare_scalar(ScalarType::Bool) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    cond.span(),
                    format!(
                        "an `if const` chain condition must be `bool`, found {}",
                        cond_ty.spelling(self.records)
                    ),
                ));
                return None;
            }
            fail_jumps.push(self.push_jif(cond.span()));
        }

        Some(fail_jumps)
    }

    /// Lower `const x = e else { … }` / `var x = e else { … }` (B6, let-else): bind
    /// `x` from the present value of the optional `e` and continue with `x` in scope
    /// for the rest of the enclosing block; when `e` is absent, run the `else` block,
    /// which must diverge. Reuses the one-binding `if const` head for the present
    /// path, and the existing `Flow::Terminates` divergence analysis proves the else
    /// diverges — so let-else adds no new control-flow analysis.
    fn lower_let_else(
        &mut self,
        is_var: bool,
        name: &str,
        annotation: Option<&TypeExpr>,
        value: &Expression,
        else_block: &Block,
    ) -> Flow {
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        let mark = self.locals.len();
        let present_mark = self.present_places.len();
        let Some(fail_jumps) = self.lower_if_const_head(&[(name, annotation, value)], None) else {
            self.locals.truncate(mark);
            self.present_places.truncate(present_mark);
            return if self.terminal_rejection() {
                Flow::Rejected
            } else {
                Flow::Fallthrough
            };
        };
        // The head bound `x` (and, for a durable entry read, a presence fact) on the
        // present edge. They belong to the continuation after the statement, not to
        // the `else` — the absent edge, where `x` is not established. Lift them out so
        // the `else` cannot see the binding (a reference there is a scoped unknown
        // name, not an uninitialized-slot image rejection) and restore them for the
        // continuation. A `var` let-else binds mutably.
        let mut bound_locals = self.locals.split_off(mark);
        let bound_present = self.present_places.split_off(present_mark);
        if is_var {
            for local in &mut bound_locals {
                local.mutable = true;
            }
        }

        // The present path continues past the `else`; the absent edge runs the
        // diverging `else` block, so control only reaches past the statement with `x`
        // bound.
        let to_after = self.push_jump(value.span());
        let absent = self.here();
        self.patch_all(fail_jumps, absent);
        let else_flow = self.lower_block(else_block);
        if else_flow == Flow::Rejected {
            return Flow::Rejected;
        }
        if else_flow != Flow::Terminates {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                else_block.span,
                "the `else` of a let-else binding must diverge, for example with \
                 `return`, `throw`, or `unreachable`"
                    .to_string(),
            ));
        }
        let after = self.here();
        self.patch(to_after, after);

        // Restore the binding and presence fact for the continuation: past the
        // statement `x` is always present, because the absent edge diverged.
        self.locals.extend(bound_locals);
        self.present_places.extend(bound_present);
        Flow::Fallthrough
    }

    /// Lower a `match` over a flat enum scrutinee (design §B). The scrutinee is
    /// evaluated once into a fresh local; the arms dispatch through a branch chain
    /// over the enum tag (`EnumTag` + `EqInt` + `JumpIfFalse`), the simplest form
    /// the verifier admits without a tag-switch opcode. The match must cover every
    /// member exactly once with no wildcard arm; exhaustiveness is a check-time
    /// rule, not an image invariant. Because the match is exhaustive, the last arm
    /// in source order runs unconditionally (no test): every other member is caught
    /// by an earlier arm, so only its own member reaches it, which also makes its
    /// positional payload reads (`EnumPayloadGet`) sound.
    pub(super) fn lower_match(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        span: SourceSpan,
    ) -> Flow {
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        let Some(scrut_ty) = self.lower_expr(scrutinee) else {
            return if self.terminal_rejection() {
                Flow::Rejected
            } else {
                Flow::Fallthrough
            };
        };
        let Some(enum_id) = scrut_ty.bare_enum() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckMatchArm.as_str(),
                self.file,
                scrutinee.span(),
                format!(
                    "`match` needs an enum value, found {}",
                    scrut_ty.spelling(self.records)
                ),
            ));
            return Flow::Fallthrough;
        };
        // The scrutinee's variants: member name plus payload type list, owned so the
        // arm loop can borrow `self` mutably while resolving each arm. A concrete
        // user `enum`, a generic enum instantiation, and the reserved `Option`/
        // `Result` (themselves generic enums) all supply their variants through the
        // one enum-shape owner.
        let variants = match self.records.enum_variants(enum_id) {
            Ok(Some(variants)) => variants,
            Ok(None) => {
                self.reject_resolution(
                    ResolveError::Invariant(LowerInvariant::ReadyBodyMissing(TypeInstId::Enum(
                        enum_id,
                    ))),
                    scrutinee.span(),
                    "this enum match",
                );
                return Flow::Rejected;
            }
            Err(invariant) => {
                self.reject_resolution(
                    ResolveError::Invariant(invariant),
                    scrutinee.span(),
                    "this enum match",
                );
                return Flow::Rejected;
            }
        };
        let enum_name = scrut_ty.spelling(self.records);
        let variants: Vec<(String, Vec<LTy>)> = variants
            .into_iter()
            .map(|(name, payload)| (name, payload.into_iter().map(garg_to_lty).collect()))
            .collect();

        let scrut_slot = self.alloc_slot();
        self.push(Instr::LocalSet(scrut_slot), span);

        let mut covered = vec![false; variants.len()];
        let mut end_jumps: Vec<usize> = Vec::new();
        let mut all_terminate = true;
        let mut any_arm = false;
        let arm_count = arms.len();

        for (position, arm) in arms.iter().enumerate() {
            let is_last = position + 1 == arm_count;
            let [member] = arm.path.as_slice() else {
                self.fail(unsupported(
                    self.file,
                    arm.span,
                    "a hierarchical (`::`) match arm",
                ));
                continue;
            };
            let Some(variant_index) = variants.iter().position(|(name, _)| name == member) else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckMatchArm.as_str(),
                    self.file,
                    arm.span,
                    format!("`{member}` is not a member of `{enum_name}`"),
                ));
                continue;
            };
            if covered[variant_index] {
                self.fail(SourceDiagnostic::at(
                    Code::CheckMatchArm.as_str(),
                    self.file,
                    arm.span,
                    format!("member `{member}` is already covered by an earlier arm"),
                ));
                continue;
            }
            covered[variant_index] = true;
            any_arm = true;
            let payload = variants[variant_index].1.clone();
            if !arm.bindings.is_empty() && arm.bindings.len() != payload.len() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckMatchArm.as_str(),
                    self.file,
                    arm.span,
                    format!(
                        "member `{member}` carries {} payload field(s), but the arm binds {}",
                        payload.len(),
                        arm.bindings.len()
                    ),
                ));
                continue;
            }

            // Non-last arms test the tag and skip to the next arm on a mismatch;
            // the exhaustive last arm runs unconditionally.
            let to_next = if is_last {
                None
            } else {
                self.push(Instr::LocalGet(scrut_slot), arm.span);
                self.push(Instr::EnumTag, arm.span);
                let konst = self.draft.intern_int(variant_index as i64);
                self.push(Instr::ConstLoad(konst.index()), arm.span);
                self.push(Instr::EqInt, arm.span);
                Some(self.push_jif(arm.span))
            };

            // Bind the payload positionally into fresh locals scoped to the arm.
            let mark = self.locals.len();
            for (field, (binding, leaf_ty)) in arm.bindings.iter().zip(&payload).enumerate() {
                let slot = self.alloc_slot();
                self.push(Instr::LocalGet(scrut_slot), binding.span);
                self.push(
                    Instr::EnumPayloadGet {
                        variant: variant_index as u16,
                        field: field as u16,
                    },
                    binding.span,
                );
                self.push(Instr::LocalSet(slot), binding.span);
                self.locals.push(Local {
                    name: binding.name.clone(),
                    ty: *leaf_ty,
                    mutable: false,
                    slot,
                });
            }
            let flow = self.lower_block(&arm.block);
            self.locals.truncate(mark);
            if flow == Flow::Rejected {
                return Flow::Rejected;
            }
            all_terminate &= flow == Flow::Terminates;
            if flow == Flow::Fallthrough && !is_last {
                end_jumps.push(self.push_jump(arm.block.span));
            }
            if let Some(to_next) = to_next {
                let next = self.here();
                self.patch(to_next, next);
            }
        }

        // Exhaustiveness: every member covered exactly once, no wildcard arm.
        let missing: Vec<(&str, usize)> = variants
            .iter()
            .zip(&covered)
            .filter(|(_, covered)| !**covered)
            .map(|((name, payload), _)| (name.as_str(), payload.len()))
            .collect();
        if !missing.is_empty() {
            let names = missing
                .iter()
                .map(|(name, _)| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            // The canonical arm head each missing member needs: a payloadless member
            // takes `member =>`; a member with an N-value payload takes N positional
            // bindings, spelled `member(_, …) =>` with author-neutral `_` placeholders.
            let arms = missing
                .iter()
                .map(|(name, arity)| match arity {
                    0 => format!("`{name} =>`"),
                    n => format!("`{name}({}) =>`", vec!["_"; *n].join(", ")),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let arm_word = if missing.len() == 1 { "arm" } else { "arms" };
            self.fail(SourceDiagnostic::at(
                Code::CheckMatchNonexhaustive.as_str(),
                self.file,
                span,
                format!(
                    "the `match` on `{enum_name}` does not cover {names}. A match covers every \
                     member of an enum exactly once and admits no wildcard arm. Add the missing \
                     {arm_word}: {arms}."
                ),
            ));
        }

        let end = self.here();
        self.patch_all(end_jumps, end);
        // The match terminates only when it is exhaustive (so the unconditional last
        // arm is reached) and every arm diverges.
        if any_arm && missing.is_empty() && all_terminate {
            Flow::Terminates
        } else {
            Flow::Fallthrough
        }
    }

    /// Lower a `for` loop. A durable root/branch traversal place (`^root` or
    /// `^root(k).branch`) takes the bounded freeze-then-run path; a range or local
    /// `List`/`Map` iterable takes the collection path. Reversed order and a range
    /// step apply only to the latter.
    #[allow(clippy::too_many_arguments)]
    fn lower_for(
        &mut self,
        binding: &ForBinding,
        order: marrow_syntax::LoopOrder,
        iterable: &Expression,
        step: Option<&Expression>,
        bound: Option<&TraversalBound>,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        // An integer range iterates its counter directly onto a pure counter loop; it
        // takes neither a durable `at most` bound nor a reversed walk in the first ring.
        if let Some(range) = range_expr(iterable) {
            if order != marrow_syntax::LoopOrder::Forward {
                self.fail(unsupported(self.file, span, "a reversed range"));
                return Flow::Fallthrough;
            }
            if bound.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    "`at most N` and `on more` apply only to a durable root or branch \
                     traversal (`for k in ^root at most N`), not to a range"
                        .to_string(),
                ));
                return Flow::Fallthrough;
            }
            return self.lower_for_range(binding, range, step, body, span);
        }
        // A `for` head over a managed index scans it. Only a nonunique index is scanned
        // (progressive-prefix); a unique index is an exact lookup, not an iteration.
        if let Some(read) = self.resolve_index_read(iterable) {
            if read.index.unique {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "unique index `{}` is an exact lookup, not a scan; read it with \
                         `if const x = ^root.{}[keys]`",
                        read.index.name, read.index.name
                    ),
                ));
                return Flow::Fallthrough;
            }
            if order != marrow_syntax::LoopOrder::Forward {
                self.fail(unsupported(self.file, span, "reversed index scan"));
                return Flow::Fallthrough;
            }
            return self.lower_index_scan(binding, read, bound, body, span);
        }
        // A durable traversal place iterates the store; it is always bounded.
        if self.is_traversal_place(iterable) {
            if order != marrow_syntax::LoopOrder::Forward {
                self.fail(unsupported(self.file, span, "reversed durable traversal"));
                return Flow::Fallthrough;
            }
            let Some(target) = self.resolve_traversal_place(iterable) else {
                return Flow::Fallthrough;
            };
            return self.lower_bounded_traversal(binding, target, bound, body, span);
        }
        // A bare `place`/pin name is one durable entry, not a family: it is not a
        // traversal base. Steer to the branch family beneath it rather than falling to the
        // generic collection refusal.
        if self.is_place_name(iterable) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "a `place` names one durable entry, not a family to iterate. Traverse a \
                 keyed branch beneath it with `for k in <place>.branch at most N`, or \
                 iterate the store root directly (`for k in ^root at most N`)."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        }
        // A range or local collection takes no `at most N` / `on more` clause.
        if bound.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`at most N` and `on more` apply only to a durable root or branch \
                 traversal (`for k in ^root at most N`)"
                    .to_string(),
            ));
            return Flow::Fallthrough;
        }
        if order != marrow_syntax::LoopOrder::Forward {
            self.fail(unsupported(self.file, span, "reversed iteration"));
            return Flow::Fallthrough;
        }
        if step.is_some() {
            self.fail(unsupported(self.file, span, "a loop step"));
            return Flow::Fallthrough;
        }
        self.lower_for_collection(binding, iterable, body, span)
    }

    /// Lower `for i in lo..hi` / `for i in lo..=hi [by step]` over an integer range: a
    /// pure counter loop. Both bounds are `int` expressions evaluated once, `lo` into the
    /// counter and `hi` into a fixed bound; the counter is bound to the loop variable each
    /// iteration and advanced by a positive integer-literal `step` (default `1`). A dead or
    /// empty range (`lo >= hi` exclusive, `lo > hi` inclusive) runs the body zero times.
    /// The advance uses the checked add, so a range that reaches the integer domain
    /// boundary ends the loop rather than raising `run.overflow`.
    fn lower_for_range(
        &mut self,
        binding: &ForBinding,
        range: RangeExpr,
        step: Option<&Expression>,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        let [name] = binding.names.as_slice() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "a range `for` binds one integer name, found {}; write `for i in lo..hi`",
                    binding.names.len()
                ),
            ));
            return Flow::Fallthrough;
        };
        let (Some(lo), Some(hi)) = (range.start, range.end) else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                range.span,
                "a range `for` iterates between two bounds, but this range leaves one end \
                 open. Write `for i in lo..hi` with both endpoints."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        let Some(stride) = self.range_step(step) else {
            return Flow::Fallthrough;
        };
        let int = LTy::bare_scalar(ScalarType::Int);

        // counter = lo
        if self.lower_as(lo, int).is_none() {
            return Flow::Fallthrough;
        }
        let counter_slot = self.alloc_slot();
        self.push(Instr::LocalSet(counter_slot), span);
        // hi is evaluated once and held for the guard.
        if self.lower_as(hi, int).is_none() {
            return Flow::Fallthrough;
        }
        let hi_slot = self.alloc_slot();
        self.push(Instr::LocalSet(hi_slot), span);
        let step_const = self.draft.intern_int(stride);

        // The advance sits at the loop top and the first entry skips it, so `continue`
        // jumps to the advance and always makes progress toward the bound.
        let skip = self.push_jump(span);
        let advance = self.here();
        self.push(Instr::LocalGet(counter_slot), span);
        self.push(Instr::ConstLoad(step_const.index()), span);
        let advance_at = self.here();
        self.push(Instr::IntAddChecked(0), span);
        self.push(Instr::LocalSet(counter_slot), span);

        let guard = self.here();
        self.patch(skip, guard);
        self.push(Instr::LocalGet(counter_slot), span);
        self.push(Instr::LocalGet(hi_slot), span);
        self.push(
            if range.inclusive_end {
                Instr::IntLe
            } else {
                Instr::IntLt
            },
            span,
        );
        let exit_jif = self.push_jif(span);

        // The loop variable reads the counter slot directly; it is immutable to the body,
        // and the advance between iterations updates it.
        let mark = self.locals.len();
        let place_mark = self.places.len();
        self.locals.push(Local {
            name: name.name.clone(),
            ty: int,
            mutable: false,
            slot: counter_slot,
        });
        self.loops.push(LoopCtx {
            continue_target: advance,
            break_jumps: Vec::new(),
        });
        let body_flow = self.lower_block(body);
        #[expect(
            clippy::expect_used,
            reason = "lowering bookkeeping: this function pushed a loop context before lowering the body, so the paired pop returns it"
        )]
        let ctx = self.loops.pop().expect("loop was pushed");
        self.locals.truncate(mark);
        self.places.truncate(place_mark);
        if body_flow == Flow::Rejected {
            return Flow::Rejected;
        }
        if body_flow == Flow::Fallthrough {
            self.push(Instr::Jump(advance as u32), body.span);
        }

        let after_loop = self.here();
        self.patch(exit_jif, after_loop);
        // Reaching the integer domain boundary ends the loop at the same exit.
        self.patch(advance_at, after_loop);
        self.patch_all(ctx.break_jumps, after_loop);
        Flow::Fallthrough
    }

    /// Evaluate a range `by step`: a positive compile-time integer literal, defaulting to
    /// `1` when the head carries no `by`. A zero, negative, or computed step is a precise
    /// diagnostic — the stride must be a literal so a non-progressing loop is refused at
    /// compile time.
    fn range_step(&mut self, step: Option<&Expression>) -> Option<i64> {
        let Some(expr) = step else {
            return Some(1);
        };
        if let Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } = expr
            && let Some(value) = parse_int(text).filter(|value| *value > 0)
        {
            return Some(value);
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            expr.span(),
            "this range step is not a positive integer literal. A range advances by a \
             positive integer literal each iteration, so `by 0`, a negative step, and a \
             computed step make no valid stride. Write `by N` with a positive literal, or \
             omit `by` for a step of 1."
                .to_string(),
        ));
        None
    }

    /// Whether `iterable` names a durable traversal place syntactically: a bare store root
    /// `^root` (the root entry family), an entry address extended by a bare branch-layer
    /// name `^root(key)….branch` (a keyed branch family under a fixed ancestor key-path, at
    /// any depth), or a bare branch selection on an in-scope `place`/pin name
    /// `<place>.branch` (the branch family beneath the entry the place already addresses).
    /// The resolver rechecks the store, place, and branch names; this only routes the head
    /// to the durable path.
    fn is_traversal_place(&self, iterable: &Expression) -> bool {
        match iterable {
            Expression::SavedRoot { .. } => true,
            Expression::Field { base, .. } => is_entry_address(base) || self.is_place_name(base),
            _ => false,
        }
    }

    /// Whether an `exists` argument names a family (a store root, or a keyed branch family)
    /// rather than a specific entry or field. A store root is always a family; a `.tail`
    /// selection on an entry address is a family only when `tail` is a declared keyed
    /// branch — a scalar-field selection is a specific-cell probe. Non-emitting: it
    /// classifies the argument before a probe is chosen, since a branch family and a
    /// scalar field share the `Field`-on-entry-address syntax.
    pub(super) fn arg_is_family(&self, expr: &Expression) -> bool {
        match expr {
            Expression::SavedRoot { .. } => true,
            Expression::Field { base, name, .. } => self
                .entry_address_node(base)
                .is_some_and(|parent| parent.branch(name).is_some()),
            _ => false,
        }
    }

    /// The durable node an entry-address expression addresses, resolved against the named
    /// durable root without emitting a diagnostic. `None` when `expr` is not a resolvable
    /// entry address (a wrong or parked root name, an unknown branch, or a non-address
    /// shape). Used only to classify an `exists` tail; the real resolvers own diagnostics.
    fn entry_address_node(&self, expr: &Expression) -> Option<DurNode<'a>> {
        let Expression::Keyed { base, .. } = expr else {
            return None;
        };
        match &**base {
            Expression::SavedRoot { name, .. } => {
                self.durable.root_by_name(name).map(DurNode::Root)
            }
            Expression::Field {
                base: parent_base,
                name: branch_name,
                ..
            } => {
                let parent = self.entry_address_node(parent_base)?;
                parent.branch(branch_name).map(DurNode::Branch)
            }
            _ => None,
        }
    }

    /// Resolve a durable traversal place into the traversed layer's entry site, its
    /// immediate key type, and the ancestor key-path locating its parent entry (empty for a
    /// root family, `[root_key]` for a single-level branch family, deeper for a nested
    /// branch layer). The iterable is the root itself, or an entry address extended by a
    /// bare branch-layer name `^root(k)….b(bk).layer`; the branch chain before the layer
    /// resolves through the recursive entry-address walker, so an inner branch layer
    /// iterates under a full ancestor key-path. Reports a precise diagnostic and returns
    /// `None` on a missing store, a wrong store name, or an unknown branch.
    pub(super) fn resolve_traversal_place<'e>(
        &mut self,
        iterable: &'e Expression,
    ) -> Option<TraversalTarget<'e>> {
        match iterable {
            Expression::SavedRoot { name, span } => {
                let root = self.resolve_root(name, *span)?;
                let entry_site = root.entry_site;
                let record = root.record;
                let key_ty = self.single_traversal_column(&root.key, *span)?;
                Some(TraversalTarget {
                    entry_site,
                    key_ty,
                    record,
                    node_kind: PlaceNodeKind::Root,
                    ancestor_keys: Vec::new(),
                    span: *span,
                })
            }
            Expression::Field {
                base,
                name: layer_name,
                name_span: layer_span,
                span,
                ..
            } => {
                // A bare branch selection on an in-scope `place`/pin name iterates the
                // branch family beneath the entry the place already addresses; its ancestor
                // key-path is the place's pre-evaluated key slots.
                if self.is_place_name(base) {
                    return self.resolve_traversal_through_place(
                        base,
                        layer_name,
                        *layer_span,
                        *span,
                    );
                }
                // The base is the addressed parent entry `^root(k)….b(bk)`; the final bare
                // name is the branch family iterated under it. Its ancestor key-path is the
                // parent entry's whole key-path (root-first). The store is resolved at the
                // base's `^name` leaf.
                let root_name = saved_root_name(base)?;
                let root = self.resolve_root(root_name, iterable.span())?;
                let (ancestor_keys, parent) = self.resolve_entry_address(root, base)?;
                let Some(layer) = parent.branch(layer_name) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *layer_span,
                        parent.no_branch_message(layer_name),
                    ));
                    return None;
                };
                let entry_site = layer.entry_site;
                let record = layer.record;
                let key_ty = self.single_traversal_column(&layer.key, *span)?;
                Some(TraversalTarget {
                    entry_site,
                    key_ty,
                    record,
                    node_kind: PlaceNodeKind::Branch,
                    ancestor_keys,
                    span: *span,
                })
            }
            _ => None,
        }
    }

    /// Resolve `<place>.branch` into the traversed branch layer's entry site, its immediate
    /// key type, and the ancestor key-path locating its parent entry. The parent is the
    /// entry the `place`/pin already addresses, so the ancestor key-path is the place's
    /// pre-evaluated key slots (`bound_keys`), root-first: one slot for a single-key root
    /// place, several for a composite-key root place or a nested branch place. The branch is
    /// found beneath the place's recorded durable node — its branch record for a branch
    /// place, its owning root for a root place — so resolution never re-parses the address.
    /// Reports a precise diagnostic and returns `None` when the place's node declares no
    /// keyed branch by that name.
    fn resolve_traversal_through_place<'e>(
        &mut self,
        place_base: &Expression,
        layer_name: &str,
        layer_span: SourceSpan,
        span: SourceSpan,
    ) -> Option<TraversalTarget<'e>> {
        let Expression::Name { segments, .. } = place_base else {
            return None;
        };
        let [name] = segments.as_slice() else {
            return None;
        };
        let place = self.lookup_place(name)?;
        // The place's key-path — evaluated once at its binding, including an entry-identity
        // operand captured into the root's key columns — is the traversal's ancestor path.
        // An identity-captured slot carries its root as a typed identity column, which the
        // bounded-traversal ancestor pop re-proves exactly as every other key-path pop does.
        let ancestor_keys = place.bound_keys();
        // The traversed branch is declared beneath the place's node — the same projection a
        // place field access uses. The node borrows the registry (`'a`), not `&self`, so a
        // diagnostic may still borrow `self` mutably.
        let node = self.place_node(place)?;
        let Some(branch) = node.branch(layer_name) else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                layer_span,
                node.no_branch_message(layer_name),
            ));
            return None;
        };
        let branch_entry_site = branch.entry_site;
        let branch_record = branch.record;
        let key_ty = self.single_traversal_column(&branch.key, span)?;
        Some(TraversalTarget {
            entry_site: branch_entry_site,
            key_ty,
            record: branch_record,
            node_kind: PlaceNodeKind::Branch,
            ancestor_keys,
            span,
        })
    }

    /// The single key column of a traversable layer, or a typed `check.unsupported` when
    /// the layer is composite-keyed. Bounded traversal binds one immediate key and takes
    /// one inclusive `from`; the current language spells no composite-key iteration, so a
    /// composite-keyed layer parks rather than inventing a last-column-under-prefix
    /// semantics.
    fn single_traversal_column(
        &mut self,
        columns: &[ScalarType],
        span: SourceSpan,
    ) -> Option<ScalarType> {
        match columns {
            [only] => Some(*only),
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "bounded traversal over a composite-keyed layer",
                ));
                None
            }
        }
    }

    /// Lower a bounded durable traversal `for k in <place> at most N [from f] on more`.
    /// Freeze the first `N` immediate keys of the traversed layer (after an inclusive
    /// `from`), run the body once per frozen key in order, then run the `on more` block
    /// when an `(N+1)`th key existed and every frozen body completed normally.
    fn lower_bounded_traversal(
        &mut self,
        binding: &ForBinding,
        target: TraversalTarget,
        bound: Option<&TraversalBound>,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        let Some(bound) = bound else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this durable traversal is unbounded. A `for` head over a durable root or \
                 branch is always bounded and states its overflow behavior. Add `at most N` \
                 and an `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        // A durable traversal binds the immediate key, and optionally a second name: a
        // per-iteration address pin (`place` semantics) over the entry at that key. More
        // than two names has no durable meaning.
        let (var, place_var) = match binding.names.as_slice() {
            [key] => (key, None),
            [key, address] => (key, Some(address)),
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "binding more than a key and a per-iteration address in a traversal",
                ));
                return Flow::Fallthrough;
            }
        };
        let Some(on_more) = &bound.on_more else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this bounded traversal has no overflow arm. A bounded `for` head states its \
                 overflow behavior in a trailing `on more` block. Add an `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        let Some(limit) = self.traversal_limit(&bound.limit) else {
            return Flow::Fallthrough;
        };
        let key_ty = target.key_ty;
        // The frozen keys materialize as one `List[K]`; mint (deduplicated) that row.
        let result = self
            .records
            .instantiate_list(self.draft, GArg::Scalar(key_ty));
        let Some(list_ty) = self.accept_resolution(result, span, "this bounded traversal result")
        else {
            return Flow::Rejected;
        };

        // Evaluate the ancestor key-path (root-first) then the inclusive `from` key, so
        // the opcode pops `from` (top) then the ancestor path. Keys are captured once,
        // before any body runs. A two-binding traversal captures the ancestor keys into
        // slots first — its per-iteration address pin reads them alongside the loop key —
        // then pushes the same slots as the opcode operands; a single-binding traversal
        // pushes the ancestor keys straight.
        let ancestor_slots: Vec<(u16, ScalarType)> = if place_var.is_some() {
            let mut slots = Vec::with_capacity(target.ancestor_keys.len());
            for column in &target.ancestor_keys {
                match column.key {
                    // A place/pin base supplies its ancestor columns as slots evaluated once
                    // at the place binding; the pin reuses those slots directly rather than
                    // re-evaluating the key.
                    PlaceKey::Bound(slot) => slots.push((slot, column.key_ty)),
                    // An inline `^root(k)….branch` base evaluates each ancestor key once here
                    // into a fresh slot, so the pin and the opcode read one evaluation.
                    PlaceKey::Expr(key_expr) => {
                        let slot = self.alloc_slot();
                        if self
                            .lower_as(key_expr, LTy::bare_scalar(column.key_ty))
                            .is_none()
                        {
                            return Flow::Fallthrough;
                        }
                        self.push(Instr::LocalSet(slot), target.span);
                        slots.push((slot, column.key_ty));
                    }
                    // An inline `^root[Id(…)].branch` base spreads the one identity operand
                    // into the addressed root's key columns, captured once into a slot per
                    // column (root-first). Each slot carries its root as a typed identity
                    // column the traversal ancestor pop re-proves, exactly as the single-emit
                    // forms do.
                    PlaceKey::Identity { expr, root, cols } => {
                        let Some(captured) =
                            self.capture_identity_key_slots(expr, root, cols, target.span)
                        else {
                            return Flow::Fallthrough;
                        };
                        #[allow(
                            clippy::expect_used,
                            reason = "lowering invariant: an identity operand's RootId names a root in this registry"
                        )]
                        let scalars = self
                            .durable
                            .root_by_id(root)
                            .expect("an identity operand's root is registered")
                            .key
                            .clone();
                        for (slot, scalar) in captured.into_iter().zip(scalars) {
                            slots.push((slot, scalar));
                        }
                    }
                }
            }
            for (slot, _) in &slots {
                self.push(Instr::LocalGet(*slot), target.span);
            }
            slots
        } else {
            if self
                .emit_key_path(&target.ancestor_keys, target.span)
                .is_none()
            {
                return Flow::Fallthrough;
            }
            Vec::new()
        };
        let has_from = bound.from.is_some();
        if let Some(from_expr) = &bound.from
            && self.lower_as(from_expr, LTy::bare_scalar(key_ty)).is_none()
        {
            return Flow::Fallthrough;
        }
        self.push(
            Instr::DurIterateBounded {
                site: target.entry_site,
                limit,
                from: has_from,
                list_ty,
            },
            span,
        );
        // Bind the on-more bit and the frozen list into fresh slots.
        let more_slot = self.alloc_slot();
        self.push(Instr::LocalSet(more_slot), span);
        let coll_slot = self.alloc_slot();
        self.push(Instr::LocalSet(coll_slot), span);

        // A positional walk over the frozen `List[K]` binds `k` per position.
        // `continue` advances to the loop top; a body `break`/`return` skips past
        // the `on more` block.
        let entry_site = target.entry_site;
        let record = target.record;
        let node_kind = target.node_kind;
        let key_name = var.name.clone();
        let place_name = place_var.map(|name| name.name.clone());
        let break_jumps = match self.lower_positional_walk(
            coll_slot,
            Instr::ListLen,
            body,
            span,
            move |lower, index_slot| {
                let key_slot = lower.alloc_slot();
                lower.push(Instr::LocalGet(coll_slot), span);
                lower.push(Instr::LocalGet(index_slot), span);
                lower.push(Instr::ListGet, span);
                // Rebinding the key slot each iteration kills, through the verifier's
                // LocalSet presence-lattice rule, any presence fact an earlier iteration
                // established on this key: a fact proven in iteration N cannot survive into
                // N+1.
                lower.push(Instr::LocalSet(key_slot), span);
                // Traversal establishes no presence fact for the body: `k` names a
                // frozen key whose entry an earlier body iteration may already have
                // erased.
                lower.locals.push(Local {
                    name: key_name,
                    ty: LTy::bare_scalar(key_ty),
                    mutable: false,
                    slot: key_slot,
                });
                // The optional second binding is a per-iteration address pin: a `place`
                // over the entry at the current key. Its key-path is the captured ancestor
                // slots followed by this iteration's key slot; it reads nothing and
                // establishes no presence fact, so a write through it is an ordinary
                // sparse set unless a dominating `exists` proves the entry present.
                if let Some(place_name) = place_name {
                    let mut key_slots = ancestor_slots;
                    key_slots.push((key_slot, key_ty));
                    lower.places.push(PlaceLocal {
                        name: place_name,
                        key_slots,
                        entry_site,
                        record,
                        node_kind,
                    });
                }
            },
        ) {
            PositionalWalkOutcome::Complete(break_jumps) => break_jumps,
            PositionalWalkOutcome::Rejected => return Flow::Rejected,
        };

        // Normal exhaustion falls through to here: run `on more` iff a further key
        // existed.
        self.push(Instr::LocalGet(more_slot), span);
        let skip_on_more = self.push_jif(span);
        if self.lower_block(on_more) == Flow::Rejected {
            return Flow::Rejected;
        }
        let end = self.here();
        self.patch(skip_on_more, end);
        // A body break jumps past the whole loop, skipping the `on more` decision.
        self.patch_all(break_jumps, end);
        Flow::Fallthrough
    }

    /// Lower a bounded scan of a nonunique managed index `^root.index[prefix…]`. The scan
    /// holds the index's leading field components as a prefix and yields the trailing
    /// identity component as the source `Id(^root)`, so the loop variable binds an
    /// identity: the frozen raw identity keys materialize as one `List[K]`, and each is
    /// wrapped into an `Id(^root)` at the binding. The scan requires a single-column
    /// identity root (so the yielded component is a whole identity) and does not admit a
    /// `from` cursor or a per-iteration address pin on this line.
    fn lower_index_scan(
        &mut self,
        binding: &ForBinding,
        read: IndexRead<'a, '_>,
        bound: Option<&TraversalBound>,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        let index = read.index;
        let keys = read.keys;
        let var = match binding.names.as_slice() {
            [key] => key,
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "binding a per-iteration address in an index scan",
                ));
                return Flow::Fallthrough;
            }
        };
        let Some(bound) = bound else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this index scan is unbounded. A `for` head over a managed index is always \
                 bounded and states its overflow behavior. Add `at most N` and an \
                 `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        let Some(on_more) = &bound.on_more else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this bounded scan has no overflow arm. A bounded `for` head states its \
                 overflow behavior in a trailing `on more` block. Add an `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        if bound.from.is_some() {
            self.fail(unsupported(
                self.file,
                span,
                "a `from` cursor on an index scan",
            ));
            return Flow::Fallthrough;
        }
        // The scan yields a whole source identity, so the root's identity is a single key
        // column; the scanned (trailing) projection component is that key. The scanned
        // index belongs to `read.root` (resolved with it), so its identity is that root's.
        let root = read.root;
        if root.key.len() != 1 {
            self.fail(unsupported(
                self.file,
                span,
                "an index scan over a composite-identity root",
            ));
            return Flow::Fallthrough;
        }
        let id_scalar = root.key[0];
        let site = index.site;
        // The held prefix is every projection component except the trailing identity key.
        let projection: Vec<ScalarType> = index.projection.clone();
        let Some((scanned, prefix_types)) = projection.split_last() else {
            self.fail(unsupported(self.file, span, "a scan over an empty index"));
            return Flow::Fallthrough;
        };
        if *scanned != id_scalar {
            self.fail(unsupported(
                self.file,
                span,
                "an index scan whose trailing component is not the source identity",
            ));
            return Flow::Fallthrough;
        }
        if keys.len() != prefix_types.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "index scan of `{}` holds its {} leading field component(s) as a prefix",
                    index.name,
                    prefix_types.len()
                ),
            ));
            return Flow::Fallthrough;
        }
        let Some(limit) = self.traversal_limit(&bound.limit) else {
            return Flow::Fallthrough;
        };
        // The frozen keys are the raw identity scalars; they materialize as `List[K]`.
        let result = self
            .records
            .instantiate_list(self.draft, GArg::Scalar(id_scalar));
        let Some(list_ty) = self.accept_resolution(result, span, "this index scan result") else {
            return Flow::Rejected;
        };
        // Emit the held prefix (leading field components, in projection order), then scan.
        for (key, key_ty) in keys.iter().zip(prefix_types) {
            if self.lower_as(key, LTy::bare_scalar(*key_ty)).is_none() {
                return Flow::Fallthrough;
            }
        }
        self.push(
            Instr::DurIndexScan {
                site,
                limit,
                from: false,
                list_ty,
            },
            span,
        );
        let more_slot = self.alloc_slot();
        self.push(Instr::LocalSet(more_slot), span);
        let coll_slot = self.alloc_slot();
        self.push(Instr::LocalSet(coll_slot), span);

        // A positional walk over the frozen `List[K]`: each raw identity key is wrapped
        // into the source `Id(^root)` the loop variable binds — the scanned root's identity.
        let key_name = var.name.clone();
        let scan_root_id = root.root_id;
        let break_jumps = match self.lower_positional_walk(
            coll_slot,
            Instr::ListLen,
            body,
            span,
            move |lower, index_slot| {
                let id_slot = lower.alloc_slot();
                lower.push(Instr::LocalGet(coll_slot), span);
                lower.push(Instr::LocalGet(index_slot), span);
                lower.push(Instr::ListGet, span);
                lower.push(
                    Instr::MakeIdentity {
                        root: scan_root_id,
                        cols: 1,
                    },
                    span,
                );
                lower.push(Instr::LocalSet(id_slot), span);
                lower.locals.push(Local {
                    name: key_name,
                    ty: LTy::Identity {
                        root: scan_root_id,
                        optional: false,
                    },
                    mutable: false,
                    slot: id_slot,
                });
            },
        ) {
            PositionalWalkOutcome::Complete(break_jumps) => break_jumps,
            PositionalWalkOutcome::Rejected => return Flow::Rejected,
        };

        self.push(Instr::LocalGet(more_slot), span);
        let skip_on_more = self.push_jif(span);
        if self.lower_block(on_more) == Flow::Rejected {
            return Flow::Rejected;
        }
        let end = self.here();
        self.patch(skip_on_more, end);
        self.patch_all(break_jumps, end);
        Flow::Fallthrough
    }

    /// Evaluate an `at most N` bound: a positive compile-time integer literal within
    /// `MAX_TRAVERSAL_BOUND`. A non-literal, non-positive, or oversized bound is a
    /// precise diagnostic.
    fn traversal_limit(&mut self, expr: &Expression) -> Option<u32> {
        let Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            span,
        } = expr
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                expr.span(),
                "`at most` requires a positive integer literal".to_string(),
            ));
            return None;
        };
        let value = parse_int(text).filter(|value| *value > 0);
        let Some(value) = value else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *span,
                "`at most N` requires a positive integer literal".to_string(),
            ));
            return None;
        };
        if value as u128 > u128::from(marrow_image::bounds::MAX_TRAVERSAL_BOUND) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *span,
                format!(
                    "`at most N` may not exceed {}",
                    marrow_image::bounds::MAX_TRAVERSAL_BOUND
                ),
            ));
            return None;
        }
        Some(value as u32)
    }

    /// Lower `for x in list` / `for k in map` / `for k, v in map`: a forward
    /// positional walk over a finite collection. A list yields elements in insertion
    /// order; a map yields keys (and values) in `CollectionKeyOrder`. The collection
    /// is evaluated once into a local, then indexed `0..length`; `continue` advances
    /// to the next position, `break` exits.
    fn lower_for_collection(
        &mut self,
        binding: &marrow_syntax::ForBinding,
        iterable: &Expression,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        let Some(coll_ty) = self.lower_expr(iterable) else {
            return Flow::Fallthrough;
        };
        let LTy::Collection {
            idx,
            optional: false,
        } = coll_ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                iterable.span(),
                format!(
                    "a `for` loop iterates a list, map, or store, found {}",
                    coll_ty.spelling(self.records)
                ),
            ));
            return Flow::Fallthrough;
        };

        // The loop variables and the per-position bind instructions, resolved from
        // the collection kind and binding arity.
        enum Bind {
            List { elem: LTy },
            MapKey { key: LTy },
            MapKeyValue { key: LTy, value: LTy },
        }
        let bind = match (self.records.collection_spec(idx), binding.names.as_slice()) {
            (CollSpec::List { elem }, [_var]) => Bind::List {
                elem: garg_to_lty(elem),
            },
            (CollSpec::Map { key, .. }, [_k]) => Bind::MapKey {
                key: garg_to_lty(key),
            },
            (CollSpec::Map { key, value }, [_k, _v]) => Bind::MapKeyValue {
                key: garg_to_lty(key),
                value: garg_to_lty(value),
            },
            (CollSpec::List { .. }, _) => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "a list `for` binds exactly one element name",
                ));
                return Flow::Fallthrough;
            }
            (CollSpec::Map { .. }, _) => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "a map `for` binds a key or a key and a value (`for k, v`)",
                ));
                return Flow::Fallthrough;
            }
        };

        // The collection value is on the stack; keep it in a local to index it.
        let coll_slot = self.alloc_slot();
        self.push(Instr::LocalSet(coll_slot), span);
        let len_instr = match self.records.collection_spec(idx) {
            CollSpec::List { .. } => Instr::ListLen,
            CollSpec::Map { .. } => Instr::MapLen,
        };

        let break_jumps = match self.lower_positional_walk(
            coll_slot,
            len_instr,
            body,
            span,
            |lower, index_slot| {
                // Bind the loop variable(s) from the current position.
                let bind_var = |lower: &mut Self, name: &str, ty: LTy, at: Instr| {
                    let slot = lower.alloc_slot();
                    lower.push(Instr::LocalGet(coll_slot), span);
                    lower.push(Instr::LocalGet(index_slot), span);
                    lower.push(at, span);
                    lower.push(Instr::LocalSet(slot), span);
                    lower.locals.push(Local {
                        name: name.to_string(),
                        ty,
                        mutable: false,
                        slot,
                    });
                };
                match bind {
                    Bind::List { elem } => {
                        bind_var(lower, &binding.names[0].name, elem, Instr::ListGet);
                    }
                    Bind::MapKey { key } => {
                        bind_var(lower, &binding.names[0].name, key, Instr::MapKeyAt);
                    }
                    Bind::MapKeyValue { key, value } => {
                        bind_var(lower, &binding.names[0].name, key, Instr::MapKeyAt);
                        bind_var(lower, &binding.names[1].name, value, Instr::MapValueAt);
                    }
                }
            },
        ) {
            PositionalWalkOutcome::Complete(break_jumps) => break_jumps,
            PositionalWalkOutcome::Rejected => return Flow::Rejected,
        };
        self.patch_all(break_jumps, self.here());
        Flow::Fallthrough
    }

    /// Lower a forward positional walk over a finite collection already resident in
    /// `coll_slot`. A `-1` cursor is incremented at the loop top, then an
    /// `index < len` guard (`len_instr` is the collection kind's length opcode)
    /// exits the loop; on each live position `bind` binds the loop variable(s) from
    /// the current index and pushes their [`Local`]s, then the body runs once.
    ///
    /// `continue` targets the increment at the loop top; the exhaustion exit is
    /// patched to fall through immediately after the loop, and the returned break
    /// jumps are left unpatched so the caller can route them past whatever trailing
    /// code it emits (a bounded traversal skips them past its `on more` block; a
    /// plain collection walk patches them to the same fall-through point). A
    /// terminal generic failure returns `Rejected` only after unwinding the loop and
    /// scoped bindings.
    fn lower_positional_walk(
        &mut self,
        coll_slot: u16,
        len_instr: Instr,
        body: &Block,
        span: SourceSpan,
        bind: impl FnOnce(&mut Self, u16),
    ) -> PositionalWalkOutcome {
        if self.terminal_rejection() {
            return PositionalWalkOutcome::Rejected;
        }
        // The cursor starts at -1 so the increment at the loop top reaches 0 first,
        // which lets `continue` jump to that increment and always make progress.
        let index_slot = self.alloc_slot();
        let neg_one = self.draft.intern_int(-1);
        self.push(Instr::ConstLoad(neg_one.index()), span);
        self.push(Instr::LocalSet(index_slot), span);
        let one = self.draft.intern_int(1);

        let top = self.here();
        // index += 1
        self.push(Instr::LocalGet(index_slot), span);
        self.push(Instr::ConstLoad(one.index()), span);
        self.push(Instr::IntAdd, span);
        self.push(Instr::LocalSet(index_slot), span);
        // index < length
        self.push(Instr::LocalGet(index_slot), span);
        self.push(Instr::LocalGet(coll_slot), span);
        self.push(len_instr, span);
        self.push(Instr::IntLt, span);
        let exit = self.push_jif(span);

        let mark = self.locals.len();
        let place_mark = self.places.len();
        bind(self, index_slot);
        self.loops.push(LoopCtx {
            continue_target: top,
            break_jumps: Vec::new(),
        });
        let body_flow = self.lower_block(body);
        #[expect(
            clippy::expect_used,
            reason = "lowering bookkeeping: this function pushed a loop context before lowering the body, so the paired pop returns it"
        )]
        let ctx = self.loops.pop().expect("loop was pushed");
        self.locals.truncate(mark);
        // A two-binding durable traversal binds a per-iteration address pin as a place;
        // drop it with the loop-variable locals so it does not escape the body.
        self.places.truncate(place_mark);
        if body_flow == Flow::Rejected {
            return PositionalWalkOutcome::Rejected;
        }
        if body_flow == Flow::Fallthrough {
            self.push(Instr::Jump(top as u32), body.span);
        }

        let after_loop = self.here();
        self.patch(exit, after_loop);
        PositionalWalkOutcome::Complete(ctx.break_jumps)
    }

    fn lower_while(&mut self, condition: &Expression, body: &Block) -> Flow {
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        let top = self.here();
        if self.lower_condition(condition).is_none() {
            return if self.terminal_rejection() {
                Flow::Rejected
            } else {
                Flow::Fallthrough
            };
        }
        let exit = self.push_jif(condition.span());
        self.loops.push(LoopCtx {
            continue_target: top,
            break_jumps: Vec::new(),
        });
        let body_flow = self.lower_block(body);
        #[expect(
            clippy::expect_used,
            reason = "lowering bookkeeping: this function pushed a loop context before lowering the body, so the paired pop returns it"
        )]
        let ctx = self.loops.pop().expect("loop was pushed");
        if body_flow == Flow::Rejected {
            return Flow::Rejected;
        }
        if body_flow == Flow::Fallthrough {
            self.push(Instr::Jump(top as u32), body.span);
        }
        let end = self.here();
        self.patch(exit, end);
        self.patch_all(ctx.break_jumps, end);
        Flow::Fallthrough
    }

    /// Lower the adjacent single-operation checked-arithmetic form. It wraps one int
    /// arithmetic operation; on a fault the diverging `on` arms run instead of the
    /// runtime raising `run.*`. Lowered to a checked op that branches to the
    /// out-of-range handler, with the zero divisor tested by an explicit branch
    /// before a checked `/`/`%`. The operands are evaluated into fresh locals so the
    /// checked op runs with exactly its two operands on the stack, leaving the fault
    /// edge at the statement-boundary (empty) stack.
    pub(super) fn lower_checked(
        &mut self,
        bind: &CheckedBind,
        op: &Expression,
        out_of_range: Option<&Block>,
        zero_divisor: Option<&Block>,
        span: SourceSpan,
    ) -> Flow {
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        // The wrapped operation: a single int `+`/`-`/`*`/`/`/`%` or negation.
        enum Wrapped<'e> {
            Binary(BinaryOp, &'e Expression, &'e Expression),
            Neg(&'e Expression),
        }
        let wrapped = match op {
            Expression::Binary {
                op:
                    bop @ (BinaryOp::Add
                    | BinaryOp::Subtract
                    | BinaryOp::Multiply
                    | BinaryOp::Divide
                    | BinaryOp::Remainder),
                left,
                right,
                ..
            } => Wrapped::Binary(*bop, left, right),
            Expression::Unary {
                op: UnaryOp::Neg,
                operand,
                ..
            } => Wrapped::Neg(operand),
            _ => {
                self.fail(unsupported(
                    self.file,
                    op.span(),
                    "a checked form wrapping anything but one int `+`, `-`, `*`, `/`, `%`, or negation",
                ));
                return Flow::Fallthrough;
            }
        };
        let is_div = matches!(
            wrapped,
            Wrapped::Binary(BinaryOp::Divide | BinaryOp::Remainder, _, _)
        );
        // A `/`/`%` whose divisor is a nonzero integer literal cannot fault with a zero
        // divisor: the fault is provably dead. This is literal-aware only — a non-literal
        // divisor is still assumed possibly zero. Overflow stays possible regardless (the
        // `i64::MIN / -1` case), so the `out_of_range` arm is untouched.
        let divisor_provably_nonzero = matches!(
            &wrapped,
            Wrapped::Binary(_, _, right) if divisor_nonzero_literal(right)
        );
        let can_zero_fault = is_div && !divisor_provably_nonzero;

        // Arm requirements: out_of_range is always possible; a zero divisor is possible
        // only for a `/`/`%` whose divisor is not a provably-nonzero literal.
        let Some(out_of_range) = out_of_range else {
            self.fail(checked_arm_error(
                self.file,
                span,
                "requires an `on out_of_range` arm",
            ));
            return Flow::Fallthrough;
        };
        if can_zero_fault && zero_divisor.is_none() {
            self.fail(checked_arm_error(
                self.file,
                span,
                "a checked `/` or `%` requires an `on zero_divisor` arm",
            ));
            return Flow::Fallthrough;
        }
        if !can_zero_fault && zero_divisor.is_some() {
            let reason = if is_div {
                "the divisor is a nonzero literal, so this checked operation cannot fault with a zero divisor and takes no `on zero_divisor` arm"
            } else {
                "this checked operation cannot fault with a zero divisor, so it takes no `on zero_divisor` arm"
            };
            self.fail(checked_arm_error(self.file, span, reason));
            return Flow::Fallthrough;
        }

        let int = LTy::bare_scalar(ScalarType::Int);
        // Evaluate the operands into fresh locals.
        let la = self.alloc_slot();
        let (checked, lb) = match wrapped {
            Wrapped::Binary(bop, left, right) => {
                if self.lower_as(left, int).is_none() {
                    return if self.terminal_rejection() {
                        Flow::Rejected
                    } else {
                        Flow::Fallthrough
                    };
                }
                self.push(Instr::LocalSet(la), span);
                let lb = self.alloc_slot();
                if self.lower_as(right, int).is_none() {
                    return if self.terminal_rejection() {
                        Flow::Rejected
                    } else {
                        Flow::Fallthrough
                    };
                }
                self.push(Instr::LocalSet(lb), span);
                let checked = match bop {
                    BinaryOp::Add => Instr::IntAddChecked(0),
                    BinaryOp::Subtract => Instr::IntSubChecked(0),
                    BinaryOp::Multiply => Instr::IntMulChecked(0),
                    BinaryOp::Divide => Instr::IntDivChecked(0),
                    BinaryOp::Remainder => Instr::IntRemChecked(0),
                    #[expect(
                        clippy::unreachable,
                        reason = "match-arm narrowing: the checker admitted only these checked-arithmetic binary operators to this lowering path"
                    )]
                    _ => unreachable!("classified as an admitted binary op"),
                };
                (checked, Some(lb))
            }
            Wrapped::Neg(operand) => {
                if self.lower_as(operand, int).is_none() {
                    return if self.terminal_rejection() {
                        Flow::Rejected
                    } else {
                        Flow::Fallthrough
                    };
                }
                self.push(Instr::LocalSet(la), span);
                (Instr::IntNegChecked(0), None)
            }
        };

        // A checked `/`/`%` tests its divisor first; a zero divisor runs the diverging
        // `on zero_divisor` arm. A provably-nonzero literal divisor has no such arm and
        // needs no runtime test — the operation cannot reach a zero divisor.
        if is_div && let Some(zero_block) = zero_divisor {
            #[expect(
                clippy::expect_used,
                reason = "parser-guaranteed shape: a division parses with a right operand, so its lowered slot is bound whenever a zero-divisor arm is present"
            )]
            let lb = lb.expect("division has a right operand");
            self.push(Instr::LocalGet(lb), span);
            let zero = self.draft.intern_int(0);
            self.push(Instr::ConstLoad(zero.index()), span);
            self.push(Instr::EqInt, span);
            let to_nonzero = self.push_jif(span);
            let zero_flow = self.lower_block(zero_block);
            if zero_flow == Flow::Rejected {
                return Flow::Rejected;
            }
            if zero_flow != Flow::Terminates {
                self.fail(checked_arm_error(
                    self.file,
                    zero_block.span,
                    "an `on zero_divisor` arm must diverge (every path must return, break, continue, throw, or be unreachable)",
                ));
            }
            let nonzero = self.here();
            self.patch(to_nonzero, nonzero);
        }

        // The checked operation. On the fault edge it transfers to the handler with
        // the operands already popped (the statement-boundary stack); on success it
        // pushes the int result.
        self.push(Instr::LocalGet(la), span);
        if let Some(lb) = lb {
            self.push(Instr::LocalGet(lb), span);
        }
        let checked_at = self.here();
        self.push(checked, span);

        // Success path: coerce the int result to the binding and store it. A
        // `const`/`var` binding (`pending` is `Some`) falls through and jumps over the
        // handler; a `return` binding leaves the frame, so no skip jump is needed.
        let pending = self.store_checked_result(bind, span);
        if self.terminal_rejection() {
            return Flow::Rejected;
        }
        let end_jump = pending.is_some().then(|| self.push_jump(span));

        // The out-of-range handler.
        let handler = self.here();
        self.patch(checked_at, handler);
        let out_of_range_flow = self.lower_block(out_of_range);
        if out_of_range_flow == Flow::Rejected {
            return Flow::Rejected;
        }
        if out_of_range_flow != Flow::Terminates {
            self.fail(checked_arm_error(
                self.file,
                out_of_range.span,
                "an `on out_of_range` arm must diverge (every path must return, break, continue, throw, or be unreachable)",
            ));
        }

        // The binding is in scope only after the whole form, on the success path.
        if let Some(end_jump) = end_jump {
            let end = self.here();
            self.patch(end_jump, end);
        }
        if let Some(local) = pending {
            self.locals.push(local);
            Flow::Fallthrough
        } else {
            // A `return` binding leaves the frame on the success path; the arms
            // diverge, so the whole form terminates.
            Flow::Terminates
        }
    }

    /// Emit the store of a checked form's int result into its binding, on the success
    /// path. Returns the local to bring into scope *after* the handler (for
    /// `const`/`var`, so the name is not visible inside the arms), or `None` for a
    /// `return` binding (which stores by returning).
    fn store_checked_result(&mut self, bind: &CheckedBind, span: SourceSpan) -> Option<Local> {
        let int = LTy::bare_scalar(ScalarType::Int);
        match bind {
            CheckedBind::Const { name, ty } | CheckedBind::Var { name, ty } => {
                let mutable = matches!(bind, CheckedBind::Var { .. });
                let target = self.coerce_int_result(ty.as_ref(), int, span)?;
                let slot = self.alloc_slot();
                self.push(Instr::LocalSet(slot), span);
                Some(Local {
                    name: name.clone(),
                    ty: target,
                    mutable,
                    slot,
                })
            }
            CheckedBind::Return => {
                match self.ret {
                    RetType::Value(want) => {
                        self.coerce_bare_int_to(want, span, span);
                        self.emit_region_return(span);
                    }
                    RetType::Unit => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            span,
                            "this function returns nothing, so it cannot `return checked`"
                                .to_string(),
                        ));
                    }
                }
                None
            }
        }
    }

    /// Coerce the bare-int checked result to a `const`/`var` annotation (`int` or
    /// `int?`), emitting a `SomeWrap` for the optional case. An annotation that is not
    /// int-compatible is a type error; a missing annotation infers `int`.
    fn coerce_int_result(
        &mut self,
        annotation: Option<&TypeExpr>,
        int: LTy,
        span: SourceSpan,
    ) -> Option<LTy> {
        let Some(annotation) = annotation else {
            return Some(int);
        };
        let declared = match self.resolve(annotation) {
            Ok(declared) => declared,
            Err(ResolveError::Refusal(ResolveRefusal::Limit)) => {
                self.failed = true;
                return None;
            }
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                self.fail(unsupported(
                    self.file,
                    annotation.span(),
                    "this type annotation",
                ));
                return Some(int);
            }
            Err(ResolveError::Invariant(invariant)) => {
                self.reject_resolution(
                    ResolveError::Invariant(invariant),
                    annotation.span(),
                    "this type annotation",
                );
                return None;
            }
        };
        self.coerce_bare_int_to(declared, annotation.span(), span);
        Some(declared)
    }

    /// Coerce the bare-int result already on the stack to `target` (`int` or `int?`),
    /// emitting a `SomeWrap` for the optional case. A `target` that is not
    /// int-compatible is a type error reported at `err_span`. One owner for the two
    /// checked-result binding sites (`const`/`var` annotation and `return`).
    fn coerce_bare_int_to(&mut self, target: LTy, err_span: SourceSpan, wrap_span: SourceSpan) {
        let int = LTy::bare_scalar(ScalarType::Int);
        if target == int.to_optional() {
            self.push(Instr::SomeWrap, wrap_span);
        } else if target != int {
            self.fail(type_mismatch(
                self.records,
                self.file,
                err_span,
                int,
                target,
            ));
        }
    }

    fn lower_condition(&mut self, expr: &Expression) -> Option<()> {
        let ty = self.lower_expr(expr)?;
        if ty != LTy::bare_scalar(ScalarType::Bool) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                expr.span(),
                format!(
                    "condition must be bool, found {}",
                    ty.spelling(self.records)
                ),
            ));
            return None;
        }
        Some(())
    }
}
