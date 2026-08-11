//! Exact-symbol absence gates over `marrow-image/src`: shapes this crate has
//! deleted must not reappear, in production code or in its own test tier.
//!
//! Each scan runs over the literal-stripped projection of the source, so a shape
//! spelled inside a comment or a string is not mistaken for the real thing. That
//! projection has exactly one owner in the workspace and is included here rather
//! than copied.

use std::fs;
use std::path::{Path, PathBuf};

#[path = "../../marrow-compile/tests/common/source_projection.rs"]
mod source_projection;
use source_projection::without_literals;

fn src_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    assert!(!files.is_empty(), "the source tree is scanned");
    files
}

/// Every `(file, line)` at which `needle` appears in code — comments and string
/// literals blanked, `#[cfg(test)]` items deliberately retained, since a deleted
/// relationship may not survive in a test either.
fn occurrences(needle: &str) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for path in src_files() {
        let code = without_literals(&fs::read_to_string(&path).expect("read source file"));
        for (index, line) in code.lines().enumerate() {
            if line.contains(needle) {
                found.push((path.clone(), index + 1));
            }
        }
    }
    found
}

/// The root count and the record-type count are independent bounds, and neither may
/// be derived from the other again.
///
/// `MAX_ROOTS` bounds root *occurrences*; `MAX_TYPES` bounds the *type population*.
/// The deleted derivation read "each root's resource is a record type, so the type
/// table bounds the root count" — true only while every root occurrence carried its
/// own record type. Many roots may occur over one Product declaration, contributing
/// one record type between them, so the implication no longer holds; and it never
/// held downward, since declarations and monomorphization grow the type population
/// with no durable root at all. Restoring either the compile-time derivation or the
/// equality known-answer test would silently couple two independently justified
/// ceilings, so that a widening of one inherited the other's evidence.
#[test]
fn no_root_count_to_type_count_derivation_exists() {
    for needle in [
        "MAX_ROOTS <= MAX_TYPES",
        "MAX_TYPES >= MAX_ROOTS",
        "MAX_ROOTS, MAX_TYPES",
        "MAX_TYPES, MAX_ROOTS",
    ] {
        let found = occurrences(needle);
        assert!(
            found.is_empty(),
            "`{needle}` re-derives one bound from the other; each carries its own \
             evidence: {found:?}",
        );
    }
}

/// A gate that cannot see its own subject passes for the wrong reason.
#[test]
fn the_scan_sees_code_and_not_prose() {
    let planted = without_literals(
        r##"
        // assert!(MAX_ROOTS <= MAX_TYPES);
        const DOC: &str = "MAX_ROOTS <= MAX_TYPES";
        const RAW: &str = r#"MAX_ROOTS <= MAX_TYPES"#;
        const LIVE: bool = MAX_ROOTS <= MAX_TYPES;
        "##,
    );
    let hits: Vec<&str> = planted
        .lines()
        .filter(|line| line.contains("MAX_ROOTS <= MAX_TYPES"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the code occurrence is visible to the scan: {hits:?}",
    );
    assert!(
        hits[0].contains("const LIVE"),
        "the visible occurrence is the code one: {hits:?}",
    );
}

/// No site id is ever a narrowed table length, and no second site mint path exists.
///
/// The site table was appended to directly, its id taken as `self.sites.len() as u16`,
/// with the bound seen only at `encode()`. A producer could request past `u16::MAX`
/// distinct durable nodes, receive a wrapped id, and hand two distinct nodes one site
/// operand. The bounded plan mints only after checking vacant capacity, so restoring
/// either the raw append or a length-narrowing cast would reopen the aliasing.
///
/// The length-narrowing needles are scanned over the **mint owners** — the draft and its
/// plan. Elsewhere a narrowed length is an ordinary encoded count: the DURABLE section
/// writes the site table's length as its own `u16` wire count, which is not an id and
/// never reaches a site operand.
#[test]
fn no_length_narrowing_site_mint_path_exists() {
    let mint_owners: Vec<String> = ["draft.rs", "site_plan.rs"]
        .into_iter()
        .map(|file| {
            let source =
                fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file))
                    .expect("read a site mint owner");
            without_literals(&source)
        })
        .collect();
    for needle in ["sites.len() as u16", "rows.len() as u16"] {
        assert!(
            !mint_owners.iter().any(|code| code.contains(needle)),
            "`{needle}` narrows a table length into a site id again",
        );
    }
    for needle in ["fn add_site", "fn alloc_site", ".add_site(", ".alloc_site("] {
        let hits = occurrences(needle);
        assert!(
            hits.is_empty(),
            "`{needle}` is a deleted second site mint path; found at {hits:?}",
        );
    }
    assert!(
        mint_owners
            .iter()
            .any(|code| code.contains("u16::try_from(self.rows.len())")),
        "the checked conversion the needles stand against is present, so this gate has a \
         live subject",
    );
}

