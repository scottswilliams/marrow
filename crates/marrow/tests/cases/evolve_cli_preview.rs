use std::fs;

use crate::support;
use crate::support_evolve;
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;
use support::{marrow, write};
use support_evolve::{
    REQUIRED_BASELINE_SOURCE, REQUIRED_DEFAULT_SOURCE, REQUIRED_NO_DEFAULT_SOURCE,
    accepted_catalog, commit_catalog, member_catalog_id, native_books_project, native_store_path,
    open_native_store, root_place, seed_member, seed_title_only,
};

#[test]
fn evolve_preview_reports_the_exact_witness_counts() -> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-preview-default", REQUIRED_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let witness = support::json(output.stdout);
    assert_eq!(witness["kind"], serde_json::json!("evolve_preview"));
    assert_eq!(witness["status"], serde_json::json!("activatable"));
    assert_eq!(witness["records_to_backfill"], serde_json::json!(1));
    assert_eq!(
        witness["nothing_to_discharge"],
        serde_json::json!(false),
        "a preview with backfill work must not render as a no-work discharge: {witness}"
    );
    // The preview carries the schema-bearing source digest and the accepted epoch the
    // store would advance from: both are present facts, not just a rendered label.
    assert!(
        witness["source_digest"]
            .as_str()
            .is_some_and(|digest| !digest.is_empty()),
        "{witness}"
    );
    assert!(witness["accepted_epoch"].is_number(), "{witness}");

    Ok(())
}

#[test]
fn evolve_preview_from_backup_uses_backup_state_while_live_store_is_locked()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-preview-from-backup", REQUIRED_BASELINE_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }
    let archive = support::backup_artifact(&root, "before-pages.mwbackup");
    let archive_arg = archive.to_str().expect("backup path utf8");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 2, "Hyperion");
    }
    write(&root, "src/books.mw", REQUIRED_DEFAULT_SOURCE);

    let _writer = TreeStore::open(&native_store_path(&root)).expect("hold live store writer open");
    let output = marrow(&[
        "evolve",
        "preview",
        "--from-backup",
        archive_arg,
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let witness = support::json(output.stdout);
    assert_eq!(witness["status"], serde_json::json!("activatable"));
    assert_eq!(
        witness["records_to_backfill"],
        serde_json::json!(1),
        "preview must count the one backed-up record, not the two-record live store: {witness}"
    );

    Ok(())
}

#[test]
fn evolve_preview_from_backup_rejects_current_catalog_drift_with_restore_code()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-preview-backup-catalog-drift",
        REQUIRED_BASELINE_SOURCE,
    );
    let program = commit_catalog(&root);
    let place = root_place(&program, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }
    let archive = support::backup_artifact(&root, "baseline.mwbackup");
    let archive_arg = archive.to_str().expect("backup path utf8");
    let accepted = accepted_catalog(&root);
    let drifted = marrow_catalog::CatalogMetadata::new(accepted.epoch + 1, accepted.entries)
        .expect("catalog builds");
    fs::write(
        root.join("marrow.catalog.json"),
        drifted.to_json_pretty().expect("catalog renders"),
    )
    .expect("write drifted catalog artifact");

    let output = marrow(&[
        "evolve",
        "preview",
        "--from-backup",
        archive_arg,
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = support::json(output.stdout);
    assert_eq!(error["code"], serde_json::json!("restore.catalog_mismatch"));

    Ok(())
}

#[test]
fn evolve_preview_from_backup_flag_usage_is_tight() {
    let cases: &[(&[&str], &str)] = &[
        (
            &[
                "evolve",
                "preview",
                "--from-backup",
                "one.mwbackup",
                "--from-backup",
                "two.mwbackup",
                "proj",
            ],
            "duplicate --from-backup",
        ),
        (
            &["evolve", "preview", "--from-backup"],
            "missing value for --from-backup",
        ),
        (
            &[
                "evolve",
                "preview",
                "--from-backup",
                "state.mwbackup",
                "proj",
                "extra",
            ],
            "evolve preview accepts one project directory",
        ),
    ];

    for (args, expected) in cases {
        let output = marrow(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}

#[test]
fn evolve_preview_reports_repair_required_from_attached_store()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-preview-repair", REQUIRED_NO_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = support::json(output.stdout);
    assert_eq!(value["kind"], serde_json::json!("evolve_preview"));
    assert_eq!(value["status"], serde_json::json!("blocked"));
    let pages_id = member_catalog_id(&place, "pages")?;
    let blocking = value["blocking"].as_array().expect("blocking reports");
    assert!(
        blocking.iter().any(|report| {
            report["code"] == serde_json::json!("evolve.repair_required")
                && report["data"]["catalog_id"] == serde_json::json!(pages_id)
        }),
        "preview should report repair required for the attached store: {value:#?}"
    );

    Ok(())
}

#[test]
fn evolve_preview_reports_when_there_is_nothing_to_discharge() {
    let root = native_books_project("evolve-preview-no-work", REQUIRED_BASELINE_SOURCE);
    commit_catalog(&root);

    let text = marrow(&["evolve", "preview", root.to_str().unwrap()]);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    let stdout = String::from_utf8(text.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("nothing to discharge"),
        "text preview must make the no-work discharge explicit: {stdout}"
    );

    let json = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    let value = support::json(json.stdout);
    assert_eq!(value["status"], serde_json::json!("activatable"));
    assert_eq!(value["records_to_backfill"], serde_json::json!(0));
    assert_eq!(value["records_to_transform"], serde_json::json!(0));
    assert_eq!(
        value["nothing_to_discharge"],
        serde_json::json!(true),
        "JSON preview must carry the explicit no-work boolean: {value}"
    );
}

#[test]
fn evolve_preview_renders_a_store_open_failure_through_the_selected_format() {
    // `evolve preview` opens the configured store read-only. A store that cannot be
    // opened is a store fault, and under `--format json` it must render as a JSON
    // error envelope on stdout carrying the store code, not hard-coded text on
    // stderr. Otherwise a machine consumer parsing stdout as JSON sees nothing.
    let root = native_books_project("evolve-preview-store-open", REQUIRED_DEFAULT_SOURCE);
    commit_catalog(&root);
    // Corrupt the native store file so opening it for inspection fails.
    let store_path = native_store_path(&root);
    fs::create_dir_all(store_path.parent().unwrap()).expect("create data dir");
    fs::write(&store_path, b"not a redb database").expect("write corrupt store");

    let output = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");

    // The failure renders as JSON on stdout, not as raw text on stderr.
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("store-open failure must be JSON on stdout");
    assert_eq!(
        value["kind"], "storage",
        "a store-open failure must carry the storage kind: {value}"
    );
    assert!(
        value["code"]
            .as_str()
            .is_some_and(|code| code.starts_with("store.")),
        "the error must carry a store code: {value}"
    );
}

#[test]
fn evolve_preview_reports_destructive_approval_requirement()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-preview-retire",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("Appendix".into()),
        );
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    // The remediation hint a blocked text-format preview renders on stderr: the typed
    // code plus the maintenance invocation a human runs to approve the retire. The code
    // is the typed oracle; this golden pins only the human guidance that has no
    // structured form.
    const APPROVE_RETIRE_HINT: &str = "rerun with --maintenance --approve-retire";

    let text = marrow(&["evolve", "preview", root.to_str().unwrap()]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let stderr = String::from_utf8(text.stderr).expect("stderr");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    // A blocked text-format preview renders the typed code on the blocking-obligation
    // stream (stderr); the preview body itself stays on stdout.
    assert!(stderr.contains("evolve.approval_required"), "{stderr}");
    assert!(
        stderr.contains(&format!("catalog id {subtitle_id} (books.Book.subtitle)")),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("--approve-retire {subtitle_id}:1")),
        "{stderr}"
    );
    assert!(
        stderr.contains("--backup <backup-file>") && stderr.contains("--no-backup"),
        "retire approval guidance must include the recovery choice: {stderr}"
    );
    assert!(
        !String::from_utf8(text.stdout)
            .expect("stdout")
            .contains("evolve.approval_required"),
        "the blocking report belongs on stderr, not stdout"
    );
    assert!(stderr.contains(APPROVE_RETIRE_HINT), "{stderr}");
    assert!(
        stderr.contains("marrow evolve preview --scaffold"),
        "blocked preview should point at the scaffold command: {stderr}"
    );

    let json = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let value = support::json(json.stdout);
    assert_eq!(value["status"], "blocked");
    let blocking = value["blocking"].as_array().expect("blocking reports");
    let report = blocking
        .iter()
        .find(|report| report["code"] == serde_json::json!("evolve.approval_required"))
        .unwrap_or_else(|| panic!("{value:#?}"));
    assert_eq!(report["data"]["catalog_id"], serde_json::json!(subtitle_id));
    assert!(
        report["message"]
            .as_str()
            .is_some_and(|message| message.contains("(books.Book.subtitle)")),
        "{report:#?}"
    );
    assert_eq!(report["data"]["populated"], serde_json::json!(1));

    Ok(())
}

