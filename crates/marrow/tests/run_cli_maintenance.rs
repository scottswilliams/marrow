mod support;

use support::{marrow_sub, temp_project, write};

#[test]
fn maintenance_flag_gates_a_whole_root_drop() {
    // A whole managed-root drop (`delete ^books`) is maintenance work. The
    // ordinary `marrow run` cannot reach it (rejected with the maintenance code);
    // `marrow run --maintenance` opts in explicitly and performs the drop. A
    // native store carries the seed across the separate runs.
    let root = temp_project("run-maintenance", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20required title: string\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\n\
             pub fn drop_root()\n\
             \x20\x20\x20\x20delete ^books\n\n\
             pub fn countRecords()\n\
             \x20\x20\x20\x20var c = 0\n\
             \x20\x20\x20\x20for book in ^books\n\
             \x20\x20\x20\x20\x20\x20\x20\x20c = c + 1\n\
             \x20\x20\x20\x20print($\"count={c}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();

    let seed = marrow_sub("run", &["--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    // Default run cannot drop the whole root.
    let denied = marrow_sub("run", &["--entry", "app::drop_root", &dir]);
    assert_eq!(denied.status.code(), Some(1), "denied: {denied:?}");
    let denied_err = String::from_utf8(denied.stderr).expect("stderr utf8");
    assert!(
        denied_err.contains("write.requires_maintenance"),
        "{denied_err}"
    );

    // Explicit maintenance opt-in performs the drop.
    let allowed = marrow_sub("run", &["--maintenance", "--entry", "app::drop_root", &dir]);
    assert_eq!(allowed.status.code(), Some(0), "allowed: {allowed:?}");

    // After the drop, no records remain.
    let after = marrow_sub("run", &["--entry", "app::countRecords", &dir]);
    assert_eq!(after.status.code(), Some(0), "count: {after:?}");
    let after_out = String::from_utf8(after.stdout).expect("stdout utf8");
    assert_eq!(after_out, "count=0\n");
}

#[test]
fn maintenance_flag_appears_in_help() {
    let output = marrow_sub("run", &["--help"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("--maintenance"), "{stdout}");
    assert!(stdout.contains("data evolution"), "{stdout}");
}
