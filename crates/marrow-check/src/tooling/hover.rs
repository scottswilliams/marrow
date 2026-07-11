use std::collections::HashMap;
use std::path::{Path, PathBuf};

use marrow_catalog::CatalogEntryKind;
use marrow_schema::{EnumSchema, IndexSchema, Node, NodeKind, ResourceSchema, StoreSchema, stdlib};
use marrow_syntax::{
    Declaration, EnumMember, FunctionDecl, IndexDecl, KeyParam, Keyword, LexedSource,
    ResourceMember, SourceFile, SourceSpan, StoreDecl, Token, TokenKind, lex_source,
};

use super::signatures::{
    CallableCalleeContext, CallableSignature, CallableValueShape, callable_callee_contexts,
    intrinsic_callable_signature_for_file,
};
use crate::{
    AnalysisSnapshot, AnalyzedFile, BindingIndex, CheckedConst, CheckedFunction, DirectEffectFacts,
    FunctionFact, MarrowType, ModuleFact, ModuleId, ResourceMemberKind, StoreFact, SymbolKind,
    SymbolOccurrence, SymbolRef, UseSiteKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSymbolDocs {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTypeHoverFact {
    pub ty: MarrowType,
    pub docs: Vec<String>,
}

/// The canonical checker-owned hover fact at a source position. Variant order
/// is semantic precedence: callers must ask through [`source_hover_fact_at`]
/// rather than reconstructing this dispatch downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceHoverFact {
    Callable(SourceCallableHoverFact),
    ModulePath(SourceModulePathHoverFact),
    StoreRoot(StoreRootHoverFact),
    Schema(SourceSchemaHoverFact),
    SavedPlace(SavedPlaceHoverFact),
    Operator(SourceOperatorHoverFact),
    Type(SourceTypeHoverFact),
}

impl SourceHoverFact {
    pub(crate) fn contains_recovery_unknown(&self) -> bool {
        match self {
            Self::Callable(fact) => callable_contains_recovery_unknown(fact),
            Self::Type(fact) => fact.ty.contains_recovery_unknown(),
            Self::ModulePath(_)
            | Self::StoreRoot(_)
            | Self::Schema(_)
            | Self::SavedPlace(_)
            | Self::Operator(_) => false,
        }
    }
}

fn callable_contains_recovery_unknown(fact: &SourceCallableHoverFact) -> bool {
    match fact {
        SourceCallableHoverFact::Intrinsic(signature) => {
            signature
                .params
                .iter()
                .any(|param| callable_shape_contains_recovery_unknown(&param.shape))
                || signature
                    .return_shape
                    .as_ref()
                    .is_some_and(callable_shape_contains_recovery_unknown)
        }
        SourceCallableHoverFact::Function(function) => {
            function
                .params
                .iter()
                .any(|param| param.ty.contains_recovery_unknown())
                || function
                    .return_type
                    .as_ref()
                    .is_some_and(MarrowType::contains_recovery_unknown)
        }
        SourceCallableHoverFact::Parameter(param) => param.ty.contains_recovery_unknown(),
        SourceCallableHoverFact::ModuleConst { ty, .. } => ty
            .as_ref()
            .is_some_and(MarrowType::contains_recovery_unknown),
    }
}

fn callable_shape_contains_recovery_unknown(shape: &CallableValueShape) -> bool {
    matches!(shape, CallableValueShape::Type(ty) if ty.contains_recovery_unknown())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceCallableHoverFact {
    Intrinsic(CallableSignature),
    Function(Box<SourceCallableFunctionFact>),
    Parameter(SourceCallableParamFact),
    ModuleConst {
        name: String,
        ty: Option<MarrowType>,
        docs: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCallableFunctionFact {
    pub name: String,
    pub params: Vec<SourceCallableParamFact>,
    pub return_type: Option<MarrowType>,
    pub docs: Vec<String>,
    pub direct_effects: Option<DirectEffectFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCallableParamFact {
    pub name: String,
    pub ty: MarrowType,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceModulePathHoverFact {
    StandardLibraryNamespace(SourceStandardLibraryNamespaceHoverFact),
    StandardLibraryModule(SourceStandardLibraryModuleHoverFact),
    ProjectModule(SourceProjectModuleHoverFact),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStandardLibraryNamespaceHoverFact {
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStandardLibraryModuleHoverFact {
    pub module: String,
    pub operations: Vec<SourceStandardLibraryOperationHoverFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStandardLibraryOperationHoverFact {
    pub name: String,
    pub required_capability: Option<SourceStandardLibraryCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStandardLibraryCapability {
    Clock,
    Context,
    Environment,
    Log,
    Filesystem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProjectModuleHoverFact {
    pub module: String,
    pub source_file: PathBuf,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceModulePathDefinitionFact {
    pub module: String,
    pub source_file: PathBuf,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSchemaHoverFact {
    Resource(SourceResourceHoverFact),
    Enum(SourceEnumHoverFact),
    EnumMember(SourceEnumMemberHoverFact),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceResourceHoverFact {
    pub name: String,
    pub docs: Vec<String>,
    pub members: Vec<SourceResourceHoverMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceResourceHoverMember {
    pub path: Vec<SourceResourceHoverPathSegment>,
    pub kind: SourceResourceHoverMemberKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceResourceHoverPathSegment {
    pub name: String,
    pub key_params: Vec<SourceSchemaHoverKeyParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceResourceHoverMemberKind {
    Field { required: bool, ty: String },
    Layer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSchemaHoverKeyParam {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEnumHoverFact {
    pub name: String,
    pub docs: Vec<String>,
    pub members: Vec<SourceEnumMemberSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEnumMemberSummary {
    pub path: Vec<String>,
    pub status: SourceEnumMemberStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEnumMemberHoverFact {
    pub enum_name: String,
    pub path: Vec<String>,
    pub docs: Vec<String>,
    pub status: SourceEnumMemberStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceEnumMemberStatus {
    Selectable,
    Category,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOperatorHoverFact {
    pub spelling: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedPlaceHoverFact {
    Field {
        name: String,
        key_params: Vec<SavedPlaceHoverKeyParam>,
        ty: String,
        required: bool,
        docs: Vec<String>,
    },
    Layer {
        name: String,
        key_params: Vec<SavedPlaceHoverKeyParam>,
        docs: Vec<String>,
    },
    Index {
        name: String,
        args: Vec<String>,
        unique: bool,
        docs: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPlaceHoverKeyParam {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRootHoverFact {
    pub root: String,
    pub identity_keys: Vec<SavedPlaceHoverKeyParam>,
    pub resource: String,
    pub store_docs: Vec<String>,
    pub members: Vec<StoreRootHoverMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRootHoverPathSegment {
    pub name: String,
    pub key_params: Vec<SavedPlaceHoverKeyParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreRootHoverMember {
    Field {
        path: Vec<StoreRootHoverPathSegment>,
        required: bool,
        ty: String,
    },
    Layer {
        path: Vec<StoreRootHoverPathSegment>,
    },
    Index {
        name: String,
        args: Vec<String>,
        unique: bool,
    },
}

/// A file's single lexical input plus derived callable contexts. The internal
/// type audit constructs one per analyzed file so all hover owners share the
/// same tokenization and do not repeatedly scan call syntax.
pub(crate) struct PrelexedSourceHover<'a> {
    lexed: &'a LexedSource,
    callable_contexts: Vec<CallableCalleeContext>,
    callable_context_by_span: HashMap<(usize, usize), usize>,
    store_declarations_by_root_span: Vec<(SourceSpan, usize)>,
}

impl<'a> PrelexedSourceHover<'a> {
    pub(crate) fn new(analyzed: &AnalyzedFile, lexed: &'a LexedSource) -> Self {
        let callable_contexts = callable_callee_contexts(&analyzed.source, lexed, &analyzed.parsed);
        let mut callable_context_by_span = HashMap::new();
        for (index, context) in callable_contexts.iter().enumerate() {
            callable_context_by_span
                .entry((
                    context.callee_leaf_span.start_byte,
                    context.callee_leaf_span.end_byte,
                ))
                .or_insert(index);
        }
        let mut store_declarations_by_root_span = analyzed
            .parsed
            .file
            .declarations
            .iter()
            .enumerate()
            .filter_map(|(index, declaration)| match declaration {
                Declaration::Store(store) => Some((store.root.span, index)),
                _ => None,
            })
            .collect::<Vec<_>>();
        store_declarations_by_root_span
            .sort_by_key(|(span, _)| (span.start_byte, span.end_byte, span.line, span.column));
        Self {
            lexed,
            callable_contexts,
            callable_context_by_span,
            store_declarations_by_root_span,
        }
    }
}

/// Return the highest-precedence transport-free hover fact at `offset`.
pub fn source_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceHoverFact> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    source_hover_fact_at_prelexed(snapshot, index, analyzed, &prelexed, offset)
}

pub(crate) fn source_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceHoverFact> {
    source_non_type_hover_fact_at_prelexed(snapshot, index, analyzed, prelexed, offset).or_else(
        || {
            source_type_hover_fact_at_prelexed(snapshot, index, analyzed, prelexed, offset)
                .map(SourceHoverFact::Type)
        },
    )
}

/// Return the canonical higher-precedence hover fact before generic expression
/// type fallback. The compiler-development recovery trace already owns the
/// inferred type, so it uses this seam to avoid walking the AST a second time.
pub(crate) fn source_non_type_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceHoverFact> {
    source_callable_hover_fact_at_prelexed(snapshot, index, analyzed, prelexed, offset)
        .map(SourceHoverFact::Callable)
        .or_else(|| {
            source_module_path_hover_fact_at_prelexed(snapshot, index, analyzed, prelexed, offset)
                .map(SourceHoverFact::ModulePath)
        })
        .or_else(|| {
            store_root_hover_fact_at_prelexed(snapshot, analyzed, prelexed, offset)
                .map(SourceHoverFact::StoreRoot)
        })
        .or_else(|| {
            source_schema_hover_fact_at_prelexed(snapshot, index, analyzed, prelexed, offset)
                .map(SourceHoverFact::Schema)
        })
        .or_else(|| {
            saved_place_hover_fact_at_prelexed(snapshot, index, analyzed, prelexed, offset)
                .map(SourceHoverFact::SavedPlace)
        })
        .or_else(|| {
            source_operator_hover_fact_at_prelexed(snapshot, analyzed, prelexed, offset)
                .map(SourceHoverFact::Operator)
        })
}

pub fn source_symbol_docs_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceSymbolDocs> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    source_symbol_docs_at_prelexed(snapshot, index, analyzed, &prelexed, offset)
}

fn source_symbol_docs_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceSymbolDocs> {
    let file = analyzed.path.as_path();
    let symbol = index.definition(file, offset)?;
    if symbol.kind == SymbolKind::Function
        && !matches!(
            source_callable_hover_fact_at_prelexed(snapshot, index, analyzed, prelexed, offset,),
            Some(SourceCallableHoverFact::Function(_))
        )
    {
        return None;
    }
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == symbol.file)?;
    let lines = declaration_docs(&analyzed.parsed.file, &symbol)?;
    (!lines.is_empty()).then(|| SourceSymbolDocs {
        lines: lines.to_vec(),
    })
}

pub fn source_type_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceTypeHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    source_type_hover_fact_at_prelexed(snapshot, index, analyzed, &prelexed, offset)
}

fn source_type_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceTypeHoverFact> {
    let file = analyzed.path.as_path();
    let ty = crate::type_at(&snapshot.program, file, &analyzed.parsed, offset)?;
    if !source_type_hover_eligible(&ty) {
        return None;
    }
    let docs = source_symbol_docs_at_prelexed(snapshot, index, analyzed, prelexed, offset)
        .map(|docs| docs.lines)
        .unwrap_or_default();
    Some(SourceTypeHoverFact { ty, docs })
}

fn source_type_hover_eligible(ty: &MarrowType) -> bool {
    match ty {
        MarrowType::Primitive(_)
        | MarrowType::Error
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. }
        | MarrowType::Identity(_)
        | MarrowType::Enum(_)
        | MarrowType::Absent
        | MarrowType::Dynamic => true,
        MarrowType::Sequence(element) | MarrowType::Optional(element) => {
            source_type_hover_eligible(element)
        }
        MarrowType::LocalTree { keys, value } => {
            keys.iter().all(source_type_hover_eligible) && source_type_hover_eligible(value)
        }
        MarrowType::Invalid | MarrowType::NoValue | MarrowType::Unknown => false,
    }
}

pub fn source_callable_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceCallableHoverFact> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    source_callable_hover_fact_at_prelexed(snapshot, index, analyzed, &prelexed, offset)
}

fn source_callable_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceCallableHoverFact> {
    let file = analyzed.path.as_path();
    if let Some(fact) =
        intrinsic_source_callable_hover_fact_at_prelexed(snapshot, analyzed, prelexed, offset)
    {
        return Some(fact);
    }
    let occurrence = index.occurrence(file, offset)?;
    match occurrence.definition.kind {
        SymbolKind::Function => function_source_callable_hover_fact(
            snapshot,
            index,
            analyzed,
            prelexed,
            offset,
            &occurrence.definition,
        ),
        SymbolKind::Param => parameter_source_callable_hover_fact(
            snapshot,
            index,
            analyzed,
            prelexed,
            offset,
            &occurrence,
        ),
        SymbolKind::ModuleConst => module_const_source_callable_hover_fact(
            snapshot,
            analyzed,
            prelexed,
            offset,
            &occurrence.definition,
        ),
        _ => None,
    }
}

pub fn source_operator_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceOperatorHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    source_operator_hover_fact_at_prelexed(snapshot, analyzed, &prelexed, offset)
}

fn source_operator_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceOperatorHoverFact> {
    let tokens = &prelexed.lexed.tokens;
    let index = token_index_at(tokens, offset)?;
    let token = &tokens[index];
    let spelling = operator_spelling(token.kind, token.text(&analyzed.source))?;
    if is_operator_keyword_non_expression(tokens, index) {
        return None;
    }
    crate::type_at(&snapshot.program, &analyzed.path, &analyzed.parsed, offset)?;
    operator_fact(spelling)
}

fn intrinsic_source_callable_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceCallableHoverFact> {
    let token = &prelexed.lexed.tokens[token_index_at(&prelexed.lexed.tokens, offset)?];
    let index = *prelexed
        .callable_context_by_span
        .get(&(token.span.start_byte, token.span.end_byte))?;
    prelexed
        .callable_contexts
        .get(index)
        .filter(|context| {
            offset_in_name(
                context.callee_leaf_span.start_byte,
                context.callee_leaf_span.end_byte,
                offset,
            )
        })
        .and_then(|context| {
            intrinsic_callable_signature_for_file(
                snapshot,
                &analyzed.path,
                &context.callee_path_segments,
            )
        })
        .map(SourceCallableHoverFact::Intrinsic)
}

fn is_operator_keyword_non_expression(tokens: &[marrow_syntax::Token], index: usize) -> bool {
    matches!(
        tokens[index].kind,
        TokenKind::Keyword(Keyword::Not | Keyword::And | Keyword::Or | Keyword::Is)
    ) && {
        let (previous, next) = significant_neighbors(tokens, index);
        previous.is_some_and(|kind| kind == TokenKind::DoubleColon)
            || next.is_some_and(|kind| kind == TokenKind::DoubleColon)
            || previous.is_some_and(|kind| {
                matches!(kind, TokenKind::Keyword(Keyword::Module | Keyword::Use))
            })
    }
}

fn operator_spelling(kind: TokenKind, text: &str) -> Option<&str> {
    match kind {
        TokenKind::Keyword(Keyword::Not | Keyword::And | Keyword::Or | Keyword::Is)
        | TokenKind::DotDot
        | TokenKind::DotDotEqual
        | TokenKind::EqualEqual
        | TokenKind::BangEqual
        | TokenKind::QuestionDot
        | TokenKind::QuestionQuestion
        | TokenKind::Less
        | TokenKind::LessEqual
        | TokenKind::Greater
        | TokenKind::GreaterEqual
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::PlusEqual
        | TokenKind::MinusEqual
        | TokenKind::StarEqual
        | TokenKind::SlashEqual
        | TokenKind::PercentEqual => Some(text),
        _ => None,
    }
}

fn operator_fact(spelling: &str) -> Option<SourceOperatorHoverFact> {
    let description = match spelling {
        "not" => "logical negation.",
        "and" => "logical conjunction.",
        "or" => "logical disjunction.",
        "is" => "type test.",
        "==" => "equality comparison.",
        "!=" => "inequality comparison.",
        "<" => "less-than comparison.",
        "<=" => "less-than-or-equal comparison.",
        ">" => "greater-than comparison.",
        ">=" => "greater-than-or-equal comparison.",
        "+" => "addition.",
        "-" => "subtraction or numeric negation.",
        "*" => "multiplication.",
        "/" => "division.",
        "%" => "remainder.",
        "+=" => "add and assign.",
        "-=" => "subtract and assign.",
        "*=" => "multiply and assign.",
        "/=" => "divide and assign.",
        "%=" => "remainder and assign.",
        "??" => "fallback value selection.",
        "?." => "optional member access.",
        ".." => "exclusive range.",
        "..=" => "inclusive range.",
        _ => return None,
    };
    Some(SourceOperatorHoverFact {
        spelling: spelling.to_string(),
        description: description.to_string(),
    })
}

pub fn source_module_path_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceModulePathHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    source_module_path_hover_fact_at_prelexed(snapshot, index, analyzed, &prelexed, offset)
}

fn source_module_path_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceModulePathHoverFact> {
    let file = analyzed.path.as_path();
    if index
        .occurrence(file, offset)
        .is_some_and(|occurrence| occurrence.definition.kind != SymbolKind::ModuleRef)
    {
        return None;
    }
    let path = module_path_at(&analyzed.source, &prelexed.lexed.tokens, offset)?;
    if let Some(fact) = standard_library_module_path_hover_fact(snapshot, file, &path) {
        return Some(fact);
    }
    project_module_path_hover_fact(snapshot, index, file, offset, &path)
}

pub fn source_module_path_definition_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceModulePathDefinitionFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let path = module_path_at(&analyzed.source, &lexed.tokens, offset)?;
    if standard_library_module_path_hover_fact(snapshot, file, &path).is_some() {
        return None;
    }
    project_module_path_definition_fact(snapshot, index, file, offset, &path)
}

pub fn source_schema_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceSchemaHoverFact> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    source_schema_hover_fact_at_prelexed(snapshot, index, analyzed, &prelexed, offset)
}

fn source_schema_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SourceSchemaHoverFact> {
    let file = analyzed.path.as_path();
    let occurrence = index.occurrence(file, offset)?;
    let symbol = &occurrence.definition;
    match symbol.kind {
        SymbolKind::Resource => {
            if !is_source_schema_hover_target(snapshot, analyzed, prelexed, offset, &occurrence) {
                return None;
            }
            resource_source_schema_hover_fact(snapshot, symbol).map(SourceSchemaHoverFact::Resource)
        }
        SymbolKind::Enum => {
            if let Some(fact) = enum_annotation_source_schema_hover_fact(snapshot, file, offset) {
                return Some(SourceSchemaHoverFact::Enum(fact));
            }
            if blocked_enum_type_symbol_hover(snapshot, analyzed, prelexed, offset, symbol)
                || !is_source_schema_hover_target(snapshot, analyzed, prelexed, offset, &occurrence)
            {
                return None;
            }
            enum_source_schema_hover_fact(snapshot, symbol).map(SourceSchemaHoverFact::Enum)
        }
        SymbolKind::EnumMember => {
            if !is_source_schema_hover_target(snapshot, analyzed, prelexed, offset, &occurrence) {
                return None;
            }
            enum_member_source_schema_hover_fact(snapshot, symbol)
                .map(SourceSchemaHoverFact::EnumMember)
        }
        _ => None,
    }
}

fn function_source_callable_hover_fact(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    symbol: &SymbolRef,
) -> Option<SourceCallableHoverFact> {
    if !is_function_hover_target(snapshot, index, analyzed, prelexed, offset, symbol) {
        return None;
    }
    let parsed_file = snapshot.files.iter().find(|f| f.path == symbol.file)?;
    let parsed = parsed_function_at(&parsed_file.parsed.file, symbol.span)?;
    let checked = checked_function_for_parsed(snapshot, symbol, &parsed_file.parsed.file, parsed)?;
    Some(SourceCallableHoverFact::Function(Box::new(
        function_hover_fact(snapshot, symbol, parsed, checked),
    )))
}

fn parameter_source_callable_hover_fact(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    occurrence: &SymbolOccurrence,
) -> Option<SourceCallableHoverFact> {
    let symbol = &occurrence.definition;
    if occurrence.reference == *symbol {
        return None;
    }
    let parameter = index.parameter_definition(symbol)?;
    let function_fact = snapshot.program.facts.function(parameter.function);
    let parsed_file = snapshot.files.iter().find(|f| f.path == symbol.file)?;
    let parsed_function = parsed_function_at(&parsed_file.parsed.file, function_fact.span)?;
    let name = parameter_use_name(
        &analyzed.source,
        &prelexed.lexed.tokens,
        offset,
        parsed_function,
    )?;
    let checked_function = checked_function_for_fact(snapshot, function_fact)?;
    let checked = checked_function.params.get(parameter.index)?;
    if checked.name != name {
        return None;
    }
    let parsed_param = parsed_function.params.get(parameter.index)?;
    Some(SourceCallableHoverFact::Parameter(
        SourceCallableParamFact {
            name: checked.name.clone(),
            ty: checked.ty.clone(),
            docs: parsed_param.docs.clone(),
        },
    ))
}

fn module_const_source_callable_hover_fact(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    symbol: &SymbolRef,
) -> Option<SourceCallableHoverFact> {
    if symbol.file != analyzed.path {
        return None;
    }
    let parsed_file = snapshot.files.iter().find(|f| f.path == symbol.file)?;
    let parsed_const = parsed_const_at(&parsed_file.parsed.file, symbol.span)?;
    if !offset_is_on_declaration_identifier(
        &parsed_file.source,
        &prelexed.lexed.tokens,
        parsed_const.span,
        &parsed_const.name,
        offset,
    ) {
        return None;
    }
    let checked = checked_const_at(snapshot, symbol)?;
    Some(SourceCallableHoverFact::ModuleConst {
        name: checked.name.clone(),
        ty: checked.ty.clone(),
        docs: parsed_const.docs.clone(),
    })
}

struct ModulePathAt {
    segments: Vec<String>,
    cursor_segment: usize,
    context: ModulePathContext,
    call_leaf: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModulePathContext {
    ModuleDeclaration,
    UseDeclaration,
    Expression,
}

impl ModulePathContext {
    fn is_declaration(self) -> bool {
        matches!(
            self,
            ModulePathContext::ModuleDeclaration | ModulePathContext::UseDeclaration
        )
    }
}

fn module_path_at(source: &str, tokens: &[Token], offset: usize) -> Option<ModulePathAt> {
    let index = token_index_at(tokens, offset)?;
    if !is_module_path_segment(tokens[index].kind) {
        return None;
    }

    let mut start = index;
    while start >= 2
        && tokens[start - 1].kind == TokenKind::DoubleColon
        && is_module_path_segment(tokens[start - 2].kind)
    {
        start -= 2;
    }

    let mut end = index;
    while end + 2 < tokens.len()
        && tokens[end + 1].kind == TokenKind::DoubleColon
        && is_module_path_segment(tokens[end + 2].kind)
    {
        end += 2;
    }

    let mut segments = Vec::new();
    let mut cursor_segment = None;
    let mut token_index = start;
    loop {
        if !is_module_path_segment(tokens[token_index].kind) {
            return None;
        }
        if token_index == index {
            cursor_segment = Some(segments.len());
        }
        segments.push(tokens[token_index].text(source).to_string());
        if token_index == end {
            break;
        }
        token_index += 2;
    }

    let previous = previous_significant_kind(tokens, start);
    Some(ModulePathAt {
        segments,
        cursor_segment: cursor_segment?,
        context: module_path_context(previous),
        call_leaf: next_significant_kind(tokens, end)
            .is_some_and(|kind| kind == TokenKind::LeftParen)
            && !previous.is_some_and(|kind| {
                matches!(
                    kind,
                    TokenKind::DoubleColon
                        | TokenKind::Dot
                        | TokenKind::QuestionDot
                        | TokenKind::Keyword(Keyword::Fn)
                )
            }),
    })
}

fn module_path_context(previous: Option<TokenKind>) -> ModulePathContext {
    match previous {
        Some(TokenKind::Keyword(Keyword::Module)) => ModulePathContext::ModuleDeclaration,
        Some(TokenKind::Keyword(Keyword::Use)) => ModulePathContext::UseDeclaration,
        _ => ModulePathContext::Expression,
    }
}

fn is_module_path_segment(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Identifier | TokenKind::Keyword(_))
}

fn standard_library_module_path_hover_fact(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    path: &ModulePathAt,
) -> Option<SourceModulePathHoverFact> {
    let expanded = crate::expand_unique_import_alias(
        &snapshot
            .files
            .iter()
            .find(|analyzed| analyzed.path == file)?
            .parsed
            .file,
        &path.segments,
    )
    .ok()?;

    match expanded.as_slice() {
        [std, module]
            if path.context == ModulePathContext::UseDeclaration
                && std == "std"
                && standard_library_module(module) =>
        {
            standard_library_prefix_hover_fact(path, module)
        }
        [std, module, op]
            if path.call_leaf && std == "std" && stdlib::lookup(module, op).is_some() =>
        {
            standard_library_prefix_hover_fact(path, module)
        }
        _ => None,
    }
}

fn standard_library_prefix_hover_fact(
    path: &ModulePathAt,
    module: &str,
) -> Option<SourceModulePathHoverFact> {
    if path
        .segments
        .first()
        .is_some_and(|segment| segment == "std")
        && path.cursor_segment == 0
    {
        return Some(SourceModulePathHoverFact::StandardLibraryNamespace(
            SourceStandardLibraryNamespaceHoverFact {
                modules: standard_library_modules(),
            },
        ));
    }
    if path.cursor_segment + 1 < path.segments.len() || path.context.is_declaration() {
        return Some(SourceModulePathHoverFact::StandardLibraryModule(
            standard_library_module_fact(module)?,
        ));
    }
    None
}

fn standard_library_modules() -> Vec<String> {
    let mut modules = stdlib::all()
        .iter()
        .map(|op| op.module.to_string())
        .collect::<Vec<_>>();
    modules.sort();
    modules.dedup();
    modules
}

fn standard_library_module(module: &str) -> bool {
    stdlib::all().iter().any(|op| op.module == module)
}

fn standard_library_module_fact(module: &str) -> Option<SourceStandardLibraryModuleHoverFact> {
    let mut operations = stdlib::all()
        .iter()
        .filter(|op| op.module == module)
        .map(|op| SourceStandardLibraryOperationHoverFact {
            name: op.op.to_string(),
            required_capability: op.requires_capability.map(standard_library_capability),
        })
        .collect::<Vec<_>>();
    if operations.is_empty() {
        return None;
    }
    operations.sort_by(|left, right| left.name.cmp(&right.name));
    Some(SourceStandardLibraryModuleHoverFact {
        module: module.to_string(),
        operations,
    })
}

fn standard_library_capability(capability: stdlib::Capability) -> SourceStandardLibraryCapability {
    match capability {
        stdlib::Capability::Clock => SourceStandardLibraryCapability::Clock,
        stdlib::Capability::Context => SourceStandardLibraryCapability::Context,
        stdlib::Capability::Environment => SourceStandardLibraryCapability::Environment,
        stdlib::Capability::Log => SourceStandardLibraryCapability::Log,
        stdlib::Capability::Filesystem => SourceStandardLibraryCapability::Filesystem,
    }
}

fn project_module_path_hover_fact(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
    path: &ModulePathAt,
) -> Option<SourceModulePathHoverFact> {
    if index.definition(file, offset).is_some_and(|symbol| {
        matches!(
            symbol.kind,
            SymbolKind::Resource | SymbolKind::Enum | SymbolKind::EnumMember
        )
    }) {
        return None;
    }

    if !path.context.is_declaration() && path.cursor_segment + 1 >= path.segments.len() {
        return None;
    }

    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let expanded = crate::expand_unique_import_alias(&analyzed.parsed.file, &path.segments).ok()?;
    let module = best_project_module_prefix(snapshot, &expanded, path.context)?;

    Some(SourceModulePathHoverFact::ProjectModule(
        SourceProjectModuleHoverFact {
            module: module.name.clone(),
            source_file: module.source_file.clone(),
            span: module.span,
        },
    ))
}

fn project_module_path_definition_fact(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
    path: &ModulePathAt,
) -> Option<SourceModulePathDefinitionFact> {
    if index.definition(file, offset).is_some() {
        return None;
    }

    if !path.context.is_declaration() && path.cursor_segment + 1 >= path.segments.len() {
        return None;
    }

    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let expanded = crate::expand_unique_import_alias(&analyzed.parsed.file, &path.segments).ok()?;
    let cursor_segment_count = expanded_cursor_segment_count(path, &expanded)?;
    let module =
        project_module_prefix_at_cursor(snapshot, &expanded, path.context, cursor_segment_count)?;

    Some(SourceModulePathDefinitionFact {
        module: module.name.clone(),
        source_file: module.source_file.clone(),
        span: module.span,
    })
}

fn expanded_cursor_segment_count(path: &ModulePathAt, expanded: &[String]) -> Option<usize> {
    if expanded.len() < path.segments.len() {
        return None;
    }
    let leading_segment_count = expanded.len() - path.segments.len() + 1;
    if path.cursor_segment == 0 {
        Some(leading_segment_count)
    } else {
        Some(leading_segment_count + path.cursor_segment)
    }
}

fn best_project_module_prefix<'a>(
    snapshot: &'a AnalysisSnapshot,
    expanded: &[String],
    context: ModulePathContext,
) -> Option<&'a ModuleFact> {
    project_module_prefix(snapshot, expanded, context, None)
}

fn project_module_prefix_at_cursor<'a>(
    snapshot: &'a AnalysisSnapshot,
    expanded: &[String],
    context: ModulePathContext,
    cursor_segment_count: usize,
) -> Option<&'a ModuleFact> {
    project_module_prefix(snapshot, expanded, context, Some(cursor_segment_count))
}

fn project_module_prefix<'a>(
    snapshot: &'a AnalysisSnapshot,
    expanded: &[String],
    context: ModulePathContext,
    segment_count: Option<usize>,
) -> Option<&'a ModuleFact> {
    let max_len = if context.is_declaration() {
        expanded.len()
    } else {
        expanded.len().saturating_sub(1)
    };
    snapshot
        .program
        .facts
        .modules()
        .iter()
        .filter(|module| {
            let count = module.name.split("::").count();
            count <= max_len
                && segment_count.is_none_or(|segment_count| count == segment_count)
                && module
                    .name
                    .split("::")
                    .eq(expanded.iter().take(count).map(String::as_str))
        })
        .max_by_key(|module| module.name.split("::").count())
}

fn enum_annotation_source_schema_hover_fact(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceEnumHoverFact> {
    let site = snapshot
        .use_sites()
        .iter()
        .filter(|site| site.kind == UseSiteKind::Enum)
        .filter(|site| site.file == file && span_covers_half_open(site.span, offset))
        .min_by_key(|site| site.span.end_byte.saturating_sub(site.span.start_byte))?;
    let declaration = snapshot.catalog_declaration(&site.catalog_id)?;
    if declaration.kind != CatalogEntryKind::Enum {
        return None;
    }
    let schema = enum_schema_in_file(snapshot, &declaration.file, &declaration.name)?;
    Some(enum_hover_fact(schema))
}

fn is_source_schema_hover_target(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    occurrence: &SymbolOccurrence,
) -> bool {
    source_schema_declaration_name(snapshot, analyzed, prelexed, offset, &occurrence.definition)
        || source_schema_reference_leaf(analyzed, prelexed, offset, occurrence)
}

fn source_schema_reference_leaf(
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    occurrence: &SymbolOccurrence,
) -> bool {
    let file = analyzed.path.as_path();
    let symbol = &occurrence.definition;
    let reference = &occurrence.reference;
    reference.kind == symbol.kind
        && reference.file == file
        && (reference.file != symbol.file || reference.span != symbol.span)
        && span_covers(reference.span, offset)
        && offset_is_on_last_identifier_half_open(&prelexed.lexed.tokens, reference.span, offset)
}

fn source_schema_declaration_name(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    symbol: &SymbolRef,
) -> bool {
    let file = analyzed.path.as_path();
    if symbol.file != file {
        return false;
    }
    let Some(analyzed) = snapshot.files.iter().find(|f| f.path == symbol.file) else {
        return false;
    };
    match symbol.kind {
        SymbolKind::Resource => {
            let Some(resource) = parsed_resource_at(&analyzed.parsed.file, symbol.span) else {
                return false;
            };
            offset_is_on_declaration_identifier(
                &analyzed.source,
                &prelexed.lexed.tokens,
                resource.span,
                &resource.name,
                offset,
            )
        }
        SymbolKind::Enum => {
            let Some(enum_decl) = parsed_enum_at(&analyzed.parsed.file, symbol.span) else {
                return false;
            };
            offset_is_on_declaration_identifier(
                &analyzed.source,
                &prelexed.lexed.tokens,
                enum_decl.span,
                &enum_decl.name,
                offset,
            )
        }
        SymbolKind::EnumMember => {
            let Some(member) = parsed_enum_member_at(&analyzed.parsed.file, symbol.span) else {
                return false;
            };
            offset_is_on_declaration_identifier(
                &analyzed.source,
                &prelexed.lexed.tokens,
                member.span,
                &member.name,
                offset,
            )
        }
        _ => false,
    }
}

fn blocked_enum_type_symbol_hover(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    symbol: &SymbolRef,
) -> bool {
    let file = analyzed.path.as_path();
    if !type_annotation_at(snapshot, file, offset) {
        return false;
    }
    identifier_path_segment_count_at(&prelexed.lexed.tokens, offset).is_some_and(|count| count > 1)
        || symbol.file != file
}

fn type_annotation_at(snapshot: &AnalysisSnapshot, file: &Path, offset: usize) -> bool {
    let Some(analyzed) = snapshot.files.iter().find(|f| f.path == file) else {
        return false;
    };
    let mut found = false;
    for declaration in &analyzed.parsed.file.declarations {
        crate::annotation_refs::walk_declaration_type_refs(
            declaration,
            crate::annotation_refs::TypeAnnotationBodies::Include,
            &mut |ty| {
                if span_covers_half_open(ty.span(), offset) {
                    found = true;
                }
            },
        );
    }
    found
}

fn identifier_path_segment_count_at(tokens: &[Token], offset: usize) -> Option<usize> {
    let index = tokens.iter().position(|token| {
        token.kind == TokenKind::Identifier
            && offset_in_name(token.span.start_byte, token.span.end_byte, offset)
    })?;

    let mut count = 1;
    let mut cursor = index;
    while cursor >= 2
        && tokens[cursor - 1].kind == TokenKind::DoubleColon
        && tokens[cursor - 2].kind == TokenKind::Identifier
    {
        count += 1;
        cursor -= 2;
    }

    cursor = index;
    while cursor + 2 < tokens.len()
        && tokens[cursor + 1].kind == TokenKind::DoubleColon
        && tokens[cursor + 2].kind == TokenKind::Identifier
    {
        count += 1;
        cursor += 2;
    }

    Some(count)
}

fn resource_source_schema_hover_fact(
    snapshot: &AnalysisSnapshot,
    symbol: &SymbolRef,
) -> Option<SourceResourceHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == symbol.file)?;
    let resource = parsed_resource_at(&analyzed.parsed.file, symbol.span)?;
    let schema = resource_schema_in_file(snapshot, &symbol.file, &resource.name)?;
    Some(SourceResourceHoverFact {
        name: schema.name.clone(),
        docs: schema.docs.clone(),
        members: source_resource_members(schema),
    })
}

fn enum_source_schema_hover_fact(
    snapshot: &AnalysisSnapshot,
    symbol: &SymbolRef,
) -> Option<SourceEnumHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == symbol.file)?;
    let enum_decl = parsed_enum_at(&analyzed.parsed.file, symbol.span)?;
    let schema = enum_schema_in_file(snapshot, &symbol.file, &enum_decl.name)?;
    Some(enum_hover_fact(schema))
}

fn enum_member_source_schema_hover_fact(
    snapshot: &AnalysisSnapshot,
    symbol: &SymbolRef,
) -> Option<SourceEnumMemberHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == symbol.file)?;
    let (enum_name, path) = parsed_enum_member_path_at(&analyzed.parsed.file, symbol.span)?;
    let schema = enum_schema_in_file(snapshot, &symbol.file, &enum_name)?;
    let segments = path.iter().map(String::as_str).collect::<Vec<_>>();
    let marrow_schema::MemberPathResolution::Found(ordinal) = schema.walk_member_path(&segments)
    else {
        return None;
    };
    let member = schema.members.get(ordinal)?;
    Some(SourceEnumMemberHoverFact {
        enum_name: schema.name.clone(),
        path,
        docs: member.docs.clone(),
        status: enum_member_status(schema, ordinal),
    })
}

fn enum_hover_fact(schema: &EnumSchema) -> SourceEnumHoverFact {
    SourceEnumHoverFact {
        name: schema.name.clone(),
        docs: schema.docs.clone(),
        members: (0..schema.members.len())
            .map(|ordinal| SourceEnumMemberSummary {
                path: schema
                    .member_path(ordinal)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                status: enum_member_status(schema, ordinal),
            })
            .collect(),
    }
}

fn enum_member_status(schema: &EnumSchema, ordinal: usize) -> SourceEnumMemberStatus {
    if schema.is_selectable_leaf(ordinal) {
        SourceEnumMemberStatus::Selectable
    } else if schema.is_category(ordinal) {
        SourceEnumMemberStatus::Category
    } else {
        SourceEnumMemberStatus::Group
    }
}

fn source_resource_members(schema: &ResourceSchema) -> Vec<SourceResourceHoverMember> {
    let mut members = Vec::new();
    for member in &schema.members {
        source_resource_member(member, &mut Vec::new(), &mut members);
    }
    members
}

fn source_resource_member(
    member: &Node,
    path: &mut Vec<SourceResourceHoverPathSegment>,
    members: &mut Vec<SourceResourceHoverMember>,
) {
    path.push(SourceResourceHoverPathSegment {
        name: member.name.clone(),
        key_params: member
            .key_params
            .iter()
            .map(schema_hover_key_param)
            .collect(),
    });
    match &member.kind {
        NodeKind::Slot { ty, required, .. } => {
            members.push(SourceResourceHoverMember {
                path: path.clone(),
                kind: SourceResourceHoverMemberKind::Field {
                    required: *required,
                    ty: render_schema_leaf_type(member, ty),
                },
            });
        }
        NodeKind::Group => {
            members.push(SourceResourceHoverMember {
                path: path.clone(),
                kind: SourceResourceHoverMemberKind::Layer,
            });
            for child in &member.members {
                source_resource_member(child, path, members);
            }
        }
    }
    path.pop();
}

fn schema_hover_key_param(key: &marrow_schema::KeyDef) -> SourceSchemaHoverKeyParam {
    SourceSchemaHoverKeyParam {
        name: key.name.clone(),
        ty: key.ty.to_string(),
    }
}

pub fn store_root_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<StoreRootHoverFact> {
    let analyzed = snapshot.files.iter().find(|f| f.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    store_root_hover_fact_at_prelexed(snapshot, analyzed, &prelexed, offset)
}

fn store_root_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<StoreRootHoverFact> {
    let file = analyzed.path.as_path();
    let declaration = store_root_declaration_at(&analyzed.parsed.file, prelexed, offset)?;
    let store = snapshot.program.facts.stores().iter().find(|store| {
        store.root == declaration.root.root
            && store.name_span == declaration.root.span
            && fact_file(snapshot, store.module) == Some(file)
    })?;
    store_root_hover_fact(snapshot, store)
}

pub fn saved_place_hover_fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SavedPlaceHoverFact> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    let lexed = lex_source(&analyzed.source);
    let prelexed = PrelexedSourceHover::new(analyzed, &lexed);
    saved_place_hover_fact_at_prelexed(snapshot, index, analyzed, &prelexed, offset)
}

fn saved_place_hover_fact_at_prelexed(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<SavedPlaceHoverFact> {
    let file = analyzed.path.as_path();
    let occurrence = index.occurrence(file, offset)?;
    let symbol = &occurrence.definition;
    if !matches!(
        symbol.kind,
        SymbolKind::Field | SymbolKind::Layer | SymbolKind::Index
    ) || !is_saved_place_hover_target(analyzed, prelexed, offset, &occurrence)
    {
        return None;
    }

    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == symbol.file)?;
    match symbol.kind {
        SymbolKind::Field | SymbolKind::Layer => member_hover_fact(snapshot, analyzed, symbol),
        SymbolKind::Index => index_hover_fact(snapshot, analyzed, symbol),
        _ => None,
    }
}

fn store_root_declaration_at<'a>(
    source: &'a SourceFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
) -> Option<&'a StoreDecl> {
    let candidate = prelexed
        .store_declarations_by_root_span
        .partition_point(|(span, _)| span.start_byte <= offset)
        .checked_sub(1)?;
    let (span, declaration_index) = prelexed.store_declarations_by_root_span.get(candidate)?;
    if !(span.start_byte <= offset && offset < span.end_byte) {
        return None;
    }
    match source.declarations.get(*declaration_index)? {
        Declaration::Store(store) => Some(store),
        _ => None,
    }
}

fn store_root_hover_fact(
    snapshot: &AnalysisSnapshot,
    store: &StoreFact,
) -> Option<StoreRootHoverFact> {
    let (schema, resource) = store_root_schemas(snapshot, store)?;
    Some(StoreRootHoverFact {
        root: schema.root.clone(),
        identity_keys: schema.identity_keys.iter().map(schema_key_param).collect(),
        resource: schema.resource.clone(),
        store_docs: schema.docs.clone(),
        members: store_root_members(resource, &schema.indexes),
    })
}

fn store_root_schemas<'a>(
    snapshot: &'a AnalysisSnapshot,
    store: &StoreFact,
) -> Option<(&'a StoreSchema, &'a ResourceSchema)> {
    let module = snapshot.program.modules.get(store.module.0 as usize)?;
    let schema = module
        .stores
        .iter()
        .find(|schema| schema.root == store.root)?;
    let resource = module
        .resources
        .iter()
        .find(|resource| resource.name == schema.resource)?;
    Some((schema, resource))
}

fn resource_schema_in_file<'a>(
    snapshot: &'a AnalysisSnapshot,
    file: &Path,
    name: &str,
) -> Option<&'a ResourceSchema> {
    snapshot
        .program
        .modules
        .iter()
        .find(|module| module.source_file == file)?
        .resources
        .iter()
        .find(|resource| resource.name == name)
}

fn enum_schema_in_file<'a>(
    snapshot: &'a AnalysisSnapshot,
    file: &Path,
    name: &str,
) -> Option<&'a EnumSchema> {
    snapshot
        .program
        .modules
        .iter()
        .find(|module| module.source_file == file)?
        .enums
        .iter()
        .find(|enum_schema| enum_schema.name == name)
}

