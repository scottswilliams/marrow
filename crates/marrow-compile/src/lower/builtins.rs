//! Built-in and constructor classification: the `Builtin`/`CtorKind` vocabulary and the free classifiers over call syntax.

use super::*;

/// The bare lowered type a built-in generic argument denotes (the inverse of
/// [`LTy::as_garg`] over the value cases).
pub(super) fn garg_to_lty(arg: GArg) -> LTy {
    match arg {
        GArg::Scalar(scalar) => LTy::bare_scalar(scalar),
        GArg::Nominal(id) => LTy::Nominal {
            id,
            optional: false,
        },
        GArg::Struct(ty) => LTy::Struct {
            ty,
            optional: false,
        },
        // A group value materializes as a nested record; its leaves resolve through
        // the group owner (`group_by_type`), reached from the record field path.
        GArg::Group(ty) => LTy::Record {
            ty,
            optional: false,
        },
        GArg::Enum(ty) => LTy::Enum {
            ty,
            optional: false,
        },
        GArg::Collection(idx) => LTy::Collection {
            idx,
            optional: false,
        },
        GArg::Param(index) => LTy::Param {
            index,
            optional: false,
        },
    }
}

/// One constructor member plan entry: a field or group leaf's name, value type, and
/// required flag, collected before emission so evaluation follows declaration order.
pub(super) type MemberPlan = (String, GArg, bool);

/// One group slot's constructor plan: the group's name, its materialized-value
/// record type, whether it has a required leaf (so an omitted argument cannot be
/// auto-completed), and the plan of its leaves.
pub(super) type GroupPlan = (String, TypeId, bool, Vec<MemberPlan>);

/// The source spelling of a built-in generic argument, recursing through nested
/// `Option`/`Result` arguments.
pub(super) fn garg_spelling(arg: GArg, records: &TypeRegistry) -> String {
    garg_to_lty(arg).spelling(records)
}

/// A built-in `Option`/`Result` constructor form in expression position. The
/// constructor names are reserved, so any `none`, `some(_)`, `ok(_)`, or `err(_)`
/// is this built-in rather than a name or call the surrounding scope resolves.
#[derive(Debug, Clone, Copy)]
pub(super) enum CtorKind {
    None,
    Some,
    Ok,
    Err,
}

impl CtorKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            CtorKind::None => "none",
            CtorKind::Some => "some",
            CtorKind::Ok => "ok",
            CtorKind::Err => "err",
        }
    }
}

/// A value-level built-in the compiler intercepts before user resolution: the
/// `Option`/`Result` constructors (`none`/`some`/`ok`/`err`), the presence test
/// (`exists`), the divergence marker (`unreachable`), and the pure text floor
/// (`isEmpty`/`contains`/`trim`/`split`/`lines`/`join`). None of these spellings is
/// a keyword, so the parser admits them as identifiers; the reservation is enforced
/// here instead.
///
/// This enum is the single owner of that name set. Call interception dispatches
/// on `from_name` (see `lower_unqualified_call`), and declaration rejection
/// consults the same classifier through [`is_reserved_builtin_name`], so a name
/// that is intercepted at a use site can never be silently shadowed by a
/// colliding value declaration. Adding a built-in is a new variant, which the
/// exhaustive dispatch match forces every consumer to account for.
#[derive(Debug, Clone, Copy)]
pub(super) enum Builtin {
    None,
    Some,
    Ok,
    Err,
    Exists,
    Unreachable,
    Todo,
    IsEmpty,
    Contains,
    Trim,
    /// The collection-returning text floor: `split(text, sep): List[string]`,
    /// `lines(text): List[string]`, `join(List[string], sep): string`. Like the rest
    /// of the floor these are reserved, so a colliding value declaration is rejected;
    /// they mint the `List[string]` COLLTYPES instantiation their result or argument
    /// names.
    Split,
    Lines,
    Join,
    /// The named temporal arithmetic floor: `addDays(date, int): date` and
    /// `daysBetween(date, date): int`. Named rather than operators so a date
    /// offset never reads as an ambiguous `date + int`; they are reserved, so a
    /// colliding value declaration is rejected. `marrow-temporal` owns the checked
    /// operations, which fault `run.temporal_overflow` past the supported range.
    DateAddDays,
    DateDaysBetween,
    /// The empty-collection constructors `List()`/`Map()`, type-directed by the
    /// expected type. They are reserved (blocking a colliding value declaration)
    /// because a bare `List`/`Map` at a use site is always the built-in constructor.
    /// The procedural collection operations (`append`/`insert`/`get`/`length`) are
    /// deliberately *not* reserved: they are common verbs, so a same-module function
    /// of that name wins and the collection op is a fallback (see
    /// [`FnLowerer::lower_collection_fallback`]).
    List,
    Map,
    /// The entry-identity constructor `Id(^root, keys…)`: a nominal value constructor
    /// wrapping the explicit key tuple as an `Id(^root)`. Reserved so a colliding value
    /// declaration is rejected; the leading `^root` argument is a saved-root reference,
    /// not an ordinary value, so it is dispatched to its own lowering.
    Id,
    /// The integer-domain bounds `maxInt` (`i64::MAX`) and `minInt` (`i64::MIN`). The
    /// owner ruling is that no source spells `9223372036854775807`; the language names
    /// the bound instead. Unlike every other variant these are argument-free *values*,
    /// not calls: a bare use folds to a constant `int` load ([`Builtin::const_int_value`]),
    /// and a call form is rejected. They are reserved (blocking a colliding declaration)
    /// so a bare `maxInt`/`minInt` is always the bound.
    MaxInt,
    MinInt,
}

