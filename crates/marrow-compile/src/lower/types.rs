//! Type-annotation and operator resolution: the type-parameter environment, unification, and the operator/comparison tables.

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
    fn lookup(&self, name: &str) -> Option<(u16, ParamBinding)> {
        self.params
            .iter()
            .position(|slot| slot.name == name)
            .map(|index| (index as u16, self.params[index].binding))
    }

    /// The constraint on the type parameter at `index`, in the abstract pass.
    pub(super) fn constraint_at(&self, index: u16) -> Option<TypeConstraint> {
        match self.params.get(index as usize).map(|slot| slot.binding) {
            Some(ParamBinding::Abstract(constraint)) => constraint,
            _ => None,
        }
    }
}

/// Resolve a parameter annotation to its lowered type: a bare scalar, a bare
/// nominal, a bare `struct`, or a bare resource-record value. Optionals and
/// unresolved names are outside the parameter subset. A resource value crosses the
/// boundary by value like any other record, sharing the image `Record` shape. One
/// owner for signature building and body lowering, so the two can never disagree on
/// a parameter's type.
pub(super) fn param_type(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
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
        Ok(_) | Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
        }
        Err(ResolveError::Refusal(ResolveRefusal::Limit)) => {
            Err(ResolveError::Refusal(ResolveRefusal::Limit))
        }
        Err(ResolveError::Invariant(invariant)) => Err(ResolveError::Invariant(invariant)),
    }
}

/// Resolve a type annotation into a lowered type, or `None` for an unsupported
/// spelling. Aliases expand first, so classification reads only scalar spellings
/// and declared type names; the no-nested-optional rule applies to the expanded
/// form, so an alias cannot smuggle a doubled optional.
pub(super) fn resolve_type(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
    durable: &DurableRegistry,
    annotation: &TypeExpr,
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    resolve_expanded(
        records,
        draft,
        durable,
        &records.expand(annotation),
        env,
        site,
    )
}

fn resolve_expanded(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
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
            if let Some(scalar) = ScalarType::from_spelling(text) {
                Ok(LTy::bare_scalar(scalar))
            } else if let Some((id, _)) = records.nominal_by_name(text) {
                Ok(LTy::Nominal {
                    id,
                    optional: false,
                })
            } else {
                match records.static_named_type_projection(text)? {
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
                    None => Err(ResolveError::Refusal(ResolveRefusal::Unsupported)),
                }
            }
        }
        TypeExpr::Optional { inner, .. } => {
            let inner = resolve_expanded(records, draft, durable, inner, env, site)?;
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
        // that root's declaration-ordered RootId. An identity over a root that is not
        // declared, or over a not-yet-executable root, is an unsupported type (`None`),
        // reported by the caller like any other unresolved annotation.
        TypeExpr::Identity(identity) => {
            let root = durable
                .root_by_name(&identity.root)
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            Ok(LTy::Identity {
                root: root.root_id,
                optional: false,
            })
        }
    }
}

