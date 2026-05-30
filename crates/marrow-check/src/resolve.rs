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
//! - **saved roots stay project-wide** ([`resolve_resource_by_root`]): a `^root`
//!   addresses its one owning resource from any module — only the resource *name*
//!   (a constructor or type reference) is module-scoped.
//!
//! Builtins and `std::` helpers are *not* resolved here: each dispatches before
//! user declarations, so callers pre-check them and only reach [`resolve`] for a
//! name that must denote a project declaration. Import aliases are expanded once,
//! up front, against the referencing module's imports.

use marrow_schema::ResourceSchema;

use crate::program::{CheckedConst, CheckedFunction, CheckedModule, CheckedProgram};
use crate::{build_alias_map, expand_alias};

/// What a name is being resolved *as*. The runtime dispatches builtins, then
/// constructors, then functions, so the kind picks which declaration table a
/// module is searched in — a bare `greet` is a function, a bare `Book` a
/// resource, `Book::Id` its identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvableKind {
    Function,
    Resource,
    ResourceIdentity,
    Const,
    Type,
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

/// The concrete declaration behind a [`Def`]. A resource resolved as a constructor
/// or as a type/identity both carry the [`ResourceSchema`]; the [`Def::kind`]
/// distinguishes how it was reached.
#[derive(Debug, Clone, Copy)]
pub enum DefItem<'p> {
    Function(&'p CheckedFunction),
    Resource(&'p ResourceSchema),
    Const(&'p CheckedConst),
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
    // A `Book::Id` identity is the only two-segment name whose prefix is not a
    // module — it is the resource name. Resolve it by name (module-scoped) before
    // the generic module/leaf split treats `Book` as a module qualifier.
    if kind == ResolvableKind::ResourceIdentity
        && let [name, id] = path
        && id == "Id"
    {
        return resolve_resource_by_name(program, from_module, name, kind);
    }

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
        if module.name == from_module {
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

/// Resolve a resource by its declared *name* (module-scoped), for a constructor
/// or `Name::Id` identity: the referencing module's own resource first, else a
/// resource of that name anywhere in the project (resource names are not yet
/// visibility-gated). Mirrors the bare-then-project fallback the constructor and
/// identity paths used before this resolver existed.
fn resolve_resource_by_name<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    name: &str,
    kind: ResolvableKind,
) -> Resolution<'p> {
    if let Some(module) = find_module(program, from_module)
        && let Some(resource) = module.resources.iter().find(|r| r.name == name)
    {
        return Resolution::Found(Def {
            module,
            kind,
            item: DefItem::Resource(resource),
        });
    }
    for module in &program.modules {
        if let Some(resource) = module.resources.iter().find(|r| r.name == name) {
            return Resolution::Found(Def {
                module,
                kind,
                item: DefItem::Resource(resource),
            });
        }
    }
    Resolution::Unresolved
}

/// The resource owning saved root `^root`, searched project-wide (saved roots are
/// global: a `^books` write addresses the one `books` resource from any module).
/// Mirrors the runtime's `find_resource`.
pub fn resolve_resource_by_root<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<&'p ResourceSchema> {
    program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .find(|resource| {
            resource
                .saved_root
                .as_ref()
                .is_some_and(|saved| saved.root == root)
        })
}

/// The resource declared with `name` anywhere in the project, for a constructor
/// or `Name::Id` identity keyed on the resource name. Resource names are not yet
/// visibility-gated, so this is a project-wide name lookup.
pub fn resolve_resource_by_name_any<'p>(
    program: &'p CheckedProgram,
    name: &str,
) -> Option<&'p ResourceSchema> {
    program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .find(|resource| resource.name == name)
}

/// Look up `leaf` in `module` for `kind`, returning the matching declaration.
/// Constants/types/functions/resources each live in their own table.
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
        ResolvableKind::Resource | ResolvableKind::ResourceIdentity | ResolvableKind::Type => {
            module
                .resources
                .iter()
                .find(|resource| resource.name == leaf)
                .map(DefItem::Resource)
        }
        ResolvableKind::Const => module
            .constants
            .iter()
            .find(|constant| constant.name == leaf)
            .map(DefItem::Const),
    }
}

/// Whether a resolved item is callable/usable across a module boundary. Functions
/// carry `pub`; resources and constants are not yet visibility-gated (a resource
/// belongs to its module but its name is project-visible; a constant is treated
/// as visible by name), so they are visible when reached by a qualified path.
fn is_public(item: &DefItem<'_>) -> bool {
    match item {
        DefItem::Function(function) => function.public,
        DefItem::Resource(_) | DefItem::Const(_) => true,
    }
}

/// The module named `name`, if the program has it.
fn find_module<'p>(program: &'p CheckedProgram, name: &str) -> Option<&'p CheckedModule> {
    program.modules.iter().find(|module| module.name == name)
}

/// The resolved `use` targets of `name`, or an empty list when no such module is
/// in the program (the bare-program path). Drives alias expansion.
fn module_imports(program: &CheckedProgram, name: &str) -> Vec<String> {
    find_module(program, name)
        .map(|module| module.imports.clone())
        .unwrap_or_default()
}
