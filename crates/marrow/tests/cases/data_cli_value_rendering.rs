//! Text rendering for `marrow data dump` and `marrow data get` values.
//! Structured formats remain byte-exact via `value_b64`; these tests pin only the
//! human text contract for typed saved values.
use crate::support;
use crate::support_data;
use support::{temp_project_uncommitted, write};
use support_data::{
    checked_place, encode_identity_keys, field_path, marrow, read_tree_value, write_tree_value,
};

use marrow_store::key::SavedKey;
use marrow_store::tree::{TreeEnumMember, decode_tree_enum_member, encode_tree_enum_member};

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
fn data_text_marks_an_undecodable_string_leaf_distinctly_from_bytes() {
    // A `string` leaf whose stored bytes are not valid UTF-8 is corruption, not a
    // bytes value. The text renderer must mark it as such so a reader cannot mistake
    // it for a legitimate `0x<hex>` bytes field, while `data integrity` stays the
    // authority that flags it and the lossless JSON form keeps the raw bytes.
    let (project, dir) = seeded_project(
        "data-value-render-undecodable-string",
        "module app\n\
         resource Note\n\
         \x20   required body: string\n\
         store ^notes(id: int): Note\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^notes(1).body = \"hello\"\n",
    );
    let place = checked_place(&project, "notes");
    let body_path = field_path(&place, "body");
    // Flip the first byte so the stored value is no longer valid UTF-8.
    let corrupt = vec![0xff, b'e', b'l', b'l', b'o'];
    write_tree_value(
        &project,
        "notes",
        &[SavedKey::Int(1)],
        &body_path,
        corrupt.clone(),
    );

    let reference = stdout(marrow(&["data", "get", &dir, "^notes(1).body"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));
    let bare_hex = hex_text(&corrupt);

    let expected = format!("<undecodable string: {bare_hex}>");
    assert_eq!(reference, format!("{expected}\n"));
    assert!(
        dump.contains(&format!("^notes(1).body\t{expected}\n")),
        "{dump}"
    );
    assert!(
        !reference.starts_with("0x") && !dump.contains(&format!("^notes(1).body\t{bare_hex}\n")),
        "a corrupt string must not render as a bare bytes value: get={reference:?} dump={dump:?}"
    );

    // `data integrity` remains the authority and still flags the corruption.
    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");

    // The lossless JSON form carries the unchanged raw bytes regardless of the text marker.
    let value_json = support_data::json(marrow(&[
        "data",
        "get",
        "--format",
        "json",
        &dir,
        "^notes(1).body",
    ]));
    let raw = marrow_run::base64::decode(value_json["value_b64"].as_str().expect("b64"))
        .expect("decode value");
    assert_eq!(raw, corrupt);
}

#[test]
fn data_text_marks_an_undecodable_numeric_leaf_distinctly_from_bytes() {
    // An `int` leaf whose stored bytes are not its canonical form is corruption, not a
    // bytes value. The renderer must mark it the same way it marks an undecodable
    // string or enum, so a reader cannot mistake the raw `0x<hex>` for a legitimate
    // bytes field, while `data integrity` stays the authority that flags it and the
    // lossless JSON form keeps the raw bytes.
    let (project, dir) = seeded_project(
        "data-value-render-undecodable-int",
        "module app\n\
         resource Counter\n\
         \x20   required value: int\n\
         store ^counters(id: int): Counter\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^counters(1).value = 1\n",
    );
    let place = checked_place(&project, "counters");
    let value_path = field_path(&place, "value");
    // `01` parses as 1 but is not the canonical int spelling, so the strict decoder
    // rejects it as corruption rather than normalizing it.
    let corrupt = b"01".to_vec();
    write_tree_value(
        &project,
        "counters",
        &[SavedKey::Int(1)],
        &value_path,
        corrupt.clone(),
    );

    let reference = stdout(marrow(&["data", "get", &dir, "^counters(1).value"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));
    let bare_hex = hex_text(&corrupt);

    let expected = format!("<undecodable int: {bare_hex}>");
    assert_eq!(reference, format!("{expected}\n"));
    assert!(
        dump.contains(&format!("^counters(1).value\t{expected}\n")),
        "{dump}"
    );
    assert!(
        !reference.starts_with("0x")
            && !dump.contains(&format!("^counters(1).value\t{bare_hex}\n")),
        "a corrupt int must not render as a bare bytes value: get={reference:?} dump={dump:?}"
    );

    // `data integrity` remains the authority and still flags the corruption.
    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");

    // The lossless JSON form carries the unchanged raw bytes regardless of the text marker.
    let value_json = support_data::json(marrow(&[
        "data",
        "get",
        "--format",
        "json",
        &dir,
        "^counters(1).value",
    ]));
    let raw = marrow_run::base64::decode(value_json["value_b64"].as_str().expect("b64"))
        .expect("decode value");
    assert_eq!(raw, corrupt);
}

#[test]
fn data_text_renders_a_drifted_leaf_by_its_accepted_catalog_type() {
    // Data was committed under `pages: int`. Drifting the source to `pages: string`
    // is a blocked populated-leaf retype; the inspection tools must render the real
    // stored value by the accepted catalog type (`int 0`), not the uncommitted
    // proposal type (a quoted `"0"`).
    let (project, dir) = seeded_project(
        "data-value-render-drift",
        "module app\n\
         resource Book\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   ^books(1).pages = 0\n",
    );

    let before = stdout(marrow(&["data", "get", &dir, "^books(1).pages"]));
    assert_eq!(before, "0\n");

    write(
        project.path(),
        "src/app.mw",
        "module app\n\
         resource Book\n\
         \x20   required pages: string\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   ^books(1).pages = \"\"\n",
    );

    let drifted = stdout(marrow(&["data", "get", &dir, "^books(1).pages"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));

    assert_eq!(
        drifted, "0\n",
        "a blocked int->string retype renders the accepted int, not a quoted string"
    );
    assert!(
        dump.contains("^books(1).pages\t0\n"),
        "dump renders the accepted int leaf: {dump}"
    );
}

#[test]
fn data_text_round_trips_control_byte_string_keys_and_values() {
    // A string key or value may legally hold any byte, including control bytes that
    // the `.mw` string grammar has no escaped spelling for (NUL, BEL, VT, FF, ESC,
    // DEL). The text format must escape every such byte so a dumped path is feedable
    // back to `data get` as a process argument and round-trips to the same record.
    let (project, dir) = seeded_project(
        "data-value-render-control-bytes",
        "module app\n\
         resource Item\n\
         \x20   required label: string\n\
         store ^items(name: string): Item\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^items(\"seed\").label = \"placeholder\"\n",
    );
    let place = checked_place(&project, "items");
    let controls = "a\u{0}b\u{7}c\u{b}d\u{c}e\u{1b}f\u{7f}g";
    let key = SavedKey::Str(controls.to_string());
    let label_path = field_path(&place, "label");
    let value_bytes = controls.as_bytes().to_vec();
    write_tree_value(
        &project,
        "items",
        std::slice::from_ref(&key),
        &label_path,
        value_bytes.clone(),
    );

    let dump = stdout(marrow(&["data", "dump", &dir]));
    let line = dump
        .lines()
        .find(|line| line.starts_with("^items(\"a"))
        .unwrap_or_else(|| panic!("no control-byte cell line in dump: {dump:?}"));
    let (path, value) = line.split_once('\t').expect("tab-separated cell");

    assert!(
        !path.contains('\u{0}') && !path.contains('\u{1b}') && !path.contains('\u{7f}'),
        "dumped path must not carry raw control bytes: {path:?}"
    );
    assert!(
        path.contains("\\x00") && path.contains("\\x1b") && path.contains("\\x7f"),
        "control bytes must escape as \\xNN: {path:?}"
    );

    let echoed = stdout(marrow(&["data", "get", &dir, path]));
    assert_eq!(echoed, format!("{value}\n"));

    // The lossless JSON form still carries the unchanged raw bytes.
    let value_json = support_data::json(marrow(&["data", "get", "--format", "json", &dir, path]));
    let raw = marrow_run::base64::decode(value_json["value_b64"].as_str().expect("b64"))
        .expect("decode value");
    assert_eq!(raw, value_bytes);
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

#[test]
fn data_text_marks_an_undecodable_enum_leaf_distinctly_from_bytes() {
    // An enum leaf whose stored member the current schema no longer names (a member
    // removed by a supported evolution) is corruption, not a healthy bytes value. The
    // text renderer must mark it as such — the enum analog of an undecodable string —
    // so a reader cannot mistake the raw `0x<hex>` for a legitimate value, while
    // `data integrity` stays the authority that flags it and the lossless JSON form
    // keeps the raw bytes.
    let (project, dir) = seeded_project(
        "data-value-render-undecodable-enum",
        "module app\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Order\n\
         \x20   required state: Status\n\
         store ^orders(id: int): Order\n\
         pub fn seed()\n\
         \x20   transaction\n\
         \x20       ^orders(1).state = Status::active\n",
    );
    let place = checked_place(&project, "orders");
    let state_path = field_path(&place, "state");

    // The stored value names a real `Status` member; keep its enum id but point the
    // member id at a catalog id the schema never declares, as a removed member leaves
    // behind. The hex still decodes as bytes, so without the marker it would render
    // indistinguishably from a healthy bytes field.
    let stored = decode_tree_enum_member(&read_tree_value(
        &project,
        "orders",
        &[SavedKey::Int(1)],
        &state_path,
    ))
    .expect("seeded enum member decodes");
    let removed_member = marrow_store::cell::CatalogId::new("cat_".to_string() + &"0".repeat(32))
        .expect("fabricated member catalog id");
    let corrupt = encode_tree_enum_member(&TreeEnumMember::new(
        stored.enum_id().clone(),
        removed_member.clone(),
    ))
    .expect("encode corrupt enum member");
    write_tree_value(
        &project,
        "orders",
        &[SavedKey::Int(1)],
        &state_path,
        corrupt.clone(),
    );

    let reference = stdout(marrow(&["data", "get", &dir, "^orders(1).state"]));
    let dump = stdout(marrow(&["data", "dump", &dir]));
    let expected = format!("<undecodable enum: {}>", removed_member.as_str());
    let bare_hex = hex_text(&corrupt);

    assert_eq!(reference, format!("{expected}\n"));
    assert!(
        dump.contains(&format!("^orders(1).state\t{expected}\n")),
        "{dump}"
    );
    assert!(
        !reference.starts_with("0x") && !dump.contains(&format!("^orders(1).state\t{bare_hex}\n")),
        "a corrupt enum must not render as a bare bytes value: get={reference:?} dump={dump:?}"
    );

    // `data integrity` remains the authority and still flags the corruption.
    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");

    // The lossless JSON form carries the unchanged raw bytes regardless of the marker.
    let value_json = support_data::json(marrow(&[
        "data",
        "get",
        "--format",
        "json",
        &dir,
        "^orders(1).state",
    ]));
    let raw = marrow_run::base64::decode(value_json["value_b64"].as_str().expect("b64"))
        .expect("decode value");
    assert_eq!(raw, corrupt);
}
