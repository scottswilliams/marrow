//! The project-wide binding index: every identifier *use* resolved to the
//! *definition* it names, respecting lexical scope (shadowing) and `use` import
//! aliases, plus a rename-safety classification. It is the resolution layer the
//! editor tooling (go-to-definition, find-all-references, rename) builds on. It
//! does not reimplement name resolution: it reuses the checker's unified `resolve`
//! (via `resolve_function_in_module`) for module-aware function calls,
//! checked executable saved-place lowering for saved data, and the same block
//! traversal shape the checker's scope lowering uses (a
//! frame per block, with loop and catch bindings scoped to their body) so a
//! local's scope cannot drift from the one the checker builds.
//!
//! A single-segment name resolves at the identifier token; a qualified call at
//! the whole path span; an enum member literal is split so the enum segment and
//! each written member-path segment resolve independently. Source-only
//! definitions are recorded at their identifier tokens, including bindings whose
//! AST node only carries an enclosing statement span.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use marrow_schema::Type as SchemaType;
use marrow_syntax::{
    Block, Declaration, EnumMember, Expression, LexedSource, MatchArm, ParamDecl, ParsedSource,
    ResourceDecl, ResourceMember, SourceSpan, Statement, StoreDecl, Token, TokenKind, TypeExpr,
};

use crate::MarrowType;
use crate::annotation_refs::{
    TypeAnnotationBodies, type_ref_path_leaf_span, walk_declaration_type_refs,
};
use crate::checks::{catch_frame, file_prelude, for_frame};
use crate::enums::{
    EnumAnnotationResolution, EnumMemberPathResolution, enum_schema_in,
    resolve_diagnosed_annotation_type, resolve_enum_annotation, resolve_enum_member_path,
    resolve_type,
};
use crate::executable::{SavedMemberRefKind, SavedPlaceResolver, lower_expr_for_file};
use crate::facts::{FunctionId, LocalId};
use crate::infer::{infer_only, local_binding};
use crate::source_spans::{identifier_span_in, last_identifier_span_in, source_span_at};
use crate::{
    AnalysisSnapshot, CheckedProgram, Def, DefItem, Resolution, ResolvableKind, expand_alias,
    resolve, resolve_function_in_module, short_name, split_type_path,
};

/// What a [`SymbolRef`] names. The kinds the index resolves, grouped by where they
/// live: function-local bindings (`Param`, `Local`), module-level declarations
/// (`ModuleConst`, `Function`, `Resource`), enum declarations and members
/// (`Enum`, `EnumMember`), the members of a resource that name saved data
/// (`Field`, `Layer`, `Index`), and imported module aliases (`ModuleRef`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// A function parameter, scoped to that function's body.
    Param,
    /// A `const`/`var` local, scoped to the block that introduces it.
    Local,
    /// A module-level constant.
    ModuleConst,
    /// A declared function.
    Function,
    /// A declared resource (its type name).
    Resource,
    /// A declared enum type.
    Enum,
    /// A declared enum member.
    EnumMember,
    /// A stored field of a resource (a top-level field or a group field).
    Field,
    /// A keyed layer or group of a resource.
    Layer,
    /// A declared lookup index of a store.
    Index,
    /// An imported module, by the short name a `use` brings into the file.
    ModuleRef,
}

/// One occurrence of a symbol in source: a definition site or a use. The `span`
/// is into the file at `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRef {
    pub file: PathBuf,
    pub span: SourceSpan,
    pub kind: SymbolKind,
}

/// The exact source occurrence under a cursor and the definition it resolves to.
///
/// `reference` preserves the internal binding selected by cursor position. Use
/// this when duplicate saved-data bindings intentionally share a definition site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolOccurrence {
    pub definition: SymbolRef,
    pub reference: SymbolRef,
}

/// Whether renaming a symbol is safe to do purely in source.
///
/// `SourceOnly` symbols exist only in source, so they are not backed by saved
/// data. Most have source rename actions, but module aliases intentionally do not:
/// v0.1 imports have no alias syntax, so editing the imported path leaf would
/// retarget the import. `SavedDataBacked` symbols name something encoded on disk
/// — a saved field/group/layer/index, or a resource attached to a saved root.
/// Renaming such a symbol requires catalog-backed evolution intent rather than a
/// source-only edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameSafety {
    /// Safe to rename in source alone.
    SourceOnly,
    /// Names data encoded on disk; source-only rename is not enough.
    SavedDataBacked,
}

/// A definition together with every reference to it (the definition included), as
/// the index records them internally before answering lookups.
#[derive(Debug, Clone)]
struct Binding {
    definition: SymbolRef,
    references: Vec<SymbolRef>,
    safety: RenameSafety,
    rename_target: Option<RenameTarget>,
    parameter: Option<ParameterDefinition>,
}

#[derive(Debug, Clone)]
enum RenameTarget {
    EvolvePath(String),
    EnumMember {
        enum_name: String,
        member_path: Vec<String>,
    },
}

impl RenameTarget {
    fn source(&self) -> String {
        match self {
            Self::EvolvePath(path) => path.clone(),
            Self::EnumMember {
                enum_name,
                member_path,
            } => format!("{enum_name}::{}", member_path.join("::")),
        }
    }

    fn renamed_source(&self, new_name: &str) -> String {
        match self {
            Self::EvolvePath(path) => renamed_evolve_path(path, new_name),
            Self::EnumMember {
                enum_name,
                member_path,
            } => {
                let mut renamed = member_path.clone();
                if let Some(last) = renamed.last_mut() {
                    *last = new_name.to_string();
                }
                format!("{enum_name}::{}", renamed.join("::"))
            }
        }
    }
}

/// A resolved project's binding index: definitions, their references, and
/// rename-safety, keyed for position lookup. Build it with
/// [`build_binding_index`].
#[derive(Debug, Clone, Default)]
pub struct BindingIndex {
    bindings: Vec<Binding>,
    positions: HashMap<PathBuf, FileOccurrenceIndex>,
}

#[derive(Debug, Clone, Default)]
struct FileOccurrenceIndex {
    // Disjoint, start-sorted winner intervals. Overlapping references are
    // resolved while building the index, so every cursor lookup is one binary
    // search and preserves the binding API's tightest-span/first-tie rule.
    segments: Vec<OccurrenceSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OccurrenceSegment {
    start: usize,
    end: usize,
    binding: usize,
    reference: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveOccurrence {
    width: usize,
    ordinal: usize,
    binding: usize,
    reference: usize,
}

#[derive(Debug, Default)]
struct OccurrenceEvents {
    add: Vec<ActiveOccurrence>,
    remove: Vec<ActiveOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEdit {
    pub file: PathBuf,
    pub span: SourceSpan,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameAction {
    pub edits: Vec<SourceEdit>,
    pub evolve_rename: Option<String>,
}

/// A source-only renameable occurrence at a cursor position.
///
/// The `reference` span is the token-tight occurrence the editor should
/// pre-select for `prepareRename`; `definition` is the binding whose source-only
/// rename action edits every occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRenameOccurrence {
    pub definition: SymbolRef,
    pub reference: SymbolRef,
}

/// Why a source-only rename cannot be produced at a cursor position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRenameError {
    /// The cursor is not on a known binding-index symbol.
    NoSymbol,
    /// The cursor is on a symbol with no independent source rename action.
    NotRenameable,
    /// The requested replacement is not one Marrow identifier.
    InvalidName,
    /// The symbol names saved data and needs catalog evolution intent.
    SavedDataBacked,
}

/// Checked identity for a parameter symbol recorded by the binding index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterDefinition {
    /// The checked function that owns the parameter.
    pub function: FunctionId,
    /// The parameter's checked local fact inside that function.
    pub local: LocalId,
    /// The parameter's ordinal in `FunctionFact::params` and the parsed signature.
    pub index: usize,
}

/// Build the binding index for an analyzed project. Walks every clean module's
/// declarations to collect definitions, then every function body and saved-path
/// expression to collect uses, resolving each use to its definition through the
/// same scope, alias, and schema logic the checker uses.
pub fn build_binding_index(snapshot: &AnalysisSnapshot) -> BindingIndex {
    let lexed = snapshot
        .files
        .iter()
        .map(|file| marrow_syntax::lex_source(&file.source))
        .collect::<Vec<_>>();
    build_binding_index_from_lexed(snapshot, &lexed)
}

/// Build the project binding index from a snapshot-aligned lexical cache.
/// Compiler-maintainer analyses use this seam so binding resolution and hover
/// classification share the same single tokenization of each file.
pub(crate) fn build_binding_index_from_lexed(
    snapshot: &AnalysisSnapshot,
    lexed: &[LexedSource],
) -> BindingIndex {
    assert_eq!(
        snapshot.files.len(),
        lexed.len(),
        "the binding-index lexical cache must align with the analyzed files",
    );
    let mut builder = IndexBuilder::new(&snapshot.program);
    // Definitions come from the parsed files, which carry source spans the
    // resolved program does not.
    for file in &snapshot.files {
        builder.collect_definitions(&file.path, &file.parsed, &file.source);
    }
    // Uses are resolved against the definitions, so collect them in a second pass.
    for (file, lexed) in snapshot.files.iter().zip(lexed) {
        builder.collect_uses(&file.path, &file.parsed, &file.source, &lexed.tokens);
    }
    builder.finish()
}

impl BindingIndex {
    /// The definition site of the symbol at byte `offset` in `file`, if the cursor
    /// sits on a definition or a use of a known symbol. A cursor on a definition
    /// returns that definition; a cursor on a use returns the definition it names.
    pub fn definition(&self, file: &Path, offset: usize) -> Option<SymbolRef> {
        self.binding_at(file, offset)
            .map(|binding| binding.definition.clone())
    }

