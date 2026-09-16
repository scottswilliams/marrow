//! One text bound over `marrow run` arguments, storeless and against a store.
//!
//! The language bounds text at 65,536 bytes and the wire bounds a JSON string at the
//! same number, so an argument the terminal admits must never be refused downstream
//! as `wire.string_limit`, and an argument it refuses must be refused the same way on
//! both paths. The table below drives (bound, bound + 1) x (storeless, `--store`)
//! through the built binary: only the boundary and the invocation path vary.
//!
//! The `--store` rows need the companion layout, so the suite stages a toolchain and
//! provisions one store through `marrow import`, then reuses both for every row.

use std::path::{Path, PathBuf};

use crate::common::{TempDir, stage_toolchain, staged_marrow_in, write};

/// The language text bound, in UTF-8 bytes.
const TEXT_BOUND: usize = 65_536;

const SOURCE: &str = r#"resource Note {
    required text: string
}

store ^notes[id: int]: Note

pub fn blank(s: string): bool {
    return s == ""
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
     id product Note 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id field Note.text 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id root notes 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id key notes.id 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     high-water 0\n\
     end\n";

/// Which invocation path a row exercises: in this process on the VM, or over the
/// wire to a companion attached to a store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RunPath {
    Storeless,
    Store,
}

#[test]
fn one_text_bound_admits_and_refuses_the_same_argument_on_both_run_paths() {
    let toolchain = stage_toolchain();
    let temp = TempDir::new("argument-bound");
    let (project, store) = project_with_store(&toolchain, &temp);
    let store_arg = store.to_str().expect("store path");

    for size in [TEXT_BOUND, TEXT_BOUND + 1] {
        let admitted = size <= TEXT_BOUND;
        let argument = "a".repeat(size);
        for path in [RunPath::Storeless, RunPath::Store] {
            let mut args = vec!["run", "main.blank"];
            if path == RunPath::Store {
                args.extend(["--store", store_arg]);
            }
            args.extend(["--", argument.as_str()]);
            let outcome = staged_marrow_in(&toolchain, &project, &args);
            let stdout = outcome.stdout_text().into_owned();
            let context = format!("{size} bytes, {path:?}: {stdout}{}", outcome.stderr_text());

            if admitted {
                assert_eq!(outcome.code(), Some(0), "{context}");
                assert_eq!(stdout, "false\n", "{context}");
            } else {
                assert_eq!(outcome.code(), Some(1), "{context}");
                assert!(
                    stdout.starts_with("cli.argument_limit: "),
                    "the bound is reported by its own code on both paths; {context}"
                );
            }
        }
    }
}

/// The refusal is the terminal's own, so it lands before any store effect and carries
/// the same typed code under `--format jsonl` as in text.
#[test]
fn an_oversized_argument_is_refused_before_the_named_store_is_touched() {
    let toolchain = stage_toolchain();
    let temp = TempDir::new("argument-bound-jsonl");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), SOURCE);
    write(&project.join(".marrow/ids"), IDS);
    let argument = "a".repeat(TEXT_BOUND + 1);

    let outcome = staged_marrow_in(
        &toolchain,
        &project,
        &[
            "run",
            "main.blank",
            "--store",
            "no-such-store",
            "--format",
            "jsonl",
            "--",
            argument.as_str(),
        ],
    );
    assert_eq!(outcome.code(), Some(1), "{}", outcome.stderr_text());
    assert_eq!(
        outcome.jsonl_lines(),
        vec![r#"{"code":"cli.argument_limit","kind":"run","outcome":"error"}"#.to_string()],
        "{}",
        outcome.stderr_text()
    );
    assert!(
        !project.join("no-such-store").exists(),
        "the named store was created"
    );
}

/// A durable project at `temp/app` with its ledger, and a store beside it provisioned
/// and populated through `marrow import`.
fn project_with_store(toolchain: &Path, temp: &TempDir) -> (PathBuf, PathBuf) {
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), SOURCE);
    write(&project.join(".marrow/ids"), IDS);
    write(&project.join("seed.jsonl"), "{\"id\":1,\"text\":\"one\"}\n");
    let store = temp.join("store");
    let imported = staged_marrow_in(
        toolchain,
        &project,
        &[
            "import",
            "--store",
            store.to_str().expect("store path"),
            "--jsonl",
            "seed.jsonl",
            "--root",
            "notes",
            "--keys",
            "id",
        ],
    );
    assert!(
        imported.success(),
        "import failed: {}",
        imported.stderr_text()
    );
    (project, store)
}
