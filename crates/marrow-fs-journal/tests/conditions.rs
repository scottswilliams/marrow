//! DEP-RUSTIX-FSJOURNAL condition gates, enforced from the manifests, the
//! lockfile, the resolved dependency graph, and the crate source rather than
//! prose:
//!
//! 1. `rustix` appears in exactly one workspace manifest (this crate's), with
//!    the exact `=1.1.4` pin and default features off.
//! 2. The resolved feature set of `rustix` is exactly `{alloc, fs, std}`
//!    (`alloc` implied by `std`) — any new feature word is a new maintainer
//!    decision.
//! 3. No `rustix` type escapes this crate's public API: the token appears in
//!    exactly one private module, and that module exports nothing `pub`.
//!    Every source scan reads the code with comment and literal contents
//!    blanked, so a comment naming a forbidden token is documentation rather
//!    than a violation; the one deliberate prose assertion says so.
//! 4. Darwin sync discipline: every sync is a plain `fsync`; the
//!    full-flush and `sync_all`/`sync_data` spellings are absent because no
//!    current envelope claims power-loss durability, and the crate docs keep
//!    the macOS disclaimer.
//! 5. The Linux qualification leg pins the `linux_raw` backend.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow-fs-journal`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

/// Every `.rs` file under the crate's `src/`, at any depth. The walk is
/// recursive because all four source conditions below rest on it: a scan that
/// stopped at the top level would let a submodule directory (`src/x/mod.rs`)
/// carry a raw descriptor, a `rustix` type, `unsafe`, or a stronger sync past
/// every gate at once.
fn crate_sources() -> Vec<(PathBuf, String)> {
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut pending = vec![workspace_root().join("crates/marrow-fs-journal/src")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .expect("read crate src")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let contents = std::fs::read_to_string(&path).expect("read crate source");
                files.push((path, contents));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!files.is_empty(), "the crate source scan found no files");
    files
}

/// Every removed character becomes a space and every newline survives, so
/// newline counting still yields the original line numbers.
fn blank(ch: char) -> char {
    if ch == '\n' { '\n' } else { ' ' }
}

/// A string literal opened by a prefix. `r`, `br`, and `cr` open a raw
/// literal, which no escape ends and whose close is a quote followed by
/// exactly the hashes it opened with; `b` and `c` open an ordinary escaped
/// literal under a prefix.
struct PrefixedLiteral {
    /// The index of the opening `"`.
    quote: usize,
    /// The hash count the close must repeat.
    hashes: usize,
    raw: bool,
}

/// The prefixed string literal opening at `index`, if one does. Reading a raw
/// string as an ordinary one is a false-negative generator for every scan
/// below: `r"a\"` ends at its quote where an escaped reading runs past it, and
/// `r#"a " b"#` carries an interior quote as content, so either spelling
/// leaves an unterminated string that blanks every following line of code. A
/// byte *character* literal needs no prefix here — the character arm consumes
/// `b'…'` whole from its quote, leaving behind only the `b`, which no scan
/// matches.
fn prefixed_literal(chars: &[char], index: usize) -> Option<PrefixedLiteral> {
    let word = word_at(chars, index)?;
    let raw = match word.as_str() {
        "r" | "br" | "cr" => true,
        "b" | "c" => false,
        _ => return None,
    };
    let mut quote = index + word.chars().count();
    let mut hashes = 0usize;
    while raw && chars.get(quote) == Some(&'#') {
        hashes += 1;
        quote += 1;
    }
    (chars.get(quote) == Some(&'"')).then_some(PrefixedLiteral { quote, hashes, raw })
}