    /// The exact occurrence at byte `offset` in `file`, paired with the definition
    /// it resolves to. Unlike [`definition`](Self::definition), this retains the
    /// reference from the selected binding, so callers do not collapse distinct
    /// bindings that share the same source definition span.
    pub fn occurrence(&self, file: &Path, offset: usize) -> Option<SymbolOccurrence> {
        let (binding, reference) = self.binding_reference_at(file, offset)?;
        Some(SymbolOccurrence {
            definition: binding.definition.clone(),
            reference: reference.clone(),
        })
    }

    /// Every reference to `def`'s symbol, the definition included, in the order
    /// they were collected (definitions first, then uses in source order). `def`
    /// may be any [`SymbolRef`] of the symbol — a definition or a use — so a caller
    /// can pass the result of [`definition`](Self::definition) straight through.
    pub fn references(&self, def: &SymbolRef) -> Vec<SymbolRef> {
        self.binding_for(def)
            .map(|binding| binding.references.clone())
            .unwrap_or_default()
    }

    /// The rename safety of `def`'s symbol: whether it is source-only or backed by
    /// data encoded on disk. `def` may be any reference of the symbol.
    pub fn rename_safety(&self, def: &SymbolRef) -> RenameSafety {
        self.binding_for(def)
            .map(|binding| binding.safety.clone())
            .unwrap_or(RenameSafety::SourceOnly)
    }

    /// Return the checked owner metadata for a parameter binding.
    ///
    /// `def` may be either the parameter definition or one of its uses. The result
    /// joins the token-tight source symbol to its checked function fact, local fact,
    /// and parameter ordinal so downstream tooling does not reconstruct signature
    /// identity from source text.
    pub fn parameter_definition(&self, def: &SymbolRef) -> Option<ParameterDefinition> {
        self.binding_for(def)?.parameter
    }

    pub fn rename_action(&self, def: &SymbolRef, new_name: &str) -> Option<RenameAction> {
        let binding = self.binding_for(def)?;
        if binding.definition.kind == SymbolKind::ModuleRef {
            return None;
        }
        let edits = binding
            .references
            .iter()
            .map(|reference| SourceEdit {
                file: reference.file.clone(),
                span: reference.span,
                replacement: new_name.to_string(),
            })
            .collect();
        let evolve_rename = match binding.safety {
            RenameSafety::SourceOnly => None,
            RenameSafety::SavedDataBacked => {
                let target = binding.rename_target.as_ref()?;
                let from = target.source();
                Some(canonical_evolve_rename(
                    &from,
                    &target.renamed_source(new_name),
                )?)
            }
        };
        Some(RenameAction {
            edits,
            evolve_rename,
        })
    }

    /// Return the token-tight source occurrence the editor should pre-select for
    /// source-only rename at `offset`.
    ///
    /// This refuses saved-data-backed symbols and imports that cannot be renamed
    /// without retargeting the import path.
    pub fn source_only_rename_occurrence_at(
        &self,
        file: &Path,
        offset: usize,
    ) -> Result<SourceRenameOccurrence, SourceRenameError> {
        let (binding, reference) = self
            .binding_reference_at(file, offset)
            .ok_or(SourceRenameError::NoSymbol)?;
        if binding.safety == RenameSafety::SavedDataBacked {
            return Err(SourceRenameError::SavedDataBacked);
        }
        if binding.definition.kind == SymbolKind::ModuleRef {
            return Err(SourceRenameError::NotRenameable);
        }
        Ok(SourceRenameOccurrence {
            definition: binding.definition.clone(),
            reference: reference.clone(),
        })
    }

    /// Build source edits for a source-only rename at `offset`.
    ///
    /// The replacement must be one valid Marrow identifier. Saved-data-backed
    /// symbols are refused here even though [`rename_action`](Self::rename_action)
    /// can describe their source edits and catalog evolution fragment.
    pub fn source_only_rename_action_at(
        &self,
        file: &Path,
        offset: usize,
        new_name: &str,
    ) -> Result<RenameAction, SourceRenameError> {
        if !is_valid_source_rename(new_name) {
            return Err(SourceRenameError::InvalidName);
        }
        let occurrence = self.source_only_rename_occurrence_at(file, offset)?;
        let action = self
            .rename_action(&occurrence.definition, new_name)
            .ok_or(SourceRenameError::NotRenameable)?;
        if action.evolve_rename.is_some() {
            return Err(SourceRenameError::SavedDataBacked);
        }
        Ok(action)
    }

    /// The binding whose definition span or any reference span covers `offset` in
    /// `file`. The tightest covering span wins, so a use nested inside a wider
    /// declaration resolves to the use's symbol rather than the enclosing one.
    fn binding_at(&self, file: &Path, offset: usize) -> Option<&Binding> {
        self.binding_reference_at(file, offset)
            .map(|(binding, _)| binding)
    }

    fn binding_reference_at(&self, file: &Path, offset: usize) -> Option<(&Binding, &SymbolRef)> {
        let location = self.positions.get(file)?.at(offset)?;
        let binding = self.bindings.get(location.binding)?;
        let reference = binding.references.get(location.reference)?;
        Some((binding, reference))
    }

    /// The binding `def` belongs to: the one whose definition site matches, falling
    /// back to one whose references include `def` (so a use ref also identifies its
    /// binding).
    fn binding_for(&self, def: &SymbolRef) -> Option<&Binding> {
        self.bindings
            .iter()
            .find(|binding| &binding.definition == def)
            .or_else(|| {
                self.bindings
                    .iter()
                    .find(|binding| binding.references.contains(def))
            })
    }
}

/// A definition's identity within the index: an index into the builder's binding
/// list. Uses are attributed to a binding by this id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DefId(usize);

struct ResolvedEnumMemberUse {
    enum_def: DefId,
    enum_index: usize,
    member_defs: Vec<(usize, DefId)>,
}

/// Accumulates definitions and the uses attributed to them. Resolution reuses the
/// checker's helpers; this only tracks identity and spans.
struct IndexBuilder<'p> {
    program: &'p CheckedProgram,
    bindings: Vec<Binding>,
    module_aliases: HashMap<PathBuf, HashMap<String, DefId>>,
    /// Module-level definitions visible project-wide, keyed for use resolution:
    /// `(module name, leaf name, kind)` -> the defining binding. Module-qualified
    /// functions, resources, and constants resolve here.
    module_scope: ModuleScope,
}

