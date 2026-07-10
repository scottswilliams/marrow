use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn docs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The substrings inside backtick pairs, left to right. Markdown fences its inline code and
/// table cells in backticks, so this reads a heading's `family.*` tokens or a kind cell without a
/// regex dependency.
fn backtick_tokens(text: &str) -> Vec<String> {
    text.split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, part)| part.to_string())
        .collect()
}

/// The first dotted segments a family heading covers, e.g. `config` and `project` for the shared
/// `config.*`/`project.*` section.
fn family_segments(heading: &str) -> Vec<String> {
    backtick_tokens(heading)
        .into_iter()
        .filter_map(|token| token.strip_suffix(".*").map(str::to_string))
        .collect()
}

/// Every `(first segment, kind)` pair the reference states: the "How `kind` Is Assigned" summary
/// table and each `### family.*` section heading. The guard holds all of them against
/// `kind_for_code`, so the documented category can never disagree with the runtime classifier.
fn documented_kind_assignments(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_summary_table = false;

    for line in text.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            in_summary_table = section.contains("How `kind` Is Assigned");
            continue;
        }
        if let Some(heading) = line.strip_prefix("### ") {
            in_summary_table = false;
            if let Some((_, kind_cell)) = heading.split_once("kind `") {
                let kind = kind_cell.split('`').next().unwrap_or_default().to_string();
                pairs.extend(
                    family_segments(heading)
                        .into_iter()
                        .map(|segment| (segment, kind.clone())),
                );
            }
            continue;
        }
        if in_summary_table && line.trim_start().starts_with('|') {
            let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
            if let [segment_cell, kind_cell] = cells.as_slice()
                && let Some(kind) = backtick_tokens(kind_cell).into_iter().next()
            {
                pairs.extend(
                    backtick_tokens(segment_cell)
                        .into_iter()
                        .map(|segment| (segment, kind.clone())),
                );
            }
        }
    }

    pairs
}

/// The dotted error-code families the reference documents. The code-truth gate keys its
/// documented-vs-emitted comparison on these families; a family documented with a `### ` section
/// but missing here fails the gate, so a new family cannot slip in.
const KNOWN_FAMILIES: &[&str] = &[
    "parse", "fmt", "check", "schema", "catalog", "doctor", "run", "value", "write", "store", "io",
    "config", "project", "data", "evolve", "test", "backup", "restore", "surface",
];

/// Production strings that share an error code's dotted lowercase grammar but are not error codes:
/// hashing domain-separation labels and a store-field validation label. The gate subtracts them from
/// the emitted scan and holds them honest — each must still occur in source and must never become a
/// documented code.
const NON_CODE_TOKENS: &[&str] = &[
    "config.default_entry",
    "config.source_roots",
    "config.tests",
    "store.backend",
];

/// Whether `token` is a well-formed dotted error code: a known family, a dot, then a lowercase
/// segment. Both the reference tables and the production sources spell codes this way, so the gate
/// reads them identically from each side.
fn is_error_code(token: &str) -> bool {
    let Some((family, segment)) = token.split_once('.') else {
        return false;
    };
    KNOWN_FAMILIES.contains(&family)
        && !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The dotted error codes spelled inside string or byte-string literals in `text` — standalone
/// (`"run.overflow"`) or embedded in a larger literal (a JSON template with escaped quotes, a
/// prose message naming a code). Each line is walked with a small in-string state machine: a `"`
/// opens a literal, a backslash escapes the next byte (so `\"` stays inside), the next bare `"`
/// closes it, and a `'"'` char literal is skipped; every maximal code-character run inside a
/// literal that parses as an error code is collected. A run continued by an uppercase letter is an
/// identifier fragment, not a code (`run.defaultEntry`), and trailing dots are sentence
/// punctuation, so both are trimmed away. State is line-local, so an unbalanced quote in one line
/// cannot desync the rest of the file.
fn quoted_error_codes(text: &str) -> BTreeSet<String> {
    fn collect(run: &str, codes: &mut BTreeSet<String>) {
        let token = run.trim_end_matches('.');
        if is_error_code(token) {
            codes.insert(token.to_string());
        }
    }

    let mut codes = BTreeSet::new();
    for line in text.lines() {
        let bytes = line.as_bytes();
        let mut in_string = false;
        let mut run_start: Option<usize> = None;
        let mut i = 0;
        while i < bytes.len() {
            let byte = bytes[i];
            if in_string {
                let is_code_char = byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || byte == b'_'
                    || byte == b'.';
                if is_code_char {
                    run_start.get_or_insert(i);
                } else {
                    if let Some(start) = run_start.take()
                        && !byte.is_ascii_uppercase()
                    {
                        collect(&line[start..i], &mut codes);
                    }
                    if byte == b'\\' {
                        i += 1;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                }
            } else if byte == b'"' {
                in_string = true;
            } else if byte == b'\'' && bytes.get(i + 1) == Some(&b'"') {
                i += 2;
            }
            i += 1;
        }
        if in_string && let Some(start) = run_start {
            collect(&line[start..], &mut codes);
        }
    }
    codes
}

/// Drop `#[cfg(test)]` module bodies so the scan reads only production emission. Test modules mint
/// fake codes to exercise rendering; those are not part of the emitted contract. A conventional
/// top-level test module opens with `#[cfg(test)]`, then `mod NAME {` at column zero, and closes
/// with a column-zero `}`; the scan removes exactly that span.
fn strip_test_modules(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i] == "#[cfg(test)]" {
            let mut j = i + 1;
            while j < lines.len() && lines[j].starts_with("#[") {
                j += 1;
            }
            let opens_module = lines.get(j).is_some_and(|line| {
                (line.starts_with("mod ") || line.starts_with("pub mod "))
                    && line.trim_end().ends_with('{')
            });
            if opens_module {
                let mut k = j + 1;
                while k < lines.len() && lines[k] != "}" {
                    k += 1;
                }
                i = k + 1;
                continue;
            }
        }
        kept.push(lines[i]);
        i += 1;
    }
    kept.join("\n")
}

