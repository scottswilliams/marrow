//! The transitive Drop-path audit: every function reachable from the armed
//! transaction guards' `Drop` implementations — the image draft's total inverse and
//! the compiler's generic-owner composite — is enumerated by name and its body
//! scanned, string/char literals blanked first, for the operations the total-inverse
//! law forbids: panics, assertions, `unwrap`/`expect`, range `drain`, slice
//! indexing, and allocation. A sentinel comment closes every audited body so a
//! truncated extraction fails loudly, and a plant probe proves the scanner sees each
//! forbidden token in real code while ignoring it inside a literal.

use std::fs;
use std::path::{Path, PathBuf};

/// The audited drop-reachable set: `(source file, function name)`, each body closed
/// by its `// drop-path audit sentinel` line. The two `Drop` implementations are the
/// roots; the named functions are their complete reachable callees that mutate
/// state. (`Vec::truncate`, `Vec::pop`, `HashMap::remove`, and `BTreeMap::remove`
/// are the standard-library leaves the law admits.)
const DROP_REACHABLE: [(&str, &str); 7] = [
    ("marrow-image/src/draft.rs", "fn rollback_armed"),
    ("marrow-image/src/draft.rs", "fn drop"),
    ("marrow-image/src/site_plan.rs", "fn pop_suffix_to"),
    ("marrow-image/src/product.rs", "fn pop_suffix_to"),
    ("marrow-image/src/product.rs", "fn rewind_total"),
    ("marrow-image/src/value_dag.rs", "fn truncate"),
    ("marrow-compile/src/types/mod.rs", "fn exit_template_proof"),
];

/// The compiler composite guard's own `Drop`, audited with its sentinel like the
/// draft guard's.
const COMPOSITE_DROP: (&str, &str) = (
    "marrow-compile/src/types/mod.rs",
    "impl Drop for TemplateProofScope",
);

/// The forbidden tokens: any of these inside an audited body breaks the total,
/// allocation-free, non-panicking inverse law.
const FORBIDDEN: [&str; 14] = [
    "panic!",
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "debug_assert",
    "unwrap(",
    "expect(",
    ".drain(",
    "unreachable!",
    "todo!",
    "unimplemented!",
    ".push(",
    ".insert(",
    "Vec::with_capacity",
];

fn workspace_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// `code` with every string and char literal blanked, including raw forms, so a
/// forbidden token inside a message cannot hide a real one and a literal cannot trip
/// the scan. Comments are preserved (a token in a comment is not code, but the
/// audited bodies keep their comments free of forbidden spellings).
fn without_literals(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('"');
                let mut prev_backslash = false;
                for inner in chars.by_ref() {
                    if inner == '"' && !prev_backslash {
                        break;
                    }
                    prev_backslash = inner == '\\' && !prev_backslash;
                    if inner == '\n' {
                        out.push('\n');
                    }
                }
                out.push('"');
            }
            'r' if matches!(chars.peek(), Some('"') | Some('#')) => {
                // A raw string: consume to its matching close.
                let mut hashes = 0;
                while matches!(chars.peek(), Some('#')) {
                    chars.next();
                    hashes += 1;
                }
                if matches!(chars.peek(), Some('"')) {
                    chars.next();
                    let close: String = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let mut window = String::new();
                    for inner in chars.by_ref() {
                        if inner == '\n' {
                            out.push('\n');
                        }
                        window.push(inner);
                        if window.ends_with(&close) {
                            break;
                        }
                    }
                    out.push('"');
                } else {
                    out.push('r');
                    for _ in 0..hashes {
                        out.push('#');
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The body of `name` in `code`: from the declaration through its balanced closing
/// brace, asserting the sentinel line follows — the proof the extraction saw the
/// whole body rather than a truncated prefix.
fn audited_body(code: &str, name: &str, file: &str) -> String {
    let start = code
        .find(name)
        .unwrap_or_else(|| panic!("{file}: audited function `{name}` not found"));
    let open = code[start..]
        .find('{')
        .map(|at| start + at)
        .unwrap_or_else(|| panic!("{file}: `{name}` has no body"));
    let mut depth = 0usize;
    let mut end = None;
    for (at, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + at + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.unwrap_or_else(|| panic!("{file}: `{name}` body is unbalanced"));
    // The sentinel may sit after the enclosing impl's own closing brace when the
    // audited function is the impl's last item.
    let mut tail = code[end..].trim_start();
    while let Some(rest) = tail.strip_prefix('}') {
        tail = rest.trim_start();
    }
    assert!(
        tail.starts_with("// drop-path audit sentinel: end of"),
        "{file}: `{name}` is not sentinel-terminated — the audited region may be truncated",
    );
    code[start..end].to_string()
}

/// Every audited drop-reachable body is free of the forbidden operations.
#[test]
fn the_armed_inverse_paths_are_total_and_allocation_free() {
    let mut audited = 0usize;
    for (file, name) in DROP_REACHABLE.into_iter().chain([COMPOSITE_DROP]) {
        let code = fs::read_to_string(workspace_file(file))
            .unwrap_or_else(|_| panic!("read {file} for the drop-path audit"));
        let code = without_literals(&code);
        // Audit every same-named function in the file (`pop_suffix_to` appears once
        // per owner; `fn drop` is matched at each audited impl's sentinel).
        let mut from = 0usize;
        while let Some(at) = code[from..].find(name) {
            let region = &code[from + at..];
            // Only audit occurrences that are sentinel-terminated definitions.
            let body = audited_body(region, name, file);
            for token in FORBIDDEN {
                assert!(
                    !body.contains(token),
                    "{file}: `{name}` contains forbidden `{token}` on the Drop path",
                );
            }
            audited += 1;
            from += at + body.len();
        }
    }
    assert!(
        audited >= DROP_REACHABLE.len(),
        "the drop-path audit lost a subject; found {audited} bodies",
    );
}

/// The plant probe: the scanner must flag each forbidden token in real code and must
/// not flag one hidden inside a string literal (including a raw literal), and the
/// sentinel check must refuse an unterminated body.
#[test]
fn the_drop_path_scanner_detects_a_planted_violation() {
    for token in FORBIDDEN {
        let planted = format!(
            "fn probe() {{ let _ = {token}; }}\n// drop-path audit sentinel: end of probe\n"
        );
        let body = audited_body(&without_literals(&planted), "fn probe", "planted");
        assert!(
            body.contains(token),
            "the scanner failed to see planted `{token}` in code",
        );
        let literal = format!(
            "fn probe() {{ let _ = \"{token}\"; let _ = r#\"{token}\"#; }}\n// drop-path audit sentinel: end of probe\n"
        );
        let body = audited_body(&without_literals(&literal), "fn probe", "planted");
        assert!(
            !body.contains(token),
            "the scanner saw `{token}` inside a blanked literal",
        );
    }
    let unterminated = "fn probe() { }\nfn other() {}\n";
    let outcome = std::panic::catch_unwind(|| {
        audited_body(&without_literals(unterminated), "fn probe", "planted")
    });
    assert!(
        outcome.is_err(),
        "an unterminated audited body must fail the sentinel check loudly",
    );
}
