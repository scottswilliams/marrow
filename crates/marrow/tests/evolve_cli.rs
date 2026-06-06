use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{
    CheckedProgram, CheckedSavedMemberKind, CheckedSavedPlace, ProjectConfig, check_project,
    checked_saved_root_place,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, ScalarType, decode_value, encode_value};

mod support;

use support::{TempProject, marrow, temp_project_uncommitted as temp_project, write};

fn config(root: impl AsRef<Path>) -> ProjectConfig {
    let text = fs::read_to_string(root.as_ref().join("marrow.json")).expect("read config");
    marrow_project::parse_config(&text).expect("parse config")
}

fn commit_catalog(root: impl AsRef<Path>) -> CheckedProgram {
    let root = root.as_ref();
    let config = config(root);
    let (report, program) = check_project(root, &config).expect("check for proposal");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let (report, program) = marrow_check::commit_pending_identity(root, &config, &program)
        .expect("commit catalog")
        .expect("a catalog proposal to commit");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

fn native_store_path(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(".data").join("marrow.redb")
}

fn open_native_store(root: impl AsRef<Path>) -> TreeStore {
    let path = native_store_path(root);
    fs::create_dir_all(path.parent().unwrap()).expect("create data dir");
    TreeStore::open(&path).expect("open native store")
}

fn root_place(program: &CheckedProgram, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

fn member_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked member `{name}`"));
    accepted_catalog_id(&member.catalog_id, name)
}

fn store_catalog_id(place: &CheckedSavedPlace) -> CatalogId {
    CatalogId::new(accepted_catalog_id(&place.store_catalog_id, "store")).expect("store catalog id")
}

fn accepted_catalog_id(id: &Option<String>, label: &str) -> String {
    id.clone()
        .unwrap_or_else(|| panic!("accepted catalog id for `{label}`"))
}

fn seed_title_only(store: &TreeStore, place: &CheckedSavedPlace, id: i64, title: &str) {
    let store_id = store_catalog_id(place);
    store
        .write_node(&store_id, &[SavedKey::Int(id)])
        .expect("write record");
    let title_id = CatalogId::new(member_catalog_id(place, "title")).expect("title id");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(title_id)],
            encode_value(&Scalar::Str(title.to_string())).expect("encode title"),
        )
        .expect("write title");
}

fn seed_member(store: &TreeStore, place: &CheckedSavedPlace, id: i64, member: &str, value: Scalar) {
    let store_id = store_catalog_id(place);
    let member_id = CatalogId::new(member_catalog_id(place, member)).expect("member id");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member_id)],
            encode_value(&value).expect("encode member"),
        )
        .expect("write member");
}

fn read_scalar(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member: &str,
    ty: ScalarType,
) -> Option<Scalar> {
    let store_id = store_catalog_id(place);
    let member_id = CatalogId::new(member_catalog_id(place, member)).expect("member id");
    store
        .read_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member_id)],
        )
        .expect("read member")
        .map(|bytes| decode_value(&bytes, ty).expect("decode value"))
}

fn read_scalar_by_catalog_id(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member_id: &str,
    ty: ScalarType,
) -> Option<Scalar> {
    let store_id = store_catalog_id(place);
    store
        .read_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(
                CatalogId::new(member_id.to_string()).unwrap(),
            )],
        )
        .expect("read member")
        .map(|bytes| decode_value(&bytes, ty).expect("decode value"))
}

fn native_books_project(name: &str, source: &str) -> TempProject {
    temp_project(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/books.mw", source);
    })
}

const REQUIRED_DEFAULT_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   required pages: int\n\
evolve\n\
\x20   default Book.pages = 0\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

const REQUIRED_NO_DEFAULT_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   required pages: int\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

const REQUIRED_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

const OPTIONAL_PAGES_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   pages: int\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

const OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   required pages: int\n\
\x20   index byPages(pages, id)\n\
evolve\n\
\x20   default Book.pages = 0\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

const PRICE_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required price: int\n\
pub fn add(price: int): Id(^books)\n\
\x20   return nextId(^books)\n";

const PRICE_CENTS_TRANSFORM_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required price: int\n\
\x20   required priceCents: int\n\
evolve\n\
\x20   transform Book.priceCents\n\
\x20       return old.price * 100\n\
pub fn add(price: int): Id(^books)\n\
\x20   return nextId(^books)\n";

