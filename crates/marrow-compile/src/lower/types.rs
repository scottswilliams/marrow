//! Type-annotation and operator resolution: the type-parameter environment,
//! unification, and the operator/comparison tables.

use super::*;

/// A generic type parameter's binding in the body being lowered.
#[derive(Clone, Copy)]
pub(super) enum ParamBinding {
    /// The once-checked template pass: an opaque type admitting only its declared
    /// constraint's operators.
    Abstract(Option<TypeConstraint>),
    /// A monomorphized instantiation: the concrete value type the parameter denotes.
    Concrete(GArg),
}

/// One declared type parameter in the body being lowered: its source name and how
/// a use of that name resolves.
pub(super) struct TypeParamSlot {
    pub(super) name: String,
    pub(super) binding: ParamBinding,
}

/// The type-parameter environment threaded through type resolution. An empty
/// environment is an ordinary monomorphic body; a non-empty one resolves a use of
/// a type-parameter name to an abstract [`LTy::Param`] (template pass) or the bound
/// concrete type (instantiation), before scalar/named-type classification.
#[derive(Clone, Copy)]
pub(super) struct TypeEnv<'a> {
    pub(super) params: &'a [TypeParamSlot],
}

impl TypeEnv<'_> {
    pub(super) const EMPTY: TypeEnv<'static> = TypeEnv { params: &[] };

    /// The declaration index and binding of the type parameter named `name`.
    fn lookup(&self, name: &str) -> Option<(TypeParamIndex, ParamBinding)> {
        self.params
            .iter()
            .position(|slot| slot.name == name)
            .map(|index| {
                (
                    TypeParamIndex::from_position(index),
                    self.params[index].binding,
                )
            })
    }

    /// The constraint on the type parameter at `index`, in the abstract pass.
    pub(super) fn constraint_at(&self, index: TypeParamIndex) -> Option<TypeConstraint> {
        match self.params.get(index.position()).map(|slot| slot.binding) {
            Some(ParamBinding::Abstract(constraint)) => constraint,
            _ => None,
        }
    }
}

/// Resolve a parameter annotation to its lowered type: a bare scalar, a bare
/// nominal, a bare `struct`, or a bare resource-record value. Optionals and
/// unresolved names are outside the parameter subset. One owner for signature
/// building and body lowering, so the two cannot disagree on a parameter's type.
pub(super) fn param_type(
    records: &mut TypeRegistry,
    draft: &mut DraftTxn<'_>,
    durable: &DurableRegistry,
    ty: &TypeExpr,
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    match resolve_type(records, draft, durable, ty, env, site) {
        Ok(
            param @ (LTy::Scalar {
                optional: false, ..
            }
            | LTy::Nominal {
                optional: false, ..
            }
            | LTy::Record {
                optional: false, ..
            }
            | LTy::Struct {
                optional: false, ..
            }
            | LTy::Enum {
                optional: false, ..
            }
            // A finite collection is a by-value value type, admitted as a parameter
            // (its element/key/value types may themselves be type parameters).
            | LTy::Collection {
                optional: false, ..
            }
            // A generic parameter is admitted as a value parameter; the collection
            // element/value positions admit it through `resolve_generic`.
            | LTy::Param {
                optional: false, ..
            }
            // An entry identity is a by-value value type, admitted as a parameter.
            | LTy::Identity {
                optional: false, ..
            }),
        ) => Ok(param),
        // A type that resolves but is outside the parameter subset is a genuine
        // subset gap. Every refusal passes through unchanged, so a refused
        // declaration keeps its cause all the way to the report.
        Ok(_) => Err(ResolveError::Refusal(ResolveRefusal::Unsupported)),
        Err(error) => Err(error),
    }
}

