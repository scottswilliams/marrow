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
use source_projection::{is_ident_byte, production_code, without_cfg_test_items, without_literals};

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
/// site operand is now the opaque [`marrow_image::PlannedSiteRef`], whose sole
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
        code.contains("DurReadField(PlannedSiteRef)"),
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
        "    DurReadEntry(PlannedSiteRef),\n",
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
        .split("pub struct PlannedSiteRef")
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
    symbol_positions(code, needle).next().is_some()
}

/// Every byte offset at which `needle` appears in `code` as a whole identifier. A gate that
/// needs to look at what surrounds a symbol — the token before it, the file it sits in —
/// takes the offsets from here rather than re-deciding what an identifier boundary is.
fn symbol_positions<'a>(code: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    let bytes = code.as_bytes();
    code.match_indices(needle).filter_map(move |(at, _)| {
        let before = at
            .checked_sub(1)
            .is_none_or(|index| !is_ident_byte(bytes[index]));
        let after = bytes
            .get(at + needle.len())
            .is_none_or(|byte| !is_ident_byte(*byte));
        (before && after).then_some(at)
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

/// The legacy site operand and its numeric escape are gone from the whole workspace, in
/// production sources and in the test tier alike.
///
/// `LegacyDraftSiteOperand` was the pre-transaction site carrier and `encodable` its
/// direct ref-to-number channel, callable anywhere in the crate. The transferred
/// [`marrow_image::PlannedSiteRef`] is the one carrier, and its wire ordinal exists only
/// through the measured plan's site projection on the policy-clean fitting arm. Either
/// name reappearing is that pre-measurement numeric channel returning.
#[test]
fn the_legacy_site_operand_and_its_numeric_escape_are_absent() {
    let sources = [workspace_sources("src"), workspace_sources("tests")].concat();
    assert!(
        sources.iter().any(|(path, code)| {
            path.ends_with("marrow-image/src/site_plan.rs")
                && contains_symbol(code, "PlannedSiteRef")
        }),
        "the transferred carrier is present, so this gate has a live subject",
    );
    for needle in ["LegacyDraftSiteOperand", "encodable"] {
        let found: Vec<String> = sources
            .iter()
            .filter(|(_, code)| contains_symbol(code, needle))
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            found.is_empty(),
            "`{needle}` is a retired site-carrier symbol: {found:?}",
        );
    }
}

/// The checked site wire projection has exactly its two spelled homes: the draft-side
/// definition and the measured plan's projection wrapper.
///
/// `site_wire_ordinal` is the one ref-to-number conversion left after `encodable` was
/// deleted, and it is meant to be reachable only through the plan-bound
/// `SiteWireProjection` the measured wire plan mints. A third caller is a numeric site
/// id escaping the policy-clean fitting arm.
#[test]
fn the_site_wire_projection_has_exactly_its_two_spelled_homes() {
    let mut homes = Vec::new();
    for (path, code) in workspace_sources("src") {
        if contains_symbol(&code, "site_wire_ordinal") {
            homes.push(path.display().to_string());
        }
    }
    let expected: Vec<bool> = homes
        .iter()
        .map(|path| {
            path.ends_with("marrow-image/src/draft.rs")
                || path.ends_with("marrow-image/src/measure.rs")
        })
        .collect();
    assert_eq!(
        homes.len(),
        2,
        "the projection is defined once and consumed once: {homes:?}",
    );
    assert!(
        expected.iter().all(|home| *home),
        "the projection's homes are the draft definition and the plan projection: {homes:?}",
    );
}

/// The measured wire plan is closed: it has exactly one constructor with exactly one
/// caller (the counting run), and no post-plan path can surface `ImageTooLarge`.
///
/// The plan is the witness that the whole image fits, so a second constructor would
/// be a forgeable witness and an `ImageTooLarge` result behind a minted plan would
/// be a policy verdict issued after the policy decision closed — the emission side
/// may only report the invariant-class encode drift.
#[test]
fn the_wire_plan_is_closed_and_no_post_plan_path_is_image_too_large() {
    let measure = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("measure.rs"),
    )
    .expect("read the measure core");
    let code = without_literals(&measure);
    let constructors: Vec<usize> = symbol_positions(&code, "new")
        .filter(|at| code[..*at].ends_with("LegacyV0WirePlan::"))
        .collect();
    assert_eq!(
        constructors.len(),
        1,
        "the plan has exactly one constructor call (the counting run's)",
    );
    let (_, tail) = code
        .split_once("fn emit_image")
        .expect("the emission entry point is present, so this gate has a live subject");
    // The emission body runs to the decisive-sentinel const that follows the plan's
    // impl block; the counting sink defined after it is pre-plan machinery.
    let (emission, _) = tail
        .split_once("DECISIVE_TOTAL")
        .expect("the decisive sentinel bounds the emission body");
    let post_plan_too_large: Vec<usize> = symbol_positions(emission, "ImageTooLarge").collect();
    assert!(
        post_plan_too_large.is_empty(),
        "no post-plan emission path surfaces ImageTooLarge",
    );
    assert!(
        contains_symbol(&code, "ImageTooLarge"),
        "the pre-plan ceiling verdict is present, so this gate has a live subject",
    );

    // Exactly: every site in the crate that can *construct* the ceiling verdict is
    // enumerated, because counting constructor calls in one file says nothing about a
    // constructor definition in another. Scanning only `measure.rs` left `encode.rs`'s
    // declaration-member walk free to map an over-wide arity to `ImageTooLarge` behind a
    // minted plan, which is what it did.
    let found = image_too_large_sites();
    let expected: Vec<(String, String)> = PRE_PLAN_IMAGE_TOO_LARGE_SITES
        .iter()
        .map(|(file, line)| (file.to_string(), line.to_string()))
        .collect();
    assert_eq!(
        found, expected,
        "the set of sites that can produce the whole-image ceiling verdict moved. Every \
         one must sit before the plan is minted: behind a minted plan the ceiling \
         decision is already closed, so the emission side may only report \
         invariant-class encode drift.",
    );
}

/// Every site in this crate's production source that names the ceiling verdict, keyed by
/// file and exact normalized source line.
fn image_too_large_sites() -> Vec<(String, String)> {
    let mut sites = Vec::new();
    for path in src_files() {
        let rel = src_relative(&path);
        let code = without_cfg_test_items(&without_literals(
            &fs::read_to_string(&path).expect("read image source"),
        ));
        for line in code.lines() {
            if contains_symbol(line, "ImageTooLarge") {
                sites.push((
                    rel.clone(),
                    line.split_whitespace().collect::<Vec<_>>().join(" "),
                ));
            }
        }
    }
    sites.sort();
    sites
}

