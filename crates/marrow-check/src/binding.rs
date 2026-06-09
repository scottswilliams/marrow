//! The project-wide binding index: every identifier *use* resolved to the
//! *definition* it names, respecting lexical scope (shadowing) and `use` import
//! aliases, plus a rename-safety classification. It is the resolution layer the
//! editor tooling (go-to-definition, find-all-references, rename) builds on. It
//! does not reimplement name resolution: it reuses the checker's unified `resolve`
//! (via `resolve_function_in_module`) for module-aware function calls,
//! `find_resource_schema` plus the schema's field/layer/index lookup for saved
//! data, and the same block traversal shape the checker's scope lowering uses (a
//! frame per block, with loop and catch bindings scoped to their body) so a
//! local's scope cannot drift from the one the checker builds.
//!
//! A single-segment name resolves at the identifier token; a qualified call at
//! the whole path span; an enum member literal is split so the enum segment and
//! each written member-path segment resolve independently. A parameter, loop, or
//! catch binding has no node span of its own, so its definition is recorded at
//! the enclosing declaration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use marrow_schema::Type as SchemaType;
use marrow_syntax::{
    Block, Declaration, EnumMember, Expression, MatchArm, ParsedSource, ResourceDecl,
    ResourceMember, SourceSpan, Statement, StoreDecl, TypeRef,
};

use crate::MarrowType;
use crate::analysis::span_covers;
use crate::checks::{file_prelude, for_frame};
use crate::enums::{enum_schema_in, resolve_enum_member_path, resolve_type};
use crate::infer::{infer_only, local_binding};
use crate::{
    AnalysisSnapshot, CheckedProgram, Def, DefItem, Resolution, ResolvableKind, expand_alias,
    find_resource_schema, resolve, resolve_function_in_module, split_type_path,
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

/// Whether renaming a symbol is safe to do purely in source.
///
/// `SourceOnly` symbols (locals, parameters, constants, functions, module
/// aliases, and resources with no saved root) exist only in source, so a rename
/// that updates every reference is complete. `SavedDataBacked` symbols name
/// something encoded on disk — a saved field/group/layer/index, or a resource
/// attached to a saved root. Renaming such a symbol requires catalog-backed
/// evolution intent rather than a source-only edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameSafety {
    /// Safe to rename in source alone.
    SourceOnly,
    /// Names data encoded on disk; source-only rename is not enough.
    SavedDataBacked,
}

/// A definition together with every reference to it (the definition included), as
/// the index records them internally before answering queries.
#[derive(Debug, Clone)]
struct Binding {
    definition: SymbolRef,
    references: Vec<SymbolRef>,
    safety: RenameSafety,
}

/// A resolved project's binding index: definitions, their references, and
/// rename-safety, keyed for position lookup. Build it with
/// [`build_binding_index`].
#[derive(Debug, Clone, Default)]
pub struct BindingIndex {
    bindings: Vec<Binding>,
}

