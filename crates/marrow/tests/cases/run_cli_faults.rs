use crate::support;
use support::{
    ParsedResult, TempProject, last_fault, marrow_sub, parse_result_line, temp_project,
    temp_project_uncommitted, write,
};

/// Parse the fault line into its typed slots. The line's grammar (`file:line:col: code:
/// message` located, `code: message` bare) is parsed by the shared [`parse_result_line`].
fn parse_fault(stderr: &[u8]) -> ParsedResult {
    parse_result_line(&last_fault(stderr))
}

/// The thrown user code an uncaught `throw Error(code: ...)` carries, read from the
/// `[code]` payload bracket the `run.uncaught_error` message renders on the fault line.
/// This payload shape is specific to `marrow run`, so it is read here rather than in the
/// shared fault parser.
fn thrown_code(stderr: &[u8]) -> Option<String> {
    let line = last_fault(stderr);
    let open = line.find('[')?;
    let close = line[open..].find(']')? + open;
    Some(line[open + 1..close].to_string())
}

#[test]
fn an_uncaught_throw_surfaces_the_located_thrown_code() {
    // The headline runtime failure surface: a throw that propagates out of the entry
    // exits non-zero, wraps as `run.uncaught_error`, carries the user's thrown dotted
    // code in the message payload so an operator sees which error escaped, and is located
    // at the throwing source file and line.
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
fn a_data_dir_occupied_by_a_file_reports_an_accurate_directory_fault() {
    // The native `dataDir` is created on the write path; a regular file occupying
    // that path is a configuration fault, not a read failure. The fault must carry
    // a config-family code and a message that names the dataDir as a directory it
    // could not create, never `io.read` or "failed to read".
    let root = temp_project_uncommitted("run-data-dir-occupied", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
        write(root, ".data", "not a directory");
    });

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert_eq!(
        fault.code.as_str(),
        "config.data_dir",
        "a dataDir occupied by a file is a config fault: {stderr}"
    );
    assert!(
        !stderr.contains("io.read") && !stderr.contains("failed to read"),
        "a directory-creation failure must not render as a read failure: {stderr}"
    );
    assert!(
        stderr.contains(".data") && stderr.contains("create") && stderr.contains("dataDir"),
        "the fault must name the dataDir directory it could not create: {stderr}"
    );
}

#[test]
fn read_only_inspection_of_a_data_dir_occupied_by_a_file_reports_a_directory_fault() {
    // A regular file occupying the native `dataDir` is the same configuration fault on
    // a read-only inspection as it is on `run`: it must carry the `config.data_dir`
    // code and the directory-creation remedy, never a `store.io` fault with a raw OS
    // errno leaked into the message. Every read-only inspection command and `doctor`
    // share that classification.
    let root = temp_project_uncommitted("inspect-data-dir-occupied", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\nresource Book\n    required title: string\nstore ^books(id: int): Book\n",
        );
        write(root, ".data", "not a directory");
    });
    let dir = root.to_str().unwrap();

    // The read-only inspection family reports the occupied dataDir as the top-level
    // `config.data_dir` fault, exactly as `run` does.
    for args in [
        vec!["data", "stats", dir],
        vec!["data", "integrity", dir],
        vec!["data", "roots", dir],
        vec!["data", "dump", dir],
    ] {
        let (command, rest) = args.split_first().expect("command word");
        let output = marrow_sub(command, rest);
        let stderr = String::from_utf8(output.stderr.clone()).expect("stderr utf8");
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?} should fail closed: {stderr}"
        );
        let fault = parse_fault(&output.stderr);
        assert_eq!(
            fault.code.as_str(),
            "config.data_dir",
            "{args:?}: a dataDir occupied by a file is a config fault: {stderr}"
        );
        assert!(
            !stderr.contains("store.io") && !stderr.contains("os error"),
            "{args:?}: must not leak a store.io errno: {stderr}"
        );
        assert!(
            stderr.contains(".data") && stderr.contains("dataDir"),
            "{args:?}: the fault must name the dataDir: {stderr}"
        );
    }

    // `doctor` reports its own finding namespace, but the occupied dataDir is a
    // configuration fault carrying the `config.data_dir` underlying code, never a
    // `store.io` finding with a leaked OS errno.
    let doctor = marrow_sub("doctor", &[dir, "--format", "json"]);
    let report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).expect("doctor json report");
    let findings = report["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|finding| finding["data"]["underlying_code"] == "config.data_dir"),
        "doctor must report the occupied dataDir as config.data_dir: {report}"
    );
    let doctor_text = serde_json::to_string(&report).expect("serialize report");
    assert!(
        !doctor_text.contains("store.io") && !doctor_text.contains("os error"),
        "doctor must not leak a store.io errno: {report}"
    );
}