/// Project-wide module-level definitions, indexed for resolution. Functions are
/// resolved through [`resolve_function_in_module`]; constants and resources are
/// resolved by name here.
#[derive(Default)]
struct ModuleScope {
    /// `module name -> (const name -> DefId)`.
    constants: HashMap<String, HashMap<String, DefId>>,
    /// `(module name, resource name) -> DefId` for the resource type symbol.
    resources: HashMap<(String, String), DefId>,
    /// `(module name, function name) -> DefId`, the function's definition.
    functions: HashMap<(String, String), DefId>,
    /// `(module name, enum name) -> DefId`, the enum's definition.
    enums: HashMap<(String, String), DefId>,
    /// `(module name, enum name, member path) -> DefId` for enum members.
    enum_members: HashMap<(String, String, Vec<String>), DefId>,
    /// `(file, saved root, member chain) -> DefId` for saved members, so a saved
    /// read resolves to the field/layer/index it touches.
    saved_members: HashMap<(String, Vec<String>), DefId>,
}

impl<'p> IndexBuilder<'p> {
    fn new(program: &'p CheckedProgram) -> Self {
        Self {
            program,
            bindings: Vec::new(),
            module_aliases: HashMap::new(),
            module_scope: ModuleScope::default(),
        }
    }

    /// Record a definition, returning its id. The definition is its own first
    /// reference.
    fn define(&mut self, def: SymbolRef, safety: RenameSafety) -> DefId {
        self.define_with_parameter(def, safety, None, None)
    }

    fn define_with_evolve_path(
        &mut self,
        def: SymbolRef,
        safety: RenameSafety,
        evolve_path: Option<String>,
    ) -> DefId {
        self.define_with_rename_target(def, safety, evolve_path.map(RenameTarget::EvolvePath))
    }

    fn define_with_rename_target(
        &mut self,
        def: SymbolRef,
        safety: RenameSafety,
        rename_target: Option<RenameTarget>,
    ) -> DefId {
        self.define_with_parameter(def, safety, rename_target, None)
    }

    fn define_parameter(
        &mut self,
        def: SymbolRef,
        parameter: Option<ParameterDefinition>,
    ) -> DefId {
        self.define_with_parameter(def, RenameSafety::SourceOnly, None, parameter)
    }

    fn define_with_parameter(
        &mut self,
        def: SymbolRef,
        safety: RenameSafety,
        rename_target: Option<RenameTarget>,
        parameter: Option<ParameterDefinition>,
    ) -> DefId {
        let id = DefId(self.bindings.len());
        self.bindings.push(Binding {
            references: vec![def.clone()],
            definition: def,
            safety,
            rename_target,
            parameter,
        });
        id
    }

    /// Attribute a use at `span` in `file` to `def`.
    fn use_of(&mut self, def: DefId, file: &Path, span: SourceSpan, kind: SymbolKind) {
        self.bindings[def.0].references.push(SymbolRef {
            file: file.to_path_buf(),
            span,
            kind,
        });
    }

    /// The module name a parsed file declares, or the empty string for a
    /// module-less script (whose top-level names are not project-visible).
    fn module_name(parsed: &ParsedSource) -> String {
        parsed
            .file
            .module
            .as_ref()
            .map(|module| module.name.clone())
            .unwrap_or_default()
    }

