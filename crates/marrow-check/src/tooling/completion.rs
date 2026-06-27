use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_schema::{EnumSchema, ResourceSchema, StoreSchema, stdlib};
use marrow_store::cell::CatalogId;
use marrow_syntax::{
    Declaration, FunctionDecl, LexedSource, ParsedSource, SourceFile, SourceSpan, Token, TokenKind,
};

use super::data::{
    DeclaredDataChild, DeclaredDataChildKind, declared_source_receiver_data_children_fact,
};
use super::expected::{ExpectedEnum, ExpectedEnumContext, expected_enum_at};
use super::render::{render_callable_signature, render_marrow_type};
use super::signatures::{CallableSignature, active_callable_context, intrinsic_callable_signature};
use crate::analysis::{ScopeCompletionBindingKind, scope_completion_bindings_at};
use crate::{
    CheckedFunction, CheckedModule, CheckedProgram, CheckedSavedKeyParam, CheckedSavedPlace,
    MarrowType, ResourceMemberId, ScalarType, StoreId, StoreLeafKind,
};

pub const SOURCE_COMPLETION_PROFILE_VERSION: &str = "source.completion.v1";

const BUILTIN_TYPE_COMPLETIONS: &[SourceTypeBuiltin] = &[
    SourceTypeBuiltin::Int,
    SourceTypeBuiltin::Decimal,
    SourceTypeBuiltin::Bool,
    SourceTypeBuiltin::String,
    SourceTypeBuiltin::Bytes,
    SourceTypeBuiltin::Date,
    SourceTypeBuiltin::Instant,
    SourceTypeBuiltin::Duration,
    SourceTypeBuiltin::ErrorCode,
    SourceTypeBuiltin::Sequence,
    SourceTypeBuiltin::Unknown,
    SourceTypeBuiltin::Error,
];

