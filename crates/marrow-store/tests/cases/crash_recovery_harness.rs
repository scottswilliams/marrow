#![cfg(feature = "native")]

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::common;
use common::{TempDir, catalog_id};
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

const CHILD_ENV: &str = "MARROW_STORE_CRASH_CHILD";
const PATH_ENV: &str = "MARROW_STORE_CRASH_PATH";
const MODE_ENV: &str = "MARROW_STORE_CRASH_MODE";
const CHILD_TEST: &str = "crash_recovery_harness::crash_harness_child";
const RECORDS: i64 = 96;
const VALUE_BYTES: usize = 128;
const OLD_GENERATION: u8 = 0;
const NEW_GENERATION: u8 = 1;

fn title_path() -> [DataPathSegment; 1] {
    [DataPathSegment::Member(catalog_id("2222222222222222"))]
}

fn write_generation(path: &Path, generation: u8) {
    let store = TreeStore::open(path).expect("open store for generation write");
    store.begin().expect("begin generation transaction");
    let books = catalog_id("1111111111111111");
    for id in 0..RECORDS {
        let identity = [SavedKey::Int(id)];
        store.write_node(&books, &identity).expect("write node");
        store
            .write_data_value(
                &books,
                &identity,
                &title_path(),
                vec![generation; VALUE_BYTES],
            )
            .expect("write generation value");
    }
    store.commit().expect("commit generation transaction");
}

fn read_committed_generation(path: &Path) -> u8 {
    let store = TreeStore::open(path).expect("open or recover crashed store");
    let books = catalog_id("1111111111111111");
    let mut generation = None;
    for id in 0..RECORDS {
        let identity = [SavedKey::Int(id)];
        let value = store
            .read_data_value(&books, &identity, &title_path())
            .expect("read generation value")
            .unwrap_or_else(|| panic!("record {id} disappeared after recovery"));
        assert_eq!(
            value.len(),
            VALUE_BYTES,
            "record {id} changed value length after recovery"
        );
        assert!(
            value.iter().all(|byte| *byte == value[0]),
            "record {id} recovered a torn value"
        );
        match generation {
            Some(expected) => assert_eq!(
                value[0], expected,
                "recovery exposed a mixed transaction at record {id}"
            ),
            None => generation = Some(value[0]),
        }
    }
    generation.expect("seeded store has records")
}

fn signal_parent(label: &str) {
    println!("{label}");
    std::io::stdout().flush().expect("flush child handshake");
}

fn wait_until_killed() -> ! {
    loop {
        std::thread::park();
    }
}

fn run_child() {
    let path = std::env::var_os(PATH_ENV).expect("child store path env");
    let mode = std::env::var(MODE_ENV).expect("child mode env");
    let path = Path::new(&path);
    let store = TreeStore::open(path).expect("child open store");
    store.begin().expect("child begin transaction");
    let books = catalog_id("1111111111111111");
    for id in 0..RECORDS {
        let identity = [SavedKey::Int(id)];
        store
            .write_node(&books, &identity)
            .expect("child write node");
        store
            .write_data_value(
                &books,
                &identity,
                &title_path(),
                vec![NEW_GENERATION; VALUE_BYTES],
            )
            .expect("child write value");
    }

    match mode.as_str() {
        "before_commit" => {
            signal_parent("before_commit");
            wait_until_killed();
        }
        "commit_race" => {
            signal_parent("commit_race");
            store.commit().expect("child commit transaction");
        }
        "after_commit" => {
            store.commit().expect("child commit transaction");
            signal_parent("after_commit");
            wait_until_killed();
        }
        other => panic!("unknown crash harness mode {other}"),
    }
}

#[test]
fn crash_harness_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    run_child();
}

fn spawn_child(path: &Path, mode: &str) -> Child {
    Command::new(std::env::current_exe().expect("current test binary"))
        .arg("--exact")
        .arg(CHILD_TEST)
        .arg("--nocapture")
        .env(CHILD_ENV, "1")
        .env(PATH_ENV, path)
        .env(MODE_ENV, mode)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn crash child")
}

fn wait_for_handshake(child: &mut Child, expected: &str) {
    let stdout = child.stdout.as_mut().expect("child stdout pipe");
    let mut reader = std::io::BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).expect("read child stdout");
        assert!(bytes > 0, "child exited before handshake {expected}");
        if line.trim() == expected {
            return;
        }
    }
}

fn kill_and_wait(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn kill_before_outer_commit_leaves_previous_generation_visible() {
    let dir = TempDir::new("marrow-store-crash-test").expect("temp dir");
    let path = dir.path().join("before-commit.redb");
    write_generation(&path, OLD_GENERATION);

    let mut child = spawn_child(&path, "before_commit");
    wait_for_handshake(&mut child, "before_commit");
    kill_and_wait(child);

    assert_eq!(read_committed_generation(&path), OLD_GENERATION);
}

#[test]
fn kill_after_outer_commit_leaves_new_generation_visible() {
    let dir = TempDir::new("marrow-store-crash-test").expect("temp dir");
    let path = dir.path().join("after-commit.redb");
    write_generation(&path, OLD_GENERATION);

    let mut child = spawn_child(&path, "after_commit");
    wait_for_handshake(&mut child, "after_commit");
    kill_and_wait(child);

    assert_eq!(read_committed_generation(&path), NEW_GENERATION);
}

#[test]
fn kill_racing_outer_commit_is_both_or_invisible() {
    for attempt in 0..12 {
        let dir = TempDir::new("marrow-store-crash-test").expect("temp dir");
        let path = dir.path().join(format!("commit-race-{attempt}.redb"));
        write_generation(&path, OLD_GENERATION);

        let mut child = spawn_child(&path, "commit_race");
        wait_for_handshake(&mut child, "commit_race");
        std::thread::sleep(Duration::from_millis(1));
        kill_and_wait(child);

        let generation = read_committed_generation(&path);
        assert!(
            generation == OLD_GENERATION || generation == NEW_GENERATION,
            "attempt {attempt} recovered unknown generation {generation}"
        );
    }
}