/// The whole `as u16` family on the site path, in every spelling a length could take.
/// The plan's own conversions are `u16::try_from`, so any `as u16` reached from a length
/// is the deleted shape returning.
#[test]
fn the_site_plan_narrows_no_length_with_an_as_cast() {
    let plan =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/site_plan.rs"))
            .expect("read the site plan");
    let code = without_literals(&plan);
    assert!(
        !code.contains("as u16"),
        "the site plan converts with `u16::try_from`, never an `as` cast",
    );
    assert!(
        code.contains("u16::try_from"),
        "the plan's checked conversion is present, so this gate has a live subject",
    );
}

/// The planted-probe half of the two gates above: each needle must be visible to the
/// scan in code and invisible in prose, or the gates pass for the wrong reason.
#[test]
fn the_site_scan_sees_code_and_not_prose() {
    let planted = without_literals(
        r##"
        // let id = self.sites.len() as u16;
        const DOC: &str = "sites.len() as u16";
        const RAW: &str = r#"sites.len() as u16"#;
        let live = self.sites.len() as u16;
        "##,
    );
    let hits: Vec<&str> = planted
        .lines()
        .filter(|line| line.contains("sites.len() as u16"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the code occurrence is visible to the scan: {hits:?}",
    );
    assert!(
        hits[0].contains("let live"),
        "the visible occurrence is the code one: {hits:?}",
    );
}

/// The encoder projects a Product declaration from its flat rows, never from a member
/// tree.
///
/// The wire bytes and the durable contract id must derive from one set of facts. While
/// the declaration was a recursive `Vec<DurableMemberDef>`, the encoder walked that tree
/// three times — to emit the DURABLE section, to build the contract descriptor, and to
/// recheck the member bounds — and any later owner could hold a second tree beside the
/// rows without either walk noticing. Reintroducing the tree in the encoder is exactly
/// that divergence returning.
#[test]
fn the_encoder_reads_declaration_rows_and_no_member_tree() {
    let encoder =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/encode.rs"))
            .expect("read the encoder");
    let code = without_literals(&encoder);
    assert!(
        !code.contains("DurableMemberDef"),
        "the encoder projects from `DeclarationNode` rows, never from a member tree",
    );
    assert!(
        code.contains("DeclarationMemberShape"),
        "the encoder's row projection is present, so this gate has a live subject",
    );
    let tree_walkers = occurrences("fn validate_member_tree");
    assert!(
        tree_walkers.is_empty(),
        "the member-bound recheck is one forward pass over the rows; found {tree_walkers:?}",
    );
}

/// No draft instruction carries a bare `u16` operation site.
///
/// A site operand used to be a plain number, so any producer could write one by hand, a
/// refused site had to travel through the same numeric channel as a real one, and two
/// unrelated numbers could be compared as if they addressed the same place. Every `Dur*`
/// site operand is now the opaque [`marrow_image::LegacyDraftSiteOperand`], whose sole
/// mint is the bounded site demand plan. A `Dur*` variant reverting to `u16` is that
/// forgeable channel returning.
#[test]
fn no_draft_instruction_carries_a_bare_u16_site() {
    let instructions =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/instr.rs"))
            .expect("read the instruction set");
    let code = without_literals(&instructions);

    assert!(
        bare_u16_site_declarations(&code).is_empty(),
        "a durable instruction names a bare `u16` site: {:?}",
        bare_u16_site_declarations(&code),
    );
    assert!(
        code.contains("DurReadField(LegacyDraftSiteOperand)"),
        "the opaque site operand is present, so this gate has a live subject",
    );
}

/// The name of every durable operation-site instruction variant in `code` whose
/// declaration names a bare `u16` site, in either shape the variants use: a positional
/// operand, or a named `site` field inside a braced variant.
///
/// A braced variant spans several lines, so the scan accumulates each variant's whole
/// declaration — to the point where its parentheses and braces close — rather than testing
/// lines independently. The temporal `Duration*` operations share the prefix and carry no
/// site, so they are not durable-site variants and are excluded by name.
fn bare_u16_site_declarations(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current: Option<(String, String)> = None;
    let mut depth = 0usize;
    for line in code.lines().map(str::trim) {
        if depth == 0
            && current.is_none()
            && line.starts_with("Dur")
            && !line.starts_with("Duration")
        {
            let name = line
                .split(['(', ' ', ',', '{'])
                .next()
                .expect("a variant line names its variant")
                .to_string();
            current = Some((name, String::new()));
        }
        let Some((_, span)) = current.as_mut() else {
            continue;
        };
        span.push_str(line);
        span.push('\n');
        depth = depth + line.matches(['(', '{']).count() - line.matches([')', '}']).count();
        if depth == 0 {
            let (name, span) = current.take().expect("a variant is being accumulated");
            if span.contains("(u16)") || span.contains("site: u16") {
                found.push(name);
            }
        }
    }
    found
}

/// The bare-`u16` scan detects both declaration shapes, and is not satisfied merely by
/// finding nothing.
///
/// A reverted variant would not compile against the rest of the crate, so the gate above
/// can never observe the defect it forbids in a running test. This plants it instead: the
/// same predicate the gate uses is run over source that does carry the defect.
#[test]
fn the_bare_u16_site_scan_detects_a_planted_declaration() {
    let planted = concat!(
        "    DurReadField(u16),\n",
        "    DurIterateBounded {\n",
        "        site: u16,\n",
        "        limit: u32,\n",
        "    },\n",
        "    DurReadEntry(LegacyDraftSiteOperand),\n",
    );

    assert_eq!(
        bare_u16_site_declarations(planted),
        ["DurReadField", "DurIterateBounded"],
        "the scan flags a positional and a braced named bare site, and passes the opaque one",
    );
}

/// The site operand is never `Copy`.
///
/// A minted site is an answer the plan gave to one demand. Copying one implicitly is how
/// a carrier acquires a site it never requested — the defect the whole operand exists to
/// make unrepresentable — so the type is `Clone` and moved deliberately.
#[test]
fn the_site_operand_does_not_derive_copy() {
    let plan =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/site_plan.rs"))
            .expect("read the site plan");
    let code = without_literals(&plan);
    let declaration = code
        .split("pub struct LegacyDraftSiteOperand")
        .next()
        .expect("the operand is declared")
        .rsplit("#[derive(")
        .next()
        .expect("the operand carries a derive list");

    assert!(
        !declaration.contains("Copy"),
        "the site operand must not derive Copy: `#[derive({declaration}`",
    );
}

/// Whether `code` contains `needle` as a whole identifier rather than as a fragment of a
/// longer one. `add_root` must not match `add_root_occurrence`: a gate that cannot tell a
/// deleted symbol from its replacement fires on the replacement and is then relaxed.
fn contains_symbol(code: &str, needle: &str) -> bool {
    fn is_ident(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_'
    }
    let bytes = code.as_bytes();
    code.match_indices(needle).any(|(at, _)| {
        let before = at
            .checked_sub(1)
            .is_none_or(|index| !is_ident(bytes[index]));
        let after = bytes
            .get(at + needle.len())
            .is_none_or(|byte| !is_ident(*byte));
        before && after
    })
}

/// Every `.rs` file under one directory name of every crate in the workspace, with its
/// literal-stripped code projection. The seam's own gates are about where it is *called
/// from*, so they cannot be answered by scanning this crate alone.
fn workspace_sources(tier: &str) -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crates directory");
    let mut files = Vec::new();
    for entry in fs::read_dir(crates).expect("read the crates directory") {
        walk(&entry.expect("dir entry").path().join(tier), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 20,
        "the whole workspace `{tier}` tree is scanned, not one crate",
    );
    files
        .into_iter()
        .map(|path| {
            let code = without_literals(&fs::read_to_string(&path).expect("read source file"));
            (path, code)
        })
        .collect()
}

/// The deleted raw durable-graph builder family is gone from the whole workspace, in
/// production sources and in the test tier alike — a deleted relationship that survives
/// under its old name in a fixture is the shape coming back by another door.
///
/// `RootIdentity`, `RootDef`, `DurableMemberDef`, and `ImageDraft::add_root` let any
/// caller hand the draft a recursive member tree with no validation, and were the only
/// construction path a deliberately compiler-free test tier had. The checked flat
/// construction seam replaced them: it states nesting by parent ordinal, so it cannot
/// express a tree, and it validates every command before a row is appended. Any of these
/// names reappearing is that unvalidated recursive channel returning.
#[test]
fn no_raw_durable_graph_builder_survives() {
    let sources = [workspace_sources("src"), workspace_sources("tests")].concat();
    for needle in [
        "RootIdentity",
        "RootDef",
        "DurableMemberDef",
        "add_root",
        "add_site",
        "alloc_site",
        "product_member_tree",
    ] {
        let found: Vec<String> = sources
            .iter()
            .filter(|(_, code)| contains_symbol(code, needle))
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            found.is_empty(),
            "`{needle}` is a deleted raw durable-graph construction symbol: {found:?}",
        );
    }
}