fn src_relative(path: &Path) -> String {
    path.display()
        .to_string()
        .split_once("src/")
        .expect("a src path")
        .1
        .to_string()
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

/// Nothing in production mints an [`marrow_image::AdmittedGraphInputPlan`] outside the one
/// admission owner.
///
/// Entering the durable graph now requires a plan, so the question the old caller allowlist
/// asked — who may construct — is answered by who may *mint*. A second minter would be a
/// second admission owner, free to state a census no store declaration backs, and every
/// entry point downstream would accept it. The compiler's store-declaration census is that
/// one owner; the verifier's structural preflight is the other, and it mints from received
/// bytes rather than from anything the compiler asserted.
#[test]
fn no_production_file_mints_a_graph_input_plan_outside_its_admission_owner() {
    let found: Vec<String> = plan_minting_sources()
        .into_iter()
        .filter(|path| {
            !PERMITTED_PLAN_MINTERS
                .iter()
                .any(|permitted| path.contains(permitted))
        })
        .collect();
    assert!(
        found.is_empty(),
        "a production file mints a durable-graph construction budget outside the pinned \
         admission owner set: {found:?}",
    );
}

/// Every production source that mints a construction budget.
///
/// A minter is a file that both names the plan type and states one of its minting entry
/// points. Requiring both is what makes the scan precise without being alias-blind: `admit`
/// alone is a common verb across the workspace's admission owners, while a file that mints
/// a plan — under any local alias — must name the type to import it.
fn plan_minting_sources() -> Vec<String> {
    let owner = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    workspace_sources("src")
        .into_iter()
        .filter(|(path, code)| {
            // Production only: a decode fixture that states a census for a draft it builds
            // itself is not an admission owner, and the gate's subject is who may mint
            // where a real graph is constructed.
            let production = without_cfg_test_items(code);
            !path.starts_with(&owner)
                && contains_symbol(&production, "AdmittedGraphInputPlan")
                && PLAN_MINTING_ENTRY_POINTS
                    .iter()
                    .any(|needle| contains_symbol(&production, needle))
        })
        .map(|(path, _)| path.display().to_string())
        .collect()
}

/// The staleness half of the minter gate: every permitted path must still mint a plan.
///
/// An allowlist entry that names no minter is a pre-opened door — it permits a future
/// widening into that file silently, and it is exactly what the gate above cannot see,
/// because a row that matches nothing never fails. An owner that stops minting must take
/// its row with it.
#[test]
fn every_permitted_plan_minter_still_mints_one() {
    let minters = plan_minting_sources();
    for permitted in PERMITTED_PLAN_MINTERS {
        assert!(
            minters.iter().any(|path| path.contains(permitted)),
            "`{permitted}` is a permitted plan minter that mints nothing: a dead allowlist \
             row pre-permits a widening the minter gate cannot see",
        );
    }
}

/// A construction budget is minted, never spelled: no source anywhere states an
/// [`marrow_image::AdmittedGraphInputPlan`] by struct literal or reaches its count fields.
///
/// The counts are private, so this cannot compile outside the owning module today. The gate
/// is here because the *reason* is not local: if the fields were ever loosened for a
/// convenience constructor, every checked ceiling in `admit` would become optional, and the
/// loosening would read as a small visibility edit rather than as the removal of the teeth.
#[test]
fn no_source_spells_a_graph_input_plan_by_literal() {
    let sources = [workspace_sources("src"), workspace_sources("tests")].concat();
    let owner = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found: Vec<String> = sources
        .iter()
        .filter(|(path, code)| !path.starts_with(&owner) && spells_a_plan_literal(code))
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert!(
        found.is_empty(),
        "an admitted graph input plan is spelled by literal rather than minted: {found:?}",
    );
}

/// The planted-probe half of the two minter gates: the discriminators must see a real mint
/// and a real literal, and must not see the same words in prose or in ordinary type
/// mentions. A gate whose discriminator answers "no" to everything passes for the wrong
/// reason.
#[test]
fn the_plan_scan_separates_a_mint_and_a_literal_from_a_mention() {
    let mint = without_literals(
        "use marrow_image::AdmittedGraphInputPlan;\nlet p = AdmittedGraphInputPlan::admit(1, 1, 1);",
    );
    assert!(
        contains_symbol(&mint, "AdmittedGraphInputPlan") && contains_symbol(&mint, "admit"),
        "the minter gate's discriminator must see a real mint",
    );

    let mention = without_literals("fn f(plan: &AdmittedGraphInputPlan) -> usize { 0 }");
    assert!(
        contains_symbol(&mention, "AdmittedGraphInputPlan") && !contains_symbol(&mention, "admit"),
        "naming the plan as a parameter type is not minting one",
    );

    let prose = without_literals("// an AdmittedGraphInputPlan is what admit() answers with\n");
    assert!(
        !contains_symbol(&prose, "AdmittedGraphInputPlan") && !contains_symbol(&prose, "admit"),
        "prose about minting is not minting",
    );

    assert!(
        spells_a_plan_literal(&without_literals(
            "let p = AdmittedGraphInputPlan { products: 1, roots: 1, commands: 1 };",
        )),
        "the literal gate's discriminator must see a real struct literal",
    );
    assert!(
        !spells_a_plan_literal(&mention),
        "a parameter type is not a struct literal",
    );
    assert!(
        !spells_a_plan_literal(&mint),
        "a mint is not a struct literal",
    );
    for opener in [
        "fn f() -> AdmittedGraphInputPlan { unimplemented!() }",
        "impl AdmittedGraphInputPlan { fn g() {} }",
    ] {
        assert!(
            !spells_a_plan_literal(&without_literals(opener)),
            "a brace that opens a body or an impl block is not a struct literal: {opener}",
        );
    }
    for spelling in [
        "let p = AdmittedGraphInputPlan {};",
        "let p = AdmittedGraphInputPlan { ..q };",
    ] {
        assert!(
            spells_a_plan_literal(&without_literals(spelling)),
            "a countless literal is still a literal: {spelling}",
        );
    }
}

/// Whether `code` states the plan by struct literal: the type name, a brace, and then the
/// counts themselves.
///
/// The brace alone is not the discriminator — `-> AdmittedGraphInputPlan {` opens a
/// function body and `impl AdmittedGraphInputPlan {` opens an impl block, and a gate that
/// read either as a literal would fire on every honest mint. What only a literal has is a
/// count named where a field goes: an empty or functional-update literal states the type
/// without a count, so those spellings are named here too rather than left as the way past.
fn spells_a_plan_literal(code: &str) -> bool {
    const NAME: &str = "AdmittedGraphInputPlan";
    const COUNTS: [&str; 3] = ["products", "roots", "commands"];
    symbol_positions(code, NAME).any(|at| {
        let Some(body) = code[at + NAME.len()..].trim_start().strip_prefix('{') else {
            return false;
        };
        let body = body.trim_start();
        body.starts_with('}')
            || body.starts_with("..")
            || COUNTS.iter().any(|count| {
                body.strip_prefix(count)
                    .is_some_and(|rest| rest.trim_start().starts_with(':'))
            })
    })
}

/// The plan's minting entry points, as the minter gates scan for them. A file is a minter
/// only if it names the plan type as well, so this needle is read in that company rather
/// than alone.
const PLAN_MINTING_ENTRY_POINTS: [&str; 2] = ["admit", "admit_saturating"];

/// The production files that mint a durable-graph construction budget.
///
/// `marrow-compile/src/durable.rs` is the compiler's admission owner: the store-declaration
/// census, taken over the whole declaration set before the first store is built.
/// `marrow-verify/src/verify/durable.rs` is the verifier's: the allocation-free structural
/// preflight over received bytes, which reads the one structural count the section states
/// and refuses root N+1 before a plan exists or a row is allocated. Widening this set is the
/// thing the minter gate exists to
/// notice; a row that no longer mints is what the staleness gate exists to notice.
const PERMITTED_PLAN_MINTERS: [&str; 2] = [
    "marrow-compile/src/durable.rs",
    "marrow-verify/src/verify/durable.rs",
];

/// No producer-side site-path length refusal survives.
///
/// A site path used to be a value a producer wrote by hand, so the encoder had to refuse
/// one that was too short to name a node or nested past the depth bound. A site is now a
/// binding of a live occurrence row to a live canonical declaration path, and its path is
/// projected from those rows: it is always the two root steps plus that node's own nesting
/// depth, which the declaration graph's one constructor bounded before the row existed.
/// Restoring either variant would mean a producer can again spell a path the graph does
/// not contain.
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
            "`{needle}` is a structurally unreachable refusal that no longer exists: {found:?}",
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
        "AdmittedGraphInputPlan",
        "admit",
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
        if line.contains("PlannedSiteRef") && !current.is_empty() {
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

/// Every `struct`/`enum` declaration in `code`, as `(name, whole declaration span)`.
///
/// A carrier's fields span many lines, and a line-at-a-time scan of a field list answers
/// about lines rather than about carriers — the exact failure mode the standing scanner law
/// names. Each span runs from the item's header to the point where its own braces or
/// parentheses close.
fn type_declaration_spans(code: &str) -> Vec<(String, String)> {
    let mut spans = Vec::new();
    let mut current: Option<(String, String)> = None;
    let mut depth = 0usize;
    for line in code.lines() {
        let trimmed = line.trim_start();
        if current.is_none() {
            let header = trimmed
                .strip_prefix("pub(crate) ")
                .or_else(|| trimmed.strip_prefix("pub "))
                .unwrap_or(trimmed);
            let Some(rest) = header
                .strip_prefix("struct ")
                .or_else(|| header.strip_prefix("enum "))
            else {
                continue;
            };
            let name = rest
                .split(['<', '(', '{', ' ', ';'])
                .next()
                .unwrap_or_default()
                .to_string();
            if name.is_empty() {
                continue;
            }
            current = Some((name, String::new()));
            depth = 0;
        }
        let Some((_, span)) = current.as_mut() else {
            continue;
        };
        span.push_str(line);
        span.push('\n');
        let opens = line.matches(['(', '{']).count();
        let closes = line.matches([')', '}']).count();
        depth += opens;
        if depth == 0 {
            // A unit struct terminated by `;` on its own header line.
            let (name, span) = current.take().expect("a declaration is accumulating");
            spans.push((name, span));
            continue;
        }
        depth -= closes;
        if depth == 0 {
            let (name, span) = current.take().expect("a declaration is accumulating");
            spans.push((name, span));
        }
    }
    spans
}

/// No carrier holds both a site handle and a minted site operand.
///
/// A handle is permission to request a site; an operand is the answer to one request. A
/// carrier holding both keeps a stale answer beside the live permission that would produce
/// a fresh one, and the two can disagree the moment the rows they stand on move — which is
/// exactly what the row stamps exist to make detectable. Descriptors keep the handle and
/// re-request; emitted code keeps the operand and nothing else.
#[test]
fn no_carrier_holds_both_a_handle_and_an_operand() {
    let mut offenders = Vec::new();
    for path in src_files() {
        let code = without_literals(&fs::read_to_string(&path).expect("read source file"));
        for (name, span) in type_declaration_spans(&code) {
            if span.contains("OccurrenceSiteHandle") && span.contains("PlannedSiteRef") {
                offenders.push(format!("{}::{name}", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these carriers hold a site permission and a minted site together: {offenders:?}",
    );
}

/// No owner retains a table of site handles keyed by anything.
///
/// A retained root-by-Product-member handle table is the `roots x members` residency the
/// declaration/occurrence split exists to remove: it costs one entry per (root, member)
/// whether or not any instruction names the place, and it holds permissions across the very
/// row movements that should invalidate them. Handles are minted at the moment of use.
#[test]
fn no_retained_handle_table_exists() {
    let mut offenders = Vec::new();
    for path in src_files() {
        let code = without_literals(&fs::read_to_string(&path).expect("read source file"));
        for (name, span) in type_declaration_spans(&code) {
            for collection in ["HashMap<", "BTreeMap<", "Vec<", "HashSet<", "BTreeSet<"] {
                for at in span.match_indices(collection).map(|(at, _)| at) {
                    let tail = &span[at..];
                    let end = tail.find('>').unwrap_or(tail.len());
                    if tail[..end].contains("OccurrenceSiteHandle") {
                        offenders.push(format!("{}::{name}", path.display()));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these owners retain a table of site permissions: {offenders:?}",
    );
}

/// Both carrier scans see a planted defect, and are not passing merely by finding nothing.
///
/// Neither defect can be planted in the live tree — the crate would still compile, but the
/// gate would then be red for the rest of the row — so the same predicates are run here over
/// source that does carry them.
#[test]
fn the_carrier_scans_detect_a_planted_declaration() {
    let planted = without_literals(
        r##"
        /// OccurrenceSiteHandle and PlannedSiteRef named in prose only.
        pub struct Innocent {
            handle: OccurrenceSiteHandle,
        }
        pub(crate) struct Guilty {
            handle: OccurrenceSiteHandle,
            minted: PlannedSiteRef,
        }
        struct Hoarder {
            per_member: BTreeMap<DeclarationNodeOrdinal, OccurrenceSiteHandle>,
        }
        "##,
    );
    let spans = type_declaration_spans(&planted);
    let names: Vec<&str> = spans.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["Innocent", "Guilty", "Hoarder"]);

    let both: Vec<&str> = spans
        .iter()
        .filter(|(_, span)| {
            span.contains("OccurrenceSiteHandle") && span.contains("PlannedSiteRef")
        })
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(both, ["Guilty"], "the doc comment naming both is not code");

    let hoarders: Vec<&str> = spans
        .iter()
        .filter(|(_, span)| {
            span.match_indices("BTreeMap<").any(|(at, _)| {
                let tail = &span[at..];
                let end = tail.find('>').unwrap_or(tail.len());
                tail[..end].contains("OccurrenceSiteHandle")
            })
        })
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(hoarders, ["Hoarder"]);
}

/// The site binder and the graph it validates against use no interior mutability and no
/// unsafe field split.
///
/// The binder answers "is this occurrence, this declaration path, and this target one live
/// place of this graph?" against rows it borrows. An interior-mutable cell or an aliasing
/// split would let the rows change under a validation that already answered, so a handle
/// could be minted against a row that no longer exists — precisely the check's subject.
#[test]
fn the_binder_holds_no_interior_mutability() {
    for owner in ["src/product.rs", "src/site_plan.rs", "src/draft.rs"] {
        let code = without_literals(
            &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(owner))
                .expect("read the binder owner"),
        );
        let found = shared_mutation_needles(&code);
        assert!(
            found.is_empty(),
            "{owner} names {found:?}: the binder validates rows it borrows",
        );
    }
    let binder = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/draft.rs"))
            .expect("read the binder owner"),
    );
    assert!(
        binder.contains("fn bind_occurrence_site"),
        "the binder itself is in the scanned owners, so this gate has a live subject",
    );
}

/// Every shared-mutation or aliasing spelling `code` names.
fn shared_mutation_needles(code: &str) -> Vec<&'static str> {
    [
        "Cell<",
        "RefCell<",
        "UnsafeCell<",
        "Mutex<",
        "RwLock<",
        "unsafe",
        "split_at_mut",
        "as *mut",
        "as *const",
    ]
    .into_iter()
    .filter(|needle| code.contains(needle))
    .collect()
}

/// The interior-mutability scan sees a planted field and not a planted mention.
///
/// A real plant would not compile, so the gate above can never observe its own defect in a
/// running test. The predicate is run here over source that carries it.
#[test]
fn the_interior_mutability_scan_detects_a_planted_field() {
    let innocent = without_literals(
        r##"
        /// A RefCell<Vec<u8>> would let the rows change under a validation.
        struct Rows {
            rows: Vec<DeclarationNode>,
        }
        "##,
    );
    assert!(shared_mutation_needles(&innocent).is_empty());

    let planted = without_literals(
        r##"
        struct Rows {
            scratch: RefCell<Vec<u8>>,
            rows: Vec<DeclarationNode>,
        }
        "##,
    );
    // `Cell<` is a substring of `RefCell<`, so a `RefCell` field trips both spellings;
    // what matters is that the field is seen and the doc mention is not.
    assert_eq!(shared_mutation_needles(&planted), ["Cell<", "RefCell<"]);
}

/// The draft savepoint is an affine allocation-identity admission token.
///
/// The prototype's `DraftSavepoint` was a `Clone` value a caller held: it could be
/// copied, stored, and offered to a draft other than the one it was taken on. Its
/// sanctioned successor keeps the name but not the capability: it strongly retains
/// the draft's allocation-identity anchor and one-shot epoch, is consumed whole by
/// `begin_transaction`, and derives neither `Clone` nor `Copy` — so a consumed epoch
/// cannot be re-presented and no mark can be transplanted. The old raw
/// `TemplateProofDraftGuard` and `rewind_to` faces stay absent: the armed
/// transaction is the one rollback owner.
#[test]
fn the_draft_savepoint_is_an_affine_admission_token() {
    assert!(
        occurrences("TemplateProofDraftGuard").is_empty(),
        "the raw template-proof guard was deleted with the transaction surface",
    );
    assert!(
        occurrences("fn rewind_to(").is_empty(),
        "no free-standing rewind mark survives",
    );
    assert!(
        !occurrences("DraftSavepoint").is_empty(),
        "the affine savepoint is present, so this gate has a live subject",
    );
    let code = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/draft.rs"))
            .expect("read the draft owner"),
    );
    assert!(
        !code.contains("impl Clone for DraftSavepoint")
            && !code.contains("impl Copy for DraftSavepoint"),
        "a hand-written Clone/Copy would let a consumed savepoint be re-presented",
    );
    let declaration = code
        .split("pub struct DraftSavepoint")
        .next()
        .expect("the declaration splits");
    let attributes: String = declaration.lines().rev().take(6).collect();
    assert!(
        !attributes.contains("Clone") && !attributes.contains("Copy"),
        "the savepoint declaration derives neither Clone nor Copy",
    );
    let snapshot = occurrences("DraftSnapshot");
    assert!(
        snapshot.iter().all(|(path, _)| path.ends_with("draft.rs")),
        "the restore snapshot stays inside the draft owner: {snapshot:?}",
    );
}

/// The draft is not `Clone`.
///
/// Every selector, handle, and operand a draft mints carries its identity, and its row
/// stamps come from one cursor. A copied draft carries a copied identity and a copied
/// cursor, so every capability minted afterwards authenticates in both copies and the stamp
/// check that detects a reused ordinal answers for a row a different draft appended. The
/// compile-fail known-answer test on `ImageDraft` pins the same absence from the other side;
/// this one catches an `impl` that would satisfy it without a derive.
#[test]
fn the_draft_is_not_clone() {
    let code = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/draft.rs"))
            .expect("read the draft owner"),
    );
    assert!(
        !code.contains("impl Clone for ImageDraft"),
        "a hand-written `Clone` copies the draft's identity and its stamp cursor",
    );
    let (_, declaration) = type_declaration_spans(&code)
        .into_iter()
        .find(|(name, _)| name == "ImageDraft")
        .expect("the draft is declared");
    let derives = code
        .split("pub struct ImageDraft")
        .next()
        .expect("the draft is declared")
        .rsplit("#[derive(")
        .next()
        .expect("the draft carries a derive list");
    assert!(
        !derives.contains("Clone"),
        "the draft's derive list names `Clone`: {derives}",
    );
    assert!(
        declaration.contains("durable: DurableContractGraph"),
        "the durable owner a copy would duplicate — the draft's identity and stamp cursor \
         live in it — is present, so this gate has a live subject",
    );
}

/// The image side declares no owned recursive durable value shape.
///
/// A durable field's value used to be a `DurableValueShape` tree: a node owned its
/// nested nodes, so a shape reached from four fields of four enclosing levels was
/// rebuilt at each of its 256 occurrences, and the expansion — not the declaration —
/// set the cost of admitting or refusing it. The replacement is a reference into one
/// `CanonicalValueShapeDag`, whose nodes hold `ValueShapeNodeId`s and can therefore
/// state neither a tree nor a cycle.
///
/// Reintroducing the tree is a one-line edit at any of the sites that carry a value:
/// a declaration row, a descriptor field, or a decoded member. This gate names the
/// deleted family and the shape of its return: an owned `Vec` of value shapes, or a
/// value-shape field that is anything but a node reference.
#[test]
fn no_owned_recursive_durable_value_shape_survives() {
    let deleted = [
        "DurableValueShape",
        "DurableEnumMemberShape",
        "Vec<ValueShapeNode>",
        "Vec<ValueShapeView",
    ];
    for source in [
        "src/durable_id.rs",
        "src/encode.rs",
        "src/product.rs",
        "src/draft.rs",
    ] {
        let code = without_literals(
            &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(source))
                .expect("read a durable value-shape owner"),
        );
        let found: Vec<&str> = deleted
            .into_iter()
            .filter(|needle| code.contains(needle))
            .collect();
        assert!(
            found.is_empty(),
            "{source} names {found:?}: a durable field's value is a reference into the \
             one value-shape arena, never an owned shape",
        );
    }

    // The arena itself is the one place a nested value is spelled, and it spells one
    // as a reference. A node that owned a `ValueShapeNode` would be a tree again.
    let arena = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/value_dag.rs"))
            .expect("read the value-shape arena"),
    );
    assert!(
        arena.contains("Struct(Vec<ValueShapeNodeId>)"),
        "the arena's struct node holds references, so this gate has a live subject",
    );
    assert!(
        !arena.contains("Box<ValueShapeNode>") && !arena.contains("Vec<ValueShapeNode>>"),
        "a node that owns a nested node is an occurrence tree",
    );
}

/// The verifier keeps no second representation of a decoded value shape.
///
/// The verifier used to decode the durable table into an owned shape tree and then
/// rebuild a second one to recompute the contract id, cloning each member on the way.
/// It now decodes straight into its own arena and holds references, so there is no
/// raw-tree-to-DAG conversion and no state retaining both.
#[test]
fn the_verifier_holds_no_raw_durable_value_tree() {
    let verify = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crates directory")
        .join("marrow-verify/src/verify");
    for source in ["model.rs", "durable.rs"] {
        let code = without_literals(
            &fs::read_to_string(verify.join(source)).expect("read a verifier durable owner"),
        );
        let found: Vec<&str> = ["DurableValueShape", "DurableEnumMemberShape"]
            .into_iter()
            .filter(|needle| code.contains(needle))
            .collect();
        assert!(
            found.is_empty(),
            "marrow-verify/src/verify/{source} names {found:?}: the verifier decodes into \
             its own value-shape arena",
        );
    }
    let decoded = without_literals(
        &fs::read_to_string(verify.join("model.rs")).expect("read the decoded model"),
    );
    assert!(
        decoded.contains("members: DurableProductGraph"),
        "a decoded root holds the shared declaration rows rather than an owned member \
         tree, so this gate has a live subject",
    );
}

/// The recursive durable contract-shape family is gone from the whole workspace.
///
/// Six public types spelled one program's durable member graph as an owned recursive tree
/// with public fields — `DurableRootShape` carrying `Vec<DurableMemberShape>`, groups and
/// branches carrying their own — and `DurableContractDescriptor` was the public entry that
/// took one by value. Any caller could build a chain of arbitrary depth from public struct
/// literals alone, and the process aborted on it: while constructing, or in the recursive
/// `Drop` of the argument the entry function had already refused. No entry function can fix
/// that; only unconstructibility can, which is why these names are absent rather than
/// guarded.
///
/// `DurableIndexShape` and `DurableIndexComponent` are deliberately **not** in this list:
/// they are flat, non-recursive draft-input rows for a managed-index declaration, they
/// carry no member vector, and nothing about them can be nested.
#[test]
fn no_recursive_durable_contract_shape_survives() {
    let sources = [workspace_sources("src"), workspace_sources("tests")].concat();
    for needle in DELETED_CONTRACT_SHAPE_FAMILY {
        let found: Vec<String> = sources
            .iter()
            .filter(|(_, code)| contains_symbol(code, needle))
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            found.is_empty(),
            "`{needle}` is a deleted recursive durable contract-shape symbol: {found:?}",
        );
    }
}

/// A deleted symbol does not survive in prose either.
///
/// The scan above reads the code projection, so a deleted name left in a doc comment, a
/// rationale, or a diagnostic message is invisible to it — and prose is where a deleted
/// symbol actually survives, because nothing stops compiling when it does. A reader then
/// meets a type the tree has not had for rows, and reasons about the deletion from the
/// thing it replaced. So the same needles are run over the unblanked sources.
///
/// This file is the one exemption, by necessity rather than by convenience: it is where the
/// needle table itself is written, so it names every deleted symbol in order to forbid it.
#[test]
fn no_deleted_contract_shape_survives_in_prose() {
    let mut found = Vec::new();
    for tier in ["src", "tests"] {
        for path in raw_workspace_sources(tier) {
            if path.ends_with("tests/absence_gates.rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("read source file");
            for needle in DELETED_CONTRACT_SHAPE_FAMILY {
                if text.contains(needle) {
                    found.push(format!("{}: {needle}", path.display()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "a deleted contract-shape symbol survives in prose: {found:#?}",
    );
}

/// Every workspace source path in `tier`, unprojected — for the one scan whose subject is
/// the text a reader sees rather than the code the compiler sees.
fn raw_workspace_sources(tier: &str) -> Vec<PathBuf> {
    workspace_sources(tier)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// The plant probe for the gate above: a source stating any of the deleted names is seen.
#[test]
fn the_recursive_contract_shape_gate_sees_a_planted_name() {
    for needle in DELETED_CONTRACT_SHAPE_FAMILY {
        let planted = without_literals(&format!(
            "fn forged() -> Vec<{needle}> {{ let _: {needle}; Vec::new() }}"
        ));
        assert!(
            contains_symbol(&planted, needle),
            "the scan sees a planted `{needle}`",
        );
    }
}

/// The exact deleted family: the six recursive member/root/key shape types plus the public
/// descriptor that owned one, and the two producer-side projections that allocated one per
/// root occurrence.
const DELETED_CONTRACT_SHAPE_FAMILY: [&str; 9] = [
    "DurableFieldShape",
    "DurableKeyShape",
    "DurableGroupShape",
    "DurableBranchShape",
    "DurableMemberShape",
    "DurableRootShape",
    "DurableContractDescriptor",
    "durable_descriptor",
    "member_shapes",
];

/// The shared `#[cfg(test)]` stripper sees a whole test module and leaves production
/// code alone. The projection has one owner in the workspace; this suite plants the probes
/// that keep the owner honest for the shapes these gates depend on.
#[test]
fn the_cfg_test_stripper_removes_exactly_the_test_items() {
    let planted = without_literals(
        r##"
        fn live() { descriptor.contract_id(); }
        #[cfg(test)]
        use super::contract_id;
        #[cfg(test)]
        mod tests {
            fn hostile() {
                if forged.contract_id().is_err() { forged.contract_id(); }
            }
        }
        fn also_live() { other.contract_id(); }
        "##,
    );
    let stripped = without_cfg_test_items(&planted);
    assert_eq!(
        stripped.matches("contract_id()").count(),
        2,
        "exactly the two production calls survive: {stripped}",
    );
    assert!(!stripped.contains("mod tests"), "{stripped}");
    assert!(stripped.contains("fn also_live"), "{stripped}");
}

/// Whether `code`'s production tier asks a durable-contract descriptor for its identity,
/// in either spelling: the method call `descriptor.contract_id()` and the UFCS form
/// `DurableContractDescriptor::contract_id(descriptor)`, which is the same call written
/// without a receiver. Matching the parenthesised call form alone reports a clean caller
/// set for a tree that grew a caller in the spelling that reads least like one.
///
/// The declaration is not an ask. It is excluded by the `fn` that introduces it rather
/// than by naming the file that holds it, so moving the owner cannot silently retire the
/// gate.
fn asks_for_contract_identity(code: &str) -> bool {
    let production = without_cfg_test_items(code);
    symbol_positions(&production, "contract_id").any(|at| preceding_token(&production, at) != "fn")
}

/// The identifier immediately before byte offset `at`, or the empty string when the text
/// there is not an identifier.
fn preceding_token(code: &str, at: usize) -> &str {
    let head = code[..at].trim_end();
    let bytes = head.as_bytes();
    let start = (0..bytes.len())
        .rev()
        .find(|index| !is_ident_byte(bytes[*index]))
        .map_or(0, |index| index + 1);
    head.get(start..).unwrap_or_default()
}

/// The brace-delimited body of the first item `marker` introduces in `code` — the
/// same anchored extraction the arena-accessor scan uses, shared by the mint gate so
/// "inside `emit_image`" is a parsed region rather than a line heuristic.
fn brace_body<'a>(code: &'a str, marker: &str) -> &'a str {
    let start = code.find(marker).expect("the marked item exists");
    let bytes = code.as_bytes();
    let mut cursor = start;
    while cursor < bytes.len() && bytes[cursor] != b'{' {
        cursor += 1;
    }
    let body_start = cursor;
    let mut depth = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &code[body_start..=cursor];
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    panic!("the marked item's braces close");
}

/// Whether `code` declares the ceiling the contract identity's refusal stands on.
///
/// The constant is matched as a whole identifier at its own `const`, not as a prefix: a
/// gate satisfied by a prefix stays green through the rename that leaves its subject with
/// no definition, and reports the ceiling present in a tree that no longer has one.
fn declares_fitting_preimage_ceiling(code: &str) -> bool {
    symbol_positions(code, "MAX_FITTING_CONTRACT_PREIMAGE_BYTES")
        .any(|at| preceding_token(code, at) == "const")
}

/// Both predicates the pinned-caller gate stands on are planted against the spellings that
/// would slip past a substring match.
///
/// A gate is only as strong as the shape it can see. `descriptor.contract_id()` and
/// `DurableContractDescriptor::contract_id(descriptor)` are one call in two spellings, and
/// a new trust path is exactly as likely to wear the second; a scan matching only the
/// parenthesised call form reports a clean caller set for a tree that grew a caller. The
/// ceiling check has the same shape of blindness in the other direction: a substring match
/// on a `const` declaration stays green through a rename that leaves the gate's subject
/// with no live definition at all.
#[test]
fn the_pinned_caller_gate_sees_both_spellings_and_an_exact_ceiling() {
    assert!(
        asks_for_contract_identity("fn caller(d: &D) { let _ = d.contract_id(); }"),
        "the method-call spelling is an ask",
    );
    assert!(
        asks_for_contract_identity(
            "fn caller(d: &D) { let _ = DurableContractDescriptor::contract_id(d); }",
        ),
        "so is the UFCS spelling of the same call",
    );
    assert!(
        !asks_for_contract_identity(
            "impl D { pub fn contract_id(&self) -> Result<Id, TooLarge> { todo!() } }",
        ),
        "the declaration is not an ask, or the owner would pin itself as a caller",
    );
    assert!(
        !asks_for_contract_identity("#[cfg(test)]\nmod tests { fn t(d: &D) { d.contract_id(); } }"),
        "a hostile call in the test tier is not a production caller",
    );

    assert!(
        declares_fitting_preimage_ceiling("const MAX_FITTING_CONTRACT_PREIMAGE_BYTES: usize = 1;"),
        "the live declaration is seen",
    );
    assert!(
        !declares_fitting_preimage_ceiling(
            "const MAX_FITTING_CONTRACT_PREIMAGE_BYTES_RENAMED: usize = 1;",
        ),
        "a renamed constant leaves the gate's subject undefined, so the gate must fire",
    );
}

/// The durable-contract identity has one production mint per side of the image boundary,
/// and asking for one is bounded work wherever it is asked from.
///
/// The producer mints the identity exactly once, inside the measure core's
/// `emit_image` — which runs only on a plan the measured ceiling already admitted —
/// as the one normalized `.contract_view().contract_id()` chain. A
/// `DurableContractView` is a view over a public arena, so any caller can state a
/// value shape whose expansion is exponential in its declared levels and ask for its
/// identity; the ceiling that answers belongs to the identity owner. This gate
/// therefore holds both halves: the ask set is counted per file across every crate's
/// production sources — exactly one per whitelisted site — and the mint itself must
/// still carry its own bound.
#[test]
fn the_contract_identity_has_one_mint_per_side_and_a_bound_of_its_own() {
    // Exactly one production ask per whitelisted file, in any spelling, and no ask
    // anywhere else in the workspace.
    for (path, code) in workspace_sources("src") {
        let production = without_cfg_test_items(&code);
        let asks = symbol_positions(&production, "contract_id")
            .filter(|&at| preceding_token(&production, at) != "fn")
            .count();
        let display = path.display().to_string();
        let expected = usize::from(
            CONTRACT_IDENTITY_CALLERS
                .iter()
                .any(|permitted| display.contains(permitted)),
        );
        assert_eq!(
            asks, expected,
            "the durable-contract identity is asked for exactly once per pinned \
             production site and nowhere else; `{display}` asks {asks} time(s)",
        );
    }

    // Mint law: the producer's one mint sits inside the brace-extracted body of
    // `fn emit_image`, as the exact normalized `.contract_view().contract_id()` chain,
    // and the `contract_id` symbol appears nowhere else in this crate's production
    // source outside the identity owner.
    let measure = without_cfg_test_items(&without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/measure.rs"))
            .expect("read the measure core"),
    ));
    let body = brace_body(&measure, "fn emit_image");
    let normalized: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(
        normalized.matches(".contract_view().contract_id()").count(),
        1,
        "`emit_image` mints the identity exactly once, over the one contract view",
    );
    assert_eq!(
        symbol_positions(&measure, "contract_id").count(),
        symbol_positions(body, "contract_id").count(),
        "every `contract_id` spelling in the measure core sits inside `emit_image`",
    );
    for (path, code) in workspace_sources("src") {
        let display = path.display().to_string();
        if !display.contains("marrow-image/src/") {
            continue;
        }
        if display.ends_with("durable_id.rs") || display.ends_with("measure.rs") {
            continue;
        }
        let production = without_cfg_test_items(&code);
        assert!(
            !contains_symbol(&production, "contract_id"),
            "`{display}` spells `contract_id`; the crate's only production spellings \
             are the identity owner's definition and the emitter's one mint",
        );
    }

    // The mint's own bound. The public constructor takes a bare arena and promises
    // nothing about it, so the refusal has to live on the identity.
    let owner = without_cfg_test_items(&without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/durable_id.rs"))
            .expect("read the durable identity owner"),
    ));
    assert!(
        owner.contains(
            "pub fn contract_id(&self) -> Result<DurableContractId, DurableGraphTooLarge>"
        ),
        "the contract identity is fallible, so a graph past the payload ceiling is refused \
         rather than allocated",
    );
    assert!(
        declares_fitting_preimage_ceiling(&owner),
        "the ceiling the refusal stands on is present, so this gate has a live subject",
    );
    assert!(
        owner.contains("values: &'a CanonicalValueShapeDag,"),
        "the public constructor still takes a bare arena, which is why the bound belongs \
         to the identity rather than to a precondition on its callers",
    );
}

/// Whether `code` takes a mutable value-shape arena from an owner it was handed.
///
/// The accessor that republishes the arena its own owner already holds mints nothing — it
/// is the call every minter makes — so its body is removed before the scan. Otherwise the
/// owner that publishes the arena would count as a minter of it, and the gate below would
/// name the draft, which opens the template proof the gate is about.
fn mints_value_shapes(code: &str) -> bool {
    without_arena_accessor(code).contains("value_shapes_mut()")
}

/// `code` with the body of every `fn value_shapes_mut` blanked, line breaks preserved.
fn without_arena_accessor(code: &str) -> String {
    let mut code = code.to_string();
    const MARKER: &str = "fn value_shapes_mut";
    let mut from = 0usize;
    while let Some(start) = code[from..].find(MARKER).map(|at| at + from) {
        let bytes = code.as_bytes();
        let mut cursor = start;
        while cursor < bytes.len() && bytes[cursor] != b'{' {
            cursor += 1;
        }
        let mut depth = 0usize;
        let mut scan = cursor;
        while scan < bytes.len() {
            match bytes[scan] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        scan += 1;
                        break;
                    }
                }
                _ => {}
            }
            scan += 1;
        }
        let blanked: String = code[start..scan]
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        code.replace_range(start..scan, &blanked);
        from = scan;
    }
    code
}

/// No value shape is minted inside a template proof.
///
/// A `ValueShapeNodeId` is an ordinal into one arena and authenticates nothing: it carries
/// no draft identity and no generation, so it means whichever node currently sits at that
/// ordinal. A template proof appends to the real draft and its guard truncates the arena
/// back when the proof is discarded, so an id minted inside a proof and kept past it would
/// name whatever shape a later mint puts in its place — a different value wearing the same
/// reference.
///
/// Nothing in the tree can reach that today: the two production owners that mint value
/// shapes open no proof, and the proof bodies mint none. That is a property of the caller
/// set rather than of the id, which is why it is pinned rather than assumed.
#[test]
fn no_value_shape_is_minted_inside_a_template_proof() {
    let sources = workspace_sources("src");
    let minting: Vec<String> = sources
        .iter()
        .filter(|(_, code)| mints_value_shapes(&without_cfg_test_items(code)))
        .map(|(path, _)| path.display().to_string())
        .collect();
    for permitted in VALUE_SHAPE_MINT_OWNERS {
        assert!(
            minting.iter().any(|path| path.contains(permitted)),
            "`{permitted}` no longer mints value shapes, so this gate's subject moved: \
             {minting:?}",
        );
    }
    assert_eq!(
        minting.len(),
        VALUE_SHAPE_MINT_OWNERS.len(),
        "value shapes are minted at exactly the pinned owners; found {minting:?}",
    );

    // The compiler's durable lowering reaches the arena only through the typed
    // transaction appenders.
    let compiler_durable = sources
        .iter()
        .find(|(path, _)| path.ends_with("marrow-compile/src/durable.rs"))
        .map(|(_, code)| without_cfg_test_items(code))
        .expect("the compiler's durable owner is scanned");
    assert!(
        compiler_durable.contains(".value_scalar(")
            && compiler_durable.contains(".value_struct(")
            && compiler_durable.contains(".value_enum("),
        "the compiler's durable lowering mints through the transaction appenders",
    );

    // The draft's raw `&mut` arena escape is gone from the public surface — on
    // `ImageDraft` the accessor survives only crate-private, behind the typed
    // transaction appenders. The one public arena escape left is the contract
    // graph's own, whose sole cross-crate caller is the verifier minting into the
    // twin arena its decoded graph owns.
    let draft_owner_code = sources
        .iter()
        .find(|(path, _)| path.ends_with("src/draft.rs"))
        .map(|(_, code)| without_cfg_test_items(code))
        .expect("the draft owner is scanned");
    assert!(
        !draft_owner_code.contains("pub fn value_shapes_mut"),
        "the draft's raw arena escape returned",
    );
    assert!(
        draft_owner_code.contains("pub(crate) fn value_shapes_mut"),
        "the crate-private interior is present, so this gate has a live subject",
    );
}

/// The production owner that mints durable value shapes: the compiler's one durable owner,
/// which resolves a durable field's value type into the draft's arena. It opens no
/// template proof. A second owner appearing here is a second place a `ValueShapeNodeId`
/// can come from, and the first thing to ask of it is whether it mints under a proof.
/// The two production owners whose text mints value shapes: the image draft — whose
/// typed transaction appenders are the sole mint path into the draft's arena now that
/// the raw `&mut` arena escape is deleted — and the verifier's durable decode, into
/// the arena its own contract graph owns. The compiler's durable lowering mints only
/// through the draft's appenders, which the assertion below names.
const VALUE_SHAPE_MINT_OWNERS: [&str; 2] = [
    "marrow-image/src/draft.rs",
    "marrow-verify/src/verify/durable.rs",
];

/// The production callers of the durable-contract identity: the producer's planned
/// emitter and the verifier's independent recomputation. Those two are the whole point
/// — one mint per side of the image boundary, agreeing by recomputation rather than by
/// transfer — so a third is a new trust path and a missing one is a side that stopped
/// checking.
const CONTRACT_IDENTITY_CALLERS: [&str; 2] = [
    "marrow-image/src/measure.rs",
    "marrow-verify/src/verify/durable.rs",
];

/// The producer keeps no second, recursive way to emit a durable body.
///
/// One walk writes the DURABLE section for both of its readers — the measure core's
/// counting run and the emission buffer — so the body a count admits is the body that
/// is built. A second emitter, especially one that recursed through an owned value
/// shape, would be free to disagree with the count that admitted it. The gate is
/// tree-wide: exactly one `fn write_durable_body` in the whole workspace, and no
/// `DurableBody`/`DurableSection` type DEFINITION anywhere (the wire-form *variant*
/// of that name is legitimate and untouched).
#[test]
fn the_durable_body_has_one_writer() {
    let mut writers = 0usize;
    for (path, code) in workspace_sources("src") {
        writers += code.matches("fn write_durable_body").count();
        assert!(
            !code.contains("struct DurableSection") && !code.contains("struct DurableBody("),
            "`{}` defines a durable body/section type; the sink-generic writer is the              only durable-body representation",
            path.display(),
        );
    }
    assert_eq!(
        writers, 1,
        "the DURABLE body has exactly one writer, generic over its sink",
    );
    let encode = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/encode.rs"))
            .expect("read the encoder"),
    );
    assert!(
        encode.contains("sink: &mut impl ImageByteSink"),
        "that writer is sink-generic, so counting and building cannot diverge",
    );
}