impl Builtin {
    /// Every built-in variant, in declaration order. This is the single registry the
    /// classifier ([`Builtin::from_name`]) and the editor completion namespace
    /// ([`builtin_value_names`]) both derive from, so the two can never disagree about
    /// which names are built-in. A new built-in is added here and given a
    /// [`Builtin::spelling`]; the exhaustive spelling match rejects a variant that is
    /// added to the enum without a spelling.
    const ALL: [Builtin; 20] = [
        Builtin::None,
        Builtin::Some,
        Builtin::Ok,
        Builtin::Err,
        Builtin::Exists,
        Builtin::Unreachable,
        Builtin::Todo,
        Builtin::IsEmpty,
        Builtin::Contains,
        Builtin::Trim,
        Builtin::Split,
        Builtin::Lines,
        Builtin::Join,
        Builtin::DateAddDays,
        Builtin::DateDaysBetween,
        Builtin::List,
        Builtin::Map,
        Builtin::Id,
        Builtin::MaxInt,
        Builtin::MinInt,
    ];

    /// The reserved source spelling of this built-in. The exhaustive match makes a new
    /// variant a compile error here until it declares its spelling.
    pub(super) fn spelling(self) -> &'static str {
        match self {
            Builtin::None => "none",
            Builtin::Some => "some",
            Builtin::Ok => "ok",
            Builtin::Err => "err",
            Builtin::Exists => "exists",
            Builtin::Unreachable => "unreachable",
            Builtin::Todo => "todo",
            Builtin::IsEmpty => "isEmpty",
            Builtin::Contains => "contains",
            Builtin::Trim => "trim",
            Builtin::Split => "split",
            Builtin::Lines => "lines",
            Builtin::Join => "join",
            Builtin::DateAddDays => "addDays",
            Builtin::DateDaysBetween => "daysBetween",
            Builtin::List => "List",
            Builtin::Map => "Map",
            Builtin::Id => "Id",
            Builtin::MaxInt => "maxInt",
            Builtin::MinInt => "minInt",
        }
    }

    pub(super) fn from_name(name: &str) -> Option<Self> {
        Builtin::ALL
            .into_iter()
            .find(|builtin| builtin.spelling() == name)
    }

    /// The `i64` an argument-free integer-bound built-in denotes, or `None` for a
    /// built-in that is a call or constructor rather than a value bound. A bare use in
    /// value position folds to a constant load of this value, and a constant
    /// initializer folds to the same; no source spells the literal.
    pub(super) fn const_int_value(self) -> Option<i64> {
        match self {
            Builtin::MaxInt => Some(i64::MAX),
            Builtin::MinInt => Some(i64::MIN),
            _ => None,
        }
    }
}

