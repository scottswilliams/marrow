//! The checked-program artifact built alongside a project's diagnostics.
//!
//! [`check_project`](crate::check_project) builds [`CheckedProgram`] best-effort:
//! it includes a [`CheckedModule`] only for a library file that declared a module,
//! matched its path, is not a duplicate, and parsed without errors.
//! [`check_tests`](crate::check_tests) adds a module per clean test file, named
//! from its path (test files are scripts). Error-bearing files contribute no
//! module. The artifact never affects diagnostics; it is a structured view of the
//! same parse the checker already produced.

use std::collections::HashSet;
use std::path::PathBuf;

use marrow_syntax::{Block, ParamMode, SourceSpan, TypeRef};

/// The resolved shape of a checked project: every clean library module, in the
/// order their files were discovered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedProgram {
    pub modules: Vec<CheckedModule>,
}

/// One library module: its qualified name, the file it came from, and the
/// declarations it contributes. Names within a module are kept in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedModule {
    /// The qualified module name, such as `shelf::books`.
    pub name: String,
    pub source_file: PathBuf,
    pub span: SourceSpan,
    /// Resolved `use` target names, in source order.
    pub imports: Vec<String>,
    pub constants: Vec<CheckedConst>,
    pub functions: Vec<CheckedFunction>,
    pub resources: Vec<marrow_schema::ResourceSchema>,
}

/// A module-level constant. Its type is the resolved annotation when one was
/// written; an unannotated constant leaves it `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedConst {
    pub name: String,
    pub ty: Option<MarrowType>,
    pub span: SourceSpan,
}

/// A checked function: its resolved signature — name, visibility, parameters,
/// return type, whether its body touches saved data — and the body itself, which
/// the runtime evaluates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedFunction {
    pub name: String,
    pub public: bool,
    pub params: Vec<CheckedParam>,
    pub return_type: Option<MarrowType>,
    pub span: SourceSpan,
    /// True when the body reads or writes any saved root (`^...`).
    pub touches_saved_data: bool,
    /// The function body, as parsed, for the runtime to evaluate.
    pub body: Block,
}

/// One resolved function parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedParam {
    pub name: String,
    pub mode: Option<ParamMode>,
    pub ty: MarrowType,
}

/// A resolved Marrow type, best-effort. Anything the checker cannot resolve
/// (including cross-module resource references) is [`MarrowType::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarrowType {
    Primitive(PrimitiveType),
    /// A resource declared in the same module, by name.
    Resource(String),
    /// A resource identity such as `Book::Id`, carrying the resource name.
    Identity(String),
    Sequence(Box<MarrowType>),
    Unknown,
}

/// The built-in scalar types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveType {
    Int,
    Decimal,
    Bool,
    String,
    Bytes,
    Date,
    Instant,
    Duration,
    ErrorCode,
    Error,
}

impl MarrowType {
    /// Resolve a [`TypeRef`] against the resource names declared in the same
    /// module. Best-effort and total: it never errors, falling back to
    /// [`MarrowType::Unknown`] for anything it cannot place.
    pub(crate) fn resolve(ty: &TypeRef, module_resources: &[String]) -> Self {
        Self::resolve_text(ty.text.trim(), module_resources)
    }

    fn resolve_text(text: &str, module_resources: &[String]) -> Self {
        // `sequence[T]` is built-in element-type sugar; recurse on the element.
        if let Some(element) = text
            .strip_prefix("sequence[")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            return Self::Sequence(Box::new(Self::resolve_text(
                element.trim(),
                module_resources,
            )));
        }
        if let Some(primitive) = PrimitiveType::from_keyword(text) {
            return Self::Primitive(primitive);
        }
        if text == "unknown" {
            return Self::Unknown;
        }
        // A resource identity such as `Book::Id` names the resource it wraps.
        if let Some(resource) = text.strip_suffix("::Id") {
            return Self::Identity(resource.to_string());
        }
        if module_resources.iter().any(|name| name == text) {
            return Self::Resource(text.to_string());
        }
        Self::Unknown
    }

    /// Whether `text` names a type the checker recognizes: a primitive, `unknown`,
    /// a `sequence[...]` of a known type, a qualified or identity type (anything
    /// containing `::`, validated more precisely later), or a resource declared
    /// anywhere in `resources` (project-wide). Used to flag unknown type
    /// annotations without false-flagging cross-module references.
    pub(crate) fn names_known_type(text: &str, resources: &HashSet<String>) -> bool {
        let text = text.trim();
        if let Some(element) = text
            .strip_prefix("sequence[")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            return Self::names_known_type(element.trim(), resources);
        }
        text.contains("::")
            || text == "unknown"
            || PrimitiveType::from_keyword(text).is_some()
            || resources.contains(text)
    }
}

impl PrimitiveType {
    /// Map a primitive type keyword to its [`PrimitiveType`], using the
    /// canonical source spelling (`int`, `string`, `ErrorCode`, ...). Anything
    /// that is not a primitive keyword returns `None`.
    fn from_keyword(text: &str) -> Option<Self> {
        Some(match text {
            "int" => Self::Int,
            "decimal" => Self::Decimal,
            "bool" => Self::Bool,
            "string" => Self::String,
            "bytes" => Self::Bytes,
            "date" => Self::Date,
            "instant" => Self::Instant,
            "duration" => Self::Duration,
            "ErrorCode" => Self::ErrorCode,
            "Error" => Self::Error,
            _ => return None,
        })
    }
}
