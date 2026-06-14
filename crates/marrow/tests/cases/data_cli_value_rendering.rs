//! Text rendering for `marrow data dump` and `marrow data get` values.
//! Structured formats remain byte-exact via `value_b64`; these tests pin only the
//! human text contract for typed saved values.
use crate::support;
use crate::support_data;
use support::{temp_project_uncommitted, write};
use support_data::{checked_place, encode_identity_keys, field_path, marrow, write_tree_value};

use marrow_store::key::SavedKey;

const VALUE_RENDER_CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#;

fn stdout(output: std::process::Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    String::from_utf8(output.stdout).expect("stdout utf8")
}

fn hex_text(bytes: &[u8]) -> String {
    let mut text = String::from("0x");
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn seeded_project(name: &str, source: &str) -> (support::TempProject, String) {
    let project = temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", VALUE_RENDER_CONFIG);
        write(root, "src/app.mw", source);
    });
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    (project, dir)
}

#[test]
fn data_text_does_not_render_type_wrong_identity_payload_as_a_path() {
    let (project, dir) = seeded_project(
        "data-value-render-identity-key-type",
        "module app\n\
         resource Author\n\
         \x20   required name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   author: Id(^authors)\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^books(1).title = \"Mort\"\n",
    );
    let place = checked_place(&project, "books");
    let author_path = field_path(&place, "author");
    let payload = encode_identity_keys(&[SavedKey::Str("not-an-int".to_string())]);
    write_tree_value(
        &project,
        "books",
        &[SavedKey::Int(1)],
        &author_path,
        payload.clone(),
    );

    let reference = stdout(marrow(&["data", "get", &dir, "^books(1).author"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));
    let rendered = hex_text(&payload);

    assert_eq!(reference, format!("{rendered}\n"));
    assert!(
        dump.contains(&format!("^books(1).author\t{rendered}\n")),
        "{dump}"
    );
    assert!(
        !dump.contains("^authors(\"not-an-int\")"),
        "type-wrong identity payload must not render as a saved path: {dump}"
    );
}

#[test]
fn data_text_quotes_strings_and_hexes_bytes() {
    let (_project, dir) = seeded_project(
        "data-value-render-strings",
        "module app\n\
         resource Item\n\
         \x20   required label: string\n\
         \x20   payload: bytes\n\
         store ^items(id: int): Item\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^items(1).label = \"first\\tline\\n^forged(9).value\"\n\
         \x20       ^items(1).payload = b\"raw\\t\\xff\"\n",
    );

    let label = stdout(marrow(&["data", "get", &dir, "^items(1).label"]));
    let payload = stdout(marrow(&["data", "get", &dir, "^items(1).payload"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));

    assert_eq!(label, "\"first\\tline\\n^forged(9).value\"\n");
    assert_eq!(payload, "0x72617709ff\n");
    assert!(
        dump.contains("^items(1).label\t\"first\\tline\\n^forged(9).value\"\n"),
        "{dump}"
    );
    assert!(dump.contains("^items(1).payload\t0x72617709ff\n"), "{dump}");
    assert!(
        !dump.contains("\n^forged(9).value"),
        "string value must not forge a second TSV record: {dump}"
    );
}

#[test]
fn data_text_renders_identity_references_as_saved_paths() {
    let (_project, dir) = seeded_project(
        "data-value-render-identity",
        "module app\n\
         resource Author\n\
         \x20   required name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   author: Id(^authors)\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   const author: Id(^authors) = nextId(^authors)\n\
         \x20   transaction\n\
         \x20       ^authors(author).name = \"Ada\"\n\
         \x20       ^books(1).title = \"Mort\"\n\
         \x20       ^books(1).author = author\n",
    );

    let reference = stdout(marrow(&["data", "get", &dir, "^books(1).author"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));

    assert_eq!(reference, "^authors(1)\n");
    assert!(dump.contains("^books(1).author\t^authors(1)\n"), "{dump}");
}

#[test]
fn data_text_renders_enum_values_as_member_identities() {
    let (_project, dir) = seeded_project(
        "data-value-render-enum",
        "module app\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order\n\
         \x20   required state: Status\n\
         store ^orders(id: int): Order\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^orders(1).state = Status::archived\n",
    );

    let state = stdout(marrow(&["data", "get", &dir, "^orders(1).state"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));

    assert_eq!(state, "app::Status::archived\n");
    assert!(
        dump.contains("^orders(1).state\tapp::Status::archived\n"),
        "{dump}"
    );
    assert!(
        !dump.contains("cat_"),
        "catalog ids leaked into output: {dump}"
    );
}