/// Blank one prefixed literal, returning the index to resume from. A raw
/// literal is consumed whole, since nothing inside it is code; a prefixed
/// escaped literal loses only its prefix, leaving its quote to the ordinary
/// string reading below.
fn blank_prefixed(
    chars: &[char],
    start: usize,
    literal: &PrefixedLiteral,
    out: &mut Vec<char>,
) -> usize {
    let mut index = start;
    let through = if literal.raw {
        literal.quote
    } else {
        literal.quote - 1
    };
    while index <= through {
        out.push(blank(chars[index]));
        index += 1;
    }
    if !literal.raw {
        return index;
    }
    while index < chars.len() {
        let closes = chars[index] == '"'
            && (1..=literal.hashes).all(|offset| chars.get(index + offset) == Some(&'#'));
        out.push(blank(chars[index]));
        index += 1;
        if closes {
            out.extend(std::iter::repeat_n(' ', literal.hashes));
            index += literal.hashes;
            break;
        }
    }
    index
}

/// The source with comment and literal *contents* blanked out, so an item scan
/// cannot be steered by prose or by a string that spells an item keyword.
fn code_only(source: &str) -> Vec<char> {
    let chars: Vec<char> = source.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        if let Some(literal) = prefixed_literal(&chars, index) {
            index = blank_prefixed(&chars, index, &literal, &mut out);
            continue;
        }
        let ch = chars[index];
        let two = |offset: usize| chars.get(index + offset).copied();
        match ch {
            '/' if two(1) == Some('/') => {
                while index < chars.len() && chars[index] != '\n' {
                    out.push(' ');
                    index += 1;
                }
            }
            '/' if two(1) == Some('*') => {
                let mut depth = 1u32;
                out.extend([' ', ' ']);
                index += 2;
                while index < chars.len() && depth > 0 {
                    if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
                        depth += 1;
                    } else if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                        depth -= 1;
                    }
                    out.push(blank(chars[index]));
                    index += 1;
                }
            }
            '"' => {
                out.push('"');
                index += 1;
                while index < chars.len() {
                    let inner = chars[index];
                    out.push(blank(inner));
                    index += 1;
                    if inner == '\\' {
                        if index < chars.len() {
                            out.push(' ');
                            index += 1;
                        }
                    } else if inner == '"' {
                        break;
                    }
                }
            }
            // A `'` opens a character literal only when an escape follows or
            // the third character closes it; otherwise it introduces a
            // lifetime or a loop label, which is code and stays. A literal
            // that is not consumed whole would leave its closing quote — or,
            // for `'"'`, a bare double quote — in the output, and the string
            // branch above would then blank every following line of code to
            // the next quote.
            '\'' if two(1) == Some('\\') || two(2) == Some('\'') => {
                out.push(' ');
                index += 1;
                while index < chars.len() {
                    let inner = chars[index];
                    out.push(blank(inner));
                    index += 1;
                    if inner == '\\' {
                        if index < chars.len() {
                            out.push(blank(chars[index]));
                            index += 1;
                        }
                    } else if inner == '\'' {
                        break;
                    }
                }
            }
            _ => {
                out.push(ch);
                index += 1;
            }
        }
    }
    out
}

/// [`code_only`] as text, for the whole-file token scans. Every scan that
/// forbids a token reads this rather than the raw file: the crate's own
/// documentation names the tokens it forbids and why, and a comment saying so
/// is not a violation.
fn code_text(source: &str) -> String {
    code_only(source).into_iter().collect()
}

fn word_at(chars: &[char], index: usize) -> Option<String> {
    let is_word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    if index > 0 && is_word(chars[index - 1]) {
        return None;
    }
    let end = chars[index..]
        .iter()
        .position(|&ch| !is_word(ch))
        .map_or(chars.len(), |offset| index + offset);
    (end > index).then(|| chars[index..end].iter().collect())
}

fn next_code_char(chars: &[char], from: usize) -> Option<char> {
    chars[from..].iter().copied().find(|ch| !ch.is_whitespace())
}

/// The word beginning at the first non-whitespace character at or after `from`.
fn next_code_word(chars: &[char], from: usize) -> Option<String> {
    let offset = chars[from..].iter().position(|ch| !ch.is_whitespace())?;
    word_at(chars, from + offset)
}