#[test]
#[cfg(unix)]
fn a_data_dir_create_denied_by_permissions_reports_a_directory_fault_not_a_read() {
    // Creating the native `dataDir` is a write-path operation. When the parent
    // directory denies write access, the `create_dir_all` fails with
    // `PermissionDenied`, a different errno from an occupied-by-file failure but
    // the same directory-creation fault: it must carry the config-family code and
    // never render as `io.read` or "failed to read".
    use std::os::unix::fs::PermissionsExt;

    let root = temp_project_uncommitted("run-data-dir-denied", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "ro/data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
        let locked = root.join("ro");
        std::fs::create_dir(&locked).expect("create read-only parent");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500))
            .expect("lock parent directory");
    });

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    // Restore write access so the self-cleaning temp project can be removed.
    std::fs::set_permissions(root.join("ro"), std::fs::Permissions::from_mode(0o700)).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert_eq!(
        fault.code.as_str(),
        "config.data_dir",
        "a dataDir whose directory cannot be created is a config fault: {stderr}"
    );
    assert!(
        !stderr.contains("io.read") && !stderr.contains("failed to read"),
        "a directory-creation failure must not render as a read failure: {stderr}"
    );
}

#[test]
#[cfg(unix)]
fn opening_an_existing_store_denied_by_permissions_names_the_path_and_the_fix() {
    // Opening an existing store the process cannot access is a permission fault, not a transient
    // I/O blip: it must carry the typed `store.permission_denied` code, name the store path, and
    // tell the developer to grant access -- never a doubled "I/O error" or a raw OS error code.
    use std::os::unix::fs::PermissionsExt;

    let root = temp_project_uncommitted("run-store-open-denied", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".marrow/data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\nresource Book\n    required title: string\nstore ^books(id: int): Book\n\npub fn main()\n    ^books(1).title = \"Mort\"\n",
        );
    });

    // A first run stamps the store on disk, then the data directory is locked so a second open is
    // denied before any read of the store file.
    let seed = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    let data_dir = root.join(".marrow").join("data");
    std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o000))
        .expect("lock the data directory");

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    // Restore access so the self-cleaning temp project can be removed.
    std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700)).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("store.permission_denied") && stderr.contains("permission denied"),
        "a denied store open must carry the typed permission code: {stderr}"
    );
    assert!(
        stderr.contains("marrow.redb"),
        "the fault must name the store path: {stderr}"
    );
    assert!(
        !stderr.contains("I/O error") && !stderr.contains("os error"),
        "the message must not leak a doubled I/O error or a raw OS error code: {stderr}"
    );
}

#[test]
#[cfg(unix)]
fn opening_a_denied_store_file_names_the_path_and_the_fix() {
    // A readable parent directory but an unreadable store *file* (mode 000) is still a
    // permission fault, classified before the engine even opens the file: the header
    // probe reads the file prefix, so a denied read must carry `store.permission_denied`,
    // name the path, and tell the developer to grant access -- never the `store.io`
    // catch-all with a raw OS error code.
    use std::os::unix::fs::PermissionsExt;

    let root = temp_project_uncommitted("run-store-file-denied", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".marrow/data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\nresource Book\n    required title: string\nstore ^books(id: int): Book\n\npub fn main()\n    ^books(1).title = \"Mort\"\n",
        );
    });

    let seed = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    let store_file = root.join(".marrow").join("data").join("marrow.redb");
    std::fs::set_permissions(&store_file, std::fs::Permissions::from_mode(0o000))
        .expect("deny access to the store file");

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    // Restore access so the self-cleaning temp project can be removed.
    std::fs::set_permissions(&store_file, std::fs::Permissions::from_mode(0o600)).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("store.permission_denied") && stderr.contains("permission denied"),
        "a denied store file must carry the typed permission code: {stderr}"
    );
    assert!(
        stderr.contains("marrow.redb"),
        "the fault must name the store path: {stderr}"
    );
    assert!(
        !stderr.contains("I/O error") && !stderr.contains("os error"),
        "the message must not leak a doubled I/O error or a raw OS error code: {stderr}"
    );
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
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unique index `byIsbn`"), "{stderr}");
    assert!(stderr.contains("(\"978-0\")"), "{stderr}");
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

