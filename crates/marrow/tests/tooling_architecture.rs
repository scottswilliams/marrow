use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn serve_protocol_does_not_import_cli_data_semantics() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let serve_dir = manifest_dir.join("src/serve");
    let mut violations = Vec::new();

    for path in rust_files_under(&serve_dir) {
        let text = fs::read_to_string(&path).expect("read serve source");
        for forbidden in ["crate::cmd_data", "super::super::cmd_data"] {
            if text.contains(forbidden) {
                violations.push(format!("{} imports {forbidden}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "serve protocol must adapt shared tooling facts, not CLI data modules:\n{}",
        violations.join("\n")
    );
}

/// The shared tooling query layer must speak canonical segment kinds. A
/// `SourceMember` identifier would mean source-text compatibility leaked out of
/// the named CLI/admin resolver (`resolve_source_text_data_query`, covered by
/// the `marrow data get` path in `data_cli.rs`) and into the shared facts. No
/// type boundary forbids merely naming a type, so this is an identifier scan.
#[test]
fn shared_tooling_query_segments_are_canonical() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tooling_dir = manifest_dir.join("../marrow-check/src/tooling");
    let mut violations = Vec::new();

    for path in rust_files_under(&tooling_dir) {
        let text = fs::read_to_string(&path).expect("read tooling source");
        if mentions_identifier(&text, "SourceMember") {
            violations.push(format!("{} mentions SourceMember", path.display()));
        }
    }

    assert!(
        violations.is_empty(),
        "shared tooling query segments must stay canonical; source-text compatibility belongs in an explicitly named CLI/admin resolver:\n{}",
        violations.join("\n")
    );
}

/// Storage locators and raw payloads must not appear in public tooling
/// signatures. The forbidden set spans several unrelated types in different
/// modules (`CatalogId`, `DataPathSegment`, `StoreLeafKind`, raw byte vectors
/// and slices, in-place string renderers), so no single trait boundary can
/// express the rule; this is a tidy identifier scan over `pub` declarations.
///
/// `DebugDataPayload::as_bytes` is the one sanctioned raw-bytes accessor: the
/// debug payload wrapper is the public type, and reading its borrowed bytes is
/// the point. Only the `&[u8]` borrow is exempt, and only on a line that begins
/// with `pub fn as_bytes(`, so an owned `Vec<u8>` payload stays forbidden even
/// from that accessor and an unrelated method whose name merely contains
/// `as_bytes` cannot smuggle raw bytes out. The positive contract that ids
/// never leak is proven by the `data_cli.rs` orphan tests, which assert no
/// `cat_` text reaches any output.
#[test]
fn public_tooling_signatures_hide_storage_locators_and_raw_payloads() {
    const FORBIDDEN: [&str; 6] = [
        "CatalogId",
        "DataPathSegment",
        "StoreLeafKind",
        "render_value_bytes",
        "push_key",
        "&mut String",
    ];
    // An owned `Vec<u8>` payload is never an acceptable public return; only the
    // one sanctioned borrowed-bytes accessor may name `&[u8]`.
    const ALWAYS_FORBIDDEN_BYTES: &str = "Vec<u8>";
    const BORROWED_BYTES: &str = "&[u8]";
    const SANCTIONED_BYTES_ACCESSOR: &str = "pub fn as_bytes(";

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tooling_dir = manifest_dir.join("../marrow-check/src/tooling");
    let mut violations = Vec::new();

    for path in rust_files_under(&tooling_dir) {
        let text = fs::read_to_string(&path).expect("read tooling source");
        for (line_number, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("pub ") {
                continue;
            }
            let mut leaks: Vec<&str> = FORBIDDEN
                .iter()
                .copied()
                .filter(|token| trimmed.contains(token))
                .collect();
            if trimmed.contains(ALWAYS_FORBIDDEN_BYTES) {
                leaks.push(ALWAYS_FORBIDDEN_BYTES);
            }
            let sanctioned_borrow = trimmed.starts_with(SANCTIONED_BYTES_ACCESSOR);
            if !sanctioned_borrow && trimmed.contains(BORROWED_BYTES) {
                leaks.push(BORROWED_BYTES);
            }
            if let Some(token) = leaks.first() {
                violations.push(format!(
                    "{}:{} exposes storage/raw type `{token}` in public tooling API: {}",
                    path.display(),
                    line_number + 1,
                    trimmed
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "public tooling facts must expose source-facing facts and debug payload wrappers, not storage locators or raw payload vectors:\n{}",
        violations.join("\n")
    );
}

#[test]
fn tooling_data_root_module_is_only_a_facade() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.join("../marrow-check/src/tooling/data/mod.rs");
    let text = fs::read_to_string(&root).expect("read data module root");
    let line_count = text.lines().count();

    assert!(
        line_count <= 250,
        "tooling data root module should be a focused facade, got {line_count} lines at {}",
        root.display()
    );
}

#[test]
fn explain_surface_does_not_claim_query_or_index_plans() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest_dir.parent().expect("repo root");
    let paths = [
        "src/cmd_explain.rs",
        "../marrow-check/src/tooling/explain.rs",
        "../../docs/cli.md",
        "../../docs/data-tools.md",
        "../../docs/tooling-surfaces.md",
        "../../docs/implementation.md",
        "../../docs/serve-protocol.md",
    ];
    let mut violations = Vec::new();

    for relative in paths {
        let path = repo.join("marrow").join(relative);
        let text = fs::read_to_string(&path).expect("read explain/doc surface");
        for (line_number, line) in text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            if lower.contains("index plan")
                || lower.contains("query plan")
                || lower.contains("optimizer")
            {
                violations.push(format!(
                    "{}:{} contains query-plan language: {}",
                    path.display(),
                    line_number + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "explain/docs must describe checked facts, not query/index plans or optimizers:\n{}",
        violations.join("\n")
    );
}

#[test]
fn explain_is_debug_admin_not_a_top_level_command() {
    // `explain` must reach the user only through the debug/admin surface. A
    // behavioral check is more durable than scanning the dispatcher's match-arm
    // text, which would break on any mechanical reshape that left the boundary
    // intact.
    let top_level = run_marrow(&["explain", "--help"]);
    assert_eq!(
        top_level.code,
        Some(2),
        "`marrow explain` must not be a top-level command: {}",
        top_level.stderr
    );
    assert!(
        top_level.stderr.contains("unknown command"),
        "`marrow explain` should be rejected as unknown: {}",
        top_level.stderr
    );

    let debug_surface = run_marrow(&["debug", "explain", "--help"]);
    assert_eq!(
        debug_surface.code,
        Some(0),
        "`marrow debug explain` must be the admin entry point: {}",
        debug_surface.stderr
    );
    assert!(
        debug_surface.stdout.contains("marrow debug explain"),
        "`marrow debug explain --help` should describe the debug surface: {}",
        debug_surface.stdout
    );

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest_dir.parent().expect("repo root");
    let docs = [
        "../../docs/cli.md",
        "../../docs/tooling-surfaces.md",
        "../../docs/implementation.md",
    ];
    let mut violations = Vec::new();
    for relative in docs {
        let path = repo.join("marrow").join(relative);
        let text = fs::read_to_string(&path).expect("read docs");
        if text.contains("marrow explain") {
            violations.push(path.display().to_string());
        }
    }

    assert!(
        violations.is_empty(),
        "canonical docs must name `marrow debug explain`, not top-level `marrow explain`:\n{}",
        violations.join("\n")
    );
}

/// Whether `text` uses `ident` as a whole identifier rather than as a substring
/// of a longer name, so a comment word or a longer type that merely contains the
/// token does not trip an identifier scan.
fn mentions_identifier(text: &str, ident: &str) -> bool {
    text.match_indices(ident).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + ident.len()..].chars().next();
        !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
    })
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

struct CliRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_marrow(args: &[&str]) -> CliRun {
    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow");
    CliRun {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).expect("stdout utf8"),
        stderr: String::from_utf8(output.stderr).expect("stderr utf8"),
    }
}

fn rust_files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            out.extend(rust_files_under(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}