/// The index of the `;` that terminates the declaration starting at `from`, or
/// the end of the source.
fn statement_end(chars: &[char], from: usize) -> usize {
    chars[from..]
        .iter()
        .position(|&ch| ch == ';')
        .map_or(chars.len(), |offset| from + offset)
}

/// What closes a span a `pub` opens. An item's signature runs to the `{` or
/// `;` that closes it; a struct field's declaration runs to its own `,`, to
/// the struct's closing `}`, or to a tuple struct's `;`. Reading a field as an
/// item would run its span past the whole struct to the next block, so one
/// field would report every following field's type as its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpanShape {
    Item,
    Field,
}

/// The keywords a public item may open with. Anything else after `pub` is a
/// field of a struct or of an enum variant.
const ITEM_KEYWORDS: [&str; 14] = [
    "fn", "struct", "enum", "union", "trait", "type", "const", "static", "mod", "use", "unsafe",
    "async", "extern", "macro",
];

/// Every public item's whole signature span, whitespace-collapsed: from the
/// `pub` or `impl` keyword that opens the item to the punctuation that closes
/// its declaration. Scanning physical lines instead would let a wrapped
/// signature — rustfmt splits a long parameter list across lines — carry a
/// forbidden token past the gate on a line that names no public item.
/// Restricted visibilities (`pub(crate)` and friends) are not public API and
/// open no span. An `impl` block also contributes the associated types its
/// body binds: a trait impl hands those to callers without spelling `pub`.
fn public_signature_spans(source: &str) -> Vec<(usize, String)> {
    let chars = code_only(source);
    let mut spans: Vec<(usize, String)> = Vec::new();
    let mut line = 1usize;
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '\n' {
            line += 1;
            index += 1;
            continue;
        }
        let Some(word) = word_at(&chars, index) else {
            index += 1;
            continue;
        };
        let after = index + word.chars().count();
        let following = next_code_word(&chars, after);
        let shape = match word.as_str() {
            "impl" => Some(SpanShape::Item),
            "pub" if next_code_char(&chars, after) == Some('(') => None,
            "pub" => Some(match &following {
                Some(next) if ITEM_KEYWORDS.contains(&next.as_str()) => SpanShape::Item,
                _ => SpanShape::Field,
            }),
            _ => None,
        };
        let Some(shape) = shape else {
            index = after;
            continue;
        };
        let is_use = following.as_deref() == Some("use");
        let mut span = String::new();
        let mut depth = 0i32;
        let mut cursor = index;
        while cursor < chars.len() {
            let field = shape == SpanShape::Field;
            match chars[cursor] {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                '{' if is_use => depth += 1,
                '}' if is_use => depth -= 1,
                // A field's own comma closes it, so its generic arguments must
                // be counted too: `pub pair: Pair<u8, Handle>` would otherwise
                // end at the argument comma and hide everything after it. The
                // `>` of a `->` closes nothing.
                '<' if field => depth += 1,
                '>' if field && chars[cursor - 1] != '-' => depth -= 1,
                ',' | '}' if depth <= 0 && field => break,
                '{' | ';' if depth <= 0 => break,
                _ => {}
            }
            span.push(chars[cursor]);
            cursor += 1;
        }
        spans.push((line, span.split_whitespace().collect::<Vec<_>>().join(" ")));
        if word == "impl" && chars.get(cursor) == Some(&'{') {
            spans.extend(associated_type_spans(&chars, cursor, line));
        }
        index = after;
    }
    spans
}