/// The measure core's affine carriers are closed: the coherence witness, the
/// policy-clean witness, the measured-length witness, and the wire plan are
/// nameable only inside the measure core, and the core's entry point is reachable
/// only from the one encode driver — so no caller can mint a second constructor,
/// pair a witness or plan with another draft, or supply its own draft at emission.
/// (Within the core the borrow-bound affine types carry the same law at the type
/// level; this scan pins that no second spelling grows outside it.)
#[test]
fn the_measure_core_carriers_are_closed() {
    // (file suffix, symbol, expected production occurrence count)
    const CARRIER_SET: [(&str, &str, usize); 9] = [
        ("marrow-image/src/measure.rs", "LegacyV0MeasureCore", 2),
        ("marrow-image/src/encode.rs", "LegacyV0MeasureCore", 2),
        ("marrow-image/src/measure.rs", "CoherentDraft", 4),
        ("marrow-image/src/measure.rs", "PolicyClean", 4),
        ("marrow-image/src/measure.rs", "MeasuredDurableLen", 4),
        ("marrow-image/src/measure.rs", "LegacyV0WirePlan", 4),
        // The sealed section sink: defined beside the tokens, received by the nine
        // writers, constructed only by the measure core's drivers.
        ("marrow-image/src/remap.rs", "SectionSink", 5),
        ("marrow-image/src/encode.rs", "SectionSink", 11),
        ("marrow-image/src/measure.rs", "SectionSink", 17),
    ];
    let lib = without_cfg_test_items(&without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
            .expect("read the crate root"),
    ));
    for (_, symbol, _) in CARRIER_SET {
        assert!(
            !contains_symbol(&lib, symbol),
            "`{symbol}` is published by lib.rs, so the carrier set is no longer closed",
        );
    }
    for (path, code) in workspace_sources("src") {
        let display = path.display().to_string();
        let production = without_cfg_test_items(&code);
        for symbol in [
            "LegacyV0MeasureCore",
            "CoherentDraft",
            "PolicyClean",
            "MeasuredDurableLen",
            "LegacyV0WirePlan",
            "SectionSink",
        ] {
            let found = symbol_positions(&production, symbol).count();
            let expected = CARRIER_SET
                .iter()
                .find(|(suffix, name, _)| display.contains(suffix) && *name == symbol)
                .map_or(0, |(_, _, count)| *count);
            assert_eq!(
                found, expected,
                "`{display}` spells `{symbol}` {found} time(s); the carrier set \
                 admits {expected}",
            );
        }
    }
}