/// The site demand plan retains no semantic path.
///
/// A retained `SemanticPath` per site row is a second copy of the durable graph: it is an
/// unbounded-length key, one heap allocation per row at `MAX_SITES`, and — decisively — a
/// copy that can drift from the declaration rows the wire bytes and the contract id are
/// written from. The plan retains three owned ordinals and the path is projected from
/// them at encode by the one path owner. A path type appearing in the plan is that second
/// copy returning.
#[test]
fn the_site_plan_retains_no_semantic_path() {
    let plan = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("site_plan.rs"),
    )
    .expect("read the site plan");
    let code = without_literals(&plan);
    for needle in ["SemanticPath", "SemanticStep", "steps"] {
        assert!(
            !contains_symbol(&code, needle),
            "`{needle}` in the site demand plan retains a path beside the demand key",
        );
    }
}

/// The checked flat construction seam has no production caller beyond the compiler's one
/// durable owner and the verifier's code decoder.
///
/// The seam exists for a deliberately compiler-free test tier and for the compiler's own
/// construction. A third production caller would make it a second construction path, and
/// it is a declared absorb-and-delete target of the admitted graph input plan: every
/// caller must be migrated in that one transaction, so the caller set is pinned here.
#[test]
fn the_construction_seam_has_no_unlisted_production_caller() {
    // The compiler's durable owner spans the store builder and the lowering boundary
    // where a field-leaf site is first demanded; the verifier's code decoder is the one
    // other production caller. Widening this set is the thing the gate exists to notice.
    const PERMITTED: [&str; 5] = [
        "marrow-compile/src/durable.rs",
        "marrow-compile/src/lower/mod.rs",
        "marrow-compile/src/lower/durable.rs",
        "marrow-compile/src/lower/exprs.rs",
        "marrow-verify/src/verify/decode_code.rs",
    ];
    let sources = workspace_sources("src");
    for needle in [
        "declare_product",
        "add_root_occurrence",
        "bind_occurrence_site",
        "request_site",
        "product_members",
    ] {
        let found: Vec<String> = sources
            .iter()
            .filter(|(path, code)| {
                contains_symbol(code, needle)
                    && !path.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
                    && !PERMITTED
                        .iter()
                        .any(|permitted| path.to_string_lossy().contains(permitted))
            })
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            found.is_empty(),
            "`{needle}` gained a production caller outside the pinned seam caller set: \
             {found:?}",
        );
    }
}

