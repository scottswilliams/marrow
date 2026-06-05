//! The one name resolver: module-aware and visibility-aware.
//!
//! [`resolve`] maps a referencing module plus a (possibly qualified) name to the
//! declaration it denotes, applying the language's resolution model in one place
//! so the checker, the runtime, and the LSP binding index cannot drift:
//!
//! - a **bare** name resolves in its *own* module first — visible there
//!   regardless of `pub` — and nowhere else, because `use` imports module names,
//!   not the names inside them (the qualified `module::name` is the cross-module
//!   spelling);
//! - a **qualified** `module::leaf` name targets exactly that module: a `pub`
//!   leaf is [`Resolution::Found`], a non-`pub` one is [`Resolution::NotVisible`]
//!   (a distinct visibility error, not "unresolved"), and a missing module or
//!   leaf is [`Resolution::Unresolved`];
//! - **saved roots stay project-wide** ([`resolve_store_by_root`]): a `^root`
//!   addresses its one owning store from any module — only source names such as
//!   resource constructors and type references are module-scoped.
//!
//! Builtins and `std::` helpers are *not* resolved here: each dispatches before
//! user declarations, so callers pre-check them and only reach [`resolve`] for a
//! name that must denote a project declaration. Import aliases are expanded once,
//! up front, against the referencing module's imports.

use marrow_schema::{ResourceSchema, StoreSchema};

use crate::program::{CheckedFunction, CheckedModule, CheckedProgram};
use crate::{build_alias_map, expand_alias};

/// What a name is being resolved *as*. The runtime dispatches builtins, then
/// constructors, then functions, so the kind picks which declaration table a
/// module is searched in — a bare `greet` is a function, a bare `Book` a
/// resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvableKind {
    Function,
    Resource,
}

/// The declaration a resolved name denotes, borrowed from the program: the
/// owning module, the kind it was resolved as, and the item itself. Callers read
/// `module` (for the call frame's import aliases / a go-to-def's source file) and
/// match on `item` for the concrete declaration.
#[derive(Debug, Clone, Copy)]
pub struct Def<'p> {
    pub module: &'p CheckedModule,
    pub kind: ResolvableKind,
    pub item: DefItem<'p>,
}

/// The concrete declaration behind a [`Def`].
#[derive(Debug, Clone, Copy)]
pub enum DefItem<'p> {
    Function(&'p CheckedFunction),
    Resource(&'p ResourceSchema),
}

/// The outcome of resolving a name against the program from one module.
#[derive(Debug, Clone)]
pub enum Resolution<'p> {
    /// The name denotes exactly this declaration.
    Found(Def<'p>),
    /// The name matches `pub` declarations in more than one place (a qualified
    /// path is needed to disambiguate). Carries the candidate module names.
    Ambiguous(Vec<String>),
    /// The name resolves to a declaration that exists but is not `pub` to the
    /// referencing module. Carries the qualified name, for a visibility error.
    NotVisible(String),
    /// The name denotes no declaration the referencing module can reach.
    Unresolved,
}

#[derive(Debug, Clone, Copy)]
pub struct StoreResource<'p> {
    pub module: &'p CheckedModule,
    pub store: &'p StoreSchema,
    pub resource: &'p ResourceSchema,
}

/// Resolve `path` as `kind`, referenced from `from_module`, against `program`.
///
/// Import aliases are expanded once against `from_module`'s imports, so a
/// short-form `books::add` resolves like `shelf::books::add`. Then:
/// a **qualified** `module::leaf` targets that module (pub → [`Resolution::Found`],
/// non-pub → [`Resolution::NotVisible`], missing → [`Resolution::Unresolved`]);
/// a **bare** `leaf` resolves in `from_module` only (self-visible regardless of
/// `pub`), and otherwise is [`Resolution::Unresolved`] — a cross-module scan runs
/// solely to upgrade the diagnostic (`pub` in two-plus modules →
/// [`Resolution::Ambiguous`]; a lone non-pub match → [`Resolution::NotVisible`]).
///
/// Builtins and `std::` helpers are the caller's pre-check; this never resolves
/// them.
pub fn resolve<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    path: &[String],
    kind: ResolvableKind,
) -> Resolution<'p> {
    let aliases = build_alias_map(&module_imports(program, from_module));
    let expanded = expand_alias(path, &aliases);
    let Some((leaf, module_prefix)) = expanded.split_last() else {
        return Resolution::Unresolved;
    };

    if module_prefix.is_empty() {
        return resolve_bare(program, from_module, leaf, kind);
    }
    resolve_qualified(program, &module_prefix.join("::"), leaf, kind)
}