const KEYWORDS: &[&str] = &[
    "const",
    "var",
    "if",
    "else",
    "while",
    "for",
    "in",
    "break",
    "continue",
    "return",
    "delete",
    "transaction",
    "try",
    "catch",
    "throw",
    "match",
    "true",
    "false",
    "not",
    "and",
    "or",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCompletionFact {
    pub profile_version: &'static str,
    pub items: Vec<SourceCompletionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCompletionItem {
    pub label: String,
    pub kind: SourceCompletionItemKind,
    pub detail: Option<String>,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCompletionItemKind {
    Keyword,
    Local,
    Function,
    Resource,
    SavedRoot,
    Field,
    Layer,
    Enum,
    EnumMember,
    Module,
    StoreIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTypeCompletionFact {
    pub candidates: Vec<SourceTypeCompletionCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSavedRootCompletionFact {
    pub candidates: Vec<SourceSavedRootCompletionCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSavedPathCompletionFact {
    pub context: SourceSavedPathCompletionContextFact,
    pub children: Vec<DeclaredDataChild>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSavedPathCompletionContextFact {
    pub receiver_span: SourceSpan,
    pub root: SourceSavedPathCompletionRoot,
    pub segments: Vec<SourceSavedPathCompletionSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSavedPathCompletionRoot {
    pub name: String,
    pub store_id: StoreId,
    pub store_catalog_id: Option<CatalogId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSavedPathCompletionSegment {
    Root {
        name: String,
        store_id: StoreId,
        store_catalog_id: Option<CatalogId>,
    },
    KeySlot {
        name: String,
        scalar: Option<ScalarType>,
    },
    Layer {
        name: String,
        member_id: Option<ResourceMemberId>,
        member_catalog_id: Option<CatalogId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSavedRootCompletionCandidate {
    pub root: String,
    pub module: String,
    pub resource_name: String,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceTypeCompletionCandidate {
    Builtin {
        spelling: SourceTypeBuiltin,
    },
    Resource {
        path: Vec<String>,
        module: String,
        name: String,
        docs: Vec<String>,
    },
    StoreIdentity {
        root: String,
        docs: Vec<String>,
    },
    Enum {
        path: Vec<String>,
        module: String,
        name: String,
        docs: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTypeBuiltin {
    Int,
    Decimal,
    Bool,
    String,
    Bytes,
    Date,
    Instant,
    Duration,
    ErrorCode,
    Sequence,
    Unknown,
    Error,
}

impl SourceTypeBuiltin {
    pub fn spelling(self) -> &'static str {
        match self {
            SourceTypeBuiltin::Int => "int",
            SourceTypeBuiltin::Decimal => "decimal",
            SourceTypeBuiltin::Bool => "bool",
            SourceTypeBuiltin::String => "string",
            SourceTypeBuiltin::Bytes => "bytes",
            SourceTypeBuiltin::Date => "date",
            SourceTypeBuiltin::Instant => "instant",
            SourceTypeBuiltin::Duration => "duration",
            SourceTypeBuiltin::ErrorCode => "ErrorCode",
            SourceTypeBuiltin::Sequence => "sequence",
            SourceTypeBuiltin::Unknown => "unknown",
            SourceTypeBuiltin::Error => "Error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceNamespaceCompletionFact {
    Module(SourceModuleNamespaceCompletionFact),
    Enum(SourceEnumNamespaceCompletionFact),
    StandardLibraryRoot(SourceStandardLibraryRootNamespaceCompletionFact),
    StandardLibraryModule(SourceStandardLibraryModuleNamespaceCompletionFact),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceModuleNamespaceCompletionFact {
    pub module: String,
    pub resources: Vec<SourceNamespaceResourceCompletion>,
    pub enums: Vec<SourceNamespaceEnumCompletion>,
    pub functions: Vec<SourceNamespaceFunctionCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStandardLibraryRootNamespaceCompletionFact {
    pub modules: Vec<SourceStandardLibraryModuleCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStandardLibraryModuleCompletion {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStandardLibraryModuleNamespaceCompletionFact {
    pub module: String,
    pub operations: Vec<SourceStandardLibraryOperationCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStandardLibraryOperationCompletion {
    pub name: String,
    pub signature: CallableSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceResourceCompletion {
    pub name: String,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceEnumCompletion {
    pub name: String,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceFunctionCompletion {
    pub name: String,
    pub params: Vec<SourceNamespaceFunctionParamCompletion>,
    pub return_type: Option<MarrowType>,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceFunctionParamCompletion {
    pub name: String,
    pub ty: MarrowType,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEnumNamespaceCompletionFact {
    pub enum_name: String,
    pub members: Vec<SourceNamespaceEnumMemberCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceEnumMemberCompletion {
    pub name: String,
    pub docs: Vec<String>,
    pub status: SourceNamespaceEnumMemberStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceNamespaceEnumMemberStatus {
    Selectable,
    Category,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceCompletionContext {
    Root,
    SavedPath { receiver_span: SourceSpan },
    InvalidSavedPath,
    Namespace { qualifier: Vec<String> },
    Type,
    Bare,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionCursorContext {
    Root,
    SavedPath { receiver: String, span: SourceSpan },
    InvalidSavedPath,
    Namespace { qualifier: Vec<String> },
    Type,
    Bare,
}

pub fn source_completion_context(
    source: &str,
    lexed: &LexedSource,
    offset: usize,
) -> SourceCompletionContext {
    let parsed = marrow_syntax::parse_source(source);
    match completion_cursor_context(source, lexed, &parsed, offset) {
        CompletionCursorContext::Root => SourceCompletionContext::Root,
        CompletionCursorContext::SavedPath { span, .. } => SourceCompletionContext::SavedPath {
            receiver_span: span,
        },
        CompletionCursorContext::InvalidSavedPath => SourceCompletionContext::InvalidSavedPath,
        CompletionCursorContext::Namespace { qualifier } => {
            SourceCompletionContext::Namespace { qualifier }
        }
        CompletionCursorContext::Type => SourceCompletionContext::Type,
        CompletionCursorContext::Bare => SourceCompletionContext::Bare,
    }
}

fn completion_cursor_context(
    source: &str,
    lexed: &LexedSource,
    parsed: &ParsedSource,
    offset: usize,
) -> CompletionCursorContext {
    let tokens = significant_tokens(lexed);
    let Some(driver) = tokens
        .iter()
        .rposition(|token| token.span.start_byte < offset)
    else {
        return CompletionCursorContext::Bare;
    };

    let anchor =
        if tokens[driver].kind == TokenKind::Identifier && tokens[driver].span.end_byte >= offset {
            driver.checked_sub(1)
        } else {
            Some(driver)
        };

    let Some(anchor) = anchor else {
        return CompletionCursorContext::Bare;
    };

    match tokens[anchor].kind {
        TokenKind::Caret => CompletionCursorContext::Root,
        TokenKind::DoubleColon => match qualifier_before(source, &tokens, anchor) {
            Some(qualifier) => CompletionCursorContext::Namespace { qualifier },
            None => CompletionCursorContext::Bare,
        },
        TokenKind::Dot | TokenKind::QuestionDot => {
            match saved_path_before_dot(source, &tokens, anchor) {
                SavedPathRecovery::Complete { receiver, span } => {
                    CompletionCursorContext::SavedPath { receiver, span }
                }
                SavedPathRecovery::Invalid => CompletionCursorContext::InvalidSavedPath,
                SavedPathRecovery::NotSavedPath => CompletionCursorContext::Bare,
            }
        }
        TokenKind::DotDot | TokenKind::DotDotEqual => {
            if saved_path_attempt_before_dot(&tokens, anchor) {
                CompletionCursorContext::InvalidSavedPath
            } else {
                CompletionCursorContext::Bare
            }
        }
        TokenKind::Colon => {
            if introduces_type(source, lexed, parsed, &tokens, anchor, offset) {
                CompletionCursorContext::Type
            } else {
                CompletionCursorContext::Bare
            }
        }
        _ if malformed_saved_member_receiver(&tokens, anchor) => {
            CompletionCursorContext::InvalidSavedPath
        }
        _ => CompletionCursorContext::Bare,
    }
}

pub fn source_completion_fact(
    program: &CheckedProgram,
    file: &Path,
    source: &str,
    parsed: &ParsedSource,
    lexed: &LexedSource,
    offset: usize,
) -> SourceCompletionFact {
    let items = match completion_cursor_context(source, lexed, parsed, offset) {
        CompletionCursorContext::Root => root_completion_items(program),
        CompletionCursorContext::SavedPath { receiver, span } => {
            let fact = source_saved_path_completion_fact_from_receiver(
                program, file, parsed, &receiver, span,
            );
            fact.as_ref()
                .map(saved_path_completion_items)
                .unwrap_or_default()
        }
        CompletionCursorContext::InvalidSavedPath => Vec::new(),
        CompletionCursorContext::Namespace { qualifier } => {
            namespace_completion_items(program, file, parsed, offset, &qualifier)
        }
        CompletionCursorContext::Type => type_completion_items(program, file, &parsed.file),
        CompletionCursorContext::Bare => {
            bare_completion_items(program, file, source, parsed, lexed, offset)
        }
    };
    SourceCompletionFact {
        profile_version: SOURCE_COMPLETION_PROFILE_VERSION,
        items,
    }
}

pub fn source_saved_path_completion_fact_at(
    program: &CheckedProgram,
    file: &Path,
    source: &str,
    parsed: &ParsedSource,
    lexed: &LexedSource,
    offset: usize,
) -> Option<SourceSavedPathCompletionFact> {
    match completion_cursor_context(source, lexed, parsed, offset) {
        CompletionCursorContext::SavedPath { receiver, span } => {
            source_saved_path_completion_fact_from_receiver(program, file, parsed, &receiver, span)
        }
        _ => None,
    }
}

fn root_completion_items(program: &CheckedProgram) -> Vec<SourceCompletionItem> {
    source_saved_root_completion_fact(program)
        .candidates
        .iter()
        .map(saved_root_completion_item)
        .collect()
}

fn saved_root_completion_item(
    candidate: &SourceSavedRootCompletionCandidate,
) -> SourceCompletionItem {
    source_completion_item(&candidate.root, SourceCompletionItemKind::SavedRoot)
        .detail(format!("saved root of {}", candidate.resource_name))
        .docs_from(&candidate.docs)
}

fn saved_path_completion_items(fact: &SourceSavedPathCompletionFact) -> Vec<SourceCompletionItem> {
    fact.children
        .iter()
        .map(declared_data_child_completion_item)
        .collect()
}

fn source_saved_path_completion_fact_from_receiver(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    receiver: &str,
    span: SourceSpan,
) -> Option<SourceSavedPathCompletionFact> {
    let receiver_fact =
        declared_source_receiver_data_children_fact(program, file, parsed, receiver, span)?;
    Some(SourceSavedPathCompletionFact {
        context: source_saved_path_completion_context_fact(&receiver_fact.place, span),
        children: receiver_fact.children,
    })
}

fn source_saved_path_completion_context_fact(
    place: &CheckedSavedPlace,
    receiver_span: SourceSpan,
) -> SourceSavedPathCompletionContextFact {
    SourceSavedPathCompletionContextFact {
        receiver_span,
        root: SourceSavedPathCompletionRoot {
            name: place.root.clone(),
            store_id: place.store_id,
            store_catalog_id: catalog_id(&place.store_catalog_id),
        },
        segments: source_saved_path_completion_segments(place),
    }
}

fn source_saved_path_completion_segments(
    place: &CheckedSavedPlace,
) -> Vec<SourceSavedPathCompletionSegment> {
    let mut segments = vec![SourceSavedPathCompletionSegment::Root {
        name: place.root.clone(),
        store_id: place.store_id,
        store_catalog_id: catalog_id(&place.store_catalog_id),
    }];
    segments.extend(key_slot_segments(&place.identity_keys));
    for layer in &place.layers {
        segments.push(SourceSavedPathCompletionSegment::Layer {
            name: layer.name.clone(),
            member_id: layer.id,
            member_catalog_id: catalog_id(&layer.catalog_id),
        });
        segments.extend(key_slot_segments(&layer.key_params));
    }
    segments
}

fn key_slot_segments(
    keys: &[CheckedSavedKeyParam],
) -> impl Iterator<Item = SourceSavedPathCompletionSegment> + '_ {
    keys.iter()
        .map(|key| SourceSavedPathCompletionSegment::KeySlot {
            name: key.name.clone(),
            scalar: key.scalar,
        })
}

fn catalog_id(raw: &Option<String>) -> Option<CatalogId> {
    CatalogId::new(raw.as_ref()?.clone()).ok()
}

fn declared_data_child_completion_item(child: &DeclaredDataChild) -> SourceCompletionItem {
    let kind = match child.kind {
        DeclaredDataChildKind::Field { .. } => SourceCompletionItemKind::Field,
        DeclaredDataChildKind::Layer => SourceCompletionItemKind::Layer,
    };
    source_completion_item(&child.name, kind).detail(declared_data_child_detail(child))
}

fn declared_data_child_detail(child: &DeclaredDataChild) -> String {
    let mut detail = match child.kind {
        DeclaredDataChildKind::Field { required: true } => "required field".to_string(),
        DeclaredDataChildKind::Field { required: false } => "field".to_string(),
        DeclaredDataChildKind::Layer => "layer".to_string(),
    };
    if !child.key_params.is_empty() {
        detail.push('(');
        detail.push_str(&declared_key_params_detail(child));
        detail.push(')');
    }
    if let Some(leaf) = child.leaf.as_ref().and_then(store_leaf_detail) {
        detail.push_str(": ");
        detail.push_str(&leaf);
    }
    detail
}

fn declared_key_params_detail(child: &DeclaredDataChild) -> String {
    child
        .key_params
        .iter()
        .map(|param| match param.scalar {
            Some(scalar) => format!("{}: {}", param.name, scalar.name()),
            None => param.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn store_leaf_detail(leaf: &StoreLeafKind) -> Option<String> {
    match leaf {
        StoreLeafKind::Scalar(scalar) => Some(scalar.name().to_string()),
        StoreLeafKind::Identity { store_root, .. } => Some(format!("Id(^{store_root})")),
        StoreLeafKind::Enum { .. } => None,
    }
}

fn namespace_completion_items(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    offset: usize,
    qualifier: &[String],
) -> Vec<SourceCompletionItem> {
    match source_namespace_completion_fact_at(program, file, parsed, offset, qualifier) {
        Some(SourceNamespaceCompletionFact::Module(fact)) => module_namespace_items(&fact),
        Some(SourceNamespaceCompletionFact::Enum(fact)) => enum_member_items(&fact),
        Some(SourceNamespaceCompletionFact::StandardLibraryRoot(fact)) => {
            standard_library_root_items(&fact)
        }
        Some(SourceNamespaceCompletionFact::StandardLibraryModule(fact)) => {
            standard_library_module_items(&fact)
        }
        None => Vec::new(),
    }
}

fn standard_library_root_items(
    fact: &SourceStandardLibraryRootNamespaceCompletionFact,
) -> Vec<SourceCompletionItem> {
    fact.modules
        .iter()
        .map(|module| {
            source_completion_item(&module.name, SourceCompletionItemKind::Module)
                .detail("std module")
        })
        .collect()
}

fn standard_library_module_items(
    fact: &SourceStandardLibraryModuleNamespaceCompletionFact,
) -> Vec<SourceCompletionItem> {
    fact.operations
        .iter()
        .map(|operation| {
            source_completion_item(&operation.name, SourceCompletionItemKind::Function)
                .detail(render_callable_signature(&operation.signature))
                .docs_from(&operation.signature.docs)
        })
        .collect()
}

fn module_namespace_items(fact: &SourceModuleNamespaceCompletionFact) -> Vec<SourceCompletionItem> {
    let mut items = Vec::new();
    items.extend(fact.resources.iter().map(|resource| {
        source_completion_item(&resource.name, SourceCompletionItemKind::Resource)
            .detail("resource")
            .docs_from(&resource.docs)
    }));
    items.extend(fact.enums.iter().map(|enum_schema| {
        source_completion_item(&enum_schema.name, SourceCompletionItemKind::Enum)
            .detail("enum")
            .docs_from(&enum_schema.docs)
    }));
    items.extend(fact.functions.iter().map(|function| {
        source_completion_item(&function.name, SourceCompletionItemKind::Function)
            .detail(source_function_signature(function))
            .docs_from(&function.docs)
    }));
    dedup_completion_items(items)
}

fn enum_member_items(fact: &SourceEnumNamespaceCompletionFact) -> Vec<SourceCompletionItem> {
    fact.members
        .iter()
        .map(|member| {
            source_completion_item(&member.name, SourceCompletionItemKind::EnumMember)
                .detail(fact.enum_name.clone())
                .docs_from(&member.docs)
        })
        .collect()
}

fn source_function_signature(function: &SourceNamespaceFunctionCompletion) -> String {
    let params = function
        .params
        .iter()
        .map(|param| format!("{}: {}", param.name, render_marrow_type(&param.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    match &function.return_type {
        Some(return_type) => format!(
            "fn {}({params}): {}",
            function.name,
            render_marrow_type(return_type)
        ),
        None => format!("fn {}({params})", function.name),
    }
}

fn type_completion_items(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
) -> Vec<SourceCompletionItem> {
    source_type_completion_fact(program, file, source_file)
        .candidates
        .iter()
        .map(type_completion_item)
        .collect()
}

fn type_completion_item(candidate: &SourceTypeCompletionCandidate) -> SourceCompletionItem {
    match candidate {
        SourceTypeCompletionCandidate::Builtin { spelling } => {
            source_completion_item(spelling.spelling(), SourceCompletionItemKind::Keyword)
                .detail("type")
        }
        SourceTypeCompletionCandidate::Resource { path, docs, .. } => source_completion_item(
            &source_type_path_label(path),
            SourceCompletionItemKind::Resource,
        )
        .detail("resource")
        .docs_from(docs),
        SourceTypeCompletionCandidate::StoreIdentity { root, docs } => source_completion_item(
            &format!("Id(^{root})"),
            SourceCompletionItemKind::StoreIdentity,
        )
        .detail(format!("identity of ^{root}"))
        .docs_from(docs),
        SourceTypeCompletionCandidate::Enum { path, docs, .. } => source_completion_item(
            &source_type_path_label(path),
            SourceCompletionItemKind::Enum,
        )
        .detail("enum")
        .docs_from(docs),
    }
}

fn source_type_path_label(path: &[String]) -> String {
    path.join("::")
}

fn bare_completion_items(
    program: &CheckedProgram,
    file: &Path,
    source: &str,
    parsed: &ParsedSource,
    lexed: &LexedSource,
    offset: usize,
) -> Vec<SourceCompletionItem> {
    let mut items = Vec::new();
    let scope_bindings = scope_completion_bindings_at(program, file, parsed, offset);
    for binding in &scope_bindings {
        if let ScopeCompletionBindingKind::Value { ty } = &binding.kind {
            items.push(
                source_completion_item(&binding.name, SourceCompletionItemKind::Local)
                    .detail(render_marrow_type(ty)),
            );
        }
    }
    items.push(
        source_completion_item("std", SourceCompletionItemKind::Module).detail("standard library"),
    );
    for binding in scope_bindings {
        match binding.kind {
            ScopeCompletionBindingKind::ModuleAlias { module } => items.push(
                source_completion_item(&binding.name, SourceCompletionItemKind::Module)
                    .detail(format!("module {}", module.join("::"))),
            ),
            ScopeCompletionBindingKind::Value { .. } => {}
        }
    }
    items.extend(current_module_bare_items(program, file, &parsed.file));
    for keyword in KEYWORDS {
        items.push(source_completion_item(
            keyword,
            SourceCompletionItemKind::Keyword,
        ));
    }
    for callable in super::signatures::intrinsic_completion_callables() {
        let label = callable.path.join("::");
        items.push(
            source_completion_item(&label, SourceCompletionItemKind::Function)
                .detail(render_callable_signature(&callable))
                .docs_from(&callable.docs),
        );
    }
    if let Some(expected) = expected_enum_at(program, file, source, parsed, lexed, offset) {
        items.extend(expected_enum_member_items(&expected));
    }
    dedup_completion_items(items)
}

fn current_module_bare_items(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
) -> Vec<SourceCompletionItem> {
    let Some(module) = current_module(program, file) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    items.extend(module.resources.iter().map(|resource| {
        source_completion_item(&resource.name, SourceCompletionItemKind::Resource)
            .detail("resource")
            .docs_from(&resource.docs)
    }));
    items.extend(module.enums.iter().map(|enum_schema| {
        source_completion_item(&enum_schema.name, SourceCompletionItemKind::Enum)
            .detail("enum")
            .docs_from(&enum_schema.docs)
    }));
    items.extend(
        function_completions(program, file, source_file, module)
            .into_iter()
            .map(|function| {
                source_completion_item(&function.name, SourceCompletionItemKind::Function)
                    .detail(source_function_signature(&function))
                    .docs_from(&function.docs)
            }),
    );
    items
}

fn expected_enum_member_items(expected: &ExpectedEnum<'_>) -> Vec<SourceCompletionItem> {
    match &expected.context {
        ExpectedEnumContext::Value { prefix } => expected_enum_value_items(expected, prefix),
        ExpectedEnumContext::MatchArm => expected_enum_match_arm_items(expected),
    }
}

fn expected_enum_value_items(
    expected: &ExpectedEnum<'_>,
    prefix: &str,
) -> Vec<SourceCompletionItem> {
    expected_enum_items_matching(expected, |schema, ordinal| {
        schema.is_selectable_leaf(ordinal).then(|| {
            let path = schema.member_path(ordinal).join("::");
            (format!("{prefix}::{path}"), schema.name.clone())
        })
    })
}

fn expected_enum_match_arm_items(expected: &ExpectedEnum<'_>) -> Vec<SourceCompletionItem> {
    expected_enum_items_matching(expected, |schema, ordinal| {
        schema
            .subtree_ordinals(ordinal)
            .any(|candidate| schema.is_selectable_leaf(candidate))
            .then(|| {
                let path = schema.member_path(ordinal).join("::");
                (path, format!("{} match arm", schema.name))
            })
    })
}

fn expected_enum_items_matching(
    expected: &ExpectedEnum<'_>,
    mut label_and_detail: impl FnMut(&EnumSchema, usize) -> Option<(String, String)>,
) -> Vec<SourceCompletionItem> {
    expected
        .schema
        .members
        .iter()
        .enumerate()
        .filter_map(|(ordinal, member)| {
            let (label, detail) = label_and_detail(expected.schema, ordinal)?;
            Some(
                source_completion_item(&label, SourceCompletionItemKind::EnumMember)
                    .detail(detail)
                    .docs_from(&member.docs),
            )
        })
        .collect()
}

trait SourceCompletionItemExt {
    fn detail(self, detail: impl Into<String>) -> Self;
    fn docs_from(self, docs: &[String]) -> Self;
}

impl SourceCompletionItemExt for SourceCompletionItem {
    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn docs_from(mut self, docs: &[String]) -> Self {
        self.docs = docs.to_vec();
        self
    }
}

fn source_completion_item(label: &str, kind: SourceCompletionItemKind) -> SourceCompletionItem {
    SourceCompletionItem {
        label: label.to_string(),
        kind,
        detail: None,
        docs: Vec::new(),
    }
}

fn dedup_completion_items(items: Vec<SourceCompletionItem>) -> Vec<SourceCompletionItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.label.clone()))
        .collect()
}

fn qualifier_before(source: &str, tokens: &[Token], colon_index: usize) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut i = colon_index;
    loop {
        let ident = i.checked_sub(1)?;
        if tokens[ident].kind != TokenKind::Identifier {
            return None;
        }
        segments.push(tokens[ident].text(source).to_string());
        match ident.checked_sub(1) {
            Some(prev) if tokens[prev].kind == TokenKind::DoubleColon => i = prev,
            _ => break,
        }
    }
    segments.reverse();
    Some(segments)
}

fn saved_path_before_dot(source: &str, tokens: &[Token], dot_index: usize) -> SavedPathRecovery {
    let Some(end) = dot_index.checked_sub(1) else {
        return unrecovered_saved_path(tokens, dot_index);
    };
    if matches!(tokens[end].kind, TokenKind::DotDot | TokenKind::DotDotEqual) {
        return unrecovered_saved_path(tokens, dot_index);
    }
    let Some(start) = saved_receiver_start(tokens, end) else {
        return unrecovered_saved_path(tokens, dot_index);
    };
    let span = recovered_receiver_span(tokens, start, end);
    let receiver = source[span.start_byte..span.end_byte].to_string();
    SavedPathRecovery::Complete { receiver, span }
}

fn saved_receiver_start(tokens: &[Token], end: usize) -> Option<usize> {
    match tokens[end].kind {
        TokenKind::Identifier => saved_receiver_identifier_start(tokens, end),
        TokenKind::RightParen => {
            let open = matching_open_paren(tokens, end)?;
            if let Some(callee) = open.checked_sub(1)
                && tokens[callee].kind == TokenKind::Identifier
            {
                return saved_receiver_identifier_start(tokens, callee);
            }
            saved_receiver_group_start(tokens, open, end)
        }
        _ => None,
    }
}

fn saved_receiver_group_start(tokens: &[Token], open: usize, close: usize) -> Option<usize> {
    let inner_end = close.checked_sub(1)?;
    if inner_end <= open {
        return None;
    }
    (saved_receiver_start(tokens, inner_end)? == open + 1).then_some(open)
}

fn saved_receiver_identifier_start(tokens: &[Token], ident: usize) -> Option<usize> {
    let before = ident.checked_sub(1)?;
    match tokens[before].kind {
        TokenKind::Caret => Some(before),
        TokenKind::Dot | TokenKind::QuestionDot => {
            saved_receiver_start(tokens, before.checked_sub(1)?)
        }
        _ => None,
    }
}

fn recovered_receiver_span(tokens: &[Token], start: usize, end: usize) -> SourceSpan {
    let start_span = tokens[start].span;
    SourceSpan {
        start_byte: start_span.start_byte,
        end_byte: tokens[end].span.end_byte,
        line: start_span.line,
        column: start_span.column,
    }
}

fn unrecovered_saved_path(tokens: &[Token], dot_index: usize) -> SavedPathRecovery {
    if malformed_saved_member_receiver_before_dot(tokens, dot_index)
        || saved_path_attempt_before_dot(tokens, dot_index)
        || open_saved_postfix_before_dot(tokens, dot_index)
    {
        SavedPathRecovery::Invalid
    } else {
        SavedPathRecovery::NotSavedPath
    }
}

fn malformed_saved_member_receiver_before_dot(tokens: &[Token], dot_index: usize) -> bool {
    dot_index
        .checked_sub(1)
        .is_some_and(|receiver| malformed_saved_member_receiver(tokens, receiver))
}

fn malformed_saved_member_receiver(tokens: &[Token], receiver: usize) -> bool {
    let Some(delimiter) = receiver.checked_sub(1) else {
        return false;
    };
    if !matches!(
        tokens[delimiter].kind,
        TokenKind::Dot | TokenKind::QuestionDot | TokenKind::DotDot | TokenKind::DotDotEqual
    ) {
        return false;
    }
    delimiter
        .checked_sub(1)
        .is_some_and(|end| saved_path_attempt_ending_at(tokens, end))
}

fn saved_path_attempt_before_dot(tokens: &[Token], dot_index: usize) -> bool {
    let Some(mut end) = dot_index.checked_sub(1) else {
        return false;
    };
    if matches!(tokens[end].kind, TokenKind::DotDot | TokenKind::DotDotEqual) {
        let Some(prev) = end.checked_sub(1) else {
            return false;
        };
        end = prev;
    }

    saved_path_attempt_ending_at(tokens, end)
}

fn saved_path_attempt_ending_at(tokens: &[Token], mut i: usize) -> bool {
    loop {
        match tokens[i].kind {
            TokenKind::Caret => return true,
            TokenKind::Identifier => return saved_path_callee_prefix_before(tokens, i),
            TokenKind::RightParen => match matching_open_paren(tokens, i) {
                Some(open) => {
                    if let Some(callee) = open.checked_sub(1)
                        && tokens[callee].kind == TokenKind::Identifier
                    {
                        return saved_path_callee_prefix_before(tokens, callee);
                    }
                    if saved_receiver_group_start(tokens, open, i).is_some() {
                        return true;
                    }
                    let Some(prev) = open.checked_sub(1) else {
                        return false;
                    };
                    i = prev;
                }
                None => {
                    let Some(prev) = i.checked_sub(1) else {
                        return false;
                    };
                    i = prev;
                }
            },
            TokenKind::RightBracket => match matching_open_bracket(tokens, i) {
                Some(open) => {
                    let Some(prev) = open.checked_sub(1) else {
                        return false;
                    };
                    return saved_path_attempt_ending_at(tokens, prev);
                }
                None => {
                    let Some(prev) = i.checked_sub(1) else {
                        return false;
                    };
                    i = prev;
                }
            },
            TokenKind::Dot | TokenKind::QuestionDot => {
                let Some(prev) = i.checked_sub(1) else {
                    return false;
                };
                i = prev;
            }
            _ => return false,
        }
    }
}

fn open_saved_postfix_before_dot(tokens: &[Token], dot_index: usize) -> bool {
    let Some(mut i) = dot_index.checked_sub(1) else {
        return false;
    };
    let dot_line = tokens[dot_index].span.line;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    loop {
        if tokens[i].span.line < dot_line {
            return false;
        }
        match tokens[i].kind {
            TokenKind::RightParen => paren_depth += 1,
            TokenKind::LeftParen if paren_depth > 0 => paren_depth -= 1,
            TokenKind::LeftParen
                if bracket_depth == 0 && open_call_has_completed_saved_receiver(tokens, i) =>
            {
                return true;
            }
            TokenKind::RightBracket => bracket_depth += 1,
            TokenKind::LeftBracket if bracket_depth > 0 => bracket_depth -= 1,
            TokenKind::LeftBracket
                if paren_depth == 0 && open_bracket_has_saved_receiver(tokens, i) =>
            {
                return true;
            }
            _ => {}
        }
        let Some(prev) = i.checked_sub(1) else {
            return false;
        };
        i = prev;
    }
}

fn open_call_has_completed_saved_receiver(tokens: &[Token], open: usize) -> bool {
    let Some(receiver) = open.checked_sub(1) else {
        return false;
    };
    matches!(
        tokens[receiver].kind,
        TokenKind::RightParen | TokenKind::RightBracket
    ) && saved_path_attempt_ending_at(tokens, receiver)
}

fn open_bracket_has_saved_receiver(tokens: &[Token], open: usize) -> bool {
    open.checked_sub(1)
        .is_some_and(|receiver| saved_path_attempt_ending_at(tokens, receiver))
}

fn saved_path_callee_prefix_before(tokens: &[Token], callee: usize) -> bool {
    let Some(before) = callee.checked_sub(1) else {
        return false;
    };
    match tokens[before].kind {
        TokenKind::Caret => true,
        TokenKind::Dot | TokenKind::QuestionDot => before
            .checked_sub(1)
            .is_some_and(|end| saved_path_attempt_ending_at(tokens, end)),
        _ => false,
    }
}

enum SavedPathRecovery {
    Complete { receiver: String, span: SourceSpan },
    Invalid,
    NotSavedPath,
}

fn matching_open_paren(tokens: &[Token], close_index: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = close_index;
    loop {
        match tokens[i].kind {
            TokenKind::RightParen => depth += 1,
            TokenKind::LeftParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i = i.checked_sub(1)?;
    }
}

fn matching_open_bracket(tokens: &[Token], close_index: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = close_index;
    loop {
        match tokens[i].kind {
            TokenKind::RightBracket => depth += 1,
            TokenKind::LeftBracket => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i = i.checked_sub(1)?;
    }
}

fn introduces_type(
    source: &str,
    lexed: &LexedSource,
    parsed: &ParsedSource,
    tokens: &[Token],
    colon_index: usize,
    offset: usize,
) -> bool {
    let Some(before) = colon_index.checked_sub(1) else {
        return false;
    };
    if active_callable_context(source, lexed, parsed, offset).is_some_and(|context| {
        context
            .named_argument
            .as_deref()
            .is_some_and(|name| tokens[before].text(source) == name)
    }) {
        return false;
    }
    matches!(
        tokens[before].kind,
        TokenKind::Identifier | TokenKind::RightParen
    )
}

fn significant_tokens(lexed: &LexedSource) -> Vec<Token> {
    lexed
        .tokens
        .iter()
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Indent
                    | TokenKind::Dedent
                    | TokenKind::Newline
                    | TokenKind::Eof
                    | TokenKind::Comment
                    | TokenKind::DocComment
            )
        })
        .copied()
        .collect()
}

pub fn source_type_completion_fact(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
) -> SourceTypeCompletionFact {
    let mut candidates = BUILTIN_TYPE_COMPLETIONS
        .iter()
        .map(|spelling| SourceTypeCompletionCandidate::Builtin {
            spelling: *spelling,
        })
        .collect::<Vec<_>>();

    let current = current_module(program, file);
    if let Some(module) = current {
        candidates.extend(module.resources.iter().map(|resource| {
            type_resource_completion(vec![resource.name.clone()], module, resource)
        }));
    }
    candidates.extend(imported_resource_completions(
        program,
        source_file,
        current
            .map(|module| module.name.as_str())
            .unwrap_or_default(),
    ));
    candidates.extend(keyed_store_identity_completions(program));
    if let Some(module) = current {
        candidates.extend(module.enums.iter().map(|enum_schema| {
            type_enum_completion(vec![enum_schema.name.clone()], module, enum_schema)
        }));
        candidates.extend(unique_visible_foreign_enum_completions(
            program, file, module,
        ));
        candidates.extend(imported_enum_completions(
            program,
            file,
            source_file,
            module.name.as_str(),
        ));
    }

    SourceTypeCompletionFact { candidates }
}

pub fn source_namespace_completion_fact(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    if qualifier.first().map(String::as_str) == Some("std") {
        if let Some(fact) = standard_library_namespace_completion_fact(qualifier) {
            return Some(fact);
        }
        return namespace_completion_fact_for_expanded_qualifier(
            program,
            file,
            source_file,
            qualifier,
        );
    }
    let expanded = expand_namespace_qualifier(source_file, qualifier)?;
    if let Some(fact) = standard_library_namespace_completion_fact(&expanded) {
        return Some(fact);
    }
    namespace_completion_fact_for_expanded_qualifier(program, file, source_file, &expanded)
}

fn source_namespace_completion_fact_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    offset: usize,
    qualifier: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    let expanded = expand_visible_namespace_qualifier(program, file, parsed, offset, qualifier)?;
    if let Some(fact) = standard_library_namespace_completion_fact(&expanded) {
        return Some(fact);
    }
    namespace_completion_fact_for_expanded_qualifier(program, file, &parsed.file, &expanded)
}

pub fn source_namespace_completion_file_fact(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    let expanded = expand_file_namespace_qualifier(source_file, qualifier)?;
    namespace_completion_fact_for_expanded_qualifier(program, file, source_file, &expanded)
}

pub fn source_saved_root_completion_fact(
    program: &CheckedProgram,
) -> SourceSavedRootCompletionFact {
    SourceSavedRootCompletionFact {
        candidates: program
            .modules
            .iter()
            .flat_map(|module| {
                module
                    .stores
                    .iter()
                    .map(|store| source_saved_root_completion_candidate(module, store))
            })
            .collect(),
    }
}

fn stores(program: &CheckedProgram) -> impl Iterator<Item = &StoreSchema> {
    program
        .modules
        .iter()
        .flat_map(|module| module.stores.iter())
}

fn source_saved_root_completion_candidate(
    module: &CheckedModule,
    store: &StoreSchema,
) -> SourceSavedRootCompletionCandidate {
    SourceSavedRootCompletionCandidate {
        root: store.root.clone(),
        module: module.name.clone(),
        resource_name: store.resource.clone(),
        docs: store.docs.clone(),
    }
}

fn imported_resource_completions(
    program: &CheckedProgram,
    source_file: &SourceFile,
    current_module: &str,
) -> Vec<SourceTypeCompletionCandidate> {
    imported_type_modules(program, source_file, current_module)
        .into_iter()
        .flat_map(|(alias, module)| {
            module.resources.iter().map(move |resource| {
                type_resource_completion(
                    vec![alias.clone(), resource.name.clone()],
                    module,
                    resource,
                )
            })
        })
        .collect()
}

fn imported_enum_completions(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    current_module: &str,
) -> Vec<SourceTypeCompletionCandidate> {
    imported_type_modules(program, source_file, current_module)
        .into_iter()
        .flat_map(|(alias, module)| {
            module
                .enums
                .iter()
                .filter(move |enum_schema| {
                    enum_visible_from_file(module, enum_schema.name.as_str(), file)
                })
                .map(move |enum_schema| {
                    type_enum_completion(
                        vec![alias.clone(), enum_schema.name.clone()],
                        module,
                        enum_schema,
                    )
                })
        })
        .collect()
}

fn imported_type_modules<'a>(
    program: &'a CheckedProgram,
    source_file: &SourceFile,
    current_module: &str,
) -> Vec<(String, &'a CheckedModule)> {
    let alias_counts = import_alias_counts(source_file);
    source_file
        .uses
        .iter()
        .filter_map(|use_decl| {
            let alias = crate::short_name(&use_decl.name);
            (alias_counts.get(alias).copied() == Some(1)
                && !crate::source_declares_top_level_name(source_file, alias))
            .then_some((alias.to_string(), use_decl))
        })
        .filter_map(|(alias, use_decl)| {
            let module = module_for_segments(program, &crate::split_type_path(&use_decl.name))?;
            (module.name != current_module).then_some((alias, module))
        })
        .collect()
}

fn import_alias_counts(source_file: &SourceFile) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for use_decl in &source_file.uses {
        *counts.entry(crate::short_name(&use_decl.name)).or_insert(0) += 1;
    }
    counts
}

fn type_resource_completion(
    path: Vec<String>,
    module: &CheckedModule,
    resource: &ResourceSchema,
) -> SourceTypeCompletionCandidate {
    SourceTypeCompletionCandidate::Resource {
        path,
        module: module.name.clone(),
        name: resource.name.clone(),
        docs: resource.docs.clone(),
    }
}

fn keyed_store_identity_completions(
    program: &CheckedProgram,
) -> impl Iterator<Item = SourceTypeCompletionCandidate> + '_ {
    stores(program)
        .filter(|store| !store.identity_keys.is_empty())
        .map(|store| SourceTypeCompletionCandidate::StoreIdentity {
            root: store.root.clone(),
            docs: store.docs.clone(),
        })
}

fn type_enum_completion(
    path: Vec<String>,
    module: &CheckedModule,
    enum_schema: &EnumSchema,
) -> SourceTypeCompletionCandidate {
    SourceTypeCompletionCandidate::Enum {
        path,
        module: module.name.clone(),
        name: enum_schema.name.clone(),
        docs: enum_schema.docs.clone(),
    }
}

fn unique_visible_foreign_enum_completions(
    program: &CheckedProgram,
    file: &Path,
    current: &CheckedModule,
) -> Vec<SourceTypeCompletionCandidate> {
    let same_module_names = current
        .enums
        .iter()
        .map(|enum_schema| enum_schema.name.as_str())
        .collect::<HashSet<_>>();
    let mut emitted = HashSet::new();
    let mut completions = Vec::new();
    for module in &program.modules {
        if module.source_file == file || module.name.is_empty() {
            continue;
        }
        for enum_schema in &module.enums {
            let name = enum_schema.name.as_str();
            if same_module_names.contains(name) || !emitted.insert(name) {
                continue;
            }
            if let Some((owner, visible_enum)) =
                unique_visible_foreign_enum(program, file, current, name)
            {
                completions.push(type_enum_completion(
                    vec![visible_enum.name.clone()],
                    owner,
                    visible_enum,
                ));
            }
        }
    }
    completions
}

fn unique_visible_foreign_enum<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    current: &CheckedModule,
    name: &str,
) -> Option<(&'a CheckedModule, &'a EnumSchema)> {
    let mut matches = program.modules.iter().filter_map(|module| {
        if module.source_file == file || module.name.is_empty() {
            return None;
        }
        if current
            .enums
            .iter()
            .any(|enum_schema| enum_schema.name == name)
        {
            return None;
        }
        if !enum_visible_from_file(module, name, file) {
            return None;
        }
        module
            .enums
            .iter()
            .find(|enum_schema| enum_schema.name == name)
            .map(|enum_schema| (module, enum_schema))
    });
    let candidate = matches.next()?;
    matches.next().is_none().then_some(candidate)
}

fn namespace_completion_fact_for_expanded_qualifier(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    expanded: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    if let Some(module) = module_for_segments(program, expanded) {
        return Some(SourceNamespaceCompletionFact::Module(
            module_completion_fact(program, file, source_file, module),
        ));
    }
    enum_for_segments(program, file, expanded)
        .map(enum_completion_fact)
        .map(SourceNamespaceCompletionFact::Enum)
}

fn standard_library_namespace_completion_fact(
    qualifier: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    match qualifier {
        [root] if root == "std" => Some(SourceNamespaceCompletionFact::StandardLibraryRoot(
            standard_library_root_completion_fact(),
        )),
        [root, module] if root == "std" && standard_library_module_exists(module) => {
            Some(SourceNamespaceCompletionFact::StandardLibraryModule(
                standard_library_module_completion_fact(module),
            ))
        }
        _ => None,
    }
}

fn standard_library_root_completion_fact() -> SourceStandardLibraryRootNamespaceCompletionFact {
    let mut emitted = HashSet::new();
    SourceStandardLibraryRootNamespaceCompletionFact {
        modules: stdlib::all()
            .iter()
            .filter(|op| emitted.insert(op.module))
            .map(|op| SourceStandardLibraryModuleCompletion {
                name: op.module.to_string(),
            })
            .collect(),
    }
}

fn standard_library_module_completion_fact(
    module: &str,
) -> SourceStandardLibraryModuleNamespaceCompletionFact {
    SourceStandardLibraryModuleNamespaceCompletionFact {
        module: module.to_string(),
        operations: stdlib::all()
            .iter()
            .filter(|op| op.module == module)
            .map(|op| {
                let path = vec!["std".to_string(), module.to_string(), op.op.to_string()];
                let signature = intrinsic_callable_signature(&path)
                    .expect("stdlib operation has a callable signature");
                SourceStandardLibraryOperationCompletion {
                    name: op.op.to_string(),
                    signature,
                }
            })
            .collect(),
    }
}

fn standard_library_module_exists(module: &str) -> bool {
    stdlib::all().iter().any(|op| op.module == module)
}

fn expand_namespace_qualifier(
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<Vec<String>> {
    match qualifier {
        [] => None,
        [segment] => expand_unique_import_module_alias(source_file, segment),
        _ => crate::expand_unique_import_alias(source_file, qualifier).ok(),
    }
}

fn expand_visible_namespace_qualifier(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    offset: usize,
    qualifier: &[String],
) -> Option<Vec<String>> {
    let head = qualifier.first()?;
    let binding = scope_completion_bindings_at(program, file, parsed, offset)
        .into_iter()
        .find(|binding| binding.name == *head);
    if binding
        .as_ref()
        .is_some_and(|binding| matches!(binding.kind, ScopeCompletionBindingKind::Value { .. }))
    {
        return None;
    }
    if head == "std" {
        return Some(qualifier.to_vec());
    }
    if crate::driver::unique_import_module_alias_path(&parsed.file, head)
        .err()
        .is_some()
    {
        return None;
    }
    if let Some(binding) = binding
        && let ScopeCompletionBindingKind::ModuleAlias { module } = binding.kind
    {
        return Some(
            module
                .into_iter()
                .chain(qualifier[1..].iter().cloned())
                .collect(),
        );
    }
    Some(qualifier.to_vec())
}

fn expand_file_namespace_qualifier(
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<Vec<String>> {
    match qualifier {
        [] => None,
        [segment] => expand_unique_file_import_module_alias(source_file, segment),
        _ => expand_unique_file_import_alias(source_file, qualifier),
    }
}

fn expand_unique_import_module_alias(
    source_file: &SourceFile,
    segment: &str,
) -> Option<Vec<String>> {
    match crate::driver::unique_import_module_alias_path(source_file, segment) {
        Ok(Some(module)) => Some(module),
        Ok(None) => Some(vec![segment.to_string()]),
        Err(_) => None,
    }
}

fn expand_unique_file_import_module_alias(
    source_file: &SourceFile,
    segment: &str,
) -> Option<Vec<String>> {
    let mut matches = source_file
        .uses
        .iter()
        .filter(|use_decl| crate::short_name(&use_decl.name) == segment);
    let Some(import) = matches.next() else {
        return Some(vec![segment.to_string()]);
    };
    if matches.next().is_some() || crate::import_alias_head_is_file_shadowed(source_file, segment) {
        return None;
    }
    Some(crate::split_type_path(&import.name))
}

fn expand_unique_file_import_alias(
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<Vec<String>> {
    let head = qualifier.first()?;
    let mut matches = source_file
        .uses
        .iter()
        .filter(|use_decl| crate::short_name(&use_decl.name) == head.as_str());
    let Some(import) = matches.next() else {
        return Some(qualifier.to_vec());
    };
    if matches.next().is_some() || crate::import_alias_head_is_file_shadowed(source_file, head) {
        return None;
    }
    Some(
        crate::split_type_path(&import.name)
            .into_iter()
            .chain(qualifier[1..].iter().cloned())
            .collect(),
    )
}

fn module_for_segments<'a>(
    program: &'a CheckedProgram,
    segments: &[String],
) -> Option<&'a CheckedModule> {
    let module_name = segments.join("::");
    program
        .modules
        .iter()
        .find(|module| module.name == module_name)
}

fn enum_for_segments<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    segments: &[String],
) -> Option<&'a EnumSchema> {
    let (enum_name, module_segments) = segments.split_last()?;
    let module = if module_segments.is_empty() {
        current_module(program, file)?
    } else {
        module_for_segments(program, module_segments)?
    };
    module.enums.iter().find(|enum_schema| {
        enum_schema.name == *enum_name
            && enum_visible_from_file(module, enum_schema.name.as_str(), file)
    })
}

fn current_module<'a>(program: &'a CheckedProgram, file: &Path) -> Option<&'a CheckedModule> {
    program
        .modules
        .iter()
        .find(|module| module.source_file == file)
}

fn enum_visible_from_file(module: &CheckedModule, enum_name: &str, file: &Path) -> bool {
    module.source_file == file || module.enum_public.get(enum_name).copied().unwrap_or(false)
}

fn module_completion_fact(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    module: &CheckedModule,
) -> SourceModuleNamespaceCompletionFact {
    SourceModuleNamespaceCompletionFact {
        module: module.name.clone(),
        resources: module.resources.iter().map(resource_completion).collect(),
        enums: module
            .enums
            .iter()
            .filter(|enum_schema| enum_visible_from_file(module, enum_schema.name.as_str(), file))
            .map(enum_completion)
            .collect(),
        functions: function_completions(program, file, source_file, module),
    }
}

fn resource_completion(resource: &ResourceSchema) -> SourceNamespaceResourceCompletion {
    SourceNamespaceResourceCompletion {
        name: resource.name.clone(),
        docs: resource.docs.clone(),
    }
}

fn enum_completion(enum_schema: &EnumSchema) -> SourceNamespaceEnumCompletion {
    SourceNamespaceEnumCompletion {
        name: enum_schema.name.clone(),
        docs: enum_schema.docs.clone(),
    }
}

fn function_completions(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    module: &CheckedModule,
) -> Vec<SourceNamespaceFunctionCompletion> {
    let parsed_functions = parsed_functions_for_file(program, file, source_file, module);
    module
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| module.source_file == file || function.public)
        .map(|(index, function)| {
            function_completion_fact(
                function,
                parsed_functions
                    .as_ref()
                    .and_then(|functions| functions.get(index)),
            )
        })
        .collect()
}

fn parsed_functions_for_file<'a>(
    program: &CheckedProgram,
    file: &Path,
    source_file: &'a SourceFile,
    module: &CheckedModule,
) -> Option<Vec<&'a FunctionDecl>> {
    if module.source_file != file || current_module(program, file)?.name != module.name {
        return None;
    }
    Some(
        source_file
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Function(function) => Some(function),
                _ => None,
            })
            .collect(),
    )
}

fn function_completion_fact(
    function: &CheckedFunction,
    parsed: Option<&&FunctionDecl>,
) -> SourceNamespaceFunctionCompletion {
    SourceNamespaceFunctionCompletion {
        name: function.name.clone(),
        params: function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| SourceNamespaceFunctionParamCompletion {
                name: param.name.clone(),
                ty: param.ty.clone(),
                docs: parsed
                    .and_then(|function| function.params.get(index))
                    .map(|param| param.docs.clone())
                    .unwrap_or_default(),
            })
            .collect(),
        return_type: function.return_type.clone(),
        docs: parsed
            .map(|function| function.docs.clone())
            .unwrap_or_default(),
    }
}

fn enum_completion_fact(enum_schema: &EnumSchema) -> SourceEnumNamespaceCompletionFact {
    SourceEnumNamespaceCompletionFact {
        enum_name: enum_schema.name.clone(),
        members: enum_schema
            .members
            .iter()
            .enumerate()
            .map(|(ordinal, member)| SourceNamespaceEnumMemberCompletion {
                name: member.name.clone(),
                docs: member.docs.clone(),
                status: enum_member_status(enum_schema, ordinal),
            })
            .collect(),
    }
}

fn enum_member_status(enum_schema: &EnumSchema, ordinal: usize) -> SourceNamespaceEnumMemberStatus {
    if enum_schema.is_selectable_leaf(ordinal) {
        SourceNamespaceEnumMemberStatus::Selectable
    } else if enum_schema.is_category(ordinal) {
        SourceNamespaceEnumMemberStatus::Category
    } else {
        SourceNamespaceEnumMemberStatus::Group
    }
}

#[cfg(test)]
mod tests;
