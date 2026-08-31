//! The narrowing census: every place a value is converted to a `u16` or `u32` carrier in
//! the three crates that own the identities this row widened, pinned to its exact source
//! line.
//!
//! Split out of `absence_gates.rs` at the census seam — it is a self-contained subject with
//! its own scanner, its own adjudicated list, and its own plant probe.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "common/source_projection.rs"]
mod source_projection;
use source_projection::{is_ident_byte, is_test_only_file, production_code};

/// Every surviving narrowing site in the three crates that own a widened carrier is
/// pinned to its exact source line — not to a per-file total.
///
/// A per-file count is weaker than its subject: swapping an unrelated cast into a file
/// that already holds a sanctioned one keeps the total and stays green, which is how the
/// type-parameter wrap escaped in the first place. The census is therefore keyed by
/// `(crate, file, the exact normalized line the site sits on)`. A cast that moves to a
/// different construct changes the line and has to be adjudicated again.
///
/// Scope is all three crates that own a carrier this row widened: `marrow-compile`,
/// `marrow-image` — which owns every one of them — and `marrow-verify`, the sealed
/// domain they widen at. Spellings are the whole narrowing family, not one literal:
/// `as u16`/`as u32` with any intervening whitespace or blanked comment, the fallible
/// `u16::try_from`/`u32::try_from`/`try_into` forms that narrow the same carriers by
/// another route, and **arithmetic over a narrow carrier** — a `+`, `-`, or `*` whose own
/// expression names a `u16`/`u32` operand. Arithmetic is the spelling that has no
/// conversion to find it by: `fn_base + row as u16` overflows the carrier without
/// narrowing anything a cast scan can see, and it is a site of this census twice — once
/// for the cast and once for the addition over it.
///
/// The blind spot, stated: arithmetic between two values that are *inferred* `u16` without
/// either being spelled at the operand is invisible to a lexical scan, which has no types.
/// Every such value in these crates reaches its carrier through one of the spellings above,
/// so the site is censused where it is spelled; a carrier that stopped being spelled
/// anywhere would leave this census with nothing to key on, which is why the widened
/// carriers are newtypes rather than bare integers.
///
/// Each surviving entry is either a function-family ordinal frozen for the function-slot
/// refounding, a count bounded by its own construct's located diagnostic, or a checked
/// conversion whose refusal is the closed builder-domain error.
///
/// Sites are compared by multiplicity, not by membership. Several sites of one crate share
/// one normalized line — six `Self(index as u32)` mints do — and a membership difference
/// cannot see one of them appear or vanish, so the census would stay green while its
/// subject moved. Counting keys the comparison exactly without pinning line numbers, which
/// every unrelated edit above a site would churn.
#[test]
fn every_narrowing_site_is_pinned_to_its_exact_census() {
    let found = tallied(&narrowing_sites());
    let expected = tallied(&sanctioned_narrowing_sites());
    let mut added = Vec::new();
    let mut missing = Vec::new();
    for (site, count) in &found {
        let sanctioned = expected.get(site).copied().unwrap_or(0);
        if *count > sanctioned {
            added.push((site.clone(), *count - sanctioned));
        }
    }
    for (site, count) in &expected {
        let present = found.get(site).copied().unwrap_or(0);
        if *count > present {
            missing.push((site.clone(), *count - present));
        }
    }
    assert!(
        missing.is_empty() && added.is_empty(),
        "the narrowing census moved. New sites must be adjudicated — either the value is \
         bounded by an exact located diagnostic and the census grows with that proof, or \
         it takes the wide-carrier treatment. Vanished sites must be removed from the \
         census. Each entry is `(site, how many occurrences moved)`.\n  \
         added: {added:#?}\n  missing: {missing:#?}",
    );
}

/// How many times each site occurs, so a repeated normalized line is compared by count.
fn tallied(sites: &[NarrowingSite]) -> BTreeMap<NarrowingSite, usize> {
    let mut tally = BTreeMap::new();
    for site in sites {
        *tally.entry(site.clone()).or_default() += 1;
    }
    tally
}

