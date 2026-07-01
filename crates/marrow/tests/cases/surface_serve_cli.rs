use crate::support;
use crate::support_surface::{
    create_descriptor, create_field_catalog_id, delete_descriptor, read_descriptor, route_by_alias,
    spawn_surface_server, spawn_surface_server_with_args, spawn_surface_server_with_env_args,
    update_descriptor, update_field_catalog_id, wait_for_client_change,
};
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use support::{
    marrow, marrow_bounded, marrow_sub, redb_store_path, temp_project, temp_project_uncommitted,
    write,
};

const SURFACE_SOURCE: &str = "module app\n\
 \n\
 resource Book\n\
 \x20\x20\x20\x20required title: string\n\
 \x20\x20\x20\x20author: string\n\
 store ^books(id: int): Book\n\
 \x20\x20\x20\x20index byAuthor(author, id)\n\
 \n\
pub fn seed()\n\
\x20\x20\x20\x20var book: Book\n\
\x20\x20\x20\x20book.title = \"Dune\"\n\
\x20\x20\x20\x20book.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20var sequel: Book\n\
\x20\x20\x20\x20sequel.title = \"Dune Messiah\"\n\
\x20\x20\x20\x20sequel.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(2) = sequel\n\
\n\
pub fn retitle(id: int, title: string): string\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(id).title = title\n\
\x20\x20\x20\x20return title\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title, author\n\
\x20\x20\x20\x20create title, author\n\
\x20\x20\x20\x20update author\n\
\x20\x20\x20\x20delete\n\
\x20\x20\x20\x20collection ^books.byAuthor as byAuthor\n\
\x20\x20\x20\x20action retitle\n";

const SINGLETON_SURFACE_SOURCE: &str = "module app\n\
 \n\
 resource Settings\n\
 \x20\x20\x20\x20required theme: string\n\
 store ^settings: Settings\n\
 \n\
pub fn seed()\n\
\x20\x20\x20\x20var settings: Settings\n\
\x20\x20\x20\x20settings.theme = \"dark\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^settings = settings\n\
\n\
surface SettingsSurface from ^settings\n\
\x20\x20\x20\x20fields theme\n\
\x20\x20\x20\x20delete\n";

const CLIENT_SURFACE_SOURCE: &str = "module app\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
\x20\x20\x20\x20author: string\n\
store ^books(id: int): Book\n\
\x20\x20\x20\x20index byAuthor(author, id)\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var book: Book\n\
\x20\x20\x20\x20book.title = \"Dune\"\n\
\x20\x20\x20\x20book.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20var sequel: Book\n\
\x20\x20\x20\x20sequel.title = \"Dune Messiah\"\n\
\x20\x20\x20\x20sequel.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(2) = sequel\n\
\n\
pub fn retitle(id: int, title: string): string\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(id).title = title\n\
\x20\x20\x20\x20return title\n\
\n\
pub fn describe(id: int): string\n\
\x20\x20\x20\x20return (^books(id).title ?? \"\") + \"|\" + (^books(id).author ?? \"\")\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title, author\n\
\x20\x20\x20\x20create title, author\n\
\x20\x20\x20\x20update author\n\
\x20\x20\x20\x20delete\n\
\x20\x20\x20\x20collection ^books.byAuthor as byAuthor\n\
\x20\x20\x20\x20action retitle\n\
\x20\x20\x20\x20read describe\n";

/// A native-store config that declares a client output path so serve regenerates the TypeScript
/// client write-if-changed at startup and on a `--watch` source change.
fn native_config_with_client() -> String {
    r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#
        .to_string()
}