/// Whether `name` is a reserved value-level built-in that a `fn`, `const`,
/// parameter, or local binding may not redeclare. A colliding value declaration
/// would be admitted and then silently shadowed at every use site the compiler
/// intercepts (`some(v)`, bare `none`, `trim(s)`, ...), surfacing later as a
/// confusing type error; rejecting the declaration keeps the reserved name and
/// its interception the single fact.
///
/// Struct fields and enum variants are excluded: both are reached only through
/// member syntax (`r.none`, `Color::err`), never a bare or unqualified-call use,
/// so they cannot collide with an intercepted built-in.
pub(crate) fn is_reserved_builtin_name(name: &str) -> bool {
    Builtin::from_name(name).is_some()
}

/// The value-level built-in spellings, in declaration order, for the editor completion
/// namespace. Derived from the single [`Builtin::ALL`] registry the classifier also uses,
/// so the completion namespace is exactly the set [`Builtin::from_name`] recognizes.
pub(crate) fn builtin_value_names() -> Vec<&'static str> {
    Builtin::ALL
        .into_iter()
        .map(|builtin| builtin.spelling())
        .collect()
}

/// The `i64` the bare name denotes as an integer-bound value built-in (`maxInt`/
/// `minInt`), or `None` for any other name. The one classifier the value-position
/// lowering and the constant folder share, so the two admit exactly the same bounds.
pub(crate) fn builtin_const_int(name: &str) -> Option<i64> {
    Builtin::from_name(name).and_then(Builtin::const_int_value)
}

/// The diagnostic for a value declaration whose name is a reserved built-in.
pub(crate) fn reserved_builtin_name(
    file: &FileIdentity,
    span: SourceSpan,
    name: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict,
        file,
        span,
        format!("`{name}` is a built-in and cannot be redeclared"),
    )
}

/// Classify an expression as a collection constructor call on the reserved type name
/// `List`/`Map`, returning the head and its positional arguments. An empty argument
/// list is the empty constructor; a non-empty one is variadic list construction (the
/// map literal is deferred, rejected by the ctor lowering).
pub(super) fn collection_ctor_call(expr: &Expression) -> Option<(&'static str, &[Argument])> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    match &**callee {
        Expression::Name { segments, .. } => match &segments[..] {
            [n] if n.text() == "List" => Some(("List", args)),
            [n] if n.text() == "Map" => Some(("Map", args)),
            _ => None,
        },
        _ => None,
    }
}

/// The diagnostic for a built-in called with the wrong argument shape.
pub(super) fn builtin_arity(
    file: &FileIdentity,
    span: SourceSpan,
    name: &str,
    arity: usize,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType,
        file,
        span,
        format!("`{name}` takes {arity} positional argument(s)"),
    )
}

