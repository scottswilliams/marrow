use std::process::Output;

use serde_json::Value;

use crate::support;

const CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#;

fn recovery_source(extra: &str) -> String {
    format!(
        "\
module books

resource Book
    title: string

store ^books(id: int): Book

{extra}fn copyKeys(xs: sequence[int])
    const values = keys(xs)
    const copied = values

pub fn printIds()
    for id in ^books
        print(id)
"
    )
}

fn recovery_project(name: &str) -> support::TempProject {
    support::temp_project_uncommitted(name, |root| {
        support::write(root, "marrow.json", CONFIG);
        support::write(root, "src/books.mw", &recovery_source(""));
    })
}

fn run(project: &support::TempProject, args: &[&str]) -> Output {
    let mut check_args = args.to_vec();
    check_args.push(project.to_str().expect("utf8 project path"));
    support::marrow_sub("check", &check_args)
}

fn diagnostic_codes(report: &Value) -> Vec<&str> {
    report["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect()
}

#[test]
fn compiler_dev_text_warning_is_located_nonfatal_and_opt_in() {
    let project = recovery_project("compiler-dev-text");

    let ordinary = run(&project, &[]);
    assert_eq!(ordinary.status.code(), Some(0), "{ordinary:?}");
    assert!(ordinary.stderr.is_empty(), "{ordinary:?}");
    assert!(
        !String::from_utf8_lossy(&ordinary.stdout).contains("compiler.dev.unknown_type"),
        "{ordinary:?}"
    );

    let dev = run(&project, &["--compiler-dev"]);
    assert_eq!(dev.status.code(), Some(0), "{dev:?}");
    let stdout = String::from_utf8(dev.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(dev.stderr).expect("stderr utf8");
    assert!(stdout.contains("checked (1 warning)"), "{stdout}");
    assert!(
        stderr.contains("books.mw") && stderr.contains(": warning: compiler.dev.unknown_type:"),
        "{stderr}"
    );
}

#[test]
fn compiler_dev_json_and_jsonl_use_the_existing_success_diagnostic_envelopes() {
    let project = recovery_project("compiler-dev-structured");

    let json = run(&project, &["--compiler-dev", "--format", "json"]);
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    assert!(json.stderr.is_empty(), "{json:?}");
    let report: Value = serde_json::from_slice(&json.stdout).expect("json report");
    assert_eq!(report["status"], serde_json::json!("ok"), "{report:#?}");
    assert_eq!(
        diagnostic_codes(&report),
        ["compiler.dev.unknown_type"],
        "{report:#?}"
    );
    let diagnostic = &report["diagnostics"][0];
    assert_eq!(diagnostic["kind"], serde_json::json!("check"));
    assert_eq!(diagnostic["severity"], serde_json::json!("warning"));
    assert!(
        diagnostic["source_span"]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("src/books.mw")),
        "{diagnostic:#?}"
    );

    let jsonl = run(&project, &["--format", "jsonl", "--compiler-dev"]);
    assert_eq!(jsonl.status.code(), Some(0), "{jsonl:?}");
    assert!(jsonl.stderr.is_empty(), "{jsonl:?}");
    let records = support::jsonl(jsonl.stdout);
    assert_eq!(records.len(), 2, "{records:#?}");
    assert_eq!(
        records[0]["code"],
        serde_json::json!("compiler.dev.unknown_type")
    );
    assert_eq!(records[0]["severity"], serde_json::json!("warning"));
    assert_eq!(records[1]["kind"], serde_json::json!("summary"));
    assert_eq!(records[1]["status"], serde_json::json!("ok"));
    assert_eq!(records[1]["diagnostics"], serde_json::json!(1));
}

#[test]
fn compiler_dev_is_hidden_and_duplicate_use_is_a_usage_error() {
    let root_help = support::marrow(&["--help"]);
    assert_eq!(root_help.status.code(), Some(0), "{root_help:?}");
    assert!(
        !String::from_utf8_lossy(&root_help.stdout).contains("compiler-dev"),
        "{root_help:?}"
    );

    let help = support::marrow_sub("check", &["--help"]);
    assert_eq!(help.status.code(), Some(0), "{help:?}");
    assert!(
        !String::from_utf8_lossy(&help.stdout).contains("compiler-dev"),
        "{help:?}"
    );

    let project = recovery_project("compiler-dev-duplicate");
    let duplicate = run(&project, &["--compiler-dev", "--compiler-dev"]);
    assert_eq!(duplicate.status.code(), Some(2), "{duplicate:?}");
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("duplicate --compiler-dev"),
        "{duplicate:?}"
    );
}

#[test]
fn compiler_dev_suppresses_internal_audit_when_user_errors_exist() {
    let project = support::temp_project_uncommitted("compiler-dev-broken", |root| {
        support::write(root, "marrow.json", CONFIG);
        support::write(
            root,
            "src/a.mw",
            "module a\npub fn broken(): int\n    return missing\n",
        );
    });

    let output = run(&project, &["--compiler-dev", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).expect("json report");
    assert_eq!(report["status"], serde_json::json!("failed"));
    let codes = diagnostic_codes(&report);
    assert!(codes.contains(&"check.unresolved_name"), "{report:#?}");
    assert!(!codes.contains(&"compiler.dev.unknown_type"), "{report:#?}");
}

#[test]
fn compiler_dev_with_no_findings_is_byte_identical_to_ordinary_check() {
    let project = support::temp_project_uncommitted("compiler-dev-clean-identity", |root| {
        support::write(root, "marrow.json", CONFIG);
        support::write(
            root,
            "src/a.mw",
            "module a\npub fn passthrough(value: unknown): unknown\n    return value\n",
        );
    });

    let ordinary = run(&project, &["--format", "json"]);
    let dev = run(&project, &["--compiler-dev", "--format", "json"]);
    assert_eq!(ordinary.status.code(), Some(0), "{ordinary:?}");
    assert_eq!(dev.status.code(), Some(0), "{dev:?}");
    assert_eq!(dev.stdout, ordinary.stdout);
    assert_eq!(dev.stderr, ordinary.stderr);
}

#[test]
fn compiler_dev_warning_composes_with_existing_check_advisories() {
    let project = support::temp_project("compiler-dev-advisory", |root| {
        support::write(root, "marrow.json", CONFIG);
        support::write(root, "src/books.mw", &recovery_source(""));
    });
    support::write(
        project.path(),
        "src/books.mw",
        &recovery_source("const VERSION: int = 1\n\n"),
    );

    let output = run(&project, &["--compiler-dev", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).expect("json report");
    assert_eq!(report["status"], serde_json::json!("ok"));
    let codes = diagnostic_codes(&report);
    assert!(codes.contains(&"compiler.dev.unknown_type"), "{report:#?}");
    assert!(codes.contains(&"check.stale_lock"), "{report:#?}");
}