#[test]
fn serve_watch_rewrites_client_on_source_change() {
    let root = temp_project("serve-watch-client", |root| {
        write(root, "marrow.json", &native_config_with_client());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let out = root.join("generated/marrow.ts");
    let before = std::fs::read_to_string(&out).expect("client present after seed");
    let (_server, _addr) = spawn_surface_server_with_args(&root, &["--write", "--watch"]);
    let changed = CLIENT_SURFACE_SOURCE.replace(
        "    read describe\n",
        "    read describe\n    read describe as summary\n",
    );
    write(&root, "src/app.mw", &changed);
    let after = wait_for_client_change(&out, &before, std::time::Duration::from_secs(8));
    assert_ne!(
        after, before,
        "serve --watch must rewrite the client on a surface change"
    );
}

#[test]
fn serve_startup_writes_declared_client() {
    let root = temp_project("serve-writes-client", |root| {
        write(
            root,
            "marrow.json",
            r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#,
        );
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let out = root.join("generated/marrow.ts");
    std::fs::remove_file(&out).ok();

    let (_server, _addr) = spawn_surface_server(&root);
    assert!(
        out.exists(),
        "serve startup must regenerate the declared client"
    );
}

#[test]
fn serve_write_holds_the_store_lock_for_its_lifetime() {
    let root = temp_project("serve-write-lock", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let (_server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);

    // A concurrent write-capable run must be refused while the write server owns the store.
    assert_store_locked(
        support::marrow_bounded(
            &["run", "--entry", "app::seed", project],
            std::time::Duration::from_secs(15),
        ),
        "racing run",
    );
    // A read-only inspection must also be refused: the write server excludes any other open.
    assert_store_locked(
        support::marrow_bounded(
            &["data", "stats", project],
            std::time::Duration::from_secs(15),
        ),
        "racing stats",
    );
    // A second write server must fail fast rather than coexisting; bind to an ephemeral port so a
    // regression that lets it listen forever surfaces as a bound timeout, not a passing test.
    assert_store_locked(
        support::marrow_bounded(
            &["serve", "--write", "--addr", "127.0.0.1:0", project],
            std::time::Duration::from_secs(15),
        ),
        "second serve --write",
    );
}

#[test]
fn read_only_serve_blocks_a_writer() {
    let root = temp_project("serve-read-blocks-writer", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let (_server, _addr) = spawn_surface_server(&root);

    // A read-only serve holds a native read-only open, which excludes a write-capable command.
    assert_store_locked(
        support::marrow_bounded(
            &["run", "--entry", "app::seed", project],
            std::time::Duration::from_secs(15),
        ),
        "writer racing a read-only serve",
    );
}

/// A surface whose only read is a computed `read describe`, so its sole read route carries the
/// `read_only_context_digest` the computed-read operation tag folds in.
const COMPUTED_READ_SURFACE_SOURCE: &str = "module app\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
\x20\x20\x20\x20author: string\n\
store ^books(id: int): Book\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var book: Book\n\
\x20\x20\x20\x20book.title = \"Dune\"\n\
\x20\x20\x20\x20book.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
\n\
pub fn describe(id: int): string\n\
\x20\x20\x20\x20return (^books(id).title ?? \"\") + \"|\" + (^books(id).author ?? \"\")\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title, author\n\
\x20\x20\x20\x20read describe\n";

/// The `describe` computed-read route's operation tag from a `marrow check --format json` report.
fn describe_read_tag(project: &str) -> String {
    let output =
        support::marrow_bounded(&["check", "--format", "json", project], STORE_OP_DEADLINE);
    assert_eq!(
        output.status.code(),
        Some(0),
        "check --format json must exit 0: {output:?}",
    );
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("check json parses: {error}; stdout={output:?}"));
    route_by_alias(&report, "describe").operation_tag
}

/// A computed-read operation tag folds in the program's `read_only_context_digest`, which binds the
/// accepted catalog identity. A momentarily writer-locked store must not divert `check` onto a
/// divergent lock-only adoption: the tag and the `--locked` gate must be identical whether or not a
/// concurrent `serve --write` holds the redb writer lock, so the committed client a CI checkout
/// validates stays in step with the running server.
#[test]
fn computed_read_tag_is_stable_while_a_writer_holds_the_store_lock() {
    let root = temp_project("serve-write-computed-read-tag", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", COMPUTED_READ_SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let baseline_tag = describe_read_tag(project);
    let baseline_locked = marrow(&["check", "--locked", project]);
    assert_eq!(
        baseline_locked.status.code(),
        Some(0),
        "baseline check --locked must pass: {baseline_locked:?}",
    );

    let (_server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);

    let locked_tag = describe_read_tag(project);
    assert_eq!(
        locked_tag, baseline_tag,
        "the computed-read operation tag must be identical whether or not a writer holds the lock",
    );

    let locked_locked = support::marrow_bounded(&["check", "--locked", project], STORE_OP_DEADLINE);
    assert_eq!(
        locked_locked.status.code(),
        Some(0),
        "check --locked must still pass while a writer holds the lock: {locked_locked:?}",
    );
}

/// The dotted fault code of a refused command, read from the shared stderr fault line.
fn fault_code(output: &std::process::Output) -> String {
    let fault = support::last_fault(&output.stderr);
    let segments: Vec<&str> = fault.split(": ").collect();
    let (_, code) = support::find_code_segment(&segments);
    code.to_string()
}

const STORE_OP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
const REMOTE_AUTH_ENV: &str = "MARROW_TEST_SURFACE_TOKEN";
const REMOTE_AUTH_TOKEN: &str = "remote-token-123";
const REMOTE_AUTH_HEADER: &str = "Bearer remote-token-123";
const REMOTE_CURSOR_TOKEN_ENV: &str = "MARROW_TEST_SURFACE_CURSOR_TOKEN_KEY";
const REMOTE_CURSOR_TOKEN_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// An idle `--write` serve stopped by the documented foreground stop (SIGTERM) closes the
/// held store handle on the normal stack, so the next read-only inspection and write-capable
/// command both open the store cleanly with the committed record count unchanged.
#[test]
fn idle_write_serve_sigterm_closes_the_store_cleanly() {
    let root = temp_project("serve-write-sigterm-clean", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let cells_before = integrity_cell_count(project);

    let (server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);
    // Idle: no request handled. Stop with the documented foreground stop.
    server.stop_with_sigterm();

    let integrity = support::marrow_bounded(&["data", "integrity", project], STORE_OP_DEADLINE);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "data integrity must pass after a graceful serve stop: {integrity:?}",
    );

    // A write-capable run also opens the store without requiring manual recovery.
    let recovered =
        support::marrow_bounded(&["run", "--entry", "app::seed", project], STORE_OP_DEADLINE);
    assert_eq!(
        recovered.status.code(),
        Some(0),
        "write-capable run must open after a graceful serve stop: {recovered:?}",
    );
    assert_eq!(
        integrity_cell_count(project),
        cells_before,
        "the graceful stop must not change the committed cell count",
    );
    let doctor = support::marrow_bounded(&["doctor", project], STORE_OP_DEADLINE);
    assert_eq!(
        doctor.status.code(),
        Some(0),
        "doctor must report a healthy store after a graceful serve stop: {doctor:?}",
    );
}

/// After a prior idle write serve is stopped with SIGTERM, a fresh `serve --write` opens and starts
/// listening without requiring recovery. (`spawn_surface_server_with_args` fails the test if the
/// server exits before printing its listen line.)
#[test]
fn idle_write_serve_sigterm_allows_the_next_write_serve() {
    let root = temp_project("serve-write-sigterm-next-serve", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let (server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);
    server.stop_with_sigterm();

    // The next write serve starts listening over the cleanly closed store.
    let (server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);
    server.stop_with_sigterm();

    // A write run then leaves the store healthy for read-only inspection.
    let run = support::marrow_bounded(&["run", "--entry", "app::seed", project], STORE_OP_DEADLINE);
    assert_eq!(run.status.code(), Some(0), "post-serve run: {run:?}");
    let integrity = support::marrow_bounded(&["data", "integrity", project], STORE_OP_DEADLINE);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "data integrity must pass after a graceful write serve stop: {integrity:?}",
    );
}

/// A native `dataDir` occupied by a non-directory file is the same configuration fault `serve`
/// must report as `run`, `evolve apply`, and the read-only inspections: the precise
/// `config.data_dir` code with the remedy, never the generic `store.io` open failure the bare
/// redb open would leak. Both serve modes open the store, so both must guard the directory first.
/// Bounded and asserts the refused exit rather than spawning a listening server.
#[test]
fn serve_over_a_data_dir_occupied_by_a_file_reports_a_config_fault() {
    for mode_args in [&["serve"][..], &["serve", "--write"][..]] {
        let root = temp_project("serve-data-dir-occupied", |root| {
            write(root, "marrow.json", support::native_config());
            write(root, "src/app.mw", SURFACE_SOURCE);
        });
        // A regular file occupies the native `dataDir` the store would open under.
        std::fs::remove_dir_all(root.join(".data")).expect("clear seeded dataDir");
        write(&root, ".data", "not a directory");
        let project = root.to_str().expect("project path utf8");

        let mut args = mode_args.to_vec();
        args.extend_from_slice(&["--addr", "127.0.0.1:0", project]);
        let serve = support::marrow_bounded(&args, STORE_OP_DEADLINE);
        assert_eq!(
            serve.status.code(),
            Some(1),
            "{mode_args:?}: an occupied dataDir must refuse, not listen: {serve:?}",
        );
        assert_eq!(
            fault_code(&serve),
            "config.data_dir",
            "{mode_args:?}: an occupied dataDir is a config fault, not store.io: {serve:?}",
        );
        let stderr = String::from_utf8_lossy(&serve.stderr);
        assert!(
            stderr.contains("occupied by a non-directory file")
                && stderr.contains(".data")
                && stderr.contains("dataDir"),
            "{mode_args:?}: the fault must name the dataDir and its remedy: {stderr}",
        );
    }
}

/// A normal `serve` over a healthy project still reaches its listen line: the dataDir guard admits
/// a present directory and the store opens as before. `spawn_surface_server` fails the test if the
/// server exits before printing its listen line, so reaching it proves the guard is non-intrusive.
#[test]
fn serve_over_a_healthy_data_dir_still_listens() {
    let root = temp_project("serve-data-dir-healthy", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let (server, _addr) = spawn_surface_server(&root);
    server.stop_with_sigterm();
}

/// `serve --write` is a write-capable open, so it agrees with `run` and `evolve apply`: an absent
/// store body under a committed lock is the disposable-store case, so the open seeds an empty store
/// from the committed identity and listens rather than failing closed. `store.corruption` is
/// reserved for a PRESENT store that lost roots. `spawn_surface_server_with_args` fails the test if
/// the server exits before printing its listen line, so reaching the listen line proves the write
/// open seeded and admitted the absent store.
#[test]
fn write_serve_over_an_absent_store_seeds_and_listens() {
    let root = temp_project("serve-write-store-lost", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    // The store body is removed while the committed lock survives, modelling a fresh checkout.
    std::fs::remove_dir_all(root.join(".data")).expect("simulate a lost local store");

    let (server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);
    let stderr = server.stop_with_sigterm_capturing_stderr();
    assert!(
        stderr.contains("initialized an empty store from marrow.lock"),
        "the absent-store serve seed must be announced loudly at startup: {stderr}"
    );

    // The write serve closes cleanly on the documented foreground stop.
    let run = support::marrow_bounded(&["run", "--entry", "app::seed", project], STORE_OP_DEADLINE);
    assert_eq!(run.status.code(), Some(0), "post-serve write run: {run:?}");
    let integrity = support::marrow_bounded(&["data", "integrity", project], STORE_OP_DEADLINE);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "the store the absent-store seed materialized must verify clean: {integrity:?}",
    );
}

/// Default read-only `serve` is a read-only sibling of doctor and `data stats`: an absent store
/// body under a committed lock is the fresh-checkout shape, so the read-only open serves the empty
/// committed identity the lock determines and reaches its listen line, without ever writing the
/// store body. `store.corruption` is reserved for a PRESENT store that lost roots, and seeding the
/// body is reserved for the write paths.
#[test]
fn read_only_serve_over_an_absent_store_serves_the_empty_committed_identity() {
    let root = temp_project("serve-read-store-lost", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    // The store body is removed while the committed lock survives, modelling a fresh checkout.
    let data_dir = root.join(".data");
    std::fs::remove_dir_all(&data_dir).expect("simulate a fresh checkout");

    // Reaching the listen line proves the read-only open served the empty committed identity rather
    // than refusing the absent store; `spawn_surface_server` fails the test if it exits first.
    let (server, _addr) = spawn_surface_server(&root);
    server.stop_with_sigterm();

    assert!(
        !data_dir.exists(),
        "read-only serve must not write the store body on a fresh checkout",
    );
}

/// A healthy store the lock-root witness does not condemn opens write-capable and listens. A first
/// `serve --write` against a freshly seeded store passes the witness (its roots match the lock), and
/// `spawn_surface_server_with_args` fails the test if the server exits before printing its listen
/// line, so reaching the listen line proves the witness admitted the write open.
#[test]
fn first_write_serve_over_a_seeded_store_listens() {
    let root = temp_project("serve-write-first-run", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let (server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);
    server.stop_with_sigterm();

    // The write serve closes cleanly on the documented foreground stop.
    let run = support::marrow_bounded(&["run", "--entry", "app::seed", project], STORE_OP_DEADLINE);
    assert_eq!(run.status.code(), Some(0), "post-serve write run: {run:?}");
    let integrity = support::marrow_bounded(&["data", "integrity", project], STORE_OP_DEADLINE);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "a healthy store the witness admits must stay healthy: {integrity:?}",
    );
}

/// A read-only serve never opens the store write-capable, so SIGTERM leaves no recovery flag:
/// the store stays healthy for the next read and the next writer with no replay needed.
#[test]
fn read_only_serve_sigterm_leaves_the_store_healthy() {
    let root = temp_project("serve-read-sigterm-healthy", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let cells_before = integrity_cell_count(project);

    let (server, _addr) = spawn_surface_server(&root);
    server.stop_with_sigterm();

    let integrity = support::marrow_bounded(&["data", "integrity", project], STORE_OP_DEADLINE);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "a read-only serve stop must leave the store healthy: {integrity:?}",
    );
    assert_eq!(
        integrity_cell_count(project),
        cells_before,
        "a read-only serve stop must not change the committed cell count",
    );
}

/// A source whose seed writes enough records that the data btree spans interior pages, so a
/// flipped byte past the header lands in load-bearing committed data the structural digest
/// covers rather than slack.
const BULK_SEED_SOURCE: &str = "module app\n\
 \n\
 resource Note\n\
 \x20\x20\x20\x20required title: string\n\
 store ^notes(id: int): Note\n\
 \n\
pub fn seed()\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20for i in 1..=400\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20var n: Note\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20n.title = \"a note title long enough to span a cell\"\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^notes(i) = n\n";

/// The replay path must not bless a store with a genuinely corrupted committed body: after a
/// flipped data byte the integrity oracle rejects, a write-capable `run` and `data recover` must
/// still fail closed as `store.corruption` rather than auto-repairing and reporting success.
#[test]
fn replay_does_not_bless_a_corrupted_store() {
    let root = temp_project("serve-replay-not-bless-corrupt", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", BULK_SEED_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    // Flip bytes past the header until one lands in committed data the integrity oracle rejects
    // as corruption, then prove the write-capable replay path refuses it rather than blessing it.
    let store = support::redb_store_path(&root);
    let clean = std::fs::read(&store).expect("read store body");
    assert!(
        clean.len() > 8192,
        "seeded bulk store should span data pages"
    );
    let mut corrupted_at = None;
    for offset in (8192..clean.len()).step_by(64) {
        let mut bytes = clean.clone();
        bytes[offset] ^= 0xff;
        std::fs::write(&store, &bytes).expect("write corrupted store body");
        let integrity = support::marrow_bounded(&["data", "integrity", project], STORE_OP_DEADLINE);
        if integrity.status.code() == Some(1) && fault_code(&integrity) == "store.corruption" {
            corrupted_at = Some(offset);
            break;
        }
    }
    let offset = corrupted_at.expect("a flipped data byte must produce store.corruption");

    // The write-capable replay path must agree, never bless the corruption as a clean replay.
    let run = support::marrow_bounded(&["run", "--entry", "app::seed", project], STORE_OP_DEADLINE);
    assert_eq!(
        run.status.code(),
        Some(1),
        "offset {offset}: the write run replay must refuse a corrupted store: {run:?}",
    );
    let recover = support::marrow_bounded(&["data", "recover", project], STORE_OP_DEADLINE);
    assert_eq!(
        recover.status.code(),
        Some(1),
        "offset {offset}: recover must refuse a corrupted store: {recover:?}",
    );
    assert_eq!(
        fault_code(&recover),
        "store.corruption",
        "offset {offset}: recover must report store.corruption: {recover:?}",
    );
}

/// The committed cell count `data integrity` reports for a healthy store, parsed from its
/// `(N cells)` success line. Used to prove serve shutdown does not change committed data.
fn integrity_cell_count(project: &str) -> u64 {
    let integrity = support::marrow_bounded(&["data", "integrity", project], STORE_OP_DEADLINE);
    assert_eq!(
        integrity.status.code(),
        Some(0),
        "expected a healthy store for the cell-count probe: {integrity:?}",
    );
    let text = String::from_utf8_lossy(&integrity.stdout);
    text.rsplit_once('(')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("could not read cell count from integrity output: {text:?}"))
}

/// Assert a CLI command was refused because the serve process holds the cross-process store lock.
/// The dotted code is read from the shared stderr fault line, the CLI's single owner of "which
/// stderr line is the fault" across run, data, and serve.
fn assert_store_locked(output: std::process::Output, what: &str) {
    assert_eq!(output.status.code(), Some(1), "{what}: {output:?}");
    let fault = support::last_fault(&output.stderr);
    let segments: Vec<&str> = fault.split(": ").collect();
    let (_, code) = support::find_code_segment(&segments);
    assert_eq!(
        code, "store.locked",
        "{what} must be refused store.locked: {output:?}"
    );
}

struct SurfaceFixture {
    _root: support::TempProject,
    report: Value,
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[test]
fn help_advertises_top_level_serve() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains(
            "marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "marrow serve --remote --addr <addr> [--write] (--auth-token-env NAME | --auth-token-file PATH)"
        ),
        "{stdout}"
    );
    assert!(
        !stdout.contains("surface serve"),
        "root help should not advertise removed surface commands: {stdout}"
    );

    let serve_help = marrow(&["serve", "--help"]);
    assert_eq!(serve_help.status.code(), Some(0), "{serve_help:?}");
    let serve_stdout = String::from_utf8(serve_help.stdout).expect("serve stdout utf8");
    assert!(
        serve_stdout.contains(
            "marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>"
        ),
        "{serve_stdout}"
    );
    assert!(
        serve_stdout.contains("marrow serve --remote --addr <addr> [--write]"),
        "{serve_stdout}"
    );
    assert!(serve_stdout.contains("--write"), "{serve_stdout}");
    assert!(serve_stdout.contains("--watch"), "{serve_stdout}");
    assert!(serve_stdout.contains("--cors-origin"), "{serve_stdout}");
    assert!(
        serve_stdout.contains("--remote-cors-origin"),
        "{serve_stdout}"
    );
    assert!(serve_stdout.contains("--auth-token-env"), "{serve_stdout}");
    assert!(
        serve_stdout.contains("/surface/v1/{read|create|update|delete|action}/<operation-tag>"),
        "{serve_stdout}"
    );
}

#[test]
fn surface_serve_rejects_non_loopback_before_project_load() {
    let dir = support::temp_dir("surface-serve-non-loopback");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");

    let output = marrow(&["serve", "--addr", "0.0.0.0:0", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "usage failure should not write stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("loopback"), "{stderr}");
    assert!(
        !stderr.contains("parse."),
        "bind validation should fail before source loading: {stderr}"
    );
}

#[test]
fn surface_serve_rejects_non_loopback_cors_origin_before_project_load() {
    let dir = support::temp_dir("surface-serve-non-loopback-cors-origin");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");

    let output = marrow(&[
        "serve",
        "--cors-origin",
        "https://example.com",
        dir.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "usage failure should not write stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("loopback origin"), "{stderr}");
    assert!(
        !stderr.contains("parse."),
        "CORS origin validation should fail before source loading: {stderr}"
    );
}

#[test]
fn surface_serve_remote_cli_validation_fails_before_project_load() {
    let dir = support::temp_dir("surface-serve-remote-cli-validation");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");
    let project = dir.to_str().unwrap();

    let missing_addr = marrow(&[
        "serve",
        "--remote",
        "--auth-token-env",
        "MARROW_SURFACE_TOKEN",
        project,
    ]);
    assert_eq!(missing_addr.status.code(), Some(2), "{missing_addr:?}");
    let stderr = String::from_utf8(missing_addr.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--remote") && stderr.contains("--addr"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("parse."),
        "remote usage validation should fail before source loading: {stderr}"
    );

    let missing_auth = marrow(&["serve", "--remote", "--addr", "0.0.0.0:0", project]);
    assert_eq!(missing_auth.status.code(), Some(2), "{missing_auth:?}");
    let stderr = String::from_utf8(missing_auth.stderr).expect("stderr utf8");
    assert!(stderr.contains("auth"), "{stderr}");

    let duplicate_auth = marrow(&[
        "serve",
        "--remote",
        "--addr",
        "0.0.0.0:0",
        "--auth-token-env",
        "MARROW_SURFACE_TOKEN",
        "--auth-token-file",
        "token.txt",
        project,
    ]);
    assert_eq!(duplicate_auth.status.code(), Some(2), "{duplicate_auth:?}");
    let stderr = String::from_utf8(duplicate_auth.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("exactly one") && stderr.contains("auth"),
        "{stderr}"
    );

    let remote_watch = marrow(&[
        "serve",
        "--remote",
        "--addr",
        "0.0.0.0:0",
        "--auth-token-env",
        "MARROW_SURFACE_TOKEN",
        "--watch",
        project,
    ]);
    assert_eq!(remote_watch.status.code(), Some(2), "{remote_watch:?}");
    let stderr = String::from_utf8(remote_watch.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--watch") && stderr.contains("--remote"),
        "{stderr}"
    );

    let empty_token = marrow(&[
        "serve",
        "--remote",
        "--addr",
        "0.0.0.0:0",
        "--auth-token-env",
        "MARROW_EMPTY_SURFACE_TOKEN",
        project,
    ]);
    assert_eq!(empty_token.status.code(), Some(2), "{empty_token:?}");
    let stderr = String::from_utf8(empty_token.stderr).expect("stderr utf8");
    assert!(stderr.contains("empty"), "{stderr}");
}

#[test]
fn surface_serve_remote_token_file_validation_fails_before_project_load() {
    let dir = support::temp_dir("surface-serve-remote-token-file-validation");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");
    write(&dir, "token.txt", " leading-space\n");

    let output = marrow(&[
        "serve",
        "--remote",
        "--addr",
        "0.0.0.0:0",
        "--auth-token-file",
        dir.join("token.txt").to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("whitespace") && !stderr.contains("parse."),
        "token validation should fail before source loading: {stderr}"
    );
}

#[test]
fn surface_serve_remote_cors_validation_fails_before_project_load() {
    let dir = support::temp_dir("surface-serve-remote-cors-validation");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");
    let project = dir.to_str().unwrap();

    let without_remote = marrow(&[
        "serve",
        "--addr",
        "127.0.0.1:0",
        "--remote-cors-origin",
        "https://app.example.com",
        project,
    ]);
    assert_eq!(without_remote.status.code(), Some(2), "{without_remote:?}");
    let stderr = String::from_utf8(without_remote.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--remote-cors-origin") && stderr.contains("--remote"),
        "{stderr}"
    );

    for origin in [
        "*",
        "null",
        "https://app.example.com/path",
        "https://app.example.com\r\nX-Injected: yes",
        "https://user@app.example.com",
        "https://[::1",
        "https://app.example.com:70000",
        "https://",
    ] {
        let output = marrow(&[
            "serve",
            "--remote",
            "--addr",
            "0.0.0.0:0",
            "--auth-token-env",
            "MARROW_SURFACE_TOKEN",
            "--remote-cors-origin",
            origin,
            project,
        ]);
        assert_eq!(output.status.code(), Some(2), "{origin}: {output:?}");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(
            stderr.contains("origin") && !stderr.contains("parse."),
            "{origin}: {stderr}"
        );
    }
}

#[test]
fn surface_serve_cursor_token_cli_validation_fails_before_project_load() {
    let dir = support::temp_dir("surface-serve-cursor-token-cli-validation");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");
    let project = dir.to_str().unwrap();

    let without_remote = marrow(&[
        "serve",
        "--addr",
        "127.0.0.1:0",
        "--cursor-token-key-id",
        "kid-1",
        "--cursor-token-key-env",
        REMOTE_CURSOR_TOKEN_ENV,
        project,
    ]);
    assert_eq!(without_remote.status.code(), Some(2), "{without_remote:?}");
    let stderr = String::from_utf8(without_remote.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("cursor-token") && stderr.contains("--remote"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("parse."),
        "cursor token usage validation should fail before source loading: {stderr}"
    );

    let missing_key_id = marrow(&[
        "serve",
        "--remote",
        "--addr",
        "0.0.0.0:0",
        "--auth-token-env",
        REMOTE_AUTH_ENV,
        "--cursor-token-key-env",
        REMOTE_CURSOR_TOKEN_ENV,
        project,
    ]);
    assert_eq!(missing_key_id.status.code(), Some(2), "{missing_key_id:?}");
    let stderr = String::from_utf8(missing_key_id.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("cursor-token-key-id") && !stderr.contains("parse."),
        "{stderr}"
    );

    let missing_key_source = marrow(&[
        "serve",
        "--remote",
        "--addr",
        "0.0.0.0:0",
        "--auth-token-env",
        REMOTE_AUTH_ENV,
        "--cursor-token-key-id",
        "kid-1",
        project,
    ]);
    assert_eq!(
        missing_key_source.status.code(),
        Some(2),
        "{missing_key_source:?}"
    );
    let stderr = String::from_utf8(missing_key_source.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("exactly one") && stderr.contains("cursor token key source"),
        "{stderr}"
    );

    let duplicate_key_source = marrow(&[
        "serve",
        "--remote",
        "--addr",
        "0.0.0.0:0",
        "--auth-token-env",
        REMOTE_AUTH_ENV,
        "--cursor-token-key-id",
        "kid-1",
        "--cursor-token-key-env",
        REMOTE_CURSOR_TOKEN_ENV,
        "--cursor-token-key-file",
        "cursor.key",
        project,
    ]);
    assert_eq!(
        duplicate_key_source.status.code(),
        Some(2),
        "{duplicate_key_source:?}"
    );
    let stderr = String::from_utf8(duplicate_key_source.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("exactly one") && stderr.contains("cursor token key source"),
        "{stderr}"
    );

    let invalid_key = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args([
            "serve",
            "--remote",
            "--addr",
            "0.0.0.0:0",
            "--auth-token-env",
            REMOTE_AUTH_ENV,
            "--cursor-token-key-id",
            "kid-1",
            "--cursor-token-key-env",
            REMOTE_CURSOR_TOKEN_ENV,
            project,
        ])
        .env(REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN)
        .env(
            REMOTE_CURSOR_TOKEN_ENV,
            format!(" {REMOTE_CURSOR_TOKEN_KEY}"),
        )
        .output()
        .expect("run marrow serve");
    assert_eq!(invalid_key.status.code(), Some(2), "{invalid_key:?}");
    let stderr = String::from_utf8(invalid_key.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("whitespace") && !stderr.contains("parse."),
        "{stderr}"
    );
}

#[test]
fn surface_serve_executes_manifest_point_read_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-point-read");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());

    let response = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": point_route.operation_tag,
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id.clone(),
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.body["profile_version"], "surface.operation.v1");
    assert_eq!(response.body["operation_tag"], point_route.operation_tag);
    assert_eq!(response.body["result"]["kind"], "record");
    let record = &response.body["result"]["record"];
    assert_eq!(
        field_value(record, "title"),
        json!({ "kind": "string", "value": "Dune" })
    );
    assert_eq!(
        field_value(record, "author"),
        json!({ "kind": "string", "value": "Frank Herbert" })
    );
}

#[test]
fn surface_serve_cursor_token_remote_pages_with_opaque_cursor_strings() {
    let fixture = seeded_surface_fixture("surface-serve-cursor-token-roundtrip");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[
            (REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN),
            (REMOTE_CURSOR_TOKEN_ENV, REMOTE_CURSOR_TOKEN_KEY),
        ],
        &[
            "--remote",
            "--auth-token-env",
            REMOTE_AUTH_ENV,
            "--cursor-token-key-id",
            "kid-1",
            "--cursor-token-key-env",
            REMOTE_CURSOR_TOKEN_ENV,
        ],
    );

    let first_page = post_json(
        addr,
        &page_route.path,
        page_request(&page_route.operation_tag, None),
        &remote_headers(),
    );
    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    let token = first_page.body["result"]["page"]["next"]
        .as_str()
        .unwrap_or_else(|| panic!("token cursor string in {:#?}", first_page.body))
        .to_string();
    assert!(
        token.starts_with("mct1.kid-1."),
        "token must carry the key id without revealing cursor JSON: {token}"
    );

    let second_page = post_json(
        addr,
        &page_route.path,
        page_request(&page_route.operation_tag, Some(json!(token))),
        &remote_headers(),
    );
    assert_eq!(second_page.status, 200, "{:#?}", second_page.body);
    assert_eq!(
        field_value(&second_page.body["result"]["page"]["rows"][0], "title"),
        json!({ "kind": "string", "value": "Dune Messiah" })
    );
    assert_eq!(second_page.body["result"]["page"]["next"], Value::Null);
}

#[test]
fn surface_serve_cursor_token_remote_rejects_tamper_and_typed_cursor_objects() {
    let fixture = seeded_surface_fixture("surface-serve-cursor-token-rejects");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[
            (REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN),
            (REMOTE_CURSOR_TOKEN_ENV, REMOTE_CURSOR_TOKEN_KEY),
        ],
        &[
            "--remote",
            "--auth-token-env",
            REMOTE_AUTH_ENV,
            "--cursor-token-key-id",
            "kid-1",
            "--cursor-token-key-env",
            REMOTE_CURSOR_TOKEN_ENV,
        ],
    );
    let first_page = post_json(
        addr,
        &page_route.path,
        page_request(&page_route.operation_tag, None),
        &remote_headers(),
    );
    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    let token = first_page.body["result"]["page"]["next"]
        .as_str()
        .expect("token cursor string");
    let mut tampered = token.as_bytes().to_vec();
    let last = tampered.last_mut().expect("token has bytes");
    *last = if *last == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).expect("tampered token utf8");

    let tampered_response = post_json(
        addr,
        &page_route.path,
        page_request(&page_route.operation_tag, Some(json!(tampered))),
        &remote_headers(),
    );
    assert_eq!(
        tampered_response.status, 400,
        "{:#?}",
        tampered_response.body
    );
    assert_eq!(tampered_response.body["code"], "surface.cursor");

    let object_response = post_json(
        addr,
        &page_route.path,
        page_request(
            &page_route.operation_tag,
            Some(json!({ "operation_tag": page_route.operation_tag })),
        ),
        &remote_headers(),
    );
    assert_eq!(object_response.status, 400, "{:#?}", object_response.body);
    assert_eq!(object_response.body["code"], "surface.cursor");
}

#[test]
fn surface_serve_cursor_token_route_body_tag_mismatch_stays_abi_mismatch() {
    let fixture = seeded_surface_fixture("surface-serve-cursor-token-abi-mismatch");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[
            (REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN),
            (REMOTE_CURSOR_TOKEN_ENV, REMOTE_CURSOR_TOKEN_KEY),
        ],
        &[
            "--remote",
            "--auth-token-env",
            REMOTE_AUTH_ENV,
            "--cursor-token-key-id",
            "kid-1",
            "--cursor-token-key-env",
            REMOTE_CURSOR_TOKEN_ENV,
        ],
    );
    let first_page = post_json(
        addr,
        &page_route.path,
        page_request(&page_route.operation_tag, None),
        &remote_headers(),
    );
    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    let token = first_page.body["result"]["page"]["next"]
        .as_str()
        .expect("token cursor string");

    let response = post_json(
        addr,
        &page_route.path,
        page_request(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            Some(json!(token)),
        ),
        &remote_headers(),
    );
    assert_eq!(response.status, 404, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.abi_mismatch");
}

#[test]
fn surface_serve_cursor_token_wrong_body_kind_stays_request_mismatch() {
    let fixture = seeded_surface_fixture("surface-serve-cursor-token-kind-mismatch");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[
            (REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN),
            (REMOTE_CURSOR_TOKEN_ENV, REMOTE_CURSOR_TOKEN_KEY),
        ],
        &[
            "--remote",
            "--auth-token-env",
            REMOTE_AUTH_ENV,
            "--cursor-token-key-id",
            "kid-1",
            "--cursor-token-key-env",
            REMOTE_CURSOR_TOKEN_ENV,
        ],
    );

    let response = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "point_read",
                "request": {
                    "cursor": "not-a-token"
                }
            }
        }),
        &remote_headers(),
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");
}

#[test]
fn surface_serve_default_page_cursor_stays_typed_json() {
    let fixture = seeded_surface_fixture("surface-serve-default-typed-cursor");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let (_server, addr) = spawn_surface_server(fixture.root());

    let first_page = post_json(
        addr,
        &page_route.path,
        page_request(&page_route.operation_tag, None),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    assert!(
        first_page.body["result"]["page"]["next"].is_object(),
        "local/default serve must keep typed cursor objects: {:#?}",
        first_page.body
    );
}

#[test]
fn surface_serve_cors_origin_allows_exact_local_browser_origin() {
    let fixture = seeded_surface_fixture("surface-serve-cors-origin");
    let point_route = route_by_alias(&fixture.report, "get");
    let origin = "http://localhost:5173";
    let (_server, addr) =
        spawn_surface_server_with_args(fixture.root(), &["--cors-origin", origin]);

    let preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(preflight.status, 204, "{:#?}", preflight.body);
    assert_eq!(
        preflight.header("access-control-allow-origin"),
        Some(origin)
    );
    assert_eq!(
        preflight.header("access-control-allow-methods"),
        Some("POST, OPTIONS")
    );
    assert_eq!(
        preflight.header("access-control-allow-headers"),
        Some("Content-Type")
    );
    assert_eq!(preflight.header("vary"), Some("Origin"));

    let non_empty_preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 2\r\n\r\n{{}}",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        non_empty_preflight.status, 400,
        "{:#?}",
        non_empty_preflight.body
    );
    assert_eq!(non_empty_preflight.body["code"], "surface.request");
    assert_eq!(
        non_empty_preflight.header("access-control-allow-origin"),
        Some(origin)
    );

    let blocked_preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://example.com\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        blocked_preflight.status, 403,
        "{:#?}",
        blocked_preflight.body
    );
    assert_eq!(blocked_preflight.body["code"], "surface.request");
    assert_eq!(
        blocked_preflight.header("access-control-allow-origin"),
        None
    );

    let blocked_post = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[
            ("Content-Type", "application/json"),
            ("Origin", "http://example.com"),
        ],
    );
    assert_eq!(blocked_post.status, 403, "{:#?}", blocked_post.body);
    assert_eq!(blocked_post.body["code"], "surface.request");
    assert_eq!(blocked_post.header("access-control-allow-origin"), None);

    let response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json"), ("Origin", origin)],
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.header("access-control-allow-origin"), Some(origin));
    assert_eq!(response.header("vary"), Some("Origin"));
}

#[test]
fn surface_serve_cors_origin_echoes_configured_origin_for_casing_variant() {
    let fixture = seeded_surface_fixture("surface-serve-cors-origin-casing");
    let point_route = route_by_alias(&fixture.report, "get");
    let origin = "http://localhost:5173";
    let request_origin = "HTTP://LoCaLhOsT:5173";
    let (_server, addr) =
        spawn_surface_server_with_args(fixture.root(), &["--cors-origin", origin]);

    let preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {request_origin}\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(preflight.status, 204, "{:#?}", preflight.body);
    assert_eq!(
        preflight.header("access-control-allow-origin"),
        Some(origin)
    );

    let response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[
            ("Content-Type", "application/json"),
            ("Origin", request_origin),
        ],
    );
    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.header("access-control-allow-origin"), Some(origin));

    let blocked = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://example.com\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(blocked.status, 403, "{:#?}", blocked.body);
    assert_eq!(blocked.header("access-control-allow-origin"), None);
}

