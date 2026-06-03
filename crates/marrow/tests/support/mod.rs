use std::fs;
use std::path::Path;

pub(crate) fn accept_catalog_if_clean(root: &Path) {
    let Ok(config_text) = fs::read_to_string(root.join("marrow.json")) else {
        return;
    };
    let Ok(config) = marrow_project::parse_config(&config_text) else {
        return;
    };
    let Ok((report, program)) = marrow_check::check_project(root, &config) else {
        return;
    };
    if report.has_errors() {
        return;
    }
    let Some(proposal) = program.catalog.proposal else {
        return;
    };
    let path = root.join(&config.accepted_catalog);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create accepted catalog dir");
    }
    fs::write(&path, proposal.to_json_pretty()).expect("write accepted catalog");
    let (report, _program) =
        marrow_check::check_project(root, &config).expect("recheck accepted catalog");
    assert!(
        !report.has_errors(),
        "accepted fixture catalog must check cleanly: {:#?}",
        report.diagnostics
    );
}
