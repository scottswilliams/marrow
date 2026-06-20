use crate::support;

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
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
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

#[test]
fn client_typescript_prints_generated_client_without_opening_store() {
    let root = temp_project("surface-client-typescript", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let store_path = root.join(".data/marrow.redb");
    assert!(store_path.exists(), "fixture should have seeded a store");
    std::fs::remove_file(&store_path).expect("remove store file");

    let output = marrow(&[
        "client",
        "typescript",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("export function createMarrowSurfaceClient"),
        "{stdout}"
    );
    assert!(stdout.contains("\"app\""), "{stdout}");
    assert!(stdout.contains("\"Books\""), "{stdout}");
    assert!(stdout.contains("/surface/v1/create/"), "{stdout}");
    assert!(stdout.contains("/surface/v1/delete/"), "{stdout}");
    assert!(stdout.contains("Number.isSafeInteger"), "{stdout}");
    assert!(
        !store_path.exists(),
        "client generation must not recreate or open the native store"
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
    assert!(stdout.contains("marrow client typescript <projectdir>"));
    assert!(
        !stdout.contains("marrow surface"),
        "client help should not advertise removed surface commands: {stdout}"
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
