//! Narrow exact-symbol absence gates over `marrow-compile/src`: the shapes the
//! bounded-diagnostic design deletes must not reappear. Each scan matches an
//! exact type or call shape, never a spelling proxy.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "common/source_projection.rs"]
mod source_projection;
use source_projection::{
    is_test_only_file, last_production_item, production_code, without_literals,
};

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

/// Every `(file, line)` at which `needle` appears in this crate's *code* — the shared
/// projection, with every comment, string, and char literal blanked to spaces and byte
/// offsets and line breaks preserved.
///
/// A scan of raw lines is not a scan for a shape: it reports a needle that appears in a doc
/// comment or a message, and a gate that reports what it must ignore gets its needle
/// narrowed until it stops seeing real code. Reading the projection is what makes each
/// assertion below — an absence, an exact count, or a live subject — a statement about
/// code.
///
/// `#[cfg(test)]` items are deliberately kept: these gates are about every type and call
/// shape the crate's sources name, and a shape declared in a test module is still declared.
/// The production-only scans below say so in their own name.
fn occurrences(needle: &str) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for path in src_files() {
        let source = fs::read_to_string(&path).expect("read source file");
        for line in scanned_lines(&source, needle) {
            found.push((path.clone(), line));
        }
    }
    found
}

/// The 1-based lines of `source` whose *code* carries `needle`.
fn scanned_lines(source: &str, needle: &str) -> Vec<usize> {
    without_literals(source)
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(index, _)| index + 1)
        .collect()
}

/// The plant probe for the scan above: a needle planted in prose is invisible to it, and
/// one planted in code is not.
///
/// Both halves matter, and they fail differently. A scan that sees prose reports what it
/// must ignore, and the repair is always to narrow the needle until the scan stops seeing
/// real code. A scan that sees nothing at all passes every absence it is asked about and
/// satisfies every "this gate has a live subject" check with a comment — which is the
/// blindness this probe exists to keep out.
#[test]
fn the_symbol_scan_reads_code_and_not_prose() {
    let sample = r##"
// forbidden_shape in a line comment
/* forbidden_shape in a block /* nested */ comment */
/// forbidden_shape in a doc comment
fn keeper<'a>(text: &'a str) -> char {
    let quoted = "forbidden_shape in a string";
    let raw = r#"forbidden_shape in a raw string"#;
    let message = format!("forbidden_shape: {text}");
    forbidden_shape();
    'x'
}
"##;

    assert_eq!(
        scanned_lines(sample, "forbidden_shape"),
        vec![9],
        "only the call is code; the comments, the literals, and the message are not",
    );
    assert!(
        scanned_lines(sample, "fn keeper").len() == 1,
        "the projection keeps the code around the blanked regions",
    );
}

/// A5: no collection of un-absorbed `ParsedSource` values can exist because the
/// type is never named — the drive destructures the parse result immediately,
/// absorbing its diagnostics terminal and retaining only the AST and the
/// logical broken status.
#[test]
fn parsed_source_is_never_named_in_the_compiler() {
    let found = occurrences("ParsedSource");
    assert!(
        found.is_empty(),
        "ParsedSource must not be named (A5 un-absorbed-set absence): {found:?}"
    );
}

/// Every production diagnostic producer writes through the one bounded
/// collector; no raw mutable diagnostic vector parameter survives.
#[test]
fn no_mutable_source_diagnostic_vector_exists() {
    let found = occurrences("&mut Vec<SourceDiagnostic");
    assert!(
        found.is_empty(),
        "&mut Vec<SourceDiagnostic> must not exist: {found:?}"
    );
}

/// The collector has no raw-vector merge: syntax terminals enter through
/// `absorb_syntax`, finished compiler terminals through `absorb`.
#[test]
fn no_raw_source_diagnostic_vector_extend_exists() {
    let found = occurrences("extend(Vec<SourceDiagnostic");
    assert!(
        found.is_empty(),
        "extend(Vec<SourceDiagnostic>) must not exist: {found:?}"
    );
}

/// The generic instantiation-limit row is the sole one-row retention exception
/// outside a collector or final output: exactly one `Pending(SourceDiagnostic)`
/// state exists.
#[test]
fn the_one_row_exception_is_declared_exactly_once() {
    let found = occurrences("Pending(SourceDiagnostic)");
    assert_eq!(
        found.len(),
        1,
        "exactly one one-row exception may exist: {found:?}"
    );
}

/// Cross-crate recurrence: the compiler never re-collects raw syntax rows. A
/// syntax terminal enters only through the consuming `absorb_syntax` bridge, so
/// the syntax row type is never held in a vector here.
#[test]
fn no_raw_syntax_diagnostic_vector_exists() {
    let found = occurrences("Vec<Diagnostic>");
    assert!(
        found.is_empty(),
        "Vec<Diagnostic> must not exist in the compiler; absorb the syntax terminal: {found:?}"
    );
}

/// `SourceDiagnostic` declares exactly the two private fields the crate's
/// `compile_fail` privacy doctests name. A `compile_fail` block that names a
/// field which no longer exists still "passes" — it fails to compile for the
/// wrong reason — so the declared field set is pinned here: renaming or adding
/// a field fails this gate and forces the doctests to be rewritten with it.
#[test]
fn source_diagnostic_fields_stay_private() {
    let diag = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("diag.rs"),
    )
    .expect("read the diagnostic owner");
    let body = diag
        .split_once("pub struct SourceDiagnostic {")
        .expect("the public diagnostic type is declared")
        .1;
    // The declaration ends at the first line that is exactly the closing brace, so a
    // doc comment or attribute containing `}` cannot truncate the field list and let a
    // newly added public field pass unseen.
    let fields: Vec<&str> = body
        .lines()
        .map(str::trim)
        .take_while(|line| *line != "}")
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with("#["))
        .map(|line| {
            line.split_once(':')
                .expect("a struct field declares a type")
                .0
        })
        .collect();
    assert_eq!(
        fields,
        ["file", "payload"],
        "the privacy doctests in lib.rs name these exact fields; update them together"
    );

    // The doctests are the enforcement; this gate only pins the names they must use,
    // so assert they actually name each declared field rather than trusting the note.
    let lib = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("read the crate root");
    let doctests = lib
        .split_once("pub mod source_diagnostic_privacy_doctests {")
        .expect("the privacy doctest module is declared")
        .1;
    for field in fields {
        assert!(
            doctests.contains(&format!("diagnostic.{field}")),
            "the privacy doctests must read `diagnostic.{field}`, or a renamed field \
             silently loses its compile_fail proof"
        );
    }
}

/// The collector is one concrete private type: no generic collector or the
/// retired generic counter family reappears.
#[test]
fn the_collector_is_concrete_not_generic() {
    let mut declared = false;
    for path in src_files() {
        let source = fs::read_to_string(&path).expect("read source file");
        declared |= source.contains("struct DiagnosticCollector");
        for forbidden in ["DiagnosticCollector<", "BoundedDiagnosticCounter"] {
            assert!(
                !source.contains(forbidden),
                "{} declares a generic collector shape: `{forbidden}`",
                path.display()
            );
        }
    }
    assert!(
        declared,
        "expected the concrete collector; if it was renamed, update this scan"
    );
}

// --- Bounded analysis facts: the retention shapes this design deletes ---

/// No snapshot field retains a parse tree. The completion and active-call queries
/// re-parse exactly one already-admitted file's already-retained bytes per query, so
/// the retired per-module tree carrier does not reappear and the snapshot's retained
/// field set stays exactly the accounted one.
///
/// The retained-bytes accounting destructures these same fields exhaustively, so a new
/// field is already a build error there; this gate names the set a reader can check
/// against the exported retention term without reading the accounting.
#[test]
fn no_parse_tree_is_retained_for_a_query() {
    let found = occurrences("CompletionModule");
    assert!(
        found.is_empty(),
        "the retired per-module tree carrier must not exist: {found:?}"
    );

    let analysis = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("analysis.rs"),
    )
    .expect("read the analysis fact owner");
    let body = analysis
        .split_once("pub struct AnalysisSnapshot {")
        .expect("the snapshot type is declared")
        .1;
    let fields: Vec<&str> = body
        .lines()
        .map(str::trim)
        .take_while(|line| *line != "}")
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with("#["))
        .map(|line| {
            line.split_once(':')
                .expect("a struct field declares a type")
                .0
        })
        .collect();
    assert_eq!(
        fields,
        [
            "input",
            "revision",
            "diagnostics",
            "hover_facts",
            "broken_files",
            "dependency_gaps",
            "document_symbols",
            "symbol_bounded_files",
        ],
        "a new retained snapshot field changes the exported retention term; \
         update the accounting and this gate together"
    );
}

