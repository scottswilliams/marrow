//! `marrow doctor` through the built binary: the terminal compiles the project, spawns the
//! release-verified companion, and relays its report and exit code.
//!
//! The companion layout is staged once into a private directory — the `marrow` and
//! `marrow-runner` binaries beside a `marrow-companions` manifest — because the terminal
//! locates the runner only beside itself. Provisioning and population go through the
//! staged runner and `marrow import`; neither binds a socket, so the whole suite runs
//! inside the sandbox. The stock runner is built into the same directory as the CLI test
//! binary by a workspace build.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::common::{MARROW_BIN, TempDir, unaccepted_ceiling_id, write};

const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const BACKUP_SOURCE: &str = r#"resource Book {
    required title: string
    required isbn: string
    details {
        pages: int
        language: string
    }
    notes[noteId: string] {
        required text: string
        tags[tagId: int] {
            required weight: int
        }
    }
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
}

pub fn bootstrap(): int { return 0 }

pub fn seed() {
    transaction {
        ^books[0] = Book(title: "kept", isbn: "zero", details: Book.details(pages: 37))
        ^books[-1] = Book(title: "removed", isbn: "negative")
        ^books[-1].notes["n"] = Book.notes(text: "removed")
        ^books[-1].notes["n"].tags[0] = Book.notes.tags(weight: 91)
        delete ^books[-1].notes["n"]
        delete ^books[-1]
    }
}

pub fn retained(): int { return ^books[-1].notes["n"].tags[0].weight ?? -1 }
pub fn rootPresent(): bool { return exists(^books[-1]) }
pub fn notePresent(): bool { return exists(^books[-1].notes["n"]) }
pub fn pages(): int { return ^books[0].details.pages ?? -1 }
pub fn lookup(): string {
    if const id = ^books.byIsbn["zero"] {
        if const book = ^books[id] { return book.title }
    }
    return "missing"
}
"#;

// Inserting before a populated field exercises accepted physical numbering through
// the compiler, explicit image operation and ordinary companion-backed reads.
fn populated_apply_preserves_old_values_and_leaves_new_fields_absent(toolchain: &Path) {
    let temp = TempDir::new("apply");
    eprintln!(
        "apply command fixture retained on failure: {}",
        temp.display()
    );
    let project = temp.join("app");
    let source = r#"resource Counter {
    required value: int
    details { tag: int }
    notes[n: int] { required note: int }
}
store ^counters[id: int]: Counter
pub fn bootstrap(): int { return 0 }
pub fn seed() {
    transaction {
        ^counters[0] = Counter(value: 42, details: Counter.details(tag: 7))
        ^counters[0].notes[1] = Counter.notes(note: 9)
    }
}
pub fn oldValue(): int { return ^counters[0].value ?? -1 }
pub fn tag(): int { return ^counters[0].details.tag ?? -1 }
pub fn note(): int { return ^counters[0].notes[1].note ?? -1 }
"#;
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), source);
    write(&temp.join("old.mw"), source);
    let record = |name: &str, output: Output| {
        fs::write(temp.join(format!("{name}.stdout")), &output.stdout).expect("stdout");
        fs::write(temp.join(format!("{name}.stderr")), &output.stderr).expect("stderr");
        fs::write(
            temp.join(format!("{name}.status")),
            format!("{:?}\n", output.status),
        )
        .expect("status");
        output
    };
    let run = |name: &str, dir: &Path, args: &[&str]| {
        let output = record(name, marrow(toolchain, dir, args));
        assert!(
            output.status.success(),
            "{name}: status={:?}\nstdout={}\nstderr={}",
            output.status,
            text(&output.stdout),
            text(&output.stderr)
        );
        output
    };
    let image = |name: &str| {
        let output = record(
            &format!("{name}-preview"),
            marrow(toolchain, &project, &["image", "--out", name]),
        );
        assert!(!output.status.success());
        let stderr = text(&output.stderr);
        assert!(stderr.contains("cli.ceiling_unaccepted"), "{stderr}");
        let ceiling = unaccepted_ceiling_id(&stderr);
        run(
            name,
            &project,
            &["image", "--out", name, "--accept-ceiling", &ceiling],
        );
        (project.join(name).join("program.image"), ceiling)
    };
    run("old-bootstrap", &project, &["run", "main.bootstrap"]);
    let old_ids = fs::read_to_string(project.join(".marrow/ids")).expect("old identities");
    write(&temp.join("old.ids"), &old_ids);
    let (old_image, old_ceiling) = image("old-deployment");
    let old_bytes = fs::read(&old_image).expect("old image");
    let old_image_id = marrow_verify::verify(&old_bytes)
        .expect("verified old artifact")
        .image_id()
        .to_hex();
    let store = temp.join("store");
    let store_arg = store.to_str().expect("store path");
    let provision = record(
        "provision",
        Command::new(toolchain.join("marrow-runner"))
            .args(["provision", "--image"])
            .arg(&old_image)
            .arg("--store")
            .arg(&store)
            .arg("--yes")
            .output()
            .expect("provision"),
    );
    assert!(provision.status.success(), "{}", text(&provision.stderr));
    run(
        "seed",
        &project,
        &["run", "main.seed", "--store", store_arg],
    );
    let old_value = run(
        "old-value-before",
        &project,
        &["run", "main.oldValue", "--store", store_arg],
    );
    assert_eq!(text(&old_value.stdout).trim(), "42");
    let before = run(
        "before",
        &project,
        &["doctor", "--store", store_arg, "--format", "jsonl"],
    );
    let before: serde_json::Value = serde_json::from_slice(&before.stdout).expect("old binding");
    assert_eq!(before["image"], old_image_id);

    let new_source = source
        .replace("    required value", "    extra: int\n    required value")
        .replace("required note", "noteExtra: int\nrequired note")
        + r#"