#[test]
fn check_data_reports_repair_required_from_attached_store() {
    let root = native_books_project("check-data-repair", REQUIRED_NO_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&[
        "check",
        "--data",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["code"], "evolve.repair_required");
    assert_eq!(
        record["data"]["catalog_id"],
        serde_json::json!(member_catalog_id(&place, "pages"))
    );
}

#[test]
fn evolve_preview_reports_the_exact_witness_counts() {
    let root = native_books_project("evolve-preview-default", REQUIRED_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["evolve", "preview", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("status: activatable"), "{stdout}");
    assert!(stdout.contains("records to backfill: 1"), "{stdout}");
    assert!(stdout.contains("source digest:"), "{stdout}");
    assert!(stdout.contains("accepted epoch:"), "{stdout}");
}

#[test]
fn evolve_apply_consumes_preview_witness_and_backfills() {
    let root = native_books_project("evolve-apply-default", REQUIRED_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages = read_scalar(&store, &place, 1, "pages", ScalarType::Int);
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("applied evolution"), "{stdout}");
    assert!(stdout.contains("records backfilled: 1"), "{stdout}");
    assert_eq!(pages, Some(Scalar::Int(0)));
    assert_eq!(
        commit.catalog_epoch,
        program.catalog.accepted_epoch.unwrap()
    );
}

#[test]
fn evolve_apply_backfills_proposal_required_default_before_accepting_catalog() {
    let root = native_books_project("evolve-apply-proposal-default", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("records backfilled: 2"), "{stdout}");

    let catalog_epoch = accepted_catalog(&root).epoch;
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    for id in [1, 2] {
        assert_eq!(
            read_scalar_by_catalog_id(&store, &accepted_place, id, &pages_id, ScalarType::Int),
            Some(Scalar::Int(0)),
            "pages backfilled before accepted catalog publication"
        );
    }
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp");
    let stamped_epoch = store.read_catalog_epoch().expect("store epoch");

    assert_eq!(catalog_epoch, baseline_epoch + 1);
    assert_eq!(commit.catalog_epoch, baseline_epoch + 1);
    assert_eq!(stamped_epoch, Some(baseline_epoch + 1));
}

#[test]
fn evolve_apply_resumes_proposal_default_after_store_commit() {
    let root = native_books_project(
        "evolve-apply-proposal-default-resume",
        REQUIRED_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");

    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch);

    let resume = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );
    let stdout = String::from_utf8(resume.stdout).expect("stdout utf8");
    assert!(stdout.contains("completed evolution"), "{stdout}");
    assert!(stdout.contains("records backfilled: 0"), "{stdout}");

    {
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        for id in [1, 2] {
            assert_eq!(
                read_scalar_by_catalog_id(&store, &accepted_place, id, &pages_id, ScalarType::Int),
                Some(Scalar::Int(0)),
                "resume must not lose the committed backfill"
            );
        }
    }
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch + 1);
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
}

