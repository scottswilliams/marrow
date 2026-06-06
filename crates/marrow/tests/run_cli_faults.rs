mod support;

use support::{marrow_sub, temp_project, write};

#[test]
fn an_uncaught_throw_exits_one_with_the_thrown_code_on_stderr() {
    // The headline runtime failure surface: a throw that propagates out of the
    // entry surfaces as run.uncaught_error with the thrown dotted code embedded.
    let root = temp_project("run-throw", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("book.absent"), "{stderr}");
}

#[test]
fn an_uncaught_unique_conflict_exits_one_with_its_write_code_on_stderr() {
    // A managed-write fault that escapes the entry is fatal: it exits non-zero and
    // its `write.unique_conflict` dotted code reaches stderr, even though the fault
    // is also catchable from within the program.
    let root = temp_project("run-conflict", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\n\
             pub fn main()\n    ^books(1).title = \"Mort\"\n    ^books(1).isbn = \"978-0\"\n    ^books(2).title = \"Pyramids\"\n    ^books(2).isbn = \"978-0\"\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("write.unique_conflict"), "{stderr}");
}

#[test]
fn an_uncaught_fault_is_located_on_stderr() {
    // A divide-by-zero that escapes the entry prints located on stderr —
    // `file:line:col: code: message`, the same shape `check` and `test` use —
    // not the bare `code: message` it printed before.
    let root = temp_project("run-located", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(): int\n    var n: int = 1\n    return n % 0\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:5:")
            && stderr.contains("run.divide_by_zero:")
            && !stderr.starts_with("run.divide_by_zero"),
        "fault must be located at its file:line:col, got: {stderr}"
    );
}

#[test]
fn a_cross_module_fault_names_the_callee_file() {
    // The entry in `app` calls into `lib`, which divides by zero. The located
    // render must name `lib`'s file — the file the fault was raised in — not the
    // entry's `app`.
    let root = temp_project("run-located-cross", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse lib\n\npub fn main(): int\n    return lib::boom()\n",
        );
        write(
            root,
            "src/lib.mw",
            "module lib\n\npub fn boom(): int\n    var n: int = 1\n    return n % 0\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/lib.mw:5:") && !stderr.contains("src/app.mw"),
        "a cross-module fault must name the callee's file, got: {stderr}"
    );
    assert!(stderr.contains("run.divide_by_zero:"), "{stderr}");
}

#[test]
fn an_overflow_fault_is_located() {
    let root = temp_project("run-located-overflow", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(): int\n    var n: int = 9223372036854775807\n    return n + 1\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:5:") && stderr.contains("run.overflow:"),
        "overflow must be located, got: {stderr}"
    );
}

#[test]
fn an_absent_element_fault_is_located() {
    let root = temp_project("run-located-absent", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\nresource Book at ^books(id: int)\n    required title: string\n\n\
             pub fn main(): string\n    return ^books(99).title\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:7:") && stderr.contains("run.absent_element:"),
        "absent_element must be located, got: {stderr}"
    );
}

#[test]
fn an_uncaught_throw_is_located() {
    let root = temp_project("run-located-throw", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:3:") && stderr.contains("run.uncaught_error:"),
        "an uncaught throw must be located, got: {stderr}"
    );
}

#[test]
fn a_fault_with_no_origin_keeps_the_bare_fallback() {
    // A missing entry never reaches a project file, so its fault carries no
    // origin and must keep the bare `code: message` form — no spurious `:0:0:`.
    let root = temp_project("run-located-noorigin", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::nope", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.unknown_function") && !stderr.contains(":0:0:"),
        "a no-origin fault must stay bare, got: {stderr}"
    );
}
