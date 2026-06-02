use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::check_project;
use marrow_project::parse_config;
use marrow_schema::NodeKind;

const LIBRARY_SOURCE: &str = include_str!("../../../fixtures/v01/library.mw");

struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    TempProject { root }
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn config() -> marrow_project::ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

#[test]
fn v01_library_fixture_checks_clean_and_exposes_store_identity_refs() {
    let root = temp_project("v01-library-check", |root| {
        write(root, "src/v01/library.mw", LIBRARY_SOURCE);
    });
    let (report, program) = check_project(root.path(), &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let module = program
        .modules
        .iter()
        .find(|module| module.name == "v01::library")
        .expect("v01 library module");
    assert!(
        module
            .resources
            .iter()
            .any(|resource| resource.name == "Author"),
        "Author resource"
    );
    let book = module
        .resources
        .iter()
        .find(|resource| resource.name == "Book")
        .expect("Book resource");

    assert_eq!(
        book.field_type(&["author"])
            .expect("author field")
            .to_string(),
        "Id(^authors)"
    );
    let author = book
        .members
        .iter()
        .find(|member| member.name == "author")
        .expect("author member");
    assert!(
        matches!(author.kind, NodeKind::Slot { required: true, .. }),
        "author is the required relationship field"
    );
    assert_eq!(
        book.field_type(&["title"])
            .expect("title field")
            .to_string(),
        "string"
    );
    assert_eq!(
        book.leaf_type(&["tags"]).expect("tags leaf").to_string(),
        "string"
    );
}