#[test]
fn surface_serve_remote_auth_rejects_before_body_read() {
    let fixture = seeded_surface_fixture("surface-serve-remote-auth-before-body");
    let point_route = route_by_alias(&fixture.report, "get");
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[(REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN)],
        &["--remote", "--auth-token-env", REMOTE_AUTH_ENV],
    );

    let missing = response_before_body(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
    );
    assert_eq!(missing.status, 401, "{:#?}", missing.body);
    assert_eq!(missing.body["code"], "surface.auth");

    let duplicate = response_before_body(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAuthorization: Bearer {REMOTE_AUTH_TOKEN}\r\nAuthorization: Bearer {REMOTE_AUTH_TOKEN}\r\nContent-Length: 4096\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
    );
    assert_eq!(duplicate.status, 401, "{:#?}", duplicate.body);
    assert_eq!(duplicate.body["code"], "surface.auth");

    let malformed = response_before_body(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAuthorization: bearer {REMOTE_AUTH_TOKEN}\r\nContent-Length: 4096\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
    );
    assert_eq!(malformed.status, 401, "{:#?}", malformed.body);
    assert_eq!(malformed.body["code"], "surface.auth");
}

#[test]
fn surface_serve_remote_auth_precedes_post_cors_validation() {
    let fixture = seeded_surface_fixture("surface-serve-remote-auth-before-cors");
    let point_route = route_by_alias(&fixture.report, "get");
    let origin = "https://app.example.com";
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[(REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN)],
        &[
            "--remote",
            "--auth-token-env",
            REMOTE_AUTH_ENV,
            "--remote-cors-origin",
            origin,
        ],
    );

    let disallowed_origin = response_before_body(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: https://other.example.com\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
    );
    assert_eq!(
        disallowed_origin.status, 401,
        "{:#?}",
        disallowed_origin.body
    );
    assert_eq!(disallowed_origin.body["code"], "surface.auth");
    assert_eq!(
        disallowed_origin.header("access-control-allow-origin"),
        None
    );

    let duplicate_origin = response_before_body(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
    );
    assert_eq!(duplicate_origin.status, 401, "{:#?}", duplicate_origin.body);
    assert_eq!(duplicate_origin.body["code"], "surface.auth");
    assert_eq!(duplicate_origin.header("access-control-allow-origin"), None);
}