/// Build a recursion fixture whose `sumTo` self-call descends `depth_arg` frames before
/// its base case. Shared by the over-limit and within-limit depth tests so the recursion
/// source has one owner and cannot drift between them.
fn recursion_project(name: &str, depth_arg: u32) -> TempProject {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            &format!(
                "module app\n\n\
                 fn sumTo(n: int): int\n\
                 \x20\x20\x20\x20if n <= 0\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20return 0\n\
                 \x20\x20\x20\x20return n + sumTo(n - 1)\n\n\
                 pub fn main()\n\
                 \x20\x20\x20\x20print(sumTo({depth_arg}))\n"
            ),
        );
    })
}

#[test]
fn unbounded_recursion_surfaces_a_located_call_depth_fault() {
    // A clean-checking recursion that attempts the 257th call frame fails closed
    // with a located `run.depth` fault at the recursive call site and
    // exit 1. The guard trips before Rust stack exhaustion can decide behavior.
    let root = recursion_project("run-recursion", 255);
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let fault = parse_fault(&output.stderr);
    assert_eq!(fault.code, "run.depth");
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
fn unbounded_recursion_surfaces_a_json_call_depth_payload() {
    let root = recursion_project("run-recursion-json", 255);
    let output = marrow_sub("run", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::json_records_in_stderr(output.stderr);
    let fault = records.last().expect("json fault");
    let diagnostic = &fault["diagnostics"][0];
    assert_eq!(diagnostic["code"], "run.depth", "{fault}");
    assert_eq!(diagnostic["data"]["callee"], "sumTo", "{fault}");
    assert_eq!(diagnostic["data"]["budget"], 256, "{fault}");
    assert_eq!(diagnostic["data"]["observed_depth"], 257, "{fault}");
    assert!(
        diagnostic["source_span"]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("src/app.mw")),
        "{fault}"
    );
    assert_eq!(diagnostic["source_span"]["line"], 6, "{fault}");
    assert!(fault.get("code").is_none(), "{fault}");
}

#[test]
fn a_host_effect_inside_a_transaction_is_its_own_typed_fault() {
    // A rollback-sensitive host effect (here `print`) attempted inside a transaction is a
    // distinct structural fault, `run.transaction_host_effect`, not the missing-capability
    // `run.capability`. It carries the runtime kind and is located at the offending call.
    let root = temp_project("run-host-effect-in-txn", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Counter\n    required value: int\n\
             store ^counter(id: int): Counter\n\n\
             pub fn main()\n    \
             var c: Counter\n    c.value = 1\n    \
             transaction\n        ^counter(1) = c\n        print(\"leaked\")\n",
        );
    });
    let output = marrow_sub("run", &["--format", "json", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = support::json_records_in_stderr(output.stderr);
    let fault = records.last().expect("json fault");
    let diagnostic = &fault["diagnostics"][0];
    assert_eq!(diagnostic["code"], "run.transaction_host_effect", "{fault}");
    assert_eq!(diagnostic["kind"], "runtime", "{fault}");
    assert!(
        diagnostic["source_span"]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("src/app.mw")),
        "{fault}"
    );
    assert!(fault.get("code").is_none(), "{fault}");
}

#[test]
fn recursion_within_the_limit_runs_normally() {
    // A recursion that stays inside the 256-frame limit runs to completion and
    // prints its result, so the bound rejects only runaway recursion.
    let root = recursion_project("run-recursion-ok", 254);
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