/// The associated types bound directly in the `impl` body opening at `open`.
/// Only depth-one declarations count: a `type` alias inside a method body is
/// local and reaches no caller.
fn associated_type_spans(chars: &[char], open: usize, open_line: usize) -> Vec<(usize, String)> {
    let mut spans: Vec<(usize, String)> = Vec::new();
    let mut line = open_line;
    let mut depth = 0i32;
    let mut index = open;
    while index < chars.len() {
        match chars[index] {
            '\n' => line += 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            _ if depth == 1 && word_at(chars, index).as_deref() == Some("type") => {
                let end = statement_end(chars, index);
                let declaration: String = chars[index..end].iter().collect();
                spans.push((
                    line,
                    declaration.split_whitespace().collect::<Vec<_>>().join(" "),
                ));
            }
            _ => {}
        }
        index += 1;
    }
    spans
}

/// One name a source binds to a type spelling.
struct AliasBinding {
    /// The bound local name.
    name: String,
    /// The spelling it stands for: a type alias's right-hand side, or the path
    /// a `use` rename renames.
    spelling: String,
}

/// Every alias binding in one already-blanked source: `type NAME = SPELLING;`
/// and each `use PATH as NAME` rename, both at any visibility. A rename is the
/// idiomatic aliasing form and a private `use` opens no public span, so
/// collecting the bound name here is what stops
/// `use <descriptor path> as Handle;` from laundering a descriptor into a
/// public signature.
fn alias_bindings(chars: &[char]) -> Vec<AliasBinding> {
    let mut bindings: Vec<AliasBinding> = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        let Some(word) = word_at(chars, index) else {
            index += 1;
            continue;
        };
        let after = index + word.chars().count();
        if word != "type" && word != "use" {
            index = after;
            continue;
        }
        let end = statement_end(chars, after);
        let declaration: String = chars[after..end].iter().collect();
        if word == "type" {
            if let Some((name, spelling)) = declaration.split_once('=') {
                let name = name.trim().split(['<', ' ']).next().unwrap_or_default();
                if !name.is_empty() {
                    bindings.push(AliasBinding {
                        name: name.to_string(),
                        spelling: spelling.to_string(),
                    });
                }
            }
        } else {
            bindings.extend(rename_bindings(&declaration));
        }
        index = end;
    }
    bindings
}

/// The `as NAME` renames of one `use` declaration. Splitting on the group
/// punctuation keeps each rename judged by the path it actually renames, so a
/// harmless rename beside a descriptor import in one braced group is not
/// red-listed with it.
fn rename_bindings(declaration: &str) -> Vec<AliasBinding> {
    let mut bindings: Vec<AliasBinding> = Vec::new();
    for segment in declaration.split([',', '{', '}']) {
        let mut path = String::new();
        let mut words = segment.split_whitespace();
        while let Some(word) = words.next() {
            if word != "as" {
                path.push_str(word);
                continue;
            }
            if let Some(name) = words.next() {
                bindings.push(AliasBinding {
                    name: name.to_string(),
                    spelling: path.clone(),
                });
            }
            break;
        }
    }
    bindings
}

/// Every locally bound name that stands for one of `tokens`. A public
/// signature spelling such a name hands out exactly the named type, so the
/// name joins the red list. The bindings are lexed once per source and the
/// closure is then taken to a fixed point, so an alias of an alias cannot
/// launder the token either.
fn descriptor_aliases(sources: &[(PathBuf, String)], tokens: &[String]) -> Vec<String> {
    let bindings: Vec<AliasBinding> = sources
        .iter()
        .flat_map(|(_, contents)| alias_bindings(&code_only(contents)))
        .collect();
    let mut red: Vec<String> = tokens.to_vec();
    let mut aliases: Vec<String> = Vec::new();
    loop {
        let before = aliases.len();
        for binding in &bindings {
            if !aliases.contains(&binding.name)
                && red
                    .iter()
                    .any(|token| binding.spelling.contains(token.as_str()))
            {
                aliases.push(binding.name.clone());
                red.push(binding.name.clone());
            }
        }
        if aliases.len() == before {
            return aliases;
        }
    }
}