#[test]
fn surface_serve_remote_authorized_post_succeeds() {
    let fixture = seeded_surface_fixture("surface-serve-remote-auth-success");
    let point_route = route_by_alias(&fixture.report, "get");
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[(REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN)],
        &["--remote", "--auth-token-env", REMOTE_AUTH_ENV],
    );
    let authorization = format!("Bearer {REMOTE_AUTH_TOKEN}");

    let response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[
            ("Content-Type", "application/json"),
            ("Authorization", authorization.as_str()),
        ],
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(
        field_value(&response.body["result"]["record"], "title"),
        json!({ "kind": "string", "value": "Dune" })
    );
}

#[test]
fn surface_serve_remote_read_only_denies_write_route_before_body_read() {
    let fixture = seeded_surface_fixture("surface-serve-remote-read-only-write-denial");
    let update_route = route_by_alias(&fixture.report, "update");
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[(REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN)],
        &["--remote", "--auth-token-env", REMOTE_AUTH_ENV],
    );

    let response = response_before_body(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAuthorization: Bearer {REMOTE_AUTH_TOKEN}\r\nContent-Length: 4096\r\n\r\n",
            update_route.path
        )
        .into_bytes(),
    );

    assert_eq!(response.status, 403, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.auth");
}

#[test]
fn surface_serve_remote_cors_preflight_is_unauthenticated_and_strict() {
    let fixture = seeded_surface_fixture("surface-serve-remote-cors");
    let point_route = route_by_alias(&fixture.report, "get");
    let origin = "https://app.example.com";
    let (_server, addr) = spawn_surface_server_with_env_args(
        fixture.root(),
        &[(REMOTE_AUTH_ENV, REMOTE_AUTH_TOKEN)],
        &[
            "--remote",
            "--auth-token-env",
            REMOTE_AUTH_ENV,
            "--remote-cors-origin",
            origin,
        ],
    );

    let preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization, content-type\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(preflight.status, 204, "{:#?}", preflight.body);
    assert_eq!(
        preflight.header("access-control-allow-origin"),
        Some(origin)
    );
    assert_eq!(
        preflight.header("access-control-allow-headers"),
        Some("Content-Type, Authorization")
    );
    assert_eq!(
        preflight.header("vary"),
        Some("Origin, Access-Control-Request-Method, Access-Control-Request-Headers")
    );

    let duplicate_origin = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: Content-Type, Authorization\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(duplicate_origin.status, 400, "{:#?}", duplicate_origin.body);
    assert_eq!(duplicate_origin.body["code"], "surface.request");
    assert_eq!(
        duplicate_origin.header("vary"),
        Some("Origin, Access-Control-Request-Method, Access-Control-Request-Headers")
    );

    let bad_headers = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: Content-Type\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(bad_headers.status, 400, "{:#?}", bad_headers.body);
    assert_eq!(bad_headers.body["code"], "surface.request");
    assert_eq!(
        bad_headers.header("access-control-allow-origin"),
        Some(origin)
    );
    assert_eq!(
        bad_headers.header("vary"),
        Some("Origin, Access-Control-Request-Method, Access-Control-Request-Headers")
    );
}