/// Resolve written annotations with local parameters and globally bound aliases.
/// Optionality is checked after name resolution, so an alias cannot hide a second layer.
pub(super) fn resolve_type(
    records: &mut TypeRegistry,
    draft: &mut DraftTxn<'_>,
    durable: &DurableRegistry,
    annotation: &TypeExpr,
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    match annotation {
        TypeExpr::Name { text, .. } => {
            // A type-parameter name resolves before scalar/named-type classification,
            // so a parameter cannot be shadowed by a same-named scalar spelling.
            if let Some((index, binding)) = env.lookup(text) {
                return Ok(match binding {
                    ParamBinding::Abstract(_) => LTy::Param {
                        index,
                        optional: false,
                    },
                    ParamBinding::Concrete(arg) => garg_to_lty(arg),
                });
            }
            // A name resolves in the tree that wrote it; a two-segment name resolves
            // in the dependency its alias declares. A spelling that names no tree is
            // outside the admitted set, like any other unknown name.
            let Some(written) = records.scoped(site.file.origin(), text) else {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            };
            // The alias hop is taken once, before the borrow it needs is released:
            // an alias's terminal is scoped to the tree that bound it, and its
            // optionality composes onto whatever that terminal resolves to.
            let (scope, optional) = match records.alias_target(&written) {
                Some(target) => (
                    target.terminal.clone(),
                    target.presence == crate::types::AliasPresence::Optional,
                ),
                None => (written, false),
            };
            let resolved = if let Some(scalar) = ScalarType::from_spelling(scope.name()) {
                Ok(LTy::bare_scalar(scalar))
            } else if let Some((id, _)) = records.nominal_by_name(&scope) {
                Ok(LTy::Nominal {
                    id,
                    optional: false,
                })
            } else {
                match records.static_named_type_projection(&scope)? {
                    Some(StaticNamedType::Struct(ty)) => Ok(LTy::Struct {
                        ty,
                        optional: false,
                    }),
                    Some(StaticNamedType::Enum(ty)) => Ok(LTy::Enum {
                        ty,
                        optional: false,
                    }),
                    Some(StaticNamedType::Record(ty)) => Ok(LTy::Record {
                        ty,
                        optional: false,
                    }),
                    // A name no table answers is either genuinely undeclared or a
                    // declaration this project refused; the ledger tells them apart,
                    // and only the first may be reported as an unsupported form.
                    None => Err(ResolveError::Refusal(
                        records.unresolved_named_type(&scope)?,
                    )),
                }
            };
            resolved.map(|ty| if optional { ty.to_optional() } else { ty })
        }
        TypeExpr::Optional { inner, .. } => {
            let inner = resolve_type(records, draft, durable, inner, env, site)?;
            if inner.is_optional() {
                Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
            } else {
                Ok(inner.to_optional())
            }
        }
        TypeExpr::Apply { head, args, .. } => {
            resolve_generic(records, draft, durable, head, args, env, site)
        }
        // `Id(^root)`: the entry-identity value type of the named store root, carrying
        // that root's declaration-ordered RootId. An undeclared or not-yet-executable
        // root is an unsupported type, reported by the caller.
        TypeExpr::Identity(identity) => {
            let root_key = crate::source::ScopedName::new(site.file.origin(), &identity.root);
            let root = match durable.root(&root_key)? {
                RootBinding::Executable(root) => root,
                RootBinding::Refused(id, _) => {
                    return Err(ResolveError::Refusal(ResolveRefusal::RefusedDeclaration(
                        id,
                    )));
                }
                RootBinding::NotYetExecutable | RootBinding::Absent => {
                    return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
                }
            };
            Ok(LTy::Identity {
                root: root.root_id,
                optional: false,
            })
        }
        // A parse-recovery leaf for a missing type annotation. Resolution runs only
        // on a `!has_errors` tree, so this is unreachable here; fail closed rather
        // than invent a type.
        TypeExpr::Incomplete { .. } => Err(ResolveError::Refusal(ResolveRefusal::Unsupported)),
    }
}