/// The hover fact — the family this row's staging defect belonged to — is not nameable
/// outside the ledger's own module, so a producer-owned carrier for it is a build error
/// rather than a scan finding.
///
/// `HoverFact` is module-private, not `pub(crate)`. Neither `hover_facts: Vec<HoverFact>`
/// on a lowering struct nor `&mut Vec<HoverFact>` on a helper can be written anywhere
/// else in the crate, which is the property the deleted staging violated: it made one
/// body's live fact set a function of the body's length rather than of the snapshot
/// ceiling. This gate keeps the declaration at that visibility.
#[test]
fn the_hover_fact_is_not_nameable_outside_the_ledger() {
    let widened: Vec<(PathBuf, usize)> = occurrences("pub(crate) struct HoverFact")
        .into_iter()
        .chain(occurrences("pub struct HoverFact"))
        .collect();
    assert!(
        widened.is_empty(),
        "widening `HoverFact` past its module lets a producer declare a carrier for it \
         again: {widened:?}"
    );
    assert_eq!(
        occurrences("struct HoverFact").len(),
        1,
        "expected exactly one `HoverFact` declaration; if it moved, move this gate"
    );
}

/// The fact families whose types stay public or crate-visible cannot be closed by
/// visibility, so they are closed by owner: no field or parameter carrying one of them
/// in bulk is declared outside `analysis.rs`, which is where the ledger and its one
/// bounded document-symbol outline live.
///
/// This is a scan over the shapes it names, not a proof about every possible carrier — a
/// producer that wrapped a fact in a struct of its own would pass it. The hover family,
/// which is the one this row's defect belonged to, is closed structurally instead, by
/// `the_hover_fact_is_not_nameable_outside_the_ledger`.
#[test]
fn no_fact_carrier_outside_the_ledger_exists() {
    for forbidden in [
        "&mut Vec<crate::analysis::HoverFact",
        "&mut Vec<(FileIdentity, SourceSpan)>",
        "Vec<(SourceSpan, Box<str>",
    ] {
        let found = occurrences(forbidden);
        assert!(
            found.is_empty(),
            "editor facts must flow through the scoped sink: `{forbidden}` at {found:?}"
        );
    }
    for carrier in ["Vec<DeclSymbol", "Vec<(FileRef, FactSpan)"] {
        let found: Vec<(PathBuf, usize)> = occurrences(carrier)
            .into_iter()
            .filter(|(path, _)| src_relative(path) != "analysis.rs")
            .collect();
        assert!(
            found.is_empty(),
            "only the ledger's own module carries editor facts in bulk: `{carrier}` at \
             {found:?}"
        );
    }
}

/// The fact ledger is one concrete private owner, structurally the sibling of the
/// diagnostic collector: no generic collector shape and no second diagnostic owner.
#[test]
fn the_fact_ledger_is_one_concrete_owner() {
    let mut declared = false;
    for path in src_files() {
        let source = fs::read_to_string(&path).expect("read source file");
        let code = without_literals(&source);
        declared |= code.contains("struct AnalysisFactCollector");
        for forbidden in ["AnalysisFactCollector<", "AnalysisDiagnosticCollector"] {
            assert!(
                !code.contains(forbidden),
                "{} declares a forbidden collector shape: `{forbidden}`",
                path.display()
            );
        }
    }
    assert!(
        declared,
        "expected the concrete analysis fact ledger; if it was renamed, update this scan"
    );
}

/// The retired attempt/receipt/epoch fact-transaction vocabulary does not reappear. A
/// snapshot bound crossing refuses the whole snapshot and drops every fact together;
/// there is no partial publication to unwind, so no transaction protocol exists to
/// unwind it.
#[test]
fn no_fact_transaction_protocol_vocabulary_exists() {
    for forbidden in [
        "FactAttempt",
        "FactReceipt",
        "FactEpoch",
        "FactTransaction",
        "TerminalToken",
        "DriveOwner",
    ] {
        let found = occurrences(forbidden);
        assert!(
            found.is_empty(),
            "the retired attempt/receipt protocol must not reappear: `{forbidden}` at {found:?}"
        );
    }
}

/// The compact fact coordinate is derived and owned here: it is an index into the
/// snapshot's own input module order, never a second file-identity family and never
/// an edge into the project owner's identity type.
#[test]
fn no_shadow_file_identity_family_exists() {
    for forbidden in ["ProjectFileId", "FileIdentityTable", "FileIdTable"] {
        let found = occurrences(forbidden);
        assert!(
            found.is_empty(),
            "no shadow file-ID family may exist: `{forbidden}` at {found:?}"
        );
    }
}

/// The compact fact coordinates declare exactly the private field the crate's
/// `compile_fail` privacy doctests rely on. A tuple newtype with a private field
/// cannot be constructed or destructured outside the crate; pinning the declaration
/// here keeps the doctests from passing for the wrong reason after a rename.
#[test]
fn fact_coordinates_declare_private_fields() {
    let analysis = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("analysis.rs"),
    )
    .expect("read the analysis fact owner");
    let declaration = "pub(crate) struct FileRef(u16);";
    assert!(
        analysis.contains(declaration),
        "expected the compact coordinate declaration `{declaration}`"
    );
}

/// The public definition fact declares exactly the private fields the crate's coordinate
/// privacy doctests name, so a rename cannot leave a `compile_fail` block passing for the
/// wrong reason.
#[test]
fn definition_fact_fields_stay_private() {
    let analysis = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("analysis.rs"),
    )
    .expect("read the analysis fact owner");
    let body = analysis
        .split_once("pub struct Definition {")
        .expect("the public definition fact is declared")
        .1;
    let fields: Vec<&str> = body
        .lines()
        .map(str::trim)
        .take_while(|line| *line != "}")
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with("#["))
        .map(|line| {
            line.split_once(':')
                .expect("a struct field declares a type")
                .0
        })
        .collect();
    assert_eq!(fields, ["file", "name_span", "declaration_range"]);

    let lib = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("read the crate root");
    let doctests = lib
        .split_once("pub mod fact_coordinate_privacy_doctests {")
        .expect("the coordinate privacy doctest module is declared")
        .1;
    assert!(doctests.contains("definition.file"));
}

// ---- Structural scans over production code only.
//
// These scans read the shared `source_projection`: the source with every comment, string,
// char literal, and `#[cfg(test)]` item blanked to spaces, byte offsets and line breaks
// preserved. The projection's plant probes live here, beside its consumers.

/// Every `(file, line)` at which `needle` appears in production code.
fn production_occurrences(needle: &str) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for path in src_files() {
        if is_test_only_file(&path) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read source file");
        let code = production_code(&source);
        for (index, line) in code.lines().enumerate() {
            if line.contains(needle) {
                found.push((path.clone(), index + 1));
            }
        }
    }
    found
}

fn production_code_of(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    production_code(&fs::read_to_string(path).expect("read source file"))
}

/// The production code of a whole module directory, concatenated.
///
/// A module split across sibling files is still one search space: the siblings see
/// their parent's private items, so scanning only `mod.rs` would leave the rest of
/// the module unscanned while still reporting a clean result.
fn production_code_of_module(module: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(module);
    let mut code = String::new();
    let mut scanned = 0usize;
    for path in src_files() {
        if !path.starts_with(&dir) || is_test_only_file(&path) {
            continue;
        }
        code.push_str(&production_code(
            &fs::read_to_string(&path).expect("read source file"),
        ));
        code.push('\n');
        scanned += 1;
    }
    assert!(
        scanned > 0,
        "the `{module}` module has no production source; the scan root moved"
    );
    code
}

/// A source file's path relative to `src`, as the stable key for the inventories
/// below. The bare file name is ambiguous once a crate has subdirectories — this
/// crate has two `durable.rs` and several `mod.rs` — so two audited sites in
/// different modules would key alike and one could be substituted for the other.
fn src_relative(path: &Path) -> String {
    path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
        .expect("a source path lies under src")
        .to_string_lossy()
        .into_owned()
}

