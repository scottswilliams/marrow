use std::fs;
use std::path::Path;

#[allow(dead_code)]
pub(crate) fn json(stdout: Vec<u8>) -> serde_json::Value {
    serde_json::from_slice(&stdout).expect("json output")
}

#[allow(dead_code)]
pub(crate) fn jsonl(stdout: Vec<u8>) -> Vec<serde_json::Value> {
    let text = String::from_utf8(stdout).expect("jsonl utf8");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("jsonl record"))
        .collect()
}

#[allow(dead_code)]
pub(crate) fn codes(records: &[serde_json::Value]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|record| record["code"].as_str())
        .collect()
}

/// Freeze a fixture project's pending durable identity through the one production
/// catalog writer, so read-only commands (`data`, `serve`) and store-backed runs see
/// a committed catalog without re-implementing the write. A project that does not
/// check cleanly, or proposes no catalog change, is left untouched.
#[allow(dead_code)]
pub(crate) fn commit_catalog_if_clean(root: &Path) {
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
    if let Some((report, _program)) = marrow_check::commit_pending_identity(root, &config, &program)
        .expect("commit fixture catalog")
    {
        assert!(
            !report.has_errors(),
            "committed fixture catalog must check cleanly: {:#?}",
            report.diagnostics
        );
    }
}