fn store_root_members(
    resource: &ResourceSchema,
    indexes: &[IndexSchema],
) -> Vec<StoreRootHoverMember> {
    let mut members = Vec::new();
    for member in &resource.members {
        resource_hover_members(member, &mut Vec::new(), &mut members);
    }
    members.extend(indexes.iter().map(|index| StoreRootHoverMember::Index {
        name: index.name.clone(),
        args: index.args.clone(),
        unique: index.unique,
    }));
    members
}

fn resource_hover_members(
    member: &Node,
    path: &mut Vec<StoreRootHoverPathSegment>,
    members: &mut Vec<StoreRootHoverMember>,
) {
    path.push(StoreRootHoverPathSegment {
        name: member.name.clone(),
        key_params: member.key_params.iter().map(schema_key_param).collect(),
    });
    match &member.kind {
        NodeKind::Slot { ty, required, .. } => {
            members.push(StoreRootHoverMember::Field {
                path: path.clone(),
                required: *required,
                ty: render_schema_leaf_type(member, ty),
            });
        }
        NodeKind::Group => {
            members.push(StoreRootHoverMember::Layer { path: path.clone() });
            for child in &member.members {
                resource_hover_members(child, path, members);
            }
        }
    }
    path.pop();
}

fn schema_key_param(key: &marrow_schema::KeyDef) -> SavedPlaceHoverKeyParam {
    SavedPlaceHoverKeyParam {
        name: key.name.clone(),
        ty: key.ty.to_string(),
    }
}

