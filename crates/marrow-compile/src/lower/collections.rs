//! Lowering for the built-in `List`/`Map` collection floor: construction, the bracket
//! read/write/unset forms over a local collection, and the length/append/emptiness
//! builtins. None of it names a `^` place.

use super::*;

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Lower an empty-collection constructor `List()`/`Map()` against the expected
    /// type: the expected `Collection` supplies the exact instantiation, so the
    /// constructor emits the `ListNew`/`MapNew` for that COLLTYPES index. A `List()`
    /// against a `Map` type (or the reverse), or against a non-collection type, is a
    /// typed diagnostic.
    /// Lower a collection constructor directed by an expected `List`/`Map` type. An
    /// empty `List()`/`Map()` mints the fresh collection; a variadic `List(a, b, c)`
    /// mints the list and then writes each element in order as a visible append. The
    /// map literal is deferred, so `Map(...)` with arguments is refused.
    pub(super) fn lower_collection_ctor(
        &mut self,
        head: &str,
        args: &[Argument],
        span: SourceSpan,
        expected: LTy,
    ) -> ConstructResult<()> {
        let LTy::Collection {
            idx,
            optional: false,
        } = expected
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!(
                    "`{head}()` constructs a collection, but {} is expected here",
                    expected.spelling(self.records)
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        match (head, self.records.collection_spec(idx)) {
            ("List", CollSpec::List { elem }) => {
                self.push(Instr::ListNew(idx), span)?;
                let elem = garg_to_lty(elem);
                self.append_list_elements(args, elem, span)
            }
            ("Map", CollSpec::Map { .. }) => {
                if !args.is_empty() {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType,
                        self.file,
                        span,
                        "a map is constructed empty with `Map()` and filled with `m[k] = v`; \
                         a map literal is not yet available"
                            .to_string(),
                    ));
                    return Err(LoweringFailure::Recoverable);
                }
                self.push(Instr::MapNew(idx), span)?;
                Ok(())
            }
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    self.file,
                    span,
                    format!(
                        "`{head}()` does not construct {}",
                        self.records.collection_spelling(idx)
                    ),
                ));
                Err(LoweringFailure::Recoverable)
            }
        }
    }

    /// Write each argument of a variadic `List(...)` as a visible element append, in
    /// source order, onto the freshly minted list already on the stack. The arity is
    /// lexical — one append per argument, no hidden loop — and each element is typed by
    /// the list's element type. A named argument is not a list element.
    fn append_list_elements(
        &mut self,
        args: &[Argument],
        elem: LTy,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        for arg in args {
            if arg.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    self.file,
                    span,
                    "`List(...)` takes positional element values, not named arguments".to_string(),
                ));
                return Err(LoweringFailure::Recoverable);
            }
            self.lower_as(&arg.value, elem)?;
            self.push(Instr::ListAppend, span)?;
        }
        Ok(())
    }

    /// Lower a variadic `List(a, b, c)` with no expected type: the element type is
    /// inferred from the first argument and every later argument is checked against it.
    /// The elements evaluate left to right into locals so the minted list can be filled
    /// in source order once its element type is known.
    pub(super) fn lower_list_literal_inferred(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        #[expect(
            clippy::unreachable,
            reason = "match-arm narrowing: the caller dispatches here only for a builtin whose non-empty argument list it already established"
        )]
        let [first, rest @ ..] = args else {
            unreachable!("caller passes a non-empty argument list");
        };
        if first.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "`List(...)` takes positional element values, not named arguments".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let elem = self.lower_expr(&first.value)?;
        let Some(elem_garg) = elem.as_garg() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                first.value.span(),
                format!(
                    "a list element is a value type, found {}",
                    elem.spelling(self.records)
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let mut slots = Vec::with_capacity(args.len());
        let first_slot = self
            .alloc_slot(first.value.span())
            .ok_or(LoweringFailure::Recoverable)?;
        self.push(Instr::LocalSet(first_slot), span)?;
        slots.push(first_slot);
        for arg in rest {
            if arg.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    self.file,
                    span,
                    "`List(...)` takes positional element values, not named arguments".to_string(),
                ));
                return Err(LoweringFailure::Recoverable);
            }
            self.lower_as(&arg.value, elem)?;
            let slot = self
                .alloc_slot(arg.value.span())
                .ok_or(LoweringFailure::Recoverable)?;
            self.push(Instr::LocalSet(slot), span)?;
            slots.push(slot);
        }
        let result = self.records.instantiate_list(self.draft, elem_garg);
        let idx = self
            .accept_resolution(result, span, "this list literal")
            .ok_or(LoweringFailure::Recoverable)?;
        self.push(Instr::ListNew(idx), span)?;
        for slot in slots {
            self.push(Instr::LocalGet(slot), span)?;
            self.push(Instr::ListAppend, span)?;
        }
        Ok(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower `isEmpty(x)` over a string or a finite collection. A string routes to
    /// the text floor; a `List`/`Map` lowers to `length(x) == 0`.
    pub(super) fn lower_is_empty(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let [arg] = args else {
            self.fail(builtin_arity(self.file, span, "isEmpty", 1));
            return Err(LoweringFailure::Recoverable);
        };
        if arg.name.is_some() {
            self.fail(builtin_arity(self.file, span, "isEmpty", 1));
            return Err(LoweringFailure::Recoverable);
        }
        let ty = self.lower_expr(&arg.value)?;
        match ty {
            LTy::Scalar {
                scalar: ScalarType::Text,
                optional: false,
            } => {
                self.push(Instr::TextIsEmpty, span)?;
                Ok(LTy::bare_scalar(ScalarType::Bool))
            }
            LTy::Collection {
                idx,
                optional: false,
            } => {
                let len = match self.records.collection_spec(idx) {
                    CollSpec::List { .. } => Instr::ListLen,
                    CollSpec::Map { .. } => Instr::MapLen,
                };
                self.push(len, span)?;
                let zero = self
                    .checked_mint(|draft| draft.intern_int(0))
                    .ok_or(LoweringFailure::Recoverable)?;
                self.push(Instr::ConstLoad(zero), span)?;
                self.push(Instr::EqInt, span)?;
                Ok(LTy::bare_scalar(ScalarType::Bool))
            }
            _ => {
                self.fail(unsupported(
                    self.file,
                    arg.value.span(),
                    "`isEmpty` on this type (it accepts a string, list, or map)",
                ));
                Err(LoweringFailure::Recoverable)
            }
        }
    }

    /// Lower `length(x): int` over a finite collection: the element or entry count.
    /// Lower a local bracket read `xs[i]` / `m[k]`: the base is a local collection and
    /// the read yields the presence-typed optional (`T?` for a list element, `V?` for a
    /// map value), joining the same presence family as sparse durable reads. A list
    /// position is a 1-based key; the literal dead indexes `xs[0]` and `xs[-1]` are
    /// refused with a teaching diagnostic, while a computed out-of-range index yields
    /// absent — Marrow has no out-of-bounds fault class. A `Map<int, V>` key `0` is a
    /// legitimate key, not a dead index.
    pub(super) fn lower_local_bracket_read(
        &mut self,
        base: &Expression,
        keys: &[Expression],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let base_ty = self.lower_expr(base)?;
        let LTy::Collection {
            idx,
            optional: false,
        } = base_ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                base.span(),
                format!(
                    "a bracket lookup needs a list or map, found {}",
                    base_ty.spelling(self.records)
                ),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let [key] = keys else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "a local bracket lookup takes exactly one key".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        match self.records.collection_spec(idx) {
            CollSpec::List { elem } => {
                if let Some(index_text) = dead_list_index_literal(key) {
                    let label = simple_base_label(base);
                    let message = match label {
                        Some(name) => format!(
                            "`{name}[{index_text}]` names no list position. List positions \
                             count from 1; the first element is `{name}[1]`"
                        ),
                        None => format!(
                            "`[{index_text}]` names no list position. List positions count \
                             from 1; the first element is at position 1"
                        ),
                    };
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType,
                        self.file,
                        key.span(),
                        message,
                    ));
                    return Err(LoweringFailure::Recoverable);
                }
                self.lower_as(key, LTy::bare_scalar(ScalarType::Int))?;
                self.push(Instr::ListIndex, span)?;
                Ok(garg_to_lty(elem).to_optional())
            }
            CollSpec::Map { key: key_ty, value } => {
                self.lower_as(key, garg_to_lty(key_ty))?;
                self.push(Instr::MapGet, span)?;
                Ok(garg_to_lty(value).to_optional())
            }
        }
    }

    /// Lower a local keyed write `m[k] = value`: on a `var` map binding, create or
    /// replace the value at the key (total, except the `run.collection_limit` growth
    /// fault), lowered as a read-modify-write with value semantics — the same shape as
    /// a durable keyed write, differing only by the absent `^`. A `const` binding gets
    /// the ordinary assignment-to-const rejection. A list has no keyed write: `xs[i] =
    /// value` is refused with a teaching diagnostic naming `append` and `Map<int, T>`.
    /// One bracket group on a bare local binding; a nested or compound base is deferred.
    pub(super) fn lower_local_bracket_write(
        &mut self,
        base: &Expression,
        keys: &[Expression],
        span: SourceSpan,
        value: &Expression,
    ) -> ConstructResult<()> {
        let Expression::Name {
            segments,
            span: base_span,
            ..
        } = base
        else {
            self.fail(unsupported(
                self.file,
                base.span(),
                "this assignment target",
            ));
            return Ok(());
        };
        let [name] = &segments[..] else {
            self.fail(unsupported(self.file, *base_span, "this assignment target"));
            return Ok(());
        };
        let name = name.text();
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *base_span, name));
            return Ok(());
        };
        let (slot, ty, mutable) = (local.slot, local.ty, local.mutable);
        let LTy::Collection {
            idx,
            optional: false,
        } = ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                *base_span,
                format!(
                    "a bracket assignment needs a list or map, found {}",
                    ty.spelling(self.records)
                ),
            ));
            return Ok(());
        };
        let [key] = keys else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "a local bracket assignment takes exactly one key".to_string(),
            ));
            return Ok(());
        };
        match self.records.collection_spec(idx) {
            CollSpec::Map {
                key: key_ty,
                value: value_ty,
            } => {
                if !mutable {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType,
                        self.file,
                        *base_span,
                        format!("`{name}` is a `const` and cannot be reassigned"),
                    ));
                    return Ok(());
                }
                self.push(Instr::LocalGet(slot), span)?;
                self.lower_as(key, garg_to_lty(key_ty))?;
                self.lower_as(value, garg_to_lty(value_ty))?;
                self.push(Instr::MapInsert, span)?;
                self.push(Instr::LocalSet(slot), span)?;
            }
            CollSpec::List { elem } => {
                let rhs = simple_value_spelling(value).unwrap_or_else(|| "_".to_string());
                self.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    self.file,
                    span,
                    format!(
                        "`{name}` is a list, and a list has no keyed write. Grow it with \
                         `append({name}, {rhs})`, or use a `Map<int, {}>` for replacement at a \
                         position",
                        garg_to_lty(elem).spelling(self.records)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Lower `unset m[k]`: remove a key from a local map, idempotent on an absent key.
    /// The base names a mutable local map; the key is coerced to the map key type and a
    /// `MapRemove` read-modify-writes the local. A list has no keyed removal — a dense
    /// list holds no holes — so `unset xs[i]` is refused with a teaching diagnostic.
    pub(super) fn lower_local_bracket_unset(
        &mut self,
        base: &Expression,
        keys: &[Expression],
        span: SourceSpan,
    ) -> ConstructResult<()> {
        let Expression::Name {
            segments,
            span: base_span,
            ..
        } = base
        else {
            self.fail(unsupported(self.file, base.span(), "this `unset` target"));
            return Ok(());
        };
        let [name] = &segments[..] else {
            self.fail(unsupported(self.file, *base_span, "this `unset` target"));
            return Ok(());
        };
        let name = name.text();
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *base_span, name));
            return Ok(());
        };
        let (slot, ty, mutable) = (local.slot, local.ty, local.mutable);
        let LTy::Collection {
            idx,
            optional: false,
        } = ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                *base_span,
                format!(
                    "a bracket removal needs a map, found {}",
                    ty.spelling(self.records)
                ),
            ));
            return Ok(());
        };
        let [key] = keys else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "a local bracket removal takes exactly one key".to_string(),
            ));
            return Ok(());
        };
        match self.records.collection_spec(idx) {
            CollSpec::Map { key: key_ty, .. } => {
                if !mutable {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType,
                        self.file,
                        *base_span,
                        format!("`{name}` is a `const` and cannot be modified"),
                    ));
                    return Ok(());
                }
                self.push(Instr::LocalGet(slot), span)?;
                self.lower_as(key, garg_to_lty(key_ty))?;
                self.push(Instr::MapRemove, span)?;
                self.push(Instr::LocalSet(slot), span)?;
            }
            CollSpec::List { elem } => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    self.file,
                    span,
                    format!(
                        "`{name}` is a list, and a list has no keyed removal — a dense list \
                         holds no holes. Use a `Map<int, {}>` when a position may be removed",
                        garg_to_lty(elem).spelling(self.records)
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn lower_length(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let [arg] = args else {
            self.fail(builtin_arity(self.file, span, "length", 1));
            return Err(LoweringFailure::Recoverable);
        };
        if arg.name.is_some() {
            self.fail(builtin_arity(self.file, span, "length", 1));
            return Err(LoweringFailure::Recoverable);
        }
        let idx = self.collection_arg(&arg.value)?;
        let len = match self.records.collection_spec(idx) {
            CollSpec::List { .. } => Instr::ListLen,
            CollSpec::Map { .. } => Instr::MapLen,
        };
        self.push(len, span)?;
        Ok(LTy::bare_scalar(ScalarType::Int))
    }

    /// Lower `append(list, value): List<T>`: append `value` after the last element,
    /// yielding the grown list (collections are values). A non-list first argument,
    /// or a `value` not of the element type, is a typed diagnostic.
    pub(super) fn lower_append(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let [list_arg, value_arg] = args else {
            self.fail(builtin_arity(self.file, span, "append", 2));
            return Err(LoweringFailure::Recoverable);
        };
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(builtin_arity(self.file, span, "append", 2));
            return Err(LoweringFailure::Recoverable);
        }
        let idx = self.collection_arg(&list_arg.value)?;
        let CollSpec::List { elem } = self.records.collection_spec(idx) else {
            self.fail(unsupported(
                self.file,
                list_arg.value.span(),
                "`append` on a map (a map is updated with `insert`)",
            ));
            return Err(LoweringFailure::Recoverable);
        };
        self.lower_as(&value_arg.value, garg_to_lty(elem))?;
        self.push(Instr::ListAppend, span)?;
        Ok(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower an expression that must be a bare collection, returning its COLLTYPES
    /// index. A non-collection value is a typed diagnostic.
    pub(super) fn collection_arg(&mut self, expr: &Expression) -> ConstructResult<CollTypeId> {
        let ty = self.lower_expr(expr)?;
        match ty {
            LTy::Collection {
                idx,
                optional: false,
            } => Ok(idx),
            other => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    self.file,
                    expr.span(),
                    format!(
                        "expected a list or map here, found {}",
                        other.spelling(self.records)
                    ),
                ));
                Err(LoweringFailure::Recoverable)
            }
        }
    }
}
