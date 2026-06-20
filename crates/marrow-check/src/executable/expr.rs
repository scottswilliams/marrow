use std::collections::HashMap;

use marrow_schema::{MemberPathResolution, ScalarType};
use marrow_syntax::{self as syntax, SourceSpan};

use crate::facts::{
    EnumMemberId, ModuleId, ResourceMemberId, StoreId, StoreIndexId, StoreIndexKeySource,
    StoredValueMeaning,
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
    pub root_span: SourceSpan,
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
    pub name_span: SourceSpan,
    pub catalog_id: Option<String>,
    pub args: Vec<CheckedArg>,
    pub key_params: Vec<CheckedSavedKeyParam>,
    pub leaf: Option<crate::StoreLeafKind>,
    /// The leaf was declared `ErrorCode`, so a value written through this layer
    /// must satisfy the dotted-lowercase grammar. A keyed-leaf or sequence element.
    pub error_code: bool,
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
    /// The leaf was declared `ErrorCode`, so a value written to it must satisfy the
    /// dotted-lowercase grammar.
    pub error_code: bool,
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

    pub(crate) fn contains_keyed_descendant(&self) -> bool {
        !self.key_params.is_empty()
            || self
                .group_members
                .iter()
                .any(Self::contains_keyed_descendant)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedSavedTerminal {
    Record,
    Field {
        name: String,
        span: SourceSpan,
        catalog_id: Option<String>,
        leaf: Option<crate::StoreLeafKind>,
    },
    Index {
        name: String,
        span: SourceSpan,
        catalog_id: Option<String>,
        args: Vec<CheckedArg>,
        unique: bool,
        arg_count: usize,
    },
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
        place: Option<CheckedSavedPlace>,
        span: SourceSpan,
    },
    Field {
        base: Box<CheckedExpr>,
        name: String,
        name_span: SourceSpan,
        quoted: bool,
        place: Option<CheckedSavedPlace>,
        span: SourceSpan,
    },
    OptionalField {
        base: Box<CheckedExpr>,
        name: String,
        name_span: SourceSpan,
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
        scope: &[HashMap<String, MarrowType>],
    ) -> Option<Self> {
        Some(match expr {
            syntax::Expression::Literal { kind, text, span } => Self::Literal {
                kind: CheckedLiteralKind::lower(*kind),
                text: text.clone(),
                span: *span,
            },
            syntax::Expression::Name { segments, span, .. } => Self::Name {
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
                let place =
                    SavedPlaceResolver::new(context.program).call_place(&callee, &args, *span);
                Self::Call {
                    callee,
                    args,
                    target,
                    place,
                    span: *span,
                }
            }
            syntax::Expression::Field {
                base,
                name,
                name_span,
                quoted,
                span,
            } => {
                let base = Box::new(Self::lower(base, context, scope)?);
                let place = SavedPlaceResolver::new(context.program)
                    .field_place(&base, name, *name_span, *span);
                Self::Field {
                    base,
                    name: name.clone(),
                    name_span: *name_span,
                    quoted: *quoted,
                    place,
                    span: *span,
                }
            }
            syntax::Expression::OptionalField {
                base,
                name,
                name_span,
                quoted,
                span,
            } => {
                let base = Box::new(Self::lower(base, context, scope)?);
                let place = SavedPlaceResolver::new(context.program)
                    .field_place(&base, name, *name_span, *span);
                Self::OptionalField {
                    base,
                    name: name.clone(),
                    name_span: *name_span,
                    quoted: *quoted,
                    place,
                    span: *span,
                }
            }
            syntax::Expression::Unary { op, operand, span } => {
                // A `-` over an integer literal folds the sign into the literal text. The
                // runtime parses a literal's magnitude as `i64` before any operator runs,
                // so only the folded `-9223372036854775808` reaches `i64::MIN`; the bare
                // magnitude is `i64::MAX + 1` and would overflow on its own.
                match crate::typerules::negated_integer_literal(*op, operand) {
                    Some((text, literal_span)) => Self::Literal {
                        kind: CheckedLiteralKind::Integer,
                        text: format!("-{text}"),
                        span: literal_span,
                    },
                    None => Self::Unary {
                        op: CheckedUnaryOp::lower(*op),
                        operand: Box::new(Self::lower(operand, context, scope)?),
                        span: *span,
                    },
                }
            }
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
    scope: &[HashMap<String, MarrowType>],
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
    path_spans: &[SourceSpan],
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
    let member_uses =
        checked_enum_member_uses(program, enum_ref, schema, path, path_spans, ordinal);
    Some(CheckedEnumMemberRef {
        enum_ref,
        member_id,
        enum_span: None,
        member_uses,
    })
}

fn checked_enum_member_ref(
    expr: &syntax::Expression,
    context: &CheckedExecutableContext<'_>,
) -> Option<CheckedEnumMemberRef> {
    let syntax::Expression::Name {
        segments,
        segment_spans,
        ..
    } = expr
    else {
        return None;
    };
    let crate::enums::EnumMemberPathResolution::Resolved(resolved) =
        crate::enums::resolve_enum_member_path(
            context.program,
            expr,
            &context.aliases,
            context.source_file,
        )
    else {
        return None;
    };
    if resolved.private.is_some() {
        return None;
    }
    let MemberPathResolution::Found(ordinal) = resolved.member else {
        return None;
    };
    let enum_ref = checked_enum_ref(context.program, &resolved.module, &resolved.enum_name)?;
    let member_id = context
        .program
        .facts
        .enum_member_by_source_order(enum_ref.enum_id, ordinal as u32)?
        .id;
    let member_start = resolved.enum_index + 1;
    let member_uses = checked_enum_member_uses(
        context.program,
        enum_ref,
        resolved.schema,
        &segments[member_start..],
        &segment_spans[member_start..],
        ordinal,
    );
    Some(CheckedEnumMemberRef {
        enum_ref,
        member_id,
        enum_span: segment_spans.get(resolved.enum_index).copied(),
        member_uses,
    })
}

fn checked_enum_member_uses(
    program: &CheckedProgram,
    enum_ref: CheckedEnumRef,
    schema: &marrow_schema::EnumSchema,
    written_path: &[String],
    written_spans: &[SourceSpan],
    terminal_ordinal: usize,
) -> Vec<(EnumMemberId, SourceSpan)> {
    let canonical_path: Vec<String> = schema
        .member_path(terminal_ordinal)
        .into_iter()
        .map(str::to_string)
        .collect();
    if written_path.len() == canonical_path.len()
        && written_path
            .iter()
            .map(String::as_str)
            .eq(canonical_path.iter().map(String::as_str))
    {
        return (1..=canonical_path.len())
            .filter_map(|prefix_len| {
                enum_member_id_for_path(program, enum_ref, schema, &canonical_path[..prefix_len])
                    .zip(written_spans.get(prefix_len - 1).copied())
            })
            .collect();
    }

    program
        .facts
        .enum_member_by_source_order(enum_ref.enum_id, terminal_ordinal as u32)
        .map(|member| member.id)
        .zip(written_spans.last().copied())
        .into_iter()
        .collect()
}

fn enum_member_id_for_path(
    program: &CheckedProgram,
    enum_ref: CheckedEnumRef,
    schema: &marrow_schema::EnumSchema,
    path: &[String],
) -> Option<EnumMemberId> {
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    let MemberPathResolution::Found(ordinal) = schema.walk_member_path(&segments) else {
        return None;
    };
    program
        .facts
        .enum_member_by_source_order(enum_ref.enum_id, ordinal as u32)
        .map(|member| member.id)
}

fn module_index(program: &CheckedProgram, module: &str) -> Option<usize> {
    program
        .modules
        .iter()
        .position(|candidate| candidate.name == module)
}
