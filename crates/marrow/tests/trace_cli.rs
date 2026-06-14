mod support;

use support::{marrow, temp_project, write};

fn assert_ordered(haystack: &str, needles: &[&str]) {
    let mut offset = 0;
    for needle in needles {
        let found = haystack[offset..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing `{needle}` after byte {offset}: {haystack}"));
        offset += found + needle.len();
    }
}

fn faulting_print_project(name: &str) -> support::TempProject {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"before fault\")\n    const boom = 1 / 0\n",
        );
    })
}

#[test]
fn run_trace_interleaves_steps_and_writes() {
    // An entry that writes a field then prints. With `--trace`, the trace stream
    // reports the writing statement, the node write and field write it produced, and
    // the print — a step, then two writes, then a step — in that execution order, and
    // the program's own output still lands on stdout.
    let project = temp_project("trace-run", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"done\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // The program's own output is unaffected and stays off the record stream.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "done\n", "stdout: {stdout}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert_ordered(
        &stderr,
        &[
            "app.mw:8",
            "write ^books(1)",
            "write ^books(1).title = Mort",
            "app.mw:9",
        ],
    );
}

#[test]
fn run_trace_renders_a_bool_write_as_its_typed_value() {
    // A managed write of a `bool` field traces as `true`, not the codec byte `1`.
    let project = temp_project("trace-bool", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Flag\n\
             \x20\x20\x20\x20on: bool\n\
             store ^flags(id: int): Flag\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^flags(1).on = true\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    // Render contract: the human text trace renders the scalar, never leaking the byte.
    const BOOL_WRITE_RENDER: &str = "write ^flags(1).on = true";
    const BOOL_CODEC_BYTE_LEAK: &str = "^flags(1).on = 1";
    let text_run = marrow(&["run", "--trace", &dir]);
    assert_eq!(text_run.status.code(), Some(0), "{text_run:?}");
    let stderr = String::from_utf8(text_run.stderr).expect("utf8");
    assert!(
        stderr.contains(BOOL_WRITE_RENDER),
        "a bool must trace as `true`, not `1`: {stderr}"
    );
    assert!(
        !stderr.contains(BOOL_CODEC_BYTE_LEAK),
        "the bool must not leak the codec byte `1`: {stderr}"
    );
}

#[test]
fn run_trace_renders_enum_and_identity_writes_as_names() {
    let project = temp_project("trace-enum-identity-writes", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             enum Status\n\
             \x20\x20\x20\x20active\n\
             \x20\x20\x20\x20archived\n\
             resource Order\n\
             \x20\x20\x20\x20state: Status\n\
             store ^orders(id: int): Order\n\
             resource Author\n\
             \x20\x20\x20\x20name: string\n\
             store ^authors(id: int): Author\n\
             resource Book\n\
             \x20\x20\x20\x20author: Id(^authors)\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^orders(1).state = Status::archived\n\
             \x20\x20\x20\x20^books(1).author = Id(^authors, 7)\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let text_run = marrow(&["run", "--trace", &dir]);
    assert_eq!(text_run.status.code(), Some(0), "{text_run:?}");
    let stderr = String::from_utf8(text_run.stderr).expect("utf8");
    assert!(
        stderr.contains("write ^orders(1).state = app::Status::archived"),
        "enum write must trace as a member path: {stderr}"
    );
    assert!(
        stderr.contains("write ^books(1).author = ^authors(7)"),
        "identity write must trace as a rooted identity: {stderr}"
    );
    assert!(
        !stderr.contains("cat_"),
        "catalog ids must not leak into trace text: {stderr}"
    );
    assert!(
        !stderr.contains("^books(1).author = 0x"),
        "identity bytes must not leak into trace text: {stderr}"
    );
}

#[test]
fn run_trace_renders_enum_index_keys_as_member_names() {
    let project = temp_project("trace-enum-index-key", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             enum Status\n\
             \x20\x20\x20\x20active\n\
             \x20\x20\x20\x20archived\n\
             resource Order\n\
             \x20\x20\x20\x20state: Status\n\
             store ^orders(id: int): Order\n\
             \x20\x20\x20\x20index byState(state, id)\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^orders(1).state = Status::archived\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let output = marrow(&["run", "--trace", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("index:^orders.byState(app::Status::archived, 1)"),
        "enum index key must trace as a member path: {stderr}"
    );
    assert!(
        !stderr.contains("cat_"),
        "catalog ids must not leak into enum index trace text: {stderr}"
    );
}

#[test]
fn run_trace_renders_enum_and_identity_locals_as_names() {
    let project = temp_project("trace-enum-identity-locals", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             enum Status\n\
             \x20\x20\x20\x20active\n\
             \x20\x20\x20\x20archived\n\
             resource Author\n\
             \x20\x20\x20\x20name: string\n\
             store ^authors(id: int): Author\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20const state = Status::archived\n\
             \x20\x20\x20\x20const author: Id(^authors) = Id(^authors, 7)\n\
             \x20\x20\x20\x20print(\"done\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let output = marrow(&["run", "--trace", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("state=app::Status::archived"),
        "enum local must trace as a member path: {stderr}"
    );
    assert!(
        stderr.contains("author=^authors(7)"),
        "identity local must trace as a rooted identity: {stderr}"
    );
    assert!(
        !stderr.contains("state=enum("),
        "enum local must not trace as numeric enum ids: {stderr}"
    );
    assert!(
        !stderr.contains("author=identity(7)"),
        "identity local must include its root: {stderr}"
    );
}

#[test]
fn run_trace_renders_an_int_write_as_canonical_digits() {
    // The text trace renders an `int` write as canonical digits.
    let project = temp_project("trace-int", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Counter\n\
             \x20\x20\x20\x20total: int\n\
             store ^counters(id: int): Counter\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^counters(1).total = 42\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("write ^counters(1).total = 42"), "{stderr}");
}

#[test]
fn run_trace_reports_non_root_deletes() {
    let project = temp_project("trace-delete", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20details\n\
             \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
             store ^books(id: int): Book\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).details.note = \"gone\"\n\n\
             pub fn dropDetails()\n\
             \x20\x20\x20\x20delete ^books(1).details\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let output = marrow(&["run", "--trace", "--entry", "app::dropDetails", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("delete ^books(1).details"),
        "trace must report the group delete: {stderr}"
    );
}

#[test]
fn an_untraced_run_emits_no_trace_and_matches_plain_run() {
    // Without `--trace` a run produces no trace and its stdout is exactly the
    // program's output — byte-identical to a plain run.
    let project = temp_project("trace-none", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hello\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let plain = marrow(&["run", &dir]);
    let traced_off = marrow(&["run", &dir]);

    assert_eq!(plain.status.code(), Some(0), "{plain:?}");
    assert_eq!(plain.stdout, traced_off.stdout);
    let stdout = String::from_utf8(plain.stdout).expect("utf8");
    assert_eq!(stdout, "hello\n");
    // Without --trace the trace stream is silent: a plain run emits nothing on stderr,
    // so no trace records leak into a consumer reading it.
    assert!(
        plain.stderr.is_empty(),
        "plain run emitted a trace: {:?}",
        String::from_utf8_lossy(&plain.stderr)
    );
}

#[test]
fn run_trace_rejects_json_format() {
    let project = temp_project("trace-run-json", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--trace") && stderr.contains("text"),
        "{stderr}"
    );
}

#[test]
fn run_trace_keeps_program_output_off_the_trace_stream() {
    // A traced run that also prints must keep the two streams apart: stdout is the
    // program's own `print` output, and the text trace lands on stderr.
    let project = temp_project("trace-text-streams", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n\
             \x20\x20\x20\x20title: string\n\
             store ^books(id: int): Book\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"done\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", &dir]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    // The program output is exactly its `print`; no JSON record reached stdout.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "done\n", "program output must own stdout: {stdout}");

    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("write ^books(1).title = Mort"),
        "the write record must be on stderr: {stderr}"
    );
}

#[test]
fn run_trace_flushes_text_records_when_the_run_faults() {
    let project = faulting_print_project("trace-text-fault");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "before fault\n");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("app.mw:4"),
        "faulting trace must include the print step: {stderr}"
    );
    assert!(
        stderr.contains("app.mw:5"),
        "faulting trace must include the faulting step: {stderr}"
    );
    assert!(stderr.contains("run.divide_by_zero"), "{stderr}");
}

#[test]
fn run_trace_rejects_json_format_before_the_run_faults() {
    let project = faulting_print_project("trace-json-fault");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--trace", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--trace") && stderr.contains("text"),
        "{stderr}"
    );
}

#[test]
fn test_trace_labels_each_test() {
    // Two tests, each traced; the trace stream attributes events to the right test
    // by name.
    let project = temp_project("trace-test", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
        );
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/suite.mw",
            "pub fn first()\n\
             \x20\x20\x20\x20std::assert::isTrue(true)\n\n\
             pub fn second()\n\
             \x20\x20\x20\x20std::assert::isTrue(true)\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["test", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    for label in ["tests::suite::first", "tests::suite::second"] {
        assert!(stderr.contains(&format!("{label}: ")), "{stderr}");
    }
}

#[test]
fn run_trace_appears_in_help() {
    // `run --help` is human-rendered text; the golden here is the one fragment that
    // proves the trace flag is documented, not the whole help body.
    const TRACE_FLAG_IN_HELP: &str = "--trace";
    let output = marrow(&["run", "--help"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains(TRACE_FLAG_IN_HELP), "{stdout}");
}
