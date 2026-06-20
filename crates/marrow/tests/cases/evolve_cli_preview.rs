use std::fs;
use std::path::Path;

use crate::support;
use crate::support_evolve;
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;
use support::{marrow, write};
use support_evolve::{
    REQUIRED_BASELINE_SOURCE, REQUIRED_DEFAULT_SOURCE, REQUIRED_NO_DEFAULT_SOURCE, commit_catalog,
    member_catalog_id, native_books_project, native_store_path, open_native_store, root_place,
    seed_member, seed_title_only, store_catalog_id,
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

    // Drift the committed reference (the lock) so it disagrees with the backup's catalog
    // section. The preview-from-backup mount reads the committed lock as the current
    // reference and refuses a backup that does not match it.
    let committed = marrow_check::read_committed_lock(&root)
        .expect("read committed lock")
        .expect("project has a committed lock");
    let drifted = marrow_catalog::CatalogLock::new(
        committed.entries.clone(),
        committed.ledger.clone(),
        committed.epoch_high_water + 1,
        committed.source_digest.clone(),
    )
    .expect("drifted lock builds");
    fs::write(
        root.join("marrow.lock"),
        drifted.to_lock_json_pretty().expect("lock renders"),
    )
    .expect("write drifted committed lock");

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
fn evolve_preview_scaffold_spells_a_retired_store_root_with_a_single_caret()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-preview-scaffold-store-root",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         resource Shelf\n\
         \x20   required name: string\n\
         store ^books(id: int): Book\n\
         store ^shelves(id: int): Shelf\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let shelves_place = root_place(&accepted, "shelves")?;
    let shelves_store_id = store_catalog_id(&shelves_place)?;
    let shelves_store_id = shelves_store_id.as_str();
    {
        let store = open_native_store(&root);
        seed_member(
            &store,
            &shelves_place,
            1,
            "name",
            Scalar::Str("Fiction".into()),
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
         \x20   retire ^shelves\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let output = marrow(&["evolve", "preview", "--scaffold", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let scaffold = String::from_utf8(output.stdout).expect("scaffold utf8");
    let parsed = marrow_syntax::parse_source(&scaffold);
    assert!(
        !parsed.has_errors(),
        "store-root retire scaffold must parse through the production parser: {:#?}\n{scaffold}",
        parsed.diagnostics
    );
    assert_eq!(
        scaffold,
        marrow_syntax::format_source(&scaffold),
        "scaffold should already be in production formatter shape"
    );
    // The store root carries its caret inside the catalog path segment, so the scaffold
    // target must read `^shelves` exactly once; a doubled caret would re-spell the root.
    assert!(
        scaffold.contains("evolve\n    retire ^shelves"),
        "store-root retire must be spelled with a single caret: {scaffold}"
    );
    assert!(
        !scaffold.contains("^^"),
        "store-root retire must not double the caret: {scaffold}"
    );
    assert!(
        scaffold.contains(&format!(
            "; approve with marrow evolve apply --maintenance --approve-retire {shelves_store_id}:1 (--backup <backup-file> | --no-backup)"
        )),
        "store-root retire scaffold should name the exact approval count and recovery choice: {scaffold}"
    );

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
        scaffold.contains("evolve\n    transform Book.price\n        return \"\""),
        "type-change repair should get a type-correct transform skeleton for the new \
         string member: {scaffold}"
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

/// A nested-module project (`shop::books`) whose source file lives at the matching
/// `src/shop/books.mw` path. A multi-segment module catalog path exposes whether the
/// scaffold strips the whole module prefix or only its first segment.
fn nested_books_project(name: &str, source: &str) -> support::TempProject {
    support::temp_project_uncommitted(name, |root: &Path| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/shop/books.mw", source);
    })
}

/// The evolve blocks a scaffold emits, spliced into source and fed back through the
/// production checker, must name targets the checker resolves and type-correct defaults:
/// no `check.evolve_target` and no `check.evolve_type`. This is the ready-to-paste
/// contract the CLI map promises, across a nested module, the default and retire
/// families, and int plus several non-int leaf types.
#[test]
fn evolve_preview_scaffold_round_trips_through_the_checker()
-> Result<(), Box<dyn std::error::Error>> {
    let baseline = "module shop::books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required cost: int\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n";
    let root = nested_books_project("evolve-preview-scaffold-roundtrip", baseline);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
        seed_member(&store, &place, 1, "cost", Scalar::Int(7));
        seed_member(
            &store,
            &place,
            1,
            "subtitle",
            Scalar::Str("Appendix".into()),
        );
    }

    // Add required leaves across int and several non-int types (default family), retype the
    // populated `cost` from int to decimal (transform family), and drop the populated
    // `subtitle` (retire family), so the scaffold emits all three across non-int leaf types.
    let evolved = "module shop::books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required cost: decimal\n\
         \x20   required pages: int\n\
         \x20   required edition: string\n\
         \x20   required hardcover: bool\n\
         \x20   required price: decimal\n\
         \x20   required published: date\n\
         store ^books(id: int): Book\n";
    write(&root, "src/shop/books.mw", evolved);

    let output = marrow(&["evolve", "preview", "--scaffold", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let scaffold = String::from_utf8(output.stdout).expect("scaffold utf8");
    assert!(
        scaffold.contains("default ")
            && scaffold.contains("transform ")
            && scaffold.contains("retire "),
        "round-trip must exercise the default, transform, and retire families: {scaffold}"
    );

    // Splice every emitted evolve block into the source, the way a developer pastes the
    // scaffold, then re-check through the production pipeline.
    write(&root, "src/shop/books.mw", &format!("{evolved}{scaffold}"));
    let check = marrow(&["check", root.to_str().unwrap()]);
    let stderr = String::from_utf8(check.stderr).expect("check stderr utf8");
    for code in [
        "check.evolve_target",
        "check.evolve_type",
        "check.evolve_transform",
        "check.return_type",
    ] {
        assert!(
            !stderr.contains(code),
            "pasted scaffold must check clean, found {code}: {stderr}\n--- pasted ---\n{evolved}{scaffold}"
        );
    }

    Ok(())
}
