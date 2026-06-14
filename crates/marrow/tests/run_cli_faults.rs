mod support;

use support::{ParsedResult, marrow_sub, parse_result_line, temp_project, write};

/// Parse the single fault line a failed run prints on stderr into its typed slots. The
/// fault is the last non-empty stderr line; a leading `std::log` stream, if any,
/// precedes it. The line's grammar (`file:line:col: code: message` located, `code:
/// message` bare) is parsed by the shared [`parse_result_line`].
fn parse_fault(stderr: &[u8]) -> ParsedResult {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a fault line");
    parse_result_line(line)
}

/// The thrown user code an uncaught `throw Error(code: ...)` carries, read from the
/// `[code]` payload bracket the `run.uncaught_error` message renders on the last
/// non-empty stderr line. This payload shape is specific to `marrow run`, so it is read
/// here rather than in the shared fault parser.
fn thrown_code(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a fault line");
    let open = line.find('[')?;
    let close = line[open..].find(']')? + open;
    Some(line[open + 1..close].to_string())
}

#[test]
fn an_uncaught_throw_surfaces_the_thrown_code() {
    // The headline runtime failure surface: a throw that propagates out of the entry
    // exits non-zero, wraps as `run.uncaught_error`, and carries the user's thrown
    // dotted code in the message payload so an operator sees which error escaped.
    let root = temp_project("run-throw", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.uncaught_error");
    assert_eq!(thrown_code(&output.stderr).as_deref(), Some("book.absent"));
}

#[test]
fn a_dynamically_built_invalid_error_code_faults_typed_at_run() {
    let root = temp_project("run-bad-dynamic-code", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"Not \" + \"Valid!\", message: \"boom\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.type");
    assert_eq!(fault.line, Some(4));
}

#[test]
fn an_uncaught_unique_conflict_surfaces_its_write_code() {
    // A managed-write fault that escapes the entry is fatal: it exits non-zero and its
    // `write.unique_conflict` code surfaces, even though the fault is also catchable
    // from within the program.
    let root = temp_project("run-conflict", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book\n    required title: string\n    isbn: string\n\
             store ^books(id: int): Book\n\n    index byIsbn(isbn) unique\n\n\
             pub fn main()\n    ^books(1).title = \"Mort\"\n    ^books(1).isbn = \"978-0\"\n    ^books(2).title = \"Pyramids\"\n    ^books(2).isbn = \"978-0\"\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(parse_fault(&output.stderr).code, "write.unique_conflict");
}

#[test]
fn an_uncaught_fault_is_located() {
    // A divide-by-zero that escapes the entry surfaces located: it carries the
    // originating file and line as structural fields, not the bare `code: message`
    // it printed before.
    let root = temp_project("run-located", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(): int\n    var n: int = 1\n    return n % 0\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.divide_by_zero");
    // Located, not bare: the fault carries an origin file and line.
    assert!(
        fault
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("src/app.mw")),
        "{:?}",
        fault.file
    );
    assert_eq!(fault.line, Some(5));
}

#[test]
fn a_cross_module_fault_names_the_callee_file() {
    // The entry in `app` calls into `lib`, which divides by zero. The located fault
    // names `lib`'s file — the file the fault was raised in — not the entry's `app`.
    let root = temp_project("run-located-cross", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
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
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.divide_by_zero");
    let file = fault.file.expect("a located cross-module fault");
    assert!(file.ends_with("src/lib.mw"), "{file}");
    assert!(!file.contains("src/app.mw"), "{file}");
    assert_eq!(fault.line, Some(5));
}

#[test]
fn an_overflow_fault_is_located() {
    let root = temp_project("run-located-overflow", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(): int\n    var n: int = 9223372036854775807\n    return n + 1\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.overflow");
    assert!(
        fault
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("src/app.mw")),
        "{:?}",
        fault.file
    );
    assert_eq!(fault.line, Some(5));
}

#[test]
fn an_absent_element_fault_is_located() {
    // `std::env::require` is a checked expression that raises a runtime absence
    // at the source expression when the host variable is missing, unlike a bare
    // saved field read which W2.7 rejects during checking.
    let root = temp_project("run-located-absent", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             pub fn main(): string\n    return std::env::require(\"MARROW_TEST_DO_NOT_SET_ABSENT_ELEMENT_FIXTURE_4D0F\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.absent_element");
    assert!(
        fault
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("src/app.mw")),
        "{:?}",
        fault.file
    );
    assert_eq!(fault.line, Some(4));
}

#[test]
fn an_uncaught_throw_is_located() {
    let root = temp_project("run-located-throw", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.uncaught_error");
    assert!(
        fault
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("src/app.mw")),
        "{:?}",
        fault.file
    );
    assert_eq!(fault.line, Some(4));
}

#[test]
fn unbounded_recursion_surfaces_a_located_recursion_limit() {
    // A clean-checking recursion that attempts the 257th call frame fails closed
    // with a located `run.recursion_limit` fault at the recursive call site and
    // exit 1. The guard trips before Rust stack exhaustion can decide behavior.
    let root = temp_project("run-recursion", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             fn sumTo(n: int): int\n\
             \x20\x20\x20\x20if n <= 0\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return 0\n\
             \x20\x20\x20\x20return n + sumTo(n - 1)\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20print(sumTo(255))\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.recursion_limit");
    assert!(
        fault
            .file
            .as_deref()
            .is_some_and(|file| file.ends_with("src/app.mw")),
        "the recursion fault is located in the source file: {:?}",
        fault.file
    );
}

#[test]
fn recursion_within_the_limit_runs_normally() {
    // A recursion that stays inside the 256-frame limit runs to completion and
    // prints its result, so the bound rejects only runaway recursion.
    let root = temp_project("run-recursion-ok", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             fn sumTo(n: int): int\n\
             \x20\x20\x20\x20if n <= 0\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return 0\n\
             \x20\x20\x20\x20return n + sumTo(n - 1)\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20print(sumTo(254))\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout.trim(), "32385");
}

#[test]
fn a_fault_with_no_origin_keeps_the_bare_fallback() {
    // A missing entry never reaches a project file, so its fault carries no origin and
    // renders bare: the code leads the line with no location prefix and no spurious
    // `:0:0:`.
    let root = temp_project("run-located-noorigin", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = marrow_sub("run", &["--entry", "app::nope", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.unknown_function");
    // Bare: no origin file and no line, so nothing rendered a `:0:0:` location.
    assert_eq!(fault.file, None);
    assert_eq!(fault.line, None);
}