#[test]
fn rustix_is_pinned_in_exactly_one_workspace_manifest() {
    let root = workspace_root();
    let mut naming: Vec<PathBuf> = Vec::new();

    let mut manifests = vec![root.join("Cargo.toml")];
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("read crates dir")
        .flatten()
    {
        let manifest = entry.path().join("Cargo.toml");
        if manifest.is_file() {
            manifests.push(manifest);
        }
    }
    for manifest in manifests {
        let contents = std::fs::read_to_string(&manifest).expect("read manifest");
        if contents.contains("rustix") {
            naming.push(manifest);
        }
    }

    assert_eq!(
        naming,
        [root.join("crates/marrow-fs-journal/Cargo.toml")],
        "rustix must appear in exactly this crate's manifest"
    );

    let own = std::fs::read_to_string(root.join("crates/marrow-fs-journal/Cargo.toml"))
        .expect("read this crate's manifest");
    assert!(
        own.contains(r#"version = "=1.1.4""#),
        "the rustix edge must carry the exact =1.1.4 pin"
    );
    assert!(
        own.contains("default-features = false"),
        "the rustix edge must disable default features"
    );

    // The lockfile carries exactly one rustix package at the pinned version.
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).expect("read lockfile");
    assert_eq!(
        lock.matches("name = \"rustix\"").count(),
        1,
        "exactly one rustix major/minor line may exist in the lock"
    );
    assert!(
        lock.contains("name = \"rustix\"\nversion = \"1.1.4\""),
        "the locked rustix version must be 1.1.4"
    );
}

