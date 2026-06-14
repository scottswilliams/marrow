//! Deeply nested source must fail closed with a located `check.nesting_limit`
//! diagnostic and exit 1, not abort the process with a native stack overflow
//! (exit 134). These run the real `marrow` binary, so they exercise the worker
//! stack the CLI runs the parser on together with the parser's depth guard.

use std::fs;

mod support;

use support::{is_code, temp_source};

/// A source nesting `if` blocks `depth` levels deep — the deep-statement form that
/// recurses through the block parser.
fn nested_ifs(depth: usize) -> String {
    let mut source = String::from("module app\n\npub fn main()\n");
    for level in 0..depth {
        let indent = "    ".repeat(level + 1);
        source.push_str(&format!("{indent}if {level} < {}\n", level + 1));
    }
    source.push_str(&"    ".repeat(depth + 1));
    source.push_str("return\n");
    source
}

/// A source returning `depth` nested parentheses — the deep-expression form that
/// recurses through the expression parser.
fn nested_parens(depth: usize) -> String {
    let expr = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
    format!("module app\n\npub fn ignore()\n    var x: int = {expr}\n")
}

/// The dotted codes on a `--format json` check, read from the typed `code` slot of
/// each diagnostic record rather than matched in any rendered prose.
fn check_json_codes(source: &str) -> (Option<i32>, Vec<String>) {
    let project = support::temp_project_uncommitted("nesting-check", |root| {
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(
            root.join("marrow.json"),
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        )
        .expect("write config");
        fs::write(root.join("src/app.mw"), source).expect("write source");
    });
    let output = support::marrow_sub("check", &["--format", "json", project.to_str().unwrap()]);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json report");
    let codes = report["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_string))
        .collect();
    (output.status.code(), codes)
}

/// The located code on a `fmt` stderr line: `file:line:col: error: code: message`.
fn fmt_located_code(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    text.lines().find_map(|line| {
        let segments: Vec<&str> = line.split(": ").collect();
        let severity = segments.iter().position(|segment| *segment == "error")?;
        let code = segments.get(severity + 1)?;
        is_code(code).then(|| (*code).to_string())
    })
}

#[test]
fn deeply_nested_if_check_reports_the_nesting_limit() {
    // ~1000 nested `if` blocks aborted the process (exit 134) before the guard; now
    // it is a located `check.nesting_limit` diagnostic at exit 1.
    let (code, codes) = check_json_codes(&nested_ifs(1000));
    assert_eq!(code, Some(1), "deep `if` nesting exits 1, not 134");
    assert!(
        codes.contains(&"check.nesting_limit".to_string()),
        "expected check.nesting_limit, got {codes:?}"
    );
}

#[test]
fn deeply_nested_parens_check_reports_the_nesting_limit() {
    // ~3000 nested parens aborted the process before the guard; now a located
    // `check.nesting_limit` diagnostic at exit 1.
    let (code, codes) = check_json_codes(&nested_parens(3000));
    assert_eq!(code, Some(1), "deep parens exit 1, not 134");
    assert!(
        codes.contains(&"check.nesting_limit".to_string()),
        "expected check.nesting_limit, got {codes:?}"
    );
}

#[test]
fn fmt_on_deeply_nested_source_reports_the_nesting_limit() {
    // `fmt` parses too, so the same depth that crashed `check` must yield the
    // located diagnostic here rather than abort.
    let path = temp_source("nesting-fmt", &nested_parens(3000));
    let output = support::marrow_sub("fmt", &[path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(
        output.status.code(),
        Some(1),
        "fmt exits 1, not 134: {output:?}"
    );
    assert_eq!(
        fmt_located_code(&output.stderr).as_deref(),
        Some("check.nesting_limit"),
    );
}

#[test]
fn nesting_just_under_the_limit_checks_normally() {
    // A source comfortably under the limit parses without the nesting error, so the
    // bound rejects only genuinely pathological nesting.
    let (code, codes) = check_json_codes(&nested_ifs(200));
    assert_eq!(code, Some(0), "under-limit nesting checks clean: {codes:?}");
    assert!(
        !codes.contains(&"check.nesting_limit".to_string()),
        "under-limit nesting should carry no nesting error: {codes:?}"
    );
}