#[test]
fn surface_serve_fails_closed_on_request_shape_mismatches() {
    let fixture = seeded_surface_fixture("surface-serve-strict");
    let point_route = route_by_alias(&fixture.report, "get");
    let create_route = route_by_alias(&fixture.report, "create");
    let delete_route = route_by_alias(&fixture.report, "delete");
    let update_route = route_by_alias(&fixture.report, "update");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let create = create_descriptor(&fixture.report, &create_route.operation_tag);
    let title_catalog_id = create_field_catalog_id(create, "title");
    let author_catalog_id = create_field_catalog_id(create, "author");
    let (_server, addr) = spawn_surface_server(fixture.root());
    let good_body = json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": point_route.operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": "1" }]
                }
            }
        }
    });

    let missing_content_type = post_json(addr, &point_route.path, good_body.clone(), &[]);
    assert_eq!(
        missing_content_type.status, 415,
        "{:#?}",
        missing_content_type.body
    );
    assert_eq!(missing_content_type.body["code"], "surface.request");

    let tag_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "request": good_body["request"].clone()
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(tag_mismatch.status, 404, "{:#?}", tag_mismatch.body);
    assert_eq!(tag_mismatch.body["code"], "surface.abi_mismatch");

    let kind_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": point_route.operation_tag,
            "request": { "kind": "singleton_read" }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(kind_mismatch.status, 400, "{:#?}", kind_mismatch.body);
    assert_eq!(kind_mismatch.body["code"], "surface.request");

    let write_route = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": []
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(write_route.status, 404, "{:#?}", write_route.body);
    assert_eq!(write_route.body["code"], "surface.abi_mismatch");

    let create_route_response = post_json(
        addr,
        &create_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": create_route.operation_tag,
            "request": {
                "kind": "point_create",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "3" }]
                    },
                    "fields": [
                        {
                            "catalog_id": title_catalog_id,
                            "value": { "kind": "string", "value": "Children of Dune" }
                        },
                        {
                            "catalog_id": author_catalog_id,
                            "value": { "kind": "string", "value": "Frank Herbert" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(
        create_route_response.status, 404,
        "{:#?}",
        create_route_response.body
    );
    assert_eq!(create_route_response.body["code"], "surface.abi_mismatch");

    let delete_route_response = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": {
                "kind": "point_delete",
                "request": {
                    "identity": {
                        "store_catalog_id": delete_descriptor(&fixture.report, &delete_route.operation_tag)["store_catalog_id"],
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(
        delete_route_response.status, 404,
        "{:#?}",
        delete_route_response.body
    );
    assert_eq!(delete_route_response.body["code"], "surface.abi_mismatch");
}

#[cfg(unix)]
#[test]
fn surface_serve_write_closes_store_cleanly_on_sigterm() {
    surface_serve_signal_closes_store_cleanly("surface-serve-sigterm", "TERM", 15);
}

#[cfg(unix)]
#[test]
fn surface_serve_write_closes_store_cleanly_on_sigint() {
    surface_serve_signal_closes_store_cleanly("surface-serve-sigint", "INT", 2);
}

#[cfg(unix)]
fn surface_serve_signal_closes_store_cleanly(name: &str, signal: &str, signum: i32) {
    let fixture = seeded_surface_fixture(name);
    let create_route = route_by_alias(&fixture.report, "create");
    let create = create_descriptor(&fixture.report, &create_route.operation_tag);
    let store_catalog_id = create["store_catalog_id"]
        .as_str()
        .expect("create store catalog id")
        .to_string();
    let title_catalog_id = create_field_catalog_id(create, "title");
    let author_catalog_id = create_field_catalog_id(create, "author");
    let (server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let create_response = post_json(
        addr,
        &create_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": create_route.operation_tag,
            "request": {
                "kind": "point_create",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "3" }]
                    },
                    "fields": [
                        {
                            "catalog_id": title_catalog_id,
                            "value": { "kind": "string", "value": "Children of Dune" }
                        },
                        {
                            "catalog_id": author_catalog_id,
                            "value": { "kind": "string", "value": "Frank Herbert" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(create_response.status, 200, "{:#?}", create_response.body);

    let status = server.signal_and_wait(signal);
    assert_eq!(
        status.code(),
        Some(128 + signum),
        "serve should exit with the conventional signal status on SIG{signal}: {status:?}"
    );

    let dump = marrow_sub(
        "data",
        &["dump", "--format", "json", fixture.root().to_str().unwrap()],
    );
    assert_eq!(
        dump.status.code(),
        Some(0),
        "data dump must open the store cleanly after SIG{signal}: stdout={} stderr={}",
        String::from_utf8_lossy(&dump.stdout),
        String::from_utf8_lossy(&dump.stderr)
    );
    let dumped: Value = support::json(dump.stdout);
    assert!(
        dumped["code"].is_null(),
        "store must not surface an open fault after SIG{signal}: {dumped:#?}"
    );
    let titles: Vec<&str> = dumped["cells"]
        .as_array()
        .expect("dump cells")
        .iter()
        .filter(|cell| cell["path"] == "^books(3).title")
        .filter_map(|cell| cell["value_b64"].as_str())
        .collect();
    assert_eq!(
        titles,
        ["Q2hpbGRyZW4gb2YgRHVuZQ=="],
        "committed record must survive SIG{signal}: {dumped:#?}"
    );
}

#[cfg(unix)]
#[test]
fn surface_serve_shutdown_is_prompt_with_a_stalled_in_flight_request() {
    let fixture = seeded_surface_fixture("surface-serve-stalled-shutdown");
    let create_route = route_by_alias(&fixture.report, "create");
    let (server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let mut stalled = TcpStream::connect(addr).expect("connect stalled client");
    write!(
        stalled,
        "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n",
        create_route.path
    )
    .expect("write stalled request head");
    stalled.flush().expect("flush stalled head");
    std::thread::sleep(Duration::from_millis(200));

    let start = std::time::Instant::now();
    let status = server.signal_and_wait("TERM");
    let elapsed = start.elapsed();
    drop(stalled);

    assert_eq!(
        status.code(),
        Some(143),
        "stalled shutdown status: {status:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "signal must abandon the stalled request promptly, took {elapsed:?}"
    );

    let dump = marrow_sub(
        "data",
        &["dump", "--format", "json", fixture.root().to_str().unwrap()],
    );
    assert_eq!(
        dump.status.code(),
        Some(0),
        "store must open cleanly after a stalled-request shutdown: stderr={}",
        String::from_utf8_lossy(&dump.stderr)
    );
}

#[test]
fn surface_serve_reports_abi_mismatch_as_not_found() {
    let fixture = seeded_surface_fixture("surface-serve-abi-mismatch-404");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());

    // An operation tag the route no longer serves is the wrong-route/stale-client class: 404.
    let tag_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(tag_mismatch.status, 404, "{:#?}", tag_mismatch.body);
    assert_eq!(tag_mismatch.body["code"], "surface.abi_mismatch");

    // A stale profile version surfaces from the runtime executor as abi_mismatch: also 404.
    let profile_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v0",
            "operation_tag": point_route.operation_tag,
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(profile_mismatch.status, 404, "{:#?}", profile_mismatch.body);
    assert_eq!(profile_mismatch.body["code"], "surface.abi_mismatch");
}

#[test]
fn surface_serve_rejects_unknown_json_fields_without_mutation() {
    let fixture = seeded_surface_fixture("surface-serve-unknown-json");
    let point_route = route_by_alias(&fixture.report, "get");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let update_route = route_by_alias(&fixture.report, "update");
    let update = update_descriptor(&fixture.report, &update_route.operation_tag);
    let store_catalog_id = update["store_catalog_id"]
        .as_str()
        .expect("update store catalog id")
        .to_string();
    let author_catalog_id = update_field_catalog_id(update, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let mut top_level = point_read_request(&fixture.report, &point_route.operation_tag, 1);
    top_level["smuggled"] = json!(true);
    let response = post_json(
        addr,
        &point_route.path,
        top_level,
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");

    let mut request = point_read_request(&fixture.report, &point_route.operation_tag, 1);
    request["request"]["request"]["smuggled"] = json!("ignored-if-not-strict");
    let response = post_json(
        addr,
        &point_route.path,
        request,
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");

    let mut identity = point_read_request(&fixture.report, &point_route.operation_tag, 1);
    identity["request"]["request"]["identity"]["smuggled"] = json!("wrong-store");
    let response = post_json(
        addr,
        &point_route.path,
        identity,
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");

    let first_page = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 1
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    let mut cursor = first_page.body["result"]["page"]["next"].clone();
    assert!(
        cursor.is_object(),
        "expected page cursor: {:#?}",
        first_page.body
    );
    cursor["smuggled"] = json!("old-boundary");
    let cursor_response = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 10,
                    "cursor": cursor
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(cursor_response.status, 400, "{:#?}", cursor_response.body);
    assert_eq!(cursor_response.body["code"], "surface.request");

    let mut boundary_cursor = first_page.body["result"]["page"]["next"].clone();
    boundary_cursor["boundary"]["smuggled"] = json!("wrong-anchor");
    let boundary_response = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 10,
                    "cursor": boundary_cursor
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(
        boundary_response.status, 400,
        "{:#?}",
        boundary_response.body
    );
    assert_eq!(boundary_response.body["code"], "surface.request");

    let update_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": { "kind": "string", "value": "Ursula Le Guin" },
                        "smuggled": { "catalog_id": author_catalog_id, "value": { "kind": "string", "value": "wrong" } }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(update_response.status, 400, "{:#?}", update_response.body);
    assert_eq!(update_response.body["code"], "surface.request");

    let value_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": {
                            "kind": "string",
                            "value": "Ursula Le Guin",
                            "smuggled": "alternate"
                        }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(value_response.status, 400, "{:#?}", value_response.body);
    assert_eq!(value_response.body["code"], "surface.request");

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "author"),
        json!({ "kind": "string", "value": "Frank Herbert" })
    );
}

#[test]
fn surface_serve_non_post_method_names_post() {
    let fixture = seeded_surface_fixture("surface-serve-non-post");
    let point_route = route_by_alias(&fixture.report, "get");
    let (_server, addr) = spawn_surface_server(fixture.root());

    let response = raw_http(
        addr,
        format!("GET {} HTTP/1.1\r\nHost: {addr}\r\n\r\n", point_route.path).into_bytes(),
        &[],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");
    let message = response.body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("POST"),
        "a non-POST request must be told POST is required: {message}"
    );
}

#[test]
fn surface_serve_negative_page_limit_reports_limit_must_be_positive() {
    let fixture = seeded_surface_fixture("surface-serve-negative-limit");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let (_server, addr) = spawn_surface_server(fixture.root());

    let response = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": -1
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    let message = response.body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("greater than zero"),
        "a negative page limit must route through the limit-must-be-greater-than-zero branch: {message}"
    );
}

#[test]
fn surface_serve_write_mode_executes_sparse_update_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-write-update");
    let point_route = route_by_alias(&fixture.report, "get");
    let update_route = route_by_alias(&fixture.report, "update");
    let update = update_descriptor(&fixture.report, &update_route.operation_tag);
    let store_catalog_id = update["store_catalog_id"]
        .as_str()
        .expect("update store catalog id")
        .to_string();
    let author_catalog_id = update_field_catalog_id(update, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let kind_mismatch = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(kind_mismatch.status, 400, "{:#?}", kind_mismatch.body);
    assert_eq!(kind_mismatch.body["code"], "surface.request");

    let update_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": { "kind": "string", "value": "Brian Herbert" }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(update_response.status, 200, "{:#?}", update_response.body);
    assert_eq!(update_response.body["result"]["kind"], "updated");

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "author"),
        json!({ "kind": "string", "value": "Brian Herbert" })
    );
}

#[test]
fn surface_serve_write_mode_executes_create_and_delete_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-write-create-delete");
    let point_route = route_by_alias(&fixture.report, "get");
    let create_route = route_by_alias(&fixture.report, "create");
    let delete_route = route_by_alias(&fixture.report, "delete");
    let create = create_descriptor(&fixture.report, &create_route.operation_tag);
    let delete = delete_descriptor(&fixture.report, &delete_route.operation_tag);
    let store_catalog_id = create["store_catalog_id"]
        .as_str()
        .expect("create store catalog id")
        .to_string();
    assert_eq!(
        delete["store_catalog_id"]
            .as_str()
            .expect("delete store catalog id"),
        store_catalog_id
    );
    let title_catalog_id = create_field_catalog_id(create, "title");
    let author_catalog_id = create_field_catalog_id(create, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let create_response = post_json(
        addr,
        &create_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": create_route.operation_tag,
            "request": {
                "kind": "point_create",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "3" }]
                    },
                    "fields": [
                        {
                            "catalog_id": title_catalog_id,
                            "value": { "kind": "string", "value": "Children of Dune" }
                        },
                        {
                            "catalog_id": author_catalog_id,
                            "value": { "kind": "string", "value": "Frank Herbert" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(create_response.status, 200, "{:#?}", create_response.body);
    assert_eq!(create_response.body["result"]["kind"], "created");
    assert_eq!(
        field_value(&create_response.body["result"]["record"], "title"),
        json!({ "kind": "string", "value": "Children of Dune" })
    );

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 3),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "author"),
        json!({ "kind": "string", "value": "Frank Herbert" })
    );

    let delete_response = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": {
                "kind": "point_delete",
                "request": {
                    "identity": {
                        "store_catalog_id": delete["store_catalog_id"],
                        "keys": [{ "kind": "int", "value": "3" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(delete_response.status, 200, "{:#?}", delete_response.body);
    assert_eq!(delete_response.body["result"]["kind"], "deleted");

    let absent_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 3),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(absent_response.status, 404, "{:#?}", absent_response.body);
    assert_eq!(absent_response.body["code"], "surface.absent");
}

#[test]
fn surface_serve_rejects_garbage_singleton_bodies_without_mutation() {
    let fixture = seeded_singleton_fixture("surface-serve-singleton-strict");
    let read_route = route_by_alias(&fixture.report, "get");
    let delete_route = route_by_alias(&fixture.report, "delete");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    // The empty closed object is the valid singleton-read request body.
    let valid_read = post_json(
        addr,
        &read_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": read_route.operation_tag,
            "request": { "kind": "singleton_read", "request": {} }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(valid_read.status, 200, "{:#?}", valid_read.body);
    assert_eq!(
        field_value(&valid_read.body["result"]["record"], "theme"),
        json!({ "kind": "string", "value": "dark" })
    );

    let garbage_bodies = [
        json!({ "kind": "singleton_read", "request": { "unexpected": true } }),
        json!({ "kind": "singleton_read", "request": "garbage" }),
        json!({ "kind": "singleton_read", "request": [] }),
        json!({ "kind": "singleton_read" }),
    ];
    for body in garbage_bodies {
        let response = post_json(
            addr,
            &read_route.path,
            json!({
                "profile_version": "surface.operation.v1",
                "operation_tag": read_route.operation_tag,
                "request": body,
            }),
            &[("Content-Type", "application/json")],
        );
        assert_eq!(response.status, 400, "{body:#?} -> {:#?}", response.body);
        assert_eq!(response.body["code"], "surface.request", "{body:#?}");
    }

    // A garbage-body singleton delete must be rejected before it can delete.
    let garbage_delete = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": { "kind": "singleton_delete", "request": { "unexpected": true } }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(garbage_delete.status, 400, "{:#?}", garbage_delete.body);
    assert_eq!(garbage_delete.body["code"], "surface.request");

    let still_present = post_json(
        addr,
        &read_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": read_route.operation_tag,
            "request": { "kind": "singleton_read", "request": {} }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(still_present.status, 200, "{:#?}", still_present.body);
    assert_eq!(
        field_value(&still_present.body["result"]["record"], "theme"),
        json!({ "kind": "string", "value": "dark" })
    );

    // The valid empty delete body removes the singleton.
    let valid_delete = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": { "kind": "singleton_delete", "request": {} }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(valid_delete.status, 200, "{:#?}", valid_delete.body);
    assert_eq!(valid_delete.body["result"]["kind"], "deleted");
}

#[test]
fn surface_serve_write_mode_kill_leaves_a_recoverable_store() {
    let fixture = seeded_surface_fixture("surface-serve-write-idle-kill");
    let create_route = route_by_alias(&fixture.report, "create");
    let create = create_descriptor(&fixture.report, &create_route.operation_tag);
    let store_catalog_id = create["store_catalog_id"]
        .as_str()
        .expect("create store catalog id")
        .to_string();
    let title_catalog_id = create_field_catalog_id(create, "title");
    let author_catalog_id = create_field_catalog_id(create, "author");
    let (server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let create_response = post_json(
        addr,
        &create_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": create_route.operation_tag,
            "request": {
                "kind": "point_create",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "3" }]
                    },
                    "fields": [
                        {
                            "catalog_id": title_catalog_id,
                            "value": { "kind": "string", "value": "Children of Dune" }
                        },
                        {
                            "catalog_id": author_catalog_id,
                            "value": { "kind": "string", "value": "Frank Herbert" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(create_response.status, 200, "{:#?}", create_response.body);

    // A write serve holds the native writer lock for its whole lifetime, so a SIGKILL skips redb's
    // clean-shutdown marker: the store is left needing a write-capable recovery, not torn. The
    // committed create must survive the replay.
    drop(server);

    let root = fixture.root().to_str().unwrap();
    let locked_dump = marrow_sub("data", &["dump", "--format", "json", root]);
    assert_eq!(
        locked_dump.status.code(),
        Some(1),
        "a read-only open after a killed write serve must report recovery, not open: {locked_dump:?}"
    );
    assert_eq!(
        support::json(locked_dump.stdout)["code"],
        json!("store.recovery_required"),
        "a killed write serve must leave a recoverable store"
    );

    let recover = marrow_sub("data", &["recover", root]);
    assert_eq!(recover.status.code(), Some(0), "data recover: {recover:?}");

    let dump = marrow_sub("data", &["dump", "--format", "json", root]);
    assert_eq!(
        dump.status.code(),
        Some(0),
        "data dump must open the store cleanly after recovery: stdout={} stderr={}",
        String::from_utf8_lossy(&dump.stdout),
        String::from_utf8_lossy(&dump.stderr)
    );
    let dumped: Value = support::json(dump.stdout);
    let titles: Vec<&str> = dumped["cells"]
        .as_array()
        .expect("dump cells")
        .iter()
        .filter(|cell| cell["path"] == "^books(3).title")
        .filter_map(|cell| cell["value_b64"].as_str())
        .collect();
    assert_eq!(
        titles,
        ["Q2hpbGRyZW4gb2YgRHVuZQ=="],
        "committed record must survive idle serve shutdown: {dumped:#?}"
    );
}

#[test]
fn surface_serve_write_mode_executes_action_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-write-action");
    let point_route = route_by_alias(&fixture.report, "get");
    let action_route = route_by_alias(&fixture.report, "retitle");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let action_response = post_json(
        addr,
        &action_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": action_route.operation_tag,
            "request": {
                "kind": "action",
                "request": {
                    "arguments": [
                        {
                            "name": "id",
                            "value": { "kind": "int", "value": "1" }
                        },
                        {
                            "name": "title",
                            "value": { "kind": "string", "value": "Dune HTTP" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(action_response.status, 200, "{:#?}", action_response.body);
    assert_eq!(action_response.body["result"]["kind"], "action");
    assert_eq!(action_response.body["result"]["result"]["output"], "");
    assert_eq!(
        action_response.body["result"]["result"]["value"],
        json!({ "kind": "string", "value": "Dune HTTP" })
    );

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "title"),
        json!({ "kind": "string", "value": "Dune HTTP" })
    );
}

#[test]
fn surface_serve_write_mode_executes_startup_source_snapshot() {
    let fixture = seeded_surface_fixture("surface-serve-write-startup-snapshot");
    let point_route = route_by_alias(&fixture.report, "get");
    let action_route = route_by_alias(&fixture.report, "retitle");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);
    let edited_source = SURFACE_SOURCE.replace(
        "        ^books(id).title = title\n    return title\n",
        "        ^books(id).title = \"Edited Source\"\n    return \"Edited Source\"\n",
    );
    assert_ne!(edited_source, SURFACE_SOURCE);
    write(fixture.root(), "src/app.mw", &edited_source);

    let action_response = post_json(
        addr,
        &action_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": action_route.operation_tag,
            "request": {
                "kind": "action",
                "request": {
                    "arguments": [
                        {
                            "name": "id",
                            "value": { "kind": "int", "value": "1" }
                        },
                        {
                            "name": "title",
                            "value": { "kind": "string", "value": "Dune Startup" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(action_response.status, 200, "{:#?}", action_response.body);
    assert_eq!(
        action_response.body["result"]["result"]["value"],
        json!({ "kind": "string", "value": "Dune Startup" })
    );

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "title"),
        json!({ "kind": "string", "value": "Dune Startup" })
    );
}

#[test]
fn surface_serve_write_mode_reports_stale_cursor_as_conflict() {
    let fixture = seeded_surface_fixture("surface-serve-write-stale-cursor");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let update_route = route_by_alias(&fixture.report, "update");
    let update = update_descriptor(&fixture.report, &update_route.operation_tag);
    let store_catalog_id = update["store_catalog_id"]
        .as_str()
        .expect("update store catalog id")
        .to_string();
    let author_catalog_id = update_field_catalog_id(update, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let first_page = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 1
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    let cursor = first_page.body["result"]["page"]["next"].clone();
    assert!(
        cursor.is_object(),
        "first page must return a cursor: {:#?}",
        first_page.body
    );

    let update_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": { "kind": "string", "value": "Brian Herbert" }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(update_response.status, 200, "{:#?}", update_response.body);

    let stale_page = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 10,
                    "cursor": cursor
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(stale_page.status, 409, "{:#?}", stale_page.body);
    assert_eq!(stale_page.body["code"], "surface.stale_cursor");
}

#[test]
fn surface_serve_rejects_smuggled_or_unbounded_http_shapes() {
    let fixture = seeded_surface_fixture("surface-serve-http-shapes");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());
    let body = serde_json::to_vec(&json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": point_route.operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": "1" }]
                }
            }
        }
    }))
    .expect("request json");

    let duplicate_length = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(duplicate_length.status, 400, "{:#?}", duplicate_length.body);
    assert_eq!(duplicate_length.body["code"], "surface.request");

    let duplicate_content_type = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        duplicate_content_type.status, 400,
        "{:#?}",
        duplicate_content_type.body
    );
    assert_eq!(duplicate_content_type.body["code"], "surface.request");

    let transfer_encoding = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        transfer_encoding.status, 400,
        "{:#?}",
        transfer_encoding.body
    );
    assert_eq!(transfer_encoding.body["code"], "surface.request");

    let oversized_header = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nX-Fill: {}\r\nContent-Length: 0\r\n\r\n",
            point_route.path,
            "a".repeat(16 * 1024)
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(oversized_header.status, 431, "{:#?}", oversized_header.body);
    assert_eq!(oversized_header.body["code"], "surface.limit");

    let mut pipelined = body.clone();
    pipelined.extend_from_slice(b"GET /surface/v1/read/unused HTTP/1.1\r\n\r\n");
    let pipelined = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            point_route.path,
            body.len()
        )
        .into_bytes(),
        &pipelined,
    );
    assert_eq!(pipelined.status, 400, "{:#?}", pipelined.body);
    assert_eq!(pipelined.body["code"], "surface.request");

    let wrong_method = raw_http(
        addr,
        format!(
            "GET {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(wrong_method.status, 405, "{:#?}", wrong_method.body);
    assert_eq!(wrong_method.body["code"], "surface.request");
}

#[test]
fn surface_serve_processes_at_most_one_paced_request_per_connection() {
    let fixture = seeded_surface_fixture("surface-serve-paced-pipeline");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());
    let body = json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": point_route.operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": "1" }]
                }
            }
        }
    });
    let response = paced_pipeline(
        addr,
        &point_route.path,
        body,
        b"POST /surface/v1/read/unused HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.body["result"]["kind"], "record");
}

impl SurfaceFixture {
    fn root(&self) -> &Path {
        &self._root
    }
}

fn seeded_surface_fixture(name: &str) -> SurfaceFixture {
    let root = temp_project(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let seed = marrow_sub("run", &["--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let checked = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);
    assert_eq!(checked.status.code(), Some(0), "check: {checked:?}");
    SurfaceFixture {
        _root: root,
        report: support::json(checked.stdout),
    }
}

fn seeded_singleton_fixture(name: &str) -> SurfaceFixture {
    let root = temp_project(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SINGLETON_SURFACE_SOURCE);
    });
    let seed = marrow_sub("run", &["--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let checked = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);
    assert_eq!(checked.status.code(), Some(0), "check: {checked:?}");
    SurfaceFixture {
        _root: root,
        report: support::json(checked.stdout),
    }
}

fn point_read_request(report: &Value, operation_tag: &str, id: i64) -> Value {
    let store_catalog_id = read_descriptor(report, operation_tag)["store_catalog_id"]
        .as_str()
        .expect("point read store catalog id")
        .to_string();
    json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": id.to_string() }]
                }
            }
        }
    })
}

fn page_request(operation_tag: &str, cursor: Option<Value>) -> Value {
    let mut request = json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": operation_tag,
        "request": {
            "kind": "page",
            "request": {
                "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                "limit": 1
            }
        }
    });
    if let Some(cursor) = cursor {
        request["request"]["request"]["cursor"] = cursor;
    }
    request
}

fn remote_headers() -> [(&'static str, &'static str); 2] {
    [
        ("Content-Type", "application/json"),
        ("Authorization", REMOTE_AUTH_HEADER),
    ]
}

fn post_json(addr: SocketAddr, path: &str, body: Value, headers: &[(&str, &str)]) -> HttpResponse {
    let body = serde_json::to_vec(&body).expect("request json");
    let mut stream = TcpStream::connect(addr).expect("connect surface server");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\n",
        body.len()
    )
    .expect("write request line");
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").expect("write request header");
    }
    stream.write_all(b"\r\n").expect("finish headers");
    stream.write_all(&body).expect("write body");
    stream.flush().expect("flush request");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("finish request");

    parse_response(&read_response(stream))
}

fn raw_http(addr: SocketAddr, mut head: Vec<u8>, body: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).expect("connect surface server");
    head.extend_from_slice(body);
    stream.write_all(&head).expect("write raw request");
    stream.flush().expect("flush raw request");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("finish raw request");
    parse_response(&read_response(stream))
}

