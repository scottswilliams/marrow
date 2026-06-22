use crate::support;

use support::{marrow, temp_project, write};

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

fn native_config_with_client() -> String {
    r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#
        .to_string()
}

#[test]
fn locked_fails_on_stale_or_absent_client_with_surface() {
    let root = temp_project("check-stale-client", |root| {
        write(root, "marrow.json", &native_config_with_client());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let fresh = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        fresh.status.code(),
        Some(0),
        "fresh client should pass --locked: {fresh:?}"
    );

    std::fs::remove_file(root.join("generated/marrow.ts")).unwrap();
    let locked = marrow(&[
        "check",
        "--locked",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(locked.status.code(), Some(1), "{locked:?}");
    let report = support::json(locked.stdout);
    assert!(
        serde_json::to_string(&report)
            .unwrap()
            .contains("check.stale_client"),
        "{report}",
    );

    let advisory = marrow(&["check", root.to_str().unwrap()]);
    assert_eq!(
        advisory.status.code(),
        Some(0),
        "plain check advises and passes: {advisory:?}"
    );
    let stderr = String::from_utf8(advisory.stderr).unwrap();
    assert!(stderr.contains("check.stale_client"), "{stderr}");
    assert!(
        !root.join("generated/marrow.ts").exists(),
        "check is read-only and must never write the declared client",
    );
}

#[test]
fn locked_ignores_client_when_no_config() {
    let root = temp_project("check-client-na", |root| {
        write(root, "marrow.json", support::native_config()); // no client field
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let locked = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        locked.status.code(),
        Some(0),
        "no client config means no client gate: {locked:?}"
    );
}
