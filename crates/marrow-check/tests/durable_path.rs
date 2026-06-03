use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use marrow_check::{PathSegment, StorePathClass, check_project, classify_store_path};
use marrow_project::parse_config;
use marrow_store::key::SavedKey;
use marrow_store::value::ScalarType;

static NEXT_TEMP_PROJECT: AtomicU64 = AtomicU64::new(0);

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let serial = NEXT_TEMP_PROJECT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "marrow-{name}-{}-{nanos}-{serial}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn checked_program(source: &str) -> marrow_check::CheckedProgram {
    let root = temp_project("durable-path", |root| {
        write(root, "src/app.mw", source);
    });
    let config = parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config");
    let (report, program) = check_project(&root, &config).expect("check project");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

#[test]
fn classifies_store_paths_from_checked_durable_facts() {
    let program = checked_program(
        "module app\n\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20shelf: string\n\
         \x20\x20\x20\x20tags(pos: int): string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         \x20\x20\x20\x20index byShelf(shelf, id)\n",
    );

    let field = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("title".into()),
    ];
    assert_eq!(
        classify_store_path(&program, &field),
        StorePathClass::Scalar(ScalarType::Str)
    );

    let leaf_layer = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("tags".into()),
        PathSegment::IndexKey(SavedKey::Int(0)),
    ];
    assert_eq!(
        classify_store_path(&program, &leaf_layer),
        StorePathClass::Scalar(ScalarType::Str)
    );

    let nested = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("versions".into()),
        PathSegment::IndexKey(SavedKey::Int(2)),
        PathSegment::Field("note".into()),
    ];
    assert_eq!(
        classify_store_path(&program, &nested),
        StorePathClass::Scalar(ScalarType::Str)
    );

    let index_marker = vec![
        PathSegment::Root("books".into()),
        PathSegment::Field("byShelf".into()),
        PathSegment::IndexKey(SavedKey::Str("A".into())),
        PathSegment::IndexKey(SavedKey::Int(1)),
    ];
    assert_eq!(
        classify_store_path(&program, &index_marker),
        StorePathClass::IndexMarker
    );

    let unknown_root = vec![
        PathSegment::Root("ghosts".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("title".into()),
    ];
    assert_eq!(
        classify_store_path(&program, &unknown_root),
        StorePathClass::Orphan
    );

    let unknown_field = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("nope".into()),
    ];
    assert_eq!(
        classify_store_path(&program, &unknown_field),
        StorePathClass::Orphan
    );
}

#[test]
fn classifies_wrong_typed_record_keys_as_key_type_mismatches() {
    let program = checked_program(
        "module app\n\n\
         resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n",
    );
    let bad_key = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Str("oops".into())),
        PathSegment::Field("title".into()),
    ];
    assert_eq!(
        classify_store_path(&program, &bad_key),
        StorePathClass::KeyTypeMismatch {
            expected: ScalarType::Int,
            found: ScalarType::Str,
        }
    );
}

#[test]
fn classifies_identity_leaves_by_the_referenced_store() {
    let program = checked_program(
        "module app\n\n\
         resource Author\n\
         \x20   name: string\n\
         \n\
         store ^authors(id: int): Author\n\
         store ^archivedAuthors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   author: Id(^authors)\n\
         \n\
         store ^books(id: int): Book\n",
    );
    let author_field = vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::Field("author".into()),
    ];

    assert_eq!(
        classify_store_path(&program, &author_field),
        StorePathClass::Identity {
            store_root: "authors".into(),
            arity: 1,
        }
    );
}