#[test]
fn evolve_apply_resumes_existing_optional_default_with_preserved_value() {
    let root = native_books_project(
        "evolve-apply-existing-optional-default-resume",
        OPTIONAL_PAGES_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(&store, &accepted_place, 1, "pages", Scalar::Int(7));
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");

    let resume = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    let preserved =
        read_scalar_by_catalog_id(&store, &accepted_place, 1, &pages_id, ScalarType::Int);
    let defaulted =
        read_scalar_by_catalog_id(&store, &accepted_place, 2, &pages_id, ScalarType::Int);
    let catalog_epoch = accepted_catalog(&root).epoch;

    assert_eq!(preserved, Some(Scalar::Int(7)));
    assert_eq!(defaulted, Some(Scalar::Int(0)));
    assert_eq!(catalog_epoch, baseline_epoch + 1);
}

#[test]
fn evolve_apply_resumes_redundant_existing_optional_default_without_backfill() {
    let root = native_books_project(
        "evolve-apply-existing-optional-default-no-backfill-resume",
        OPTIONAL_PAGES_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(&store, &accepted_place, 1, "pages", Scalar::Int(7));
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
        seed_member(&store, &accepted_place, 2, "pages", Scalar::Int(9));
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");

    let resume = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    let first_pages =
        read_scalar_by_catalog_id(&store, &accepted_place, 1, &pages_id, ScalarType::Int);
    let second_pages =
        read_scalar_by_catalog_id(&store, &accepted_place, 2, &pages_id, ScalarType::Int);
    let catalog_epoch = accepted_catalog(&root).epoch;

    assert_eq!(resume.status.code(), Some(0), "{resume:?}");
    assert_eq!(first_pages, Some(Scalar::Int(7)));
    assert_eq!(second_pages, Some(Scalar::Int(9)));
    assert_eq!(catalog_epoch, baseline_epoch + 1);
}
#[test]
fn evolve_apply_resumes_proposal_transform_after_store_commit() {
    let root = native_books_project(
        "evolve-apply-proposal-transform-resume",
        PRICE_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        let store_id = store_catalog_id(&accepted_place);
        store
            .write_node(&store_id, &[SavedKey::Int(1)])
            .expect("write record");
        seed_member(&store, &accepted_place, 1, "price", Scalar::Int(3));
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", PRICE_CENTS_TRANSFORM_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let price_cents_id = accepted_catalog_entry_id(&root, "books::Book::priceCents");

    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    let resume = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let cents =
        read_scalar_by_catalog_id(&store, &accepted_place, 1, &price_cents_id, ScalarType::Int);
    let catalog_epoch = accepted_catalog(&root).epoch;

    assert_eq!(cents, Some(Scalar::Int(300)));
    assert_eq!(catalog_epoch, baseline_epoch + 1);
}
#[test]
fn evolve_apply_rejects_repair_required_witness() {
    let root = native_books_project("evolve-apply-repair", REQUIRED_NO_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages = read_scalar(&store, &place, 1, "pages", ScalarType::Int);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["code"], serde_json::json!("evolve.repair_required"));
    assert_eq!(pages, None, "repair-required apply must not write data");
}

#[test]
fn evolve_preview_reports_destructive_approval_requirement() {
    let root = native_books_project(
        "evolve-preview-retire",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
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
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let text = marrow(&["evolve", "preview", root.to_str().unwrap()]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let stderr = String::from_utf8(text.stderr).expect("stderr");
    assert!(stderr.contains("evolve.approval_required"), "{stderr}");
    assert!(stderr.contains("--approve-retire"), "{stderr}");

    let json = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);

    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let value = support::json(json.stdout);
    assert_eq!(value["status"], "blocked");
    let blocking = value["blocking"].as_array().expect("blocking reports");
    let report = blocking
        .iter()
        .find(|report| report["code"] == serde_json::json!("evolve.approval_required"))
        .unwrap_or_else(|| panic!("{value:#?}"));
    assert_eq!(
        report["data"]["catalog_id"],
        serde_json::json!(member_catalog_id(&accepted_place, "subtitle"))
    );
    assert_eq!(report["data"]["populated"], serde_json::json!(1));
}

#[test]
fn evolve_apply_accepts_two_repeated_approve_retire_flags() {
    let root = native_books_project(
        "evolve-apply-multi-retire",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   notes: string\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    let notes_id = member_catalog_id(&accepted_place, "notes");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
        seed_member(
            &store,
            &accepted_place,
            1,
            "notes",
            Scalar::Str("note".into()),
        );
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         \x20   retire Book.notes\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--approve-retire",
        &format!("{notes_id}:1"),
        root.to_str().unwrap(),
    ]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let subtitle_present = read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str);
    let notes_present = read_scalar(&store, &accepted_place, 1, "notes", ScalarType::Str);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("records retired: 2"), "{stdout}");
    assert_eq!(subtitle_present, None, "subtitle was retired");
    assert_eq!(notes_present, None, "notes was retired");
}

fn accepted_catalog(root: impl AsRef<Path>) -> marrow_project::CatalogMetadata {
    let json = fs::read_to_string(root.as_ref().join("marrow.catalog.json"))
        .expect("read accepted catalog");
    marrow_project::CatalogMetadata::from_json(&json).expect("parse accepted catalog")
}

fn accepted_catalog_entry_id(root: impl AsRef<Path>, path: &str) -> String {
    accepted_catalog(root)
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("accepted catalog entry `{path}`"))
        .stable_id
}

fn store_epoch(root: impl AsRef<Path>) -> Option<u64> {
    let store = TreeStore::open(&native_store_path(root)).expect("reopen native store");
    store.read_catalog_epoch().expect("read store epoch")
}

const RETIRE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
evolve\n\
\x20   retire Book.subtitle\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

const RETIRE_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   subtitle: string\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

const RENAME_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   blurb: string\n\
evolve\n\
\x20   rename Book.subtitle -> Book.blurb\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

// Baseline with a runnable zero-arg entry: a `subtitle` member a later rename/retire
// consumes, plus a `seed` that writes through the store so the fence is exercised.
const BLOCK_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   subtitle: string\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";

// The renamed source with the consumed rename block present.
const RENAME_BLOCK_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   blurb: string\n\
evolve\n\
\x20   rename Book.subtitle -> Book.blurb\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";

// The renamed source with the consumed rename block removed: the rename is already
// recorded in the accepted catalog, so the block is transient and safe to delete.
const RENAME_BLOCK_DELETED_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   blurb: string\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";

// The retired source with the consumed retire block present.
const RETIRE_BLOCK_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
evolve\n\
\x20   retire Book.subtitle\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";

// The retired source with the consumed retire block removed.
const RETIRE_BLOCK_DELETED_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";

#[test]
fn evolve_apply_advances_accepted_catalog_in_lockstep_for_retire() {
    let root = native_books_project("evolve-apply-retire-lockstep", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let file_epoch = accepted_catalog(&root).epoch;
    let store_epoch = store_epoch(&root);
    assert_eq!(
        store_epoch,
        Some(baseline_epoch + 1),
        "store advanced one epoch"
    );
    assert_eq!(
        file_epoch,
        baseline_epoch + 1,
        "accepted catalog file advanced in lockstep with the store"
    );

    // With the accepted file left behind the store epoch, the open fence rejects every
    // later run as `run.store_evolved` with no recovery; the lockstep advance keeps the
    // file and store at one epoch, so the fence never reports the store as evolved.
    let run = marrow(&["run", "--entry", "books::add", root.to_str().unwrap()]);
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after lockstep advance: {stderr}"
    );
}

#[test]
fn evolve_apply_advances_accepted_catalog_in_lockstep_for_rename() {
    let root = native_books_project("evolve-apply-rename-lockstep", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", RENAME_SOURCE);

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let catalog = accepted_catalog(&root);
    assert_eq!(
        catalog.epoch,
        baseline_epoch + 1,
        "file advanced in lockstep"
    );
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // The renamed member keeps its stable id, records the new path, and leaves
    // the old spelling as an alias rather than a live path.
    let blurb = catalog
        .entries
        .iter()
        .find(|entry| entry.path == "books::Book::blurb")
        .expect("renamed member recorded at its new path");
    assert_eq!(
        blurb.stable_id, subtitle_id,
        "rename preserves the stable id"
    );
    assert!(
        catalog
            .entries
            .iter()
            .all(|entry| entry.path != "books::Book::subtitle"),
        "old path is not left as a live spelling"
    );
    assert!(
        blurb
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::subtitle"),
        "old path survives as an alias"
    );

    let run = marrow(&["run", "--entry", "books::add", root.to_str().unwrap()]);
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after lockstep advance: {stderr}"
    );
}

// After a rename apply, the rename is recorded in the accepted catalog. The evolve
// block is a transient transition the author may keep or delete; neither choice may
// break `marrow run`. The store fences on the durable shape, which a consumed rename
// block does not change, and the consumed rename is treated as satisfied at check.
#[test]
fn run_succeeds_after_rename_apply_with_block_present_or_deleted() {
    let root = native_books_project("run-after-rename-block", BLOCK_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");

    // The baseline run stamps the store and writes record 2; a subtitle cell on that
    // stamped record gives the later rename real data to carry forward.
    let baseline = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        baseline.status.code(),
        Some(0),
        "baseline run: {baseline:?}"
    );
    {
        let store = open_native_store(&root);
        seed_member(
            &store,
            &accepted_place,
            2,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }

    write(&root, "src/books.mw", RENAME_BLOCK_SOURCE);
    let apply = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(apply.status.code(), Some(0), "rename apply: {apply:?}");

    let kept = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "run with the consumed rename block still present: {kept:?}"
    );

    write(&root, "src/books.mw", RENAME_BLOCK_DELETED_SOURCE);
    let deleted = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        deleted.status.code(),
        Some(0),
        "run after deleting the consumed rename block: {deleted:?}"
    );
}