#[test]
fn evolve_preview_scaffold_emits_parseable_formatted_evolve_blocks()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-preview-scaffold",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   required price: int\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("Appendix".into()),
        );
        seed_member(&store, &accepted_place, 1, "price", Scalar::Int(3));
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         \x20   required price: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let output = marrow(&["evolve", "preview", "--scaffold", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let scaffold = String::from_utf8(output.stdout).expect("scaffold utf8");
    let parsed = marrow_syntax::parse_source(&scaffold);
    assert!(
        !parsed.has_errors(),
        "scaffold must parse through the production parser: {:#?}\n{scaffold}",
        parsed.diagnostics
    );
    assert_eq!(
        scaffold,
        marrow_syntax::format_source(&scaffold),
        "scaffold should already be in production formatter shape"
    );
    assert!(
        scaffold.contains("evolve\n    retire Book.subtitle"),
        "retire block should be ready to paste: {scaffold}"
    );
    assert!(
        scaffold.contains(&format!(
            "; approve with marrow evolve apply --maintenance --approve-retire {subtitle_id}:1 (--backup <backup-file> | --no-backup)"
        )),
        "retire scaffold should name the exact approval count and recovery choice: {scaffold}"
    );
    assert!(
        scaffold.contains("evolve\n    default Book.pages = 0"),
        "missing required member should get a parseable default skeleton: {scaffold}"
    );
    assert!(
        scaffold.contains("evolve\n    transform Book.price\n        return 0"),
        "type-change repair should get a parseable transform skeleton: {scaffold}"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/books.mw")).expect("read source"),
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         \x20   required price: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
        "--scaffold must not edit source"
    );

    Ok(())
}