/// Resolve a bare `leaf` in `from_module` only. A match there is visible
/// regardless of `pub` (a module sees its own declarations). Otherwise the name
/// is unresolved — `use` imports module names, not the names inside them — but a
/// cross-module scan upgrades the diagnostic: a name reachable as `pub` in two or
/// more modules is [`Resolution::Ambiguous`] (a qualified path is needed to pick
/// one), and a name that exists elsewhere *only* as a single non-`pub`
/// declaration is [`Resolution::NotVisible`] rather than a bare "unresolved".
fn resolve_bare<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    leaf: &str,
    kind: ResolvableKind,
) -> Resolution<'p> {
    if let Some(module) = find_module(program, from_module)
        && let Some(item) = lookup_in_module(module, leaf, kind)
    {
        return Resolution::Found(Def { module, kind, item });
    }
    // Not in our own module, so the bare name does not resolve to a declaration —
    // `use` imports module names, not the names inside them. Scan the rest of the
    // project only to enrich the diagnostic for this already-erroring reference:
    // collect the modules that expose `leaf` as `pub` (each reachable as
    // `module::leaf`) and note a lone non-`pub` match.
    let mut public: Vec<String> = Vec::new();
    let mut sole_private: Option<&str> = None;
    let mut private_count = 0usize;
    for module in &program.modules {
        // An empty-named module is a single-file script, which no `use` can name,
        // so it must never be surfaced as a candidate in this enrichment hint.
        if module.name == from_module || module.name.is_empty() {
            continue;
        }
        if let Some(item) = lookup_in_module(module, leaf, kind) {
            if is_public(&item) {
                public.push(module.name.clone());
            } else {
                private_count += 1;
                sole_private = Some(&module.name);
            }
        }
    }
    match (public.len(), private_count, sole_private) {
        // Two or more modules expose `leaf` as `pub`: the bare name cannot pick
        // one, so qualifying it is required. Name the candidates for the hint.
        (2.., _, _) => Resolution::Ambiguous(public),
        // Reachable as `pub` in exactly one module, but only via `module::leaf`;
        // the bare name still does not resolve to it. Plainly unresolved.
        (1, _, _) => Resolution::Unresolved,
        // The only matches anywhere are a single non-`pub` declaration: a
        // visibility problem, not a missing one.
        (0, 1, Some(module)) => Resolution::NotVisible(format!("{module}::{leaf}")),
        _ => Resolution::Unresolved,
    }
}

/// Resolve a qualified `module::leaf`: the module must exist, the leaf must be
/// declared in it, and (cross-module) it must be `pub`. A non-`pub` leaf is a
/// distinct [`Resolution::NotVisible`]; a missing module or leaf is
/// [`Resolution::Unresolved`].
fn resolve_qualified<'p>(
    program: &'p CheckedProgram,
    module_name: &str,
    leaf: &str,
    kind: ResolvableKind,
) -> Resolution<'p> {
    let Some(module) = find_module(program, module_name) else {
        return Resolution::Unresolved;
    };
    let Some(item) = lookup_in_module(module, leaf, kind) else {
        return Resolution::Unresolved;
    };
    if is_public(&item) {
        Resolution::Found(Def { module, kind, item })
    } else {
        Resolution::NotVisible(format!("{module_name}::{leaf}"))
    }
}

/// The store owning saved root `^root`, plus the resource tree shape it stores.
pub fn resolve_store_by_root<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<StoreResource<'p>> {
    for module in &program.modules {
        if let Some(store) = module.stores.iter().find(|store| store.root == root)
            && let Some(resource) = module
                .resources
                .iter()
                .find(|resource| resource.name == store.resource)
        {
            return Some(StoreResource {
                module,
                store,
                resource,
            });
        }
    }
    None
}

/// Look up `leaf` in `module` for `kind`, returning the matching declaration.
/// Functions and resources each live in their own table.
fn lookup_in_module<'p>(
    module: &'p CheckedModule,
    leaf: &str,
    kind: ResolvableKind,
) -> Option<DefItem<'p>> {
    match kind {
        ResolvableKind::Function => module
            .functions
            .iter()
            .find(|function| function.name == leaf)
            .map(DefItem::Function),
        ResolvableKind::Resource => module
            .resources
            .iter()
            .find(|resource| resource.name == leaf)
            .map(DefItem::Resource),
    }
}

/// Whether a resolved item is callable/usable across a module boundary. Functions
/// carry `pub`; a resource is not yet visibility-gated (it belongs to its module
/// but its name is project-visible), so it is visible when reached by a qualified
/// path.
fn is_public(item: &DefItem<'_>) -> bool {
    match item {
        DefItem::Function(function) => function.public,
        DefItem::Resource(_) => true,
    }
}

/// The module named `name`, if the program has it.
fn find_module<'p>(program: &'p CheckedProgram, name: &str) -> Option<&'p CheckedModule> {
    program.modules.iter().find(|module| module.name == name)
}

/// The resolved `use` targets of `name`, or an empty list when no such module is
/// in the program. Drives alias expansion.
fn module_imports(program: &CheckedProgram, name: &str) -> Vec<String> {
    find_module(program, name)
        .map(|module| module.imports.clone())
        .unwrap_or_default()
}
