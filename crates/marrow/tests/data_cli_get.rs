//! `marrow data get`: reading one path's value, distinguishing a present value, a
//! children-only node, and a truly absent path. Presence and value are asserted as
//! typed JSON fields; a malformed path is asserted by its usage exit code. The
//! default human text output is pinned by a separate render-contract test.

mod support;
mod support_data;

use support_data::{json, marrow, native_project, seeded_project};

/// The human-rendered placeholders `data get` prints in its default text format for a
/// children-only identity node and a missing path. These are render-contract goldens:
/// `data get --format json` is the typed oracle for presence (`children_only` / `absent`)
/// and value, asserted in the JSON tests below; these strings only pin the text rendering
/// of those branches, which has no typed surface of its own. Regenerate only on an
/// intentional change to the rendered placeholders.
const CHILDREN_ONLY_TEXT_GOLDEN: &str = "(no value; has children)";
const ABSENT_TEXT_GOLDEN: &str = "(absent)";

#[test]
fn data_get_reads_a_path_value_and_reports_absence() {
    let (_project, dir) = seeded_project("data-get");
    let present = marrow(&["data", "get", "--format", "json", &dir, "^counter(1).value"]);
    let absent = marrow(&["data", "get", "--format", "json", &dir, "^counter(2).value"]);
    let malformed = marrow(&["data", "get", &dir, "counter(1)"]);

    assert_eq!(present.status.code(), Some(0), "{present:?}");
    let present_json = json(present);
    assert_eq!(present_json["presence"], serde_json::json!("value_only"));
    let value = marrow_run::base64::decode(present_json["value_b64"].as_str().expect("b64"))
        .expect("decode value");
    assert_eq!(value, b"42");

    assert_eq!(absent.status.code(), Some(0), "{absent:?}");
    let absent_json = json(absent);
    assert_eq!(absent_json["presence"], serde_json::json!("absent"));
    assert_eq!(absent_json["value_b64"], serde_json::Value::Null);

    // A path that does not parse fails before touching the store: a usage error.
    assert_eq!(malformed.status.code(), Some(2), "{malformed:?}");
}

#[test]
fn data_get_text_format_renders_each_presence_branch() {
    // Render contract: with no --format, `data get` prints the human default for each
    // presence branch -- the raw value for a present leaf, a children placeholder for a
    // record identity node, and an absence marker for a missing path. The typed presence
    // and value assertions live in the JSON tests above.
    let (_project, dir) = seeded_project("data-get-text");
    let value_only = marrow(&["data", "get", &dir, "^counter(1).value"]);
    let children_only = marrow(&["data", "get", &dir, "^counter(1)"]);
    let absent = marrow(&["data", "get", &dir, "^counter(2).value"]);

    // A present leaf renders its stored value verbatim; the typed value (`b"42"`) is
    // asserted from `value_b64` in `data_get_reads_a_path_value_and_reports_absence`.
    assert_eq!(value_only.status.code(), Some(0), "{value_only:?}");
    let value_stdout = String::from_utf8(value_only.stdout).expect("utf8");
    assert!(value_stdout.contains("42"), "{value_stdout}");

    assert_eq!(children_only.status.code(), Some(0), "{children_only:?}");
    let children_stdout = String::from_utf8(children_only.stdout).expect("utf8");
    assert!(
        children_stdout.contains(CHILDREN_ONLY_TEXT_GOLDEN),
        "{children_stdout}"
    );

    assert_eq!(absent.status.code(), Some(0), "{absent:?}");
    let absent_stdout = String::from_utf8(absent.stdout).expect("utf8");
    assert!(
        absent_stdout.contains(ABSENT_TEXT_GOLDEN),
        "{absent_stdout}"
    );
}

#[test]
fn data_get_distinguishes_a_children_only_path_from_absent() {
    // `^counter(1)` is a record identity node: it has a `.value` child but no direct
    // value, so `get` must report children-only presence, distinct from absent.
    let (_project, dir) = seeded_project("data-get-children");
    let children = marrow(&["data", "get", "--format", "json", &dir, "^counter(1)"]);

    assert_eq!(children.status.code(), Some(0), "{children:?}");
    let value = json(children);
    assert_eq!(value["presence"], serde_json::json!("children_only"));
    assert_eq!(value["value_b64"], serde_json::Value::Null);
}

#[test]
fn data_get_and_integrity_on_an_unseeded_project_create_nothing() {
    let project = native_project("data-readonly");
    let dir = project.to_str().unwrap().to_string();
    let get = marrow(&["data", "get", "--format", "json", &dir, "^counter(1).value"]);
    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);
    // Read-only: no command may create the store file.
    let created = project.join(".data").join("marrow.redb").exists();

    // An absent path on an empty store is a successful, queryable absence.
    assert_eq!(get.status.code(), Some(0), "{get:?}");
    assert_eq!(json(get)["presence"], serde_json::json!("absent"));
    // Nothing to verify is healthy: no problems on the empty store.
    assert_eq!(integrity.status.code(), Some(0), "{integrity:?}");
    assert_eq!(json(integrity)["problems"], serde_json::json!([]));
    assert!(!created, "inspection must not create the store");
}