fn response_before_body(addr: SocketAddr, head: Vec<u8>) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).expect("connect surface server");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream.write_all(&head).expect("write request head");
    stream.flush().expect("flush request head");

    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            panic!(
                "surface server did not respond before the request body was sent: {}",
                String::from_utf8_lossy(&raw)
            );
        }
        Err(error) if error.kind() == ErrorKind::ConnectionReset && !raw.is_empty() => {}
        Err(error) => panic!("read early response: {error}"),
    }
    parse_response(&raw)
}

fn paced_pipeline(addr: SocketAddr, path: &str, body: Value, delayed_extra: &[u8]) -> HttpResponse {
    let body = serde_json::to_vec(&body).expect("request json");
    let mut stream = TcpStream::connect(addr).expect("connect surface server");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .expect("write paced request headers");
    stream.write_all(&body).expect("write paced body");
    stream.flush().expect("flush paced request");
    std::thread::sleep(Duration::from_millis(25));
    let _ = stream.write_all(delayed_extra);
    let _ = stream.flush();
    let raw = read_response(stream);
    let response_count = String::from_utf8_lossy(&raw)
        .match_indices("HTTP/1.1 ")
        .count();
    assert_eq!(
        response_count,
        1,
        "surface server must emit one response per connection: {}",
        String::from_utf8_lossy(&raw)
    );
    parse_response(&raw)
}