/// The DURABLE traversal's access set is closed: `expand`, `ValueShapeWireForm`, and
/// `write_durable_body` are reachable from exactly the enumerated production sites,
/// so a renamed or transplanted second walker cannot appear without failing here.
///
/// The measured-DURABLE-length witness is the type-level half of the same law: it is
/// private to the measure core and mintable only by the counting run over the one
/// writer, and the wire plan's constructor demands it — so even a walker this text
/// scan somehow missed could not feed a plan. The positive needle below pins that the
/// counting run really does call the canonical writer.
#[test]
fn the_durable_traversal_has_a_closed_access_set() {
    // (file suffix, symbol, expected production occurrence count)
    const ACCESS_SET: [(&str, &str, usize); 8] = [
        // The definition (its test-tier uses are stripped before the count).
        ("marrow-image/src/value_dag.rs", "expand", 1),
        // The import and the one body writer's field-value expansion call.
        ("marrow-image/src/encode.rs", "expand", 2),
        // The import and the identity preimage walk's call.
        ("marrow-image/src/durable_id.rs", "expand", 2),
        ("marrow-image/src/value_dag.rs", "ValueShapeWireForm", 4),
        ("marrow-image/src/encode.rs", "ValueShapeWireForm", 2),
        ("marrow-image/src/durable_id.rs", "ValueShapeWireForm", 2),
        // The writer's definition beside its emission-order helpers...
        ("marrow-image/src/encode.rs", "write_durable_body", 1),
        // ...and the measure core's two runs: the counting run and the planned
        // emission.
        ("marrow-image/src/measure.rs", "write_durable_body", 2),
    ];
    // The three symbols are crate-internal — `lib.rs` publishes none of them — so the
    // reachable caller set is exactly this crate's production sources.
    let lib = without_cfg_test_items(&without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
            .expect("read the crate root"),
    ));
    for symbol in ["expand", "ValueShapeWireForm", "write_durable_body"] {
        assert!(
            !contains_symbol(&lib, symbol),
            "`{symbol}` is published by lib.rs, so the access set is no longer closed \
             over this crate",
        );
    }
    for (path, code) in workspace_sources("src") {
        let display = path.display().to_string();
        if !display.contains("marrow-image/src") {
            continue;
        }
        let production = without_cfg_test_items(&code);
        for symbol in ["expand", "ValueShapeWireForm", "write_durable_body"] {
            let found = symbol_positions(&production, symbol).count();
            let expected = ACCESS_SET
                .iter()
                .find(|(suffix, name, _)| display.contains(suffix) && *name == symbol)
                .map_or(0, |(_, _, count)| *count);
            assert_eq!(
                found, expected,
                "`{display}` reaches `{symbol}` {found} time(s); the DURABLE \
                 traversal's access set admits {expected}",
            );
        }
    }
    // The counting run drives the canonical writer through the checked sink.
    let measure = without_cfg_test_items(&without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/measure.rs"))
            .expect("read the measure core"),
    ));
    assert!(
        measure.contains("draft.write_durable_body(sink, strings)"),
        "the measured DURABLE length is counted through the one installed writer",
    );
}