/// Resolve a generic type application to a bare instantiation, monomorphizing it
/// into the draft on first use. `List`/`Map` are the compiler collections; every
/// other head is a value-type template resolved through the one instantiation owner.
/// A wrong arity, a non-value-type argument, or a constraint violation refuses as an
/// unsupported type. An argument may itself be an abstract type parameter, whose
/// declared constraint then stands in for the concrete one during revalidation.
fn resolve_generic(
    records: &mut TypeRegistry,
    draft: &mut DraftTxn<'_>,
    durable: &DurableRegistry,
    head: &str,
    args: &[TypeExpr],
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    match head {
        "List" => {
            let [elem] = args else {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            };
            let elem = resolve_type(records, draft, durable, elem, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            Ok(LTy::Collection {
                idx: records.instantiate_list(draft, elem)?,
                optional: false,
            })
        }
        "Map" => {
            let [key, value] = args else {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            };
            let key = resolve_type(records, draft, durable, key, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            records.check_map_key_admissibility(key)?;
            let value = resolve_type(records, draft, durable, value, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            Ok(LTy::Collection {
                idx: records.instantiate_map(draft, key, value)?,
                optional: false,
            })
        }
        _ => {
            let template = records.application_template(site.file.origin(), head)?;
            let params = records.template_type_params(template);
            if args.len() != params.len() {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            }
            let mut resolved = Vec::with_capacity(args.len());
            for arg in args {
                resolved.push(
                    resolve_type(records, draft, durable, arg, env, site)?
                        .as_garg()
                        .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?,
                );
            }
            // Per-application constraint revalidation: a concrete argument must
            // support the constraint; an abstract parameter satisfies it when its own
            // declared constraint does.
            for ((_, constraint), arg) in
                records.template_type_params(template).iter().zip(&resolved)
            {
                if let Some(constraint) = constraint {
                    let satisfied = match arg {
                        GArg::Param(index) => {
                            env.constraint_at(*index)
                                .is_some_and(|outer| match constraint {
                                    TypeConstraint::Equality => outer.admits_equality(),
                                    TypeConstraint::Order => outer.admits_order(),
                                })
                        }
                        other => other.satisfies(*constraint),
                    };
                    if !satisfied {
                        // A malformed registry remains an invariant even when this
                        // application also violates a source constraint, so the
                        // preflight still runs before the refusal.
                        records.validate_type_arguments(&resolved)?;
                        return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
                    }
                }
            }
            match records.mint_type_instance(draft, template, &resolved, site)? {
                TypeInstId::Record(ty) => Ok(LTy::Struct {
                    ty,
                    optional: false,
                }),
                TypeInstId::Enum(id) => Ok(LTy::Enum {
                    ty: id,
                    optional: false,
                }),
            }
        }
    }
}

/// Why structural unification of a generic parameter against an argument failed.
///
/// Inference is exact: a bare parameter position requires a bare argument (no
/// implicit bare-to-optional widening), and a concrete named position requires an
/// exactly matching argument.
pub(super) enum UnifyError {
    Mismatch(String),
    Invariant(LowerInvariant),
}

impl From<LowerInvariant> for UnifyError {
    fn from(invariant: LowerInvariant) -> Self {
        Self::Invariant(invariant)
    }
}

/// Structurally unify a generic parameter's declared type against an argument's
/// inferred type, binding each type parameter to the value type filling its position.
pub(super) fn unify_type_param(
    records: &TypeRegistry,
    origin: &SourceOrigin,
    type_params: &[(String, Option<TypeConstraint>)],
    annotation: &TypeExpr,
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), UnifyError> {
    records.with_metadata_session(|metadata| {
        if let Some(arg) = got.to_bare().as_garg() {
            metadata.validate_type_arguments(&[arg])?;
        }
        unify_type_param_with(
            records,
            metadata,
            origin,
            type_params,
            annotation,
            got,
            subst,
        )
    })
}

/// The template's annotations are written in the tree that declares it, so every
/// name in them resolves in `origin` — never in the tree of the call site that
/// instantiates the template.
#[allow(clippy::too_many_arguments)]
fn unify_type_param_with(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
    origin: &SourceOrigin,
    type_params: &[(String, Option<TypeConstraint>)],
    annotation: &TypeExpr,
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), UnifyError> {
    match annotation {
        TypeExpr::Name { text, .. } => {
            if let Some(index) = type_params.iter().position(|(name, _)| name == text) {
                if got.is_optional() {
                    return Err(UnifyError::Mismatch(format!(
                        "type parameter `{text}` matches a bare value, but the argument is `{}`",
                        got.spelling_in(records, metadata)?
                    )));
                }
                let arg = got.as_garg().ok_or_else(|| {
                    UnifyError::Mismatch(format!(
                        "`{}` is not a value type that can instantiate `{text}`",
                        got.spelling(records)
                    ))
                })?;
                match subst[index] {
                    None => subst[index] = Some(arg),
                    Some(previous) if previous == arg => {}
                    Some(previous) => {
                        let previous = garg_to_lty(previous).spelling_in(records, metadata)?;
                        let current = garg_to_lty(arg).spelling_in(records, metadata)?;
                        return Err(UnifyError::Mismatch(format!(
                            "type parameter `{text}` is inferred as both `{}` and `{}`",
                            previous, current
                        )));
                    }
                }
                Ok(())
            } else {
                match named_type(records, metadata, origin, text)? {
                    Some(expected) if expected == got => Ok(()),
                    Some(expected) => Err(UnifyError::Mismatch(format!(
                        "expected `{}`, found `{}`",
                        expected.spelling_in(records, metadata)?,
                        got.spelling_in(records, metadata)?
                    ))),
                    None => Err(UnifyError::Mismatch(format!(
                        "unknown type `{text}` in a generic parameter"
                    ))),
                }
            }
        }
        TypeExpr::Optional { inner, .. } => {
            if !got.is_optional() {
                return Err(UnifyError::Mismatch(format!(
                    "expected an optional argument, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            }
            unify_type_param_with(
                records,
                metadata,
                origin,
                type_params,
                inner,
                got.to_bare(),
                subst,
            )
        }
        TypeExpr::Apply { head, args, .. } => unify_apply_with(
            records,
            metadata,
            origin,
            type_params,
            head,
            args,
            got,
            subst,
        ),
        _ => Err(UnifyError::Mismatch(
            "this parameter type is not supported for generic inference".to_string(),
        )),
    }
}

/// Unify a built-in generic parameter application (`List`/`Map`/`Option`/`Result`)
/// against an argument, recursing into the argument's element/key/value/payload
/// types.
#[allow(clippy::too_many_arguments)]
fn unify_apply_with(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
    origin: &SourceOrigin,
    type_params: &[(String, Option<TypeConstraint>)],
    head: &str,
    args: &[TypeExpr],
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), UnifyError> {
    match head {
        "List" => {
            let [elem] = args else {
                return Err(UnifyError::Mismatch(
                    "`List` takes one type argument".to_string(),
                ));
            };
            let LTy::Collection {
                idx,
                optional: false,
            } = got
            else {
                return Err(UnifyError::Mismatch(format!(
                    "expected a List, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            };
            match metadata.collection_spec(idx)? {
                CollSpec::List { elem: got_elem } => unify_type_param_with(
                    records,
                    metadata,
                    origin,
                    type_params,
                    elem,
                    garg_to_lty(got_elem),
                    subst,
                ),
                CollSpec::Map { .. } => Err(UnifyError::Mismatch(format!(
                    "expected a List, found `{}`",
                    got.spelling_in(records, metadata)?
                ))),
            }
        }
        "Map" => {
            let [key, value] = args else {
                return Err(UnifyError::Mismatch(
                    "`Map` takes two type arguments".to_string(),
                ));
            };
            let LTy::Collection {
                idx,
                optional: false,
            } = got
            else {
                return Err(UnifyError::Mismatch(format!(
                    "expected a Map, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            };
            match metadata.collection_spec(idx)? {
                CollSpec::Map {
                    key: got_key,
                    value: got_value,
                } => {
                    unify_type_param_with(
                        records,
                        metadata,
                        origin,
                        type_params,
                        key,
                        garg_to_lty(got_key),
                        subst,
                    )?;
                    unify_type_param_with(
                        records,
                        metadata,
                        origin,
                        type_params,
                        value,
                        garg_to_lty(got_value),
                        subst,
                    )
                }
                CollSpec::List { .. } => Err(UnifyError::Mismatch(format!(
                    "expected a Map, found `{}`",
                    got.spelling_in(records, metadata)?
                ))),
            }
        }
        // Every other generic head is a value-type template: the argument must be an
        // instantiation of the same template, and each type argument unifies
        // positionally against its parameter.
        _ => {
            let template = records
                .scoped(origin, head)
                .and_then(|scope| records.type_template_by_name(&scope))
                .ok_or_else(|| {
                    UnifyError::Mismatch(format!(
                        "`{head}` is not a generic type usable in a parameter"
                    ))
                })?;
            if args.len() != records.template_type_params(template).len() {
                return Err(UnifyError::Mismatch(format!(
                    "`{head}` takes {} type argument(s)",
                    records.template_type_params(template).len()
                )));
            }
            let inst_id = match got {
                LTy::Struct {
                    ty,
                    optional: false,
                } => TypeInstId::Record(ty),
                LTy::Enum {
                    ty,
                    optional: false,
                } => TypeInstId::Enum(ty),
                _ => {
                    return Err(UnifyError::Mismatch(format!(
                        "expected a {head}, found `{}`",
                        got.spelling_in(records, metadata)?
                    )));
                }
            };
            let Some((got_template, got_args)) = metadata.instantiation_of(inst_id)? else {
                return Err(UnifyError::Mismatch(format!(
                    "expected a {head}, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            };
            if got_template != template {
                return Err(UnifyError::Mismatch(format!(
                    "expected a {head}, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            }
            for (arg, got_arg) in args.iter().zip(&got_args) {
                unify_type_param_with(
                    records,
                    metadata,
                    origin,
                    type_params,
                    arg,
                    garg_to_lty(*got_arg),
                    subst,
                )?;
            }
            Ok(())
        }
    }
}

/// Resolve a concrete named type or global alias, preserving alias optionality,
/// without minting into any draft, for exact-match generic inference.
fn named_type(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
    origin: &SourceOrigin,
    text: &str,
) -> Result<Option<LTy>, LowerInvariant> {
    let Some(written) = records.scoped(origin, text) else {
        return Ok(None);
    };
    let (scope, optional) = match records.alias_target(&written) {
        Some(target) => (
            target.terminal.clone(),
            target.presence == crate::types::AliasPresence::Optional,
        ),
        None => (written, false),
    };
    let resolved = if let Some(scalar) = ScalarType::from_spelling(scope.name()) {
        Ok(Some(LTy::bare_scalar(scalar)))
    } else if let Some((id, _)) = records.nominal_by_name(&scope) {
        Ok(Some(LTy::Nominal {
            id,
            optional: false,
        }))
    } else {
        Ok(match metadata.static_named_type(&scope)? {
            Some(StaticNamedType::Struct(ty)) => Some(LTy::Struct {
                ty,
                optional: false,
            }),
            Some(StaticNamedType::Enum(ty)) => Some(LTy::Enum {
                ty,
                optional: false,
            }),
            Some(StaticNamedType::Record(ty)) => Some(LTy::Record {
                ty,
                optional: false,
            }),
            None => None,
        })
    };
    resolved.map(|ty| ty.map(|ty| if optional { ty.to_optional() } else { ty }))
}

/// The ordering instruction `op` lowers to over two values of `scalar`, or `None`
/// when `op` is not an order comparison or `scalar` has no order (`bool`). The one
/// order table: the operator lowering selects from it, a nominal int compares through
/// its `int` row, and a `supports order` bound admits exactly the scalars it orders.
/// Equality stays with [`eq_instr`], which every scalar has.
pub(crate) fn scalar_order(op: BinaryOp, scalar: ScalarType) -> Option<Instr> {
    let [lt, le, gt, ge] = match scalar {
        ScalarType::Int => [Instr::IntLt, Instr::IntLe, Instr::IntGt, Instr::IntGe],
        ScalarType::Text => [Instr::TextLt, Instr::TextLe, Instr::TextGt, Instr::TextGe],
        ScalarType::Bytes => [
            Instr::BytesLt,
            Instr::BytesLe,
            Instr::BytesGt,
            Instr::BytesGe,
        ],
        ScalarType::Date => [Instr::DateLt, Instr::DateLe, Instr::DateGt, Instr::DateGe],
        ScalarType::Instant => [
            Instr::InstantLt,
            Instr::InstantLe,
            Instr::InstantGt,
            Instr::InstantGe,
        ],
        ScalarType::Duration => [
            Instr::DurationLt,
            Instr::DurationLe,
            Instr::DurationGt,
            Instr::DurationGe,
        ],
        ScalarType::Bool => return None,
    };
    Some(match op {
        BinaryOp::Less => lt,
        BinaryOp::LessEqual => le,
        BinaryOp::Greater => gt,
        BinaryOp::GreaterEqual => ge,
        _ => return None,
    })
}

pub(super) fn eq_instr(scalar: ScalarType) -> Instr {
    match scalar {
        ScalarType::Int => Instr::EqInt,
        ScalarType::Bool => Instr::EqBool,
        ScalarType::Text => Instr::EqText,
        ScalarType::Bytes => Instr::EqBytes,
        ScalarType::Date => Instr::EqDate,
        ScalarType::Instant => Instr::EqInstant,
        ScalarType::Duration => Instr::EqDuration,
    }
}

pub(crate) fn parse_int(text: &str) -> Option<i64> {
    text.replace('_', "").parse().ok()
}

impl<'a, 'd> FnLowerer<'a, 'd> {
    // --- type resolution ---

    pub(super) fn resolve(&mut self, annotation: &TypeExpr) -> Result<LTy, ResolveError> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        let site = MintSite {
            file: self.file,
            span: annotation.span(),
        };
        resolve_type(
            self.records,
            self.draft,
            self.durable,
            annotation,
            env,
            site,
        )
    }

    pub(super) fn param_type(&mut self, ty: &TypeExpr) -> Option<LTy> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        let site = MintSite {
            file: self.file,
            span: ty.span(),
        };
        match param_type(self.records, self.draft, self.durable, ty, env, site) {
            Ok(param) => Some(param),
            Err(refusal) => {
                self.reject_resolution(refusal, ty.span(), "this parameter type");
                None
            }
        }
    }
}
