use std::collections::HashMap;
use std::path::Path;

use marrow_schema::ScalarType;
use marrow_schema::stdlib::Capability;
use marrow_syntax::SourceSpan;

use crate::facts::{EnumId, EnumMemberId};
use crate::program::CheckedProgram;

mod call_target;
mod expr;
mod place;
mod runtime_value;
mod stmt;
mod syntax_parts;

pub use expr::{
    CheckedExpr, CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedLayer,
    CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, CheckedSavedTerminal,
};
use expr::{checked_enum_member_ref_in, function_ref};
pub(crate) use runtime_value::checked_runtime_value_type;
pub use runtime_value::{
    CheckedResourceConstructor, CheckedResourceConstructorField, CheckedResourceRef,
    CheckedRuntimeValueType,
};
use runtime_value::{checked_resource_constructor, resource_ref};
pub use stmt::{CheckedBody, CheckedStmt};
pub use syntax_parts::{
    CheckedArg, CheckedArgMode, CheckedBinaryOp, CheckedCatchClause, CheckedElseIf,
    CheckedForBinding, CheckedInterpolationPart, CheckedLiteralKind, CheckedMatchArm,
    CheckedParamMode, CheckedUnaryOp,
};

pub fn checked_saved_root_place(
    program: &CheckedProgram,
    root: &str,
    span: SourceSpan,
) -> Option<CheckedSavedPlace> {
    place::checked_root_place(program, root, span)
}

pub(crate) struct CheckedExecutableContext<'a> {
    program: &'a CheckedProgram,
    from_module: &'a str,
    source_file: &'a Path,
    aliases: HashMap<String, Vec<String>>,
}

impl<'a> CheckedExecutableContext<'a> {
    pub(crate) fn new(program: &'a CheckedProgram, module_index: usize) -> Self {
        let module = &program.modules[module_index];
        Self {
            program,
            from_module: &module.name,
            source_file: &module.source_file,
            aliases: crate::build_alias_map(&module.imports),
        }
    }

    pub(crate) fn module_name(&self) -> &str {
        self.from_module
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedEnumRef {
    pub enum_id: EnumId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedEnumMemberRef {
    pub enum_ref: CheckedEnumRef,
    pub member_id: EnumMemberId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedCallTarget {
    SavedIndexLookup,
    SavedLayerRead,
    SavedResourceRead,
    ErrorConstructor,
    Builtin(CheckedBuiltinCall),
    Std(CheckedStdCall),
    ResourceConstructor(CheckedResourceConstructor),
    LocalCollection { name: String },
    Function(CheckedFunctionRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedBuiltinCall {
    Print,
    Write,
    Exists,
    NextId,
    Append,
    Bytes,
    ErrorCode,
    Conversion(ScalarType),
    Keys,
    Count,
    Values,
    Entries,
    Reversed,
    Next,
    Prev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedStdCall {
    pub module: &'static str,
    pub op: &'static str,
    pub capability: Capability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedFunctionRef {
    pub module: u32,
    pub function: u32,
}
