//! Source-absence gates over the kernel's own `src/`.
//!
//! Two families are gated here.
//!
//! **Raw store-shape intake.** The store's roots and its site table are published together,
//! as one `StoreProjection` whose sites are resolved against the completed roots. A signature
//! that takes them apart — a bare `Vec<StoreSchema>` beside a bare `Vec<SiteSpec>` — is the
//! shape that let a site name a position no root declared, which the session-setup resolver
//! then used as a raw slice index. The gate scans **crate-visible** signatures, not only
//! `pub` ones: two of the intake points are `pub(crate)`, and a `pub`-only scan would pass
//! vacuously over exactly those.
//!
//! **Unbounded-lifetime escapes.** `unsafe`, `ManuallyDrop`, and `mem::forget` are the three
//! ways a bounded owner stops being bounded. The kernel has none and must keep none.
//!
//! Both scans strip comments and string literals before looking, because a scanner that reads
//! its own needle inside a doc comment or a message string reports a hit that proves nothing —
//! and, worse, a scanner written to tolerate those hits is one plausible edit away from
//! tolerating the real thing. `the_scanner_sees_a_planted_needle_and_not_a_quoted_one` is the
//! permanent probe for that: it plants both and requires the gate to separate them.

use std::path::{Path, PathBuf};

/// The needles that would mean the store's shape had been split back into raw vectors. The
/// retired spellings stay listed beside the current ones: a rename back to `SiteSpec` must be
/// as visible as a fresh raw intake.
const RAW_STORE_SHAPE_INTAKE: &[&str] = &[
    "Vec<StoreSchema>",
    "Vec<SiteSpec>",
    "Vec<Site>",
    "&[StoreSchema]",
    "&[SiteSpec]",
    "&[Site]",
];

/// The escapes from bounded ownership. The scan covers the kernel's `src/` only, so this
/// file's own listing of the needles is never itself a hit.
const LIFETIME_ESCAPES: &[&str] = &["unsafe", "ManuallyDrop", "mem::forget"];