pub fn extraPresent(): bool { return exists(^counters[0].extra) }
pub fn writeExtra() {
    transaction {
        place counter = ^counters[0]
        if exists(counter) { counter.extra = 77 }
        place note = ^counters[0].notes[1]
        if exists(note) { note.noteExtra = 99 }
    }
}
pub fn extraValue(): int { return ^counters[0].extra ?? -1 }
pub fn noteExtra(): int { return ^counters[0].notes[1].noteExtra ?? -1 }
"#;
    write(&project.join("src/main.mw"), &new_source);
    write(&temp.join("new.mw"), &new_source);
    run("new-bootstrap", &project, &["run", "main.bootstrap"]);
    let new_ids = fs::read_to_string(project.join(".marrow/ids")).expect("new identities");
    write(&temp.join("new.ids"), &new_ids);
    for line in old_ids.lines().filter(|line| line.starts_with("id ")) {
        assert!(
            new_ids.lines().any(|new| new == line),
            "changed identity: {line}"
        );
    }
    let (new_image, new_ceiling) = image("new-deployment");
    assert_eq!(
        fs::read(&old_image).expect("preserved old image"),
        old_bytes
    );
    let new_bytes = fs::read(&new_image).expect("new image");
    assert_ne!(new_bytes, old_bytes);
    let new_image_id = marrow_verify::verify(&new_bytes)
        .expect("verified new artifact")
        .image_id()
        .to_hex();
    let applied = run(
        "apply",
        &temp,
        &[
            "apply",
            "--store",
            store_arg,
            "--old-image",
            old_image.to_str().expect("old image path"),
            "--new-image",
            new_image.to_str().expect("new image path"),
            "--accept-ceiling",
            &new_ceiling,
            "--format",
            "jsonl",
        ],
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&applied.stdout).expect("apply receipt");
    assert_eq!(receipt["kind"], "apply");
    assert_eq!(receipt["outcome"], "applied");
    assert_eq!(receipt["instance"], before["instance"]);
    assert_eq!(receipt["old_image"], before["image"]);
    assert_eq!(receipt["old_ceiling"], old_ceiling);
    assert_eq!(receipt["ceiling"], new_ceiling);
    assert_eq!(receipt["new_image"], new_image_id);
    for (name, export, expected) in [
        ("old-value-after", "main.oldValue", "42"),
        ("extra-absent", "main.extraPresent", "false"),
        ("nested-extra-absent", "main.noteExtra", "-1"),
    ] {
        let result = run(name, &project, &["run", export, "--store", store_arg]);
        assert_eq!(text(&result.stdout).trim(), expected);
    }
    run(
        "write-extra",
        &project,
        &["run", "main.writeExtra", "--store", store_arg],
    );
    for (name, export, expected) in [
        ("extra-value", "main.extraValue", "77"),
        ("old-value-final", "main.oldValue", "42"),
        ("old-group-final", "main.tag", "7"),
        ("old-note-final", "main.note", "9"),
        ("new-note-final", "main.noteExtra", "99"),
    ] {
        let result = run(name, &project, &["run", export, "--store", store_arg]);
        assert_eq!(text(&result.stdout).trim(), expected);
    }
    let applied_head = fs::read(store.join("head")).expect("applied head");
    let backup = temp.join("applied.backup");
    let backed = run(
        "backup-applied",
        &project,
        &[
            "backup",
            "--store",
            store_arg,
            "--out",
            backup.to_str().expect("backup path"),
            "--format",
            "jsonl",
        ],
    );
    let backed: serde_json::Value = serde_json::from_slice(&backed.stdout).expect("backup receipt");
    assert_eq!(backed["image"], new_image_id);
    assert_eq!(backed["instance"], receipt["instance"]);
    assert_eq!(
        fs::read(store.join("head")).expect("source head"),
        applied_head
    );
    write(
        &project.join("src/main.mw"),
        "invalid source during restore and recovery",
    );
    let restored = temp.join("restored");
    let restored_arg = restored.to_str().expect("restored path");
    let restored_receipt = run(
        "restore-applied",
        &temp,
        &[
            "restore",
            "--from",
            backup.to_str().expect("backup path"),
            "--store",
            restored_arg,
            "--format",
            "jsonl",
        ],
    );
    let restored_receipt: serde_json::Value =
        serde_json::from_slice(&restored_receipt.stdout).expect("restore receipt");
    for (value, digits) in [
        (&receipt["instance"], 32),
        (&backed["instance"], 32),
        (&restored_receipt["instance"], 32),
        (&backed["content_digest"], 64),
        (&restored_receipt["content_digest"], 64),
    ] {
        let value = value.as_str().expect("hex identity");
        assert_eq!(value.len(), digits);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }
    assert_eq!(restored_receipt["outcome"], "complete");
    assert_ne!(restored_receipt["instance"], receipt["instance"]);
    assert_eq!(restored_receipt["image"], new_image_id);
    assert_eq!(restored_receipt["content_digest"], backed["content_digest"]);
    assert_eq!(
        fs::read(restored.join("head")).expect("restored head"),
        applied_head
    );
    fs::write(restored.join("envelope.replacing"), b"partial envelope").expect("debris");
    let recovered = run(
        "recover-applied-image",
        &temp,
        &[
            "recover",
            "--store",
            restored_arg,
            "--image",
            new_image.to_str().expect("new image path"),
            "--format",
            "jsonl",
        ],
    );
    let recovered: serde_json::Value =
        serde_json::from_slice(&recovered.stdout).expect("recovery record");
    assert_eq!(recovered["kind"], "recovery");
    assert_eq!(recovered["outcome"], "activated");
    assert_eq!(recovered["store"], restored_arg);
    assert_eq!(recovered["instance"], restored_receipt["instance"]);
    assert_eq!(recovered["image"], new_image_id);
    let preserved = recovered["preserved"].as_array().expect("preserved names");
    assert_eq!(preserved.len(), 1);
    assert_eq!(
        fs::read(restored.join(preserved[0].as_str().expect("preserved filename")))
            .expect("preserved debris"),
        b"partial envelope"
    );
    assert_eq!(
        fs::read(restored.join("head")).expect("recovered head"),
        applied_head
    );
    write(&project.join("src/main.mw"), &new_source);
    for (name, export, expected) in [
        ("restored-old", "main.oldValue", "42"),
        ("restored-extra", "main.extraValue", "77"),
        ("restored-group", "main.tag", "7"),
        ("restored-note", "main.note", "9"),
        ("restored-note-extra", "main.noteExtra", "99"),
    ] {
        let result = run(name, &project, &["run", export, "--store", restored_arg]);
        assert_eq!(text(&result.stdout).trim(), expected);
    }
}

