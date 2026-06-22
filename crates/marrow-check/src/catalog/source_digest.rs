use crate::CheckedProgram;

/// A stable `sha256:<hex>` digest of the program's durable shape: each `resource`, `store`,
/// `enum`, and module `const`. This is what the store stamps at commit and the
/// activation-window fence enforces, so it binds exactly the facts a stored snapshot must
/// satisfy.
///
/// The `evolve` block is excluded: a consumed block describes work already recorded in the
/// accepted catalog, so hashing it would read its deletion as schema drift; the fence tracks
/// the durable shape, not the transition that produced it.
///
/// Hashing the canonical formatter's rendering rather than the fields means a shape change
/// drifts the digest while a whitespace reformat does not, which makes the formatter a frozen
/// anchor: a formatter change that moves the text for an unchanged shape must be handled as a
/// store-format decision, not silently re-read as drift over every committed snapshot.
pub(crate) fn analyzed_source_digest(program: &CheckedProgram) -> String {
    digest_of(&render_declarations(program), DigestScope::Shape)
}

/// Both digests from one captured rendering set, so a caller that needs both (the evolution
/// preview witness) hashes the checked program's in-memory source renderings once.
pub(crate) fn source_and_evolution_digests(program: &CheckedProgram) -> (String, String) {
    let renderings = render_declarations(program);
    (
        digest_of(&renderings, DigestScope::Shape),
        digest_of(&renderings, DigestScope::ShapeAndEvolve),
    )
}

/// A stable `sha256:<hex>` digest of the shape plus the evolve decision surface: everything
/// [`analyzed_source_digest`] binds plus each `evolve` block, so a changed default value or
/// transform body drifts it.
///
/// The evolution witness records this digest, not the shape digest, so apply aborts when the
/// source it activates no longer matches what was discharged — including a transform-body edit
/// the shape digest cannot see. The store fences on shape so a consumed block is deletable;
/// the witness fences on shape-plus-intent so the preview-to-apply transition cannot silently
/// change.
pub(crate) fn evolution_digest(program: &CheckedProgram) -> String {
    digest_of(&render_declarations(program), DigestScope::ShapeAndEvolve)
}

/// The durable identity of one `evolve transform`: a `sha256:<hex>` of its target's stable
/// catalog id and its canonical body rendering. Apply stamps this on the target so a
/// re-bind recognizes the same transform, and discharge compares against it to skip a
/// transform already applied. Keying on the transform's own target and body — not the
/// whole-program shape — means an unrelated durable edit never moves the mark, so a
/// discharged transform cannot re-execute and corrupt already-migrated data, while a
/// changed body computes a different identity and is correctly a fresh obligation.
pub(crate) fn transform_identity(stable_id: &str, body_text: &str) -> String {
    marrow_project::sha256_digest(format!("transform-v1\0{stable_id}\0{body_text}").as_bytes())
}

/// Which declarations a digest binds. The shape digest the store stamps excludes the
/// evolve block; the evolution digest the witness records includes it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DigestScope {
    Shape,
    ShapeAndEvolve,
}

impl DigestScope {
    fn binds(self, kind: DurableKind) -> bool {
        match self {
            DigestScope::Shape => kind != DurableKind::Evolve,
            DigestScope::ShapeAndEvolve => true,
        }
    }
}

/// Every durable declaration rendered in deterministic order, the single pass each scope's
/// digest hashes a subset of. Each module's renderings are captured while the checker still
/// holds the in-memory source text and parse.
fn render_declarations(program: &CheckedProgram) -> Vec<DurableRendering> {
    let mut entries: Vec<DurableRendering> = Vec::new();
    for (module_index, module) in program.modules.iter().enumerate() {
        let module_index = module_index as u32;
        assert!(
            program
                .facts
                .has_captured_durable_digest_renderings_for_module_index(module_index),
            "checked program is missing captured durable source renderings for module `{}`",
            module.name
        );
        for rendering in program
            .facts
            .durable_digest_renderings_for_module_index(module_index)
        {
            entries.push(rendering.clone());
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

/// One declaration's normalized rendering, with the `(module, kind, name)` keys that order it
/// deterministically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableRendering {
    module_index: u32,
    module: String,
    kind: DurableKind,
    name: String,
    text: String,
}

impl DurableRendering {
    pub(crate) fn module_index(&self) -> u32 {
        self.module_index
    }
}

pub(crate) fn durable_renderings_for_source(
    module_index: u32,
    module: &str,
    source: &str,
    parsed: &marrow_syntax::ParsedSource,
) -> Vec<DurableRendering> {
    parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| {
            durable_kind(declaration).map(|kind| DurableRendering {
                module_index,
                module: module.to_string(),
                kind,
                name: declaration_name(declaration),
                text: marrow_syntax::format_declaration(source, declaration),
            })
        })
        .collect()
}

/// The declaration kinds a stored snapshot must satisfy. The discriminant orders renderings
/// deterministically within a module; an evolve block carries no name, so its kind alone keeps
/// it last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableKind {
    Resource = 0,
    Store = 1,
    Enum = 2,
    Const = 3,
    Evolve = 4,
}

/// The digest kind of a declaration, or `None` for source that does not change
/// durable shape.
fn durable_kind(declaration: &marrow_syntax::Declaration) -> Option<DurableKind> {
    match declaration {
        marrow_syntax::Declaration::Resource(_) => Some(DurableKind::Resource),
        marrow_syntax::Declaration::Store(_) => Some(DurableKind::Store),
        marrow_syntax::Declaration::Enum(_) => Some(DurableKind::Enum),
        marrow_syntax::Declaration::Const(_) => Some(DurableKind::Const),
        marrow_syntax::Declaration::Evolve(_) => Some(DurableKind::Evolve),
        marrow_syntax::Declaration::Function(_) | marrow_syntax::Declaration::Surface(_) => None,
    }
}

/// The within-module sort key for a declaration: its declared name, the store root, or the
/// empty string for a nameless evolve block. Equal `(module, kind, name)` keys (such as multiple
/// nameless evolve blocks) retain their source declaration order through the stable sort; the
/// rendered text is hashed but does not participate in ordering.
fn declaration_name(declaration: &marrow_syntax::Declaration) -> String {
    match declaration {
        marrow_syntax::Declaration::Resource(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Store(decl) => decl.root.root.clone(),
        marrow_syntax::Declaration::Enum(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Const(decl) => decl.name.clone(),
        marrow_syntax::Declaration::Evolve(_)
        | marrow_syntax::Declaration::Function(_)
        | marrow_syntax::Declaration::Surface(_) => String::new(),
    }
}