/// The shared section writers resolve string and constant references only through
/// opaque remap tokens, never by indexing a sort map.
///
/// A writer that can read a remapped index can branch on it, which is exactly how a
/// counted section and an emitted section drift apart. The owner-safe raw reads —
/// the constant comparator's sort key and the test-entry permutation's comparator
/// keys — live in the draft owner, outside the writers' file, so the writers' file
/// spells no sort-map indexing at all.
#[test]
fn the_shared_writers_index_no_sort_map() {
    let encode = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/encode.rs"))
            .expect("read the encoder"),
    );
    for needle in ["str_map[", "const_map["] {
        assert!(
            !encode.contains(needle),
            "`{needle}` reads a sort map inside the shared-writer file; writers receive \
             opaque tokens",
        );
    }
    assert!(
        encode.contains("StringRemap") && encode.contains("ConstRemap"),
        "the token providers are present, so this gate has a live subject",
    );
}

/// A token is spent only inside the writers' file: the sealed `.emit(` spelling and
/// the DURABLE writer's `.emit_durable(` spelling appear in production code at
/// exactly the writer sites (plus the token owner's one internal delegation), so no
/// other owner can route a token into a sink of its own — the bypass the sealed
/// sink exists to forbid.
#[test]
fn tokens_are_spent_only_inside_the_writers() {
    // (file suffix, spelling, expected production occurrence count)
    const SPEND_SET: [(&str, &str, usize); 3] = [
        ("marrow-image/src/encode.rs", ".emit(", 9),
        ("marrow-image/src/encode.rs", ".emit_durable(", 2),
        // The token owner's `emit` delegates to the one two-byte append.
        ("marrow-image/src/remap.rs", ".emit_durable(", 1),
    ];
    for (path, code) in workspace_sources("src") {
        let display = path.display().to_string();
        if !display.contains("marrow-image/src") {
            continue;
        }
        let production = without_cfg_test_items(&code);
        // Both call spellings per path: the method call and the UFCS form, which is
        // the same call written without a receiver.
        for spelling in [".emit(", ".emit_durable(", "::emit(", "::emit_durable("] {
            let found = production.matches(spelling).count();
            let expected = SPEND_SET
                .iter()
                .find(|(suffix, name, _)| display.contains(suffix) && *name == spelling)
                .map_or(0, |(_, _, count)| *count);
            assert_eq!(
                found, expected,
                "`{display}` spells `{spelling}` {found} time(s); tokens are spent at                  exactly the writer sites, which admit {expected}",
            );
        }
    }
}

