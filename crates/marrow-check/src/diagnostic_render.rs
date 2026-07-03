//! The single owner of diagnostic prose for codes built through
//! [`CheckDiagnostic::new`](crate::CheckDiagnostic::new). A diagnostic's human
//! message is a pure function of its registry [`Code`] and typed
//! [`DiagnosticPayload`]: the construction site supplies typed facts, and the
//! message is rendered here, once, so prose is never built beside the facts.

use marrow_codes::Code;

use crate::diagnostics::{DefaultEntryProblem, DiagnosticPayload};
use crate::typerules::mismatch_display;

/// The codes whose prose is owned by [`render_message`]. Their construction sites
/// pass a typed payload to [`CheckDiagnostic::new`](crate::CheckDiagnostic::new) and
/// build no message, so a message-bearing `CheckDiagnostic::error`/`warning` call
/// must never name one. The `no_prose_at_migrated_construction` scan enforces that;
/// extend this list as each diagnostic family migrates.
pub(crate) const MIGRATED_CODES: &[Code] = &[
    Code::CheckReturnType,
    Code::CheckAssignmentType,
    Code::CheckDefaultEntry,
    Code::CheckMultipleScripts,
];

/// Render the human message for a migrated `(code, payload)` pair. Total over
/// [`MIGRATED_CODES`] with their emitted payloads, which is every pair
/// [`CheckDiagnostic::new`](crate::CheckDiagnostic::new) can reach.
pub(crate) fn render_message(code: Code, payload: &DiagnosticPayload) -> String {
    debug_assert!(
        MIGRATED_CODES.contains(&code),
        "render_message reached for {code:?}, which CheckDiagnostic::new does not own yet",
    );
    match (code, payload) {
        (Code::CheckReturnType, DiagnosticPayload::TypeMismatch { expected, found }) => {
            let (expected, found) = mismatch_display(expected, found);
            format!("function returns `{expected}`, but this value is `{found}`")
        }
        (Code::CheckAssignmentType, DiagnosticPayload::TypeMismatch { expected, found }) => {
            let (expected, found) = mismatch_display(expected, found);
            format!("expected `{expected}`, but the value is `{found}`")
        }
        (Code::CheckDefaultEntry, DiagnosticPayload::DefaultEntry { entry, problem }) => {
            format!(
                "`run.defaultEntry` `{entry}` {}",
                default_entry_reason(*problem)
            )
        }
        (Code::CheckMultipleScripts, DiagnosticPayload::None) => "a project may have at most \
             one file without a `module` declaration (its single-file script); declare a \
             `module` for this file"
            .to_string(),
        (code, payload) => {
            unreachable!("no message template for {code:?} with payload {payload:?}")
        }
    }
}