// This process-level control earns its cost by crossing compiler, companion,
// native publication and restore without the original project being available.
fn backup_restores_absent_ancestor_descendants_without_a_project(toolchain: &Path) {
    let temp = std::mem::ManuallyDrop::new(TempDir::new("backup"));
    eprintln!(
        "backup command fixture retained on failure: {}",
        temp.display()
    );
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), BACKUP_SOURCE);
    let run = |dir: &Path, args: &[&str]| {
        let result = marrow(toolchain, dir, args);
        assert!(
            result.status.success(),
            "{args:?}: {}",
            text(&result.stderr)
        );
        result
    };
    run(&project, &["run", "main.bootstrap"]);
    let refused = marrow(toolchain, &project, &["image", "--out", "deployment"]);
    assert!(!refused.status.success());
    let stderr = text(&refused.stderr);
    let ceiling = &unaccepted_ceiling_id(&stderr);
    run(
        &project,
        &["image", "--out", "deployment", "--accept-ceiling", ceiling],
    );
    let image = project.join("deployment/program.image");
    let store = temp.join("source");
    let provision = Command::new(toolchain.join("marrow-runner"))
        .args(["provision", "--image"])
        .arg(&image)
        .arg("--store")
        .arg(&store)
        .arg("--yes")
        .output()
        .expect("provision");
    assert!(provision.status.success(), "{}", text(&provision.stderr));
    run(
        &project,
        &["run", "main.seed", "--store", store.to_str().unwrap()],
    );
    let head = fs::read(store.join("head")).expect("source head");
    fs::write(store.join("lock"), b"unclean").expect("prior marker");
    let before = store_files(&store);
    let backup = temp.join("complete.backup");
    let receipt = run(
        &project,
        &[
            "backup",
            "--store",
            store.to_str().unwrap(),
            "--out",
            backup.to_str().unwrap(),
            "--format",
            "jsonl",
        ],
    );
    let backed: serde_json::Value =
        serde_json::from_slice(&receipt.stdout).expect("backup receipt");
    assert_eq!(backed["outcome"], "complete");
    assert!(store_files(&store) == before, "store artifacts changed");

    // A code edit cannot silently change the source binding for backup.
    write(
        &project.join("src/main.mw"),
        BACKUP_SOURCE.replace("return 0", "return 1"),
    );
    let refused_path = temp.join("stale.backup");
    let refused = marrow(
        toolchain,
        &project,
        &[
            "backup",
            "--store",
            store.to_str().unwrap(),
            "--out",
            refused_path.to_str().unwrap(),
            "--format",
            "jsonl",
        ],
    );
    assert!(!refused.status.success());
    let failure: serde_json::Value = serde_json::from_slice(&refused.stdout).expect("refusal");
    assert_eq!(
        failure["code"],
        marrow_codes::Code::StoreImageNotActive.as_str()
    );
    assert!(!refused_path.exists());
    assert!(store_files(&store) == before, "store artifacts changed");
    assert_eq!(fs::read(store.join("head")).unwrap(), head);
    write(&project.join("src/main.mw"), "not valid Marrow");
    let restored = temp.join("restored");
    let receipt = run(
        &temp,
        &[
            "restore",
            "--from",
            backup.to_str().unwrap(),
            "--store",
            restored.to_str().unwrap(),
            "--format",
            "jsonl",
        ],
    );
    let restored_receipt: serde_json::Value =
        serde_json::from_slice(&receipt.stdout).expect("restore receipt");
    for (record, field, digits) in [
        (&backed, "instance", 32),
        (&restored_receipt, "instance", 32),
        (&backed, "image", 64),
        (&restored_receipt, "image", 64),
        (&backed, "content_digest", 64),
        (&restored_receipt, "content_digest", 64),
    ] {
        let value = record[field].as_str().expect("hex identity");
        assert_eq!(value.len(), digits);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }
    assert_eq!(restored_receipt["outcome"], "complete");
    assert_ne!(backed["instance"], restored_receipt["instance"]);
    assert_eq!(backed["image"], restored_receipt["image"]);
    assert_eq!(backed["content_digest"], restored_receipt["content_digest"]);
    assert_eq!(fs::read(restored.join("head")).unwrap(), head);
    write(&project.join("src/main.mw"), BACKUP_SOURCE);
    for (export, expected) in [
        ("main.retained", "91"),
        ("main.rootPresent", "false"),
        ("main.notePresent", "false"),
        ("main.pages", "37"),
        ("main.lookup", "kept"),
    ] {
        let result = run(
            &project,
            &["run", export, "--store", restored.to_str().unwrap()],
        );
        assert_eq!(text(&result.stdout).trim(), expected, "{export}");
    }
    // Only the successful fixture is retired; failure unwinding keeps its original bytes.
    drop(std::mem::ManuallyDrop::into_inner(temp));
}