/// The scanner sees code and only code. Each probe is a shape a literal-blind scan would
/// get wrong: a needle hidden in a comment or a string reads as a hit, and a needle in
/// real code beside them must still be found.
#[test]
fn the_production_scanner_reads_code_and_not_prose() {
    let sample = r##"
// forbidden_shape in a line comment
/* forbidden_shape in a block /* nested */ comment */
fn keeper<'a>(text: &'a str) -> char {
    let quoted = "forbidden_shape in a string";
    let raw = r#"forbidden_shape in a raw string"#;
    let byte_raw = br"forbidden_shape and a trailing backslash \";
    let byte_raw_hashed = br#"forbidden_shape with an inner "quote" and a backslash \"#;
    let c_raw = cr"forbidden_shape in a C raw string \";
    let byte_escaped = b"forbidden_shape with an escaped backslash \\";
    let tick = '\'';
    let ticked = ['\'','a', adjacent_call()];
    forbidden_shape();
    'x'
}

#[cfg(test)]
mod tests {
    fn hidden() {
        forbidden_shape();
    }
}

#[cfg(test)]
#[path = "elsewhere.rs"]
mod elsewhere;

fn after(): {}
"##;
    let code = production_code(sample);
    assert_eq!(
        code.matches("forbidden_shape").count(),
        1,
        "exactly the one real call site survives:\n{code}",
    );
    assert!(
        code.contains("fn keeper<'a>(text: &'a str)"),
        "lifetimes are code, not char literals:\n{code}",
    );
    assert_eq!(
        code.matches('\'').count(),
        2,
        "the only quotes the projection keeps are the two lifetime ticks. An escaped-quote \
         literal measured from the backslash rather than past the escape's payload returns \
         3 for the four bytes of `'\\''`, leaving a stray quote that pairs with the next \
         one and blanks the code between them:\n{code}",
    );
    assert!(
        code.contains("adjacent_call()"),
        "the call beside a pair of char literals is code and survives whole:\n{code}",
    );
    assert!(
        code.contains("fn after"),
        "scanning continues past a blanked test module:\n{code}",
    );
    assert!(
        !code.contains("mod tests"),
        "the test module is blanked:\n{code}",
    );
    assert!(
        !code.contains("elsewhere.rs"),
        "an out-of-line test module declaration is blanked:\n{code}",
    );
    assert_eq!(
        code.len(),
        sample.len(),
        "byte offsets are preserved so a reported line is the source's own",
    );
    assert_eq!(
        code.lines().count(),
        sample.lines().count(),
        "line numbering is preserved",
    );
}

/// The projection over the real tree still contains the production shapes it must see,
/// and no longer contains the test-only ones. A scanner that blanked too much would pass
/// every absence gate below while proving nothing.
#[test]
fn the_production_scanner_still_sees_the_real_compiler() {
    let compile = production_code_of("compile.rs");
    for shape in [
        "fn drive(",
        "fn analyze_project(",
        "enum SemanticOutcome",
        "struct Artifacts",
        "fn all_available(",
    ] {
        assert!(
            compile.contains(shape),
            "the projection lost a production shape: {shape}",
        );
    }
    let diag = production_code_of("diag.rs");
    assert!(
        diag.contains("fn absorb("),
        "the projection lost the collector's production surface",
    );
    assert!(
        !diag.contains("fn expect_complete("),
        "a test-only helper must not survive the projection",
    );
}

/// The projection reaches the END of every scanned production file. The sentinels above
/// sit at the top and middle of two files; a blanking bug that runs away — an
/// unterminated literal, an unrecognised raw-string spelling — erases a file's tail
/// instead, and every gate scanning for a shape in that tail then passes because the
/// shape was erased rather than absent. The sentinel is derived per file, so it covers
/// files no list here names and needs no maintenance when a file gains a declaration.
#[test]
fn the_projection_reaches_the_end_of_every_scanned_file() {
    let mut unqualified: Vec<PathBuf> = Vec::new();
    for path in src_files() {
        if is_test_only_file(&path) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read source file");
        let code = production_code(&source);
        let Some(sentinel) = last_production_item(&source) else {
            unqualified.push(path);
            continue;
        };
        // Searched in the projection, not the source: a match in the source can land in a
        // region the projection blanks — the same header text inside a test module — and
        // the gate would then be asking about the tail of the test code it deliberately
        // erased. Every byte of the projection outside a blanked region is the source's.
        let offset = code.rfind(&sentinel).unwrap_or_else(|| {
            panic!(
                "the projection lost the tail of {}: the file's last production item \
                 header `{sentinel}` did not survive blanking",
                path.display(),
            )
        });
        assert_eq!(
            &source[offset..offset + sentinel.len()],
            sentinel,
            "the projection moved the tail of {}: `{sentinel}` survived at a byte offset \
             that is not the one it occupies in the source, so a reported line number \
             would not be the source's own",
            path.display(),
        );
    }
    // A file with no qualifying sentinel is skipped, and a skipped file is a file this
    // gate does not cover. No scanned file is skipped today; if one stops qualifying —
    // its last production item gains a string, a comment, or a char literal — the gate
    // says so instead of quietly narrowing to the files that still happen to qualify.
    assert!(
        unqualified.is_empty(),
        "every scanned file must offer a sentinel this gate can follow to its end; \
         these no longer do: {unqualified:?}",
    );
}

/// The image is produced at exactly one place. `CheckedProgram::encode` is the only
/// caller of `ImageDraft::encode`, and `Driven::into_built` is the only caller of that —
/// so the production projection is the single point at which an image-policy verdict can
/// be taken, and no analysis or tooling path can reach one.
#[test]
fn the_image_is_encoded_at_exactly_one_call_site() {
    let draft_calls = production_occurrences("draft.encode()");
    assert_eq!(
        draft_calls.len(),
        1,
        "ImageDraft::encode has exactly one production call site: {draft_calls:?}",
    );
    let all_calls = production_occurrences(".encode()");
    assert_eq!(
        all_calls.len(),
        2,
        "only the draft call and the checked program's wrapper may encode: {all_calls:?}",
    );
    assert!(
        production_occurrences("ImageDraft::encode").is_empty(),
        "the encoder is reached through the draft, not by an explicit path",
    );
}

/// No phase runs on the strength of an empty diagnostic set. Each continued phase takes
/// its own typed prerequisite, so the only `diagnostics.is_empty()` left in the compiler
/// driver is the non-empty diagnostic set's own constructor guard — and the collector's
/// `is_empty` is test-only, so a production caller cannot reintroduce the pattern at all.
#[test]
fn no_phase_runs_on_an_empty_diagnostic_set() {
    let found = production_occurrences("diagnostics.is_empty()");
    assert_eq!(
        found.len(),
        1,
        "only NonEmptySourceDiagnostics may ask whether a row set is empty: {found:?}",
    );
    let diag = production_code_of("diag.rs");
    assert_eq!(
        diag.matches("fn is_empty(&self) -> bool").count(),
        1,
        "diag.rs declares two logical-emptiness readers — the finished terminal's and \
         the collector's — and exactly the terminal's may be production code; the \
         collector's stays behind cfg(test), which the projection blanks",
    );
}

/// The `debug_assert` sites this crate deliberately keeps, each as its exact statement
/// text. Every one was audited and each is annotated at its line with why the two
/// profiles cannot disagree about it: the guarded condition is unreachable inside a
/// bound the crate already enforces, the write it precedes is idempotent, or nothing
/// downstream reads the value it compares.
///
/// The list is a multiset compared exactly in both directions, so a new site fails and
/// so does a removed one. It pins the statement rather than a per-file count, because a
/// count licenses a *quantity*: deleting one audited guard and adding an unaudited one
/// in the same file substituted silently under a count and does not under this. The
/// statement text is also stable under the line movement that pinning line numbers
/// would churn on.
const PROFILE_GUARD_ALLOWLIST: &[(&str, &str)] = &[
    ("analysis.rs", "debug_assert!("),
    ("analysis.rs", "debug_assert!("),
    (
        "types/metadata.rs",
        "debug_assert!(scratch.seen_rows[index]);",
    ),
    (
        "types/metadata.rs",
        "debug_assert!(scratch.tasks.is_empty());",
    ),
    ("types/mod.rs", "debug_assert_eq!(*active, 1);"),
    ("types/mod.rs", "debug_assert_eq!(*active, 1);"),
    (
        "types/mod.rs",
        "debug_assert_eq!(collections.len(), cache_index);",
    ),
    (
        "types/mod.rs",
        "debug_assert_eq!(id.index() as usize, cache_index);",
    ),
    (
        "types/render.rs",
        "debug_assert_eq!(removed, Some(DisplayNode::Collection(index)));",
    ),
    (
        "types/render.rs",
        "debug_assert_eq!(removed, Some(DisplayNode::Row(row)));",
    ),
    ("types/render.rs", "debug_assert_eq!(removed, Some(node));"),
    ("types/render.rs", "debug_assert_eq!(removed, Some(node));"),
];