fn render_schema_leaf_type(member: &Node, ty: &marrow_schema::Type) -> String {
    if member.is_error_code() {
        "ErrorCode".to_string()
    } else {
        ty.to_string()
    }
}

fn is_saved_place_hover_target(
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    occurrence: &SymbolOccurrence,
) -> bool {
    let file = analyzed.path.as_path();
    let symbol = &occurrence.definition;
    is_saved_place_declaration_name(file, offset, symbol)
        || is_saved_place_reference_leaf(analyzed, prelexed, offset, occurrence)
}

fn is_saved_place_reference_leaf(
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    occurrence: &SymbolOccurrence,
) -> bool {
    let file = analyzed.path.as_path();
    let symbol = &occurrence.definition;
    let reference = &occurrence.reference;
    reference.kind == symbol.kind
        && reference.file == file
        && (reference.file != symbol.file || reference.span != symbol.span)
        && span_covers(reference.span, offset)
        && offset_is_on_last_identifier(&prelexed.lexed.tokens, reference.span, offset)
}

fn is_saved_place_declaration_name(file: &Path, offset: usize, symbol: &SymbolRef) -> bool {
    symbol.file == file && span_covers(symbol.span, offset)
}

fn member_hover_fact(
    snapshot: &AnalysisSnapshot,
    analyzed: &crate::AnalyzedFile,
    symbol: &SymbolRef,
) -> Option<SavedPlaceHoverFact> {
    let member_fact = snapshot
        .program
        .facts
        .resource_members()
        .iter()
        .find(|member| {
            let resource = snapshot.program.facts.resource(member.resource);
            member.name_span == symbol.span
                && fact_file(snapshot, resource.module) == Some(symbol.file.as_path())
                && matches!(
                    (member.kind, symbol.kind),
                    (ResourceMemberKind::Field, SymbolKind::Field)
                        | (ResourceMemberKind::Group, SymbolKind::Layer)
                )
        })?;
    let member = resource_member_at(&analyzed.parsed.file, member_fact.span)?;
    match member {
        ResourceMember::Field(field) => Some(SavedPlaceHoverFact::Field {
            name: field.name.clone(),
            key_params: hover_key_params(&field.keys),
            ty: field.ty.to_string(),
            required: field.required,
            docs: field.docs.clone(),
        }),
        ResourceMember::Group(group) => Some(SavedPlaceHoverFact::Layer {
            name: group.name.clone(),
            key_params: hover_key_params(&group.keys),
            docs: group.docs.clone(),
        }),
    }
}