/// The planted-probe half of the token-spending gate: the spelling must be visible
/// to the scan in code and invisible in prose or the test tier.
#[test]
fn the_token_spend_scan_sees_code_and_not_prose() {
    let planted = without_cfg_test_items(&without_literals(
        r##"
        // strings.token(id).emit(&mut probe);
        const DOC: &str = "strings.token(id).emit(&mut probe)";
        fn live(sink: &mut S) { strings.token(id).emit(sink); }
        fn ufcs_probe() { crate::remap::StringToken::emit_durable(token, &mut probe); }
        #[cfg(test)]
        mod tests {
            fn hostile() { strings.token(id).emit(&mut probe); }
        }
        "##,
    ));
    assert_eq!(
        planted.matches(".emit(").count(),
        1,
        "exactly the production method-call occurrence is visible to the scan",
    );
    assert_eq!(
        planted.matches("::emit_durable(").count(),
        1,
        "the UFCS spelling of the same call is visible to the scan",
    );
}

/// The planted-probe half of the sort-map gate: each needle must be visible to the
/// scan in code and invisible in prose, or the gate passes for the wrong reason.
#[test]
fn the_sort_map_scan_sees_code_and_not_prose() {
    let planted = without_literals(
        r##"
        // let final_index = str_map[old as usize];
        const DOC: &str = "str_map[old]";
        const RAW: &str = r#"const_map[old]"#;
        let live = str_map[old as usize];
        let also_live = const_map[old as usize];
        "##,
    );
    let hits: Vec<&str> = planted
        .lines()
        .filter(|line| line.contains("str_map[") || line.contains("const_map["))
        .collect();
    assert_eq!(
        hits.len(),
        2,
        "exactly the code occurrences are visible to the scan: {hits:?}",
    );
    assert!(
        hits[0].contains("let live") && hits[1].contains("let also_live"),
        "the visible occurrences are the code ones: {hits:?}",
    );
}