/// Classify an expression as a built-in constructor form: bare `none`, or a call
/// `some(..)`/`ok(..)`/`err(..)`. Returns `None` for anything else.
pub(super) fn constructor_kind(expr: &Expression) -> Option<CtorKind> {
    match expr {
        Expression::Name { segments, .. } if matches!(&segments[..], [n] if n.text() == "none") => {
            Some(CtorKind::None)
        }
        Expression::Call { callee, .. } => match &**callee {
            Expression::Name { segments, .. } => match &segments[..] {
                [n] if n.text() == "some" => Some(CtorKind::Some),
                [n] if n.text() == "ok" => Some(CtorKind::Ok),
                [n] if n.text() == "err" => Some(CtorKind::Err),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Split a dotted constructor base into its single-segment head name (with span) and the
/// branch-name chain before the final call segment. `Book` yields `("Book", span, [])`;
/// `Book.notes` yields `("Book", span, ["notes"])`; deeper chains accumulate. `None` for a
/// head that is not a single-segment name (a `::`-qualified or otherwise non-head base).
pub(super) fn split_dotted_head(expr: &Expression) -> Option<(&str, SourceSpan, Vec<&str>)> {
    match expr {
        Expression::Name { segments, span, .. } if segments.len() == 1 => {
            Some((segments[0].text(), *span, Vec::new()))
        }
        Expression::Field { base, name, .. } => {
            let (head, span, mut names) = split_dotted_head(base)?;
            names.push(&**name);
            Some((head, span, names))
        }
        _ => None,
    }
}

/// The source-shaped display of a branch constructor head, `Resource.b1.….bn`, for a
/// diagnostic.
pub(super) fn branch_ctor_display(resource: &str, path: &[&str]) -> String {
    std::iter::once(resource)
        .chain(path.iter().copied())
        .collect::<Vec<_>>()
        .join(".")
}

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Lower a call in the closed pure text floor: `isEmpty(string): bool`,
    /// `contains(string, string): bool`, `trim(string): string`. One owner for the
    /// whole floor; there is no general string library.
    pub(super) fn lower_text_builtin(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let (arity, instr, result): (usize, Instr, LTy) = match name {
            "isEmpty" => (1, Instr::TextIsEmpty, bool_ty),
            "contains" => (2, Instr::TextContains, bool_ty),
            "trim" => (1, Instr::TextTrim, text),
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller dispatched on this exact set of text-floor builtin names before entering this match"
            )]
            _ => unreachable!("caller matched the text-floor names"),
        };
        if args.len() != arity || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!("`{name}` takes {arity} positional string argument(s)"),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        for arg in args {
            self.lower_as(&arg.value, text)?;
        }
        self.push(instr, span)?;
        Ok(result)
    }

    /// Lower a collection-returning text-floor call: `split(text, sep): List[string]`
    /// or `lines(text): List[string]`. Both mint (and reuse) the one `List[string]`
    /// COLLTYPES instantiation and emit the split/lines opcode carrying it; the VM
    /// bounds the result by the same law-9 collection limits `append` observes.
    pub(super) fn lower_text_split(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        let arity = if name == "split" { 2 } else { 1 };
        if args.len() != arity || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!("`{name}` takes {arity} positional string argument(s)"),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        for arg in args {
            self.lower_as(&arg.value, text)?;
        }
        let result = self
            .records
            .instantiate_list(self.draft, GArg::Scalar(ScalarType::Text));
        let idx = self
            .accept_resolution(result, span, "this text collection result")
            .ok_or(LoweringFailure::Recoverable)?;
        let instr = if name == "split" {
            Instr::TextSplit(idx)
        } else {
            Instr::TextLines(idx)
        };
        self.push(instr, span)?;
        Ok(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower `join(parts: List[string], sep: string): string`: concatenate the list's
    /// text elements with a separator. A first argument that is not a `List[string]`
    /// is a typed diagnostic; the VM bounds the result by the `run.text_limit`
    /// concatenation ceiling.
    pub(super) fn lower_text_join(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        if args.len() != 2 || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "`join` takes 2 positional argument(s): a list of string and a separator"
                    .to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let idx = self.collection_arg(&args[0].value)?;
        match self.records.collection_spec(idx) {
            CollSpec::List {
                elem: GArg::Scalar(ScalarType::Text),
            } => {}
            _ => {
                self.fail(unsupported(
                    self.file,
                    args[0].value.span(),
                    "`join` on this type (it joins a list of string)",
                ));
                return Err(LoweringFailure::Recoverable);
            }
        }
        self.lower_as(&args[1].value, text)?;
        self.push(Instr::TextJoin, span)?;
        Ok(text)
    }

    /// Lower a temporal constructor `date("…")` / `instant("…")` / `duration("…")`.
    /// Construction is from exactly one static string literal, validated and folded
    /// at compile time: a malformed or out-of-range canonical form is a typed
    /// `check.type` diagnostic here, so no ordinary program produces an out-of-range
    /// temporal value at runtime. The folded raw scalar is interned as a temporal
    /// constant. `marrow-temporal` owns the canonical text grammar.
    pub(super) fn lower_temporal_construct(
        &mut self,
        scalar: ScalarType,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let spelling = scalar.spelling();
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!("`{spelling}` takes one string-literal argument"),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                arg.value.span(),
                format!("the `{spelling}` argument is positional"),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        // A temporal value is constructed only from a static string literal, so its
        // canonical form is validated once at compile time rather than parsed at
        // runtime (there is no ambient clock or runtime temporal parse in the floor).
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: arg_span,
        } = &arg.value
        else {
            self.fail(unsupported(
                self.file,
                arg.value.span(),
                &format!("constructing a `{spelling}` from a non-literal value"),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *arg_span, "this string literal"));
            return Err(LoweringFailure::Recoverable);
        };
        let bytes = decoded.as_bytes();
        let minted = match scalar {
            ScalarType::Date => match marrow_temporal::parse_date(bytes) {
                Some(days) => self.draft.intern_date(days),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            ScalarType::Instant => match marrow_temporal::parse_instant(bytes) {
                Some(nanos) => self.draft.intern_instant(nanos),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            ScalarType::Duration => match marrow_temporal::parse_duration(bytes) {
                Some(nanos) => self.draft.intern_duration(nanos),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller restricts this dispatch to the temporal scalar types matched above"
            )]
            _ => unreachable!("caller passes only a temporal scalar"),
        };
        let const_id = self
            .checked_mint(|_| minted)
            .ok_or(LoweringFailure::Recoverable)?;
        self.push(Instr::ConstLoad(const_id), span)?;
        Ok(LTy::bare_scalar(scalar))
    }

    /// Report a malformed or out-of-range temporal literal.
    fn fail_temporal_literal(
        &mut self,
        scalar: ScalarType,
        value: &str,
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let form = match scalar {
            ScalarType::Date => "a canonical date `YYYY-MM-DD` in years 0001-9999",
            ScalarType::Instant => {
                "a canonical UTC instant `YYYY-MM-DDTHH:MM:SS[.fraction]Z` in years 0001-9999"
            }
            ScalarType::Duration => "a canonical duration `[-]PT<seconds>[.fraction]S`",
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller restricts this dispatch to the temporal scalar types matched above"
            )]
            _ => unreachable!("caller passes only a temporal scalar"),
        };
        self.fail(SourceDiagnostic::at(
            Code::CheckType,
            self.file,
            span,
            format!(
                "`{value}` is not {form}, so it is not a `{}` literal",
                scalar.spelling()
            ),
        ));
        Err(LoweringFailure::Recoverable)
    }

    /// Lower `addDays(date, int): date` or `daysBetween(date, date): int`,
    /// emitting the checked temporal instruction after type-checking the operands.
    pub(super) fn lower_date_arith(
        &mut self,
        builtin: Builtin,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let (name, second, instr, result) = match builtin {
            Builtin::DateAddDays => (
                "addDays",
                ScalarType::Int,
                Instr::DateAddDays,
                ScalarType::Date,
            ),
            Builtin::DateDaysBetween => (
                "daysBetween",
                ScalarType::Date,
                Instr::DateDaysBetween,
                ScalarType::Int,
            ),
            #[allow(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller restricts this dispatch to the date-arithmetic builtins matched above"
            )]
            _ => unreachable!("caller passes only a date-arithmetic builtin"),
        };
        let [first_arg, second_arg] = args else {
            self.fail(builtin_arity(self.file, span, name, 2));
            return Err(LoweringFailure::Recoverable);
        };
        if first_arg.name.is_some() || second_arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!("`{name}` arguments are positional"),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        self.expect_bare_scalar(&first_arg.value, ScalarType::Date, name)?;
        self.expect_bare_scalar(&second_arg.value, second, name)?;
        self.push(instr, span)?;
        Ok(LTy::bare_scalar(result))
    }

    /// Lower `expr` and require it to be exactly the bare scalar `expected`, failing
    /// with a `check.type` diagnostic (naming `builtin`) otherwise.
    fn expect_bare_scalar(
        &mut self,
        expr: &Expression,
        expected: ScalarType,
        builtin: &str,
    ) -> ConstructResult<()> {
        let ty = self.lower_expr(expr)?;
        if ty == LTy::bare_scalar(expected) {
            return Ok(());
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType,
            self.file,
            expr.span(),
            format!(
                "`{builtin}` expects a `{}` argument, found `{}`",
                expected.spelling(),
                ty.spelling(self.records)
            ),
        ));
        Err(LoweringFailure::Recoverable)
    }

    pub(super) fn lower_conversion(
        &mut self,
        target: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                format!("`{target}` conversion takes one value"),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                arg.value.span(),
                "a conversion argument is positional".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let source = self.lower_expr(&arg.value)?;
        // `string(value)` renders any interpolable value — a scalar, an enum, or an
        // entry identity — to its canonical text, the same rendering interpolation and
        // program output use.
        if target == "string" && is_interpolable(source) {
            self.push(Instr::ConvString, span)?;
            return Ok(LTy::bare_scalar(ScalarType::Text));
        }
        use ScalarType::{Bytes, Text};
        let (instr, result) = match (target, source.bare_scalar_type()) {
            ("bytes", Some(Text)) => (Instr::ConvBytesText, Bytes),
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    &format!("converting {} to {target}", source.spelling(self.records)),
                ));
                return Err(LoweringFailure::Recoverable);
            }
        };
        self.push(instr, span)?;
        Ok(LTy::bare_scalar(result))
    }

    /// Lower `unreachable("static text")`: the sole application-invariant fault. It
    /// takes exactly one static string literal, emits a fault instruction carrying
    /// that text, and diverges (control never continues past it).
    pub(super) fn lower_unreachable(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<CallResult> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "`unreachable` takes one static string literal".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                arg.value.span(),
                "`unreachable` takes one positional static string literal".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: lit_span,
        } = &arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                arg.value.span(),
                "`unreachable` requires a static string literal, not a computed value".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *lit_span, "this string literal"));
            return Err(LoweringFailure::Recoverable);
        };
        let const_id = self
            .checked_mint(|draft| draft.intern_text(&decoded))
            .ok_or(LoweringFailure::Recoverable)?;
        self.push(Instr::Unreachable(const_id), span)?;
        Ok(CallResult::Diverges)
    }

    /// Lower `todo("static text")`: a deferred path the author has not implemented. It
    /// mirrors `unreachable` exactly — one static string literal, a fault instruction
    /// carrying that text, and divergence — but raises `run.todo` when reached.
    pub(super) fn lower_todo(
        &mut self,
        args: &[Argument],
        span: SourceSpan,
    ) -> ConstructResult<CallResult> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                span,
                "`todo` takes one static string literal".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                arg.value.span(),
                "`todo` takes one positional static string literal".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        }
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: lit_span,
        } = &arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType,
                self.file,
                arg.value.span(),
                "`todo` requires a static string literal, not a computed value".to_string(),
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *lit_span, "this string literal"));
            return Err(LoweringFailure::Recoverable);
        };
        let const_id = self
            .checked_mint(|draft| draft.intern_text(&decoded))
            .ok_or(LoweringFailure::Recoverable)?;
        self.push(Instr::Todo(const_id), span)?;
        Ok(CallResult::Diverges)
    }
}

