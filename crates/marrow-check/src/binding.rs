//! The project-wide binding index: every identifier *use* resolved to the
//! *definition* it names, respecting lexical scope (shadowing) and `use` import
//! aliases, plus a rename-safety classification.
//!
//! This is the resolution layer the editor tooling (go-to-definition,
//! find-all-references, rename) builds on. It does not reimplement name
//! resolution: it reuses the checker's unified `resolve` (via
//! `resolve_function_in_module`) for module-aware function calls — short-form
//! `use` aliases and visibility included — `find_resource_schema` plus the
//! schema's field/layer/index lookup for saved data, and the same block
//! traversal shape the A1 scope primitives use so a local's scope cannot drift
//! from the one the checker builds.
//!
//! Spans. A *use* of a single-segment name (a local, parameter, module constant,
//! or unqualified function) is recorded at the identifier itself, because the
//! parser gives a single-segment [`Expression::Name`] a span covering exactly that
//! token. A qualified call (`shelf::books::add`, `books::add`) is recorded at the
//! whole path span, while an enum member literal (`Cat::tiger::bengal`) is split
//! so the enum segment and each written member-path segment resolve
//! independently. A *definition* is recorded at its declaration node's span; a
//! parameter, loop, or catch binding has no node span of its own in the AST, so
//! its definition is recorded at the enclosing declaration. Consumers that need a
//! tighter range can narrow within these spans using the source text.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use marrow_schema::Type as SchemaType;
use marrow_syntax::{
    Block, Declaration, EnumMember, Expression, MatchArm, ParsedSource, ResourceDecl,
    ResourceMember, SourceSpan, Statement, TypeRef,
};

use crate::MarrowType;
use crate::enums::resolve_enum;
use crate::{
    AnalysisSnapshot, CheckedProgram, Def, DefItem, Resolution, ResolvableKind, expand_alias,
    expand_module_alias, file_prelude, find_resource_schema, for_frame, infer_only, local_binding,
    resolve, resolve_enum_member_path, resolve_function_in_module, resolve_type,
};

