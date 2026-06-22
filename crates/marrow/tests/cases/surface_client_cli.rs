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
\x20\x20\x20\x20update author\n\
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
    type_check_strict(&app);

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
fn type_check_strict(app_dir: &std::path::Path) {
    write(app_dir, "strict-consumer.ts", STRICT_CONSUMER);
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
  await client.Books.update(id, { author: "c" });
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

await client.Books.update(booksId(1), { author: "Ursula Le Guin" });
const updatedRead = await client.Books.get(booksId(1));
assert.equal(updatedRead.author, "Ursula Le Guin");
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

/// The decode unit checks, each tied to a red-team soundness correction: an i64 above 2^53 round
/// trips through bigint (F9), a composite non-int key brands correctly (F6), an enum decodes by its
/// catalog id (F4), an optional field arriving as `value: null` becomes `null` (F8), the page cursor
/// is preserved verbatim (D8), and a missing record throws a typed `MarrowSurfaceError` (F* errors).
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

// F9: an i64 above 2^53 survives as an exact bigint, never a truncated number. A JS number would
// collapse this odd value to the even 2^53 below it, so the bigint must differ from that float.
const withNote = await client.Entries.get(entriesId("alpha", 7));
assert.equal(withNote.score, 9007199254740993n);
assert.equal(typeof withNote.score, "bigint");
assert.notEqual(withNote.score, BigInt(Number(9007199254740993n)));
assert.equal(BigInt(Number(9007199254740993n)), 9007199254740992n);

// F4: the enum decodes through the generated member-id table to its label.
assert.equal(withNote.tier, "gold");

// F8: a present optional field decodes; an absent one arrives as value:null and becomes null.
assert.equal(withNote.note, "present");
const withoutNote = await client.Entries.get(entriesId("alpha", 8));
assert.equal(withoutNote.note, null);
assert.equal(withoutNote.tier, "bronze");

// F6: a composite, non-int key brands and decodes back to its two-scalar key vector.
assert.deepEqual(withNote.id.keys, [
  { kind: "string", value: "alpha" },
  { kind: "int", value: "7" },
]);

// D8: the typed page cursor equals the raw envelope cursor, byte for byte.
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
