use crate::support;
use crate::support_surface::{
    route_by_alias, spawn_surface_server, spawn_surface_server_with_args,
};

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use support::{marrow, temp_project, temp_project_uncommitted, write};

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
\x20\x20\x20\x20update title, author\n\
\x20\x20\x20\x20delete\n\
\x20\x20\x20\x20collection ^books.byAuthor as byAuthor\n\
\x20\x20\x20\x20action retitle\n\
\x20\x20\x20\x20read describe\n";

/// A native-store config that declares a client output path so run/serve/evolve
/// regenerate the TypeScript client write-if-changed.
fn native_config_with_client() -> String {
    r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#
        .to_string()
}

#[test]
fn run_writes_declared_client_then_skips_unchanged() {
    let root = temp_project_uncommitted("run-writes-client", |root| {
        write(root, "marrow.json", &native_config_with_client());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let out = root.join("generated/marrow.ts");
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let first = std::fs::read_to_string(&out).expect("client written by run");
    assert!(first.contains("export function createClient"), "{first}");

    // A non-surface edit (private helper fn) must not change the digest header.
    let mut src = CLIENT_SURFACE_SOURCE.to_string();
    src.push_str("\nfn helperOnly(): int\n    return 7\n");
    write(&root, "src/app.mw", &src);
    let again = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(again.status.code(), Some(0), "rerun: {again:?}");
    let second = std::fs::read_to_string(&out).expect("client still present");
    assert_eq!(first, second, "non-surface edit must not churn the client");

    // A surface change (a new read alias over an existing fn) rewrites the file.
    let changed = src.replace(
        "    read describe\n",
        "    read describe\n    read describe as summary\n",
    );
    write(&root, "src/app.mw", &changed);
    let third_run = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(third_run.status.code(), Some(0), "third: {third_run:?}");
    let third = std::fs::read_to_string(&out).expect("client rewritten");
    assert_ne!(first, third, "a surface change must rewrite the client");
}

#[test]
fn run_warns_when_client_set_without_surface() {
    let root = temp_project_uncommitted("run-client-no-surface", |root| {
        write(root, "marrow.json", &native_config_with_client());
        write(root, "src/app.mw", support::counter_source()); // no surface block
    });
    let run = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    let stderr = String::from_utf8(run.stderr).unwrap();
    assert!(
        stderr.contains("client"),
        "expected a surfaceless-client warning: {stderr}"
    );
    assert!(
        !root.join("generated/marrow.ts").exists(),
        "no client without a surface"
    );
}

#[test]
fn dry_run_does_not_write_declared_client() {
    let root = temp_project_uncommitted("run-dry-no-client", |root| {
        write(root, "marrow.json", &native_config_with_client());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&[
        "run",
        "--entry",
        "app::seed",
        "--dry-run",
        root.to_str().unwrap(),
    ]);
    assert_eq!(seed.status.code(), Some(0), "dry seed: {seed:?}");
    assert!(
        !root.join("generated/marrow.ts").exists(),
        "a dry run must not write the declared client"
    );
}

#[test]
fn client_typescript_uses_lock_projection_when_store_is_absent() {
    let root = temp_project("surface-client-typescript", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let store_path = root.join(".data/marrow.redb");
    assert!(store_path.exists(), "fixture should have seeded a store");
    fs::remove_file(&store_path).expect("remove store file");

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("export function createClient"), "{stdout}");
    assert!(!stdout.contains("createMarrowSurfaceClient"), "{stdout}");
    assert!(
        stdout.contains("// Generated by marrow — do not edit."),
        "{stdout}"
    );
    assert!(
        stdout.contains("// marrow-surface-digest: sha256:"),
        "{stdout}"
    );
    // The client flattens to the surface name and exposes typed brand constructors and a typed
    // error class rather than the old module-keyed, untyped transport shape.
    assert!(stdout.contains("Books: {"), "{stdout}");
    assert!(stdout.contains("export function booksId("), "{stdout}");
    assert!(
        stdout.contains("export class MarrowSurfaceError"),
        "{stdout}"
    );
    assert!(stdout.contains("export type SurfaceErrorCode"), "{stdout}");
    assert!(stdout.contains("export function invokeRaw"), "{stdout}");
    assert!(stdout.contains("/surface/v1/create/"), "{stdout}");
    assert!(stdout.contains("/surface/v1/delete/"), "{stdout}");
    assert!(stdout.contains("Number.isSafeInteger"), "{stdout}");
    assert!(
        !store_path.exists(),
        "client generation must not recreate the native store"
    );
}

/// A surfaced store `^a` whose record projects an identity field referencing `^b`, a store with no
/// surface of its own. The reference brand must read from the target store's source name (`BId`),
/// never a catalog-id-derived `Ref_cat_...` symbol — the write/construct side of the relation must
/// stay as hash-free as the read side already is.
const RELATION_ID_SURFACE_SOURCE: &str = "module app\n\
\n\
resource Other\n\
\x20\x20\x20\x20required label: string\n\
store ^b(id: int): Other\n\
\n\
resource Thing\n\
\x20\x20\x20\x20required name: string\n\
\x20\x20\x20\x20required link: Id(^b)\n\
store ^a(id: int): Thing\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var other: Other\n\
\x20\x20\x20\x20other.label = \"target\"\n\
\x20\x20\x20\x20var thing: Thing\n\
\x20\x20\x20\x20thing.name = \"source\"\n\
\x20\x20\x20\x20thing.link = Id(^b, 1)\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^b(1) = other\n\
\x20\x20\x20\x20\x20\x20\x20\x20^a(1) = thing\n\
\n\
surface A from ^a\n\
\x20\x20\x20\x20fields name, link\n\
\x20\x20\x20\x20create name, link\n";

#[test]
fn client_typescript_relation_id_brand_uses_target_store_name() {
    let root = temp_project("surface-client-relation-id", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", RELATION_ID_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    // The reference to the surface-less `^b` store brands and constructs from its source name.
    assert!(stdout.contains("export type BId"), "{stdout}");
    assert!(stdout.contains("export function bId("), "{stdout}");

    // No catalog-id hash may surface in any exported client symbol. The catalog ids live only in
    // private consts, so a `cat_` substring on an `export` line is the leak this fixture guards.
    for line in stdout.lines() {
        if line.contains("export ") {
            assert!(
                !line.contains("cat_"),
                "exported symbol leaks a catalog-id hash: {line}"
            );
        }
    }
}

/// A surfaced store referenced by a surfaced store's identity field: the reference must keep using
/// the target surface's brand name, not the target store's bare source name. This pins the prior
/// typed-client behavior so the surface-less fallback never overrides a real surface name.
const SURFACED_RELATION_SOURCE: &str = "module app\n\
\n\
resource Author\n\
\x20\x20\x20\x20required name: string\n\
store ^authors(id: int): Author\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
\x20\x20\x20\x20required writtenBy: Id(^authors)\n\
store ^books(id: int): Book\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var herbert: Author\n\
\x20\x20\x20\x20herbert.name = \"Frank Herbert\"\n\
\x20\x20\x20\x20var dune: Book\n\
\x20\x20\x20\x20dune.title = \"Dune\"\n\
\x20\x20\x20\x20dune.writtenBy = Id(^authors, 1)\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^authors(1) = herbert\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = dune\n\
\n\
surface Writers from ^authors\n\
\x20\x20\x20\x20fields name\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title, writtenBy\n";

#[test]
fn client_typescript_relation_id_brand_uses_surface_name_when_surfaced() {
    let root = temp_project("surface-client-surfaced-relation", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACED_RELATION_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");

    // The reference to the surfaced `^authors` store keeps the `Writers` surface brand, not `AuthorsId`.
    assert!(stdout.contains("export type WritersId"), "{stdout}");
    assert!(stdout.contains("export function writersId("), "{stdout}");
    assert!(!stdout.contains("AuthorsId"), "{stdout}");
}

#[test]
fn client_typescript_out_writes_file_and_is_silent_on_stdout() {
    let root = temp_project("surface-client-out", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let out = root.join("generated/marrow.ts");
    let output = marrow(&[
        "client",
        "typescript",
        "--out",
        out.to_str().unwrap(),
        root.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "--out must not echo to stdout: {output:?}"
    );
    let written = std::fs::read_to_string(&out).expect("client file written");
    assert!(
        written.contains("export function createClient"),
        "{written}"
    );
    assert!(
        written.contains("// marrow-surface-digest: sha256:"),
        "{written}"
    );
}

#[test]
fn client_typescript_uses_active_computed_read_route_tags() {
    let root = temp_project("surface-client-typescript-computed-read-tag", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let check = marrow(&["check", "--format", "json", root.to_str().unwrap()]);
    assert_eq!(check.status.code(), Some(0), "check: {check:?}");
    let report = support::json(check.stdout);
    let route = route_by_alias(&report, "describe");
    fs::remove_file(root.join("marrow.lock")).expect("remove committed lock");

    let client = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(client.status.code(), Some(0), "client: {client:?}");
    let stdout = String::from_utf8(client.stdout).expect("client stdout utf8");
    // The active computed-read tag drives both the method invoke and the route-prefix table that
    // builds its POST path; the path is the prefix plus the tag, assembled at request time.
    assert!(
        stdout.contains(&format!("transport.invoke({:?}", route.operation_tag)),
        "generated client must invoke active computed-read operation tag {}; client:\n{stdout}",
        route.operation_tag
    );
    let prefix = route
        .path
        .strip_suffix(&route.operation_tag)
        .expect("route path ends with its operation tag");
    assert!(
        stdout.contains(&format!("{:?}: {:?}", route.operation_tag, prefix)),
        "generated client route table must map the active tag to its prefix {prefix}; client:\n{stdout}",
    );
}

#[test]
fn client_typescript_warns_on_stale_lock() {
    // A committed lock that the source has since outrun is the `check.stale_lock` condition: the
    // generated client may not reflect the accepted catalog the lock projects, so generating one
    // silently would hand the developer a client whose shape they cannot trust. The command must
    // warn loudly, naming the run that re-projects the lock.
    let root = temp_project("surface-client-stale-lock", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    // Drift the stored resource shape (a new field) so the committed lock's source digest falls
    // behind the current source — the same condition `check` reports as `check.stale_lock`.
    let changed = CLIENT_SURFACE_SOURCE.replace(
        "    author: string\n",
        "    author: string\n    pages: int\n",
    );
    assert_ne!(changed, CLIENT_SURFACE_SOURCE, "shape edit must apply");
    write(&root, "src/app.mw", &changed);

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("check.stale_lock"),
        "a stale lock must raise the stale-lock advisory: {stderr}"
    );
    assert!(
        stderr.contains(&format!("marrow run {}", root.to_str().unwrap())),
        "the advisory must name the run that re-projects the lock: {stderr}"
    );
}

#[test]
fn client_typescript_relative_out_resolves_against_cwd_and_prints_path() {
    // A relative `--out` follows the POSIX convention: it resolves against the process cwd, not the
    // project directory, and success prints the resolved path so the write is never invisible.
    let root = temp_project("surface-client-out-relative", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let cwd = support::temp_dir("surface-client-out-relative-cwd");
    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args([
            "client",
            "typescript",
            "--out",
            "client.ts",
            root.to_str().unwrap(),
        ])
        .current_dir(cwd.path())
        .output()
        .expect("run marrow");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let landed = cwd.join("client.ts");
    assert!(
        landed.exists(),
        "a relative --out must land under cwd, not the project dir: {output:?}"
    );
    assert!(
        !root.join("client.ts").exists(),
        "a relative --out must not resolve against the project dir"
    );
    let written = fs::read_to_string(&landed).expect("client file written");
    assert!(
        written.contains("export function createClient"),
        "{written}"
    );
    assert!(
        output.stdout.is_empty(),
        "--out must not echo the client to stdout: {output:?}"
    );
    let resolved = landed.canonicalize().expect("resolve written path");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains(&format!("wrote {}", resolved.display())),
        "success must print the resolved output path: {stderr}"
    );
}

#[test]
fn client_typescript_refreshes_declared_client_when_no_out() {
    // With a declared `client` path and no `--out`, the command must refresh the on-disk declared
    // client write-if-changed (matching run/serve/evolve), not dump to stdout and leave the
    // declared client to go stale.
    let root = temp_project_uncommitted("surface-client-declared-refresh", |root| {
        write(root, "marrow.json", &native_config_with_client());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let out = root.join("generated/marrow.ts");
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    assert!(out.exists(), "run should have written the declared client");

    // Stale the declared client so a refresh must rewrite it.
    write(&root, "generated/marrow.ts", "// stale\n");

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "a declared client with no --out must refresh on disk, not dump to stdout: {output:?}"
    );
    let refreshed = fs::read_to_string(&out).expect("declared client present");
    assert!(
        refreshed.contains("export function createClient"),
        "the declared client must be refreshed, not left stale: {refreshed}"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains(out.to_str().unwrap()),
        "refreshing the declared client must report the written path: {stderr}"
    );
}

#[test]
fn client_typescript_no_declared_client_still_prints_to_stdout() {
    // With no declared `client` and no `--out`, stdout remains the correct default.
    let root = temp_project("surface-client-no-declared", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("export function createClient"),
        "no declared client and no --out must print to stdout: {stdout}"
    );
}

#[test]
fn client_typescript_reports_project_diagnostics() {
    let root = temp_project_uncommitted("surface-client-typescript-bad-check", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", "module app\npub fn broken(\n");
    });

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "failed check should not print a partial client"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("parse."), "{stderr}");
}

#[test]
fn client_help_advertises_top_level_command() {
    let output = marrow(&["client", "--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("marrow client typescript [--out <path>] <projectdir>"));
    assert!(
        stdout.contains("--out"),
        "client help must advertise the shipped --out flag: {stdout}"
    );
    assert!(
        !stdout.contains("marrow surface"),
        "client help should not advertise removed surface commands: {stdout}"
    );
}

#[test]
fn bare_client_is_a_usage_failure() {
    // A forgotten subcommand (`marrow client`) ran nothing, so it must exit 2 like every other
    // missing-subcommand usage error rather than passing a CI gate green.
    let output = marrow(&["client"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "usage text goes to stderr, not stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("marrow client typescript"),
        "bare client must print usage on stderr: {stderr}"
    );
}

#[test]
fn client_typescript_usage_failures_exit_two() {
    let output = marrow(&["client", "typescript"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("missing project directory"), "{stderr}");
}

#[test]
fn client_typescript_generated_client_runs_against_live_surface_server() {
    let Some(node) = node_with_type_stripping() else {
        if std::env::var_os("CI").is_some() || std::env::var_os("MARROW_TEST_NODE").is_some() {
            panic!("generated TypeScript client E2E requires Node with --experimental-strip-types");
        }
        eprintln!("skipping generated TypeScript client E2E; compatible node not found");
        return;
    };
    let root = temp_project("surface-client-typescript-e2e", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let client = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(client.status.code(), Some(0), "client: {client:?}");
    assert!(client.stderr.is_empty(), "client: {client:?}");
    let app = support::temp_dir("surface-client-typescript-e2e-app");
    write(
        &app,
        "marrow-client.ts",
        &String::from_utf8(client.stdout).expect("client utf8"),
    );
    write(&app, "app.ts", GENERATED_CLIENT_APP);

    // The whole point of the typed client is that it compiles in a strict TS project. Node's
    // type stripping only erases syntax, so it cannot catch an undeclared type or a bad assignment;
    // a real `tsc --strict` pass over the generated client and its consumer is the load-bearing gate.
    type_check_strict(&app, STRICT_CONSUMER);

    {
        let (_server, addr) = spawn_surface_server_with_args(&root, &["--write"]);
        let output = Command::new(node)
            .arg("--experimental-strip-types")
            .arg("--no-warnings")
            .arg(app.join("app.ts"))
            .current_dir(&app)
            .env("MARROW_SURFACE_BASE_URL", format!("http://{addr}"))
            .output()
            .expect("run generated client app");

        assert_eq!(
            output.status.code(),
            Some(0),
            "generated client app failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout)
                .expect("app stdout utf8")
                .trim(),
            "generated-client-e2e-ok"
        );
    }

    let recover = marrow(&[
        "data",
        "recover",
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(recover.status.code(), Some(0), "recover: {recover:?}");

    let describe = marrow(&[
        "run",
        "--entry",
        "app::describe",
        "--arg",
        "id=1",
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(describe.status.code(), Some(0), "describe: {describe:?}");
    let envelope = support::json(describe.stdout);
    assert_eq!(envelope["result"]["kind"], "value", "{envelope}");
    assert_eq!(envelope["result"]["value"]["kind"], "string", "{envelope}");
    assert_eq!(
        envelope["result"]["value"]["value"], "The Dispossessed|Ursula Le Guin",
        "{envelope}"
    );

    let integrity = marrow(&[
        "data",
        "integrity",
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(integrity.status.code(), Some(0), "integrity: {integrity:?}");
}

/// A composite-key store with an enum and an optional field, exercising every decode path the unit
/// test probes against real saved data: a non-int multi-key brand, an i64 above 2^53 round-tripping
/// through bigint, an enum decoded by catalog id, an optional field arriving as `value: null`, and a
/// verbatim page cursor.
const DECODE_SURFACE_SOURCE: &str = "module app\n\
\n\
enum Tier\n\
\x20\x20\x20\x20bronze\n\
\x20\x20\x20\x20gold\n\
\n\
resource Entry\n\
\x20\x20\x20\x20required score: int\n\
\x20\x20\x20\x20required tier: Tier\n\
\x20\x20\x20\x20note: string\n\
store ^entries(group: string, seq: int): Entry\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var withNote: Entry\n\
\x20\x20\x20\x20withNote.score = 9007199254740993\n\
\x20\x20\x20\x20withNote.tier = Tier::gold\n\
\x20\x20\x20\x20withNote.note = \"present\"\n\
\x20\x20\x20\x20var withoutNote: Entry\n\
\x20\x20\x20\x20withoutNote.score = 1\n\
\x20\x20\x20\x20withoutNote.tier = Tier::bronze\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^entries(\"alpha\", 7) = withNote\n\
\x20\x20\x20\x20\x20\x20\x20\x20^entries(\"alpha\", 8) = withoutNote\n\
\n\
surface Entries from ^entries\n\
\x20\x20\x20\x20fields score, tier, note\n\
\x20\x20\x20\x20collection ^entries as list\n";

#[test]
fn client_typescript_decode_unit_tests() {
    let Some(node) = node_with_type_stripping() else {
        if std::env::var_os("CI").is_some() || std::env::var_os("MARROW_TEST_NODE").is_some() {
            panic!(
                "generated TypeScript client decode unit tests require Node with --experimental-strip-types"
            );
        }
        eprintln!(
            "skipping generated TypeScript client decode unit tests; compatible node not found"
        );
        return;
    };
    let root = temp_project("surface-client-decode-unit", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", DECODE_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let client = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(client.status.code(), Some(0), "client: {client:?}");
    let app = support::temp_dir("surface-client-decode-unit-app");
    write(
        &app,
        "marrow-client.ts",
        &String::from_utf8(client.stdout).expect("client utf8"),
    );
    write(&app, "decode.ts", DECODE_UNIT_APP);

    let (_server, addr) = spawn_surface_server(&root);
    let output = Command::new(node)
        .arg("--experimental-strip-types")
        .arg("--no-warnings")
        .arg(app.join("decode.ts"))
        .current_dir(&app)
        .env("MARROW_SURFACE_BASE_URL", format!("http://{addr}"))
        .output()
        .expect("run decode unit app");
    assert_eq!(
        output.status.code(),
        Some(0),
        "decode unit app failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("decode stdout utf8")
            .trim(),
        "decode-unit-ok"
    );
}

/// A keyless singleton surface: the read returns one record whose wire identity is `null`, and the
/// delete carries no identity. The generated client must read the record without dereferencing the
/// null identity and delete it without threading a phantom id.
const SINGLETON_CLIENT_SOURCE: &str = "module app\n\
\n\
resource Settings\n\
\x20\x20\x20\x20required theme: string\n\
\x20\x20\x20\x20mode: string\n\
store ^settings: Settings\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var settings: Settings\n\
\x20\x20\x20\x20settings.theme = \"dark\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^settings = settings\n\
\n\
surface SettingsSurface from ^settings\n\
\x20\x20\x20\x20fields theme, mode\n\
\x20\x20\x20\x20delete\n";

#[test]
fn client_typescript_keyless_singleton_reads_and_deletes_against_live_surface_server() {
    let Some(node) = node_with_type_stripping() else {
        if std::env::var_os("CI").is_some() || std::env::var_os("MARROW_TEST_NODE").is_some() {
            panic!(
                "generated TypeScript client singleton E2E requires Node with --experimental-strip-types"
            );
        }
        eprintln!("skipping generated TypeScript client singleton E2E; compatible node not found");
        return;
    };
    let root = temp_project("surface-client-singleton-e2e", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SINGLETON_CLIENT_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let client = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(client.status.code(), Some(0), "client: {client:?}");
    let client_source = String::from_utf8(client.stdout).expect("client utf8");

    // A keyless singleton record carries no synthetic id and its decoder never reads the null wire
    // identity, so the raw TypeError that crashed the read can never be emitted.
    assert!(
        !client_source.contains("record.identity.keys"),
        "singleton record decoder must not dereference the null identity: {client_source}"
    );

    let app = support::temp_dir("surface-client-singleton-e2e-app");
    write(&app, "marrow-client.ts", &client_source);
    write(&app, "app.ts", SINGLETON_CLIENT_APP);
    type_check_strict(&app, SINGLETON_STRICT_CONSUMER);

    let (_server, addr) = spawn_surface_server_with_args(&root, &["--write"]);
    let output = Command::new(node)
        .arg("--experimental-strip-types")
        .arg("--no-warnings")
        .arg(app.join("app.ts"))
        .current_dir(&app)
        .env("MARROW_SURFACE_BASE_URL", format!("http://{addr}"))
        .output()
        .expect("run singleton client app");
    assert_eq!(
        output.status.code(),
        Some(0),
        "singleton client app failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("app stdout utf8")
            .trim(),
        "singleton-client-e2e-ok"
    );
}

/// The singleton read returns the typed record with no `id` field and no argument; the singleton
/// delete takes no identity. A keyed `.get(id)` here would be a type error, so this also pins that the
/// singleton method signatures dropped identity.
const SINGLETON_STRICT_CONSUMER: &str = r#"import {
  createClient,
  type SettingsSurfaceRecord,
} from "./marrow-client.ts";

export async function pinTypes(client: ReturnType<typeof createClient>): Promise<void> {
  const record: SettingsSurfaceRecord = await client.SettingsSurface.get();
  const theme: string = record.theme;
  const mode: string | null = record.mode;
  const removed: void = await client.SettingsSurface.delete();
  void theme;
  void mode;
  void removed;
}
"#;

const SINGLETON_CLIENT_APP: &str = r#"import assert from "node:assert/strict";
import { createClient } from "./marrow-client.ts";

const client = createClient({ baseUrl: process.env.MARROW_SURFACE_BASE_URL });

// The singleton read decodes the record even though the server sends identity:null. Before the fix
// this threw a raw TypeError reading `identity.keys`.
const settings = await client.SettingsSurface.get();
assert.equal(settings.theme, "dark");
assert.equal(settings.mode, null);
assert.ok(!("id" in settings));

// The singleton delete takes no identity and removes the record; a second read then 404s.
await client.SettingsSurface.delete();
let thrown: unknown;
try {
  await client.SettingsSurface.get();
} catch (error) {
  thrown = error;
}
assert.ok(thrown);

console.log("singleton-client-e2e-ok");
"#;

/// A store whose paged index keys on an enum then an identity, the two exact-key shapes a raw-string
/// encoding would silently corrupt. Its generated `byStatus(status, author, limit)` must send the
/// checked request enum/identity argument shapes, while the `label` action must send the distinct
/// entry argument enum shape an action decodes; both contracts run against the live server.
const INDEX_KEY_SURFACE_SOURCE: &str = "module app\n\
\n\
enum Status\n\
\x20\x20\x20\x20draft\n\
\x20\x20\x20\x20published\n\
\n\
resource Author\n\
\x20\x20\x20\x20required name: string\n\
store ^authors(id: int): Author\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
\x20\x20\x20\x20required status: Status\n\
\x20\x20\x20\x20required writtenBy: Id(^authors)\n\
store ^catalog(id: int): Book\n\
\x20\x20\x20\x20index byStatus(status, writtenBy, id)\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var herbert: Author\n\
\x20\x20\x20\x20herbert.name = \"Frank Herbert\"\n\
\x20\x20\x20\x20var dune: Book\n\
\x20\x20\x20\x20dune.title = \"Dune\"\n\
\x20\x20\x20\x20dune.status = Status::published\n\
\x20\x20\x20\x20dune.writtenBy = Id(^authors, 1)\n\
\x20\x20\x20\x20var messiah: Book\n\
\x20\x20\x20\x20messiah.title = \"Dune Messiah\"\n\
\x20\x20\x20\x20messiah.status = Status::published\n\
\x20\x20\x20\x20messiah.writtenBy = Id(^authors, 1)\n\
\x20\x20\x20\x20var notes: Book\n\
\x20\x20\x20\x20notes.title = \"Working Notes\"\n\
\x20\x20\x20\x20notes.status = Status::draft\n\
\x20\x20\x20\x20notes.writtenBy = Id(^authors, 1)\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^authors(1) = herbert\n\
\x20\x20\x20\x20\x20\x20\x20\x20^catalog(1) = dune\n\
\x20\x20\x20\x20\x20\x20\x20\x20^catalog(2) = messiah\n\
\x20\x20\x20\x20\x20\x20\x20\x20^catalog(3) = notes\n\
\n\
pub fn label(status: Status): string\n\
\x20\x20\x20\x20match status\n\
\x20\x20\x20\x20\x20\x20\x20\x20draft\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return \"draft-label\"\n\
\x20\x20\x20\x20\x20\x20\x20\x20published\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return \"published-label\"\n\
\x20\x20\x20\x20return \"\"\n\
\n\
surface Authors from ^authors\n\
\x20\x20\x20\x20fields name\n\
\n\
surface Catalog from ^catalog\n\
\x20\x20\x20\x20fields title, status\n\
\x20\x20\x20\x20collection ^catalog.byStatus as byStatus\n\
\x20\x20\x20\x20action label\n";

#[test]
fn client_typescript_index_key_collection_runs_against_live_surface_server() {
    let Some(node) = node_with_type_stripping() else {
        if std::env::var_os("CI").is_some() || std::env::var_os("MARROW_TEST_NODE").is_some() {
            panic!(
                "generated TypeScript client index-key E2E requires Node with --experimental-strip-types"
            );
        }
        eprintln!("skipping generated TypeScript client index-key E2E; compatible node not found");
        return;
    };
    let root = temp_project("surface-client-index-key-e2e", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", INDEX_KEY_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let client = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(client.status.code(), Some(0), "client: {client:?}");
    let app = support::temp_dir("surface-client-index-key-e2e-app");
    write(
        &app,
        "marrow-client.ts",
        &String::from_utf8(client.stdout).expect("client utf8"),
    );
    write(&app, "index.ts", INDEX_KEY_APP);

    let (_server, addr) = spawn_surface_server_with_args(&root, &["--write"]);
    let output = Command::new(node)
        .arg("--experimental-strip-types")
        .arg("--no-warnings")
        .arg(app.join("index.ts"))
        .current_dir(&app)
        .env("MARROW_SURFACE_BASE_URL", format!("http://{addr}"))
        .output()
        .expect("run index-key app");
    assert_eq!(
        output.status.code(),
        Some(0),
        "index-key app failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("index-key stdout utf8")
            .trim(),
        "index-key-e2e-ok"
    );
}

/// The index-key consumer drives the enum+identity paged index and an enum-argument action against a
/// live server. Index keys and action arguments are distinct wire contracts; a raw-string or
/// mis-tagged encoding of either fails the server's checked-shape validation, so a successful page
/// with the expected rows and a correct action result prove both contracts serialize correctly.
const INDEX_KEY_APP: &str = r#"import assert from "node:assert/strict";
import {
  createClient,
  authorsId,
  isMarrowSurfaceError,
} from "./marrow-client.ts";

const client = createClient({ baseUrl: process.env.MARROW_SURFACE_BASE_URL });

// The enum exact-key encodes to the checked enum argument shape and the identity exact-key to the
// checked identity argument shape; the server returns exactly the two published books by this author.
const published = await client.Catalog.byStatus("published", authorsId(1), 10);
assert.deepEqual(
  published.rows.map((row) => row.title),
  ["Dune", "Dune Messiah"],
);
for (const row of published.rows) {
  assert.equal(row.status, "published");
}

// A different enum member selects the lone draft, confirming the enum key is matched by member, not
// collapsed to a constant.
const drafts = await client.Catalog.byStatus("draft", authorsId(1), 10);
assert.deepEqual(
  drafts.rows.map((row) => row.title),
  ["Working Notes"],
);

// A member outside the generated catalog is rejected before any request leaves the client.
await assert.rejects(
  () => client.Catalog.byStatus("retired", authorsId(1), 10),
  /generated catalog/,
);

// An enum action argument uses the entry argument shape, which differs from the index-key enum shape:
// the server accepts it and the action runs only when the member-tagged wire value decodes correctly.
const draftLabel = await client.Catalog.label("draft");
assert.equal(draftLabel.value, "draft-label");
const publishedLabel = await client.Catalog.label("published");
assert.equal(publishedLabel.value, "published-label");
void isMarrowSurfaceError;

console.log("index-key-e2e-ok");
"#;

/// A date-keyed store whose resource carries every non-int scalar as a projection field, plus
/// callables that take each scalar kind as an action/computed-read argument and a date identity as a
/// callable argument. It exercises the whole temporal/bytes scalar family the renderer must encode
/// and decode per wire context: a `date` identity key (request shape), the same date as a callable
/// identity argument (entry shape), `instant`/`duration` as exact-bigint response values and entry
/// arguments, `bytes` as a base64 response value and a hex entry argument, and `decimal` as canonical
/// text. The two seeded dates are 2020-01-01 (day 18262) and 2021-06-15 (day 18793).
const TEMPORAL_SURFACE_SOURCE: &str = "module app\n\
\n\
resource Event\n\
\x20\x20\x20\x20required label: string\n\
\x20\x20\x20\x20when: instant\n\
\x20\x20\x20\x20span: duration\n\
\x20\x20\x20\x20payload: bytes\n\
\x20\x20\x20\x20cost: decimal\n\
store ^events(day: date): Event\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var launch: Event\n\
\x20\x20\x20\x20launch.label = \"launch\"\n\
\x20\x20\x20\x20launch.when = instant(\"2020-01-01T00:00:00Z\")\n\
\x20\x20\x20\x20launch.span = duration(\"PT3600S\")\n\
\x20\x20\x20\x20launch.payload = bytes(\"hello\")\n\
\x20\x20\x20\x20launch.cost = 12.5\n\
\x20\x20\x20\x20var followup: Event\n\
\x20\x20\x20\x20followup.label = \"followup\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^events(date(\"2020-01-01\")) = launch\n\
\x20\x20\x20\x20\x20\x20\x20\x20^events(date(\"2021-06-15\")) = followup\n\
\n\
pub fn dayLabel(at: date): string\n\
\x20\x20\x20\x20return (^events(at).label ?? \"absent\")\n\
\n\
pub fn eventNote(ev: Id(^events)): string\n\
\x20\x20\x20\x20return (^events(ev).label ?? \"absent\")\n\
\n\
pub fn echoSpan(s: duration): string\n\
\x20\x20\x20\x20return string(s)\n\
\n\
pub fn echoMoment(m: instant): string\n\
\x20\x20\x20\x20return string(m)\n\
\n\
pub fn echoBytes(b: bytes): string\n\
\x20\x20\x20\x20return string(b)\n\
\n\
surface Events from ^events\n\
\x20\x20\x20\x20fields label, when, span, payload, cost\n\
\x20\x20\x20\x20collection ^events as list\n\
\x20\x20\x20\x20read dayLabel\n\
\x20\x20\x20\x20read eventNote\n\
\x20\x20\x20\x20action echoSpan\n\
\x20\x20\x20\x20action echoMoment\n\
\x20\x20\x20\x20action echoBytes\n";

/// Drive every temporal and bytes scalar through the generated client against a live `serve --write`:
/// a `date` identity key, a `date` callable identity argument, each scalar kind as a decoded response
/// field, and each scalar kind as an action/computed-read argument. A wrong (kind, context) cell
/// throws on the client or is rejected by the server's checked-shape validation, so a clean run proves
/// the whole family encodes and decodes correctly.
const TEMPORAL_APP: &str = r#"import assert from "node:assert/strict";
import { createClient, eventsId } from "./marrow-client.ts";

const client = createClient({ baseUrl: process.env.MARROW_SURFACE_BASE_URL });

// A date identity key uses the request shape (days_since_epoch); 2020-01-01 is day 18262.
const launch = await client.Events.get(eventsId(18262));
assert.equal(launch.label, "launch");

// instant and duration decode as exact bigints, never a lossy number.
assert.equal(typeof launch.when, "bigint");
assert.equal(launch.when, 1577836800000000000n);
assert.equal(launch.span, 3600000000000n);

// bytes decodes as base64, decimal as canonical text.
assert.equal(launch.payload, Buffer.from("hello").toString("base64"));
assert.equal(launch.cost, "12.5");

// The date identity decodes back to its faithful day count.
assert.deepEqual(launch.id.keys, [{ kind: "date", days_since_epoch: 18262 }]);

// An absent optional temporal/bytes field arrives as value:null and becomes null.
const followup = await client.Events.get(eventsId(18793));
assert.equal(followup.when, null);
assert.equal(followup.span, null);
assert.equal(followup.payload, null);

// A date scalar entry argument encodes to canonical YYYY-MM-DD text the entry decoder parses.
assert.equal(await client.Events.dayLabel(18262), "launch");
assert.equal(await client.Events.dayLabel(18793), "followup");

// A date identity entry argument encodes each key into the entry scalar shape, not the request shape.
assert.equal(await client.Events.eventNote(eventsId(18262)), "launch");
assert.equal(await client.Events.eventNote(eventsId(18793)), "followup");

// instant, duration, and bytes action arguments round-trip through the server's canonical render.
assert.equal((await client.Events.echoMoment(1577836800000000000n)).value, "2020-01-01T00:00:00Z");
assert.equal((await client.Events.echoSpan(3600000000000n)).value, "PT3600S");
assert.equal((await client.Events.echoBytes(Buffer.from("hi").toString("base64"))).value, "0x6869");

// The paged list decodes both rows, including every temporal field.
const page = await client.Events.list(10);
assert.equal(page.rows.length, 2);

console.log("temporal-e2e-ok");
"#;

#[test]
fn client_typescript_temporal_scalars_run_against_live_surface_server() {
    let Some(node) = node_with_type_stripping() else {
        if std::env::var_os("CI").is_some() || std::env::var_os("MARROW_TEST_NODE").is_some() {
            panic!(
                "generated TypeScript client temporal E2E requires Node with --experimental-strip-types"
            );
        }
        eprintln!("skipping generated TypeScript client temporal E2E; compatible node not found");
        return;
    };
    let root = temp_project("surface-client-temporal-e2e", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", TEMPORAL_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let client = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);
    assert_eq!(client.status.code(), Some(0), "client: {client:?}");
    let app = support::temp_dir("surface-client-temporal-e2e-app");
    write(
        &app,
        "marrow-client.ts",
        &String::from_utf8(client.stdout).expect("client utf8"),
    );
    write(&app, "temporal.ts", TEMPORAL_APP);

    let (_server, addr) = spawn_surface_server_with_args(&root, &["--write"]);
    let output = Command::new(node)
        .arg("--experimental-strip-types")
        .arg("--no-warnings")
        .arg(app.join("temporal.ts"))
        .current_dir(&app)
        .env("MARROW_SURFACE_BASE_URL", format!("http://{addr}"))
        .output()
        .expect("run temporal app");
    assert_eq!(
        output.status.code(),
        Some(0),
        "temporal app failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("temporal stdout utf8")
            .trim(),
        "temporal-e2e-ok"
    );
}

fn node_with_type_stripping() -> Option<PathBuf> {
    let candidates = std::env::var_os("MARROW_TEST_NODE")
        .map(PathBuf::from)
        .into_iter()
        .chain(std::env::var_os("PATH").into_iter().flat_map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("node"))
                .collect::<Vec<_>>()
        }));
    for candidate in candidates {
        let Ok(output) = Command::new(&candidate).arg("--help").output() else {
            continue;
        };
        let help = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && help.contains("--experimental-strip-types") {
            return Some(candidate);
        }
    }
    None
}

/// Type-check the generated client under `tsc --strict`, the gate that proves the client is genuinely
/// typed rather than merely strip-able. Node's `--experimental-strip-types` only erases type syntax,
/// so it cannot catch an undeclared type or a bad assignment; only a resolving type-checker does. The
/// consumer is a Node-free stub that asserts each typed signature lands in its declared type (a
/// branded id, the enum union, the non-null action result, a `Page` cursor), so the gate measures the
/// client's types, not `@types/node`. The checker is `MARROW_TEST_TSC` if set, otherwise an
/// `npx`-resolved `typescript@5`. When neither is reachable the gate is skipped, except under CI where
/// a typed client that does not compile is a hard failure.
fn type_check_strict(app_dir: &std::path::Path, consumer: &str) {
    write(app_dir, "strict-consumer.ts", consumer);
    write(
        app_dir,
        "tsconfig.json",
        concat!(
            "{ \"compilerOptions\": { \"strict\": true, \"noEmit\": true, \"target\": \"ES2022\", ",
            "\"module\": \"ESNext\", \"moduleResolution\": \"Bundler\", \"skipLibCheck\": true, ",
            "\"allowImportingTsExtensions\": true, \"types\": [] }, ",
            "\"include\": [\"marrow-client.ts\", \"strict-consumer.ts\"] }",
        ),
    );
    let Some(mut command) = strict_type_checker() else {
        if std::env::var_os("CI").is_some() {
            panic!("generated TypeScript client strict gate requires tsc; set MARROW_TEST_TSC");
        }
        eprintln!("skipping generated TypeScript client strict gate; no tsc found");
        return;
    };
    let output = command
        .arg("--project")
        .arg(app_dir.join("tsconfig.json"))
        .current_dir(app_dir)
        .output()
        .expect("run tsc --strict");
    assert!(
        output.status.success(),
        "generated client failed tsc --strict\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn strict_type_checker() -> Option<Command> {
    if let Some(tsc) = std::env::var_os("MARROW_TEST_TSC") {
        return Some(Command::new(tsc));
    }
    let mut npx = Command::new("npx");
    npx.args(["-y", "-p", "typescript@5", "tsc"]);
    Command::new("npx")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| npx)
}

/// A Node-free consumer that pins each typed signature of the `Books` client to its declared type, so
/// `tsc --strict` fails if a brand, the enum union, the non-null action result, or the page cursor
/// ever decays to `string`/`any`/`undefined`. It is never executed; it exists only to be type-checked.
const STRICT_CONSUMER: &str = r#"import {
  createClient,
  booksId,
  type BooksRecord,
  type BooksCursor,
} from "./marrow-client.ts";

export async function pinTypes(client: ReturnType<typeof createClient>): Promise<void> {
  const id = booksId(1);
  const record: BooksRecord = await client.Books.get(id);
  const title: string = record.title;
  const author: string | null = record.author;

  const page = await client.Books.byAuthor("Frank Herbert", 10);
  const rows: BooksRecord[] = page.rows;
  const cursor: BooksCursor | null = page.next;
  await client.Books.byAuthor("Frank Herbert", 10, cursor);

  const created: BooksRecord = await client.Books.create(id, { title: "a", author: "b" });
  // Every update field is optional: a sparse one-field patch and a full patch both type-check.
  await client.Books.update(id, { author: "c" });
  await client.Books.update(id, { title: "t" });
  await client.Books.update(id, { title: "t", author: "c" });
  await client.Books.update(id, {});
  const removed: void = await client.Books.delete(id);

  const retitled: { value: string; output: string } = await client.Books.retitle(1, "x");
  const described: string = await client.Books.describe(1);

  void title;
  void author;
  void rows;
  void created;
  void removed;
  void retitled.value;
  void retitled.output;
  void described;
}
"#;

/// The hand-authored E2E exercises the typed client end to end against a live `serve --write`:
/// branded ids, name-keyed create, typed records, decoded computed-read and action values, a
/// `Page` whose cursor round-trips verbatim into a `surface.stale_cursor`, and a typed
/// `MarrowSurfaceError`.
const GENERATED_CLIENT_APP: &str = r#"import assert from "node:assert/strict";
import {
  createClient,
  booksId,
  isMarrowSurfaceError,
  type BooksRecord,
} from "./marrow-client.ts";

const client = createClient({ baseUrl: process.env.MARROW_SURFACE_BASE_URL });

const seeded: BooksRecord = await client.Books.get(booksId(1));
assert.equal(seeded.title, "Dune");
assert.equal(typeof seeded.title, "string");

const staleAfterCreate = await client.Books.byAuthor("Frank Herbert", 1);
assert.ok(staleAfterCreate.next);
const verbatimCursor = staleAfterCreate.next;

const created: BooksRecord = await client.Books.create(booksId(3), {
  title: "Children of Dune",
  author: "Frank Herbert",
});
assert.equal(created.title, "Children of Dune");

await assert.rejects(
  () => client.Books.byAuthor("Frank Herbert", 10, verbatimCursor),
  (error: unknown) =>
    isMarrowSurfaceError(error) && error.code === "surface.stale_cursor",
);

const staleAfterUpdate = await client.Books.byAuthor("Frank Herbert", 1);
assert.ok(staleAfterUpdate.next);

// A sparse update over the multi-field body patches only `author`; the omitted `title` is preserved
// rather than cleared, the exact race a whole-record read-modify-write would lose.
await client.Books.update(booksId(1), { author: "Ursula Le Guin" });
const updatedRead = await client.Books.get(booksId(1));
assert.equal(updatedRead.author, "Ursula Le Guin");
assert.equal(updatedRead.title, "Dune");
// A full update sets both fields at once and still type-checks and applies.
await client.Books.update(booksId(1), { title: "Dune", author: "Frank Herbert" });
const fullUpdated = await client.Books.get(booksId(1));
assert.equal(fullUpdated.title, "Dune");
assert.equal(fullUpdated.author, "Frank Herbert");
await client.Books.update(booksId(1), { author: "Ursula Le Guin" });
await assert.rejects(
  () => client.Books.byAuthor("Frank Herbert", 10, staleAfterUpdate.next),
  (error: unknown) =>
    isMarrowSurfaceError(error) && error.code === "surface.stale_cursor",
);

const frank = await client.Books.byAuthor("Frank Herbert", 10);
assert.deepEqual(
  frank.rows.map((record) => record.title),
  ["Dune Messiah", "Children of Dune"],
);

const retitled = await client.Books.retitle(1, "The Dispossessed");
assert.deepEqual(retitled, { value: "The Dispossessed", output: "" });
const retitledRead = await client.Books.get(booksId(1));
assert.equal(retitledRead.title, "The Dispossessed");

const described = await client.Books.describe(1);
assert.equal(described, "The Dispossessed|Ursula Le Guin");

const deleted = await client.Books.delete(booksId(3));
assert.equal(deleted, undefined);
await assert.rejects(
  () => client.Books.get(booksId(3)),
  (error: unknown) =>
    isMarrowSurfaceError(error) && error.code === "surface.absent",
);

// The branded id constructor validates eagerly, so an unsafe number throws at construction.
assert.throws(() => booksId(Number.MAX_SAFE_INTEGER + 1), /safe integers/);

console.log("generated-client-e2e-ok");
"#;

/// The decode unit checks: an i64 above 2^53 round trips through bigint, a composite non-int key
/// brands correctly, an enum decodes by its catalog id, an optional field arriving as `value: null`
/// becomes `null`, the page cursor is preserved verbatim, and a missing record throws a typed
/// `MarrowSurfaceError`.
const DECODE_UNIT_APP: &str = r#"import assert from "node:assert/strict";
import {
  createClient,
  entriesId,
  isMarrowSurfaceError,
  MarrowSurfaceError,
} from "./marrow-client.ts";

// A recording fetch tees the last response body so the verbatim-cursor check can compare the typed
// page cursor against exactly what the server sent.
let lastBody: any = null;
const recordingFetch = async (input: string, init: any) => {
  const response = await fetch(input, init);
  const body = await response.json();
  lastBody = body;
  return { ok: response.ok, json: async () => body };
};
const options = { baseUrl: process.env.MARROW_SURFACE_BASE_URL, fetch: recordingFetch };
const client = createClient(options);

// An i64 above 2^53 survives as an exact bigint, never a truncated number. A JS number would
// collapse this odd value to the even 2^53 below it, so the bigint must differ from that float.
const withNote = await client.Entries.get(entriesId("alpha", 7));
assert.equal(withNote.score, 9007199254740993n);
assert.equal(typeof withNote.score, "bigint");
assert.notEqual(withNote.score, BigInt(Number(9007199254740993n)));
assert.equal(BigInt(Number(9007199254740993n)), 9007199254740992n);

// The enum decodes through the generated member-id table to its label.
assert.equal(withNote.tier, "gold");

// A present optional field decodes; an absent one arrives as value:null and becomes null.
assert.equal(withNote.note, "present");
const withoutNote = await client.Entries.get(entriesId("alpha", 8));
assert.equal(withoutNote.note, null);
assert.equal(withoutNote.tier, "bronze");

// A composite, non-int key brands and decodes back to its two-scalar key vector.
assert.deepEqual(withNote.id.keys, [
  { kind: "string", value: "alpha" },
  { kind: "int", value: "7" },
]);

// The typed page cursor equals the raw envelope cursor, byte for byte.
const page = await client.Entries.list(1);
assert.equal(page.rows.length, 1);
assert.ok(page.next);
assert.deepEqual(page.next, lastBody.result.page.next);

// The cursor passes back unchanged and continues the scan.
const second = await client.Entries.list(1, page.next);
assert.equal(second.rows.length, 1);
assert.notDeepEqual(second.rows[0].id.keys, page.rows[0].id.keys);

// A missing record throws a typed MarrowSurfaceError with a stable code.
let thrown: unknown;
try {
  await client.Entries.get(entriesId("alpha", 999));
} catch (error) {
  thrown = error;
}
assert.ok(isMarrowSurfaceError(thrown));
assert.ok(thrown instanceof MarrowSurfaceError);
assert.equal((thrown as MarrowSurfaceError).code, "surface.absent");

console.log("decode-unit-ok");
"#;
