use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use marrow_check::{
    CheckedProgram, CheckedSavedMemberKind, CheckedSavedPlace, ProjectConfig, check_project,
    checked_saved_root_place,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, ScalarType, decode_value, encode_value};

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn marrow(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

fn config(root: &Path) -> ProjectConfig {
    let text = fs::read_to_string(root.join("marrow.json")).expect("read config");
    marrow_project::parse_config(&text).expect("parse config")
}

fn accept_catalog(root: &Path) -> CheckedProgram {
    let config = config(root);
    let (report, program) = check_project(root, &config).expect("check for proposal");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let (report, program) = marrow_check::accept_catalog_proposal(root, &config, &program)
        .expect("accept catalog")
        .expect("a catalog proposal to accept");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

fn native_store_path(root: &Path) -> PathBuf {
    root.join(".data").join("marrow.redb")
}

fn open_native_store(root: &Path) -> TreeStore {
    let path = native_store_path(root);
    fs::create_dir_all(path.parent().unwrap()).expect("create data dir");
    TreeStore::open(&path).expect("open native store")
}

fn root_place(program: &CheckedProgram, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

fn member_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked member `{name}`"))
        .catalog_id
        .clone()
}

fn seed_title_only(store: &TreeStore, place: &CheckedSavedPlace, id: i64, title: &str) {
    let store_id = CatalogId::new(place.store_catalog_id.clone()).expect("store catalog id");
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
    let store_id = CatalogId::new(place.store_catalog_id.clone()).expect("store catalog id");
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
    let store_id = CatalogId::new(place.store_catalog_id.clone()).expect("store catalog id");
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

fn native_books_project(name: &str, source: &str) -> PathBuf {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
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

#[test]
fn catalog_preview_is_read_only_and_reports_the_proposal() {
    let root = temp_project("catalog-preview", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n",
        );
    });

    let output = marrow(&["catalog", "preview", root.to_str().unwrap()]);
    let catalog_path = root.join("marrow.catalog.json");
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("proposal epoch: 1"), "{stdout}");
    assert!(stdout.contains("entries:"), "{stdout}");
    assert!(
        !catalog_path.exists(),
        "preview must not write the accepted catalog"
    );
}

#[test]
fn catalog_accept_writes_the_exact_current_proposal() {
    let root = temp_project("catalog-accept", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n",
        );
    });

    let output = marrow(&["catalog", "accept", root.to_str().unwrap()]);
    let catalog = fs::read_to_string(root.join("marrow.catalog.json")).expect("accepted catalog");
    let check = marrow(&["check", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("accepted catalog epoch 1"), "{stdout}");
    assert!(catalog.contains("\"epoch\": 1"), "{catalog}");
    assert_eq!(check.status.code(), Some(0), "{check:?}");
}

#[test]
fn check_data_reports_repair_required_from_attached_store() {
    let root = native_books_project("check-data-repair", REQUIRED_NO_DEFAULT_SOURCE);
    let program = accept_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["check", "--data", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("evolve.repair_required"), "{stderr}");
    assert!(
        stderr.contains("Book.pages") || stderr.contains("pages"),
        "{stderr}"
    );
}

#[test]
fn evolve_preview_reports_the_exact_witness_counts() {
    let root = native_books_project("evolve-preview-default", REQUIRED_DEFAULT_SOURCE);
    let program = accept_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["evolve", "preview", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

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
    let program = accept_catalog(&root);
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
    fs::remove_dir_all(&root).ok();

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
fn evolve_apply_rejects_repair_required_witness() {
    let root = native_books_project("evolve-apply-repair", REQUIRED_NO_DEFAULT_SOURCE);
    let program = accept_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages = read_scalar(&store, &place, 1, "pages", ScalarType::Int);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("evolve.repair_required"), "{stderr}");
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
    let accepted = accept_catalog(&root);
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
    fs::remove_dir_all(&root).ok();

    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let stdout = String::from_utf8(json.stdout).expect("stdout");
    assert!(stdout.contains("\"status\":\"blocked\""), "{stdout}");
    assert!(stdout.contains("\"evolve.approval_required\""), "{stdout}");
    assert!(stdout.contains("\"catalog_id\""), "{stdout}");
    assert!(stdout.contains("\"populated\""), "{stdout}");
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
    let accepted = accept_catalog(&root);
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
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("records retired: 2"), "{stdout}");
    assert_eq!(subtitle_present, None, "subtitle was retired");
    assert_eq!(notes_present, None, "notes was retired");
}

#[test]
fn legacy_evolution_subcommands_are_absent() {
    let root = native_books_project("evolve-legacy", REQUIRED_DEFAULT_SOURCE);

    let output = marrow(&["evolve", "migrate", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown evolve subcommand"), "{stderr}");
    assert!(
        stderr.contains("preview") && stderr.contains("apply"),
        "{stderr}"
    );
}