/// A profile-dependent guard makes the release build disagree with the debug build about
/// what the compiler proved. None may decide an outcome anywhere in this crate.
///
/// The gate was once scoped to the driver files by name, which said nothing about the
/// checker and analysis owners where every such guard in the crate actually lives. It
/// reads the whole production inventory instead, so a guard added to a file no list here
/// names is a failure rather than a review miss.
#[test]
fn no_profile_dependent_guard_decides_a_compiler_outcome() {
    // A conditional-compilation form has no audited use at all: unlike `debug_assert`,
    // it changes which code exists rather than which checks run.
    for shape in ["cfg!(debug_assertions)", "#[cfg(debug_assertions)]"] {
        let found = production_occurrences(shape);
        assert!(
            found.is_empty(),
            "a profile-dependent guard must not decide a compiler outcome: {shape} at \
             {found:?}",
        );
    }

    let mut found: Vec<(String, String)> = Vec::new();
    for path in src_files() {
        if is_test_only_file(&path) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read source file");
        let code = production_code(&source);
        let name = path
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
            .expect("a source under this crate's src")
            .to_string_lossy()
            .into_owned();
        for line in code.lines() {
            if line.contains("debug_assert") {
                found.push((name.clone(), line.trim().to_string()));
            }
        }
    }
    found.sort();
    let mut expected: Vec<(String, String)> = PROFILE_GUARD_ALLOWLIST
        .iter()
        .map(|(file, statement)| ((*file).to_string(), (*statement).to_string()))
        .collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "the audited `debug_assert` sites changed. A new one needs the audit the \
         allowlist records — that the two profiles cannot disagree about it — and a \
         removed one needs its entry dropped so this list keeps saying what is actually \
         there",
    );
}

/// The row derives no pending-diagnostic representation and mints no per-artifact cause
/// carrier. The substrate exports no receipt, terminal token, epoch, or candidate key,
/// and this row does not invent one.
#[test]
fn no_attempt_or_receipt_vocabulary_exists() {
    for shape in [
        "attempt",
        "Attempt",
        "receipt",
        "Receipt",
        "epoch",
        "Epoch",
        "candidate_key",
        "CandidateKey",
        "terminal_token",
        "TerminalToken",
    ] {
        let found = production_occurrences(shape);
        assert!(
            found.is_empty(),
            "no per-artifact cause carrier vocabulary may exist: {shape} at {found:?}",
        );
    }
}

/// E6 — the durable registry's parallel steering machinery is gone, not merely
/// unused.
///
/// Each of these answered one part of a question the ledger now answers whole: was
/// this root declared, was it parked, was it refused, and has a use of it already
/// been steered. Leaving any of them reachable would let a caller reconstruct the
/// four-way answer from probes again, which is how nine refusal causes came to share
/// one steer.
#[test]
fn the_durable_steering_probes_are_deleted() {
    for deleted in [
        "admission_failed_roots",
        "admission_failed_root_named",
        "not_yet_executable_root_named",
        "admission_steered",
        "StoreBuild::IdentityIncomplete",
        "StoreBuild::Rejected",
    ] {
        let found = production_occurrences(deleted);
        assert!(
            found.is_empty(),
            "`{deleted}` is replaced by the declaration ledger and must not exist: {found:?}"
        );
    }
}

/// The plant-probe for the scan above, in both directions: it must find a needle that
/// is present in production code, and must not find one that appears only inside a
/// string literal. A scan that cannot fail is not a gate.
#[test]
fn the_deletion_scan_reads_production_code_and_only_production_code() {
    assert!(
        !production_occurrences("fn refuse_store").is_empty(),
        "the scan must see production code; if `refuse_store` was renamed, update this probe"
    );
    assert!(
        production_occurrences("{}.{}").is_empty(),
        "the scan must not read string literals: this format spelling exists only \
         inside one, and it is not a sentence a diagnostic rewording could delete"
    );
}

/// E5c — a refusal summary built without a report of its own names the pass that
/// covers it, on an exact allowlist.
///
/// `refuse_covered` is the one constructor that mints a retained cause without
/// pushing the row that reports it. Two shapes need it and no third may: a cause
/// whose owning pass runs later, and a cause an earlier occurrence of the same
/// project-wide anchor already reported. Left open, it would become the way a
/// refusal escapes being reported at all — a refused declaration with no diagnostic
/// anywhere, which is worse than the fabricated absence this lane removes.
#[test]
fn every_covered_refusal_names_its_covering_report() {
    let calls = production_occurrences("refuse_covered(");
    let sites: Vec<(String, usize)> = calls
        .iter()
        .map(|(path, line)| (src_relative(path), *line))
        .collect();
    // The definition, the two durable classes that use it — a repeated project-wide
    // identity anchor, reported by the first store to reach it, and a durable value
    // cycle, reported by `types::reject_value_cycles` after lowering — the signature
    // whose annotation names a cause an earlier use already steered to or the shared
    // instantiation limit, and the two declaration sites of that same limit, which
    // the monomorphization owner reports once for the whole pass rather than at each
    // struct field or resource member that hit it.
    let allowed = [
        "decl.rs",
        "durable.rs",
        "durable.rs",
        "lower/registry.rs",
        "types/build.rs",
        "types/build.rs",
    ];
    let names: Vec<&str> = sites.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names, allowed,
        "a new `refuse_covered` call site must name the pass that reports its cause \
         and be added here deliberately: {sites:?}"
    );
}

/// E6 — the module and signature namespaces' parallel probe sets are gone, not
/// merely unused.
///
/// Each answered one part of a question its ledger now answers whole: is this
/// dotted path a module of the project, did it parse, did every signature resolve,
/// and did the region behind the signature table run at all. Leaving any of them
/// reachable would let a caller reconstruct the answer from probes again, which is
/// how a module the reader can see came to be reported as one the project does not
/// contain.
#[test]
fn the_module_and_signature_probes_are_deleted() {
    for deleted in [
        "broken_modules",
        "names_broken_module",
        "module_names",
        "FunctionRegistryOutcome",
        "RegistryPhases::unavailable",
    ] {
        let found = production_occurrences(deleted);
        assert!(
            found.is_empty(),
            "`{deleted}` is replaced by the declaration ledger and must not exist: {found:?}"
        );
    }
}

/// E7 — the ledger owns no diagnostic terminal.
///
/// A refusal's row and its retained summary are one statement in the namespace's
/// own collector. A collector constructed inside `decl.rs` would be a second
/// terminal whose rows nothing seals, so a retained cause could name a report no
/// stage ever reports.
#[test]
fn the_declaration_ledger_constructs_no_diagnostic_collector() {
    let code = production_code_of("decl.rs");
    for shape in ["DiagnosticCollector::new(", ".finish()"] {
        assert!(
            !code.contains(shape),
            "`decl.rs` must not own a diagnostic terminal: `{shape}` is present",
        );
    }
}

