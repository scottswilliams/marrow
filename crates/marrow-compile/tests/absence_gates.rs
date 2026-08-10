//! Narrow exact-symbol absence gates over `marrow-compile/src`: the shapes the
//! bounded-diagnostic design deletes must not reappear. Each scan matches an
//! exact type or call shape, never a spelling proxy.

use std::fs;
use std::path::{Path, PathBuf};

#[path = "common/source_projection.rs"]
mod source_projection;
use source_projection::{is_test_only_file, last_production_item, production_code};

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

fn occurrences(needle: &str) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for path in src_files() {
        let source = fs::read_to_string(&path).expect("read source file");
        for (index, line) in source.lines().enumerate() {
            if line.contains(needle) {
                found.push((path.clone(), index + 1));
            }
        }
    }
    found
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
            .filter(|(path, _)| path.file_name().is_none_or(|name| name != "analysis.rs"))
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
        declared |= source.contains("struct AnalysisFactCollector");
        for forbidden in ["AnalysisFactCollector<", "AnalysisDiagnosticCollector"] {
            assert!(
                !source.contains(forbidden),
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

/// The `debug_assert` sites this crate deliberately keeps, with the exact count each
/// file carries. Every one was audited and each is annotated at its line with why the
/// two profiles cannot disagree about it: the guarded condition is unreachable inside a
/// bound the crate already enforces, the write it precedes is idempotent, or nothing
/// downstream reads the value it compares.
///
/// The counts are exact in both directions. A new site fails this gate, and so does a
/// removed one, so the list cannot drift into naming sites that no longer exist and
/// silently licensing new ones in their place.
const PROFILE_GUARD_ALLOWLIST: &[(&str, usize)] = &[("types.rs", 10), ("analysis.rs", 2)];

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

    let mut counted: Vec<(String, usize)> = Vec::new();
    for path in src_files() {
        if is_test_only_file(&path) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read source file");
        let code = production_code(&source);
        let hits = code
            .lines()
            .filter(|line| line.contains("debug_assert"))
            .count();
        if hits > 0 {
            let name = path
                .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
                .expect("a source under this crate's src")
                .to_string_lossy()
                .into_owned();
            counted.push((name, hits));
        }
    }
    counted.sort();
    let mut expected: Vec<(String, usize)> = PROFILE_GUARD_ALLOWLIST
        .iter()
        .map(|(file, count)| ((*file).to_string(), *count))
        .collect();
    expected.sort();
    assert_eq!(
        counted, expected,
        "the audited `debug_assert` sites moved. A new one needs the audit the allowlist \
         records — that the two profiles cannot disagree about it — and a removed one \
         needs its count dropped so this list keeps saying what is actually there",
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
        production_occurrences("failed identity admission").is_empty(),
        "the scan must not read string literals: that phrase exists only inside one"
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
        .map(|(path, line)| {
            (
                path.file_name()
                    .and_then(|name| name.to_str())
                    .expect("a source file name")
                    .to_string(),
                *line,
            )
        })
        .collect();
    // The definition, the two durable classes that use it — a repeated project-wide
    // identity anchor, reported by the first store to reach it, and a durable value
    // cycle, reported by `types::reject_value_cycles` after lowering — and the
    // shared instantiation limit, which the monomorphization owner reports once for
    // the whole pass rather than at each declaration that hit it.
    let allowed = ["decl.rs", "durable.rs", "durable.rs", "types.rs"];
    let names: Vec<&str> = sites.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names, allowed,
        "a new `refuse_covered` call site must name the pass that reports its cause \
         and be added here deliberately: {sites:?}"
    );
}