// ---- The bounded-representation discipline the falsifier's claim rests on.

/// The crates that own the durable contract graph and the verified graph it decodes into.
/// The discipline below is theirs; the rest of the workspace is out of this gate's subject.
const BOUNDED_GRAPH_CRATES: [&str; 2] = ["marrow-image", "marrow-verify"];

/// The spellings that would let a representation escape its own drop, alias, or bound.
///
/// Each is a way of making the falsifier's result meaningless rather than of fixing what it
/// found: a custom `Drop` can hide a recursive teardown behind a manual one, `ManuallyDrop`
/// and `forget`/`leak` retire the teardown entirely, and `unsafe` puts the representation
/// outside the compiler's reach altogether. The row's stop condition names all of them, so
/// they are held by a gate rather than by memory.
const UNBOUNDED_REPRESENTATION_NEEDLES: [&str; 6] = [
    "unsafe",
    "ManuallyDrop",
    "mem::forget",
    "Box::leak",
    "impl Drop for",
    "MaybeUninit",
];

/// Neither graph owner reaches for a spelling that would put its representation outside the
/// bound the falsifier measures.
///
/// The falsifier proves the whole journey fits a 64 KiB stack. That result is only about the
/// representation if the representation is the ordinary one: a hand-written `Drop`, a leaked
/// owner, or an `unsafe` block could make the same journey fit while leaving the recursive
/// teardown the row exists to remove. So the discipline is asserted directly and the
/// measurement is left to say what it can say.
#[test]
fn neither_graph_owner_escapes_its_own_representation() {
    let sources = [workspace_sources("src"), workspace_sources("tests")].concat();
    let mut found = Vec::new();
    for (path, code) in &sources {
        let owned = BOUNDED_GRAPH_CRATES
            .iter()
            .any(|krate| path.components().any(|part| part.as_os_str() == *krate));
        if !owned {
            continue;
        }
        for needle in UNBOUNDED_REPRESENTATION_NEEDLES {
            for (index, line) in code.lines().enumerate() {
                if !line.contains(needle) {
                    continue;
                }
                if needle == "impl Drop for" && line.contains(PRESERVED_PROOF_GUARD) {
                    continue;
                }
                found.push(format!("{}:{}: {needle}", path.display(), index + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "the durable graph owners must hold their representation in ordinary safe Rust, so \
         the falsifier's 64 KiB result is about the representation and not about a manual \
         teardown: {found:#?}"
    );
}

/// **The plant probe for the gate above.** The scan must see each needle in real code, must
/// not see it inside a comment or a string, and must be able to tell an `impl Drop` from a
/// mention of dropping.
#[test]
fn the_representation_scan_detects_a_planted_escape() {
    for needle in UNBOUNDED_REPRESENTATION_NEEDLES {
        let planted = without_literals(&format!(
            "fn f() {{ let _ = {needle}; }}\n// a comment naming {needle}\nconst S: &str = \"{needle}\";\n"
        ));
        let hits = planted.lines().filter(|line| line.contains(needle)).count();
        assert_eq!(
            hits, 1,
            "the scan must see `{needle}` in code exactly once and neither in the comment \
             nor in the string literal: {planted}"
        );
    }

    // The `impl Drop for` needle is deliberately the whole clause: a file that merely
    // mentions dropping, or that calls `drop`, is not implementing one.
    let innocent = without_literals("fn f(v: Vec<u8>) { drop(v); } // the drop leg\n");
    assert!(
        !innocent.contains("impl Drop for"),
        "an ordinary drop call must not read as a custom Drop implementation"
    );
}

/// The one preserved `Drop` implementation in either crate: the template-proof guard's
/// inherited rollback.
///
/// It is not a durable-graph owner — it discards what a proof body appended to a draft, in
/// reverse dependency order, allocating and indexing nothing — and preserving it exactly is
/// this row's own stop condition. It is exempted by name rather than by relaxing the needle,
/// so a second `Drop` implementation still fires.
const PRESERVED_PROOF_GUARD: &str = "DraftTxn";

/// The graph's arena has one owner, and no second index shadows it.
///
/// The opaque graph is a view spine over the canonical Product/occurrence tables and the one
/// value-shape arena. A second arena, or a side index keyed by the same identities, would
/// reintroduce exactly the divergence the single owner exists to prevent — two answers to
/// one question, kept in step by hand — and would do it without any public type changing.
#[test]
fn no_second_value_shape_arena_or_index_shadow_survives() {
    let mut mints = Vec::new();
    for path in src_files() {
        let source = fs::read_to_string(&path).expect("read source file");
        let code = without_literals(&without_cfg_test_items(&source));
        for needle in [
            "CanonicalValueShapeDag::new",
            "CanonicalValueShapeDag::default",
        ] {
            for (index, line) in code.lines().enumerate() {
                if line.contains(needle) {
                    mints.push(format!("{}:{}: {needle}", path.display(), index + 1));
                }
            }
        }
    }
    // One mint, in the one durable-graph owner. The compiler's draft and the graph the
    // verifier reconstructs are the same owner, so the arena a declaration interns into and
    // the arena a decoded row references are minted by one line of code. Any further mint
    // would be a shadow of it.
    let owned: Vec<&String> = mints
        .iter()
        .filter(|mint| mint.contains("/product.rs:"))
        .collect();
    assert_eq!(
        (mints.len(), owned.len()),
        (1, 1),
        "the value-shape arena is minted once, by the durable graph both sides build \
         through, and nowhere else: {mints:#?}"
    );
}

/// A semantic path's step count is never narrowed by a cast.
///
/// `SemanticPath` is bounded at `MAX_SITE_PATH_STEPS` on both public routes into it —
/// construction and extension — and every owner that spells the count in a fixed-width
/// field spells it with a checked conversion off that bound. A narrowing cast is what the
/// closed defect was: `steps.len() as u16` framed a 65,536-step path as zero steps, and
/// `steps().len() as u8` sat one sibling away from the same shape. The two spellings are
/// gated together because they are one class: a cast here is total only by a property of
/// the caller, and the whole point of bounding the type was to stop reasoning that way.
///
/// Scoped to the production projection, unlike this file's other scans. The gate's subject
/// is the owners that *frame* bytes from a path a caller handed over; the independent
/// decoder reconstructions in `demand.rs` and `ceiling.rs` exist precisely to spell the
/// wire's field widths by hand, share no code with the owner by design, and consume a path
/// the bounded type already produced, so a width there cannot narrow anything.
#[test]
fn no_semantic_path_step_count_is_narrowed_by_a_cast() {
    let mut casts = Vec::new();
    for path in src_files() {
        let source = fs::read_to_string(&path).expect("read source file");
        let code = without_literals(&without_cfg_test_items(&source));
        for (index, line) in code.lines().enumerate() {
            for needle in ["steps().len() as ", "steps.len() as "] {
                if line.contains(needle) {
                    casts.push(format!("{}:{}: {needle}", path.display(), index + 1));
                }
            }
        }
    }
    assert!(
        casts.is_empty(),
        "a semantic path's step count is spelled by checked conversion off its bound, \
         never by a narrowing cast: {casts:#?}"
    );
}

/// The step-count scan sees a real cast and is not fooled by prose.
#[test]
fn the_step_count_cast_scan_separates_a_cast_from_a_mention() {
    let planted = without_literals(
        "fn f(p: &SemanticPath) -> u16 { p.steps().len() as u16 }\n\
         // the deleted spelling wrote steps.len() as u16 here\n\
         let text = \"steps.len() as u8\";\n",
    );
    let hits: Vec<&str> = planted
        .lines()
        .filter(|line| line.contains("steps().len() as ") || line.contains("steps.len() as "))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the planted cast is seen, not the comment or the string: {hits:#?}"
    );
}

/// Every adjudicated site that can produce the whole-image ceiling verdict. All of them
/// sit before the wire plan is minted: the plan is the witness that the image fits, so a
/// ceiling verdict issued after it would be a policy decision made after the policy
/// decision closed.
const PRE_PLAN_IMAGE_TOO_LARGE_SITES: &[(&str, &str)] = &[
    // The closed builder-error variant itself.
    ("draft.rs", "ImageTooLarge,"),
    // The one production site: the capped counting sink's decisive N+1 sentinel, which
    // runs before any plan exists.
    ("measure.rs", "return Err(ImageBuildError::ImageTooLarge);"),
];

/// A coupled mint prepares its whole delta before it mutates anything.
///
/// `intern_text` appends a string row and a constant row with their two index entries and
/// whatever policy kinds either append crosses. Deriving and mutating one row and then the
/// other leaves the string appended if the constant's own derivation refuses — which is
/// what the read-only preflight-accumulator law forbids. The split that makes this hold is
/// typed: `prepare_*` takes `&self`, so it *cannot* mutate, and `commit_*` returns no
/// `Result`, so it cannot stop partway.
#[test]
fn a_coupled_mint_prepares_its_whole_delta_before_it_mutates() {
    let code = production_code(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/draft.rs"))
            .expect("read the draft owner"),
    );

    // The preparing halves cannot mutate, and the committing halves cannot fail.
    for prepare in ["fn prepare_string(&self", "fn prepare_const(&self"] {
        assert!(
            code.contains(prepare),
            "`{prepare}...` takes the draft by shared reference, which is what makes \
             preparation unable to touch an owner",
        );
    }
    for commit in [
        "fn commit_string(&mut self, prepared: PreparedString) -> StrId",
        "fn commit_const(&mut self, prepared: PreparedConst) -> ConstId",
    ] {
        assert!(
            code.contains(commit),
            "`{commit}` returns no Result, so applying a prepared delta cannot stop partway",
        );
    }

    assert!(
        compound_prepares_before_committing(&code),
        "`intern_text` must prepare both rows before it commits either; committing the \
         string first leaves it appended when the constant's derivation refuses",
    );
}

/// Whether `code`'s `intern_text` prepares both halves before committing either.
fn compound_prepares_before_committing(code: &str) -> bool {
    // The owner, not the transaction surface's one-line delegation to it: both are spelled
    // `fn intern_text`, and the wrapper comes first and prepares nothing. A gate that
    // scanned the wrapper would pass whatever the owner did.
    let start = code.find("pub(crate) fn intern_text").unwrap_or_else(|| {
        panic!("the `intern_text` owner is present, so this gate has a subject")
    });
    let body = &code[start..];
    let end = body
        .find("\n    }")
        .unwrap_or_else(|| panic!("`intern_text` has a body"));
    let body = &body[..end];
    let last_prepare = ["prepare_string(", "prepare_const("]
        .iter()
        .map(|call| {
            body.find(call)
                .unwrap_or_else(|| panic!("`intern_text` prepares through `{call}`"))
        })
        .max()
        .unwrap_or(0);
    let first_commit = ["commit_string(", "commit_const("]
        .iter()
        .filter_map(|call| body.find(call))
        .min()
        .unwrap_or_else(|| panic!("`intern_text` commits a prepared delta"));
    last_prepare < first_commit
}

/// The ordering scanner sees a planted inversion.
#[test]
fn the_coupled_mint_scanner_sees_a_planted_inversion() {
    let ordered = "pub(crate) fn intern_text(&mut self) {\n        let a = self.prepare_string(t)?;\n        \
                   let b = self.prepare_const(v)?;\n        self.commit_string(a);\n        \
                   self.commit_const(b)\n    }\n";
    assert!(
        compound_prepares_before_committing(ordered),
        "the scanner accepts the real ordering",
    );

    let inverted = "pub(crate) fn intern_text(&mut self) {\n        let a = self.prepare_string(t)?;\n        \
                    self.commit_string(a);\n        let b = self.prepare_const(v)?;\n        \
                    self.commit_const(b)\n    }\n";
    assert!(
        !compound_prepares_before_committing(inverted),
        "the scanner must reject a body that commits its first row before preparing its second",
    );
}