/// E2 — a declaration builder cannot push a diagnostic and drop the key.
///
/// The shape this lane exists to kill is `diagnostics.push(row); continue;` inside
/// a block that builds a namespace: the row reports the cause once, and every later
/// lookup of the dropped name then reads as *never declared*. Refusing through
/// `refuse`/`refuse_row`/`refuse_first` makes the pushed row and the retained
/// summary one statement, so a raw push that leaves such a block without declaring
/// is the defect reappearing.
///
/// Scanned over the production projection — comments, every string spelling, and
/// `#[cfg(test)]` items blanked, offsets preserved — by brace depth rather than by
/// line, and bounded to blocks that declare, over an exact allowlist of the one
/// category that legitimately drops nothing: a report about a name an earlier
/// declaration already holds, and a `use` import, which is not a ledger namespace
/// at all. Both leave the name answerable, so neither can fabricate an absence.
#[test]
fn no_declaration_builder_pushes_a_row_and_drops_the_key() {
    // Keyed by `(file, reported code)` with an exact count, not by position: two
    // sites in one file reporting the same cause are indistinguishable in a
    // positional list, so one could be swapped for the other with the list still
    // matching, and any reordering of a file would read as a change.
    let allowed: BTreeMap<(&str, &str), usize> = BTreeMap::from([
        (("compile.rs", "Code::CheckImport"), 3),
        // The whole pass stops on the shared instantiation limit, merging the
        // monomorphization owner's rows into the terminal and returning: no
        // declaration is dropped because no declaration is reached.
        (("compile.rs", "?"), 1),
        (("durable.rs", "Code::CheckType"), 1),
        (("konst.rs", "Code::CheckNameConflict"), 1),
        (("types/build.rs", "Code::CheckNameConflict"), 9),
        (("types/build.rs", "Code::CheckType"), 1),
    ]);
    let found = declaration_drop_sites();
    let mut shapes: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (file, _, code) in &found {
        *shapes.entry((file.as_str(), code.as_str())).or_default() += 1;
    }
    assert_eq!(
        shapes, allowed,
        "a declaration builder wrote a diagnostic and left its block without \
         declaring the key. Refuse through `refuse`/`refuse_row`/`refuse_first` so \
         the pushed row and the retained cause are one statement; a site that \
         genuinely drops nothing is added here deliberately: {found:?}",
    );
}

/// The plant-probe for the scan above, in every direction it could disable itself:
/// reading literals or comments as code, missing a drop that sits on another line,
/// mistaking a refusal for a drop, mistaking a report-only check for a builder, and
/// finding no declaring block in the real tree at all.
#[test]
fn the_declaration_drop_scan_finds_a_planted_drop_and_only_a_real_one() {
    let planted = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        let commentary = "diagnostics.push(row); continue;";
        let raw = r#"diagnostics.push(row); continue;"#;
        // diagnostics.push(row); continue;
        if bad {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                span,
                message,
            ));
            continue;
        }
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(planted)),
        vec![(8, "Code::CheckUnsupported".to_string())],
        "the planted drop is found, and the three literal and comment copies of it \
         are not",
    );

    // The two ways a receiver-spelled scan disables itself: rebinding the collector,
    // and handing it to a helper that reports on the block's behalf. Both are the
    // same drop written differently, and both are permanent probes here.
    let aliased = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        let sink = &mut *diagnostics;
        if bad {
            sink.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                span,
                message,
            ));
            continue;
        }
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(aliased)),
        vec![(6, "Code::CheckUnsupported".to_string())],
        "a rebound receiver is the same drop and must be found",
    );

    // The rebinding written `let mut`, which a scan reading the word after `let ` as
    // the bound name loses entirely.
    let aliased_mut = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        let mut sink = &mut *diagnostics;
        if bad {
            sink.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                span,
                message,
            ));
            continue;
        }
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(aliased_mut)),
        vec![(6, "Code::CheckUnsupported".to_string())],
        "a `let mut` rebinding is the same drop and must be found",
    );

    let delegated = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        if bad {
            report_the_defect(diagnostics, at, Code::CheckUnsupported.as_str());
            continue;
        }
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(delegated)),
        vec![(5, "Code::CheckUnsupported".to_string())],
        "handing the collector to a helper is the same drop and must be found",
    );

    let coupled = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        if bad {
            refuse_first(&mut refusal, diagnostics, at, row);
            continue;
        }
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert!(
        drop_sites_in(&production_code(coupled)).is_empty(),
        "a coupling constructor retains the cause it reports and is not a drop",
    );

    let refused = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        let occurrence = match resolve(item) {
            Ok(value) => DeclarationOccurrence::Accepted(value),
            Err(_) => DeclarationOccurrence::Refused(refuse_row(diagnostics, at, row)),
        };
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert!(
        drop_sites_in(&production_code(refused)).is_empty(),
        "refusing through the one constructor is not a drop",
    );

    // The three ways a scan keyed on the bare identifier disables itself: reaching
    // the collector through a field of another value, through a field of `self` —
    // a live production spelling — and through an accessor that returns it. All
    // three are the same drop, and all three are permanent probes here.
    let field_of_a_value = r##"
fn build(cx: &mut Context) {
    for item in items {
        if bad {
            cx.diagnostics.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                span,
                message,
            ));
            continue;
        }
        cx.ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(field_of_a_value)),
        vec![(5, "Code::CheckUnsupported".to_string())],
        "a collector reached through a field is the same drop and must be found",
    );

    let field_of_self = r##"
fn build(&mut self) {
    for item in items {
        if bad {
            self.diagnostics
                .push(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    file,
                    span,
                    message,
                ));
            continue;
        }
        self.ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(field_of_self)),
        vec![(5, "Code::CheckUnsupported".to_string())],
        "a collector reached through a field of `self`, across a line break, is the \
         same drop and must be found",
    );

    let accessor = r##"
fn build(&mut self) {
    for item in items {
        if bad {
            self.diagnostics().push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                span,
                message,
            ));
            continue;
        }
        self.ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(accessor)),
        vec![(5, "Code::CheckUnsupported".to_string())],
        "a collector reached through an accessor is the same drop and must be found",
    );

    // A closure moves the write off the statement list: nothing is pushed at the
    // drop, and the block reports and leaves all the same.
    let captured = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        let mut report = |row| diagnostics.push(row);
        if bad {
            report(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                span,
                message,
            ));
            continue;
        }
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert_eq!(
        drop_sites_in(&production_code(captured)),
        vec![(6, "Code::CheckUnsupported".to_string())],
        "a push captured in a closure is the same drop, made where the closure is \
         called",
    );

    // `.push(` on anything else is not a report. A receiver-reading scan that took
    // every push would report the whole tree and be turned off.
    let unrelated_push = r##"
fn build(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        if bad {
            params.push(resolved);
            self.diagnostics_seen.push(name);
            continue;
        }
        ledger.declare(key, occurrence)?;
    }
}
"##;
    assert!(
        drop_sites_in(&production_code(unrelated_push)).is_empty(),
        "a push onto something that is not the collector is not a report",
    );

    // A block that never declares is not a declaration builder, so a report-only
    // check that pushes and continues is not this defect.
    let report_only = r##"
fn reject_duplicates(diagnostics: &mut DiagnosticCollector) {
    for item in items {
        if seen {
            diagnostics.push(row);
            continue;
        }
    }
}
"##;
    assert!(
        drop_sites_in(&production_code(report_only)).is_empty(),
        "a check that declares nothing is not a declaration builder",
    );

    // The scan must reach declaring blocks in the real tree; if every ledger owner
    // were renamed out from under it, it would pass by scanning nothing.
    let blocks = declaring_block_count();
    assert!(
        blocks >= 6,
        "the scan must see the real declaration builders; it found only {blocks}",
    );
}

/// Every production site at which a row is pushed and the enclosing declaring block
/// is left without declaring the key, as `(file name, line, pushed code)`.
fn declaration_drop_sites() -> Vec<(String, usize, String)> {
    let mut found = Vec::new();
    for path in src_files() {
        if is_test_only_file(&path) {
            continue;
        }
        let name = src_relative(&path);
        let source = fs::read_to_string(&path).expect("read source file");
        for (line, code) in drop_sites_in(&production_code(&source)) {
            found.push((name.clone(), line, code));
        }
    }
    found
}

/// How many declaring blocks the scan finds across production code, so the gate can
/// prove it is scanning something.
fn declaring_block_count() -> usize {
    let mut count = 0;
    for path in src_files() {
        if is_test_only_file(&path) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read source file");
        count += declaring_blocks(&production_code(&source)).len();
    }
    count
}

/// The `(open, close)` byte offsets of every block whose own statements call
/// `declare(` — the blocks that build a namespace.
fn declaring_blocks(code: &str) -> Vec<(usize, usize)> {
    let bytes = code.as_bytes();
    let mut opens: Vec<usize> = Vec::new();
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for (at, byte) in bytes.iter().enumerate() {
        match byte {
            b'{' => opens.push(at),
            b'}' => {
                if let Some(open) = opens.pop()
                    && code[open + 1..at].contains(".declare(")
                {
                    blocks.push((open, at));
                }
            }
            _ => {}
        }
    }
    blocks.sort_unstable();
    blocks
}