fn index_hover_fact(
    snapshot: &AnalysisSnapshot,
    analyzed: &crate::AnalyzedFile,
    symbol: &SymbolRef,
) -> Option<SavedPlaceHoverFact> {
    let index_fact = snapshot
        .program
        .facts
        .store_indexes()
        .iter()
        .find(|index| {
            let store = snapshot.program.facts.store(index.store);
            index.name_span == symbol.span
                && fact_file(snapshot, store.module) == Some(symbol.file.as_path())
        })?;
    let index = store_index_at(&analyzed.parsed.file, index_fact.span)?;
    Some(SavedPlaceHoverFact::Index {
        name: index.name.clone(),
        args: index.args.clone(),
        unique: index.unique,
        docs: index.docs.clone(),
    })
}

fn fact_file(snapshot: &AnalysisSnapshot, module: ModuleId) -> Option<&Path> {
    snapshot
        .program
        .facts
        .modules()
        .get(module.0 as usize)
        .map(|module| module.source_file.as_path())
}

fn hover_key_params(keys: &[KeyParam]) -> Vec<SavedPlaceHoverKeyParam> {
    keys.iter()
        .map(|key| SavedPlaceHoverKeyParam {
            name: key.name.clone(),
            ty: key.ty.to_string(),
        })
        .collect()
}

