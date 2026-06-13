use std::collections::{BTreeSet, HashMap};

use marrow_schema::{MemberPathResolution, ScalarType};
use marrow_syntax::{self as syntax, SourceSpan};

use crate::facts::{
    ModuleId, ResourceMemberId, StoreId, StoreIndexId, StoreIndexKeySource, StoredValueMeaning,
};
use crate::program::{CheckedFunction, CheckedModule, CheckedProgram, MarrowType};

use super::place::SavedPlaceResolver;
use super::{
    CheckedArg, CheckedBinaryOp, CheckedCallTarget, CheckedEnumMemberRef, CheckedEnumRef,
    CheckedExecutableContext, CheckedFunctionRef, CheckedInterpolationPart, CheckedLiteralKind,
    CheckedUnaryOp,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSavedPlace {
    pub root: String,
    pub store_id: StoreId,
    pub store_catalog_id: Option<String>,
    pub resource_name: String,
    pub root_members: Vec<CheckedSavedMember>,
    pub members: Vec<CheckedSavedMember>,
    pub indexes: Vec<CheckedSavedIndex>,
    pub identity_args: Vec<CheckedArg>,
    pub identity_keys: Vec<CheckedSavedKeyParam>,
    pub next_id_shape: String,
    pub layers: Vec<CheckedSavedLayer>,
    pub terminal: CheckedSavedTerminal,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteFallibilityFact {
    Infallible,
    UniqueConflict(BTreeSet<StoreIndexId>),
    MaintenanceGated,
}

impl WriteFallibilityFact {
    pub(crate) fn for_assignment(place: Option<&CheckedSavedPlace>) -> Self {
        let Some(place) = place else {
            return Self::Infallible;
        };
        let conflicts = assignment_unique_conflicts(place);
        if conflicts.is_empty() {
            Self::Infallible
        } else {
            Self::UniqueConflict(conflicts)
        }
    }

    pub(crate) fn for_delete(place: Option<&CheckedSavedPlace>) -> Self {
        let Some(place) = place else {
            return Self::Infallible;
        };
        if deletes_keyed_root(place)
            || deletes_required_field(place)
            || deletes_unkeyed_required_group(place)
        {
            Self::MaintenanceGated
        } else {
            Self::Infallible
        }
    }

    pub(crate) fn for_append(place: Option<&CheckedSavedPlace>) -> Option<Self> {
        place.map(|_| Self::Infallible)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSavedIndex {
    pub id: StoreIndexId,
    pub name: String,
    pub catalog_id: Option<String>,
    pub unique: bool,
    pub keys: Vec<CheckedSavedIndexKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSavedIndexKey {
    pub name: String,
    pub source: StoreIndexKeySource,
    pub value_meaning: StoredValueMeaning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSavedKeyParam {
    pub name: String,
    pub scalar: Option<ScalarType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSavedLayer {
    pub id: Option<ResourceMemberId>,
    pub name: String,
    pub catalog_id: Option<String>,
    pub args: Vec<CheckedArg>,
    pub key_params: Vec<CheckedSavedKeyParam>,
    pub leaf: Option<crate::StoreLeafKind>,
    pub typed_entry: bool,
    pub members: Vec<CheckedSavedMember>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSavedMember {
    pub id: Option<ResourceMemberId>,
    pub name: String,
    pub key_params: Vec<CheckedSavedKeyParam>,
    pub kind: CheckedSavedMemberKind,
    pub catalog_id: Option<String>,
    pub leaf: Option<crate::StoreLeafKind>,
    pub typed_entry: bool,
    pub group_members: Vec<CheckedSavedMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedSavedMemberKind {
    Field { required: bool },
    Group,
}

impl CheckedSavedMember {
    pub fn is_plain_field(&self) -> bool {
        self.key_params.is_empty() && matches!(self.kind, CheckedSavedMemberKind::Field { .. })
    }

    pub fn is_unkeyed_group(&self) -> bool {
        self.key_params.is_empty() && matches!(self.kind, CheckedSavedMemberKind::Group)
    }

    pub fn is_field(&self) -> bool {
        matches!(self.kind, CheckedSavedMemberKind::Field { .. })
    }

    pub fn plain_field(&self) -> Option<(&crate::StoreLeafKind, bool)> {
        match &self.kind {
            CheckedSavedMemberKind::Field { required } if self.key_params.is_empty() => {
                self.leaf.as_ref().map(|leaf| (leaf, *required))
            }
            _ => None,
        }
    }

    pub fn field(&self) -> Option<(&crate::StoreLeafKind, bool)> {
        match &self.kind {
            CheckedSavedMemberKind::Field { required } => {
                self.leaf.as_ref().map(|leaf| (leaf, *required))
            }
            CheckedSavedMemberKind::Group => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedSavedTerminal {
    Record,
    Field {
        name: String,
        catalog_id: Option<String>,
        leaf: Option<crate::StoreLeafKind>,
    },
    Index {
        name: String,
        catalog_id: Option<String>,
        args: Vec<CheckedArg>,
        unique: bool,
        arg_count: usize,
    },
}

fn assignment_unique_conflicts(place: &CheckedSavedPlace) -> BTreeSet<StoreIndexId> {
    match &place.terminal {
        CheckedSavedTerminal::Record if place.layers.is_empty() => place
            .indexes
            .iter()
            .filter(|index| index.unique)
            .map(|index| index.id)
            .collect(),
        CheckedSavedTerminal::Field { name, .. } if place.layers.is_empty() => {
            let field_id = place
                .root_members
                .iter()
                .find(|member| member.name == *name)
                .and_then(|member| member.id);
            place
                .indexes
                .iter()
                .filter(|index| index.unique)
                .filter(|index| {
                    index.keys.iter().any(|key| {
                        key.name == *name
                            && field_id.is_none_or(|id| {
                                key.source == StoreIndexKeySource::ResourceMember(id)
                            })
                    })
                })
                .map(|index| index.id)
                .collect()
        }
        _ => BTreeSet::new(),
    }
}

fn deletes_keyed_root(place: &CheckedSavedPlace) -> bool {
    matches!(place.terminal, CheckedSavedTerminal::Record)
        && place.layers.is_empty()
        && !place.identity_keys.is_empty()
        && place.identity_args.is_empty()
}

fn deletes_required_field(place: &CheckedSavedPlace) -> bool {
    let CheckedSavedTerminal::Field { name, .. } = &place.terminal else {
        return false;
    };
    place
        .members
        .iter()
        .find(|member| member.name == *name)
        .is_some_and(|member| {
            matches!(
                member.kind,
                CheckedSavedMemberKind::Field { required: true }
            )
        })
}

fn deletes_unkeyed_required_group(place: &CheckedSavedPlace) -> bool {
    let CheckedSavedTerminal::Record = place.terminal else {
        return false;
    };
    let Some(layer) = place.layers.last() else {
        return false;
    };
    layer.key_params.is_empty()
        && layer.leaf.is_none()
        && members_contain_required_field(&layer.members)
}

fn members_contain_required_field(members: &[CheckedSavedMember]) -> bool {
    members.iter().any(|member| match member.kind {
        CheckedSavedMemberKind::Field { required } => required,
        CheckedSavedMemberKind::Group => members_contain_required_field(&member.group_members),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedExpr {
    Literal {
        kind: CheckedLiteralKind,
        text: String,
        span: SourceSpan,
    },
    Name {
        segments: Vec<String>,
        enum_member: Option<CheckedEnumMemberRef>,
        span: SourceSpan,
    },
    SavedRoot {
        name: String,
        place: Option<CheckedSavedPlace>,
        span: SourceSpan,
    },
    Call {
        callee: Box<CheckedExpr>,
        args: Vec<CheckedArg>,
        target: CheckedCallTarget,
        write_fallibility: Option<WriteFallibilityFact>,
        place: Option<CheckedSavedPlace>,
        span: SourceSpan,
    },
    Field {
        base: Box<CheckedExpr>,
        name: String,
        quoted: bool,
        place: Option<CheckedSavedPlace>,
        span: SourceSpan,
    },
    OptionalField {
        base: Box<CheckedExpr>,
        name: String,
        quoted: bool,
        place: Option<CheckedSavedPlace>,
        span: SourceSpan,
    },
    Unary {
        op: CheckedUnaryOp,
        operand: Box<CheckedExpr>,
        span: SourceSpan,
    },
    Binary {
        op: CheckedBinaryOp,
        left: Box<CheckedExpr>,
        right: Box<CheckedExpr>,
        span: SourceSpan,
    },
    Range {
        start: Option<Box<CheckedExpr>>,
        end: Option<Box<CheckedExpr>>,
        inclusive_end: bool,
        step: Option<Box<CheckedExpr>>,
        span: SourceSpan,
    },
    Interpolation {
        parts: Vec<CheckedInterpolationPart>,
        span: SourceSpan,
    },
}

impl CheckedExpr {
    pub(crate) fn lower(
        expr: &syntax::Expression,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(match expr {
            syntax::Expression::Literal { kind, text, span } => Self::Literal {
                kind: CheckedLiteralKind::lower(*kind),
                text: text.clone(),
                span: *span,
            },
            syntax::Expression::Name { segments, span } => Self::Name {
                segments: segments.clone(),
                enum_member: checked_enum_member_ref(expr, context),
                span: *span,
            },
            syntax::Expression::SavedRoot { name, span } => Self::SavedRoot {
                name: name.clone(),
                place: SavedPlaceResolver::new(context.program).root_place(name, *span),
                span: *span,
            },
            syntax::Expression::Call {
                callee, args, span, ..
            } => {
                let callee = Box::new(Self::lower(callee, context, scope)?);
                let args = args
                    .iter()
                    .map(|arg| CheckedArg::lower(arg, context, scope))
                    .collect::<Option<Vec<_>>>()?;
                let target = CheckedCallTarget::for_call(
                    &callee,
                    &args,
                    context.program,
                    context.module_name(),
                    &context.aliases,
                    scope,
                )?;
                let write_fallibility = match &target {
                    CheckedCallTarget::Builtin(super::CheckedBuiltinCall::Append) => {
                        WriteFallibilityFact::for_append(
                            args.first().and_then(|arg| arg.value.saved_place()),
                        )
                    }
                    _ => None,
                };
                let place =
                    SavedPlaceResolver::new(context.program).call_place(&callee, &args, *span);
                Self::Call {
                    callee,
                    args,
                    target,
                    write_fallibility,
                    place,
                    span: *span,
                }
            }
            syntax::Expression::Field {
                base,
                name,
                quoted,
                span,
            } => {
                let base = Box::new(Self::lower(base, context, scope)?);
                let place =
                    SavedPlaceResolver::new(context.program).field_place(&base, name, *span);
                Self::Field {
                    base,
                    name: name.clone(),
                    quoted: *quoted,
                    place,
                    span: *span,
                }
            }
            syntax::Expression::OptionalField {
                base,
                name,
                quoted,
                span,
            } => {
                let base = Box::new(Self::lower(base, context, scope)?);
                let place =
                    SavedPlaceResolver::new(context.program).field_place(&base, name, *span);
                Self::OptionalField {
                    base,
                    name: name.clone(),
                    quoted: *quoted,
                    place,
                    span: *span,
                }
            }
            syntax::Expression::Unary { op, operand, span } => Self::Unary {
                op: CheckedUnaryOp::lower(*op),
                operand: Box::new(Self::lower(operand, context, scope)?),
                span: *span,
            },
            syntax::Expression::Binary {
                op,
                left,
                right,
                span,
            } => Self::Binary {
                op: CheckedBinaryOp::lower(*op),
                left: Box::new(Self::lower(left, context, scope)?),
                right: Box::new(Self::lower(right, context, scope)?),
                span: *span,
            },
            syntax::Expression::Range {
                start,
                end,
                inclusive_end,
                step,
                span,
            } => Self::Range {
                start: match start {
                    Some(start) => Some(Box::new(Self::lower(start, context, scope)?)),
                    None => None,
                },
                end: match end {
                    Some(end) => Some(Box::new(Self::lower(end, context, scope)?)),
                    None => None,
                },
                inclusive_end: *inclusive_end,
                step: match step {
                    Some(step) => Some(Box::new(Self::lower(step, context, scope)?)),
                    None => None,
                },
                span: *span,
            },
            syntax::Expression::Interpolation { parts, span } => Self::Interpolation {
                parts: parts
                    .iter()
                    .map(|part| CheckedInterpolationPart::lower(part, context, scope))
                    .collect::<Option<Vec<_>>>()?,
                span: *span,
            },
        })
    }

    pub fn saved_place(&self) -> Option<&CheckedSavedPlace> {
        match self {
            Self::SavedRoot { place, .. }
            | Self::Call { place, .. }
            | Self::Field { place, .. }
            | Self::OptionalField { place, .. } => place.as_ref(),
            Self::Literal { .. }
            | Self::Name { .. }
            | Self::Unary { .. }
            | Self::Binary { .. }
            | Self::Range { .. }
            | Self::Interpolation { .. } => None,
        }
    }

    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Literal { span, .. }
            | Self::Name { span, .. }
            | Self::SavedRoot { span, .. }
            | Self::Call { span, .. }
            | Self::Field { span, .. }
            | Self::OptionalField { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Range { span, .. }
            | Self::Interpolation { span, .. } => *span,
        }
    }
}

pub(super) fn lower_optional_expr(
    expr: Option<&syntax::Expression>,
    context: &CheckedExecutableContext<'_>,
    scope: &mut Vec<HashMap<String, MarrowType>>,
) -> Option<Option<CheckedExpr>> {
    match expr {
        Some(expr) => Some(Some(CheckedExpr::lower(expr, context, scope)?)),
        None => Some(None),
    }
}

pub(super) fn function_ref(
    program: &CheckedProgram,
    module: &CheckedModule,
    function: &CheckedFunction,
) -> Option<CheckedFunctionRef> {
    let module_index = program
        .modules
        .iter()
        .position(|candidate| std::ptr::eq(candidate, module))?;
    let function_index = module
        .functions
        .iter()
        .position(|candidate| std::ptr::eq(candidate, function))?;
    Some(CheckedFunctionRef {
        module: module_index as u32,
        function: function_index as u32,
        presence: function.return_presence,
    })
}

pub(super) fn checked_enum_ref(
    program: &CheckedProgram,
    module: &str,
    name: &str,
) -> Option<CheckedEnumRef> {
    let module_index = module_index(program, module)?;
    let enum_id = program.facts.enum_id(ModuleId(module_index as u32), name)?;
    Some(CheckedEnumRef { enum_id })
}

pub(super) fn checked_enum_member_ref_in(
    program: &CheckedProgram,
    module: &str,
    enum_name: &str,
    path: &[String],
) -> Option<CheckedEnumMemberRef> {
    let enum_ref = checked_enum_ref(program, module, enum_name)?;
    let module_index = module_index(program, module)?;
    let schema = program.modules[module_index]
        .enums
        .iter()
        .find(|schema| schema.name == enum_name)?;
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    let MemberPathResolution::Found(ordinal) = schema.walk_member_path(&segments) else {
        return None;
    };
    let member_id = program
        .facts
        .enum_member_by_source_order(enum_ref.enum_id, ordinal as u32)?
        .id;
    Some(CheckedEnumMemberRef {
        enum_ref,
        member_id,
    })
}

fn checked_enum_member_ref(
    expr: &syntax::Expression,
    context: &CheckedExecutableContext<'_>,
) -> Option<CheckedEnumMemberRef> {
    let resolved = crate::enums::resolve_enum_member_path(
        context.program,
        expr,
        &context.aliases,
        context.source_file,
    )?;
    let MemberPathResolution::Found(ordinal) = resolved.member else {
        return None;
    };
    let enum_ref = checked_enum_ref(context.program, &resolved.module, &resolved.enum_name)?;
    let member_id = context
        .program
        .facts
        .enum_member_by_source_order(enum_ref.enum_id, ordinal as u32)?
        .id;
    Some(CheckedEnumMemberRef {
        enum_ref,
        member_id,
    })
}

fn module_index(program: &CheckedProgram, module: &str) -> Option<usize> {
    program
        .modules
        .iter()
        .position(|candidate| candidate.name == module)
}