    /// Collect every definition a file contributes: module constants, functions,
    /// resources, store-backed members, and `use` aliases.
    fn collect_definitions(&mut self, file: &Path, parsed: &ParsedSource, source: &str) {
        let module = Self::module_name(parsed);
        let stores: Vec<&StoreDecl> = parsed
            .file
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Store(store) => Some(store),
                _ => None,
            })
            .collect();

        // `use shelf::books` introduces the short name `books` as a module
        // reference.
        for use_decl in &parsed.file.uses {
            let short = short_name(&use_decl.name);
            if let Some(span) = last_name_span(source, use_decl.span, short) {
                let id = self.define(
                    SymbolRef {
                        file: file.to_path_buf(),
                        span,
                        kind: SymbolKind::ModuleRef,
                    },
                    RenameSafety::SourceOnly,
                );
                self.module_aliases
                    .entry(file.to_path_buf())
                    .or_default()
                    .insert(short.to_string(), id);
            }
        }

        for declaration in &parsed.file.declarations {
            match declaration {
                Declaration::Const(constant) => {
                    let Some(span) = name_span(source, constant.span, &constant.name) else {
                        continue;
                    };
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span,
                            kind: SymbolKind::ModuleConst,
                        },
                        RenameSafety::SourceOnly,
                    );
                    self.module_scope
                        .constants
                        .entry(module.clone())
                        .or_default()
                        .insert(constant.name.clone(), id);
                }
                Declaration::Function(function) => {
                    let Some(span) = name_span(source, function.span, &function.name) else {
                        continue;
                    };
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span,
                            kind: SymbolKind::Function,
                        },
                        RenameSafety::SourceOnly,
                    );
                    self.module_scope
                        .functions
                        .insert((module.clone(), function.name.clone()), id);
                }
                Declaration::Resource(resource) => {
                    let has_store = stores.iter().any(|store| store.resource == resource.name);
                    self.collect_resource(file, source, &module, resource, has_store);
                }
                Declaration::Store(store) => {
                    if let Some(resource) = resource_decl(parsed, &store.resource) {
                        self.collect_store(file, source, store, resource);
                    }
                }
                Declaration::Enum(enum_decl) => {
                    if let Some(span) = name_span(source, enum_decl.span, &enum_decl.name) {
                        let id = self.define_with_evolve_path(
                            SymbolRef {
                                file: file.to_path_buf(),
                                span,
                                kind: SymbolKind::Enum,
                            },
                            RenameSafety::SavedDataBacked,
                            Some(enum_decl.name.clone()),
                        );
                        self.module_scope
                            .enums
                            .insert((module.clone(), enum_decl.name.clone()), id);
                    }
                    self.collect_enum_members(
                        file,
                        source,
                        &module,
                        &enum_decl.name,
                        &mut Vec::new(),
                        &enum_decl.members,
                    );
                }
                // Evolve blocks declare catalog intent, not source symbols.
                // Surfaces expose syntax-only declarations here, not bindable symbols.
                Declaration::Evolve(_) | Declaration::Surface(_) => {}
            }
        }
    }

    fn collect_enum_members(
        &mut self,
        file: &Path,
        source: &str,
        module: &str,
        enum_name: &str,
        path: &mut Vec<String>,
        members: &[EnumMember],
    ) {
        for member in members {
            path.push(member.name.clone());
            if let Some(span) = name_span(source, member.span, &member.name) {
                let id = self.define_with_rename_target(
                    SymbolRef {
                        file: file.to_path_buf(),
                        span,
                        kind: SymbolKind::EnumMember,
                    },
                    RenameSafety::SavedDataBacked,
                    Some(RenameTarget::EnumMember {
                        enum_name: enum_name.to_string(),
                        member_path: path.clone(),
                    }),
                );
                self.module_scope.enum_members.insert(
                    (module.to_string(), enum_name.to_string(), path.clone()),
                    id,
                );
            }
            self.collect_enum_members(file, source, module, enum_name, path, &member.members);
            path.pop();
        }
    }

    /// Collect a resource definition, data-backed exactly when a store references
    /// it. Keyed by declaring module plus source name; use sites pick the module
    /// through the checker resolver.
    fn collect_resource(
        &mut self,
        file: &Path,
        source: &str,
        module: &str,
        resource: &ResourceDecl,
        has_store: bool,
    ) {
        let safety = if has_store {
            RenameSafety::SavedDataBacked
        } else {
            RenameSafety::SourceOnly
        };
        let Some(span) = name_span(source, resource.span, &resource.name) else {
            return;
        };
        let resource_id = self.define_with_evolve_path(
            SymbolRef {
                file: file.to_path_buf(),
                span,
                kind: SymbolKind::Resource,
            },
            safety,
            has_store.then(|| resource.name.clone()),
        );
        self.module_scope
            .resources
            .insert((module.to_string(), resource.name.clone()), resource_id);
    }

    fn collect_store(
        &mut self,
        file: &Path,
        source: &str,
        store: &StoreDecl,
        resource: &ResourceDecl,
    ) {
        self.collect_members(
            file,
            source,
            &store.root.root,
            &resource.name,
            &mut Vec::new(),
            &resource.members,
        );
        for index in &store.indexes {
            self.collect_index(file, source, &store.root.root, index);
        }
    }

    fn collect_index(
        &mut self,
        file: &Path,
        source: &str,
        root: &str,
        index: &marrow_syntax::IndexDecl,
    ) {
        let Some(span) = name_span(source, index.span, &index.name) else {
            return;
        };
        let id = self.define_with_evolve_path(
            SymbolRef {
                file: file.to_path_buf(),
                span,
                kind: SymbolKind::Index,
            },
            RenameSafety::SavedDataBacked,
            Some(format!("^{root}.{}", index.name)),
        );
        self.module_scope
            .saved_members
            .insert((root.to_string(), vec![index.name.clone()]), id);
    }

    /// Collect a resource's members, descending groups. `chain` is the saved-path
    /// member chain to the current level (group names already entered), so a use
    /// `^root(..).group.field` resolves to the field by its full chain.
    fn collect_members(
        &mut self,
        file: &Path,
        source: &str,
        root: &str,
        resource: &str,
        chain: &mut Vec<String>,
        members: &[ResourceMember],
    ) {
        for member in members {
            match member {
                ResourceMember::Field(field) => {
                    chain.push(field.name.clone());
                    if let Some(span) = name_span(source, field.span, &field.name) {
                        let id = self.define_with_evolve_path(
                            SymbolRef {
                                file: file.to_path_buf(),
                                span,
                                kind: SymbolKind::Field,
                            },
                            RenameSafety::SavedDataBacked,
                            Some(format!("{resource}.{}", chain.join("."))),
                        );
                        self.module_scope
                            .saved_members
                            .insert((root.to_string(), chain.clone()), id);
                    }
                    chain.pop();
                }
                ResourceMember::Group(group) => {
                    chain.push(group.name.clone());
                    if let Some(span) = name_span(source, group.span, &group.name) {
                        let id = self.define_with_evolve_path(
                            SymbolRef {
                                file: file.to_path_buf(),
                                span,
                                kind: SymbolKind::Layer,
                            },
                            RenameSafety::SavedDataBacked,
                            Some(format!("{resource}.{}", chain.join("."))),
                        );
                        self.module_scope
                            .saved_members
                            .insert((root.to_string(), chain.clone()), id);
                    }
                    self.collect_members(file, source, root, resource, chain, &group.members);
                    chain.pop();
                }
            }
        }
    }

    /// Collect the uses in a file: the `use` alias references, the saved-path reads
    /// in every function body, and the bare-name/call uses resolved against
    /// reconstructed lexical scope.
    fn collect_uses(&mut self, file: &Path, parsed: &ParsedSource, source: &str, tokens: &[Token]) {
        let module = Self::module_name(parsed);
        let prelude = file_prelude(self.program, file, parsed);

        let mut function_source_index = 0u32;
        for declaration in &parsed.file.declarations {
            self.collect_type_refs(file, declaration, source, &prelude.aliases, &module);
            if let Declaration::Function(function) = declaration {
                let function_id = self.function_id_for_source(file, function_source_index);
                function_source_index += 1;
                let mut scope: Vec<HashMap<String, DefId>> = vec![HashMap::new()];
                let mut type_scope: Vec<HashMap<String, MarrowType>> =
                    vec![prelude.module_constants.clone()];
                for (param_index, param) in function.params.iter().enumerate() {
                    let Some(span) = param_name_span(source, tokens, function.span, param) else {
                        continue;
                    };
                    let parameter = self.parameter_definition(function_id, param_index);
                    let id = self.define_parameter(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span,
                            kind: SymbolKind::Param,
                        },
                        parameter,
                    );
                    scope[0].insert(param.name.clone(), id);
                    type_scope[0].insert(
                        param.name.clone(),
                        MarrowType::keyed(
                            param.keys.iter().map(|key| {
                                resolve_type(&key.ty, self.program, &prelude.aliases, file)
                            }),
                            resolve_type(&param.ty, self.program, &prelude.aliases, file),
                        ),
                    );
                }
                let mut walker = UseWalker {
                    builder: self,
                    file,
                    source,
                    module: &module,
                    aliases: &prelude.aliases,
                };
                walker.walk_block(&function.body, &mut scope, &mut type_scope);
            }
        }
    }

    fn function_id_for_source(&self, file: &Path, source_index: u32) -> Option<FunctionId> {
        let module_index = self.program.module_index_by_file(file)?;
        self.program
            .facts
            .function_id_at(module_index as u32, source_index)
    }

    fn parameter_definition(
        &self,
        function: Option<FunctionId>,
        index: usize,
    ) -> Option<ParameterDefinition> {
        let function = function?;
        let local = self.program.facts.function(function).params.get(index)?.id;
        Some(ParameterDefinition {
            function,
            local,
            index,
        })
    }

    fn collect_type_refs(
        &mut self,
        file: &Path,
        declaration: &Declaration,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
    ) {
        let bodies = match declaration {
            Declaration::Evolve(_) => TypeAnnotationBodies::Include,
            _ => TypeAnnotationBodies::Omit,
        };
        walk_declaration_type_refs(declaration, bodies, &mut |ty| {
            self.collect_type_ref(file, source, aliases, module, ty);
        });

        if let Declaration::Store(store) = declaration
            && let Some(def) = self
                .module_scope
                .resources
                .get(&(module.to_string(), store.resource.clone()))
                .copied()
        {
            let Some(span) = name_span(source, store.span, &store.resource) else {
                return;
            };
            self.use_of(def, file, span, SymbolKind::Resource);
        }
    }

    fn collect_type_ref(
        &mut self,
        file: &Path,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
        ty: &TypeExpr,
    ) {
        let schema_type = SchemaType::resolve(ty);
        self.collect_type_module_alias_use(file, source, ty, &schema_type);
        if let EnumAnnotationResolution::Visible(resolved) =
            resolve_enum_annotation(ty, self.program, aliases, file)
        {
            let Some(def) = self
                .module_scope
                .enums
                .get(&(resolved.module.clone(), resolved.name.clone()))
                .copied()
            else {
                return;
            };
            if let Some(span) = type_ref_path_leaf_span(source, ty, &resolved.name) {
                self.use_of(def, file, span, SymbolKind::Enum);
            }
            return;
        }
        self.collect_resource_type_ref(file, source, aliases, module, ty);
    }

    fn collect_type_module_alias_use(
        &mut self,
        file: &Path,
        source: &str,
        ty: &TypeExpr,
        schema_type: &SchemaType,
    ) {
        match schema_type {
            SchemaType::Sequence(element) | SchemaType::Optional(element) => {
                self.collect_type_module_alias_use(file, source, ty, element);
            }
            SchemaType::Named(name) => {
                let segments = split_type_path(name);
                self.collect_module_alias_use(file, source, ty.span(), &segments);
            }
            SchemaType::Identity(_) | SchemaType::Scalar(_) | SchemaType::Unknown => {}
        }
    }

    fn collect_module_alias_use(
        &mut self,
        file: &Path,
        source: &str,
        span: SourceSpan,
        segments: &[String],
    ) {
        let Some(first) = segments.first().filter(|_| segments.len() > 1) else {
            return;
        };
        let Some(def) = self
            .module_aliases
            .get(file)
            .and_then(|aliases| aliases.get(first.as_str()))
            .copied()
        else {
            return;
        };
        if let Some(segment_spans) = qualified_segment_spans(source, span, segments)
            && let Some(alias_span) = segment_spans.first().copied()
        {
            self.use_of(def, file, alias_span, SymbolKind::ModuleRef);
        }
    }

    fn collect_resource_type_ref(
        &mut self,
        file: &Path,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
        ty: &TypeExpr,
    ) {
        self.collect_resource_schema_type_ref(
            file,
            source,
            aliases,
            module,
            ty,
            &SchemaType::resolve(ty),
        );
    }

    fn collect_resource_schema_type_ref(
        &mut self,
        file: &Path,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
        ty: &TypeExpr,
        schema_type: &SchemaType,
    ) {
        match schema_type {
            SchemaType::Sequence(element) | SchemaType::Optional(element) => {
                self.collect_resource_schema_type_ref(file, source, aliases, module, ty, element);
            }
            SchemaType::Identity(_) => {}
            SchemaType::Named(resource) => {
                let segments = split_type_path(resource);
                let resolved_segments = expand_alias(&segments, aliases);
                if let Some(def) =
                    self.module_scope
                        .resolved_resource(self.program, module, &resolved_segments)
                    && let Some(span) = type_ref_path_tail_span(source, ty, &segments, 1)
                {
                    self.use_of(def, file, span, SymbolKind::Resource);
                }
            }
            SchemaType::Scalar(_) | SchemaType::Unknown => {}
        }
    }

    fn finish(self) -> BindingIndex {
        let positions = FileOccurrenceIndex::build_by_file(&self.bindings);
        BindingIndex {
            bindings: self.bindings,
            positions,
        }
    }
}