fn function_hover_fact(
    snapshot: &AnalysisSnapshot,
    symbol: &SymbolRef,
    parsed: &FunctionDecl,
    checked: &CheckedFunction,
) -> SourceCallableFunctionFact {
    SourceCallableFunctionFact {
        name: checked.name.clone(),
        params: checked
            .params
            .iter()
            .zip(parsed.params.iter())
            .map(|(checked, parsed)| SourceCallableParamFact {
                name: checked.name.clone(),
                ty: checked.ty.clone(),
                docs: parsed.docs.clone(),
            })
            .collect(),
        return_type: checked.return_type.clone(),
        docs: parsed.docs.clone(),
        direct_effects: function_fact_for_symbol(snapshot, symbol, checked)
            .map(|fact| fact.direct_effects.clone()),
    }
}

fn is_function_hover_target(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    symbol: &SymbolRef,
) -> bool {
    let file = analyzed.path.as_path();
    index.references(symbol).iter().any(|reference| {
        reference.kind == SymbolKind::Function
            && reference.file == file
            && (reference.file != symbol.file || reference.span != symbol.span)
            && span_covers(reference.span, offset)
            && offset_is_on_last_identifier_half_open(
                &prelexed.lexed.tokens,
                reference.span,
                offset,
            )
    }) || is_function_declaration_name(snapshot, analyzed, prelexed, offset, symbol)
}

