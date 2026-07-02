use std::fs;
use std::path::Path;

use crate::support;
use support::{marrow, temp_project_uncommitted, write};

const CLIENT_BASELINE_SOURCE: &str = "module app\n\
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
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
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
\x20\x20\x20\x20read describe\n";

// Adds an optional `summary` field surfaced through `fields`, a clean evolution
// that changes the surface ABI so the declared client must be regenerated.
const CLIENT_EVOLVED_SOURCE: &str = "module app\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
\x20\x20\x20\x20author: string\n\
\x20\x20\x20\x20summary: string\n\
store ^books(id: int): Book\n\
\x20\x20\x20\x20index byAuthor(author, id)\n\
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
\x20\x20\x20\x20fields title, author, summary\n\
\x20\x20\x20\x20create title, author\n\
\x20\x20\x20\x20update author\n\
\x20\x20\x20\x20delete\n\
\x20\x20\x20\x20collection ^books.byAuthor as byAuthor\n\
\x20\x20\x20\x20read describe\n";

#[test]
fn evolve_apply_refreshes_declared_client() {
    let root = temp_project_uncommitted("evolve-apply-client", |root| {
        write(
            root,
            "marrow.json",
            r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#,
        );
        write(root, "src/app.mw", CLIENT_BASELINE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let out = root.join("generated/marrow.ts");
    let before = fs::read_to_string(&out).expect("client written by seed run");

    write(&root, "src/app.mw", CLIENT_EVOLVED_SOURCE);
    let apply = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(apply.status.code(), Some(0), "apply: {apply:?}");

    let after = fs::read_to_string(&out).expect("client present after apply");
    assert_ne!(
        before, after,
        "a surface-changing evolution must refresh the declared client"
    );

    // Refreshing is not enough: the written client must match the surface `check --locked`
    // recomputes from the freshly committed catalog, so CI is green immediately after apply.
    let locked = marrow(&[
        "check",
        "--locked",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        locked.status.code(),
        Some(0),
        "locked check must pass right after apply: {locked:?}"
    );
}

// A restored, already-accepted store gains a required field backfilled by an `evolve default`. This
// is the DOGFOOD-R001-03 shape: the evolution is pending until apply activates it, so a client
// rendered from the pre-apply program would project an empty surface and fail the locked check.
const REQUIRED_BASELINE_SOURCE: &str = "module app\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
store ^books(id: int): Book\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var book: Book\n\
\x20\x20\x20\x20book.title = \"Dune\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title\n\
\x20\x20\x20\x20create title\n";

const REQUIRED_DEFAULT_SOURCE: &str = "module app\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
\x20\x20\x20\x20required format: string\n\
store ^books(id: int): Book\n\
\n\
evolve\n\
\x20\x20\x20\x20default Book.format = \"paperback\"\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var book: Book\n\
\x20\x20\x20\x20book.title = \"Dune\"\n\
\x20\x20\x20\x20book.format = \"paperback\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title, format\n\
\x20\x20\x20\x20create title, format\n";

#[test]
fn evolve_apply_over_a_required_default_leaves_the_locked_check_green() {
    let root = temp_project_uncommitted("evolve-apply-required-default-client", |root| {
        write(
            root,
            "marrow.json",
            r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#,
        );
        write(root, "src/app.mw", REQUIRED_BASELINE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    write(&root, "src/app.mw", REQUIRED_DEFAULT_SOURCE);
    let apply = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(apply.status.code(), Some(0), "apply: {apply:?}");

    let locked = marrow(&[
        "check",
        "--locked",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        locked.status.code(),
        Some(0),
        "the declared client must be fresh right after apply, not stale: {locked:?}"
    );

    // The refreshed client carries the newly accepted field, proving apply rendered the stable
    // post-apply surface rather than the pre-apply program's empty one.
    let client = fs::read_to_string(root.join("generated/marrow.ts")).expect("client after apply");
    assert!(
        client.contains("format"),
        "apply must render the full post-apply surface: {client}"
    );
}

#[test]
fn declared_client_refresh_has_a_single_shared_owner() {
    // The declared client is (re)written in exactly one place. Run, serve, and evolve apply must
    // refresh it through the shared `sync_declared_client` owner, never a second render-and-write
    // copy, so no command can drift into projecting a different client than `check --locked` expects.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |relative: &str| fs::read_to_string(src.join(relative)).expect(relative);

    let main = read("main.rs");
    assert_eq!(
        main.matches("fn write_declared_client_if_changed").count(),
        1,
        "the declared-client write owner is defined exactly once"
    );
    assert_eq!(
        main.matches("fn sync_declared_client").count(),
        1,
        "the shared refresh wrapper is defined exactly once"
    );
    assert_eq!(
        main.matches("render_typescript_client(").count(),
        1,
        "the declared client is rendered in exactly one owner"
    );

    for command in ["cmd_run.rs", "cmd_serve.rs", "cmd_evolve/mod.rs"] {
        let body = read(command);
        assert_eq!(
            body.matches("render_typescript_client").count(),
            0,
            "{command} must not render the declared client itself"
        );
        assert!(
            body.contains("sync_declared_client"),
            "{command} must refresh the declared client through the shared owner"
        );
    }
}
