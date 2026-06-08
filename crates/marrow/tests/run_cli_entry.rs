mod support;

use support::{marrow_sub, temp_project, write};

/// The diagnostic code of a run fault, read from the structured position of the
/// rendered fault line rather than matched anywhere in the stderr blob. A run
/// reports an entry-resolution fault as `code: message` on its last stderr line; the
/// code is the dotted token before the first `: ` separator (codes carry no spaces,
/// so the split is unambiguous). Asserting the code in this position keeps the oracle
/// reword-proof against changes to the human message that follows it.
fn fault_code(stderr: &[u8]) -> String {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a fault line");
    line.split_once(": ")
        .map(|(code, _)| code.to_string())
        .unwrap_or_else(|| line.to_string())
}

#[test]
fn runs_the_default_entry_and_prints_its_output() {
    let root = temp_project("run-default", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hello from marrow\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "hello from marrow\n");
}

#[test]
fn entry_flag_overrides_the_default_entry() {
    let root = temp_project("run-entry", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"main\")\n\npub fn greet()\n    print(\"greet\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::greet", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "greet\n");
}

#[test]
fn bare_entry_flag_resolves_a_unique_public_function() {
    let root = temp_project("run-bare-entry", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/util.mw",
            "module util\n\npub fn helper(): int\n    print(\"helper ran\")\n    return 1\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "helper", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "helper ran\n");
}

#[test]
fn bare_entry_flag_rejects_ambiguous_public_functions() {
    let root = temp_project("run-ambiguous-entry", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"app\")\n",
        );
        write(
            root,
            "src/admin.mw",
            "module admin\n\npub fn main()\n    print(\"admin\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "main", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.ambiguous_function");
}

#[test]
fn entry_flag_rejects_private_functions() {
    let root = temp_project("run-private-entry", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\nfn main()\n    print(\"private\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::main", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.private_function");
}

#[test]
fn run_rejects_duplicate_format_flag() {
    let output = marrow_sub(
        "run",
        &["--format", "json", "--format", "text", "missing-project"],
    );

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--format"), "{stderr}");
}

#[test]
fn a_plain_run_rejects_format_because_it_shapes_no_report() {
    // A plain run's only output is the program's own stream, which `--format` cannot
    // shape, so the flag is a usage error rather than silently ignored. Only `--trace`
    // and `--dry-run` emit a report that `--format` controls.
    let output = marrow_sub("run", &["--format", "json", "missing-project"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--format"), "{stderr}");
}

#[test]
fn reports_a_missing_entry() {
    let root = temp_project("run-noentry", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.no_entry");
}

#[test]
fn maps_an_unknown_entry_to_a_runtime_code() {
    let root = temp_project("run-unknown", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::nope", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fault_code(&output.stderr), "run.unknown_function");
}

#[test]
fn runs_a_module_less_script_bare_entry() {
    // A module-less script is self-resolvable: its `pub fn main` lives in the
    // empty module, so the bare entry `main` resolves to it and runs. This is the
    // legitimate path for this construction: no `run.no_entry`.
    let root = temp_project("run-module-less-script", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "pub fn main()\n    print(\"from a script\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "from a script\n");
}
