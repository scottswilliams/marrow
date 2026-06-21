//! `marrow data get`: reading one path's value, distinguishing a present value, a
//! children-only node, and a truly absent path. Presence and value are asserted as
//! typed JSON fields; a malformed path is asserted by its usage exit code. The
//! default human text output is pinned by a separate render-contract test.
use crate::support::{self, TempProject};
use crate::support_data;
use marrow_store::tree::TreeStore;

use support_data::{
    assert_stable_store_snapshot_eq, assert_store_snapshot, json, marrow, native_project,
    seeded_project,
};

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
    assert_store_snapshot(&present_json);
    let value = marrow_run::base64::decode(present_json["value_b64"].as_str().expect("b64"))
        .expect("decode value");
    assert_eq!(value, b"42");

    assert_eq!(absent.status.code(), Some(0), "{absent:?}");
    let absent_json = json(absent);
    assert_eq!(absent_json["presence"], serde_json::json!("absent"));
    assert_eq!(absent_json["value_b64"], serde_json::Value::Null);
    assert_store_snapshot(&absent_json);

    // A path that does not parse fails before touching the store: a usage error.
    assert_eq!(malformed.status.code(), Some(2), "{malformed:?}");
}

/// A string-keyed store seeded with three records: the plain key `abc` -> 100, a key
/// carrying every recognized string escape (`a\nb\tc\\d"e`) -> 200, and a key holding a
/// raw control char and a non-ASCII scalar (`k\x1b\u{00e9}z`) -> 300. Raw ESC and `é`
/// are `string_text`, so the language writes them literally rather than as escapes;
/// they expose any encoder that emits a Rust-debug `\u{NN}` the decoder cannot accept.
/// Together these prove the path-key decoder accepts exactly the language's five escapes
/// and that every path `dump` emits round-trips back through `get`.
fn escaped_key_project(name: &str) -> (TempProject, String) {
    let project = support::temp_project_uncommitted(name, |root| {
        support::write(root, "marrow.json", support::native_config());
        support::write(
            root,
            "src/app.mw",
            "module app\n\
             \n\
             resource Item\n\
             \x20\x20\x20\x20required v: int\n\
             store ^items(key: string): Item\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20var plain: Item\n\
             \x20\x20\x20\x20plain.v = 100\n\
             \x20\x20\x20\x20var escaped: Item\n\
             \x20\x20\x20\x20escaped.v = 200\n\
             \x20\x20\x20\x20var raw: Item\n\
             \x20\x20\x20\x20raw.v = 300\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^items(\"abc\") = plain\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^items(\"a\\nb\\tc\\\\d\\\"e\") = escaped\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^items(\"k\x1b\u{00e9}z\") = raw\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    (project, dir)
}

#[test]
fn data_get_rejects_an_unrecognized_string_key_escape() {
    // The saved-path string-key decoder shares the language's five-escape vocabulary
    // (`\\ \" \n \r \t`). An unknown escape such as `\b` must fail closed with a
    // malformed-path usage error, never silently strip the backslash and resolve a
    // different key -- which would let `^items("a\bc")` read the value stored at `abc`.
    let (_project, dir) = escaped_key_project("data-get-bad-escape");
    for path in [
        r#"^items("a\bc").v"#,
        r#"^items("\a").v"#,
        r#"^items("a\qbc").v"#,
    ] {
        let rejected = marrow(&["data", "get", &dir, path]);
        assert_eq!(rejected.status.code(), Some(2), "{path}: {rejected:?}");
    }
}

#[test]
fn data_get_decodes_the_recognized_string_key_escapes() {
    // The five recognized escapes decode to the seeded key `a\nb\tc\\d"e`, so `get`
    // resolves the record stored under that key and reads its value.
    let (_project, dir) = escaped_key_project("data-get-good-escape");
    let escaped = marrow(&[
        "data",
        "get",
        "--format",
        "json",
        &dir,
        r#"^items("a\nb\tc\\d\"e").v"#,
    ]);

    assert_eq!(escaped.status.code(), Some(0), "{escaped:?}");
    let escaped = json(escaped);
    assert_eq!(escaped["presence"], serde_json::json!("value_only"));
    let value =
        marrow_run::base64::decode(escaped["value_b64"].as_str().expect("b64")).expect("decode");
    assert_eq!(value, b"200");
}

#[test]
fn data_get_resolves_a_plain_string_key_and_round_trips_a_dumped_path() {
    // A plain quoted key resolves, and every path `data dump` emits re-parses through
    // `get` to the same value: the decoder and the dump renderer agree on one grammar.
    let (_project, dir) = escaped_key_project("data-get-round-trip");

    let plain = marrow(&[
        "data",
        "get",
        "--format",
        "json",
        &dir,
        r#"^items("abc").v"#,
    ]);
    assert_eq!(plain.status.code(), Some(0), "{plain:?}");
    let plain = json(plain);
    let plain_value =
        marrow_run::base64::decode(plain["value_b64"].as_str().expect("b64")).expect("decode");
    assert_eq!(plain_value, b"100");

    let dump = marrow(&["data", "dump", &dir]);
    assert_eq!(dump.status.code(), Some(0), "{dump:?}");
    let dump_text = String::from_utf8(dump.stdout).expect("utf8");

    // The raw control char and non-ASCII scalar are `string_text`, so the renderer must
    // emit them literally; a Rust-debug `\u{1b}` spelling would not re-parse through the
    // five-escape key decoder and would silently break the loop below.
    assert!(
        dump_text.contains("^items(\"k\x1b\u{00e9}z\").v"),
        "dump must spell the raw-control-char key as literal string_text, got:\n{dump_text}"
    );
    assert!(
        !dump_text.contains("\\u{"),
        "dump must not emit Rust-debug escapes, got:\n{dump_text}"
    );

    for line in dump_text.lines() {
        let Some((path, value)) = line.split_once('\t') else {
            continue;
        };
        let got = marrow(&["data", "get", "--format", "json", &dir, path]);
        assert_eq!(got.status.code(), Some(0), "{path}: {got:?}");
        let got = json(got);
        assert_eq!(got["presence"], serde_json::json!("value_only"), "{path}");
        let got_value =
            marrow_run::base64::decode(got["value_b64"].as_str().expect("b64")).expect("decode");
        assert_eq!(
            String::from_utf8(got_value).expect("utf8"),
            value,
            "dumped path {path} did not round-trip"
        );
    }
}

#[test]
fn data_get_reads_backup_while_live_store_is_locked() {
    let (project, dir) = seeded_project("data-get-backup");
    let archive = support::backup_artifact(&project, "counter.mwbackup");
    let archive_arg = archive.to_str().expect("backup path utf8");

    let live = support::marrow(&["data", "get", "--format", "json", &dir, "^counter(1).value"]);
    assert_eq!(live.status.code(), Some(0), "{live:?}");
    let live = support::json(live.stdout);

    let _writer = TreeStore::open(&project.join(".data").join("marrow.redb"))
        .expect("hold the native writer open");
    let backup = support::marrow(&[
        "data",
        "get",
        "--backup",
        archive_arg,
        "--format",
        "json",
        &dir,
        "^counter(1).value",
    ]);

    assert_eq!(backup.status.code(), Some(0), "{backup:?}");
    let backup = support::json(backup.stdout);
    assert_eq!(backup["path"], live["path"]);
    assert_eq!(backup["presence"], live["presence"]);
    assert_eq!(backup["value_b64"], live["value_b64"]);
    assert_stable_store_snapshot_eq(&backup, &live);
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
fn data_get_and_integrity_on_an_unseeded_project_write_no_records() {
    // The data harness freezes the clean project before invoking the command, so the
    // command observes a committed empty store and never writes a record of its own.
    let project = native_project("data-readonly");
    let dir = project.to_str().unwrap().to_string();
    let get = marrow(&["data", "get", "--format", "json", &dir, "^counter(1).value"]);
    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);
    let stats = marrow(&["data", "stats", "--format", "json", &dir]);

    // An absent path on an empty store is a successful, queryable absence.
    assert_eq!(get.status.code(), Some(0), "{get:?}");
    let get = json(get);
    assert_eq!(get["presence"], serde_json::json!("absent"));
    assert_store_snapshot(&get);
    // Nothing to verify is healthy: no problems on the empty store.
    assert_eq!(integrity.status.code(), Some(0), "{integrity:?}");
    assert_eq!(json(integrity)["problems"], serde_json::json!([]));
    // Inspection writes no cells: the store holds zero saved entities and cells.
    assert_eq!(stats.status.code(), Some(0), "{stats:?}");
    let stats_json = json(stats);
    assert_eq!(stats_json["records"], serde_json::json!(0));
    assert_eq!(stats_json["cells"], serde_json::json!(0));
}

#[test]
fn data_get_on_a_memory_backed_durable_project_is_a_check_error() {
    // A durable surface under the `memory` backend has no durable identity, so the
    // project does not check. `data get` checks the project first and reports the typed
    // error rather than inspecting a store that could never exist.
    let project = support::temp_project("data-get-memory-store", |root| {
        support::write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        support::write(root, "src/app.mw", support::counter_source());
    });
    let dir = project.to_str().unwrap().to_string();

    let get = marrow(&["data", "get", "--format", "json", &dir, "^counter(1).value"]);

    assert_eq!(get.status.code(), Some(1), "{get:?}");
    let get = json(get);
    assert_eq!(get["status"], serde_json::json!("failed"));
    assert_eq!(
        get["diagnostics"][0]["code"],
        serde_json::json!("check.durable_store_required")
    );
}

#[test]
fn data_get_pending_member_reports_null_snapshot_with_native_store() {
    let (project, dir) = seeded_project("data-get-pending-member");
    support::write(
        &project,
        "src/app.mw",
        "module app\n\
         \n\
         resource Counter\n\
         \x20\x20\x20\x20required value: int\n\
         \x20\x20\x20\x20bonus: int\n\
         store ^counter(id: int): Counter\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var c: Counter\n\
         \x20\x20\x20\x20c.value = 42\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n",
    );

    let get = support::marrow(&["data", "get", "--format", "json", &dir, "^counter(1).bonus"]);

    assert_eq!(get.status.code(), Some(0), "{get:?}");
    let get = support::json(get.stdout);
    assert_eq!(get["presence"], serde_json::json!("absent"));
    assert_eq!(get["value_b64"], serde_json::Value::Null);
    assert_eq!(get["store_snapshot"], serde_json::Value::Null);
}
