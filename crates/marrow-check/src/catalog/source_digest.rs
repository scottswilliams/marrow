use crate::CheckedProgram;

/// A stable digest of the analyzed program's durable shape, in the same
/// `sha256:<hex>` form the catalog digest uses. This is the digest the store stamps
/// at commit and the activation-window fence enforces, so it binds exactly the facts a
/// stored snapshot must satisfy: each `resource`, `store`, `enum`, and module `const`.
///
/// It excludes the `evolve` block. A consumed block describes work already recorded in
/// the accepted catalog, so hashing it would read its deletion as schema drift; the fence
/// tracks the durable shape, not the transition that produced it.
///
/// The digest hashes the canonical formatter's rendering of those declarations rather than
/// enumerating their fields, so any shape change drifts it while a whitespace reformat does
/// not. The formatter is therefore a frozen anchor: a golden over its output pins the text,
/// so a formatter change that moved it for an unchanged shape must be handled as a
/// store-format decision rather than silently re-reading every committed snapshot as drift.
pub(crate) fn analyzed_source_digest(program: &CheckedProgram) -> String {
    digest_of(&render_declarations(program), DigestScope::Shape)
}

/// Both the shape and shape-plus-evolve digests from a single render pass, so a caller that
/// needs both (the evolution preview witness) reads and parses each module's source once.
pub(crate) fn source_and_evolution_digests(program: &CheckedProgram) -> (String, String) {
    let renderings = render_declarations(program);
    (
        digest_of(&renderings, DigestScope::Shape),
        digest_of(&renderings, DigestScope::ShapeAndEvolve),
    )
}

/// A stable digest of the analyzed shape *and* the evolve decision surface, in the same
/// `sha256:<hex>` form. It binds everything [`analyzed_source_digest`] binds plus each
/// `evolve` block, so a changed evolve default value or transform body drifts it.
///
/// The evolution witness records this digest, not the shape digest, so apply aborts
/// when the source it activates no longer matches what was discharged — including a
/// transform-body edit the shape digest cannot see. The two digests divide the work:
/// the store fences on shape so a consumed block is deletable, and the witness fences on
/// shape-plus-intent so the preview-to-apply transition cannot silently change.
pub(crate) fn evolution_digest(program: &CheckedProgram) -> String {
    digest_of(&render_declarations(program), DigestScope::ShapeAndEvolve)
}

/// Which declarations a digest binds. The shape digest the store stamps excludes the
/// evolve block; the evolution digest the witness records includes it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DigestScope {
    Shape,
    ShapeAndEvolve,
}

impl DigestScope {
    /// Whether a declaration of `kind` contributes to a digest at this scope.
    fn binds(self, kind: DurableKind) -> bool {
        match self {
            DigestScope::Shape => kind != DurableKind::Evolve,
            DigestScope::ShapeAndEvolve => true,
        }
    }
}

/// Render every durable declaration of the program into the deterministically ordered
/// renderings the digests hash. Each scope hashes a subset of this single pass, so the source
/// is read and parsed once per render rather than once per digest.
///
/// The rendering reads each module's source file because the formatter operates on the
/// syntax tree, which the checked program drops. A source file that no longer reads or
/// parses (a checked-program invariant violation) contributes a path-tagged marker so
/// the digest stays deterministic and never silently collides with a clean rendering.
fn render_declarations(program: &CheckedProgram) -> Vec<DurableRendering> {
    let mut entries: Vec<DurableRendering> = Vec::new();
    for module in &program.modules {
        let source = std::fs::read_to_string(&module.source_file).ok();
        let parsed = source.as_deref().map(marrow_syntax::parse_source);
        match (&source, &parsed) {
            (Some(source), Some(parsed)) => {
                for declaration in &parsed.file.declarations {
                    let Some(kind) = durable_kind(declaration) else {
                        continue;
                    };
                    entries.push(DurableRendering {
                        module: module.name.clone(),
                        kind,
                        name: declaration_name(declaration),
                        text: marrow_syntax::format_declaration(source, declaration),
                    });
                }
            }
            _ => entries.push(DurableRendering {
                module: module.name.clone(),
                kind: DurableKind::Unreadable,
                name: module.source_file.display().to_string(),
                text: String::new(),
            }),
        }
    }
    entries.sort_by(|a, b| {
        (&a.module, a.kind as u8, &a.name).cmp(&(&b.module, b.kind as u8, &b.name))
    });
    entries
}

/// Hash the renderings that bind at `scope` into the canonical `sha256:<hex>` digest.
fn digest_of(entries: &[DurableRendering], scope: DigestScope) -> String {
    let payload = entries
        .iter()
        .filter(|entry| scope.binds(entry.kind))
        .map(|entry| {
            format!(
                "{}\0{}\0{}\0{}",
                entry.module, entry.kind as u8, entry.name, entry.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\0\n");
    marrow_project::sha256_digest(payload.as_bytes())
}

/// One digest-bound declaration's normalized rendering, with the keys that order it
/// deterministically: its module, declaration kind, and declaration name.
struct DurableRendering {
    module: String,
    kind: DurableKind,
    name: String,
    text: String,
}

/// The declaration kinds whose shape or transform-visible value a stored snapshot
/// must satisfy. The discriminant orders renderings deterministically within a module;
/// an evolve block carries no name, so its kind alone keeps it last.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DurableKind {
    Resource = 0,
    Store = 1,
    Enum = 2,
    Const = 3,
    Evolve = 4,
    Unreadable = 5,
}

/// The digest kind of a declaration, or `None` for a function. Transform bodies cannot
/// call user functions, but they can read module constants.
fn durable_kind(declaration: &marrow_syntax::Declaration) -> Option<DurableKind> {
    match declaration {
        marrow_syntax::Declaration::Resource(_) => Some(DurableKind::Resource),
        marrow_syntax::Declaration::Store(_) => Some(DurableKind::Store),
        marrow_syntax::Declaration::Enum(_) => Some(DurableKind::Enum),
        marrow_syntax::Declaration::Const(_) => Some(DurableKind::Const),
        marrow_syntax::Declaration::Evolve(_) => Some(DurableKind::Evolve),
        marrow_syntax::Declaration::Function(_) => None,
    }
}

/// The ordering name for a durable declaration: its declared name, the store root, or
/// the empty string for a nameless evolve block. The normalized text disambiguates
/// equal names, so this only needs a stable within-module sort key.
fn declaration_name(declaration: &marrow_syntax::Declaration) -> String {
    match declaration {
        marrow_syntax::Declaration::Resource(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Store(decl) => decl.root.root.clone(),
        marrow_syntax::Declaration::Enum(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Const(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Evolve(_) | marrow_syntax::Declaration::Function(_) => {
            String::new()
        }
    }
}