/// The constructors that couple a pushed row to the retained cause. A call handing
/// the collector to one of these is the sanctioned refusal, not a drop: the summary
/// it returns is what the block goes on to declare, and a block that discards it
/// still fails this scan through the `declare(` it never reaches.
const COUPLING_CONSTRUCTORS: &[&str] = &[
    "refuse",
    "refuse_row",
    "refuse_first",
    "refuse_covered",
    "refuse_store",
    "refuse_annotation",
    "refuse_at_earlier_stage",
];

/// Every `let` binding in `body`, as `(bound name, offset of the bound value)`.
///
/// An optional `mut` is skipped: `let mut sink = &mut *diagnostics;` binds `sink`,
/// and reading `mut` as the name would lose the alias and disable the scan for
/// exactly the rebinding it is meant to follow.
fn let_bindings(body: &str) -> Vec<(&str, usize)> {
    let mut bindings = Vec::new();
    let mut at = 0usize;
    while let Some(hit) = body[at..].find("let ") {
        let mut start = at + hit + "let ".len();
        at = start;
        if body[start..].starts_with("mut ") {
            start += "mut ".len();
        }
        let rest = &body[start..];
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            continue;
        }
        let tail_at = start + end;
        let trimmed = body[tail_at..].trim_start();
        let Some(value) = trimmed.strip_prefix('=') else {
            continue;
        };
        let value_at = body.len() - value.trim_start().len();
        bindings.push((&rest[..end], value_at));
    }
    bindings
}

/// The names that stand for the diagnostic collector inside one block: its own
/// identifier, every simple rebinding of it, and every closure that writes to it.
///
/// A rebinding is what defeats a receiver-spelled scan — `let sink = &mut
/// *diagnostics; sink.push(row); continue;` is the same drop written differently —
/// so the alias is followed rather than the spelling trusted. A closure moves the
/// write off the statement list entirely: `let mut report = |row|
/// diagnostics.push(row); … report(row); continue;` reports and drops with no push
/// in sight at the drop, so a closure whose body writes to the collector binds its
/// name into this set too.
fn collector_names<'a>(body: &'a str, closures: &[&'a str]) -> Vec<&'a str> {
    let mut names = vec!["diagnostics"];
    names.extend(closures.iter().copied());
    for (name, value_at) in let_bindings(body) {
        let value = &body[value_at..];
        for bound in names.clone() {
            for prefix in ["&mut *", "&mut ", "&*", "&", ""] {
                let spelled = format!("{prefix}{bound}");
                if value.starts_with(&spelled)
                    && !value[spelled.len()..]
                        .starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.')
                {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                    break;
                }
            }
        }
    }
    names
}

/// The names bound to a closure whose body writes to the collector, so a call
/// through the name counts as the write it performs.
fn closure_aliases<'a>(body: &'a str, writes: &[usize]) -> Vec<&'a str> {
    let mut aliases = Vec::new();
    for (name, value_at) in let_bindings(body) {
        let Some((start, end)) = closure_extent(body, value_at) else {
            continue;
        };
        if writes.iter().any(|write| (start..end).contains(write)) && !aliases.contains(&name) {
            aliases.push(name);
        }
    }
    aliases
}

/// The byte range of the closure bound at `value_at`, if the bound value is one: its
/// parameter list through its body, to the closing brace of a block body or to the
/// statement's own `;`.
fn closure_extent(body: &str, value_at: usize) -> Option<(usize, usize)> {
    let bytes = body.as_bytes();
    let mut at = value_at;
    if body[at..].starts_with("move ") {
        at += "move ".len();
    }
    if *bytes.get(at)? != b'|' {
        return None;
    }
    at += 1;
    while *bytes.get(at)? != b'|' {
        at += 1;
    }
    at += 1;
    let mut depth = 0i32;
    while at < bytes.len() {
        match bytes[at] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' if depth == 1 => return Some((value_at, at + 1)),
            b'}' | b')' | b']' => {
                depth -= 1;
                if depth < 0 {
                    return Some((value_at, at));
                }
            }
            b';' if depth == 0 => return Some((value_at, at)),
            _ => {}
        }
        at += 1;
    }
    Some((value_at, bytes.len()))
}

/// Every offset in `body` at which a `.push(` is written on a receiver expression
/// that names the diagnostic collector, reported at the start of that receiver.
///
/// The receiver is read rather than one spelling matched, so a scan keyed on the
/// bare identifier — which sees `diagnostics.push(` and nothing else — is not
/// defeated by reaching the same collector through a field (`cx.diagnostics.push(…)`
/// and `self.diagnostics.push(…)`, a live production spelling) or through an
/// accessor (`self.diagnostics().push(…)`). A `.push(` on anything else — a vector
/// of parameters, of variants, of rows being built — is not a report and is not read
/// here.
///
/// A collector reached under an unrelated identifier, such as a `self.diags()`
/// accessor, is outside this rule: keying on arbitrary accessor names would read
/// every `.push(` in the tree as a report. Naming the collector field or accessor
/// `diagnostics` is what keeps it in reach.
fn receiver_writes(body: &str) -> Vec<usize> {
    let mut writes = Vec::new();
    let mut at = 0usize;
    while let Some(hit) = body[at..].find(".push(") {
        let push = at + hit;
        at = push + 1;
        let start = receiver_start(body, push);
        if names_the_collector(&body[start..push]) {
            writes.push(start);
        }
    }
    writes
}

/// The start offset of the receiver expression whose `.push(` sits at `push`: the
/// call/field/index chain to its left, with bracketed groups crossed whole and a
/// line break inside the chain followed.
fn receiver_start(body: &str, push: usize) -> usize {
    let bytes = body.as_bytes();
    let mut at = push;
    let mut depth = 0i32;
    while at > 0 {
        let byte = bytes[at - 1];
        match byte {
            b')' | b']' => depth += 1,
            b'(' | b'[' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            b'.' | b'?' | b'_' => {}
            _ if byte.is_ascii_alphanumeric() => {}
            // A chain broken across lines is one receiver; anything else to the left
            // of the break ends it.
            _ if byte.is_ascii_whitespace() => {
                let head = body[..at].trim_end();
                if !head.ends_with(|c: char| c.is_alphanumeric() || "_).]".contains(c)) {
                    return at;
                }
                at = head.len() + 1;
            }
            _ => return at,
        }
        at -= 1;
    }
    at
}

/// Whether `receiver` contains `diagnostics` as a whole identifier.
fn names_the_collector(receiver: &str) -> bool {
    let mut at = 0usize;
    while let Some(hit) = receiver[at..].find("diagnostics") {
        let start = at + hit;
        at = start + 1;
        let before = receiver[..start].chars().next_back();
        let after = receiver[start + "diagnostics".len()..].chars().next();
        if !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
            && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            return true;
        }
    }
    false
}

/// The callee name of the call whose argument list encloses `at`, if any: the
/// identifier before the nearest unmatched `(` to the left.
fn enclosing_callee(body: &str, at: usize) -> Option<&str> {
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut cursor = at;
    while cursor > 0 {
        cursor -= 1;
        match bytes[cursor] {
            b')' => depth += 1,
            b'(' if depth == 0 => {
                let head = &body[..cursor];
                let start = head
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |boundary| boundary + 1);
                return Some(&head[start..]);
            }
            b'(' => depth -= 1,
            b'{' | b'}' | b';' => return None,
            _ => {}
        }
    }
    None
}

/// Every offset inside `body` at which the diagnostic collector is written to: a
/// `.push(` on any receiver naming the collector, a `.push(` on a collector name, a
/// call through a closure that writes to it, and a call handing a collector name to
/// a callee that is not one of the coupling constructors.
///
/// Every form is needed, and each is a way one of the others is defeated. A scan for
/// `diagnostics.push(` alone is defeated by rebinding the receiver, by reaching the
/// collector through a field or accessor, and by hiding the push in a closure; a
/// scan for the receiver alone is defeated by handing the collector to a helper that
/// pushes on the block's behalf.
fn collector_writes(body: &str) -> Vec<usize> {
    let mut writes = receiver_writes(body);
    let closures = closure_aliases(body, &writes);
    for name in collector_names(body, &closures) {
        let mut at = 0usize;
        while let Some(hit) = body[at..].find(name) {
            let start = at + hit;
            at = start + 1;
            let before = body[..start].chars().next_back();
            let rest = &body[start + name.len()..];
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            if rest.starts_with(".push(") {
                writes.push(start);
                continue;
            }
            // Calling a closure that writes to the collector is that write, made at
            // the call rather than where the closure was bound.
            if rest.starts_with('(') && closures.contains(&name) {
                writes.push(start);
                continue;
            }
            if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            match enclosing_callee(body, start) {
                Some(callee) if !COUPLING_CONSTRUCTORS.contains(&callee) => writes.push(start),
                _ => {}
            }
        }
    }
    writes.sort_unstable();
    writes.dedup();
    writes
}