// After a retire apply, the retire is recorded in the accepted catalog. The evolve
// block is transient; keeping or deleting it must not break `marrow run`.
#[test]
fn run_succeeds_after_retire_apply_with_block_present_or_deleted() {
    let root = native_books_project("run-after-retire-block", BLOCK_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");

    // The baseline run stamps the store and writes record 2; a subtitle cell on that
    // stamped record gives the later retire one populated cell to approve.
    let baseline = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        baseline.status.code(),
        Some(0),
        "baseline run: {baseline:?}"
    );
    {
        let store = open_native_store(&root);
        seed_member(
            &store,
            &accepted_place,
            2,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }

    write(&root, "src/books.mw", RETIRE_BLOCK_SOURCE);
    let apply = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(apply.status.code(), Some(0), "retire apply: {apply:?}");

    let kept = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "run with the consumed retire block still present: {kept:?}"
    );

    write(&root, "src/books.mw", RETIRE_BLOCK_DELETED_SOURCE);
    let deleted = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        deleted.status.code(),
        Some(0),
        "run after deleting the consumed retire block: {deleted:?}"
    );
}

#[test]
fn evolve_apply_resumes_a_half_applied_store_by_writing_the_file_only() {
    let root = native_books_project("evolve-apply-resume", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    // First apply advances both the store and the file.
    let first = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // Re-create the half-applied crash window: the store is stamped to the target
    // epoch, but the accepted file was never advanced (it still records the baseline).
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch);

    // The subtitle cell is already gone (the first apply deleted it), so a resume must
    // do no data re-apply.
    {
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        assert_eq!(
            read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
            None,
            "data was already retired by the first apply"
        );
    }

    let resume = marrow(&["evolve", "apply", "--maintenance", root.to_str().unwrap()]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );

    // Resuming completes the file side without re-applying data work.
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch + 1);
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
    let stdout = String::from_utf8(resume.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("records retired: 0"),
        "resume re-applies no data: {stdout}"
    );

    let run = marrow(&["run", "--entry", "books::add", root.to_str().unwrap()]);
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after resume completes the file: {stderr}"
    );
}
#[test]
fn evolve_apply_resume_fails_closed_when_source_diverges_from_the_store_commit() {
    // The half-applied crash window leaves the store at the target epoch while the file
    // still records the baseline. A resume completes by writing the file alone, but only
    // if the source still describes the evolution the store actually committed. Here the
    // store committed a retire, then the author rewrote the source to a divergent rename
    // before re-running apply. The rename proposes the same epoch the store holds, so the
    // epoch signature alone cannot tell the two apart; the schema-bearing source digest
    // can. Resume must refuse to freeze the rename catalog over the retire the store ran.
    let root = native_books_project("evolve-apply-resume-divergent", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    // First apply commits the retire to both the store and the file.
    let first = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // Re-create the crash window: the store stays at the retire epoch, the file is rewound
    // to the baseline, and the source is replaced with a divergent rename that proposes the
    // same epoch the store already holds.
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    write(&root, "src/books.mw", RENAME_SOURCE);

    let resume = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);

    // The file must remain at the baseline: the divergent rename catalog is never frozen.
    assert_eq!(
        accepted_catalog(&root).epoch,
        baseline_epoch,
        "the divergent rename catalog must not be frozen over the committed retire",
    );
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
    let code = resume.status.code();
    let record = support::json(resume.stdout);
    assert_eq!(code, Some(1), "resume fails closed: {code:?} {record}");
    assert_eq!(
        record["code"],
        serde_json::json!("run.schema_drift"),
        "resume reports schema drift against the committed shape"
    );
}