/// Build the binding index for an analyzed project. Walks every clean module's
/// declarations to collect definitions, then every function body and saved-path
/// expression to collect uses, resolving each use to its definition through the
/// same scope, alias, and schema logic the checker uses.
pub fn build_binding_index(snapshot: &AnalysisSnapshot) -> BindingIndex {
    let mut builder = IndexBuilder::new(&snapshot.program);
    // Definitions come from the parsed files, which carry source spans the
    // resolved program does not.
    for file in &snapshot.files {
        builder.collect_definitions(&file.path, &file.parsed);
    }
    // Uses are resolved against the definitions, so collect them in a second pass.
    for file in &snapshot.files {
        builder.collect_uses(&file.path, &file.parsed, &file.source);
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

    /// The binding whose definition span or any reference span covers `offset` in
    /// `file`. The tightest covering span wins, so a use nested inside a wider
    /// declaration resolves to the use's symbol rather than the enclosing one.
    fn binding_at(&self, file: &Path, offset: usize) -> Option<&Binding> {
        let mut best: Option<(&Binding, usize)> = None;
        for binding in &self.bindings {
            for reference in &binding.references {
                if reference.file == file && span_covers(reference.span, offset) {
                    let width = span_width(reference.span);
                    if best.is_none_or(|(_, current)| width < current) {
                        best = Some((binding, width));
                    }
                }
            }
        }
        best.map(|(binding, _)| binding)
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
            module_scope: ModuleScope::default(),
        }
    }

    /// Record a definition, returning its id. The definition is its own first
    /// reference.
    fn define(&mut self, def: SymbolRef, safety: RenameSafety) -> DefId {
        let id = DefId(self.bindings.len());
        self.bindings.push(Binding {
            references: vec![def.clone()],
            definition: def,
            safety,
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
    fn collect_definitions(&mut self, file: &Path, parsed: &ParsedSource) {
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
        // reference. The whole `use` path is the definition span.
        for use_decl in &parsed.file.uses {
            self.define(
                SymbolRef {
                    file: file.to_path_buf(),
                    span: use_decl.span,
                    kind: SymbolKind::ModuleRef,
                },
                RenameSafety::SourceOnly,
            );
        }

        for declaration in &parsed.file.declarations {
            match declaration {
                Declaration::Const(constant) => {
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span: constant.span,
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
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span: function.span,
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
                    self.collect_resource(file, &module, resource, has_store);
                }
                Declaration::Store(store) => {
                    if let Some(resource) = resource_decl(parsed, &store.resource) {
                        self.collect_store(file, store, resource);
                    }
                }
                Declaration::Enum(enum_decl) => {
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span: enum_decl.span,
                            kind: SymbolKind::Enum,
                        },
                        RenameSafety::SourceOnly,
                    );
                    self.module_scope
                        .enums
                        .insert((module.clone(), enum_decl.name.clone()), id);
                    self.collect_enum_members(
                        file,
                        &module,
                        &enum_decl.name,
                        &mut Vec::new(),
                        &enum_decl.members,
                    );
                }
                // Evolve blocks declare catalog intent, not source symbols.
                Declaration::Evolve(_) => {}
            }
        }
    }

    fn collect_enum_members(
        &mut self,
        file: &Path,
        module: &str,
        enum_name: &str,
        path: &mut Vec<String>,
        members: &[EnumMember],
    ) {
        for member in members {
            path.push(member.name.clone());
            let id = self.define(
                SymbolRef {
                    file: file.to_path_buf(),
                    span: member.span,
                    kind: SymbolKind::EnumMember,
                },
                RenameSafety::SourceOnly,
            );
            self.module_scope.enum_members.insert(
                (module.to_string(), enum_name.to_string(), path.clone()),
                id,
            );
            self.collect_enum_members(file, module, enum_name, path, &member.members);
            path.pop();
        }
    }

    /// Collect a resource definition, data-backed exactly when a store references
    /// it. Keyed by declaring module plus source name; use sites pick the module
    /// through the checker resolver.
    fn collect_resource(
        &mut self,
        file: &Path,
        module: &str,
        resource: &ResourceDecl,
        has_store: bool,
    ) {
        let safety = if has_store {
            RenameSafety::SavedDataBacked
        } else {
            RenameSafety::SourceOnly
        };
        let resource_id = self.define(
            SymbolRef {
                file: file.to_path_buf(),
                span: resource.span,
                kind: SymbolKind::Resource,
            },
            safety,
        );
        self.module_scope
            .resources
            .insert((module.to_string(), resource.name.clone()), resource_id);
    }

    fn collect_store(&mut self, file: &Path, store: &StoreDecl, resource: &ResourceDecl) {
        self.collect_members(file, &store.root.root, &mut Vec::new(), &resource.members);
        for index in &store.indexes {
            self.collect_index(file, &store.root.root, index);
        }
    }

    fn collect_index(&mut self, file: &Path, root: &str, index: &marrow_syntax::IndexDecl) {
        let id = self.define(
            SymbolRef {
                file: file.to_path_buf(),
                span: index.span,
                kind: SymbolKind::Index,
            },
            RenameSafety::SavedDataBacked,
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
        root: &str,
        chain: &mut Vec<String>,
        members: &[ResourceMember],
    ) {
        for member in members {
            match member {
                ResourceMember::Field(field) => {
                    chain.push(field.name.clone());
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span: field.span,
                            kind: SymbolKind::Field,
                        },
                        RenameSafety::SavedDataBacked,
                    );
                    self.module_scope
                        .saved_members
                        .insert((root.to_string(), chain.clone()), id);
                    chain.pop();
                }
                ResourceMember::Group(group) => {
                    chain.push(group.name.clone());
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span: group.span,
                            kind: SymbolKind::Layer,
                        },
                        RenameSafety::SavedDataBacked,
                    );
                    self.module_scope
                        .saved_members
                        .insert((root.to_string(), chain.clone()), id);
                    self.collect_members(file, root, chain, &group.members);
                    chain.pop();
                }
            }
        }
    }

    /// Collect the uses in a file: the `use` alias references, the saved-path reads
    /// in every function body, and the bare-name/call uses resolved against
    /// reconstructed lexical scope.
    fn collect_uses(&mut self, file: &Path, parsed: &ParsedSource, source: &str) {
        let module = Self::module_name(parsed);
        let prelude = file_prelude(self.program, file, parsed);

        for declaration in &parsed.file.declarations {
            self.collect_type_refs(file, declaration, source, &prelude.aliases, &module);
            if let Declaration::Function(function) = declaration {
                // Parameters have no AST span, so each is defined at the function
                // span and tracked by name.
                let mut scope: Vec<HashMap<String, DefId>> = vec![HashMap::new()];
                let mut type_scope: Vec<HashMap<String, MarrowType>> =
                    vec![prelude.module_constants.clone()];
                for param in &function.params {
                    let id = self.define(
                        SymbolRef {
                            file: file.to_path_buf(),
                            span: function.span,
                            kind: SymbolKind::Param,
                        },
                        RenameSafety::SourceOnly,
                    );
                    scope[0].insert(param.name.clone(), id);
                    type_scope[0].insert(
                        param.name.clone(),
                        resolve_type(&param.ty, self.program, &prelude.aliases, file),
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

    fn collect_type_refs(
        &mut self,
        file: &Path,
        declaration: &Declaration,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
    ) {
        match declaration {
            Declaration::Const(constant) => {
                if let Some(ty) = &constant.ty {
                    self.collect_type_ref(file, source, aliases, module, ty);
                }
            }
            Declaration::Resource(resource) => {
                self.collect_resource_member_type_refs(
                    file,
                    source,
                    aliases,
                    module,
                    &resource.members,
                );
            }
            Declaration::Store(store) => {
                self.collect_key_type_refs(file, source, aliases, module, &store.root.keys);
            }
            Declaration::Function(function) => {
                for param in &function.params {
                    self.collect_type_ref(file, source, aliases, module, &param.ty);
                }
                if let Some(ty) = &function.return_type {
                    self.collect_type_ref(file, source, aliases, module, ty);
                }
            }
            Declaration::Enum(_) | Declaration::Evolve(_) => {}
        }
    }

    fn collect_resource_member_type_refs(
        &mut self,
        file: &Path,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
        members: &[ResourceMember],
    ) {
        for member in members {
            match member {
                ResourceMember::Field(field) => {
                    self.collect_key_type_refs(file, source, aliases, module, &field.keys);
                    self.collect_type_ref(file, source, aliases, module, &field.ty);
                }
                ResourceMember::Group(group) => {
                    self.collect_key_type_refs(file, source, aliases, module, &group.keys);
                    self.collect_resource_member_type_refs(
                        file,
                        source,
                        aliases,
                        module,
                        &group.members,
                    );
                }
            }
        }
    }

    fn collect_key_type_refs(
        &mut self,
        file: &Path,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
        keys: &[marrow_syntax::KeyParam],
    ) {
        for key in keys {
            self.collect_type_ref(file, source, aliases, module, &key.ty);
        }
    }

    fn collect_type_ref(
        &mut self,
        file: &Path,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
        ty: &TypeRef,
    ) {
        let resolved = resolve_type(ty, self.program, aliases, file);
        if let Some((enum_module, enum_name)) = enum_identity(&resolved) {
            let Some(def) = self
                .module_scope
                .enums
                .get(&(enum_module.to_string(), enum_name.to_string()))
                .copied()
            else {
                return;
            };
            if let Some(span) = type_ref_enum_leaf_span(source, ty, enum_name) {
                self.use_of(def, file, span, SymbolKind::Enum);
            }
            return;
        }
        self.collect_resource_type_ref(file, source, aliases, module, ty);
    }

    fn collect_resource_type_ref(
        &mut self,
        file: &Path,
        source: &str,
        aliases: &HashMap<String, Vec<String>>,
        module: &str,
        ty: &TypeRef,
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
        ty: &TypeRef,
        schema_type: &SchemaType,
    ) {
        match schema_type {
            SchemaType::Sequence(element) => {
                self.collect_resource_schema_type_ref(file, source, aliases, module, ty, element);
            }
            SchemaType::Identity(_) => {}
            SchemaType::Named(resource) => {
                let segments = split_type_path(resource);
                let resolved_segments = expand_alias(&segments, aliases);
                if let Some(def) = self.module_scope.resolved_resource(
                    self.program,
                    module,
                    &resolved_segments,
                    ResolvableKind::Resource,
                ) && let Some(span) = type_ref_path_tail_span(source, ty, &segments, 1)
                {
                    self.use_of(def, file, span, SymbolKind::Resource);
                }
            }
            SchemaType::Scalar(_) | SchemaType::Unknown => {}
        }
    }

    fn finish(self) -> BindingIndex {
        BindingIndex {
            bindings: self.bindings,
        }
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
                self.walk_expr(value, scope);
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
                    self.walk_expr(value, scope);
                }
                self.bind_local(statement, name, *span, scope, type_scope);
            }
            Statement::Assign { target, value, .. } => {
                self.walk_expr(target, scope);
                self.walk_expr(value, scope);
            }
            Statement::Delete { path, .. } => self.walk_expr(path, scope),
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    self.walk_expr(value, scope);
                }
            }
            Statement::Throw { value, .. } | Statement::Expr { value, .. } => {
                self.walk_expr(value, scope);
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                if let Some(condition) = condition {
                    self.walk_expr(condition, scope);
                }
                self.walk_block(then_block, scope, type_scope);
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        self.walk_expr(condition, scope);
                    }
                    self.walk_block(&else_if.block, scope, type_scope);
                }
                if let Some(block) = else_block {
                    self.walk_block(block, scope, type_scope);
                }
            }
            Statement::While {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    self.walk_expr(condition, scope);
                }
                self.walk_block(body, scope, type_scope);
            }
            Statement::For {
                binding,
                iterable,
                body,
                ..
            } => self.walk_for(statement.span(), binding, iterable, body, scope, type_scope),
            Statement::Transaction { body, .. } => self.walk_block(body, scope, type_scope),
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => self.walk_try(body, catch.as_ref(), finally.as_ref(), scope, type_scope),
            Statement::Match {
                scrutinee, arms, ..
            } => self.walk_match(scrutinee.as_ref(), arms, scope, type_scope),
            Statement::Break { .. } | Statement::Continue { .. } => {}
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
        let id = self.define_local(span);
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
    /// the loop binding(s). The loop variables have no AST span of their own, so
    /// each is defined at the statement span.
    fn walk_for(
        &mut self,
        statement_span: SourceSpan,
        binding: &marrow_syntax::ForBinding,
        iterable: &Expression,
        body: &Block,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        self.walk_expr(iterable, scope);
        let mut frame = HashMap::new();
        frame.insert(binding.first.clone(), self.define_local(statement_span));
        if let Some(second) = &binding.second {
            frame.insert(second.clone(), self.define_local(statement_span));
        }
        let type_frame = for_frame(
            self.builder.program,
            binding,
            iterable,
            type_scope,
            self.aliases,
            self.file,
        );
        scope.push(frame);
        type_scope.push(type_frame);
        for inner in &body.statements {
            self.walk_statement(inner, scope, type_scope);
        }
        scope.pop();
        type_scope.pop();
    }

    /// Walk a `try`: the body, then the catch block under a frame binding the
    /// caught error, then the finally block. The catch binding is scoped to the
    /// catch block only.
    fn walk_try(
        &mut self,
        body: &Block,
        catch: Option<&marrow_syntax::CatchClause>,
        finally: Option<&Block>,
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        self.walk_block(body, scope, type_scope);
        if let Some(clause) = catch {
            if let Some(ty) = &clause.ty {
                self.resolve_type_ref(ty);
            }
            let mut frame = HashMap::new();
            let mut type_frame = HashMap::new();
            frame.insert(clause.name.clone(), self.define_local(clause.block.span));
            type_frame.insert(clause.name.clone(), MarrowType::Error);
            scope.push(frame);
            type_scope.push(type_frame);
            for inner in &clause.block.statements {
                self.walk_statement(inner, scope, type_scope);
            }
            scope.pop();
            type_scope.pop();
        }
        if let Some(finally) = finally {
            self.walk_block(finally, scope, type_scope);
        }
    }

    /// Walk a `match`: the scrutinee, then resolve each arm's member path against
    /// the scrutinee's enum, then each arm body under its own block frame.
    fn walk_match(
        &mut self,
        scrutinee: Option<&Expression>,
        arms: &[MatchArm],
        scope: &mut Vec<HashMap<String, DefId>>,
        type_scope: &mut Vec<HashMap<String, MarrowType>>,
    ) {
        if let Some(scrutinee) = scrutinee {
            self.walk_expr(scrutinee, scope);
        }
        let enum_identity =
            scrutinee.and_then(|scrutinee| self.match_scrutinee_enum(scrutinee, type_scope));
        if let Some((enum_module, enum_name)) = enum_identity {
            for arm in arms {
                self.resolve_match_arm(&enum_module, &enum_name, arm);
            }
        }
        for arm in arms {
            self.walk_block(&arm.block, scope, type_scope);
        }
    }

    /// Define a function-local binding (a `const`/`var`, loop, or catch binding) at
    /// `span` and return its id.
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

    fn resolve_type_ref(&mut self, ty: &TypeRef) {
        self.builder
            .collect_type_ref(self.file, self.source, self.aliases, self.module, ty);
    }

    /// Walk an expression, attributing each name/call/saved read to its definition.
    fn walk_expr(&mut self, expr: &Expression, scope: &[HashMap<String, DefId>]) {
        match expr {
            Expression::Name { segments, span } => {
                self.resolve_name(segments, *span, scope);
            }
            // A bare saved root resolves only as part of a larger saved-path
            // expression; resolution happens in the enclosing Call/Field arm
            // where the chain is known.
            Expression::SavedRoot { .. } => {}
            Expression::Unary { operand, .. } => self.walk_expr(operand, scope),
            Expression::Binary { left, right, .. } => {
                self.walk_expr(left, scope);
                self.walk_expr(right, scope);
            }
            Expression::Call { callee, args, .. } => {
                // A saved keyed/record read is call-shaped; resolve its saved
                // member before treating the callee as a function or value.
                if !self.resolve_saved_path(expr) && !self.resolve_call(callee) {
                    self.walk_expr(callee, scope);
                }
                for arg in args {
                    self.walk_expr(&arg.value, scope);
                }
            }
            Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
                // Resolve as a saved path if applicable; otherwise descend into
                // the base.
                if !self.resolve_saved_path(expr) {
                    self.walk_expr(base, scope);
                }
            }
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let marrow_syntax::InterpolationPart::Expr(inner) = part {
                        self.walk_expr(inner, scope);
                    }
                }
            }
            Expression::Literal { .. } => {}
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
                ResolvableKind::Resource,
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
            span: SourceSpan::default(),
        };
        let resolved =
            resolve_enum_member_path(self.builder.program, &expr, self.aliases, self.file)?;
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
            MarrowType::Enum { module, name } => Some((module, name)),
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
        } = callee
        else {
            return false;
        };
        // Constructors are call-shaped but name resources, not functions. Resolve
        // them with the same module-aware resource resolver used by the checker.
        let resolved_segments = expand_alias(segments, self.aliases);
        if let Some(def) = self.builder.module_scope.resolved_resource(
            self.builder.program,
            self.module,
            &resolved_segments,
            ResolvableKind::Resource,
        ) {
            let use_span =
                path_tail_span(self.source, *callee_span, segments, 1).unwrap_or(*callee_span);
            self.builder
                .use_of(def, self.file, use_span, SymbolKind::Resource);
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
            self.builder
                .use_of(def, self.file, *callee_span, SymbolKind::Function);
            return true;
        }
        false
    }

    /// Resolve a saved-path expression (`^root(..).field`, `^root(..).layer(..)`,
    /// `^root.index(..)`, a singleton `^root.field`) to the saved member it reads,
    /// recording the use at the member-name position. Returns whether it resolved.
    fn resolve_saved_path(&mut self, expr: &Expression) -> bool {
        let Some((root, chain, name_span, kind)) = saved_member_ref(expr) else {
            return false;
        };
        // Only a declared saved root resolves; confirm the root is owned.
        if find_resource_schema(self.builder.program, root).is_none() {
            return false;
        }
        let chain: Vec<String> = chain.iter().map(|name| name.to_string()).collect();
        if let Some(def) = self
            .builder
            .module_scope
            .saved_members
            .get(&(root.to_string(), chain))
            .copied()
        {
            self.builder.use_of(def, self.file, name_span, kind);
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
        kind: ResolvableKind,
    ) -> Option<DefId> {
        let Resolution::Found(Def {
            module,
            item: DefItem::Resource(resource),
            ..
        }) = resolve(program, from_module, segments, kind)
        else {
            return None;
        };
        let key = (module.name.clone(), resource.name.clone());
        match kind {
            ResolvableKind::Resource => self.resources.get(&key).copied(),
            ResolvableKind::Function => None,
        }
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

fn enum_identity(ty: &MarrowType) -> Option<(&str, &str)> {
    match ty {
        MarrowType::Enum { module, name } => Some((module, name)),
        MarrowType::Sequence(element) => enum_identity(element),
        _ => None,
    }
}

fn type_ref_enum_leaf_span(source: &str, ty: &TypeRef, enum_name: &str) -> Option<SourceSpan> {
    let end_byte = ty.span.end_byte.min(source.len());
    let text = source.get(ty.span.start_byte..end_byte)?;
    let offset = text.rfind(enum_name)?;
    let start_byte = ty.span.start_byte + offset;
    Some(source_span_at(
        source,
        start_byte,
        start_byte + enum_name.len(),
    ))
}

fn type_ref_path_tail_span(
    source: &str,
    ty: &TypeRef,
    segments: &[String],
    tail_segments: usize,
) -> Option<SourceSpan> {
    path_tail_span(source, ty.span, segments, tail_segments)
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
    let end_byte = span.end_byte.min(source.len());
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

fn source_span_at(source: &str, start_byte: usize, end_byte: usize) -> SourceSpan {
    let prefix = &source.as_bytes()[..start_byte.min(source.len())];
    let line_start = prefix
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |index| index + 1);
    SourceSpan {
        start_byte,
        end_byte,
        line: prefix.iter().filter(|&&byte| byte == b'\n').count() as u32 + 1,
        column: start_byte.saturating_sub(line_start) as u32 + 1,
    }
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

/// Extract `(root, member-chain, member-name span, kind)` from a saved-path read.
/// The chain is the named segments after the identity, outermost first, and the
/// span points at the leaf member name. Covers a top-level field
/// `^root(..).field` / singleton `^root.field`, a keyed layer `^root(..).layer(..)`
/// or unkeyed group hop, and a unique-index lookup `^root.index(..)`. Returns
/// `None` for anything that is not a saved read of a named member.
fn saved_member_ref(expr: &Expression) -> Option<(&str, Vec<&str>, SourceSpan, SymbolKind)> {
    match expr {
        // A field/group read names a stored field; an index is always *called*, so
        // it never appears in a `Field` position.
        Expression::Field {
            base, name, span, ..
        }
        | Expression::OptionalField {
            base, name, span, ..
        } => {
            let (root, mut chain) = saved_field_chain(base)?;
            chain.push(name);
            Some((root, chain, *span, SymbolKind::Field))
        }
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::Field {
                base,
                name,
                span: name_span,
                ..
            } => {
                // `^root.index(args)`: base is the bare saved root, so this names
                // an index by its single-name chain.
                if let Expression::SavedRoot { name: root, .. } = base.as_ref() {
                    return Some((root, vec![name], *name_span, SymbolKind::Index));
                }
                let (root, mut chain) = saved_layer_base(base)?;
                chain.push(name);
                Some((root, chain, *name_span, SymbolKind::Layer))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The `(root, [member..])` chain leading to the base of a field read, peeling
/// group hops.
fn saved_field_chain(expr: &Expression) -> Option<(&str, Vec<&str>)> {
    match expr {
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { name: root, .. } => Some((root, Vec::new())),
            Expression::Field { .. } => saved_layer_base(callee),
            _ => None,
        },
        Expression::SavedRoot { name: root, .. } => Some((root, Vec::new())),
        Expression::Field { base, name, .. } => {
            let (root, mut chain) = saved_field_chain(base)?;
            chain.push(name);
            Some((root, chain))
        }
        _ => None,
    }
}

/// The `(root, [layer..])` chain to a keyed layer base `^root(..).layer`, layer
/// names outermost first. Stricter than the inference-side layer decoder: it
/// requires a keyed `Call` base, so a bare `^root.layer` does not resolve to a
/// layer symbol here and editor symbol resolution does not widen.
fn saved_layer_base(expr: &Expression) -> Option<(&str, Vec<&str>)> {
    let Expression::Field {
        base, name: layer, ..
    } = expr
    else {
        return None;
    };
    let Expression::Call { callee, .. } = base.as_ref() else {
        return None;
    };
    match callee.as_ref() {
        Expression::SavedRoot { name: root, .. } => Some((root, vec![layer])),
        Expression::Field { .. } => {
            let (root, mut layers) = saved_layer_base(callee)?;
            layers.push(layer);
            Some((root, layers))
        }
        _ => None,
    }
}
