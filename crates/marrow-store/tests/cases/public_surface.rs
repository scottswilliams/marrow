use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::cell::{CatalogId, DataCellKind, DataPathSegment};
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    TREE_BACKUP_MAX_CELL_BYTES, TREE_BACKUP_MAX_MANIFEST_BYTES, TreeBackupCellBuf, TreeStore,
    read_tree_backup_archive_chunk, read_tree_backup_archive_header,
    write_tree_backup_archive_chunk, write_tree_backup_archive_header,
};

#[test]
fn public_surface_external_user_reaches_store_through_typed_tree_and_backup_api() {
    let store = TreeStore::memory();
    let root = catalog_id("00000000000000000000000000000001");
    let title = catalog_id("00000000000000000000000000000002");
    let notes = catalog_id("00000000000000000000000000000003");
    let identity = vec![SavedKey::Int(7)];

    store
        .write_record_presence(&root, &identity)
        .expect("write typed record presence");
    store
        .write_leaf(&root, &identity, &title, b"title".to_vec())
        .expect("write typed leaf");
    store
        .write_data_value(
            &root,
            &identity,
            &[
                DataPathSegment::Member(notes.clone()),
                DataPathSegment::Key(SavedKey::Int(1)),
            ],
            b"note".to_vec(),
        )
        .expect("write typed nested data value");

    let mut framed = Vec::new();
    let mut targets = Vec::new();
    let mut values = Vec::new();

    store
        .visit_backup_cells(|cell| {
            let target = cell.data_key();
            assert_eq!(&target.store, &root);
            assert_eq!(&target.identity, &identity);
            assert_typed_path(&target.path());

            cell.write_framed(&mut framed)
                .expect("write typed backup frame");
            targets.push(target.clone());
            values.push(cell.value().to_vec());
            Ok(())
        })
        .expect("visit typed backup cells");

    assert_eq!(targets.len(), 3);
    assert!(
        targets
            .iter()
            .any(|target| matches!(target.kind, DataCellKind::Node))
    );
    assert!(
        targets
            .iter()
            .any(|target| matches!(target.kind, DataCellKind::Leaf { .. }))
    );
    assert!(
        targets
            .iter()
            .any(|target| matches!(target.kind, DataCellKind::Value { .. }))
    );

    let mut reader = framed.as_slice();
    for (target, value) in targets.iter().zip(&values) {
        let decoded =
            TreeBackupCellBuf::read_framed_optional(&mut reader, TREE_BACKUP_MAX_CELL_BYTES)
                .expect("read typed backup frame")
                .expect("backup frame");
        assert_eq!(decoded.data_key(), target);
        assert_eq!(decoded.value(), value);
    }
    assert!(
        TreeBackupCellBuf::read_framed_optional(&mut reader, TREE_BACKUP_MAX_CELL_BYTES)
            .expect("read trailing backup frame")
            .is_none()
    );
}