fn is_function_declaration_name(
    snapshot: &AnalysisSnapshot,
    analyzed: &AnalyzedFile,
    prelexed: &PrelexedSourceHover<'_>,
    offset: usize,
    symbol: &SymbolRef,
) -> bool {
    let file = analyzed.path.as_path();
    if symbol.file != file {
        return false;
    }
    let Some(analyzed) = snapshot.files.iter().find(|f| f.path == symbol.file) else {
        return false;
    };
    let Some(function) = parsed_function_at(&analyzed.parsed.file, symbol.span) else {
        return false;
    };
    offset_is_on_declaration_identifier(
        &analyzed.source,
        &prelexed.lexed.tokens,
        function.span,
        &function.name,
        offset,
    )
}

fn parsed_function_at(source: &SourceFile, span: SourceSpan) -> Option<&FunctionDecl> {
    source
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if span_contains_span(function.span, span) => {
                Some(function)
            }
            _ => None,
        })
}

fn parsed_const_at(source: &SourceFile, span: SourceSpan) -> Option<&marrow_syntax::ConstDecl> {
    source
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Const(constant) if span_contains_span(constant.span, span) => {
                Some(constant)
            }
            _ => None,
        })
}

fn parsed_resource_at(
    source: &SourceFile,
    span: SourceSpan,
) -> Option<&marrow_syntax::ResourceDecl> {
    source
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Resource(resource) if span_contains_span(resource.span, span) => {
                Some(resource)
            }
            _ => None,
        })
}