#[test]
fn no_crate_visible_kernel_signature_takes_a_raw_store_shape_vector() {
    let mut found = Vec::new();
    for file in kernel_sources() {
        let source = strip_comments_and_literals(&std::fs::read_to_string(&file).expect("read"));
        for signature in crate_visible_signatures(&source) {
            for needle in RAW_STORE_SHAPE_INTAKE {
                if signature.contains(needle) {
                    found.push(format!("{}: {needle} in `{signature}`", file.display()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "the store's roots and sites are published as one checked projection; these signatures \
         take them apart:\n{}",
        found.join("\n"),
    );
}

#[test]
fn the_kernel_holds_no_escape_from_bounded_ownership() {
    let mut found = Vec::new();
    for file in kernel_sources() {
        let source = strip_comments_and_literals(&std::fs::read_to_string(&file).expect("read"));
        for needle in LIFETIME_ESCAPES {
            if source.contains(needle) {
                found.push(format!("{}: {needle}", file.display()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "a bounded kernel owner stops being bounded through one of these:\n{}",
        found.join("\n"),
    );
}

/// The permanent probe. A text gate that never sees a hit is indistinguishable from one that
/// cannot see: this case plants a real crate-visible signature and a decoy that names the same
/// needle only inside a comment and a string, and requires the gate to catch the first and
/// ignore the second. It also plants a return type, which is a *read* of the roots, never an
/// intake — a gate that flagged it would force the projection's own accessor to disappear.
#[test]
fn the_scanner_sees_a_planted_needle_and_not_a_quoted_one() {
    let planted = r#"
        //! A doc comment naming Vec<SiteSpec> proves nothing.
        /// Nor does Vec<StoreSchema> in an item doc.
        pub fn published(engine: E, sites: Vec<SiteSpec>) -> Self {
            let _message = "Vec<StoreSchema> in a string is not a signature";
        }
        pub(crate) fn crate_visible(schemas: Vec<StoreSchema>) {}
        fn private_helper(schemas: Vec<StoreSchema>) {}
        pub fn roots(&self) -> &[StoreSchema] {
            &self.roots
        }
    "#;
    let stripped = strip_comments_and_literals(planted);
    let hits: Vec<String> = crate_visible_signatures(&stripped)
        .into_iter()
        .filter(|signature| {
            RAW_STORE_SHAPE_INTAKE
                .iter()
                .any(|needle| signature.contains(needle))
        })
        .collect();

    assert_eq!(
        hits.len(),
        2,
        "the gate must catch the pub and the pub(crate) intake and nothing else, saw: {hits:?}",
    );
    assert!(
        hits.iter().any(|hit| hit.contains("sites")),
        "the planted public intake must be caught: {hits:?}",
    );
    assert!(
        hits.iter().any(|hit| hit.contains("schemas")),
        "the planted crate-visible intake must be caught: {hits:?}",
    );
    assert!(
        !stripped.contains("in a string is not a signature"),
        "literals must be blanked before the scan, or the gate reads its own prose",
    );
}

/// Every `.rs` file under the kernel's `src/`, so a new module joins the gates by existing
/// rather than by being remembered.
fn kernel_sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    assert!(
        files.len() > 10,
        "the kernel source walk found almost nothing, so the gates would pass vacuously",
    );
    files
}

fn collect(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read kernel source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Blank every comment and string/char literal, preserving newlines so a reported line still
/// lines up with the file. Blanking rather than deleting keeps the surrounding code's shape
/// intact, so a signature split across a literal is still read as one signature.
fn strip_comments_and_literals(source: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Code,
        LineComment,
        BlockComment(usize),
        Str,
        RawStr(usize),
        Char,
    }

    let bytes: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut mode = Mode::Code;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();
        match mode {
            Mode::Code => {
                if c == '/' && next == Some('/') {
                    mode = Mode::LineComment;
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == '/' && next == Some('*') {
                    mode = Mode::BlockComment(1);
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == 'r' && matches!(next, Some('"') | Some('#')) {
                    let mut hashes = 0;
                    let mut j = i + 1;
                    while bytes.get(j) == Some(&'#') {
                        hashes += 1;
                        j += 1;
                    }
                    if bytes.get(j) == Some(&'"') {
                        mode = Mode::RawStr(hashes);
                        out.extend(std::iter::repeat_n(' ', j + 1 - i));
                        i = j + 1;
                        continue;
                    }
                }
                if c == '"' {
                    mode = Mode::Str;
                    out.push(' ');
                    i += 1;
                    continue;
                }
                // A lifetime (`'a`) opens no literal; a char literal closes within a few
                // characters, so the two are told apart by looking for the closing quote.
                if c == '\'' && is_char_literal(&bytes, i) {
                    mode = Mode::Char;
                    out.push(' ');
                    i += 1;
                    continue;
                }
                out.push(c);
                i += 1;
            }
            Mode::LineComment => {
                if c == '\n' {
                    mode = Mode::Code;
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                i += 1;
            }
            Mode::BlockComment(depth) => {
                if c == '/' && next == Some('*') {
                    mode = Mode::BlockComment(depth + 1);
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == '*' && next == Some('/') {
                    mode = if depth == 1 {
                        Mode::Code
                    } else {
                        Mode::BlockComment(depth - 1)
                    };
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                out.push(if c == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            Mode::Str | Mode::Char => {
                if c == '\\' {
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                let closing = if mode == Mode::Str { '"' } else { '\'' };
                if c == closing {
                    mode = Mode::Code;
                }
                out.push(if c == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            Mode::RawStr(hashes) => {
                if c == '"' && bytes[i + 1..].iter().take(hashes).all(|c| *c == '#') {
                    mode = Mode::Code;
                    out.extend(std::iter::repeat_n(' ', hashes + 1));
                    i += hashes + 1;
                    continue;
                }
                out.push(if c == '\n' { '\n' } else { ' ' });
                i += 1;
            }
        }
    }
    out
}

/// Whether the quote at `at` opens a char literal rather than a lifetime.
fn is_char_literal(chars: &[char], at: usize) -> bool {
    match chars.get(at + 1) {
        Some('\\') => true,
        Some(_) => chars.get(at + 2) == Some(&'\''),
        None => false,
    }
}

/// Every crate-visible (`pub` or `pub(crate)`/`pub(super)`) function's declaration through its
/// parameter list, with the return type deliberately excluded: returning the roots is a
/// borrowed read of a published projection, while taking them is the intake this gate exists
/// for. The parameter list is matched by balancing parentheses, so a nested function type in
/// a parameter does not truncate it.
fn crate_visible_signatures(source: &str) -> Vec<String> {
    let mut signatures = Vec::new();
    for (start, _) in source.match_indices("fn ") {
        if !preceded_by_crate_visibility(source, start) {
            continue;
        }
        let Some(open) = source[start..].find('(').map(|offset| start + offset) else {
            continue;
        };
        let mut depth = 0usize;
        let mut close = None;
        for (offset, c) in source[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(close) = close {
            signatures.push(
                source[start..=close]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    }
    signatures
}

/// Whether the `fn` at `start` is preceded, on its own item, by a `pub` visibility. The item's
/// modifiers sit between the visibility and the keyword, so the scan walks back over them
/// rather than requiring `pub fn` to be adjacent.
fn preceded_by_crate_visibility(source: &str, start: usize) -> bool {
    let head = &source[..start];
    let line_start = head.rfind('\n').map_or(0, |offset| offset + 1);
    let prefix = head[line_start..].trim();
    prefix == "pub" || prefix.starts_with("pub(") || prefix.starts_with("pub ")
}