impl FileOccurrenceIndex {
    /// Sweep each file's inclusive reference intervals into disjoint winner
    /// segments. An end+1 removal event retains inclusive-end cursor behavior
    /// without allocating one entry per source byte.
    fn build_by_file(bindings: &[Binding]) -> HashMap<PathBuf, Self> {
        let mut by_file: HashMap<PathBuf, BTreeMap<usize, OccurrenceEvents>> = HashMap::new();
        let mut ordinal = 0usize;
        for (binding, entry) in bindings.iter().enumerate() {
            for (reference, symbol) in entry.references.iter().enumerate() {
                let span = symbol.span;
                if span.end_byte < span.start_byte {
                    continue;
                }
                let occurrence = ActiveOccurrence {
                    width: span_width(span),
                    ordinal,
                    binding,
                    reference,
                };
                ordinal = ordinal.saturating_add(1);
                let events = by_file.entry(symbol.file.clone()).or_default();
                events
                    .entry(span.start_byte)
                    .or_default()
                    .add
                    .push(occurrence);
                if let Some(after_end) = span.end_byte.checked_add(1) {
                    events.entry(after_end).or_default().remove.push(occurrence);
                }
            }
        }
        by_file
            .into_iter()
            .map(|(file, events)| (file, Self::from_events(events)))
            .collect()
    }

    fn from_events(events: BTreeMap<usize, OccurrenceEvents>) -> Self {
        let mut active: BTreeSet<ActiveOccurrence> = BTreeSet::new();
        let mut segments = Vec::new();
        let mut previous = None;
        for (position, events) in events {
            if let Some(start) = previous
                && start < position
                && let Some(winner) = active.first().copied()
            {
                Self::push_segment(
                    &mut segments,
                    OccurrenceSegment {
                        start,
                        end: position - 1,
                        binding: winner.binding,
                        reference: winner.reference,
                    },
                );
            }
            for occurrence in events.remove {
                active.remove(&occurrence);
            }
            active.extend(events.add);
            previous = Some(position);
        }
        if let Some(start) = previous
            && let Some(winner) = active.first().copied()
        {
            Self::push_segment(
                &mut segments,
                OccurrenceSegment {
                    start,
                    end: usize::MAX,
                    binding: winner.binding,
                    reference: winner.reference,
                },
            );
        }
        Self { segments }
    }

    fn push_segment(segments: &mut Vec<OccurrenceSegment>, next: OccurrenceSegment) {
        if let Some(previous) = segments.last_mut()
            && previous.end.checked_add(1) == Some(next.start)
            && previous.binding == next.binding
            && previous.reference == next.reference
        {
            previous.end = next.end;
            return;
        }
        segments.push(next);
    }

    fn at(&self, offset: usize) -> Option<OccurrenceSegment> {
        let index = self
            .segments
            .partition_point(|segment| segment.start <= offset)
            .checked_sub(1)?;
        let segment = self.segments[index];
        (offset <= segment.end).then_some(segment)
    }
}

fn resource_decl<'a>(parsed: &'a ParsedSource, name: &str) -> Option<&'a ResourceDecl> {
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Resource(resource) if resource.name == name => Some(resource),
            _ => None,
        })
}

/// Walks a function body collecting identifier uses against a scope of local
/// bindings, mirroring the checker's block/statement scope traversal (push a
/// frame per block; record `const`/`var` bindings as they appear; push a
/// loop/catch frame for the body that runs under it) so a local's scope cannot
/// drift from the one the checker builds.
struct UseWalker<'a, 'p> {
    builder: &'a mut IndexBuilder<'p>,
    file: &'a Path,
    source: &'a str,
    module: &'a str,
    aliases: &'a HashMap<String, Vec<String>>,
}

impl UseWalker<'_, '_> {
    /// Walk a block: push a fresh frame, then process each statement in order,
    /// binding the local it introduces (so a later sibling sees it, but its own
    /// initializer does not) before descending.
    fn walk_block(
        &mut self,
        block: &Block,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        scope.push(HashMap::new());
        type_scope.push(HashMap::new());
        for statement in &block.statements {
            self.walk_statement(statement, scope, type_scope);
        }
        scope.pop();
        type_scope.pop();
    }