/// No producer-side site-path length refusal survives.
///
/// A site path used to be a value a producer wrote by hand, so the encoder had to refuse
/// one that was too short to name a node or nested past the depth bound. A site is now a
/// binding of a live occurrence row to a live canonical declaration path, and its path is
/// projected from those rows: it is always the two root steps plus that node's own nesting
/// depth, which `validate_declaration_graph` has already bounded. Restoring either variant
/// would mean a producer can again spell a path the graph does not contain.
#[test]
fn no_producer_side_site_path_length_refusal_exists() {
    let sources = [workspace_sources("src"), workspace_sources("tests")].concat();
    for needle in ["SitePathTooShort", "SitePathTooDeep"] {
        let found: Vec<String> = sources
            .iter()
            .filter(|(_, code)| contains_symbol(code, needle))
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            found.is_empty(),
            "`{needle}` is a structurally unreachable refusal this row deleted: {found:?}",
        );
    }
}

/// The planted-probe half of the gates above: each needle must be visible to the scan in
/// code and invisible in prose, or the gates pass for the wrong reason.
#[test]
fn the_seam_scan_sees_code_and_not_prose() {
    for needle in [
        "RootIdentity",
        "add_root",
        "SitePathTooShort",
        "SemanticPath",
        "bind_occurrence_site",
    ] {
        let planted = without_literals(&format!(
            "
        // let a = {needle};
        const DOC: &str = \"{needle}\";
        const RAW: &str = r#\"{needle}\"#;
        let live = {needle};
        "
        ));
        let hits: Vec<&str> = planted
            .lines()
            .filter(|line| line.contains(needle))
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "exactly the code occurrence of `{needle}` is visible to the scan: {hits:?}",
        );
        assert!(
            hits[0].contains("let live"),
            "the visible occurrence of `{needle}` is the code one: {hits:?}",
        );
    }
}