/// Stage the CLI, runner and release manifest once for the command suite.
fn toolchain() -> TempDir {
    let runner = Path::new(MARROW_BIN)
        .parent()
        .expect("binary dir")
        .join("marrow-runner");
    assert!(
        runner.is_file(),
        "stock runner not built at {}; run a workspace build first",
        runner.display()
    );
    let dir = TempDir::new("toolchain");
    fs::copy(MARROW_BIN, dir.join("marrow")).expect("copy marrow");
    fs::copy(&runner, dir.join("marrow-runner")).expect("copy runner");
    let bytes = fs::read(&runner).expect("read runner");
    let id = marrow_image::companion_release_id(&bytes).to_hex();
    fs::write(
        dir.join("marrow-companions"),
        format!(
            "marrow companions v0\nrelease {}\nrunner marrow-runner {id}\nend\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .expect("write manifest");
    dir
}

fn store_files(dir: &Path) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
    fs::read_dir(dir)
        .expect("store entries")
        .map(|entry| {
            let entry = entry.expect("store entry");
            (
                entry.file_name(),
                fs::read(entry.path()).expect("store file"),
            )
        })
        .collect()
}

/// A durable project at `dir` with its ledger, and a provisioned store beside it
/// populated with two counters through `marrow import`.
fn project_with_store(toolchain: &Path, temp: &TempDir) -> (PathBuf, PathBuf) {
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), SOURCE);
    write(&project.join(".marrow/ids"), IDS);
    write(
        &project.join("seed.jsonl"),
        "{\"id\":1,\"value\":10,\"label\":\"ten\"}\n{\"id\":2,\"value\":20}\n",
    );
    let store = temp.join("store");
    let imported = marrow(
        toolchain,
        &project,
        &[
            "import",
            "--store",
            store.to_str().expect("store path"),
            "--jsonl",
            "seed.jsonl",
            "--root",
            "counters",
            "--keys",
            "id",
        ],
    );
    assert!(
        imported.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&imported.stderr)
    );
    (project, store)
}