    fn walk_statement(
        &mut self,
        statement: &Statement,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        match statement {
            Statement::Const {
                name,
                ty,
                value,
                span,
                ..
            } => {
                if let Some(ty) = ty {
                    self.resolve_type_ref(ty);
                }
                // The initializer runs before the binding, so resolve it first.
                self.walk_expr(value, scope, type_scope);
                self.bind_local(statement, name, *span, scope, type_scope);
            }
            Statement::Var {
                name,
                keys,
                ty,
                value,
                span,
                ..
            } => {
                for key in keys {
                    self.resolve_type_ref(&key.ty);
                }
                if let Some(ty) = ty {
                    self.resolve_type_ref(ty);
                }
                if let Some(value) = value {
                    self.walk_expr(value, scope, type_scope);
                }
                self.bind_local(statement, name, *span, scope, type_scope);
            }
            Statement::Assign { target, value, .. }
            | Statement::CompoundAssign { target, value, .. } => {
                self.walk_expr(target, scope, type_scope);
                self.walk_expr(value, scope, type_scope);
            }
            Statement::Delete { path, .. } => self.walk_expr(path, scope, type_scope),
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    self.walk_expr(value, scope, type_scope);
                }
            }
            Statement::Throw { value, .. } | Statement::Expr { value, .. } => {
                self.walk_expr(value, scope, type_scope);
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                self.walk_expr(condition, scope, type_scope);
                self.walk_block(then_block, scope, type_scope);
                for else_if in else_ifs {
                    self.walk_expr(&else_if.condition, scope, type_scope);
                    self.walk_block(&else_if.block, scope, type_scope);
                }
                if let Some(block) = else_block {
                    self.walk_block(block, scope, type_scope);
                }
            }
            Statement::IfConst {
                name,
                ty,
                value,
                then_block,
                else_ifs,
                else_block,
                span,
            } => {
                self.walk_expr(value, scope, type_scope);
                // A written annotation resolves for go-to/hover and types the binding,
                // the same as on `const`/`var`; without one the binding takes the
                // read's inferred type.
                let binding_type = match ty {
                    Some(ty) => {
                        self.resolve_type_ref(ty);
                        resolve_diagnosed_annotation_type(
                            ty,
                            self.builder.program,
                            self.aliases,
                            self.file,
                        )
                    }
                    None => infer_only(
                        self.builder.program,
                        value,
                        type_scope,
                        self.aliases,
                        self.file,
                    ),
                };
                self.walk_if_const_then(*span, name, binding_type, then_block, scope, type_scope);
                for else_if in else_ifs {
                    self.walk_expr(&else_if.condition, scope, type_scope);
                    self.walk_block(&else_if.block, scope, type_scope);
                }
                if let Some(block) = else_block {
                    self.walk_block(block, scope, type_scope);
                }
            }
            Statement::While {
                condition, body, ..
            } => {
                self.walk_expr(condition, scope, type_scope);
                self.walk_block(body, scope, type_scope);
            }
            Statement::For {
                binding,
                iterable,
                body,
                ..
            } => self.walk_for(statement.span(), binding, iterable, body, scope, type_scope),
            Statement::Transaction { body, .. } => self.walk_block(body, scope, type_scope),
            Statement::Try { body, catch, .. } => {
                self.walk_try(body, catch.as_ref(), scope, type_scope)
            }
            Statement::Match {
                scrutinee, arms, ..
            } => self.walk_match(scrutinee, arms, scope, type_scope),
            Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => {}
        }
    }

    /// Bind the local a `const`/`var` introduces: a fresh def id in the value
    /// scope, plus its inferred type in the type scope when one is known. The
    /// caller resolves the initializer first, since it runs before the binding.
    fn bind_local(
        &mut self,
        statement: &Statement,
        name: &str,
        span: SourceSpan,
        scope: &mut [HashMap<String, DefId>],
        type_scope: &mut [HashMap<String, MarrowType>],
    ) {
        let Some(name_span) = first_line_name_span(self.source, span, name) else {
            return;
        };
        let id = self.define_local(name_span);
        bind(scope, name, id);
        if let Some((_, ty)) = local_binding(
            self.builder.program,
            statement,
            type_scope,
            self.aliases,
            self.file,
        ) {
            bind(type_scope, name, ty);
        }
    }

    /// Walk a `for` over its iterable, then its body under a fresh frame holding
    /// the loop binding(s).
    fn walk_for(
        &mut self,
        statement_span: SourceSpan,
        binding: &marrow_syntax::ForBinding,
        iterable: &Expression,
        body: &Block,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        self.walk_expr(iterable, scope, type_scope);
        let mut frame = HashMap::new();
        for name in &binding.names {
            if let Some(span) = for_binding_name_span(self.source, statement_span, &name.name) {
                frame.insert(name.name.clone(), self.define_local(span));
            }
        }
        let type_frame = for_frame(
            self.builder.program,
            binding,
            iterable,
            type_scope,
            self.aliases,
            self.file,
        );
        self.walk_body_in_frame(body, frame, type_frame, scope, type_scope);
    }

    fn walk_if_const_then(
        &mut self,
        statement_span: SourceSpan,
        name: &str,
        binding_type: MarrowType,
        body: &Block,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        let mut frame = HashMap::new();
        if let Some(span) = first_line_name_span(self.source, statement_span, name) {
            frame.insert(name.to_string(), self.define_local(span));
        }
        let mut type_frame = HashMap::new();
        type_frame.insert(name.to_string(), binding_type);
        self.walk_body_in_frame(body, frame, type_frame, scope, type_scope);
    }

    /// Walk a `try`: the body, then the catch block under a frame binding the
    /// caught error. The catch binding is scoped to the catch block only.
    fn walk_try(
        &mut self,
        body: &Block,
        catch: Option<&marrow_syntax::CatchClause>,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        self.walk_block(body, scope, type_scope);
        if let Some(clause) = catch {
            if let Some(ty) = &clause.ty {
                self.resolve_type_ref(ty);
            }
            let mut frame = HashMap::new();
            let type_frame = catch_frame(clause);
            if let Some(span) = catch_binding_name_span(self.source, clause) {
                frame.insert(clause.name.clone(), self.define_local(span));
            }
            self.walk_body_in_frame(&clause.block, frame, type_frame, scope, type_scope);
        }
    }

    /// Walk a block's statements under a freshly pushed value/type frame pair,
    /// popping both afterward so the push/pop can never mismatch.
    fn walk_body_in_frame(
        &mut self,
        body: &Block,
        frame: HashMap<String, DefId>,
        type_frame: HashMap<String, MarrowType>,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        scope.push(frame);
        type_scope.push(type_frame);
        for inner in &body.statements {
            self.walk_statement(inner, scope, type_scope);
        }
        scope.pop();
        type_scope.pop();
    }

    /// Walk a `match`: the scrutinee, then resolve each arm's member path against
    /// the scrutinee's enum, then each arm body under its own block frame.
    fn walk_match(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        self.walk_expr(scrutinee, scope, type_scope);
        let enum_identity = self.match_scrutinee_enum(scrutinee, type_scope);
        if let Some((enum_module, enum_name)) = enum_identity {
            for arm in arms {
                self.resolve_match_arm(&enum_module, &enum_name, arm);
            }
        }
        for arm in arms {
            self.walk_block(&arm.block, scope, type_scope);
        }
    }

    /// Define a function-local binding at its identifier token.
    fn define_local(&mut self, span: SourceSpan) -> DefId {
        self.builder.define(
            SymbolRef {
                file: self.file.to_path_buf(),
                span,
                kind: SymbolKind::Local,
            },
            RenameSafety::SourceOnly,
        )
    }

    fn resolve_type_ref(&mut self, ty: &TypeExpr) {
        self.builder
            .collect_type_ref(self.file, self.source, self.aliases, self.module, ty);
    }

    /// Walk an expression, attributing each name/call/saved read to its definition.
    fn walk_expr(
        &mut self,
        expr: &Expression,
        scope: &[HashMap<String, DefId>],
        type_scope: &[HashMap<String, MarrowType>],
    ) {
        match expr {
            Expression::Name { segments, span, .. } => {
                self.resolve_name(segments, *span, scope);
            }
            // A bare saved root resolves only as part of a larger saved-path
            // expression; resolution happens in the enclosing Call/Field arm
            // where the chain is known. `absent` is a leaf primary value with no
            // name to attribute.
            Expression::SavedRoot { .. } | Expression::Absent { .. } => {}
            Expression::Unary { operand, .. } => self.walk_expr(operand, scope, type_scope),
            Expression::Binary { left, right, .. } => {
                self.walk_expr(left, scope, type_scope);
                self.walk_expr(right, scope, type_scope);
            }
            Expression::Range {
                start, end, step, ..
            } => {
                for part in [start.as_deref(), end.as_deref(), step.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    self.walk_expr(part, scope, type_scope);
                }
            }
            Expression::Call { callee, args, .. } => {
                // A saved keyed/record read is call-shaped; resolve its saved
                // member before treating the callee as a function or value.
                if !self.resolve_saved_path(expr, type_scope) && !self.resolve_call(callee) {
                    self.walk_expr(callee, scope, type_scope);
                }
                for arg in args {
                    self.walk_expr(&arg.value, scope, type_scope);
                }
            }
            Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
                // Resolve as a saved path if applicable; otherwise descend into
                // the base.
                if !self.resolve_saved_path(expr, type_scope) {
                    self.walk_expr(base, scope, type_scope);
                }
            }
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let marrow_syntax::InterpolationPart::Expr(inner) = part {
                        self.walk_expr(inner, scope, type_scope);
                    }
                }
            }
            Expression::Literal { .. } | Expression::Error { .. } => {}
        }
    }

    /// Resolve a bare or qualified name used as a value: a local/parameter
    /// (innermost scope first, shadowing-aware), a module alias, a module constant,
    /// or a resource. Records the use when it resolves.
    fn resolve_name(
        &mut self,
        segments: &[String],
        span: SourceSpan,
        scope: &[HashMap<String, DefId>],
    ) {
        self.resolve_module_alias_use(segments, span);
        if let [name] = segments {
            // A single-segment name is a local, parameter, module constant,
            // resource, or imported alias.
            if let Some(id) = lookup(scope, name) {
                let kind = self.builder.bindings[id.0].definition.kind;
                self.builder.use_of(id, self.file, span, kind);
                return;
            }
            if let Some(id) = self.builder.module_scope.constant(self.module, name) {
                self.builder
                    .use_of(id, self.file, span, SymbolKind::ModuleConst);
                return;
            }
            if let Some(id) = self.builder.module_scope.resolved_resource(
                self.builder.program,
                self.module,
                segments,
            ) {
                self.builder
                    .use_of(id, self.file, span, SymbolKind::Resource);
            }
            return;
        }
        if let Some(resolved) = self.resolve_enum_member(segments) {
            let segment_spans = qualified_segment_spans(self.source, span, segments);
            if let Some(enum_span) = segment_spans
                .as_ref()
                .and_then(|spans| spans.get(resolved.enum_index))
                .copied()
            {
                self.builder
                    .use_of(resolved.enum_def, self.file, enum_span, SymbolKind::Enum);
            }
            for (member_index, member_def) in resolved.member_defs {
                if let Some(member_span) = segment_spans
                    .as_ref()
                    .and_then(|spans| spans.get(member_index))
                    .copied()
                {
                    self.builder
                        .use_of(member_def, self.file, member_span, SymbolKind::EnumMember);
                }
            }
        }
    }

    fn resolve_enum_member(&self, segments: &[String]) -> Option<ResolvedEnumMemberUse> {
        let expr = Expression::Name {
            segments: segments.to_vec(),
            segment_spans: Vec::new(),
            span: SourceSpan::default(),
        };
        let EnumMemberPathResolution::Resolved(resolved) =
            resolve_enum_member_path(self.builder.program, &expr, self.aliases, self.file)
        else {
            return None;
        };
        if resolved.private.is_some() {
            return None;
        }
        let marrow_schema::MemberPathResolution::Found(ordinal) = resolved.member else {
            return None;
        };
        let canonical_path = resolved
            .schema
            .member_path(ordinal)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let enum_index = resolved.enum_index;
        let enum_def = self
            .builder
            .module_scope
            .enums
            .get(&(resolved.module.clone(), resolved.enum_name.clone()))
            .copied()?;
        let mut member_defs = Vec::new();
        let member_start = enum_index + 1;
        let written_members = &segments[member_start..];
        let written_is_canonical = written_members.len() == canonical_path.len()
            && written_members
                .iter()
                .map(String::as_str)
                .eq(canonical_path.iter().map(String::as_str));

        if written_is_canonical {
            for prefix_len in 1..=canonical_path.len() {
                let member_index = member_start + prefix_len - 1;
                if let Some(def) = self.enum_member_def(
                    &resolved.module,
                    &resolved.enum_name,
                    &canonical_path[..prefix_len],
                ) {
                    member_defs.push((member_index, def));
                }
            }
        } else if let Some(def) =
            self.enum_member_def(&resolved.module, &resolved.enum_name, &canonical_path)
        {
            member_defs.push((segments.len() - 1, def));
        }
        if member_defs.is_empty() {
            return None;
        }
        Some(ResolvedEnumMemberUse {
            enum_def,
            enum_index,
            member_defs,
        })
    }

    fn enum_member_def(&self, module: &str, enum_name: &str, path: &[String]) -> Option<DefId> {
        self.builder
            .module_scope
            .enum_members
            .get(&(module.to_string(), enum_name.to_string(), path.to_vec()))
            .copied()
    }

    fn match_scrutinee_enum(
        &self,
        scrutinee: &Expression,
        type_scope: &[HashMap<String, MarrowType>],
    ) -> Option<(String, String)> {
        match infer_only(
            self.builder.program,
            scrutinee,
            type_scope,
            self.aliases,
            self.file,
        ) {
            MarrowType::Enum(id) => self
                .builder
                .program
                .enum_by_id(id)
                .map(|(module, name)| (module.to_string(), name.to_string())),
            _ => None,
        }
    }

    fn resolve_match_arm(&mut self, enum_module: &str, enum_name: &str, arm: &MatchArm) {
        let Some(schema) = enum_schema_in(self.builder.program, enum_module, enum_name) else {
            return;
        };
        let segments: Vec<&str> = arm.path.iter().map(String::as_str).collect();
        let marrow_schema::MemberPathResolution::Found(ordinal) =
            schema.walk_member_path(&segments)
        else {
            return;
        };
        let path = schema
            .member_path(ordinal)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let Some(def) = self
            .builder
            .module_scope
            .enum_members
            .get(&(enum_module.to_string(), enum_name.to_string(), path))
            .copied()
        else {
            return;
        };
        self.builder.use_of(
            def,
            self.file,
            match_arm_header_span(arm),
            SymbolKind::EnumMember,
        );
    }

    /// Resolve a call whose callee is a name to a function definition, expanding
    /// short-form `use` aliases first (`books::add` -> `shelf::books::add`).
    /// Returns whether the call resolved to a function (so the caller does not also
    /// treat the callee as a value).
    fn resolve_call(&mut self, callee: &Expression) -> bool {
        let Expression::Name {
            segments,
            span: callee_span,
            ..
        } = callee
        else {
            return false;
        };
        self.resolve_module_alias_use(segments, *callee_span);
        // Constructors are call-shaped but name resources, not functions. Resolve
        // them with the same module-aware resource resolver used by the checker.
        let resolved_segments = expand_alias(segments, self.aliases);
        if let Some(def) = self.builder.module_scope.resolved_resource(
            self.builder.program,
            self.module,
            &resolved_segments,
        ) {
            if let Some(use_span) = path_tail_span(self.source, *callee_span, segments, 1) {
                self.builder
                    .use_of(def, self.file, use_span, SymbolKind::Resource);
            }
            return true;
        }
        // Marrow has no first-class functions, so a bare name in callee position
        // resolves as a function (not a value). Resolve through the same unified
        // resolver the checker's `check_call` uses, from this file's module, so a
        // bare go-to-def lands on the calling module's own function — never a
        // foreign one — and a private cross-module call resolves to nothing.
        if let Some((module, function)) =
            resolve_function_in_module(self.builder.program, self.module, segments)
            && let Some(def) = self
                .builder
                .module_scope
                .functions
                .get(&(module.name.clone(), function.name.clone()))
                .copied()
        {
            let Some(span) = path_tail_span(self.source, *callee_span, segments, 1) else {
                return true;
            };
            self.builder
                .use_of(def, self.file, span, SymbolKind::Function);
            return true;
        }
        false
    }

    fn resolve_module_alias_use(&mut self, segments: &[String], span: SourceSpan) {
        self.builder
            .collect_module_alias_use(self.file, self.source, span, segments);
    }

    fn resolve_saved_path(
        &mut self,
        expr: &Expression,
        type_scope: &[HashMap<String, MarrowType>],
    ) -> bool {
        let Some(checked) = lower_expr_for_file(self.builder.program, self.file, expr, type_scope)
        else {
            return false;
        };
        let Some(reference) =
            SavedPlaceResolver::new(self.builder.program).binding_member_ref(&checked)
        else {
            return false;
        };
        let kind = match reference.kind {
            SavedMemberRefKind::Field => SymbolKind::Field,
            SavedMemberRefKind::Layer => SymbolKind::Layer,
            SavedMemberRefKind::Index => SymbolKind::Index,
        };
        let chain = reference.chain.clone();
        if let Some(def) = self
            .builder
            .module_scope
            .saved_members
            .get(&(reference.root.clone(), chain.clone()))
            .copied()
        {
            let Some(name) = reference.chain.last() else {
                return true;
            };
            let Some(span) = last_name_span(self.source, reference.span, name) else {
                return true;
            };
            self.builder.use_of(def, self.file, span, kind);
            return true;
        }
        false
    }
}