#[cfg(test)]
mod tests {
    use super::{Builtin, builtin_const_int, builtin_value_names};
    use crate::{CompileFailure, compile};
    use marrow_codes::Code;
    use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

    /// The integer bounds classify as value built-ins carrying exactly the `i64`
    /// domain edges, and a built-in that is a call or constructor carries no bound
    /// value, so only `maxInt`/`minInt` fold in value or constant position.
    #[test]
    fn the_integer_bounds_carry_the_domain_edges() {
        assert_eq!(builtin_const_int("maxInt"), Some(i64::MAX));
        assert_eq!(builtin_const_int("minInt"), Some(i64::MIN));
        assert_eq!(Builtin::MaxInt.const_int_value(), Some(i64::MAX));
        assert_eq!(Builtin::MinInt.const_int_value(), Some(i64::MIN));
        assert_eq!(Builtin::Trim.const_int_value(), None);
        assert_eq!(Builtin::None.const_int_value(), None);
        assert_eq!(builtin_const_int("trim"), None);
        assert_eq!(builtin_const_int("length"), None);
    }

    /// The editor completion namespace is exactly the set the classifier recognizes, in
    /// both directions: both derive from the single [`Builtin::ALL`] registry, so neither
    /// can gain (or lose) a name the other lacks. Every registry name classifies and
    /// round-trips through `spelling`/`from_name`; adding a variant is a compile error in
    /// `spelling` until it is named, and it then joins both consumers at once.
    #[test]
    fn completion_names_match_the_classifier() {
        for builtin in Builtin::ALL {
            let name = builtin.spelling();
            let classified =
                Builtin::from_name(name).expect("a registry spelling must classify as a built-in");
            assert_eq!(
                classified.spelling(),
                name,
                "`{name}` must round-trip to the same built-in",
            );
        }
        let offered = builtin_value_names();
        let registry: Vec<&str> = Builtin::ALL.iter().map(|b| b.spelling()).collect();
        assert_eq!(
            offered, registry,
            "the completion namespace is exactly the classifier registry",
        );
    }

