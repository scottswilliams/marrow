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
use std::path::{Path, PathBuf};

use marrow_schema::{ScalarType, Type};
use marrow_syntax::{Block, ParamMode, SourceSpan, TypeRef};

/// Identifies one source file in a [`CheckedProgram`] by the index of the module
/// that came from it. A program's modules are 1:1 with their files, so the index
/// is the file's stable id and the program needs no separate file table. A
/// runtime fault stamps the id of the module it was raised in, and a renderer maps
/// it back to a path with [`CheckedProgram::file_path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// The resolved shape of a checked project: every clean library module, in the
/// order their files were discovered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedProgram {
    pub modules: Vec<CheckedModule>,
}

impl CheckedProgram {
    /// The source file the given file id names, or `None` if the id is out of
    /// range (an id from a different program, or a fault with no project file).
    pub fn file_path(&self, id: FileId) -> Option<&Path> {
        self.modules
            .get(id.0 as usize)
            .map(|module| module.source_file.as_path())
    }

    /// The file id of `module`, identifying it by pointer within this program's
    /// own `modules`. Runs only on the cold path where a fault is leaving the
    /// frame that raised it, so the linear scan is off the hot path.
    pub fn file_id_of(&self, module: &CheckedModule) -> Option<FileId> {
        self.modules
            .iter()
            .position(|candidate| std::ptr::eq(candidate, module))
            .map(|index| FileId(index as u32))
    }
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
    pub enums: Vec<marrow_schema::EnumSchema>,
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
    /// One of the storable scalar types.
    Primitive(ScalarType),
    /// The checker-only type of a caught or thrown error value (`catch e: Error`,
    /// `throw Error(...)`). It has no storage form and never resolves to a scalar.
    Error,
    /// A resource declared in the same module, by name.
    Resource(String),
    /// A resource identity such as `Book::Id`, carrying the resource name.
    Identity(String),
    /// An enum, identified by its owning module and bare name. Identity is
    /// module-qualified: a bare `Status` referenced in module `b` resolves to
    /// `b::Status` (same-module first), and two same-named enums in different
    /// modules never alias. Nominal: an enum value equals only a value of the
    /// same enum.
    Enum {
        module: String,
        name: String,
    },
    Sequence(Box<MarrowType>),
    Unknown,
}

/// The module's named types — resources and enums — that resolution needs to
/// promote a bare [`Type::Named`] to a reference rather than [`MarrowType::Unknown`].
/// Both are looked up by name; an enum name and a resource name never collide
/// (the checker reports that as a duplicate declaration).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TypeNames<'a> {
    /// The qualified name of the module these names belong to, so a bare enum
    /// annotation resolves to that module's enum (`module::name` identity). Empty
    /// for a module-less script, whose enums are project-unique by construction.
    pub module: &'a str,
    pub resources: &'a [String],
    pub enums: &'a [String],
}

impl MarrowType {
    /// Resolve a [`TypeRef`] against the named types declared in the same module.
    /// Best-effort and total: it never errors, falling back to
    /// [`MarrowType::Unknown`] for anything it cannot place.
    pub(crate) fn resolve(ty: &TypeRef, names: TypeNames<'_>) -> Self {
        Self::from_resolved(Type::resolve(ty), names)
    }

    /// Promote a schema-resolved [`Type`] to the checker's lattice using the
    /// module's named types. The structure (scalar, sequence, identity,
    /// `unknown`) is already decided; this layer only places a bare [`Type::Named`]
    /// as a resource reference, an enum reference, the checker-only `Error` type,
    /// or `Unknown`.
    pub(crate) fn from_resolved(ty: Type, names: TypeNames<'_>) -> Self {
        match ty {
            Type::Scalar(scalar) => Self::Primitive(scalar),
            Type::Sequence(element) => {
                Self::Sequence(Box::new(Self::from_resolved(*element, names)))
            }
            Type::Identity(resource) => Self::Identity(resource),
            Type::Unknown => Self::Unknown,
            // `Error` is the one checker-only type the store does not model, so it
            // never resolves to a scalar; recognize it here.
            Type::Named(name) if name == "Error" => Self::Error,
            Type::Named(name) if names.resources.contains(&name) => Self::Resource(name),
            // A bare enum annotation names the owning module's enum.
            Type::Named(name) if names.enums.contains(&name) => Self::Enum {
                module: names.module.to_string(),
                name,
            },
            Type::Named(_) => Self::Unknown,
        }
    }

    /// Whether `ty` names a type the checker recognizes: a scalar, `Error`,
    /// `unknown`, a `sequence[...]` of a known type, a qualified or identity type
    /// (anything containing `::`, validated more precisely later), or a resource
    /// or enum declared anywhere in the project. Used to flag unknown type
    /// annotations without false-flagging cross-module references.
    pub(crate) fn names_known_type(
        ty: &TypeRef,
        resources: &HashSet<String>,
        enums: &HashSet<String>,
    ) -> bool {
        Self::resolved_is_known(&Type::resolve(ty), resources, enums)
    }

    fn resolved_is_known(ty: &Type, resources: &HashSet<String>, enums: &HashSet<String>) -> bool {
        match ty {
            Type::Scalar(_) | Type::Identity(_) | Type::Unknown => true,
            Type::Sequence(element) => Self::resolved_is_known(element, resources, enums),
            // A qualified name (any `::`) is assumed a cross-module reference and
            // accepted; `Error`, declared resources, and declared enums are known
            // by name.
            Type::Named(name) => {
                name.contains("::")
                    || name == "Error"
                    || resources.contains(name)
                    || enums.contains(name)
            }
        }
    }
}
