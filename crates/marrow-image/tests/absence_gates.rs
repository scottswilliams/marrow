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
    let sources = workspace_sources("src");
    for needle in SEAM_ENTRY_POINTS {
        let found: Vec<String> = sources
            .iter()
            .filter(|(path, code)| {
                contains_symbol(code, needle)
                    && !path.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
                    && !PERMITTED_SEAM_CALLERS
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

/// The staleness half of the caller gate: every permitted path must still call the seam.
///
/// An allowlist entry that names no caller is a pre-opened door — it permits a future
/// widening into that file silently, and it is exactly what the gate above cannot see,
/// because a row that matches nothing never fails. A caller that migrates off the seam
/// must take its row with it.
#[test]
fn every_permitted_seam_caller_still_calls_the_seam() {
    let sources = workspace_sources("src");
    for permitted in PERMITTED_SEAM_CALLERS {
        let calls = sources.iter().any(|(path, code)| {
            path.to_string_lossy().contains(permitted)
                && SEAM_ENTRY_POINTS
                    .iter()
                    .any(|needle| contains_symbol(code, needle))
        });
        assert!(
            calls,
            "`{permitted}` is a permitted seam caller that calls nothing in the seam: \
             a dead allowlist row pre-permits a widening the caller gate cannot see",
        );
    }
}

/// The construction seam's published entry points, as the caller gates scan for them.
const SEAM_ENTRY_POINTS: [&str; 5] = [
    "declare_product",
    "add_root_occurrence",
    "bind_occurrence_site",
    "request_site",
    "product_members",
];

/// The production callers of the construction seam: the compiler's one durable owner —
/// the store builder and the lowering boundary that first demands a field-leaf site — and
/// the verifier's code decoder. Widening this set is the thing the caller gate exists to
/// notice; a row that no longer calls anything is what the staleness gate exists to notice.
const PERMITTED_SEAM_CALLERS: [&str; 3] = [
    "marrow-compile/src/durable.rs",
    "marrow-compile/src/lower/mod.rs",
    "marrow-verify/src/verify/decode_code.rs",
];

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
            if span.contains("OccurrenceSiteHandle") && span.contains("LegacyDraftSiteOperand") {
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
        /// OccurrenceSiteHandle and LegacyDraftSiteOperand named in prose only.
        pub struct Innocent {
            handle: OccurrenceSiteHandle,
        }
        pub(crate) struct Guilty {
            handle: OccurrenceSiteHandle,
            minted: LegacyDraftSiteOperand,
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
            span.contains("OccurrenceSiteHandle") && span.contains("LegacyDraftSiteOperand")
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

/// There is no free-standing draft rollback mark.
///
/// `DraftSavepoint` was a `Clone` value a caller held: it could be copied, stored, and
/// offered to a draft other than the one it was taken on, where it truncated unrelated
/// tables to lengths that meant nothing there. Its replacement is
/// `TemplateProofDraftGuard`, whose checkpoint is a private type this crate never returns —
/// so there is no mark to transplant, and the exclusive borrow makes the draft unreachable
/// while a proof is open. A returned mark is that capability coming back.
#[test]
fn no_free_standing_draft_rollback_mark_exists() {
    for needle in ["DraftSavepoint", "fn savepoint", "fn rewind_to"] {
        let found = occurrences(needle);
        assert!(
            found.is_empty(),
            "`{needle}` is a rollback mark a caller can hold: {found:?}",
        );
    }
    assert!(
        !occurrences("TemplateProofDraftGuard").is_empty(),
        "the guard that replaced the mark is present, so this gate has a live subject",
    );
    let checkpoint = occurrences("DraftCheckpoint");
    assert!(
        checkpoint
            .iter()
            .all(|(path, _)| path.ends_with("draft.rs")),
        "the checkpoint stays inside the draft owner: {checkpoint:?}",
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
        declaration.contains("identity: DraftIdentity"),
        "the identity a copy would duplicate is present, so this gate has a live subject",
    );
}

/// The image side declares no owned recursive durable value shape (red R34).
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

/// The verifier keeps no second representation of a decoded value shape (red R34).
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
        decoded.contains("value: ValueShapeNodeId"),
        "a decoded field carries a value reference, so this gate has a live subject",
    );
}

/// `code` with every `#[cfg(test)]` item removed: from the attribute through the point
/// where that item's own braces (or its terminating `;`) close.
///
/// A gate about production call sites that cannot tell a test module from the code it
/// guards answers about the test tier instead — and the test tier is exactly where a
/// deliberately hostile call belongs. The input is already literal-stripped, so the brace
/// counting is over code.
fn without_cfg_test(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(at) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let mut depth = 0usize;
        let mut opened = false;
        let mut end = tail.len();
        for (index, byte) in tail.bytes().enumerate() {
            match byte {
                b'{' => {
                    depth += 1;
                    opened = true;
                }
                b'}' if opened => {
                    depth -= 1;
                    if depth == 0 {
                        end = index + 1;
                        break;
                    }
                }
                // A `#[cfg(test)]` `use` or item declaration ends at its semicolon.
                b';' if !opened => {
                    end = index + 1;
                    break;
                }
                _ => {}
            }
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// The `#[cfg(test)]` stripper sees a whole test module and leaves production code alone.
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
    let stripped = without_cfg_test(&planted);
    assert_eq!(
        stripped.matches("contract_id()").count(),
        2,
        "exactly the two production calls survive: {stripped}",
    );
    assert!(!stripped.contains("mod tests"), "{stripped}");
    assert!(stripped.contains("fn also_live"), "{stripped}");
}

/// The durable-contract identity has one production mint per side of the image boundary,
/// and asking for one is bounded work wherever it is asked from.
///
/// The old encoder built the DURABLE body first and hashed the graph at the end of it, so
/// a body larger than any image could carry was still hashed — over a preimage larger than
/// the body, since the contract spells a ledger identity as a 25-byte `IDREF` where the
/// body writes 16 raw bytes. The producer now counts the body first and mints the identity
/// only from `LegacyDurableBodyLowerBoundFence`, which exists only for a body that fits.
///
/// That fence bounds the producer, and only the producer. A `DurableContractDescriptor` is
/// a public view over a public arena, so any caller can state a value shape whose expansion
/// is exponential in its declared levels and ask for its identity; the ceiling that answers
/// belongs to the identity owner. This gate therefore holds both halves: the call-site set
/// is scanned across every crate's production sources rather than one file, and the mint
/// itself must still carry its own bound.
#[test]
fn the_contract_identity_has_one_mint_per_side_and_a_bound_of_its_own() {
    let callers: Vec<String> = workspace_sources("src")
        .into_iter()
        .filter(|(_, code)| without_cfg_test(code).contains("contract_id()"))
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert_eq!(
        callers.len(),
        CONTRACT_IDENTITY_CALLERS.len(),
        "the durable-contract identity is asked for at exactly the pinned production \
         sites; found {callers:?}",
    );
    for permitted in CONTRACT_IDENTITY_CALLERS {
        assert!(
            callers.iter().any(|path| path.contains(permitted)),
            "`{permitted}` no longer asks for a contract identity, so a pinned site is \
             stale: {callers:?}",
        );
    }

    // The producer's one call sits inside the body fence, between the fence's `impl` and
    // the draft's, so it is reachable only from a body that already fits.
    let encode = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/encode.rs"))
            .expect("read the encoder"),
    );
    let lines: Vec<&str> = encode.lines().collect();
    let line_of = |needle: &str| {
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains(needle))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "the encoder names `{needle}` exactly once, at lines {hits:?}",
        );
        hits[0]
    };
    let fence = line_of("impl<'a> LegacyDurableBodyLowerBoundFence<'a> {");
    let minted = line_of("contract_id()");
    let descriptor = line_of("self.draft.durable_descriptor()");
    let draft_impl = line_of("impl ImageDraft {");
    assert!(
        fence < minted && minted < draft_impl,
        "the producer's one contract-identity mint sits inside the body fence",
    );
    assert!(
        fence < descriptor && descriptor < draft_impl,
        "so does the one descriptor construction it hashes",
    );

    // The mint's own bound. The public constructor takes a bare arena and promises
    // nothing about it, so the refusal has to live on the identity.
    let owner = without_cfg_test(&without_literals(
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
        owner.contains("const MAX_CONTRACT_GRAPH_BYTES"),
        "the ceiling the refusal stands on is present, so this gate has a live subject",
    );
    assert!(
        owner.contains("values: &'a CanonicalValueShapeDag,"),
        "the public constructor still takes a bare arena, which is why the bound belongs \
         to the identity rather than to a precondition on its callers",
    );
}

/// The production callers of the durable-contract identity: the producer's body fence and
/// the verifier's independent recomputation. Those two are the whole point — one mint per
/// side of the image boundary, agreeing by recomputation rather than by transfer — so a
/// third is a new trust path and a missing one is a side that stopped checking.
const CONTRACT_IDENTITY_CALLERS: [&str; 2] = [
    "marrow-image/src/encode.rs",
    "marrow-verify/src/verify/durable.rs",
];

/// The producer keeps no second, recursive way to emit a durable body (red R34).
///
/// One walk writes the DURABLE section for both of its readers — the lower bound that
/// counts its bytes and the buffer that keeps them — so the body a count admits is the
/// body that is built. A second emitter, especially one that recursed through an owned
/// value shape, would be free to disagree with the count that admitted it.
#[test]
fn the_durable_body_has_one_writer() {
    let encode = without_literals(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/encode.rs"))
            .expect("read the encoder"),
    );
    let writers = encode.matches("fn write_durable_body").count();
    assert_eq!(
        writers, 1,
        "the DURABLE body has exactly one writer, generic over its sink",
    );
    assert!(
        encode.contains("sink: &mut impl ImageByteSink"),
        "that writer is sink-generic, so counting and building cannot diverge",
    );
    assert!(
        !encode.contains("struct DurableSection") && !encode.contains("struct DurableBody("),
        "the saturating body buffer the fence replaced is gone",
    );
}