fn default_entry_reason(problem: DefaultEntryProblem) -> &'static str {
    match problem {
        DefaultEntryProblem::Missing => "names no public entry",
        DefaultEntryProblem::Private => "names a private function; mark it `pub`",
        DefaultEntryProblem::Ambiguous => "is ambiguous; qualify it as `module::function`",
        DefaultEntryProblem::HasParameters => {
            "declares parameters, but a default entry runs with no arguments"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MIGRATED_CODES, render_message};
    use crate::diagnostics::{DefaultEntryProblem, DiagnosticPayload};
    use crate::program::MarrowType;
    use marrow_codes::Code;
    use marrow_store::value::ScalarType;
    use std::path::{Path, PathBuf};

    fn primitive(scalar: ScalarType) -> MarrowType {
        MarrowType::Primitive(scalar)
    }

    /// Every migrated `(code, payload)` renders exactly the message its old
    /// construction site built. This pins the prose the renderer now owns, so a
    /// drift from the original wording fails here.
    #[test]
    fn renders_migrated_prose_byte_identical() {
        assert_eq!(
            render_message(
                Code::CheckReturnType,
                &DiagnosticPayload::TypeMismatch {
                    expected: primitive(ScalarType::Int),
                    found: primitive(ScalarType::Str),
                },
            ),
            "function returns `int`, but this value is `string`",
        );
        assert_eq!(
            render_message(
                Code::CheckAssignmentType,
                &DiagnosticPayload::TypeMismatch {
                    expected: primitive(ScalarType::Bool),
                    found: primitive(ScalarType::Int),
                },
            ),
            "expected `bool`, but the value is `int`",
        );
        for (problem, reason) in [
            (DefaultEntryProblem::Missing, "names no public entry"),
            (
                DefaultEntryProblem::Private,
                "names a private function; mark it `pub`",
            ),
            (
                DefaultEntryProblem::Ambiguous,
                "is ambiguous; qualify it as `module::function`",
            ),
            (
                DefaultEntryProblem::HasParameters,
                "declares parameters, but a default entry runs with no arguments",
            ),
        ] {
            assert_eq!(
                render_message(
                    Code::CheckDefaultEntry,
                    &DiagnosticPayload::DefaultEntry {
                        entry: "main".to_string(),
                        problem,
                    },
                ),
                format!("`run.defaultEntry` `main` {reason}"),
            );
        }
        assert_eq!(
            render_message(Code::CheckMultipleScripts, &DiagnosticPayload::None),
            "a project may have at most one file without a `module` declaration \
             (its single-file script); declare a `module` for this file",
        );
    }

    /// The identifiers a migrated code would appear as in a first argument to a
    /// message-bearing `CheckDiagnostic::error`/`warning` call: its `Code` variant
    /// and its `CHECK_*` wire-string constant. Mirrors [`MIGRATED_CODES`]; kept in
    /// step by the length assertion in `no_prose_at_migrated_construction`.
    const MIGRATED_CONSTRUCTION_TOKENS: &[&str] = &[
        "Code::CheckReturnType",
        "CHECK_RETURN_TYPE",
        "Code::CheckAssignmentType",
        "CHECK_ASSIGNMENT_TYPE",
        "Code::CheckDefaultEntry",
        "CHECK_DEFAULT_ENTRY",
        "Code::CheckMultipleScripts",
        "CHECK_MULTIPLE_SCRIPTS",
    ];

    fn src_root() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
    }

    fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("src entry").path();
            if path.is_dir() {
                rust_sources(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }

    /// The first argument of a call, given the text just after its opening `(`:
    /// everything up to the first top-level comma or the closing `)`.
    fn first_argument(after_open_paren: &str) -> &str {
        let mut depth = 0usize;
        for (index, byte) in after_open_paren.bytes().enumerate() {
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' if depth == 0 => return &after_open_paren[..index],
                b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => return &after_open_paren[..index],
                _ => {}
            }
        }
        after_open_paren
    }

    /// The message-bearing constructors take a `message` argument, so their code is
    /// the first argument. A migrated code must never appear there: its prose lives
    /// only in `render_message`, reached through the message-less
    /// `CheckDiagnostic::new`.
    ///
    /// Blind spots, as with the L3/L4 tidy scans: this matches only the literal
    /// `CheckDiagnostic::error(`/`warning(` spellings and the hand-maintained tokens
    /// above, so an aliased or renamed constructor, or a code assembled at runtime,
    /// would slip past. Reviewers block those the same way.
    #[test]
    fn no_prose_at_migrated_construction() {
        assert_eq!(
            MIGRATED_CONSTRUCTION_TOKENS.len(),
            MIGRATED_CODES.len() * 2,
            "MIGRATED_CONSTRUCTION_TOKENS must list the Code variant and CHECK_* constant \
             for every code in MIGRATED_CODES",
        );
        let mut files = Vec::new();
        rust_sources(&src_root(), &mut files);
        let mut offenders = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).expect("read rust source");
            for constructor in ["CheckDiagnostic::error(", "CheckDiagnostic::warning("] {
                for (index, _) in text.match_indices(constructor) {
                    let after = &text[index + constructor.len()..];
                    let code_arg = first_argument(after);
                    for token in MIGRATED_CONSTRUCTION_TOKENS {
                        if code_arg.contains(token) {
                            offenders.push(format!("{}: {constructor}{token}", file.display()));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "migrated codes must be built through CheckDiagnostic::new, not a message-bearing \
             constructor:\n{}",
            offenders.join("\n"),
        );
    }
}