/// Resolve a generic type application to a bare instantiation, monomorphizing it
/// into the draft on first use. `List`/`Map` are the compiler collections; every
/// other head is a value-type template (the reserved `Option`/`Result` or a user
/// `struct`/`enum`) resolved through the one instantiation owner. A wrong arity, an
/// argument that is not a value type, or a constraint violation yields `None`, so
/// the caller reports it as an unsupported type. An argument may itself be an
/// abstract type parameter in the once-checked template pass; its constraint then
/// stands in for the concrete one during revalidation.
fn resolve_generic(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
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
            let elem = resolve_expanded(records, draft, durable, elem, env, site)?
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
            let key = resolve_expanded(records, draft, durable, key, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            records.check_map_key_admissibility(key)?;
            let value = resolve_expanded(records, draft, durable, value, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            Ok(LTy::Collection {
                idx: records.instantiate_map(draft, key, value)?,
                optional: false,
            })
        }
        _ => {
            let template = records.application_template(head)?;
            let params = records.template_type_params(template);
            if args.len() != params.len() {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            }
            let mut resolved = Vec::with_capacity(args.len());
            for arg in args {
                resolved.push(
                    resolve_expanded(records, draft, durable, arg, env, site)?
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
                        // application also violates a source constraint. The normal
                        // successful mint path owns the same preflight and must not
                        // rebuild it here.
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

/// Structurally unify a generic parameter's declared type against an argument's
/// inferred type, binding each type parameter to the concrete value type filling
/// its position. `annotation` is already alias-expanded. Inference is exact: a bare
/// parameter position requires a bare argument (no implicit bare-to-optional
/// widening), and a concrete named position requires an exactly matching argument. A
/// conflicting binding or a shape mismatch is an error the caller reports.
pub(super) enum UnifyError {
    Mismatch(String),
    Invariant(LowerInvariant),
}

impl From<LowerInvariant> for UnifyError {
    fn from(invariant: LowerInvariant) -> Self {
        Self::Invariant(invariant)
    }
}

pub(super) fn unify_type_param(
    records: &TypeRegistry,
    type_params: &[(String, Option<TypeConstraint>)],
    annotation: &TypeExpr,
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), UnifyError> {
    records.with_metadata_session(|metadata| {
        if let Some(arg) = got.to_bare().as_garg() {
            metadata.validate_type_arguments(&[arg])?;
        }
        unify_type_param_with(records, metadata, type_params, annotation, got, subst)
    })
}

fn unify_type_param_with(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
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
                match named_type(records, metadata, text)? {
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
            unify_type_param_with(records, metadata, type_params, inner, got.to_bare(), subst)
        }
        TypeExpr::Apply { head, args, .. } => {
            unify_apply_with(records, metadata, type_params, head, args, got, subst)
        }
        _ => Err(UnifyError::Mismatch(
            "this parameter type is not supported for generic inference".to_string(),
        )),
    }
}

/// Unify a built-in generic parameter application (`List`/`Map`/`Option`/`Result`)
/// against an argument, recursing into the argument's element/key/value/payload
/// types.
fn unify_apply_with(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
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
                        type_params,
                        key,
                        garg_to_lty(got_key),
                        subst,
                    )?;
                    unify_type_param_with(
                        records,
                        metadata,
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
        // Every other generic head is a value-type template (the reserved
        // `Option`/`Result` or a user `struct`/`enum`): the argument must be an
        // instantiation of the same template, and each type argument unifies
        // positionally against its parameter.
        _ => {
            let template = records.type_template_by_name(head).ok_or_else(|| {
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

/// Resolve a concrete named type (a scalar spelling or a declared nominal/struct/
/// enum/record) to its bare lowered type without minting into any draft, for
/// exact-match generic inference.
fn named_type(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
    text: &str,
) -> Result<Option<LTy>, LowerInvariant> {
    if let Some(scalar) = ScalarType::from_spelling(text) {
        Ok(Some(LTy::bare_scalar(scalar)))
    } else if let Some((id, _)) = records.nominal_by_name(text) {
        Ok(Some(LTy::Nominal {
            id,
            optional: false,
        }))
    } else {
        Ok(match metadata.static_named_type(text)? {
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
    }
}

/// The instruction an int ordering comparison lowers to, shared by the bare-int
/// operator table and the same-nominal comparison path (one owner). Equality
/// stays with [`eq_instr`].
pub(super) fn int_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::IntLt,
        BinaryOp::LessEqual => Instr::IntLe,
        BinaryOp::Greater => Instr::IntGt,
        BinaryOp::GreaterEqual => Instr::IntGe,
        _ => return None,
    })
}

/// Whether `op` is one of the four order comparisons, the guard the temporal
/// operator arms share before selecting the per-type instruction.
pub(super) fn temporal_comparison(op: BinaryOp) -> Option<()> {
    matches!(
        op,
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
    )
    .then_some(())
}

pub(super) fn date_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::DateLt,
        BinaryOp::LessEqual => Instr::DateLe,
        BinaryOp::Greater => Instr::DateGt,
        BinaryOp::GreaterEqual => Instr::DateGe,
        _ => return None,
    })
}

pub(super) fn instant_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::InstantLt,
        BinaryOp::LessEqual => Instr::InstantLe,
        BinaryOp::Greater => Instr::InstantGt,
        BinaryOp::GreaterEqual => Instr::InstantGe,
        _ => return None,
    })
}

pub(super) fn duration_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::DurationLt,
        BinaryOp::LessEqual => Instr::DurationLe,
        BinaryOp::Greater => Instr::DurationGt,
        BinaryOp::GreaterEqual => Instr::DurationGe,
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

pub(super) fn operator_symbol(op: BinaryOp) -> &'static str {
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

pub(crate) fn parse_int(text: &str) -> Option<i64> {
    text.replace('_', "").parse().ok()
}

impl<'a> FnLowerer<'a> {
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
