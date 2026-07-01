//! Module-aware name resolution for functions and resources. Function `pub`
//! controls cross-module calls; resources and saved roots remain project-visible
//! in v0.1. Builtins and `std::` helpers dispatch before user declarations and
//! are the caller's pre-check, never resolved here.

use marrow_schema::{ResourceSchema, StoreSchema};

use crate::program::{CheckedFunction, CheckedModule, CheckedProgram};
use crate::{build_alias_map, expand_alias};

/// What a name is being resolved *as*, picking which declaration table of a
/// module is searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvableKind {
    Function,
    Resource,
}

/// The declaration a resolved name denotes, borrowed from the program: its owning
/// module, the module's position in the program (so a caller building a
/// [`CheckedFunctionRef`] needs no second whole-program scan to recover it), the kind
/// it was resolved as, and the item itself.
#[derive(Debug, Clone, Copy)]
pub struct Def<'p> {
    pub module: &'p CheckedModule,
    pub module_index: usize,
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

/// Resolve `path` as `kind`, referenced from `from_module`. Import aliases are
/// expanded once against `from_module`'s imports, so a short-form `books::add`
/// resolves like `shelf::books::add`, then a qualified path dispatches to
/// [`resolve_qualified`] and a bare one to [`resolve_bare`].
pub fn resolve<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    path: &[String],
    kind: ResolvableKind,
) -> Resolution<'p> {
    let imports = find_module_indexed(program, from_module)
        .map(|(_, module)| module.imports.clone())
        .unwrap_or_default();
    let aliases = build_alias_map(&imports);
    let expanded = expand_alias(path, &aliases);
    let Some((leaf, module_prefix)) = expanded.split_last() else {
        return Resolution::Unresolved;
    };

    if module_prefix.is_empty() {
        return resolve_bare(program, from_module, leaf, kind);
    }
    resolve_qualified(program, from_module, &module_prefix.join("::"), leaf, kind)
}

/// Resolve a bare `leaf` in `from_module` only, where it is visible regardless of
/// `pub`. A name found nowhere else is unresolved, but a cross-module scan can
/// upgrade the diagnostic to [`Resolution::Ambiguous`] or [`Resolution::NotVisible`].
fn resolve_bare<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    leaf: &str,
    kind: ResolvableKind,
) -> Resolution<'p> {
    if let Some((module_index, module)) = find_module_indexed(program, from_module)
        && let Some(item) = lookup_in_module(program, module_index, module, leaf, kind)
    {
        return Resolution::Found(Def {
            module,
            module_index,
            kind,
            item,
        });
    }
    // The bare name does not resolve. Consult the index for the modules declaring
    // `leaf` as `kind`, only to enrich the diagnostic: collect modules exposing it as
    // `pub` and note a lone non-`pub` match.
    let mut public: Vec<String> = Vec::new();
    let mut sole_private: Option<&str> = None;
    let mut private_count = 0usize;
    for module_index in program.modules_declaring(leaf, kind) {
        let module = &program.modules[module_index];
        // An empty-named module is a single-file script, which no `use` can name,
        // so it must never be surfaced as a candidate in this enrichment hint.
        if module.name == from_module || module.name.is_empty() {
            continue;
        }
        if let Some(item) = lookup_in_module(program, module_index, module, leaf, kind) {
            if is_public(&item) {
                public.push(module.name.clone());
            } else {
                private_count += 1;
                sole_private = Some(&module.name);
            }
        }
    }
    match (public.len(), private_count, sole_private) {
        // Reachable as `pub` from two-plus modules: the bare name cannot pick one,
        // so name the candidates and require qualification.
        (2.., _, _) => Resolution::Ambiguous(public),
        // Reachable as `pub` from exactly one module, but only via `module::leaf`,
        // so the bare name still does not resolve.
        (1, _, _) => Resolution::Unresolved,
        // The only match anywhere is a single non-`pub` declaration: a visibility
        // problem, not a missing one.
        (0, 1, Some(module)) => Resolution::NotVisible(format!("{module}::{leaf}")),
        _ => Resolution::Unresolved,
    }
}

/// Resolve a qualified `module::leaf` referenced from `from_module`. A non-`pub`
/// leaf is visible only when `module_name` is the referencing module itself, so a
/// module reaches its own private items by their full path just as a bare name
/// does; from any other module a non-`pub` leaf is a distinct
/// [`Resolution::NotVisible`], not an unresolved name.
fn resolve_qualified<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    module_name: &str,
    leaf: &str,
    kind: ResolvableKind,
) -> Resolution<'p> {
    let Some((module_index, module)) = find_module_indexed(program, module_name) else {
        return Resolution::Unresolved;
    };
    let Some(item) = lookup_in_module(program, module_index, module, leaf, kind) else {
        return Resolution::Unresolved;
    };
    if module_name == from_module || is_public(&item) {
        Resolution::Found(Def {
            module,
            module_index,
            kind,
            item,
        })
    } else {
        Resolution::NotVisible(format!("{module_name}::{leaf}"))
    }
}

/// The store owning saved root `^root`, plus the resource tree shape it stores. The
/// owning module is found in O(1) through the store-root fact index, then the store and
/// its resource are located by their bounded in-module declaration lists; a store fact
/// exists only once its resource resolved in its own module, so that resource is always
/// present here. The former whole-program scan ran on every `^root` reference, making a
/// store-heavy project quadratic in its store count.
pub fn resolve_store_by_root<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<StoreResource<'p>> {
    let store_fact = program.facts.store_by_root(root)?;
    let module = program.modules.get(store_fact.module.0 as usize)?;
    let store = module.stores.iter().find(|store| store.root == root)?;
    let resource = module
        .resources
        .iter()
        .find(|resource| resource.name == store.resource)?;
    Some(StoreResource {
        module,
        store,
        resource,
    })
}

/// Look up `leaf` in the module at `module_index` for `kind`, returning the matching
/// declaration. Functions and resources each live in their own table, resolved O(1)
/// through the program's per-module name index rather than a per-reference scan.
fn lookup_in_module<'p>(
    program: &'p CheckedProgram,
    module_index: usize,
    module: &'p CheckedModule,
    leaf: &str,
    kind: ResolvableKind,
) -> Option<DefItem<'p>> {
    match kind {
        ResolvableKind::Function => program
            .function_index_in_module(module_index, leaf)
            .and_then(|index| module.functions.get(index))
            .map(DefItem::Function),
        ResolvableKind::Resource => program
            .resource_index_in_module(module_index, leaf)
            .and_then(|index| module.resources.get(index))
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

/// The module named `name` and its position in the program, resolved O(1) through the
/// program's module-name index. The position lets a resolved [`Def`] carry its owning
/// module's index so building a [`CheckedFunctionRef`] needs no second scan.
fn find_module_indexed<'p>(
    program: &'p CheckedProgram,
    name: &str,
) -> Option<(usize, &'p CheckedModule)> {
    let index = program.module_index_by_name(name)?;
    Some((index, &program.modules[index]))
}