fn marrow(toolchain: &Path, dir: &Path, args: &[&str]) -> Output {
    Command::new(toolchain.join("marrow"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run the staged marrow binary")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn a_clean_store_audits_with_a_stable_digest_and_exit_zero(toolchain: &Path) {
    let temp = TempDir::new("clean");
    let (project, store) = project_with_store(toolchain, &temp);
    let store_arg = store.to_str().expect("store path");
    fs::write(store.join("lock"), b"unclean").expect("prior marker");
    let before = store_files(&store);

    let first = marrow(toolchain, &project, &["doctor", "--store", store_arg]);
    assert!(first.status.success(), "{}", text(&first.stderr));
    assert!(store_files(&store) == before, "store artifacts changed");
    let out = text(&first.stdout);
    assert!(
        out.starts_with(&format!("Logical store audit: {store_arg}\n")),
        "{out}"
    );
    assert!(
        out.contains("Physical integrity was not checked.\n"),
        "{out}"
    );
    assert!(out.contains("entries 2, index cells 0, cells 6\n"), "{out}");
    assert!(out.contains("\nno findings\n"), "{out}");
    let digest = out
        .lines()
        .find_map(|line| line.strip_prefix("digest "))
        .expect("the report names the digest")
        .to_string();
    assert_eq!(digest.len(), 64);

    let second = marrow(
        toolchain,
        &project,
        &["doctor", "--store", store_arg, "--format", "jsonl"],
    );
    assert!(second.status.success(), "{}", text(&second.stderr));
    assert!(store_files(&store) == before, "store artifacts changed");
    let out = text(&second.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "{out}");
    assert!(lines[0].starts_with("{\"cells\":6,\"digest\":\""), "{out}");
    assert!(
        lines[0].contains(&format!("\"digest\":\"{digest}\"")),
        "{out}"
    );
    assert!(
        lines[0].contains("\"entries\":2,\"findings\":0,\"image\":\""),
        "{out}"
    );
    assert!(
        lines[0].ends_with(&format!(
            "\"index_cells\":0,\"instance\":\"{}\",\"kind\":\"doctor\",\"listed\":0,\"outcome\":\"clean\",\"physical_integrity\":\"not_checked\",\"scope\":\"logical\",\"store\":\"{store_arg}\"}}",
            instance_of(lines[0])
        )),
        "{out}"
    );
}

fn instance_of(line: &str) -> &str {
    let start = line.find("\"instance\":\"").expect("instance") + "\"instance\":\"".len();
    &line[start..start + 32]
}

fn recovery_refuses_an_altered_engine_with_exit_one(toolchain: &Path) {
    let temp = TempDir::new("flip");
    let (project, store) = project_with_store(toolchain, &temp);
    let engine = store.join("store.redb");
    let mut bytes = fs::read(&engine).expect("read engine");
    let at = bytes
        .windows(3)
        .position(|window| window == b"ten")
        .expect("the stored label is in the engine file");
    bytes[at] = b'T';
    fs::write(&engine, bytes).expect("write engine");
    let envelope = fs::read(store.join("envelope")).expect("envelope");
    fs::write(store.join("envelope.replacing"), b"unexplained").expect("debris");

    let output = marrow(
        toolchain,
        &project,
        &[
            "recover",
            "--store",
            store.to_str().expect("store path"),
            "--format",
            "jsonl",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("recovery error");
    assert_eq!(report["outcome"], "error");
    assert_eq!(report["code"], "store.corruption");
    assert!(report["preserved"].as_array().expect("names").is_empty());
    assert_eq!(
        fs::read(store.join("envelope")).expect("no activation"),
        envelope
    );
    assert_eq!(
        fs::read(store.join("envelope.replacing")).expect("debris unmoved"),
        b"unexplained"
    );
}

fn a_code_only_edit_must_be_rebound_before_it_audits(toolchain: &Path) {
    let temp = TempDir::new("stale");
    let (project, store) = project_with_store(toolchain, &temp);
    fs::remove_file(store.join("lock")).expect("remove clean marker");
    let before = store_files(&store);
    write(&project.join("src/main.mw"), SOURCE.replace("?? 0", "?? 1"));
    let output = marrow(
        toolchain,
        &project,
        &["doctor", "--store", store.to_str().expect("store path")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(store_files(&store) == before, "store artifacts changed");
    assert!(
        text(&output.stderr).starts_with("store.image_not_active: "),
        "{}",
        text(&output.stderr)
    );

    let output = marrow(
        toolchain,
        &project,
        &[
            "doctor",
            "--store",
            store.to_str().expect("store path"),
            "--format",
            "jsonl",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(store_files(&store) == before, "store artifacts changed");
    assert!(
        text(&output.stdout).starts_with(
            "{\"code\":\"store.image_not_active\",\"kind\":\"doctor\",\"outcome\":\"error\",\"store\":\""
        ),
        "{}",
        text(&output.stdout)
    );
}

fn usage_and_absent_store_refusals_keep_their_codes(toolchain: &Path) {
    let temp = TempDir::new("usage");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), SOURCE);
    write(&project.join(".marrow/ids"), IDS);
    let output = marrow(toolchain, &project, &["doctor"]);
    assert_eq!(output.status.code(), Some(2));
    let output = marrow(
        toolchain,
        &project,
        &["doctor", "--store", "nowhere", "--format", "yaml"],
    );
    assert_eq!(output.status.code(), Some(2));
    let output = marrow(toolchain, &project, &["doctor", "--store", "nowhere"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).starts_with("store.io: "),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_compiler_resource_limit_keeps_its_typed_code_before_store_access() {
    let temp = TempDir::new("compiler-limit");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    let mut source = String::from("module main\n\n");
    for i in 0..257 {
        source.push_str(&format!("pub fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    write(&project.join("src/main.mw"), &source);

    for format in ["text", "jsonl"] {
        let output = Command::new(MARROW_BIN)
            .args(["doctor", "--store", "nowhere", "--format", format])
            .current_dir(&project)
            .env("NO_COLOR", "1")
            .output()
            .expect("run doctor over the over-limit project");
        assert_eq!(output.status.code(), Some(1));
        let error = text(&output.stderr);
        assert!(
            error.starts_with(&format!(
                "{}: ",
                marrow_codes::Code::CliCompilerResourceLimit.as_str()
            )),
            "{format}: {error}"
        );
        assert!(output.stdout.is_empty(), "no audit report was produced");
        assert!(!project.join("nowhere").exists());
    }
}

fn a_storeless_program_has_nothing_to_audit(toolchain: &Path) {
    let temp = TempDir::new("storeless");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &project.join("src/main.mw"),
        "pub fn answer(): int {\n    return 42\n}\n",
    );
    let output = marrow(toolchain, &project, &["doctor", "--store", "nowhere"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).starts_with("cli.durable_unsupported: "),
        "{}",
        text(&output.stderr)
    );
}

fn an_invalid_scalar_reports_a_logical_finding(toolchain: &Path) {
    let temp = TempDir::new("invalid-scalar");
    let (project, store) = project_with_store(toolchain, &temp);
    let engine = store.join("store.redb");
    let mut bytes = fs::read(&engine).expect("read engine");
    let at = bytes
        .windows(3)
        .position(|window| window == b"ten")
        .expect("stored label");
    bytes[at] = 0xff;
    fs::write(&engine, &bytes).expect("write malformed UTF-8");
    let code = marrow_codes::Code::StoreAuditUndecodable;
    let place = "^counters[1].label";
    for format in ["text", "jsonl"] {
        let output = marrow(
            toolchain,
            &project,
            &[
                "doctor",
                "--store",
                store.to_str().expect("store path"),
                "--format",
                format,
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        let out = text(&output.stdout);
        if format == "text" {
            assert!(
                out.contains(&format!("  {} at {place}\n", code.as_str())),
                "{out}"
            );
            assert!(
                out.contains("Physical integrity was not checked.\n"),
                "{out}"
            );
        } else {
            let lines: Vec<_> = out.lines().collect();
            assert_eq!(lines.len(), 2, "{out}");
            assert!(lines[0].contains("\"outcome\":\"findings\""), "{out}");
            assert!(lines[0].contains("\"scope\":\"logical\""), "{out}");
            assert!(
                lines[0].contains("\"physical_integrity\":\"not_checked\""),
                "{out}"
            );
            assert_eq!(
                lines[1],
                format!(
                    "{{\"code\":\"{}\",\"kind\":\"finding\",\"place\":\"{place}\"}}",
                    code.as_str()
                )
            );
        }
        assert_eq!(fs::read(&engine).expect("unchanged engine"), bytes);
    }
}

#[test]
fn doctor_reports_and_refusals_share_one_owned_toolchain() {
    let staged = toolchain();
    let path = staged.to_path_buf();
    populated_apply_preserves_old_values_and_leaves_new_fields_absent(&path);
    a_clean_store_audits_with_a_stable_digest_and_exit_zero(&path);
    explicit_recovery_preserves_data_and_reports_moved_files(&path);
    recovery_refuses_an_altered_engine_with_exit_one(&path);
    #[cfg(unix)]
    recovery_failure_reports_a_preservation_move(&path);
    a_code_only_edit_must_be_rebound_before_it_audits(&path);
    usage_and_absent_store_refusals_keep_their_codes(&path);
    a_storeless_program_has_nothing_to_audit(&path);
    an_invalid_scalar_reports_a_logical_finding(&path);
    backup_restores_absent_ancestor_descendants_without_a_project(&path);
    drop(staged);
    assert!(!path.exists(), "the suite removes its staged toolchain");
}

fn explicit_recovery_preserves_data_and_reports_moved_files(toolchain: &Path) {
    use serde_json::Value;

    let temp = TempDir::new("recover");
    let (project, store) = project_with_store(toolchain, &temp);
    let store_arg = store.to_str().expect("store path");
    let before = marrow(
        toolchain,
        &project,
        &["doctor", "--store", store_arg, "--format", "jsonl"],
    );
    assert!(before.status.success(), "{}", text(&before.stderr));
    fs::write(store.join("envelope.replacing"), b"partial envelope").expect("debris");
    let recovered = marrow(
        toolchain,
        &project,
        &["recover", "--store", store_arg, "--format", "jsonl"],
    );
    if !recovered.status.success() {
        let original = temp.to_path_buf();
        std::mem::forget(temp);
        panic!(
            "explicit recovery failed: {}; preserve {}",
            text(&recovered.stderr),
            original.display()
        );
    }
    let Value::Object(fields) = serde_json::from_slice(&recovered.stdout).expect("recovery record")
    else {
        panic!("recovery object");
    };
    assert_eq!(fields["kind"], "recovery");
    assert_eq!(fields["outcome"], "activated");
    assert_eq!(fields["store"], store_arg);
    let prior: Value = serde_json::from_slice(&before.stdout).expect("doctor record");
    assert_eq!(fields["instance"], prior["instance"]);
    assert_eq!(fields["image"], prior["image"]);
    let Some(Value::Array(names)) = fields.get("preserved") else {
        panic!("preserved names");
    };
    assert_eq!(names.len(), 1);
    let Value::String(name) = &names[0] else {
        panic!("preserved filename");
    };
    assert_eq!(
        fs::read(store.join(name)).expect("preserved bytes"),
        b"partial envelope"
    );
    let after = marrow(
        toolchain,
        &project,
        &["doctor", "--store", store_arg, "--format", "jsonl"],
    );
    assert!(after.status.success(), "{}", text(&after.stderr));
    assert_eq!(after.stdout, before.stdout);
}

#[cfg(unix)]
fn recovery_failure_reports_a_preservation_move(toolchain: &Path) {
    for format in ["text", "jsonl"] {
        let temp = TempDir::new("recover-refusal");
        let (project, store) = project_with_store(toolchain, &temp);
        let envelope = fs::read(store.join("envelope")).expect("envelope");
        let head = fs::read(store.join("head")).expect("head");
        let peer = temp.join("peer");
        fs::write(&peer, b"peer bytes").expect("peer");
        fs::write(store.join("envelope.replacing"), b"partial envelope").expect("first slot");
        std::os::unix::fs::symlink(&peer, store.join("head.replacing"))
            .expect("refused second slot");
        let output = marrow(
            toolchain,
            &project,
            &[
                "recover",
                "--store",
                store.to_str().expect("path"),
                "--format",
                format,
            ],
        );
        assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
        let name = if format == "jsonl" {
            let report: serde_json::Value =
                serde_json::from_slice(&output.stdout).expect("failure record");
            assert_eq!(report["kind"], "recovery");
            assert_eq!(report["outcome"], "error");
            assert_eq!(report["code"], "store.corruption");
            assert!(report.get("instance").is_none());
            assert_eq!(report["preserved"].as_array().expect("names").len(), 1);
            report["preserved"][0].as_str().expect("name").to_owned()
        } else {
            let report = text(&output.stdout);
            assert!(report.starts_with("store.corruption:"));
            report
                .lines()
                .find_map(|line| line.strip_prefix("preserved "))
                .expect("known move")
                .to_owned()
        };
        assert_eq!(
            fs::read(store.join(name)).expect("known move bytes"),
            b"partial envelope"
        );
        assert_eq!(
            fs::read(store.join("envelope")).expect("no activation"),
            envelope
        );
        assert_eq!(fs::read(store.join("head")).expect("head unchanged"), head);
        assert_eq!(
            fs::read_link(store.join("head.replacing")).expect("link retained"),
            peer
        );
        assert_eq!(fs::read(&peer).expect("peer unchanged"), b"peer bytes");
    }
}
