//! A complete operand inside an unterminated `(` or a call argument list missing
//! its `,`/`)` must report the expected closing delimiter or separator at the gap,
//! not the generic "expected a statement" anchored at the statement keyword. These
//! run the real `marrow check` binary in both `--format json` and text so the
//! diagnostic carries a valid 1-based location in each rendering.

use std::fs;

use crate::support;

/// A project whose sole source file is `body`, indented one level under
/// `pub fn main()`, so the offending expression lands on source line 4.
fn project_with_body(name: &str, body: &str) -> support::TempProject {
    support::temp_project_uncommitted(name, |root| {
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(
            root.join("marrow.json"),
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        )
        .expect("write config");
        fs::write(
            root.join("src/app.mw"),
            format!("module app\n\npub fn main()\n    {body}\n"),
        )
        .expect("write source");
    })
}

/// The sole `parse.syntax` diagnostic record from a `--format json` check, with
/// its message and 1-based span.
fn sole_parse_diagnostic(project: &support::TempProject) -> (String, i64, i64) {
    let output = support::marrow_sub(
        "check",
        &["--format", "json", project.path().to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json report");
    let parses: Vec<&serde_json::Value> = report["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter(|diagnostic| diagnostic["code"] == "parse.syntax")
        .collect();
    assert_eq!(parses.len(), 1, "exactly one parse diagnostic: {report:#?}");
    let diagnostic = parses[0];
    let line = diagnostic["source_span"]["line"]
        .as_i64()
        .expect("span line");
    let column = diagnostic["source_span"]["column"]
        .as_i64()
        .expect("span column");
    assert!(
        line >= 1 && column >= 1,
        "the gap must carry a valid 1-based location, never `0:0`: {diagnostic:#?}"
    );
    (
        diagnostic["message"].as_str().expect("message").to_string(),
        line,
        column,
    )
}

/// The located text-format line `file:line:col: code: message` for the project's
/// sole parse error.
fn sole_text_line(project: &support::TempProject) -> String {
    let output = support::marrow_sub("check", &[project.path().to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let mut rendered = String::from_utf8(output.stdout).expect("stdout utf8");
    rendered.push_str(&String::from_utf8(output.stderr).expect("stderr utf8"));
    let located: Vec<String> = rendered
        .lines()
        .filter(|line| line.contains("parse.syntax"))
        .map(str::to_string)
        .collect();
    assert_eq!(located.len(), 1, "exactly one parse line: {rendered}");
    located[0].clone()
}

#[test]
fn unclosed_paren_reports_expected_close_paren_in_json_and_text() {
    let project = project_with_body("close-paren", "return (a");

    let (message, line, _column) = sole_parse_diagnostic(&project);
    assert_eq!(line, 4, "the gap is on the offending line, not the keyword");
    assert!(
        message.contains("expected `)`"),
        "expected the close-paren message, got {message:?}"
    );

    let text = sole_text_line(&project);
    assert!(
        text.contains(":4:") && text.contains("expected `)`"),
        "located text line: {text}"
    );
}

#[test]
fn call_missing_separator_reports_expected_comma_or_close_in_json_and_text() {
    let project = project_with_body("missing-separator", "return g(a b)");

    let (message, line, _column) = sole_parse_diagnostic(&project);
    assert_eq!(line, 4, "the gap is on the offending line, not the keyword");
    assert!(
        message.contains("expected `,` or `)`"),
        "expected the separator-or-close message, got {message:?}"
    );

    let text = sole_text_line(&project);
    assert!(
        text.contains(":4:") && text.contains("expected `,` or `)`"),
        "located text line: {text}"
    );
}

#[test]
fn genuinely_missing_statement_still_reports_expected_a_statement() {
    let project = project_with_body("missing-statement", "@");

    let (message, _line, _column) = sole_parse_diagnostic(&project);
    assert!(
        message.contains("expected a statement"),
        "a line with no operand keeps the statement fallback, got {message:?}"
    );
}