/// The `name` in each `#[cfg(test)] mod name;` external-module declaration in `text`. Such a module
/// lives in its own file beside the source, so the gate excludes that file from the production scan.
fn external_test_module_names(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut names = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i] == "#[cfg(test)]" {
            let mut j = i + 1;
            while j < lines.len() && lines[j].starts_with("#[") {
                j += 1;
            }
            if let Some(rest) = lines.get(j).and_then(|line| {
                line.strip_prefix("mod ")
                    .or_else(|| line.strip_prefix("pub mod "))
            }) && let Some(name) = rest.strip_suffix(';')
            {
                names.push(name.trim().to_string());
            }
        }
        i += 1;
    }
    names
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("source dir entry").path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Every `.rs` file under a crate's `src/`, minus the files reached only through a
/// `#[cfg(test)] mod name;` declaration. A submodule declared in `foo.rs` lives under `foo/`, except
/// a crate/dir root (`lib.rs`, `main.rs`, `mod.rs`) whose submodules are its siblings.
fn production_source_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates = repo_root().join("crates");
    for entry in std::fs::read_dir(&crates).expect("read crates dir") {
        let src = entry.expect("crates entry").path().join("src");
        if src.is_dir() {
            collect_rust_files(&src, &mut files);
        }
    }

    let mut excluded: Vec<PathBuf> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("read source");
        let dir = path.parent().expect("source has parent directory");
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let base = if matches!(stem, "lib" | "main" | "mod") {
            dir.to_path_buf()
        } else {
            dir.join(stem)
        };
        for name in external_test_module_names(&text) {
            excluded.push(base.join(format!("{name}.rs")));
            excluded.push(base.join(&name));
        }
    }

    files.retain(|path| !excluded.iter().any(|prefix| path.starts_with(prefix)));
    files.sort();
    files
}

/// Absence gate: no dotted error code is spelled inside a string literal in production source —
/// standalone (`"run.overflow"`) or embedded in a larger literal (a JSON template behind escaped
/// quotes, a prose message naming a code). Every code lives once in the `marrow-codes` registry
/// and is rendered through `Code::as_str`, so any code text inside a production literal is a code
/// escaping the registry. The scan strips `#[cfg(test)]` modules and the registry crate itself
/// (whose table and generated reference prose legitimately spell every code), then subtracts the
/// non-code look-alikes.
///
/// Blind spots, stated honestly: a code assembled at runtime from fragments that are not
/// themselves code-shaped (`format!("run.{segment}")`, `concat!` pieces) is invisible, as is any
/// spelling inside a `#[cfg(test)]` module, and the interior lines of a multi-line raw string
/// (the state machine is line-local). Intact code text cannot dodge the scan by embedding,
/// escaping, or respelling a segment — only deliberate string assembly evades it.
#[test]
fn no_inline_string_literal_codes_outside_registry() {
    // Every family the reference documents must be known to the scan, or its codes would escape the
    // `is_error_code` grammar check unseen.
    let reference =
        std::fs::read_to_string(docs_dir().join("error-codes.md")).expect("read error-codes");
    for (family, _) in documented_kind_assignments(&reference) {
        assert!(
            KNOWN_FAMILIES.contains(&family.as_str()),
            "family `{family}` is documented but missing from KNOWN_FAMILIES, so its codes would escape the gate"
        );
    }

    let non_code: BTreeSet<String> = NON_CODE_TOKENS
        .iter()
        .map(|code| code.to_string())
        .collect();

    let mut scanned: BTreeSet<String> = BTreeSet::new();
    let mut offenders: Vec<String> = Vec::new();
    for path in production_source_files() {
        if path
            .components()
            .any(|part| part.as_os_str() == "marrow-codes")
        {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source");
        let codes = quoted_error_codes(&strip_test_modules(&source));
        for code in codes {
            scanned.insert(code.clone());
            if !non_code.contains(&code) {
                let relative = path.strip_prefix(repo_root()).unwrap_or(&path);
                offenders.push(format!("{} in {}", code, relative.display()));
            }
        }
    }

    // The non-code allowlist stays honest: every entry still occurs in source and none is a real code.
    for token in &non_code {
        assert!(
            scanned.contains(token),
            "stale non-code allowlist entry `{token}`: no source string matches it anymore"
        );
        assert!(
            marrow_codes::Code::from_code(token).is_none(),
            "`{token}` is a registered error code and must not be on the non-code allowlist"
        );
    }

    offenders.sort();
    assert!(
        offenders.is_empty(),
        "inline error-code string literals escaped the registry; name the `marrow_codes::Code` \
         variant and render it with `.as_str()`:\n{}",
        offenders.join("\n")
    );
}

/// Code-truth guard for the `kind` column: every family's documented `kind`, in both the summary
/// table and the section headings, must equal what `kind_for_code` derives from its first segment.
#[test]
fn documented_error_kinds_match_kind_for_code() {
    let text =
        std::fs::read_to_string(docs_dir().join("error-codes.md")).expect("read error-codes");
    let assignments = documented_kind_assignments(&text);

    for (segment, kind) in &assignments {
        assert_eq!(
            marrow_check::kind_for_code(&format!("{segment}.example")),
            kind,
            "error-codes.md documents `{segment}.*` as kind `{kind}`, but kind_for_code disagrees"
        );
    }

    assert!(
        assignments.len() >= 30,
        "expected the kind contract to cover every family plus the summary table, found {}",
        assignments.len()
    );
}
