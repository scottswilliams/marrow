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

use crate::support::{TempProject, temp_project_uncommitted as temp_project, write};

#[allow(dead_code)]
fn config(root: impl AsRef<Path>) -> ProjectConfig {
    let text = fs::read_to_string(root.as_ref().join("marrow.json")).expect("read config");
    marrow_project::parse_config(&text).expect("parse config")
}

#[allow(dead_code)]
pub(crate) fn commit_catalog(root: impl AsRef<Path>) -> CheckedProgram {
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

#[allow(dead_code)]
pub(crate) fn native_store_path(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(".data").join("marrow.redb")
}

#[allow(dead_code)]
pub(crate) fn open_native_store(root: impl AsRef<Path>) -> TreeStore {
    let path = native_store_path(root);
    fs::create_dir_all(path.parent().unwrap()).expect("create data dir");
    TreeStore::open(&path).expect("open native store")
}

#[allow(dead_code)]
pub(crate) fn root_place(program: &CheckedProgram, root: &str) -> CheckedSavedPlace {
    checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

#[allow(dead_code)]
pub(crate) fn member_catalog_id(place: &CheckedSavedPlace, name: &str) -> String {
    let member = place
        .root_members
        .iter()
        .find(|member| {
            member.name == name && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        })
        .unwrap_or_else(|| panic!("checked member `{name}`"));
    accepted_catalog_id(&member.catalog_id, name)
}

#[allow(dead_code)]
pub(crate) fn store_catalog_id(place: &CheckedSavedPlace) -> CatalogId {
    CatalogId::new(accepted_catalog_id(&place.store_catalog_id, "store")).expect("store catalog id")
}

#[allow(dead_code)]
fn accepted_catalog_id(id: &Option<String>, label: &str) -> String {
    id.clone()
        .unwrap_or_else(|| panic!("accepted catalog id for `{label}`"))
}

#[allow(dead_code)]
pub(crate) fn seed_title_only(store: &TreeStore, place: &CheckedSavedPlace, id: i64, title: &str) {
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

#[allow(dead_code)]
pub(crate) fn seed_member(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member: &str,
    value: Scalar,
) {
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

#[allow(dead_code)]
pub(crate) fn read_scalar(
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

#[allow(dead_code)]
pub(crate) fn read_scalar_by_catalog_id(
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

#[allow(dead_code)]
pub(crate) fn native_books_project(name: &str, source: &str) -> TempProject {
    temp_project(name, |root| {
        write(root, "marrow.json", crate::support::native_config());
        write(root, "src/books.mw", source);
    })
}

#[allow(dead_code)]
pub(crate) fn accepted_catalog(root: impl AsRef<Path>) -> marrow_project::CatalogMetadata {
    let json = fs::read_to_string(root.as_ref().join("marrow.catalog.json"))
        .expect("read accepted catalog");
    marrow_project::CatalogMetadata::from_json(&json).expect("parse accepted catalog")
}

#[allow(dead_code)]
pub(crate) fn accepted_catalog_entry_id(root: impl AsRef<Path>, path: &str) -> String {
    accepted_catalog(root)
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("accepted catalog entry `{path}`"))
        .stable_id
}

#[allow(dead_code)]
pub(crate) fn store_epoch(root: impl AsRef<Path>) -> Option<u64> {
    let store = TreeStore::open(&native_store_path(root)).expect("reopen native store");
    store.read_catalog_epoch().expect("read store epoch")
}

#[allow(dead_code)]
pub(crate) const REQUIRED_DEFAULT_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   required pages: int\n\
evolve\n\
\x20   default Book.pages = 0\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const REQUIRED_NO_DEFAULT_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   required pages: int\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const REQUIRED_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const OPTIONAL_PAGES_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   pages: int\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   required pages: int\n\
\x20   index byPages(pages, id)\n\
evolve\n\
\x20   default Book.pages = 0\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const PRICE_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required price: int\n\
pub fn add(price: int): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const PRICE_CENTS_TRANSFORM_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required price: int\n\
\x20   required priceCents: int\n\
evolve\n\
\x20   transform Book.priceCents\n\
\x20       return old.price * 100\n\
pub fn add(price: int): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const RETIRE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
evolve\n\
\x20   retire Book.subtitle\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const RETIRE_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   subtitle: string\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

#[allow(dead_code)]
pub(crate) const RENAME_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   blurb: string\n\
evolve\n\
\x20   rename Book.subtitle -> Book.blurb\n\
pub fn add(title: string): Id(^books)\n\
\x20   return nextId(^books)\n";

// Baseline with a runnable zero-arg entry: a `subtitle` member a later rename/retire
// consumes, plus a `seed` that writes through the store so the fence is exercised.
#[allow(dead_code)]
pub(crate) const BLOCK_BASELINE_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   subtitle: string\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";

// The renamed source with the consumed rename block present.
#[allow(dead_code)]
pub(crate) const RENAME_BLOCK_SOURCE: &str = "module books\n\
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
#[allow(dead_code)]
pub(crate) const RENAME_BLOCK_DELETED_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
\x20   blurb: string\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";

// The retired source with the consumed retire block present.
#[allow(dead_code)]
pub(crate) const RETIRE_BLOCK_SOURCE: &str = "module books\n\
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
#[allow(dead_code)]
pub(crate) const RETIRE_BLOCK_DELETED_SOURCE: &str = "module books\n\
resource Book at ^books(id: int)\n\
\x20   required title: string\n\
pub fn seed()\n\
\x20   var b: Book\n\
\x20   b.title = \"Dune\"\n\
\x20   transaction\n\
\x20       ^books(2) = b\n";
