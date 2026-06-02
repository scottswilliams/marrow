//! The checked-program artifact built alongside a project's diagnostics.
//!
//! [`check_project`](crate::check_project) builds [`CheckedProgram`] best-effort:
//! it includes a [`CheckedModule`] only for a library file that declared a module,
//! matched its path, is not a duplicate, and parsed without errors.
//! [`check_tests`](crate::check_tests) adds a module per clean test file, named
//! from its path (test files are scripts). Error-bearing files contribute no
//! module. The artifact never affects diagnostics; it is a structured view of the
//! same parse the checker already produced.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use marrow_schema::{ScalarType, Type};
use marrow_syntax::{Block, Expression, ParamMode, ParsedSource, SourceSpan, TypeRef};

use crate::facts::CheckedFacts;

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
    pub facts: CheckedFacts,
}

impl CheckedProgram {
    pub fn from_modules(modules: Vec<CheckedModule>) -> Self {
        let mut program = Self {
            modules,
            facts: CheckedFacts::default(),
        };
        program.rebuild_facts();
        program
    }

    fn rebuild_facts(&mut self) {
        self.facts = CheckedFacts::from_modules(&self.modules, &HashMap::new());
    }

    pub(crate) fn rebuild_facts_with_sources<'a, I>(&mut self, sources: I)
    where
        I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
    {
        let sources: HashMap<PathBuf, &ParsedSource> = sources
            .into_iter()
            .map(|(path, parsed)| (path.to_path_buf(), parsed))
            .collect();
        self.facts = CheckedFacts::from_modules(&self.modules, &sources);
    }

    pub(crate) fn rebuild_facts_with_sources_preserving_prefix<'a, I>(
        &mut self,
        prefix: &CheckedProgram,
        sources: I,
    ) where
        I: IntoIterator<Item = (&'a Path, &'a ParsedSource)>,
    {
        self.rebuild_facts_with_sources(sources);
        self.facts.overwrite_prefix_from(&prefix.facts);
    }

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
    pub stores: Vec<marrow_schema::StoreSchema>,
    pub enums: Vec<marrow_schema::EnumSchema>,
    pub enum_public: HashMap<String, bool>,
}

/// A module-level constant. Its type is the resolved annotation when one was
/// written; an unannotated constant leaves it `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedConst {
    pub name: String,
    pub ty: Option<MarrowType>,
    pub value: Option<Expression>,
    pub span: SourceSpan,
}

/// A checked function: its resolved signature, effect summary, and the temporary
/// syntax body bridge the current runtime still evaluates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedFunction {
    pub name: String,
    pub public: bool,
    pub params: Vec<CheckedParam>,
    pub return_type: Option<MarrowType>,
    pub span: SourceSpan,
    /// True when the body reads or writes any saved root (`^...`).
    pub touches_saved_data: bool,
    /// Temporary syntax body bridge. Checked executable IR will replace this as
    /// the runtime stops evaluating source syntax directly.
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
    /// A saved keyed-group entry, identified by its owning resource and group
    /// layer chain.
    GroupEntry {
        resource: String,
        layers: Vec<String>,
    },
    /// A store identity such as `Id(^books)`, carrying the store root.
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
    LocalTree {
        keys: Vec<MarrowType>,
        value: Box<MarrowType>,
    },
    /// An expression whose own type check already produced a primary diagnostic.
    /// It suppresses secondary "untyped value" hints while still keeping unknown
    /// dynamic values distinct.
    Invalid,
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
            Type::Identity(root) => Self::Identity(root),
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
}