/// The 1-based line and pushed code of every write to the diagnostic collector
/// inside a declaring block whose own statement list leaves that block — by
/// `continue`, `break`, or `return` — before reaching a `declare(`.
fn drop_sites_in(code: &str) -> Vec<(usize, String)> {
    let mut sites: Vec<(usize, String)> = Vec::new();
    for (open, close) in declaring_blocks(code) {
        let body = &code[open + 1..close];
        for write in collector_writes(body) {
            if leaves_without_declaring(body, write) {
                sites.push((
                    code[..open + 1 + write].lines().count(),
                    pushed_code(&body[write..]),
                ));
            }
        }
    }
    sites.sort_unstable();
    sites.dedup();
    sites
}

/// The `Code::` constant a pushed row names, which is what the allowlist is keyed
/// on: a site that starts reporting a different cause is a different contract and
/// has to be looked at again. `?` when the row is built elsewhere.
fn pushed_code(rest: &str) -> String {
    let window = &rest[..rest.len().min(160)];
    match window.find("Code::") {
        Some(at) => {
            let tail = &window[at..];
            let end = tail
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                .unwrap_or(tail.len());
            tail[..end].to_string()
        }
        None => "?".to_string(),
    }
}

/// Whether the statement list containing `push` reaches `continue`/`break`/`return`
/// at its own nesting level before any `declare(` at that level.
fn leaves_without_declaring(body: &str, push: usize) -> bool {
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut at = push;
    while at < bytes.len() {
        match bytes[at] {
            b'{' => depth += 1,
            // The end of the statement list this push sits in.
            b'}' if depth == 0 => return false,
            b'}' => depth -= 1,
            _ if depth == 0 => {
                let rest = &body[at..];
                if rest.starts_with(".declare(") {
                    return false;
                }
                for exit in ["continue", "break", "return"] {
                    if rest.starts_with(exit)
                        && !rest[exit.len()..]
                            .starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
        at += 1;
    }
    false
}

/// No non-encodable stand-in stands for a refused operation site.
///
/// While the compiler's descriptors and instruction operands carried a bare `u16` site, a
/// site the bounded plan refused had to travel through that same numeric channel, and
/// `OVER_POLICY_SITE` was the reserved number that did it. A reserved number is only ever
/// one careless comparison away from being read as a real site. The refusal is now a state
/// of the opaque site operand itself, which carries no number at all, so no such constant
/// can exist.
#[test]
fn no_reserved_number_stands_for_a_refused_site() {
    for needle in ["OVER_POLICY_SITE", "u16::MAX, |site|"] {
        let found = occurrences(needle);
        assert!(
            found.is_empty(),
            "`{needle}` reintroduces a numeric stand-in for a refused site: {found:?}",
        );
    }
}

/// NOMLEAF01: a reserved value-type row is never removed from the registry.
///
/// Pass one reserves every struct's and enum's image id before any body resolves, so
/// an earlier fill pass binds that reservation for a verdict that does not exist
/// yet. Removing a refused declaration's row leaves every such reference addressing
/// nothing, and a dangling type argument is a `GenericInvariant` — which outranks
/// diagnostics, so the cause reported at the declaration never reaches the reader.
/// A refused declaration therefore keeps its row and records
/// `DeclarationVerdict::Refused`; only the name lookups exclude it.
#[test]
fn no_reserved_value_type_row_is_removed_from_the_registry() {
    for shape in [
        "structs.retain",
        "enums.retain",
        "structs.remove",
        "enums.remove",
        "structs.swap_remove",
        "enums.swap_remove",
    ] {
        let found = production_occurrences(shape);
        assert!(
            found.is_empty(),
            "a reserved value-type row must stay addressable so a reference minted \
             before its verdict existed cannot dangle: {shape} at {found:?}",
        );
    }
}

/// A traversal of the struct or enum table, flattened to one line: the text from
/// the table's field access through the end of the chain that reads it.
///
/// Whitespace is collapsed so a rustfmt-wrapped chain reads as one expression, which
/// is what lets the classifier below see a predicate and its scan together.
fn value_type_table_traversals(code: &str) -> Vec<String> {
    let flat = code
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" .", ".")
        .replace(". ", ".");
    let mut found = Vec::new();
    for table in [".structs", ".enums"] {
        let mut from = 0;
        while let Some(offset) = flat[from..].find(table) {
            let start = from + offset;
            found.push(flat[start..start + traversal_extent(&flat[start..])].to_string());
            from = start + table.len();
        }
    }
    found
}

/// How far a traversal beginning at `text` reaches: to the delimiter that closes the
/// construct enclosing it — the end of the chain, of the `for` body, or of the
/// function. Bounding by nesting rather than by a character budget is what keeps a
/// predicate written several lines below its scan inside the scan's own region.
fn traversal_extent(text: &str) -> usize {
    let mut depth = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => return index,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
    }
    text.len()
}

/// Every traversal that answers a *source spelling* — the shape that must exclude a
/// refused row — paired with whether it does.
///
/// A name-keyed traversal compares a row's `name` field against a searched-for
/// value. A traversal that reaches its row by index (`[row]`, `.get(row)`) is an
/// id lookup that happens to read the row's name for display; it resolves a
/// reserved id on purpose and must not filter.
fn unfiltered_value_type_name_lookups(code: &str) -> Vec<String> {
    value_type_table_traversals(code)
        .into_iter()
        .filter(|traversal| {
            let name_keyed =
                traversal.contains("name ==") || traversal.contains("name.as_str() ==");
            let mut head = traversal.len().min(60);
            while !traversal.is_char_boundary(head) {
                head -= 1;
            }
            let head = &traversal[..head];
            let row_indexed = head.contains("[row]") || head.contains(".get(row)");
            name_keyed && !row_indexed && !traversal.contains("verdict.is_accepted()")
        })
        .collect()
}

/// Every lookup over the value-type tables that answers a source spelling excludes a
/// refused row, and every lookup that resolves a reserved id does not.
///
/// The candidate set is *derived*, not pinned: the tables are module-private fields
/// of the registry, so the `types` module is the only code that can traverse them, and every
/// traversal there that compares a row's name is required to test the verdict. A
/// sixth name lookup therefore cannot join the family unfiltered — it is classified
/// by its own shape, not by having been listed here.
///
/// The defect this bounds is not a missing report but a wrong resolution: a refused
/// row is retained with its image index reserved and its body never filled, so a
/// lookup that answers its name hands back a *live empty type*. Every later question
/// asked of that type — a field read, a match arm, a return check — then fabricates
/// an answer about source the compiler had already diagnosed.
#[test]
fn every_value_type_name_lookup_excludes_a_refused_row() {
    let code = production_code_of_module("types");
    assert!(
        unfiltered_value_type_name_lookups(&code).is_empty(),
        "a lookup keyed on a source spelling must answer only for an accepted \
         declaration, so a refused one reaches the ledger steer instead: {:#?}",
        unfiltered_value_type_name_lookups(&code),
    );

    // The classifier is proved live on this exact file rather than assumed: with the
    // filter removed from the source it scans, it must report the two owners it is
    // there to hold. A derivation that silently matched nothing would pass the
    // assertion above and fail here.
    let unfiltered = code.replace(" && info.verdict.is_accepted()", "");
    assert_eq!(
        unfiltered_value_type_name_lookups(&unfiltered).len(),
        2,
        "the registry answers a value-type spelling once per table, and the \
         classifier sees both: {:#?}",
        unfiltered_value_type_name_lookups(&unfiltered),
    );

    for lookup in ["fn struct_by_type", "fn enum_by_id"] {
        let start = code
            .find(lookup)
            .unwrap_or_else(|| panic!("{lookup} is declared in the type registry"));
        let body = &code[start..start + 200];
        assert!(
            !body.contains("verdict"),
            "{lookup} resolves a reserved id, including one a later pass refused; \
             filtering it would restore the dangling reference: {body}",
        );
    }
}

/// The classifier's plant probes. A derived gate that matches nothing passes for the
/// wrong reason, so each shape it must catch — and each it must leave alone — is
/// planted here rather than trusted.
#[test]
fn the_value_type_lookup_classifier_catches_a_new_unfiltered_scan() {
    let planted = "fn static_struct_by_name(&self, name: &str) -> Option<&StructInfo> {\n\
                   \x20   self.structs\n\
                   \x20       .iter()\n\
                   \x20       .find(|info| info.name == name)\n\
                   }\n";
    assert_eq!(
        unfiltered_value_type_name_lookups(planted).len(),
        1,
        "a sixth name lookup added without the verdict filter is caught by its shape",
    );

    let filtered = "fn static_struct_by_name(&self, name: &str) -> Option<&StructInfo> {\n\
                    \x20   self.structs\n\
                    \x20       .iter()\n\
                    \x20       .find(|info| info.name == name && info.verdict.is_accepted())\n\
                    }\n";
    assert!(unfiltered_value_type_name_lookups(filtered).is_empty());

    let delegating = "fn static_struct_by_name(&self, name: &str) -> Option<StructInfo> {\n\
                      \x20   self.view.registry.struct_by_name(name).cloned()\n\
                      }\n";
    assert!(
        unfiltered_value_type_name_lookups(delegating).is_empty(),
        "a reader that delegates to the filtered owner traverses no table at all",
    );

    let by_row = "fn display_name(&self, row: usize) -> &str {\n\
                  \x20   self.structs[row].name.as_str() == text.as_str()\n\
                  }\n";
    assert!(
        unfiltered_value_type_name_lookups(by_row).is_empty(),
        "a row-indexed read resolves a reserved id on purpose and must not filter",
    );
}

/// The module scan's own probe: it must aggregate the whole directory, not just
/// `mod.rs`. A regression to a single file would leave the derived gates above
/// reporting clean over a fraction of their search space, which is the failure mode
/// they exist to prevent.
#[test]
fn the_module_scan_covers_every_sibling_not_only_mod_rs() {
    let whole = production_code_of_module("types");
    let only_mod = production_code_of("types/mod.rs");
    assert!(
        whole.len() > only_mod.len(),
        "the `types` module scan returned no more than `mod.rs` alone; the sibling \
         files are not being read"
    );
    for sibling in [
        "fn build_nominals(",
        "fn place_generic_row(",
        "fn render_best_effort_display(",
    ] {
        assert!(
            whole.contains(sibling),
            "the `types` module scan is missing `{sibling}`, which lives in a sibling \
             file rather than in `mod.rs`"
        );
        assert!(
            !only_mod.contains(sibling),
            "`{sibling}` moved back into `mod.rs`; pick a probe that still proves the \
             siblings are read"
        );
    }
}

/// The tables the gate above derives over are module-private, which is what makes
/// the `types` module the whole search space. A `pub(crate)` on either field would put a
/// name lookup outside the derivation with nothing to catch it.
#[test]
fn the_value_type_tables_are_private_to_their_registry() {
    let code = production_code_of_module("types");
    for field in ["structs: Vec<StructInfo>", "enums: Vec<EnumInfo>"] {
        let start = code
            .find(field)
            .unwrap_or_else(|| panic!("{field} is declared in the type registry"));
        let line_start = code[..start].rfind('\n').map_or(0, |index| index + 1);
        assert_eq!(
            code[line_start..start].trim(),
            "",
            "`{field}` must carry no visibility, so no code outside this module can \
             scan it for a name",
        );
    }
}

/// CQGATE01: no `Expression` variant carries a block or a statement.
///
/// The statement walker binds every field of every arm, so a new block-bearing field
/// on an existing variant stops the build instead of leaving its statements never
/// lowered and never diagnosed. The expression walker covers unread fields with `..`
/// because the defect has no shape there: an expression's operands are expressions,
/// which every arm already walks. That exemption is a property of the syntax tree,
/// so it is checked rather than asserted in prose — the day an expression gains a
/// block, this fails and the expression arms must name their fields too.
#[test]
fn no_expression_variant_carries_a_block() {
    let ast = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../marrow-syntax/src/ast.rs")
            .canonicalize()
            .expect("the syntax owner's ast module"),
    )
    .expect("read ast.rs");
    assert_eq!(
        block_bearing_expression_fields(&ast),
        Vec::<&str>::new(),
        "an `Expression` variant now carries a block. The expression lowering arms \
         cover unread fields with `..`, which would leave that field's statements \
         never lowered and never diagnosed: name the fields as `lower_statement`'s \
         arms do, then narrow this gate",
    );
}