fn parsed_enum_at(source: &SourceFile, span: SourceSpan) -> Option<&marrow_syntax::EnumDecl> {
    source
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Enum(enum_decl) if span_contains_span(enum_decl.span, span) => {
                Some(enum_decl)
            }
            _ => None,
        })
}

fn parsed_enum_member_at(source: &SourceFile, span: SourceSpan) -> Option<&EnumMember> {
    for declaration in &source.declarations {
        let Declaration::Enum(enum_decl) = declaration else {
            continue;
        };
        if let Some(member) = enum_member_in(&enum_decl.members, span) {
            return Some(member);
        }
    }
    None
}

fn enum_member_in(members: &[EnumMember], span: SourceSpan) -> Option<&EnumMember> {
    for member in members {
        if span_contains_span(member.span, span) {
            return Some(member);
        }
        if let Some(member) = enum_member_in(&member.members, span) {
            return Some(member);
        }
    }
    None
}

fn parsed_enum_member_path_at(
    source: &SourceFile,
    span: SourceSpan,
) -> Option<(String, Vec<String>)> {
    for declaration in &source.declarations {
        let Declaration::Enum(enum_decl) = declaration else {
            continue;
        };
        let mut path = Vec::new();
        if enum_member_path_in(&enum_decl.members, span, &mut path) {
            return Some((enum_decl.name.clone(), path));
        }
    }
    None
}

fn enum_member_path_in(members: &[EnumMember], span: SourceSpan, path: &mut Vec<String>) -> bool {
    for member in members {
        path.push(member.name.clone());
        if span_contains_span(member.span, span) || enum_member_path_in(&member.members, span, path)
        {
            return true;
        }
        path.pop();
    }
    false
}

fn checked_function_for_parsed<'a>(
    snapshot: &'a AnalysisSnapshot,
    symbol: &SymbolRef,
    source_file: &SourceFile,
    parsed_function: &FunctionDecl,
) -> Option<&'a CheckedFunction> {
    let function_index = source_file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .position(|function| function.span == parsed_function.span)?;
    snapshot
        .program
        .modules
        .iter()
        .find(|module| module.source_file == symbol.file)?
        .functions
        .get(function_index)
}

fn checked_function_for_fact<'a>(
    snapshot: &'a AnalysisSnapshot,
    fact: &FunctionFact,
) -> Option<&'a CheckedFunction> {
    let module_fact = snapshot
        .program
        .facts
        .modules()
        .iter()
        .find(|module| module.id == fact.module)?;
    snapshot
        .program
        .modules
        .iter()
        .find(|module| module.source_file == module_fact.source_file)?
        .functions
        .get(fact.source_index as usize)
}

fn checked_const_at<'a>(
    snapshot: &'a AnalysisSnapshot,
    symbol: &SymbolRef,
) -> Option<&'a CheckedConst> {
    snapshot
        .program
        .modules
        .iter()
        .find(|module| module.source_file == symbol.file)?
        .constants
        .iter()
        .find(|constant| span_contains_span(constant.span, symbol.span))
}

fn function_fact_for_symbol<'a>(
    snapshot: &'a AnalysisSnapshot,
    symbol: &SymbolRef,
    checked_function: &CheckedFunction,
) -> Option<&'a FunctionFact> {
    let module = snapshot
        .program
        .modules
        .iter()
        .find(|module| module.source_file == symbol.file)?;
    let module_fact = snapshot
        .program
        .facts
        .modules()
        .iter()
        .find(|fact| fact.name == module.name && fact.source_file == module.source_file)?;
    snapshot.program.facts.functions().iter().find(|fact| {
        fact.module == module_fact.id
            && fact.name == checked_function.name
            && fact.span == checked_function.span
    })
}

fn parameter_use_name<'a>(
    source: &'a str,
    tokens: &[Token],
    offset: usize,
    function: &FunctionDecl,
) -> Option<&'a str> {
    if !span_covers(function.body.span, offset) {
        return None;
    }
    let index = token_index_at(tokens, offset)?;
    let token = &tokens[index];
    if token.kind != TokenKind::Identifier {
        return None;
    }
    let (previous, next) = significant_neighbors(tokens, index);
    if previous.is_some_and(|kind| {
        matches!(
            kind,
            TokenKind::DoubleColon | TokenKind::Dot | TokenKind::QuestionDot
        )
    }) || next.is_some_and(|kind| kind == TokenKind::Colon)
    {
        return None;
    }
    Some(token.text(source))
}