/// What a [`SymbolRef`] names. The kinds the index resolves, grouped by where they
/// live: function-local bindings (`Param`, `Local`), module-level declarations
/// (`ModuleConst`, `Function`, `Resource` and its generated `ResourceIdentity`),
/// enum declarations and members (`Enum`, `EnumMember`), the members of a
/// resource that name saved data (`Field`, `Layer`, `Index`), and imported module
/// aliases (`ModuleRef`).
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
    /// A resource's generated `Type::Id` identity.
    ResourceIdentity,
    /// A declared enum type.
    Enum,
    /// A declared enum member.
    EnumMember,
    /// A stored field of a resource (a top-level field or a group field).
    Field,
    /// A keyed layer or group of a resource.
    Layer,
    /// A declared lookup index of a resource.
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
/// aliases, and resources/identities with no saved root) exist only in source, so
/// a rename that updates every reference is complete. `SavedDataBacked` symbols
/// name something encoded on disk — a saved field/group/layer/index, or a
/// resource/identity attached to a saved root — whose stored path uses the source
/// name. Renaming such a symbol changes the on-disk path and orphans stored data
/// unless explicit maintenance work moves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameSafety {
    /// Safe to rename in source alone.
    SourceOnly,
    /// Names data encoded on disk; renaming orphans stored data unless migrated.
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
    // Definitions come from the parsed files (they carry source spans the
    // resolved program does not): module constants, functions, resources and
    // their members, and `use` aliases.
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
/// resolved through [`resolve_function_in_module`]; constants, resources, and
/// identities are resolved by name here.
#[derive(Default)]
struct ModuleScope {
    /// `module name -> (const name -> DefId)`.
    constants: HashMap<String, HashMap<String, DefId>>,
    /// `(module name, resource name) -> DefId` for the resource type symbol.
    resources: HashMap<(String, String), DefId>,
    /// `(module name, resource name) -> DefId` for the generated identity symbol.
    identities: HashMap<(String, String), DefId>,
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
    /// resources (and their identity and members), and `use` aliases.
    fn collect_definitions(&mut self, file: &Path, parsed: &ParsedSource) {
        let module = Self::module_name(parsed);

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
                    self.collect_resource(file, &module, resource);
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

    /// Collect a resource definition: the resource type, its generated identity,
    /// and its saved members (fields, groups/layers, indexes). The resource and
    /// identity are data-backed exactly when the resource has a saved root.
    /// Resources and identities are keyed by their declaring module plus source
    /// name; use sites choose the module through the checker resolver.
    fn collect_resource(&mut self, file: &Path, module: &str, resource: &ResourceDecl) {
        let saved_root = resource.store.as_ref().map(|store| store.root.clone());
        let type_safety = match &saved_root {
            Some(_) => RenameSafety::SavedDataBacked,
            None => RenameSafety::SourceOnly,
        };
        let resource_id = self.define(
            SymbolRef {
                file: file.to_path_buf(),
                span: resource.span,
                kind: SymbolKind::Resource,
            },
            type_safety.clone(),
        );
        self.module_scope
            .resources
            .insert((module.to_string(), resource.name.clone()), resource_id);
        // The generated `Type::Id` identity shares the resource's declaration site
        // and saved-data backing.
        let identity_id = self.define(
            SymbolRef {
                file: file.to_path_buf(),
                span: resource.span,
                kind: SymbolKind::ResourceIdentity,
            },
            type_safety,
        );
        self.module_scope
            .identities
            .insert((module.to_string(), resource.name.clone()), identity_id);

        if let Some(root) = &saved_root {
            self.collect_members(file, root, &mut Vec::new(), &resource.members);
        }
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
                    // Descend into the group's members with this layer on the chain.
                    self.collect_members(file, root, chain, &group.members);
                    chain.pop();
                }
                ResourceMember::Index(index) => {
                    // An index is addressed as `^root.index(..)`, a single-name
                    // chain distinct from a field's path. Key it under its own name.
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
                // The function body runs under a scope of its parameters over the
                // module's bindings. Parameters have no AST span, so a parameter
                // binding is defined here at the function span and tracked by name.
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
                if let Some(store) = &resource.store {
                    self.collect_key_type_refs(file, source, aliases, module, &store.keys);
                }
                self.collect_resource_member_type_refs(
                    file,
                    source,
                    aliases,
                    module,
                    &resource.members,
                );
            }
            Declaration::Function(function) => {
                for param in &function.params {
                    self.collect_type_ref(file, source, aliases, module, &param.ty);
                }
                if let Some(ty) = &function.return_type {
                    self.collect_type_ref(file, source, aliases, module, ty);
                }
            }
            Declaration::Enum(_) => {}
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
                ResourceMember::Index(_) => {}
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
            SchemaType::Identity(resource) => {
                let mut segments = split_type_path(resource);
                segments.push("Id".to_string());
                let resolved_segments = expand_alias(&segments, aliases);
                if let Some(def) = self.module_scope.resolved_resource(
                    self.program,
                    module,
                    &resolved_segments,
                    ResolvableKind::ResourceIdentity,
                ) && let Some(span) = type_ref_path_tail_span(source, ty, &segments, 2)
                {
                    self.use_of(def, file, span, SymbolKind::ResourceIdentity);
                }
            }
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

/// Walks a function body collecting identifier uses against a scope of local
/// bindings, mirroring the block/statement traversal the A1 scope primitives use
/// (push a frame per block; record `const`/`var` bindings as they appear; push a
/// loop/catch frame for the body that runs under it) so a local's scope cannot
/// drift from the checker's.
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
                // The initializer is in the scope *before* the binding, so resolve
                // it first, then bind the name for later statements.
                self.walk_expr(value, scope);
                let id = self.define_local(*span);
                bind(scope, name, id);
                if let Some((_, ty)) = local_binding(
                    self.builder.program,
                    statement,
                    type_scope,
                    self.aliases,
                    self.file,
                ) {
                    bind_type(type_scope, name, ty);
                }
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
                let id = self.define_local(*span);
                bind(scope, name, id);
                if let Some((_, ty)) = local_binding(
                    self.builder.program,
                    statement,
                    type_scope,
                    self.aliases,
                    self.file,
                ) {
                    bind_type(type_scope, name, ty);
                }
            }
            Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
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
            } => {
                self.walk_expr(iterable, scope);
                // The loop binding(s) are in scope only within the body. They have
                // no AST span, so each is defined at the statement span.
                let mut frame = HashMap::new();
                frame.insert(binding.first.clone(), self.define_local(statement.span()));
                if let Some(second) = &binding.second {
                    frame.insert(second.clone(), self.define_local(statement.span()));
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
                // Body statements run under the loop frame plus their own block.
                for inner in &body.statements {
                    self.walk_statement(inner, scope, type_scope);
                }
                scope.pop();
                type_scope.pop();
            }
            Statement::Transaction { body, .. } => self.walk_block(body, scope, type_scope),
            Statement::Lock { path, body, .. } => {
                if let Some(path) = path {
                    self.walk_expr(path, scope);
                }
                self.walk_block(body, scope, type_scope);
            }
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => {
                self.walk_block(body, scope, type_scope);
                if let Some(clause) = catch {
                    if let Some(ty) = &clause.ty {
                        self.resolve_type_ref(ty);
                    }
                    // The caught error is bound for the catch block only.
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
            Statement::Match {
                scrutinee, arms, ..
            } => {
                if let Some(scrutinee) = scrutinee {
                    self.walk_expr(scrutinee, scope);
                }
                let enum_identity = scrutinee
                    .as_ref()
                    .and_then(|scrutinee| self.match_scrutinee_enum(scrutinee, type_scope));
                if let Some((enum_module, enum_name)) = enum_identity {
                    for arm in arms {
                        self.resolve_match_arm(&enum_module, &enum_name, arm);
                    }
                }
                for arm in arms {
                    self.walk_block(&arm.block, scope, type_scope);
                }
            }
            Statement::Break { .. } | Statement::Continue { .. } => {}
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
            Expression::SavedRoot { .. } => {
                // A bare saved root resolves to the resource owning it only as part
                // of a larger saved-path expression; resolution happens in the
                // enclosing Call/Field arm where the chain is known.
            }
            Expression::Unary { operand, .. } => self.walk_expr(operand, scope),
            Expression::Binary { left, right, .. } => {
                self.walk_expr(left, scope);
                self.walk_expr(right, scope);
            }
            Expression::Call { callee, args, .. } => {
                // A saved keyed read `^root(..).layer(..)` or a record read
                // `^root(..)` is call-shaped; resolve its saved member if it names
                // one before treating the callee as a function/value.
                if !self.resolve_saved_path(expr) && !self.resolve_call(callee) {
                    self.walk_expr(callee, scope);
                }
                for arg in args {
                    self.walk_expr(&arg.value, scope);
                }
            }
            Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
                // A saved field read `^root(..).field` (or a deeper group path)
                // resolves to the field; otherwise descend into the base.
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
    /// or a resource/identity. Records the use when it resolves.
    fn resolve_name(
        &mut self,
        segments: &[String],
        span: SourceSpan,
        scope: &[HashMap<String, DefId>],
    ) {
        if let [name] = segments {
            // A single-segment name is a local, parameter, module constant,
            // resource/identity, or imported alias.
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
        // A qualified name: `Resource::Id` or `module::Resource::Id` names the
        // generated identity, with module aliases expanded by the checker resolver.
        let resolved_segments = expand_alias(segments, self.aliases);
        if segments.last().is_some_and(|segment| segment == "Id")
            && let Some(def) = self.builder.module_scope.resolved_resource(
                self.builder.program,
                self.module,
                &resolved_segments,
                ResolvableKind::ResourceIdentity,
            )
        {
            let use_span = path_tail_span(self.source, span, segments, 2).unwrap_or(span);
            self.builder
                .use_of(def, self.file, use_span, SymbolKind::ResourceIdentity);
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
        let enum_index =
            self.resolved_enum_index(segments, &resolved.module, &resolved.enum_name)?;
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

    fn resolved_enum_index(
        &self,
        segments: &[String],
        resolved_module: &str,
        resolved_enum: &str,
    ) -> Option<usize> {
        if segments.first()? == resolved_enum
            && resolve_enum(self.builder.program, Some(self.module), &segments[0])
                .is_some_and(|(module, _)| module == resolved_module)
        {
            return Some(0);
        }

        for enum_index in (1..segments.len().saturating_sub(1)).rev() {
            if segments[enum_index] != resolved_enum {
                continue;
            }
            let module = expand_module_alias(&segments[..enum_index].join("::"), self.aliases);
            if module == resolved_module
                && self.enum_schema(&module, &segments[enum_index]).is_some()
            {
                return Some(enum_index);
            }
        }
        None
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
        let Some(schema) = self.enum_schema(enum_module, enum_name) else {
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

    fn enum_schema(
        &self,
        enum_module: &str,
        enum_name: &str,
    ) -> Option<&marrow_schema::EnumSchema> {
        self.builder
            .program
            .modules
            .iter()
            .find(|module| module.name == enum_module)?
            .enums
            .iter()
            .find(|schema| schema.name == enum_name)
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
        // them with the same module-aware resource resolver used by the checker,
        // including the identity-precedence path for `Book::Id(...)`.
        let resolved_segments = expand_alias(segments, self.aliases);
        if segments.last().is_some_and(|segment| segment == "Id")
            && let Some(def) = self.builder.module_scope.resolved_resource(
                self.builder.program,
                self.module,
                &resolved_segments,
                ResolvableKind::ResourceIdentity,
            )
        {
            let use_span =
                path_tail_span(self.source, *callee_span, segments, 2).unwrap_or(*callee_span);
            self.builder
                .use_of(def, self.file, use_span, SymbolKind::ResourceIdentity);
            return true;
        }
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
        // Only a declared saved root with this member resolves; reuse the checker's
        // resource lookup to confirm the root is owned.
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
            ResolvableKind::ResourceIdentity => self.identities.get(&key).copied(),
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
fn bind(scope: &mut [HashMap<String, DefId>], name: &str, id: DefId) {
    if let Some(frame) = scope.last_mut() {
        frame.insert(name.to_string(), id);
    }
}

fn bind_type(scope: &mut [HashMap<String, MarrowType>], name: &str, ty: MarrowType) {
    if let Some(frame) = scope.last_mut() {
        frame.insert(name.to_string(), ty);
    }
}

fn enum_identity(ty: &MarrowType) -> Option<(&str, &str)> {
    match ty {
        MarrowType::Enum { module, name } => Some((module, name)),
        MarrowType::Sequence(element) => enum_identity(element),
        _ => None,
    }
}

fn split_type_path(path: &str) -> Vec<String> {
    path.split("::").map(str::to_string).collect()
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

/// Whether `span` covers `offset`, inclusive of the end byte.
fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

fn span_width(span: SourceSpan) -> usize {
    span.end_byte.saturating_sub(span.start_byte)
}

/// Extract `(root, member-chain, member-name span, kind)` from a saved-path read,
/// where the member chain is the named segments after the identity (outermost
/// first) and the span points at the leaf member name. Resolves a top-level field
/// `^root(..).field` / singleton `^root.field`, a keyed layer `^root(..).layer(..)`
/// or unkeyed group hop, and a unique-index lookup `^root.index(..)`. Returns
/// `None` for anything that is not a saved read of a named member.
fn saved_member_ref(expr: &Expression) -> Option<(&str, Vec<&str>, SourceSpan, SymbolKind)> {
    match expr {
        // A field/group read off a saved base: `^root(..).name`, `^root.name`, or a
        // deeper group hop `^root(..).group.name`.
        Expression::Field {
            base, name, span, ..
        }
        | Expression::OptionalField {
            base, name, span, ..
        } => {
            // A field/group read names a stored field of the resource (an index is
            // always *called*, so it never appears in a `Field` position).
            let (root, mut chain) = saved_field_chain(base)?;
            chain.push(name);
            Some((root, chain, *span, SymbolKind::Field))
        }
        // A keyed layer read `^root(..).layer(..)` (a call whose callee is the layer
        // field) or an index lookup `^root.index(..)`.
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::Field {
                base,
                name,
                span: name_span,
                ..
            } => {
                // `^root.index(args)`: base is the bare saved root, so this names an
                // index by its single-name chain.
                if let Expression::SavedRoot { name: root, .. } = base.as_ref() {
                    return Some((root, vec![name], *name_span, SymbolKind::Index));
                }
                // `^root(..).layer(..)`: base is a keyed record or deeper layer.
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
/// group hops: a keyed record `^root(..)`, a singleton root `^root`, or a deeper
/// unkeyed group `..group`.
fn saved_field_chain(expr: &Expression) -> Option<(&str, Vec<&str>)> {
    match expr {
        // `^root(..)`: the keyed record. The following field is the first member.
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::SavedRoot { name: root, .. } => Some((root, Vec::new())),
            // A keyed layer `^root(..).layer(..)` as a base: peel it as a layer.
            Expression::Field { .. } => saved_layer_base(callee),
            _ => None,
        },
        // `^root`: a singleton resource addressed by its root.
        Expression::SavedRoot { name: root, .. } => Some((root, Vec::new())),
        // `..group`: an unkeyed group hop; recurse and append this group name.
        Expression::Field { base, name, .. } => {
            let (root, mut chain) = saved_field_chain(base)?;
            chain.push(name);
            Some((root, chain))
        }
        _ => None,
    }
}

/// The `(root, [layer..])` chain to a keyed layer base `^root(..).layer`, with
/// layer names outermost first. Each `Field` peels one layer; its base is the
/// keyed record `^root(..)` or a deeper keyed layer entry.
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
