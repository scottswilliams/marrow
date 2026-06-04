use std::fs;
use std::path::Path;

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

#[test]
fn shared_tooling_query_segments_are_canonical() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tooling_dir = manifest_dir.join("../marrow-check/src/tooling");
    let mut violations = Vec::new();

    for path in rust_files_under(&tooling_dir) {
        let text = fs::read_to_string(&path).expect("read tooling source");
        if text.contains("SourceMember") {
            violations.push(format!("{} mentions SourceMember", path.display()));
        }
    }

    assert!(
        violations.is_empty(),
        "shared tooling query segments must stay canonical; source-text compatibility belongs in an explicitly named CLI/admin resolver:\n{}",
        violations.join("\n")
    );
}

#[test]
fn public_tooling_signatures_hide_storage_locators_and_raw_payloads() {
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
            if trimmed.contains("CatalogId")
                || trimmed.contains("DataPathSegment")
                || trimmed.contains("Vec<u8>")
                || trimmed.contains("StoreLeafKind")
                || trimmed.contains("&mut String")
                || trimmed.contains("render_value_bytes")
                || trimmed.contains("push_key")
                || (trimmed.contains("&[u8]") && !trimmed.starts_with("pub fn as_bytes("))
            {
                violations.push(format!(
                    "{}:{} exposes storage/raw type in public tooling API: {}",
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
    let data_rs = manifest_dir.join("../marrow-check/src/tooling/data.rs");
    let data_mod = manifest_dir.join("../marrow-check/src/tooling/data/mod.rs");
    let root = if data_rs.exists() { data_rs } else { data_mod };
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
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest_dir.parent().expect("repo root");
    let main_rs = repo.join("marrow/src/main.rs");
    let main_text = fs::read_to_string(&main_rs).expect("read main");

    assert!(
        !main_text.contains("\"explain\" =>"),
        "explain must not be a top-level CLI command"
    );
    assert!(
        main_text.contains("\"debug\" => cmd_explain::debug(rest)"),
        "debug explain must stay under the explicit debug/admin dispatcher"
    );

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