    fn project(source: &str) -> ProjectInput {
        let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
        let captured = vec![CapturedFile::new(
            "src/main.mw".to_string(),
            source.as_bytes().to_vec(),
        )];
        marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
            .expect("capture project")
    }

    /// The `(code, message)` rows a source is refused with, or the empty vector when
    /// it compiles clean.
    fn refusals(source: &str) -> Vec<(Code, String)> {
        match compile(&project(source)) {
            Ok(_) => Vec::new(),
            Err(CompileFailure::Diagnostics(rows)) => rows
                .into_iter()
                .map(|row| (row.code(), row.message().to_string()))
                .collect(),
            Err(other) => panic!("expected source diagnostics, got {other:?}"),
        }
    }

    /// The built-in spellings the lexer reserves as keywords, which the parser refuses
    /// before the semantic conflict check is reached. The reservation is stronger and
    /// earlier, not absent — but it is a *different* owner, so it is named here: a
    /// change that stops treating one of these as a keyword must add the semantic
    /// reservation in the same lane, and this list is what makes that visible.
    const KEYWORD_RESERVED: &[&str] = &["Id"];

    /// BLTNSHDW01: every reserved built-in is refused in every value-declaration
    /// position, swept from the registry rather than from a hand-listed sample.
    ///
    /// A built-in absent from the conflict check is admitted as a declaration and then
    /// silently shadowed at every use the compiler intercepts, so the reader's `fn` is
    /// never called and the failure surfaces far from its cause. The sweep is driven by
    /// [`Builtin::ALL`], so a built-in added to the registry without its reservation
    /// fails here rather than shipping shadowable.
    #[test]
    fn every_builtin_is_refused_in_every_value_declaration_position() {
        for builtin in Builtin::ALL {
            let name = builtin.spelling();
            let positions = [
                (
                    "a function",
                    format!(
                        "module main\n\nfn {name}(a: int): int {{\n    return a\n}}\n\n\
                         pub fn go(): int {{\n    return 1\n}}\n"
                    ),
                ),
                (
                    "a constant",
                    format!(
                        "module main\n\nconst {name} = 5\n\n\
                         pub fn go(): int {{\n    return 1\n}}\n"
                    ),
                ),
                (
                    "a parameter",
                    format!("module main\n\npub fn go({name}: int): int {{\n    return 1\n}}\n"),
                ),
                (
                    "a local binding",
                    format!(
                        "module main\n\npub fn go(): int {{\n    const {name} = 5\n    \
                         return 1\n}}\n"
                    ),
                ),
            ];
            for (position, source) in positions {
                let rows = refusals(&source);
                assert!(
                    !rows.is_empty(),
                    "{position} named `{name}` compiled clean: the built-in is silently \
                     shadowed at every use the compiler intercepts",
                );
                if KEYWORD_RESERVED.contains(&name) {
                    assert!(
                        rows.iter().any(|(code, _)| *code == Code::ParseSyntax),
                        "`{name}` is reserved by the lexer, so the parser refuses \
                         {position} before the semantic check: got {rows:?}",
                    );
                    continue;
                }
                let expected = format!("`{name}` is a built-in and cannot be redeclared");
                assert!(
                    rows.iter()
                        .any(|(code, message)| *code == Code::CheckNameConflict
                            && message == &expected),
                    "{position} named `{name}` must be refused as a built-in \
                     redeclaration: got {rows:?}",
                );
            }
        }
    }
}