#[test]
fn evolve_apply_noop_when_store_and_file_already_at_target() {
    // A defaulting evolution that backfills one record, then applies a second time with
    // the store and file already at the target: the catalog shape is unchanged by a
    // backfill, so the proposal is identity-stable and the second apply must touch
    // neither the catalog file nor the commit id.
    let root = native_books_project("evolve-apply-noop", REQUIRED_DEFAULT_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");

    let path = root.join("marrow.catalog.json");
    let before = fs::read_to_string(&path).expect("read catalog");
    let before_commit = TreeStore::open(&native_store_path(&root))
        .expect("reopen")
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp")
        .commit_id;

    let second = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "no-op apply: {second:?}");

    let after = fs::read_to_string(&path).expect("read catalog");
    let after_commit = TreeStore::open(&native_store_path(&root))
        .expect("reopen")
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp")
        .commit_id;

    assert_eq!(before, after, "no-op apply does not churn the catalog file");
    assert_eq!(
        before_commit, after_commit,
        "no-op apply does not bump the commit id"
    );
}

#[test]
fn legacy_evolution_subcommands_are_absent() {
    let root = native_books_project("evolve-legacy", REQUIRED_DEFAULT_SOURCE);

    let output = marrow(&["evolve", "migrate", root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown evolve subcommand"), "{stderr}");
    assert!(
        stderr.contains("preview") && stderr.contains("apply"),
        "{stderr}"
    );
}