/// The census proves it is scanning something: the three crates together hold this many
/// production source files, so a scan that silently reads an empty set fails here rather
/// than reporting a clean census.
#[test]
fn the_narrowing_census_scans_all_three_carrier_crates() {
    let mut per_crate: BTreeMap<&str, usize> = BTreeMap::new();
    for (krate, _) in narrowing_crate_files() {
        *per_crate.entry(krate).or_default() += 1;
    }
    assert_eq!(
        per_crate.len(),
        3,
        "three crates are scanned: {per_crate:?}"
    );
    for (krate, count) in &per_crate {
        assert!(*count > 5, "{krate} contributed only {count} source files");
    }
    // The scanner finds real sites, so a projection bug that blanks everything is loud.
    assert!(
        !narrowing_sites().is_empty(),
        "the scan found no narrowing site at all, which means it is not reading code",
    );
}

/// The narrowing scanner sees code and not prose or test items, and it is not defeated by
/// whitespace or a comment between the operator and its type.
#[test]
fn the_narrowing_census_sees_code_and_not_prose_and_resists_spelling_evasion() {
    let planted = "/// prose about as u16 stays prose\n\
fn probe(n: usize) -> u16 {\n    n as u16\n}\n\
fn spread(n: usize) -> u16 {\n    n as\n        u16\n}\n\
fn commented(n: usize) -> u16 {\n    n as /* here */ u16\n}\n\
fn fallible(n: usize) -> u16 {\n    u16::try_from(n).unwrap_or(0)\n}\n\
#[cfg(test)]\nmod tests {\n    fn hidden(n: usize) -> u16 {\n        n as u16\n    }\n}\n";
    let hits = narrowing_hits_in(&production_code(planted));
    assert_eq!(
        hits.len(),
        4,
        "the plain, line-split, comment-separated, and fallible spellings all count, and \
         prose and test items do not: {hits:#?}",
    );
}

/// The scanner reads arithmetic over a narrow carrier, and does not read an arithmetic
/// operator whose own expression names no narrow carrier — nor a dereference, a return
/// arrow, or a compound assignment, none of which are binary arithmetic.
#[test]
fn the_narrowing_census_reads_arithmetic_over_a_narrow_carrier() {
    // The exact shape the cast scan could not see on its own account.
    let carried = "fn probe(base: u16, row: usize) -> u16 {\n    base + row as u16\n}\n";
    assert_eq!(
        narrowing_hits_in(&production_code(carried)).len(),
        2,
        "the cast is one site and the addition over the narrowed carrier is a second",
    );

    let suffixed = "fn probe(base: u16) -> u16 {\n    base + 1u16\n}\n";
    assert_eq!(
        narrowing_hits_in(&production_code(suffixed)).len(),
        1,
        "a narrow-suffixed literal names the carrier at the operand",
    );

    let bound = "fn probe(base: usize) -> usize {\n    base + usize::from(u16::MAX)\n}\n";
    assert_eq!(
        narrowing_hits_in(&production_code(bound)).len(),
        0,
        "the addition is over a widened value: `u16::MAX` sits inside its own call window",
    );

    let wide = "fn probe(a: usize, b: usize) -> usize {\n    a + b * 2\n}\n";
    assert!(
        narrowing_hits_in(&production_code(wide)).is_empty(),
        "arithmetic naming no narrow carrier is not a site",
    );

    let not_arithmetic =
        "fn probe(v: &u16, n: u16) -> u16 {\n    let mut t = *v;\n    t += n;\n    t\n}\n";
    assert!(
        narrowing_hits_in(&production_code(not_arithmetic)).is_empty(),
        "a dereference, a return arrow, and a compound assignment are not binary arithmetic",
    );
}