impl ModuleScope {
    /// A module constant visible to a file in `module`. Bare constant references
    /// resolve within the file's own module.
    fn constant(&self, module: &str, name: &str) -> Option<DefId> {
        self.constants
            .get(module)
            .and_then(|consts| consts.get(name))
            .copied()
    }

    fn resolved_resource(
        &self,
        program: &CheckedProgram,
        from_module: &str,
        segments: &[String],
    ) -> Option<DefId> {
        let Resolution::Found(Def {
            module,
            item: DefItem::Resource(resource),
            ..
        }) = resolve(program, from_module, segments, ResolvableKind::Resource)
        else {
            return None;
        };
        let key = (module.name.clone(), resource.name.clone());
        self.resources.get(&key).copied()
    }
}

/// Look up a name in a scope stack, innermost frame first, so an inner binding
/// shadows an outer one of the same name.
fn lookup(scope: &[HashMap<String, DefId>], name: &str) -> Option<DefId> {
    scope
        .iter()
        .rev()
        .find_map(|frame| frame.get(name))
        .copied()
}

/// Record `name`'s binding in the innermost scope frame.
fn bind<V>(scope: &mut [HashMap<String, V>], name: &str, value: V) {
    if let Some(frame) = scope.last_mut() {
        frame.insert(name.to_string(), value);
    }
}