fn read_response(mut stream: TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::ConnectionReset && !raw.is_empty() => {}
        Err(error) => panic!("read response: {error}"),
    }
    raw
}

fn parse_response(raw: &[u8]) -> HttpResponse {
    let text = String::from_utf8(raw.to_vec()).expect("response utf8");
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("response missing header terminator: {text:?}"));
    let status = head
        .lines()
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.to_string(), value.trim().to_string()))
        })
        .collect();
    HttpResponse {
        status,
        headers,
        body: if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body).expect("response json body")
        },
    }
}

fn field_value(record: &Value, label: &str) -> Value {
    record["fields"]
        .as_array()
        .expect("record fields")
        .iter()
        .find(|field| field["render_label"] == label)
        .and_then(|field| field["value"].as_object().map(|_| field["value"].clone()))
        .unwrap_or_else(|| panic!("field {label} in {record:#?}"))
}

/// A 200-record `surface Library from ^books` store, large enough that its data btree spans
/// interior pages so a single-byte flip can silently drop or rewrite one cell.
fn bulk_book_surface_source(records: u32) -> String {
    format!(
        "module app\n\
         \n\
         resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20required author: string\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20for i in 1..={records}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20b.title = \"a book title long enough to span a cell\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20b.author = \"an author name\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^books(i) = b\n\
         \n\
         surface Library from ^books\n\
         \x20\x20\x20\x20fields title, author\n"
    )
}

