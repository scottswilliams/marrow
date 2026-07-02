//! Drift gate: `docs/error-codes.md` is generated from the registry, so the
//! committed page must equal what [`marrow_codes::generate`] renders. Update the
//! page by rerunning generation (set `MARROW_UPDATE_ERROR_CODES=1`) and reviewing
//! the diff as a documented-meaning contract change.

fn error_codes_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("error-codes.md")
}

#[test]
fn error_codes_doc_is_generated_from_registry() {
    let generated = marrow_codes::generate();
    let path = error_codes_path();

    if std::env::var_os("MARROW_UPDATE_ERROR_CODES").is_some() {
        std::fs::write(&path, &generated).expect("write error-codes.md");
        return;
    }

    let committed = std::fs::read_to_string(&path).expect("read error-codes.md");
    assert!(
        generated == committed,
        "docs/error-codes.md drifted from the registry. Rerun generation with \
         MARROW_UPDATE_ERROR_CODES=1 and review the diff as a contract change."
    );
}