fn type_ref_path_tail_span(
    source: &str,
    ty: &TypeExpr,
    segments: &[String],
    tail_segments: usize,
) -> Option<SourceSpan> {
    path_tail_span(source, ty.span(), segments, tail_segments)
}

fn path_tail_span(
    source: &str,
    span: SourceSpan,
    segments: &[String],
    tail_segments: usize,
) -> Option<SourceSpan> {
    let segment_spans = qualified_segment_spans(source, span, segments)?;
    let start_index = segments.len().checked_sub(tail_segments)?;
    let start_byte = segment_spans.get(start_index)?.start_byte;
    let end_byte = segment_spans.last()?.end_byte;
    Some(source_span_at(source, start_byte, end_byte))
}

fn qualified_segment_spans(
    source: &str,
    span: SourceSpan,
    segments: &[String],
) -> Option<Vec<SourceSpan>> {
    let end_byte = span.end_byte.saturating_add(1).min(source.len());
    if span.start_byte > end_byte {
        return None;
    }
    let mut search_start = span.start_byte;
    let mut spans = Vec::with_capacity(segments.len());
    for segment in segments {
        let text = source.get(search_start..end_byte)?;
        let offset = text.find(segment)?;
        let start_byte = search_start + offset;
        let end_byte = start_byte + segment.len();
        spans.push(source_span_at(source, start_byte, end_byte));
        search_start = end_byte;
    }
    Some(spans)
}

fn param_name_span(
    source: &str,
    tokens: &[marrow_syntax::Token],
    function_span: SourceSpan,
    param: &ParamDecl,
) -> Option<SourceSpan> {
    let mut found = None;
    for token in tokens {
        if token.span.start_byte < function_span.start_byte
            || token.span.end_byte > param.ty.span().start_byte
        {
            continue;
        }
        if token.kind == TokenKind::Identifier && token.text(source) == param.name {
            found = Some(token.span);
        }
    }
    found
}

fn for_binding_name_span(
    source: &str,
    statement_span: SourceSpan,
    name: &str,
) -> Option<SourceSpan> {
    first_line_name_span(source, statement_span, name)
}

fn catch_binding_name_span(
    source: &str,
    clause: &marrow_syntax::CatchClause,
) -> Option<SourceSpan> {
    let end = clause.block.span.start_byte.min(source.len());
    let before_block = source.get(..end)?;
    let catch_start = before_block.rfind("catch")?;
    let span = first_line_span(source, source_span_at(source, catch_start, end))?;
    name_span(source, span, &clause.name)
}

fn first_line_name_span(source: &str, span: SourceSpan, name: &str) -> Option<SourceSpan> {
    let span = first_line_span(source, span)?;
    name_span(source, span, name)
}

fn first_line_span(source: &str, span: SourceSpan) -> Option<SourceSpan> {
    let start = span.start_byte.min(source.len());
    let end = span.end_byte.saturating_add(1).min(source.len());
    if start > end {
        return None;
    }
    let text = source.get(start..end)?;
    let line_end = start + text.find('\n').unwrap_or(text.len());
    Some(source_span_at(source, start, line_end))
}

fn name_span(source: &str, span: SourceSpan, name: &str) -> Option<SourceSpan> {
    identifier_span_in(source, span, name)
}

fn last_name_span(source: &str, span: SourceSpan, name: &str) -> Option<SourceSpan> {
    last_identifier_span_in(source, span, name)
}

fn match_arm_header_span(arm: &MatchArm) -> SourceSpan {
    let label_len = arm.path.join("::").len();
    // `span_covers` end is inclusive, so a label of length N ends at S + N - 1;
    // an exclusive end would let the byte just past the label resolve as a member.
    SourceSpan {
        start_byte: arm.span.start_byte,
        end_byte: arm
            .span
            .start_byte
            .saturating_add(label_len.saturating_sub(1)),
        line: arm.span.line,
        column: arm.span.column,
    }
}

fn span_width(span: SourceSpan) -> usize {
    span.end_byte.saturating_sub(span.start_byte)
}

fn is_valid_source_rename(name: &str) -> bool {
    let lexed = marrow_syntax::lex_source(name);
    if !lexed.diagnostics.is_empty() {
        return false;
    }

    let mut tokens = lexed
        .tokens
        .iter()
        .filter(|token| token.kind != TokenKind::Eof);
    let Some(token) = tokens.next() else {
        return false;
    };

    tokens.next().is_none()
        && token.kind == TokenKind::Identifier
        && token.span.start_byte == 0
        && token.span.end_byte == name.len()
}

fn renamed_evolve_path(path: &str, new_name: &str) -> String {
    if let Some((prefix, _)) = path.rsplit_once('.') {
        return format!("{prefix}.{new_name}");
    }
    if path.starts_with('^') {
        return format!("^{new_name}");
    }
    new_name.to_string()
}

fn canonical_evolve_rename(from: &str, to: &str) -> Option<String> {
    let source = format!("module __rename_action\n\nevolve\n    rename {from} -> {to}\n");
    let formatted = marrow_syntax::format_source(&source);
    formatted
        .find("evolve\n")
        .map(|start| formatted[start..].to_string())
}

#[cfg(test)]
mod occurrence_index_tests {
    use super::*;

    fn symbol(file: &Path, start: usize, end: usize, kind: SymbolKind) -> SymbolRef {
        SymbolRef {
            file: file.to_path_buf(),
            span: SourceSpan {
                start_byte: start,
                end_byte: end,
                line: 1,
                column: start as u32 + 1,
            },
            kind,
        }
    }

    fn binding(definition: SymbolRef, references: Vec<SymbolRef>) -> Binding {
        Binding {
            definition,
            references,
            safety: RenameSafety::SourceOnly,
            rename_target: None,
            parameter: None,
        }
    }

    #[test]
    fn occurrence_segments_preserve_tightest_first_and_inclusive_lookup() {
        let file = Path::new("src/m.mw");
        let broad = symbol(file, 2, 12, SymbolKind::Function);
        let first_equal = symbol(file, 5, 8, SymbolKind::Local);
        let second_equal = symbol(file, 5, 8, SymbolKind::Param);
        let point = symbol(file, 7, 7, SymbolKind::Field);
        let bindings = vec![
            binding(broad.clone(), vec![broad]),
            binding(first_equal.clone(), vec![first_equal]),
            binding(second_equal.clone(), vec![second_equal]),
            binding(point.clone(), vec![point]),
        ];
        let index = BindingIndex {
            positions: FileOccurrenceIndex::build_by_file(&bindings),
            bindings,
        };

        assert!(index.binding_reference_at(file, 1).is_none());
        assert_eq!(
            index.binding_reference_at(file, 2).unwrap().1.kind,
            SymbolKind::Function,
        );
        assert_eq!(
            index.binding_reference_at(file, 5).unwrap().1.kind,
            SymbolKind::Local,
        );
        assert_eq!(
            index.binding_reference_at(file, 7).unwrap().1.kind,
            SymbolKind::Field,
        );
        assert_eq!(
            index.binding_reference_at(file, 8).unwrap().1.kind,
            SymbolKind::Local,
        );
        assert_eq!(
            index.binding_reference_at(file, 12).unwrap().1.kind,
            SymbolKind::Function,
        );
        assert!(index.binding_reference_at(file, 13).is_none());
    }
}