fn significant_neighbors(
    tokens: &[marrow_syntax::Token],
    index: usize,
) -> (Option<TokenKind>, Option<TokenKind>) {
    (
        previous_significant_kind(tokens, index),
        next_significant_kind(tokens, index),
    )
}

fn previous_significant_kind(tokens: &[marrow_syntax::Token], index: usize) -> Option<TokenKind> {
    tokens[..index]
        .iter()
        .rev()
        .find(|token| !is_trivia(token.kind))
        .map(|token| token.kind)
}

fn next_significant_kind(tokens: &[marrow_syntax::Token], index: usize) -> Option<TokenKind> {
    tokens
        .get(index + 1..)?
        .iter()
        .find(|token| !is_trivia(token.kind))
        .map(|token| token.kind)
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline | TokenKind::Comment | TokenKind::DocComment
    )
}

fn offset_is_on_declaration_identifier(
    source: &str,
    tokens: &[Token],
    span: SourceSpan,
    name: &str,
    offset: usize,
) -> bool {
    let Some((start, end)) = declaration_identifier_span(source, tokens, span, name) else {
        return false;
    };
    offset_in_name(start, end, offset)
}

fn offset_is_on_last_identifier_half_open(
    tokens: &[Token],
    span: SourceSpan,
    offset: usize,
) -> bool {
    let Some((start, end)) = last_identifier_span(tokens, span) else {
        return false;
    };
    offset_in_name(start, end, offset)
}

fn offset_in_name(start: usize, end: usize, offset: usize) -> bool {
    start <= offset && offset < end
}

fn token_index_at(tokens: &[Token], offset: usize) -> Option<usize> {
    let index = tokens.partition_point(|token| token.span.end_byte <= offset);
    tokens.get(index).and_then(|token| {
        (token.span.start_byte <= offset && offset < token.span.end_byte).then_some(index)
    })
}

fn declaration_docs<'a>(source: &'a SourceFile, symbol: &SymbolRef) -> Option<&'a [String]> {
    match symbol.kind {
        SymbolKind::Function => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Function(function)
                        if span_contains_span(function.span, symbol.span) =>
                    {
                        Some(function.docs.as_slice())
                    }
                    _ => None,
                })
        }
        SymbolKind::ModuleConst => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Const(constant)
                        if span_contains_span(constant.span, symbol.span) =>
                    {
                        Some(constant.docs.as_slice())
                    }
                    _ => None,
                })
        }
        SymbolKind::Resource => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Resource(resource)
                        if span_contains_span(resource.span, symbol.span) =>
                    {
                        Some(resource.docs.as_slice())
                    }
                    _ => None,
                })
        }
        SymbolKind::Enum => source
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Enum(enum_decl) if span_contains_span(enum_decl.span, symbol.span) => {
                    Some(enum_decl.docs.as_slice())
                }
                _ => None,
            }),
        SymbolKind::EnumMember => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Enum(enum_decl) => {
                        enum_member_docs(&enum_decl.members, symbol.span)
                    }
                    _ => None,
                })
        }
        SymbolKind::Field | SymbolKind::Layer => {
            source
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Resource(resource) => member_docs(&resource.members, symbol.span),
                    _ => None,
                })
        }
        SymbolKind::Index => source
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Store(store) => store
                    .indexes
                    .iter()
                    .find(|index| span_contains_span(index.span, symbol.span))
                    .map(|index| index.docs.as_slice()),
                _ => None,
            }),
        SymbolKind::Local | SymbolKind::Param | SymbolKind::ModuleRef => None,
    }
}

fn enum_member_docs(members: &[EnumMember], span: SourceSpan) -> Option<&[String]> {
    for member in members {
        if span_contains_span(member.span, span) {
            return Some(&member.docs);
        }
        if let Some(docs) = enum_member_docs(&member.members, span) {
            return Some(docs);
        }
    }
    None
}

fn member_docs(members: &[ResourceMember], span: SourceSpan) -> Option<&[String]> {
    for member in members {
        match member {
            ResourceMember::Field(field) if span_contains_span(field.span, span) => {
                return Some(&field.docs);
            }
            ResourceMember::Group(group) if span_contains_span(group.span, span) => {
                return Some(&group.docs);
            }
            ResourceMember::Group(group) => {
                if let Some(docs) = member_docs(&group.members, span) {
                    return Some(docs);
                }
            }
            _ => {}
        }
    }
    None
}

fn resource_member_at(source: &SourceFile, span: SourceSpan) -> Option<&ResourceMember> {
    for declaration in &source.declarations {
        let Declaration::Resource(resource) = declaration else {
            continue;
        };
        if let Some(member) = resource_member_in(&resource.members, span) {
            return Some(member);
        }
    }
    None
}

fn resource_member_in(members: &[ResourceMember], span: SourceSpan) -> Option<&ResourceMember> {
    for member in members {
        match member {
            ResourceMember::Field(field) if span_contains_span(field.span, span) => {
                return Some(member);
            }
            ResourceMember::Group(group) if span_contains_span(group.span, span) => {
                return Some(member);
            }
            ResourceMember::Group(group) => {
                if let Some(member) = resource_member_in(&group.members, span) {
                    return Some(member);
                }
            }
            _ => {}
        }
    }
    None
}

fn store_index_at(source: &SourceFile, span: SourceSpan) -> Option<&IndexDecl> {
    source
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Store(store) => store
                .indexes
                .iter()
                .find(|index| span_contains_span(index.span, span)),
            _ => None,
        })
}

fn offset_is_on_last_identifier(tokens: &[Token], span: SourceSpan, offset: usize) -> bool {
    let Some((start, end)) = last_identifier_span(tokens, span) else {
        return false;
    };
    start <= offset && offset <= end
}

fn last_identifier_span(tokens: &[Token], span: SourceSpan) -> Option<(usize, usize)> {
    let mut found = None;
    for token in tokens {
        if token.kind == TokenKind::Identifier
            && span_covers(span, token.span.start_byte)
            && span_covers(span, token.span.end_byte)
        {
            found = Some((token.span.start_byte, token.span.end_byte));
        }
    }
    found
}

fn declaration_identifier_span(
    source: &str,
    tokens: &[Token],
    span: SourceSpan,
    name: &str,
) -> Option<(usize, usize)> {
    tokens
        .iter()
        .find(|token| {
            token.kind == TokenKind::Identifier
                && span_covers(span, token.span.start_byte)
                && span_covers(span, token.span.end_byte)
                && token.text(source) == name
        })
        .map(|token| (token.span.start_byte, token.span.end_byte))
}

fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

fn span_covers_half_open(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset < span.end_byte
}

fn span_contains_span(outer: SourceSpan, inner: SourceSpan) -> bool {
    outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn hover_recovery_inspection_distinguishes_dynamic_from_nested_recovery() {
        let recovery = SourceHoverFact::Callable(SourceCallableHoverFact::ModuleConst {
            name: "items".to_string(),
            ty: Some(MarrowType::Sequence(Box::new(MarrowType::Unknown))),
            docs: Vec::new(),
        });
        assert!(recovery.contains_recovery_unknown());

        let dynamic = SourceHoverFact::Type(SourceTypeHoverFact {
            ty: MarrowType::Dynamic,
            docs: Vec::new(),
        });
        assert!(!dynamic.contains_recovery_unknown());

        let poison = SourceHoverFact::Type(SourceTypeHoverFact {
            ty: MarrowType::Invalid,
            docs: Vec::new(),
        });
        assert!(!poison.contains_recovery_unknown());
    }

    #[test]
    fn token_index_matches_first_half_open_token_at_every_source_byte() {
        let source = "module m\n\nfn f(value: int)\n    print($\"value {value}\") # note\n";
        let lexed = lex_source(source);
        for offset in 0..source.len() {
            let expected = lexed
                .tokens
                .iter()
                .position(|token| token.span.start_byte <= offset && offset < token.span.end_byte);
            assert_eq!(token_index_at(&lexed.tokens, offset), expected, "{offset}");
        }
    }
}