/// Every instruction that carries an operation site is answered by the one accessor the
/// checked function append reads.
///
/// `Instr::site_operand` is where a site operand's evidence is spent: appending a function
/// validates each operand it returns, so an instruction the accessor does not answer for
/// carries an operand nothing checks — exactly the unvalidated channel the opaque operand
/// replaced. The accessor ends in a wildcard, because listing the hundred-odd instructions
/// that carry no site would bury the ones that do; this counts both sides instead, so a
/// site-bearing instruction added without extending the accessor is conspicuous.
#[test]
fn every_site_bearing_instruction_is_answered_by_the_site_accessor() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("instr.rs"),
    )
    .expect("read the instruction set");
    let code = without_literals(&source);
    let (declarations, accessor) = code
        .split_once("fn site_operand")
        .expect("the site accessor is present, so this gate has a live subject");
    let body = declarations
        .split_once("pub enum Instr")
        .expect("the instruction set is present")
        .1;

    // A variant declaration is a name at one level of indentation inside the enum; the
    // operand may sit on that same line (a tuple variant) or on a field line below it (a
    // struct variant), so the scan carries the name forward until the next one.
    let mut declared: Vec<&str> = Vec::new();
    let mut current = "";
    for line in body.lines() {
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() == 4
            && trimmed.starts_with(|first: char| first.is_ascii_uppercase())
        {
            current = trimmed
                .split(['(', '{', ',', ' '])
                .next()
                .expect("a variant line has a name");
        }
        if line.contains("LegacyDraftSiteOperand") && !current.is_empty() {
            declared.push(current);
        }
    }
    declared.sort_unstable();
    declared.dedup();

    assert!(
        declared.len() > 10,
        "the scan sees the instruction set's site-bearing variants: {declared:?}",
    );
    let unanswered: Vec<&&str> = declared
        .iter()
        .filter(|variant| !accessor.contains(&format!("Instr::{variant}")))
        .collect();
    assert!(
        unanswered.is_empty(),
        "these instructions carry an operation site that `Instr::site_operand` does not \
         answer for, so an appended function never validates it: {unanswered:?}",
    );
}