/// `serve` performs a store read, so its store-open must run the same data-family structural-digest
/// cross-check the inspection and runtime families run: a present btree-corrupt store fails closed
/// as `store.corruption` at open. A regression that skipped the check would admit the store and
/// stream a truncated prefix over HTTP until a page reached the corrupt cell. The sweep flips one
/// byte at a time, and at every offset `data integrity` flags `store.corruption`, both the
/// read-only and the `--write` serve open must exit `store.corruption` rather than ever printing a
/// listen line. The bound turns a regression that lets serve listen into a clear failure.
#[test]
fn serve_over_a_btree_corrupt_store_fails_closed_rather_than_listen() {
    const SEEDED: u32 = 200;
    let project = temp_project_uncommitted("cli-serve-corrupt-store", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", &bulk_book_surface_source(SEEDED));
    });
    let dir = project.to_str().expect("dir utf8").to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0),
        "seed the bulk book surface store",
    );
    let store = redb_store_path(&project);
    let clean = std::fs::read(&store).expect("read seeded store body");

    let mut covered = false;
    for offset in (8192..clean.len()).step_by(256) {
        let mut corrupt = clean.clone();
        corrupt[offset] ^= 0xff;
        std::fs::write(&store, &corrupt).expect("write corrupted store body");

        let integrity = marrow_bounded(&["data", "integrity", &dir], STORE_OP_DEADLINE);
        if integrity.status.code() == Some(0) || fault_code(&integrity) != "store.corruption" {
            continue;
        }
        covered = true;

        for extra in [&[][..], &["--write"][..]] {
            let mut args = vec!["serve", "--addr", "127.0.0.1:0"];
            args.extend_from_slice(extra);
            args.push(&dir);
            let serve = marrow_bounded(&args, STORE_OP_DEADLINE);
            assert!(
                !String::from_utf8_lossy(&serve.stdout).contains("serve listening"),
                "offset {offset}: `marrow {}` listened over a btree-corrupt store: {serve:?}",
                args.join(" "),
            );
            assert_eq!(
                serve.status.code(),
                Some(1),
                "offset {offset}: `marrow {}` admitted a btree-corrupt store instead of failing \
                 closed: {serve:?}",
                args.join(" "),
            );
            assert_eq!(
                fault_code(&serve),
                "store.corruption",
                "offset {offset}: a btree-corrupt store must fail `marrow {}` closed as \
                 store.corruption: {serve:?}",
                args.join(" "),
            );
        }
    }
    assert!(
        covered,
        "the sweep must reach at least one offset `data integrity` flags as store.corruption",
    );
}

/// A healthy 200-record `surface ... from ^books` store still serves: serve must reach its listen
/// line. The digest cross-check at open guards against corruption, not a clean store, and
/// `spawn_surface_server` fails the test if serve exits before printing its listen line.
#[test]
fn serve_over_a_healthy_surface_store_still_listens() {
    const SEEDED: u32 = 200;
    let root = temp_project("cli-serve-healthy-store", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", &bulk_book_surface_source(SEEDED));
    });
    let project = root.to_str().expect("project path utf8");
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", project])
            .status
            .code(),
        Some(0),
        "seed the bulk book surface store",
    );

    let (server, _addr) = spawn_surface_server(&root);
    server.stop_with_sigterm();
}