#[test]
fn the_resolved_rustix_feature_set_is_exactly_std_fs() {
    let root = workspace_root();
    let output = Command::new(env!("CARGO"))
        .arg("metadata")
        .args(["--format-version", "1"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .expect("run cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");
    let text = String::from_utf8(output.stdout).expect("metadata is utf-8");

    // Minimal dependency-free extraction over the resolve graph only (package
    // objects also carry `"features"` arrays inside their dependency lists,
    // so the scan starts at the resolve section).
    let resolve_start = text
        .find("\"resolve\":")
        .expect("metadata has a resolve graph");
    let resolve = &text[resolve_start..];
    let mut feature_lists: Vec<Vec<String>> = Vec::new();
    for chunk in resolve.split("\"id\":\"").skip(1) {
        let id = chunk.split('"').next().expect("id terminates");
        if !id.contains("#rustix@1.1.4") {
            continue;
        }
        let scope = chunk.split("\"id\":\"").next().expect("chunk head");
        let Some((_, rest)) = scope.split_once("\"features\":[") else {
            continue;
        };
        let body = rest.split(']').next().expect("feature array terminates");
        let mut features: Vec<String> = body
            .split(',')
            .map(|item| item.trim().trim_matches('"').to_string())
            .filter(|item| !item.is_empty())
            .collect();
        features.sort();
        feature_lists.push(features);
    }

    assert_eq!(
        feature_lists,
        [["alloc", "fs", "std"]],
        "the resolved rustix feature set must be exactly {{alloc, fs, std}} \
         with default features off; a new feature word is a new maintainer decision"
    );
}

#[test]
fn no_rustix_type_escapes_the_public_api() {
    let sources = crate_sources();

    // The token is confined to the one private adapter module.
    let naming: Vec<&Path> = sources
        .iter()
        .filter(|(_, contents)| code_text(contents).contains("rustix"))
        .map(|(path, _)| path.as_path())
        .collect();
    assert_eq!(
        naming.len(),
        1,
        "rustix must be named by exactly one source file: {naming:?}"
    );
    assert!(
        naming[0].ends_with("sys.rs"),
        "the rustix consumer must be the private sys module: {naming:?}"
    );

    // The adapter module is private and exports nothing `pub`.
    let (_, lib) = sources
        .iter()
        .find(|(path, _)| path.ends_with("lib.rs"))
        .expect("lib.rs exists");
    let lib = code_text(lib);
    assert!(
        lib.contains("mod sys;") && !lib.contains("pub mod sys"),
        "the sys module must be private"
    );
    assert!(
        !lib.contains("pub use sys"),
        "the sys module must not be re-exported"
    );
    let (_, sys) = sources
        .iter()
        .find(|(path, _)| path.ends_with("sys.rs"))
        .expect("sys.rs exists");
    let sys = code_text(sys);
    for declaration in [
        "pub fn",
        "pub struct",
        "pub enum",
        "pub type",
        "pub use",
        "pub mod",
        "pub const",
        "pub trait",
        "pub(super)",
    ] {
        assert!(
            !sys.contains(declaration),
            "sys.rs must export nothing beyond pub(crate): found `{declaration}`"
        );
    }
}

#[test]
fn every_sync_is_a_plain_fsync_and_the_darwin_disclaimer_stands() {
    // No current envelope claims power-loss durability, so the full-flush
    // fcntl and the std sync wrappers (whose Darwin implementation issues that
    // fcntl) must be absent. The patterns are concatenated so this test file
    // does not match itself if the scan ever widens.
    let forbidden = [
        ["fcntl_", "fullfsync"].concat(),
        ["F_", "FULLFSYNC"].concat(),
        ["sync", "_all"].concat(),
        ["sync", "_data"].concat(),
        ["fdata", "sync"].concat(),
    ];
    let mut violations: Vec<String> = Vec::new();
    for (path, contents) in crate_sources() {
        let code = code_text(&contents);
        for pattern in &forbidden {
            if code.contains(pattern.as_str()) {
                violations.push(format!("{}: {pattern}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "a sync stronger or weaker than plain fsync entered the crate:\n{}",
        violations.join("\n")
    );

    // The disclaimer is prose, so this one assertion reads the raw file.
    let (_, lib) = crate_sources()
        .into_iter()
        .find(|(path, _)| path.ends_with("lib.rs"))
        .expect("lib.rs exists");
    assert!(
        lib.contains("Sudden-power-loss") && lib.contains("on macOS is not established"),
        "the macOS sudden-power-loss disclaimer must remain in the crate docs"
    );
}

#[test]
fn the_crate_contains_no_unsafe_code() {
    // The workspace forbids `unsafe_code` by lint; this scan keeps the
    // absence conspicuous from the conformance suite as well.
    for (path, contents) in crate_sources() {
        let code = code_text(&contents);
        for pattern in ["unsafe ", "unsafe{"] {
            assert!(
                !code.contains(pattern),
                "{} contains unsafe code",
                path.display()
            );
        }
    }
}

/// The third leg of the escape red-list: no raw descriptor reaches the public
/// API. The public custody types wrap their descriptors in private fields
/// (`sys` handles), so descriptor tokens may appear in private plumbing but
/// never where they would hand a descriptor to callers.
///
/// The scan covers each public item's whole declaration span, each `impl`
/// header, and each associated type an `impl` body binds; it red-lists every
/// local name that stands for a forbidden type, whether bound by a `type`
/// alias or by a `use ... as` rename at any visibility. So neither rustfmt
/// wrapping, nor an alias, nor a rename launders a descriptor out. It does not
/// see through macro expansion, which is why the crate declares no macros —
/// the assertion below keeps that true. The tokens are concatenated so a
/// widened scan cannot match this file.
#[test]
fn no_raw_descriptor_escapes_a_public_signature_or_impl() {
    let sources = crate_sources();
    for (path, contents) in &sources {
        assert!(
            !code_text(contents).contains(&["macro_", "rules!"].concat()),
            "{} declares a macro: the span scan reads source text, so a public \
             item produced by macro expansion would bypass this gate",
            path.display()
        );
    }
    let violations = descriptor_violations(&sources);
    assert!(
        violations.is_empty(),
        "a raw descriptor reached a public signature or impl:\n{}",
        violations.join("\n")
    );
}

/// The forbidden descriptor spellings. The tokens are concatenated so a
/// widened scan cannot match this file.
fn descriptor_tokens() -> Vec<String> {
    vec![
        ["As", "Fd"].concat(),
        // `Raw` + `Fd` also covers `AsRawFd`, `FromRawFd`, and `IntoRawFd`.
        ["Raw", "Fd"].concat(),
        ["Owned", "Fd"].concat(),
        ["Borrowed", "Fd"].concat(),
        ["as_raw", "_fd"].concat(),
        ["from_raw", "_fd"].concat(),
        ["into_raw", "_fd"].concat(),
        // The module paths themselves: a `pub use` of any of them re-exports
        // the whole descriptor family under this crate's name. `os::unix::io`
        // is the older spelling of `os::fd`, and the Unix prelude re-exports
        // that same family; the `std` prefix is left off so a path reached
        // through a renamed root is caught as well.
        ["os::", "fd"].concat(),
        ["os::unix::", "io"].concat(),
        ["os::unix::", "prelude"].concat(),
    ]
}

/// Every descriptor escape in `sources`, located and named.
fn descriptor_violations(sources: &[(PathBuf, String)]) -> Vec<String> {
    let mut tokens = descriptor_tokens();
    tokens.extend(descriptor_aliases(sources, &tokens));
    let mut violations: Vec<String> = Vec::new();
    for (path, contents) in sources {
        for (line, span) in public_signature_spans(contents) {
            for token in &tokens {
                if span.contains(token.as_str()) {
                    violations.push(format!("{}:{line}: {token} in `{span}`", path.display()));
                }
            }
        }
    }
    violations
}

/// The descriptor gate's own coverage, pinned by planting each laundering form
/// it exists to catch. A gate that quietly stopped matching one of these forms
/// would keep passing over a crate that had already handed a descriptor out.
#[test]
fn the_descriptor_gate_catches_every_laundering_form() {
    let owned = ["Owned", "Fd"].concat();
    let module = ["std::os::", "fd"].concat();
    let older = ["std::os::unix::", "io"].concat();
    let prelude = ["std::os::unix::", "prelude"].concat();
    let planted = [
        format!("pub fn direct() -> {owned} {{ todo!() }}\n"),
        format!("pub use {module}::*;\n"),
        format!("pub use {older}::*;\n"),
        format!("pub use {prelude}::*;\n"),
        format!("use {module}::{owned} as Handle;\npub fn renamed() -> Handle {{ todo!() }}\n"),
        format!("type Held = {owned};\npub fn aliased() -> Held {{ todo!() }}\n"),
        format!(
            "use {module}::{owned} as Handle;\ntype Held = Handle;\n\
             pub fn chained() -> Held {{ todo!() }}\n"
        ),
        format!("impl Custody for Journal {{\n    type Handle = {owned};\n}}\n"),
        format!(
            "fn is_quote(ch: char) -> bool {{ ch == '\"' }}\n\
             pub fn after_a_quoting_literal() -> {owned} {{ todo!() }}\n"
        ),
        format!("pub struct Row {{\n    pub first: u8,\n    pub second: {owned},\n}}\n"),
        format!("pub struct Row {{\n    pub pair: Pair<u8, {owned}>,\n    pub tail: u8,\n}}\n"),
        format!("pub struct Row(pub Vec<{owned}>);\n"),
    ];
    for source in planted {
        let sources = vec![(PathBuf::from("planted.rs"), source.clone())];
        assert!(
            !descriptor_violations(&sources).is_empty(),
            "the descriptor gate missed a planted escape:\n{source}"
        );
    }
}

/// A public field's span covers its own declaration alone. A span that ran to
/// the next block would report every following field's type as this field's,
/// naming the wrong line and the wrong item.
#[test]
fn a_public_field_span_ends_at_its_own_declaration() {
    let owned = ["Owned", "Fd"].concat();
    let source = format!(
        "pub struct Row {{\n    pub first: u8,\n    pub pair: Pair<u8, {owned}>,\n    \
         pub last: fn(u8) -> u8,\n}}\n"
    );
    let spans = public_signature_spans(&source);
    assert_eq!(
        spans,
        [
            (1, "pub struct Row".to_string()),
            (2, "pub first: u8".to_string()),
            (3, format!("pub pair: Pair<u8, {owned}>")),
            (4, "pub last: fn(u8) -> u8".to_string()),
        ],
    );
}

/// Comment and literal blanking consumes each literal whole and preserves
/// every other character, including a lifetime's quote and a string's opening
/// quote. A character literal holding a double quote is the sharp case:
/// leaving any of it behind would blank the code from there to the next quote
/// anywhere in the file.
#[test]
fn blanking_consumes_literals_whole_and_leaves_lifetimes_alone() {
    let source = "let a = '\\''; let b: &'x u8; let c = '\"'; let d = \"s\";";
    let blanked = code_text(source);
    assert_eq!(blanked.chars().count(), source.chars().count());
    assert_eq!(
        blanked,
        "let a =     ; let b: &'x u8; let c =    ; let d = \"  ;"
    );
}

/// Raw and byte string literals are consumed whole, so a source that spells
/// one keeps every following line readable as code. Reading a raw string as an
/// ordinary one leaves an unterminated literal behind, and an unterminated
/// literal blanks the rest of the file: one raw string in `src/` would disable
/// every source condition in this suite at once. Each case plants a forbidden
/// token inside the literal — which must stay invisible — and a real violation
/// after it, which must survive verbatim.
#[test]
fn blanking_consumes_raw_and_byte_literals_whole() {
    let hidden = ["Owned", "Fd"].concat();
    let literals = [
        format!(r#"r"{hidden}\""#),
        format!(r##"r#"{hidden} " one"#"##),
        format!(r###"r##"{hidden} "# two"##"###),
        format!(r#"b"{hidden}\"""#),
        format!(r##"br#"{hidden} " three"#"##),
        format!(r##"cr#"{hidden}"#"##),
        format!(r#"c"{hidden}""#),
    ];
    let code = format!("pub fn kept() -> {hidden} {{ 0 }}");
    for literal in literals {
        let source = format!("const PROBE: &str = {literal};\n{code}\n");
        let blanked = code_text(&source);
        assert_eq!(
            blanked.chars().count(),
            source.chars().count(),
            "blanking {literal} changed the character count"
        );
        let mut lines = blanked.lines();
        let literal_line = lines.next().expect("the literal's line");
        assert!(
            !literal_line.contains(hidden.as_str()),
            "the contents of {literal} survived as code: {literal_line}"
        );
        assert_eq!(
            lines.next(),
            Some(code.as_str()),
            "the code after {literal} was blanked"
        );
    }
}

/// The Linux qualification leg: the `linux_raw` backend must actually be in
/// effect. rustix's build script selects `linux_raw` exactly when the target
/// is a supported Linux architecture and neither the `rustix_use_libc` nor the
/// `rustix_no_linux_raw` cfg is present; those cfgs arrive via `RUSTFLAGS`,
/// which cargo applies to every crate in the build — including this test — so
/// their absence here proves their absence for rustix in the same build.
#[cfg(target_os = "linux")]
#[test]
#[allow(unexpected_cfgs)]
fn the_linux_backend_is_linux_raw() {
    assert!(
        cfg!(any(target_arch = "x86_64", target_arch = "aarch64")),
        "Linux qualification covers x86_64 and aarch64 only; this architecture is unqualified"
    );
    assert!(
        !cfg!(rustix_use_libc),
        "RUSTFLAGS carries --cfg rustix_use_libc: the qualified linux_raw backend is not in effect"
    );
    assert!(
        !cfg!(rustix_no_linux_raw),
        "RUSTFLAGS carries --cfg rustix_no_linux_raw: the qualified linux_raw backend is not in effect"
    );
    // The `use-libc` feature is the remaining flip; the resolved-feature gate
    // above pins the feature set to exactly {alloc, fs, std} on every leg.
}