#[test]
fn public_surface_external_user_can_use_bounded_archive_framing_helpers() {
    let mut archive = Vec::new();
    write_tree_backup_archive_header(&mut archive).expect("write backup header");
    write_tree_backup_archive_chunk(&mut archive, br#"{"format_version":6}"#)
        .expect("write manifest chunk");

    let mut reader = archive.as_slice();
    read_tree_backup_archive_header(&mut reader).expect("read backup header");
    assert_eq!(
        read_tree_backup_archive_chunk(&mut reader, TREE_BACKUP_MAX_MANIFEST_BYTES, "manifest")
            .expect("read manifest chunk"),
        br#"{"format_version":6}"#
    );
    assert!(reader.is_empty());
}

#[test]
fn public_surface_rejects_external_raw_engine_and_key_access() {
    for (name, source) in [
        (
            "backend_trait",
            r#"
                use marrow_store::backend::Backend;
                pub fn accepts_backend(_: &dyn Backend) {}
            "#,
        ),
        (
            "engine_page_types",
            r#"
                use marrow_store::backend::{ScanPage, ValuePrefix};
                pub fn accepts_raw_pages(_: Option<ScanPage>, _: Option<ValuePrefix>) {}
            "#,
        ),
        (
            "memory_backend",
            r#"
                use marrow_store::mem::MemStore;
                pub fn accepts_mem_store(_: MemStore) {}
            "#,
        ),
        (
            "native_backend",
            r#"
                use marrow_store::redb::RedbStore;
                pub fn accepts_redb_store(_: RedbStore) {}
            "#,
        ),
        (
            "physical_cell_key",
            r#"
                use marrow_store::cell::{CellKey, CellRange};
                pub fn accepts_raw_cell_key(_: Option<CellKey>, _: Option<CellRange>) {}
            "#,
        ),
        (
            "raw_key_codecs",
            r#"
                use marrow_store::key::{decode_key_value, encode_key_value};
                use marrow_store::key::SavedKey;
                pub fn raw_key_round_trip(key: &SavedKey) {
                    let bytes = encode_key_value(key);
                    let _ = decode_key_value(&bytes);
                }
            "#,
        ),
        (
            "raw_backup_cell_constructor",
            r#"
                use marrow_store::tree::TreeBackupCellBuf;
                pub fn raw_backup_cell() {
                    let _ = TreeBackupCellBuf::from_raw(Vec::new(), Vec::new());
                }
            "#,
        ),
        (
            "raw_backend_constructor",
            r#"
                use marrow_store::tree::TreeStore;
                pub fn raw_backend_constructor() {
                    let _ = TreeStore::from_backend;
                }
            "#,
        ),
    ] {
        assert_external_compile_fails(name, source);
    }
}

fn catalog_id(suffix: &str) -> CatalogId {
    CatalogId::new(format!("cat_{suffix}")).expect("valid catalog id")
}

fn assert_typed_path(path: &[DataPathSegment]) {
    for segment in path {
        match segment {
            DataPathSegment::Member(id) => {
                assert!(id.as_str().starts_with("cat_"));
            }
            DataPathSegment::Key(SavedKey::Int(_)) => {}
            DataPathSegment::Key(other) => panic!("unexpected typed key segment: {other:?}"),
        }
    }
}

fn assert_external_compile_fails(name: &str, source: &str) {
    let project = TempRustcProject::new(name);
    let source_path = project.write_source(source);
    let output_path = project.path.join("snippet.rlib");
    let output = rustc_external(&source_path, &output_path, usable_marrow_store_rlib())
        .unwrap_or_else(|error| panic!("run rustc for {name}: {error}"));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "snippet `{name}` unexpectedly compiled\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
    assert!(
        expected_boundary_error(&stderr),
        "snippet `{name}` failed for the wrong reason\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
}

fn rustc() -> String {
    std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string())
}

fn usable_marrow_store_rlib() -> &'static PathBuf {
    static RLIB: OnceLock<PathBuf> = OnceLock::new();
    RLIB.get_or_init(find_usable_marrow_store_rlib)
}

fn find_usable_marrow_store_rlib() -> PathBuf {
    let mut failures = Vec::new();
    for candidate in marrow_store_rlib_candidates().into_iter().rev() {
        let project = TempRustcProject::new("positive");
        let source_path = project.write_source(
            r#"
                use marrow_store::tree::TreeStore;
                pub fn typed_store() {
                    let _ = TreeStore::memory();
                }
            "#,
        );
        let output_path = project.path.join("positive.rlib");
        let output = rustc_external(&source_path, &output_path, &candidate)
            .unwrap_or_else(|error| panic!("run positive rustc probe: {error}"));
        if output.status.success() {
            return candidate;
        }
        failures.push(format!(
            "{}\n{}",
            candidate.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    panic!(
        "no usable marrow_store rlib found for external public-surface probes:\n{}",
        failures.join("\n---\n")
    );
}

fn marrow_store_rlib_candidates() -> Vec<PathBuf> {
    let exe = std::env::current_exe().expect("current test executable");
    let deps = exe.parent().expect("test executable has deps parent");
    let mut candidates = std::fs::read_dir(deps)
        .unwrap_or_else(|error| panic!("read {}: {error}", deps.display()))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("libmarrow_store-"))
                && path
                    .extension()
                    .is_some_and(|extension| extension == "rlib")
        })
        .collect::<Vec<_>>();

    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    assert!(
        !candidates.is_empty(),
        "missing compiled marrow_store rlib in {}",
        deps.display()
    );
    candidates
}

fn rustc_external(
    source_path: &Path,
    output_path: &Path,
    lib: &Path,
) -> std::io::Result<std::process::Output> {
    let deps_dir = lib.parent().expect("rlib has a deps parent");
    Command::new(rustc())
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg(source_path)
        .arg("--extern")
        .arg(format!("marrow_store={}", lib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps_dir.display()))
        .arg("-o")
        .arg(output_path)
        .output()
}

fn expected_boundary_error(stderr: &str) -> bool {
    !stderr.contains("E0514")
        && !stderr.contains("compiled by an incompatible")
        && (stderr.contains("E0432")
            || stderr.contains("E0433")
            || stderr.contains("E0599")
            || stderr.contains("E0603")
            || stderr.contains("E0624")
            || stderr.contains("private")
            || stderr.contains("unresolved import")
            || stderr.contains("could not find"))
}

struct TempRustcProject {
    path: PathBuf,
}

impl TempRustcProject {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "marrow-store-public-surface-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path)
            .unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
        Self { path }
    }

    fn write_source(&self, source: &str) -> PathBuf {
        let path = self.path.join("snippet.rs");
        std::fs::write(&path, source)
            .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
        path
    }
}

impl Drop for TempRustcProject {
    fn drop(&mut self) {
        let _ = remove_dir_all_if_exists(&self.path);
    }
}

fn remove_dir_all_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