/// The production source files of every crate that owns a carrier this row widened.
fn narrowing_crate_files() -> Vec<(&'static str, PathBuf)> {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits under crates/");
    let mut files = Vec::new();
    for krate in ["marrow-compile", "marrow-image", "marrow-verify"] {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("read src dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        let mut owned = Vec::new();
        walk(&crates_dir.join(krate).join("src"), &mut owned);
        owned.sort();
        assert!(!owned.is_empty(), "{krate} has production source");
        files.extend(
            owned
                .into_iter()
                .filter(|p| !is_test_only_file(p))
                .map(|p| (krate, p)),
        );
    }
    files
}

/// One narrowing site: the crate, the `src`-relative file, and the exact normalized
/// source line it sits on.
type NarrowingSite = (String, String, String);

/// Every narrowing site across the three carrier crates, sorted.
fn narrowing_sites() -> Vec<NarrowingSite> {
    let mut sites = Vec::new();
    for (krate, path) in narrowing_crate_files() {
        let rel = path
            .display()
            .to_string()
            .split_once("src/")
            .expect("a src path")
            .1
            .to_string();
        let code = production_code(&fs::read_to_string(&path).expect("read source file"));
        for line in narrowing_hits_in(&code) {
            sites.push((krate.to_string(), rel.clone(), line));
        }
    }
    sites.sort();
    sites
}

/// The normalized source line of every narrowing spelling in `code`.
///
/// Matching runs over the whole projection rather than line by line, so a spelling split
/// across lines is one hit reported at the line the operator opens on. Whitespace is
/// collapsed so indentation, line breaks, and blanked comments cannot change a site's
/// identity.
fn narrowing_hits_in(code: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let bytes = code.as_bytes();
    let mut at = 0usize;
    while at < code.len() {
        let hit = match narrowing_at(code, bytes, at) {
            Some(end) => end,
            None => {
                at += 1;
                continue;
            }
        };
        let line_start = code[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let line_end = code[at..].find('\n').map(|n| at + n).unwrap_or(code.len());
        hits.push(normalize_line(&code[line_start..line_end]));
        at = hit;
    }
    hits
}

/// The narrow-carrier spellings an arithmetic operand may name. A window holding one of
/// these is arithmetic on a `u16`/`u32` carrier, whatever the surrounding types are.
const NARROW_OPERAND_SPELLINGS: [&str; 10] = [
    "as u16",
    "as u32",
    "u16::MAX",
    "u16::MIN",
    "u32::MAX",
    "u32::MIN",
    "u16::from",
    "u32::from",
    "u16>",
    "u32>",
];

/// Whether an arithmetic operator opens at `at`, with `-` in `->`, `*` in a dereference or
/// a raw pointer, and every comparison form excluded.
///
/// Compound assignment counts. `offset += width` is addition over `offset`'s carrier as
/// surely as `offset = offset + width` is, and excluding it left an accumulator over a
/// `u32` carrier — the one at `code_layout` in the image encoder — outside the census that
/// exists to hold exactly those. A census that cannot see a live site is the failure it
/// exists to prevent, so the compound forms are read as the arithmetic they are.
fn arithmetic_operator_at(bytes: &[u8], at: usize) -> Option<usize> {
    let before = (0..at)
        .rev()
        .find(|index| !bytes[*index].is_ascii_whitespace())
        .map(|index| bytes[index]);
    let after = bytes.get(at + 1).copied();
    // An operand must precede the operator, which is what separates `x * y` from a
    // dereference or a raw-pointer type. `<<`/`>>` are excluded by the same rule.
    let operand_before = before.is_some_and(|byte| {
        is_ident_byte(byte) || byte == b')' || byte == b']' || byte == b'\'' || byte == b'"'
    });
    match bytes[at] {
        b'+' | b'*' if operand_before => Some(at + 1),
        b'-' if operand_before && after != Some(b'>') => Some(at + 1),
        _ => None,
    }
}

/// The expression window an arithmetic operator at `at` sits in: the text between the
/// nearest enclosing delimiters. A window is one expression rather than a whole statement,
/// so a narrow spelling elsewhere in the same statement does not make an unrelated
/// operator a narrow-carrier site.
fn operand_window(code: &str, bytes: &[u8], at: usize) -> (usize, usize) {
    const BOUNDARY: [u8; 9] = [b';', b'{', b'}', b',', b'(', b')', b'=', b'&', b'|'];
    let mut start = at;
    while start > 0 && !BOUNDARY.contains(&bytes[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < bytes.len() && !BOUNDARY.contains(&bytes[end]) {
        end += 1;
    }
    let _ = code;
    (start, end)
}

/// The whole statement an operator at `at` sits in: the text between the nearest enclosing
/// statement delimiters. Used for the compound-assignment forms, whose operands straddle
/// the `=` an expression window stops at.
fn statement_window(bytes: &[u8], at: usize) -> (usize, usize) {
    const BOUNDARY: [u8; 3] = [b';', b'{', b'}'];
    let mut start = at;
    while start > 0 && !BOUNDARY.contains(&bytes[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < bytes.len() && !BOUNDARY.contains(&bytes[end]) {
        end += 1;
    }
    (start, end)
}

/// If a narrowing spelling starts at `at`, its end offset.
fn narrowing_at(code: &str, bytes: &[u8], at: usize) -> Option<usize> {
    if let Some(end) = arithmetic_operator_at(bytes, at) {
        // A compound assignment's two operands sit on opposite sides of its `=`, which is
        // an expression-window boundary. Reading the expression window alone therefore
        // reads only the accumulator and never the value being folded into it — which is
        // where the narrow spelling is. `offset += instr.encoded_len() as u32` is exactly
        // that shape, and it is why widening the operator set alone did not reach it.
        let (start, stop) = if bytes.get(at + 1) == Some(&b'=') {
            statement_window(bytes, at)
        } else {
            operand_window(code, bytes, at)
        };
        let window = &code[start..stop];
        if NARROW_OPERAND_SPELLINGS
            .iter()
            .any(|spelling| window.contains(spelling))
            || narrow_suffixed_literal(window)
        {
            return Some(end);
        }
    }
    let boundary = at == 0 || !is_ident_byte(bytes[at - 1]);
    if !boundary {
        return None;
    }
    for fallible in ["u16::try_from", "u32::try_from"] {
        if code[at..].starts_with(fallible) {
            return Some(at + fallible.len());
        }
    }
    // `try_into` narrows through an annotated destination rather than a spelled type, so
    // it counts wherever a narrow integer type is named on the same statement.
    if code[at..].starts_with("try_into") {
        let line_end = code[at..].find(';').map(|n| at + n).unwrap_or(code.len());
        if code[at..line_end].contains("u16") || code[at..line_end].contains("u32") {
            return Some(at + "try_into".len());
        }
        return None;
    }
    if !code[at..].starts_with("as") || bytes.get(at + 2).is_some_and(|b| is_ident_byte(*b)) {
        return None;
    }
    let mut cursor = at + 2;
    while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
        cursor += 1;
    }
    for narrow in ["u16", "u32"] {
        if code[cursor..].starts_with(narrow)
            && !bytes
                .get(cursor + narrow.len())
                .is_some_and(|b| is_ident_byte(*b))
        {
            return Some(cursor + narrow.len());
        }
    }
    None
}

/// Whether `window` names a narrow-suffixed integer literal — `1u16`, `0_u32` — which is
/// a `u16`/`u32` carrier stated at the operand with no cast to find it by.
fn narrow_suffixed_literal(window: &str) -> bool {
    let bytes = window.as_bytes();
    ["u16", "u32"].iter().any(|suffix| {
        window.match_indices(suffix).any(|(at, _)| {
            at > 0
                && (bytes[at - 1].is_ascii_digit() || bytes[at - 1] == b'_')
                && !bytes.get(at + 3).is_some_and(|byte| is_ident_byte(*byte))
        })
    })
}

/// A source line reduced to its identity: leading/trailing space removed and every
/// internal whitespace run collapsed, so reformatting is not a census change.
fn normalize_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The adjudicated narrowing sites. Generated once from the tree and reviewed
/// site-by-site; a change here is a deliberate decision, not a count that drifted.
fn sanctioned_narrowing_sites() -> Vec<NarrowingSite> {
    let mut sites: Vec<NarrowingSite> = SANCTIONED_NARROWING
        .iter()
        .map(|(krate, file, line)| (krate.to_string(), file.to_string(), line.to_string()))
        .collect();
    sites.sort();
    sites
}

/// The adjudicated census: every place a value is converted to a `u16` or `u32` carrier
/// in the three crates that own the identities this row widened.
///
/// Entries are keyed by their exact source line, so a cast that changes construct — or a
/// second cast added beside a sanctioned one — is a census change and has to be looked at
/// again. The families present are: wire-ordinal projections behind the plan, the
/// function-family ordinals frozen for the function-slot refounding, source offsets and
/// counts bounded by their own construct's located diagnostic, and checked conversions
/// whose refusal is the closed builder-domain error.
///
/// This list pins the exact current set; it is not itself a per-site proof. That is what
/// makes it useful: a new site cannot arrive silently, and the adjudication happens where
/// the change is made.
const SANCTIONED_NARROWING: &[(&str, &str, &str)] = &[
    (
        "marrow-compile",
        "analysis.rs",
        "(span.end_byte as u32) < offset",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        ".filter(|argument| (argument.value.span().end_byte as u32) < offset)",
    ),
    (
        "marrow-compile",
        "analysis/facts.rs",
        ".map(|module| module.identity().as_str().len() as u32)",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "Ok(active_call::resolve(&tree, source, offset as u32))",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "Ok(completion::resolve(&tree, offset as u32))",
    ),
    ("marrow-compile", "analysis.rs", "Some(index as u16)"),
    (
        "marrow-compile",
        "analysis.rs",
        "end: span.end_byte.min(u32::MAX as usize) as u32,",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "if offset <= site.span.end_byte as u32 {",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "let callee_end = site.callee.span().end_byte as u32;",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "let offset = offset as u32;",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "let offset = offset as u32;",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "return Some(index as u16);",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "span.start_byte as u32 <= offset && offset <= span.end_byte as u32",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "span.start_byte as u32 <= offset && offset <= span.end_byte as u32",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "start as u32 <= offset && offset <= end as u32",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "start as u32 <= offset && offset <= end as u32",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "start: span.start_byte.min(u32::MAX as usize) as u32,",
    ),
    (
        "marrow-compile",
        "analysis.rs",
        "u16::try_from(index).ok().map(Self)",
    ),
    (
        "marrow-compile",
        "compile.rs",
        "Ok(AdmittedModules(modules.len() as u16))",
    ),
    (
        "marrow-compile",
        "decl.rs",
        "index: self.refusals.len() as u32,",
    ),
    (
        "marrow-compile",
        "durable.rs",
        ".map(|(index, field)| (index as u16, field))",
    ),
    (
        "marrow-compile",
        "durable.rs",
        ".map(|(index, field)| (index as u16, field))",
    ),
    (
        "marrow-compile",
        "durable.rs",
        "let command = u32::try_from(commands.len()).expect( );",
    ),
    (
        "marrow-compile",
        "lower/durable.rs",
        "cols: key_columns.len() as u16,",
    ),
    (
        "marrow-compile",
        "lower/durable.rs",
        "cols: root.key.len() as u16,",
    ),
    (
        "marrow-compile",
        "lower/mod.rs",
        "let index = self.code.len() as u32;",
    ),
    (
        "marrow-compile",
        "lower/mod.rs",
        "| Instr::IntRemChecked(t) => *t = target as u32,",
    ),
    ("marrow-compile", "lower/stmts.rs", "Some(value as u32)"),
    ("marrow-compile", "lower/stmts.rs", "field: field as u16,"),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(advance as u32), body.span)?;",
    ),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(target as u32), span)?;",
    ),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(top as u32), body.span)?;",
    ),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(top as u32), body.span)?;",
    ),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "variant: variant_index as u16,",
    ),
    (
        "marrow-compile",
        "types/metadata.rs",
        "index: index as u16,",
    ),
    (
        "marrow-compile",
        "types/mod.rs",
        ".map(|(index, field)| (index as u16, field))",
    ),
    (
        "marrow-compile",
        "types/mod.rs",
        ".map(|(index, variant)| (index as u16, variant))",
    ),
    (
        "marrow-compile",
        "types/mod.rs",
        ".map(|(ordinal, group)| ((self.fields.len() + ordinal) as u16, group))",
    ),
    (
        "marrow-compile",
        "types/mod.rs",
        ".map(|index| (NominalId(index as u32), &self.nominals[index]))",
    ),
    // All five image-function-index producers accumulate wide and narrow only here.
    // An out-of-domain value is bounded by the exact typed refusal
    // `GenericInvariant::FunctionIndexDomain`; it is deliberately locationless because
    // the aggregate function count has no single offending source span.
    (
        "marrow-compile",
        "types/function_index.rs",
        "u16::try_from(wide).map_err(|_| GenericInvariant::FunctionIndexDomain)",
    ),
    (
        "marrow-compile",
        "types/mod.rs",
        "Self(u32::try_from(position).expect( ))",
    ),
    (
        "marrow-compile",
        "types/mod.rs",
        "let variant_index = u16::try_from(selection.index).map_err(|_| {",
    ),
    (
        "marrow-image",
        "demand.rs",
        "let step_count = u16::try_from(steps.len())",
    ),
    (
        "marrow-image",
        "demand.rs",
        "payload.extend_from_slice(&(self.atoms.len() as u32).to_be_bytes());",
    ),
    (
        "marrow-image",
        "demand.rs",
        "self.take(2)?.try_into().expect( ),",
    ),
    (
        "marrow-image",
        "draft.rs",
        "CurrentValidationOccurrence::at_row(bounds::MAX_COLLECTIONS as u32),",
    ),
    (
        "marrow-image",
        "draft.rs",
        "CurrentValidationOccurrence::at_row(bounds::MAX_CONSTS as u32),",
    ),
    (
        "marrow-image",
        "draft.rs",
        "CurrentValidationOccurrence::at_row(bounds::MAX_ENUMS as u32),",
    ),
    (
        "marrow-image",
        "draft.rs",
        "CurrentValidationOccurrence::at_row(bounds::MAX_ROOTS as u32),",
    ),
    (
        "marrow-image",
        "draft.rs",
        "CurrentValidationOccurrence::at_row(bounds::MAX_SITES as u32),",
    ),
    (
        "marrow-image",
        "draft.rs",
        "CurrentValidationOccurrence::at_row(bounds::MAX_STRINGS as u32),",
    ),
    (
        "marrow-image",
        "draft.rs",
        "CurrentValidationOccurrence::at_row(bounds::MAX_TYPES as u32),",
    ),
    ("marrow-image", "draft.rs", "Self(index as u32)"),
    ("marrow-image", "draft.rs", "Self(index as u32)"),
    ("marrow-image", "draft.rs", "Self(index as u32)"),
    ("marrow-image", "draft.rs", "Self(index as u32)"),
    ("marrow-image", "draft.rs", "Self(index as u32)"),
    ("marrow-image", "draft.rs", "Self(index as u32)"),
    (
        "marrow-image",
        "draft.rs",
        "u16::try_from(len).map_err(|_| DraftStateError::CarrierDomain)",
    ),
    (
        "marrow-image",
        "draft.rs",
        "u32::try_from(len).map_err(|_| DraftStateError::CarrierDomain)",
    ),
    (
        "marrow-image",
        "durable_id.rs",
        "u16::try_from(count).map_err(|_| DurableGraphTooLarge)",
    ),
    // Code layout accumulates wide and narrows at the two consuming boundaries. Both
    // sites are bounded by the exact `ImageBuildError::CodeTooLong` refusal. Production
    // lowering refuses the crossing instruction at its source span before retaining it,
    // so an encode-time occurrence is a producer invariant rather than a locationless
    // compiler resource outcome.
    (
        "marrow-image",
        "encode.rs",
        "offsets.push(u32::try_from(offset).map_err(|_| ImageBuildError::CodeTooLong)?);",
    ),
    (
        "marrow-image",
        "encode.rs",
        "let total_len = u32::try_from(offset).map_err(|_| ImageBuildError::CodeTooLong)?;",
    ),
    (
        "marrow-image",
        "encode.rs",
        "push_frame(out, id, body.len() as u32);",
    ),
    (
        "marrow-image",
        "encode.rs",
        "push_u16(sink, function.spans.len() as u16);",
    ),
    (
        "marrow-image",
        "encode.rs",
        "push_u16(sink, key_slots.len() as u16);",
    ),
    (
        "marrow-image",
        "encode.rs",
        "push_u16(sink, self.export_count() as u16);",
    ),
    (
        "marrow-image",
        "encode.rs",
        "push_u16(sink, self.functions().len() as u16);",
    ),
    (
        "marrow-image",
        "encode.rs",
        "push_u16(sink, self.test_entry_count() as u16);",
    ),
    (
        "marrow-image",
        "interface.rs",
        "body.extend_from_slice(&(self.params.len() as u16).to_be_bytes());",
    ),
    (
        "marrow-image",
        "interface.rs",
        "out.extend_from_slice(&(fields.len() as u16).to_be_bytes());",
    ),
    (
        "marrow-image",
        "interface.rs",
        "out.extend_from_slice(&(keys.len() as u16).to_be_bytes());",
    ),
    (
        "marrow-image",
        "interface.rs",
        "out.extend_from_slice(&(variant.payload.len() as u16).to_be_bytes());",
    ),
    (
        "marrow-image",
        "interface.rs",
        "out.extend_from_slice(&(variants.len() as u16).to_be_bytes());",
    ),
    (
        "marrow-image",
        "interface.rs",
        "payload.extend_from_slice(&(self.descriptors.len() as u32).to_be_bytes());",
    ),
    (
        "marrow-image",
        "measure.rs",
        "*durable = Some(MeasuredDurableLen((sink.total() - body_start) as u32));",
    ),
    ("marrow-image", "measure.rs", "Ok((self.0 - before) as u32)"),
    (
        "marrow-image",
        "measure.rs",
        "framed[slot] = (counter.total() - framed_start) as u32;",
    ),
    (
        "marrow-image",
        "measure.rs",
        "if has_assert && !is_test_entry(index as u16) {",
    ),
    (
        "marrow-image",
        "measure.rs",
        "let tail = (counter.total() - tail_start) as u32;",
    ),
    (
        "marrow-image",
        "measure.rs",
        "u16::try_from(count).expect( )",
    ),
    (
        "marrow-image",
        "measure.rs",
        "u16::try_from(ordinal).expect( )",
    ),
    (
        "marrow-image",
        "policy_ledger.rs",
        "(len > max).then(|| CurrentValidationOccurrence::at_row(max as u32))",
    ),
    (
        "marrow-image",
        "policy_ledger.rs",
        ".map(|_| CurrentValidationOccurrence::at_row(bounds::MAX_SITES as u32))",
    ),
    (
        "marrow-image",
        "policy_ledger.rs",
        ".map(|row| CurrentValidationOccurrence::at_row(row as u32))",
    ),
    (
        "marrow-image",
        "product.rs",
        ".map(|row| DeclarationNodeOrdinal(u32::try_from(row).expect( )))",
    ),
    (
        "marrow-image",
        "product.rs",
        "let ordinal = DeclarationNodeOrdinal(u32::try_from(index).expect( ));",
    ),
    (
        "marrow-image",
        "product.rs",
        "let start = u32::try_from(self.rows.len()).expect( );",
    ),
    (
        "marrow-image",
        "product.rs",
        "ordinal: CanonicalDeclarationPathOrdinal::RootIndex(u32::try_from(index).ok()?),",
    ),
    (
        "marrow-image",
        "product.rs",
        "u32::try_from(index).expect( );",
    ),
    (
        "marrow-image",
        "product.rs",
        "u32::try_from(self.rows.len())",
    ),
    (
        "marrow-image",
        "product.rs",
        "u32::try_from(self.rows.len()).expect( ),",
    ),
    (
        "marrow-image",
        "site_plan.rs",
        "let Ok(ordinal) = u16::try_from(self.rows.len()).map(SiteId::from_ordinal) else {",
    ),
    (
        "marrow-image",
        "store_digest.rs",
        "payload.extend_from_slice(&(sorted.len() as u32).to_be_bytes());",
    ),
    (
        "marrow-image",
        "value_dag.rs",
        "let count = u32::try_from(self.store.len()).unwrap_or(u32::MAX);",
    ),
    ("marrow-image", "value_dag.rs", "+ size_of::<u32>() as u64"),
    (
        "marrow-image",
        "value_dag.rs",
        "u16::try_from(count).map_err(|_| DurableGraphTooLarge)",
    ),
    // A value-shape node ordinal narrows only after exact-node authentication has
    // validated every child. Exhausting the u32 carrier is bounded by the exact
    // `DraftStateError::CarrierDomain` builder refusal at this mint.
    (
        "marrow-image",
        "value_dag.rs",
        "ordinal: u32::try_from(self.store.len()).map_err(|_| DraftStateError::CarrierDomain)?,",
    ),
    (
        "marrow-verify",
        "verify/decode_code.rs",
        ".binary_search(&(byte_offset as u32))",
    ),
    (
        "marrow-verify",
        "verify/decode_code.rs",
        "let offset = (code.len() - reader.remaining()) as u32;",
    ),
    (
        "marrow-verify",
        "verify/durable.rs",
        "columns.entry(*id).or_insert(column as u16);",
    ),
    (
        "marrow-verify",
        "verify/durable.rs",
        "fields.entry(field.id()).or_insert(position as u16);",
    ),
    (
        "marrow-verify",
        "verify/durable.rs",
        "index_positions.insert(index.id, (global as u16, index.unique));",
    ),
    (
        "marrow-verify",
        "verify/durable.rs",
        "let index = u32::try_from(commands.len())",
    ),
    (
        "marrow-verify",
        "verify/durable.rs",
        "root_positions.insert(root.placement, position as u16);",
    ),
    (
        "marrow-verify",
        "verify/durable.rs",
        "u16::try_from(index).expect( )",
    ),
    (
        "marrow-verify",
        "verify/seal.rs",
        "indexes.extend(seal_root_indexes(root_index as u16, root)?);",
    ),
    (
        "marrow-verify",
        "verify/seal.rs",
        "let function_demands: Vec<ExportDemand> = (0..functions.len() as u16)",
    ),
];
