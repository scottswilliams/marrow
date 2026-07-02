use crate::support;
use marrow_check::check_project;

use support::{config, temp_project, write};

const MIN_DOCUMENTED_MODULE_EXAMPLES: usize = 5;

struct MwBlock {
    file_name: String,
    index: usize,
    source: String,
}

fn language_docs_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language")
}

fn docs_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn mw_blocks(file_name: &str) -> Vec<MwBlock> {
    let path = language_docs_dir().join(file_name);
    let text = std::fs::read_to_string(path).expect("read language doc");
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut index = 0usize;
    let mut source = String::new();

    for line in text.lines() {
        if line.trim() == "```mw" {
            in_block = true;
            index += 1;
            source.clear();
            continue;
        }
        if line.trim() == "```" && in_block {
            blocks.push(MwBlock {
                file_name: file_name.to_string(),
                index,
                source: source.clone(),
            });
            in_block = false;
            continue;
        }
        if in_block {
            source.push_str(line);
            source.push('\n');
        }
    }

    blocks
}

fn all_mw_blocks() -> Vec<MwBlock> {
    let mut files = std::fs::read_dir(language_docs_dir())
        .expect("read language docs")
        .map(|entry| entry.expect("language doc entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();

    files
        .into_iter()
        .flat_map(|path| {
            let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
            mw_blocks(&file_name)
        })
        .collect()
}

fn source_path_for_module(source: &str) -> String {
    let module_line = source
        .lines()
        .find(|line| line.starts_with("module "))
        .expect("documented example must be a complete module");
    let module = module_line.trim_start_matches("module ").trim();
    format!("src/{}.mw", module.replace("::", "/"))
}

/// Implementation-only storage and identity vocabulary that must never surface in
/// the language reference. Each token is matched case-insensitively as a substring,
/// so spacing and hyphenation variants of the same engine concept are all caught.
const FORBIDDEN_VOCABULARY: &[&str] = &[
    "marrow.catalog.json",
    "catalog",
    "epoch",
    "structural signature",
    "shape signature",
    "source digest",
    "shape digest",
    "catalog digest",
    "engine-profile digest",
    "opaque id",
    "opaque stable id",
    "stable-id annotation",
    "never-reuse",
    "id ledger",
    "identity ledger",
    "ledger",
    "commit stamp",
    "id stamp",
    "engine-profile",
    "engine profile",
    "value-codec",
    "value codec",
    "tree-cell key",
    "fence",
];

/// Public saved-data vocabulary the storage reference must keep, so the rewrite
/// cannot pass by deleting the section instead of restating it for developers.
const REQUIRED_VOCABULARY: &[&str] = &[
    "marrow.lock",
    "stale lock",
    "pending evolution",
    "backup",
    "restore",
    "rename",
    "retire",
];

#[test]
fn language_docs_use_public_vocabulary_only() {
    let mut files = std::fs::read_dir(language_docs_dir())
        .expect("read language docs")
        .map(|entry| entry.expect("language doc entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();

    let mut violations: Vec<(String, &str, usize)> = Vec::new();
    for path in &files {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).expect("read language doc");
        for (line_index, line) in text.to_lowercase().lines().enumerate() {
            for token in FORBIDDEN_VOCABULARY {
                if line.contains(token) {
                    violations.push((file_name.clone(), token, line_index + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "language docs leak implementation-only vocabulary: {violations:#?}"
    );

    let storage_doc = std::fs::read_to_string(language_docs_dir().join("resources-and-storage.md"))
        .expect("read resources-and-storage doc")
        .to_lowercase();
    let missing: Vec<&str> = REQUIRED_VOCABULARY
        .iter()
        .copied()
        .filter(|term| !storage_doc.contains(term))
        .collect();
    assert!(
        missing.is_empty(),
        "resources-and-storage.md must state the public saved-data model, missing: {missing:#?}"
    );
}

#[test]
fn documented_module_examples_check_clean() {
    let mut checked = 0usize;

    for block in all_mw_blocks()
        .into_iter()
        .filter(|block| block.source.trim_start().starts_with("module "))
    {
        let relative_path = source_path_for_module(&block.source);
        let root = temp_project("docs-module-example", |root| {
            write(root, &relative_path, &block.source);
        });
        let (report, _program) = check_project(&root, &config()).expect("check");

        assert!(
            report.diagnostics.is_empty(),
            "{} block {} produced checker diagnostics: {:#?}",
            block.file_name,
            block.index,
            report.diagnostics
        );
        checked += 1;
    }

    assert!(
        checked >= MIN_DOCUMENTED_MODULE_EXAMPLES,
        "expected at least {MIN_DOCUMENTED_MODULE_EXAMPLES} documented module examples, found {checked}"
    );
}

/// The `exists()`-narrowing example in `types.md` must check clean through the
/// production pipeline. `exists(place)` narrows the guarded read to present, so the
/// body reads the narrowed path directly; a redundant inner `if const` over the
/// already-present value is rejected. The example is a bare statement snippet, so it
/// is wrapped in the smallest module that gives `^books(id).subtitle` a saved sparse
/// field before checking.
#[test]
fn types_doc_exists_narrowing_example_checks_clean() {
    let block = mw_blocks("types.md")
        .into_iter()
        .find(|block| block.source.contains("if exists(^books(id).subtitle)"))
        .expect("types.md documents the exists() narrowing example");

    let mut module = String::from(
        "module main\n\n\
         resource Book\n    required title: string\n    subtitle: string\n\n\
         store ^books(id: int): Book\n\n\
         fn show(id: int)\n",
    );
    for line in block.source.lines() {
        module.push_str("    ");
        module.push_str(line);
        module.push('\n');
    }

    let root = temp_project("docs-exists-narrowing", |root| {
        write(root, "src/main.mw", &module);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report.diagnostics.is_empty(),
        "types.md exists() narrowing example produced checker diagnostics: {:#?}",
        report.diagnostics
    );
}

/// The nested-loop `findWanted` example in `control-flow-and-effects.md` must
/// check clean through the production pipeline. It iterates a saved store root and
/// a keyed child layer and `return`s out of both loops, the idiom under
/// identity-streaming semantics. The block also shows a `const id = findWanted()`
/// call site, which a module const cannot hold, so only the function is wrapped in
/// the smallest module that gives it the `^books` root and `tags` keyed layer.
#[test]
fn control_flow_doc_find_wanted_example_checks_clean() {
    let block = mw_blocks("control-flow-and-effects.md")
        .into_iter()
        .find(|block| block.source.contains("fn findWanted"))
        .expect("control-flow doc documents the findWanted nested-loop example");

    let function = block
        .source
        .split("\n\nconst ")
        .next()
        .expect("findWanted function body");

    let module = format!(
        "module main\n\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\n\
         store ^books(id: int): Book\n\n\
         {function}\n"
    );

    let root = temp_project("docs-find-wanted", |root| {
        write(root, "src/main.mw", &module);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report.diagnostics.is_empty(),
        "control-flow doc findWanted example produced checker diagnostics: {:#?}",
        report.diagnostics
    );
}

/// Recursively collect the repo-relative paths of files under `dir`, skipping build
/// output, version-control state, and any hidden entry, so the scan sees the tracked
/// source tree and never a stray artifact dir.
fn tracked_files(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("read repo dir");
    for entry in entries {
        let path = entry.expect("repo dir entry").path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            tracked_files(&path, root, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("path under root")
                    .to_path_buf(),
            );
        }
    }
}

/// Whole-repo absence gate: the removed `marrow.catalog.json` artifact name must not survive
/// anywhere in the source tree except the two places that legitimately spell it — this gate's
/// own allowlist and the L8 forbidden-vocabulary token (here, in `language_reference_docs.rs`)
/// and the run-path negative assertion that proves the run projects a lock and never the
/// removed artifact (`run_cli_fence.rs`). The store is the saved-data identity authority and
/// `marrow.lock` its committed projection; reintroducing the file name fails this gate.
#[test]
fn marrow_catalog_json_appears_only_in_allowed_places() {
    const ARTIFACT: &str = "marrow.catalog.json";
    let allowed: [&std::path::Path; 2] = [
        std::path::Path::new("crates/marrow-check/tests/cases/language_reference_docs.rs"),
        std::path::Path::new("crates/marrow-run/tests/cases/run_cli_fence.rs"),
    ];

    let root = repo_root();
    let mut files = Vec::new();
    tracked_files(&root, &root, &mut files);
    files.sort();

    let mut violations: Vec<String> = Vec::new();
    for relative in &files {
        if allowed.contains(&relative.as_path()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(relative)) else {
            continue;
        };
        for (line_index, line) in text.lines().enumerate() {
            if line.contains(ARTIFACT) {
                violations.push(format!("{}:{}", relative.display(), line_index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "the removed `{ARTIFACT}` artifact name reappeared outside its allowlist: {violations:#?}"
    );
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

/// The dotted error-code families the reference documents, including the reserved `decode.*` family.
/// The code-truth gate keys its documented-vs-emitted comparison on these families; a family
/// documented with a `### ` section but missing here fails the gate, so a new family cannot slip in.
const KNOWN_FAMILIES: &[&str] = &[
    "parse", "fmt", "check", "schema", "catalog", "doctor", "run", "value", "write", "store", "io",
    "config", "project", "data", "evolve", "test", "backup", "restore", "surface", "decode",
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

/// The dotted error codes quoted as string literals in `text`. A code is always a standalone
/// `"family.segment"` literal, so the scan matches each such literal locally — a `"`, a run of code
/// characters, then a closing `"` — rather than tracking string state across the file, which an
/// unbalanced quote in a comment or a `'"'` char literal would desync.
fn quoted_error_codes(text: &str) -> std::collections::BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut codes = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len()
                && (bytes[end].is_ascii_lowercase()
                    || bytes[end].is_ascii_digit()
                    || bytes[end] == b'_'
                    || bytes[end] == b'.')
            {
                end += 1;
            }
            if end > start
                && end < bytes.len()
                && bytes[end] == b'"'
                && is_error_code(&text[start..end])
            {
                codes.insert(text[start..end].to_string());
            }
        }
        i += 1;
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

fn collect_rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
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
fn production_source_files() -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let crates = repo_root().join("crates");
    for entry in std::fs::read_dir(&crates).expect("read crates dir") {
        let src = entry.expect("crates entry").path().join("src");
        if src.is_dir() {
            collect_rust_files(&src, &mut files);
        }
    }

    let mut excluded: Vec<std::path::PathBuf> = Vec::new();
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

/// Absence gate: no dotted error code is spelled as an inline string literal in production source.
/// Every code lives once in the `marrow-codes` registry and is rendered through `Code::as_str`, so a
/// production `"family.segment"` literal is a code escaping the registry. The scan strips
/// `#[cfg(test)]` modules and the registry crate itself (whose table and generated reference prose
/// legitimately spell every code), then subtracts the non-code look-alikes.
///
/// Blind spots, stated honestly: only an exact standalone `"family.segment"` literal is detected. A
/// code assembled at runtime (`format!("run.{segment}")`) or split across tokens is invisible to the
/// scan, as is any code spelled inside a `#[cfg(test)]` module. The scan cannot be dodged by
/// respelling a plain literal — a renamed segment is still a `family.segment` literal and still
/// caught — but it is not a defense against deliberate string assembly.
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

    let non_code: std::collections::BTreeSet<String> = NON_CODE_TOKENS
        .iter()
        .map(|code| code.to_string())
        .collect();

    let mut scanned: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
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

/// Absence gate: the throwaway root fixtures a checker session once left behind must not reappear
/// anywhere in the tracked tree. They are scratch parser inputs, not source, examples, or part of
/// the conformance corpus.
#[test]
fn stray_root_scratch_fixtures_are_absent() {
    const STRAY: [&str; 3] = ["bare_param.mw", "colon_no_ret.mw", "no_annot.mw"];

    let root = repo_root();
    let mut files = Vec::new();
    tracked_files(&root, &root, &mut files);

    let found: Vec<String> = files
        .iter()
        .filter(|relative| {
            relative
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| STRAY.contains(&name))
        })
        .map(|relative| relative.display().to_string())
        .collect();

    assert!(
        found.is_empty(),
        "stray scratch fixtures must stay deleted, found: {found:#?}"
    );
}
