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
/// `as u16`/`as u32` with any intervening whitespace or blanked comment, and the
/// fallible `u16::try_from`/`u32::try_from`/`try_into` forms that narrow the same
/// carriers by another route.
///
/// Each surviving entry is either a function-family ordinal frozen for the function-slot
/// refounding, a count bounded by its own construct's located diagnostic, or a checked
/// conversion whose refusal is the closed builder-domain error.
#[test]
fn every_narrowing_site_is_pinned_to_its_exact_census() {
    let found = narrowing_sites();
    let expected = sanctioned_narrowing_sites();
    let missing: Vec<_> = expected.iter().filter(|s| !found.contains(s)).collect();
    let added: Vec<_> = found.iter().filter(|s| !expected.contains(s)).collect();
    assert!(
        missing.is_empty() && added.is_empty(),
        "the narrowing census moved. New sites must be adjudicated — either the value is \
         bounded by an exact located diagnostic and the census grows with that proof, or \
         it takes the wide-carrier treatment. Vanished sites must be removed from the \
         census.\n  added: {added:#?}\n  missing: {missing:#?}",
    );
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

/// If a narrowing spelling starts at `at`, its end offset.
fn narrowing_at(code: &str, bytes: &[u8], at: usize) -> Option<usize> {
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
        "analysis.rs",
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
    ("marrow-compile", "compile.rs", ".count() as u16"),
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
    (
        "marrow-compile",
        "lower/registry.rs",
        "self.sigs.accepted_occurrences().count() as u16",
    ),
    ("marrow-compile", "lower/stmts.rs", "Some(value as u32)"),
    ("marrow-compile", "lower/stmts.rs", "field: field as u16,"),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(advance as u32), body.span);",
    ),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(target as u32), span);",
    ),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(top as u32), body.span);",
    ),
    (
        "marrow-compile",
        "lower/stmts.rs",
        "self.push(Instr::Jump(top as u32), body.span);",
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
    (
        "marrow-compile",
        "types/mod.rs",
        "Self(u32::try_from(position).expect( ))",
    ),
    (
        "marrow-compile",
        "types/mod.rs",
        "let func = generics.fn_base + row as u16;",
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
    (
        "marrow-image",
        "encode.rs",
        "code.iter().map(|instr| instr.encoded_len() as u32).sum()",
    ),
    (
        "marrow-image",
        "encode.rs",
        "offset += instr.encoded_len() as u32;",
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
        "let count = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);",
    ),
    (
        "marrow-image",
        "value_dag.rs",
        "u16::try_from(count).map_err(|_| DurableGraphTooLarge)",
    ),
    (
        "marrow-image",
        "value_dag.rs",
        "u32::try_from(self.nodes.len()).map_err(|_| DraftStateError::CarrierDomain)?,",
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