/// The block-bearing shapes named inside `pub enum Expression`, read from production
/// code so a shape spelled in a doc comment or a fixture literal is not one.
fn block_bearing_expression_fields(ast: &str) -> Vec<&'static str> {
    let code = production_code(ast);
    let start = code
        .find("pub enum Expression {")
        .expect("the expression tree is declared");
    let body = &code[start..];
    let end = body
        .find("\n}")
        .expect("the expression tree's declaration closes at column zero");
    let body = body[..end].to_string();
    ["Block", "Statement", "Stmt"]
        .into_iter()
        .filter(|shape| body.contains(shape))
        .collect()
}

/// The scan finds a planted block-bearing field and is not satisfied by the real
/// tree's absence alone — the plant probe the crate's other scanners carry, so a gate
/// that stopped looking cannot pass by looking at nothing.
#[test]
fn the_expression_block_scan_finds_a_planted_block() {
    let planted = "\
/// A doc comment naming Block, which is not a field.
pub enum Expression {
    Name { segments: Vec<Segment> },
    Probe { body: Block },
}
";
    assert_eq!(block_bearing_expression_fields(planted), vec!["Block"]);

    let clean = "\
pub enum Expression {
    Name { segments: Vec<Segment> },
}
";
    assert_eq!(
        block_bearing_expression_fields(clean),
        Vec::<&str>::new(),
        "a tree with no block-bearing field is clean",
    );
}

/// Every surviving narrow `as u16` spelling in production `marrow-compile` source is
/// pinned to an exact per-file census — the enforcement artifact the type-parameter
/// wrap escaped through. A new narrowing cast fails this gate and must be
/// adjudicated: either the value is bounded by an exact located diagnostic (and the
/// census grows deliberately, with that proof), or it takes the wide-carrier
/// treatment. The surviving entries are function-family ordinals frozen for the
/// function-slot refounding plus counts bounded by their construct's own diagnostic.
#[test]
fn every_narrowing_cast_is_pinned_to_its_census() {
    let allowed: BTreeMap<&str, usize> = BTreeMap::from([
        ("analysis.rs", 2),
        ("compile.rs", 2),
        ("durable.rs", 2),
        ("lower/durable.rs", 2),
        ("lower/registry.rs", 1),
        ("lower/stmts.rs", 2),
        ("types/metadata.rs", 1),
        ("types/mod.rs", 4),
    ]);
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for path in src_files() {
        if is_test_only_file(&path) {
            continue;
        }
        let code = production_code(&fs::read_to_string(&path).expect("read source file"));
        let count = code.matches("as u16").count();
        if count > 0 {
            let rel = path
                .display()
                .to_string()
                .split_once("src/")
                .expect("a src path")
                .1
                .to_string();
            found.insert(rel, count);
        }
    }
    let expected: BTreeMap<String, usize> = allowed
        .into_iter()
        .map(|(file, count)| (file.to_string(), count))
        .collect();
    assert_eq!(
        found, expected,
        "the production `as u16` census moved; adjudicate the new or vanished cast",
    );
}

/// The narrowing-cast census scanner sees code and not prose or test items.
#[test]
fn the_narrowing_cast_census_sees_code_and_not_prose() {
    let planted = production_code(
        "/// prose about as u16 stays prose\nfn probe(n: usize) -> u16 {\n    n as u16\n}\n#[cfg(test)]\nmod tests {\n    fn hidden(n: usize) -> u16 {\n        n as u16\n    }\n}\n",
    );
    assert_eq!(
        planted.matches("as u16").count(),
        1,
        "the census counts the production cast once and ignores prose and test items",
    );
}
