//! Enforcement artifacts for the store-admission owner.
//!
//! The type chain enforces the two admission stages: `TreeStore`'s on-disk constructors
//! are crate-private (stage 1 cannot be skipped), and `AdmittedStore`'s constructor is
//! private to the admission module, reachable only through `admit_read`/`admit_write`
//! (stage 2 cannot be forged). These scans add what visibility alone cannot say: the
//! sealed constructor has exactly one production caller besides its definition, and every
//! module that holds a stage-1 `SealedStore` without admitting it is named here with the
//! reason the skip is legitimate.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn production_sources(crates_dir: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    for entry in fs::read_dir(crates_dir).expect("read crates dir").flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut sources);
        }
    }
    sources
}

fn scan(crates_dir: &Path, pattern: &str, allowed: &[PathBuf]) -> Vec<String> {
    let mut offenders = Vec::new();
    for source in production_sources(crates_dir) {
        if allowed.contains(&source) {
            continue;
        }
        let text = fs::read_to_string(&source).expect("read source");
        for (line_index, line) in text.lines().enumerate() {
            if line.contains(pattern) {
                offenders.push(format!(
                    "{}:{}",
                    source.strip_prefix(crates_dir).unwrap_or(&source).display(),
                    line_index + 1
                ));
            }
        }
    }
    offenders
}

/// The admission ladder — the one open witness `run`, `serve`, and the inspection point-reads
/// share — validates the O(1) sealed commit record and never a store-wide data scan. The full O(N)
/// data re-derivation (`verify_readable`, `verify_structural_digests`, `verify_data_cells_seek_reachable`)
/// moved off admission: a run verifies each data root as it enumerates it, and the deep re-walk is
/// the `data integrity`/`recover`/`backup` owner's. (The derived-index cross-check still runs on the
/// run and serve opens outside this module, since a dropped index entry has no touch to fault on.)
#[test]
fn admission_validates_the_record_not_the_data_scan() {
    let crates_dir = workspace_crates_dir();
    let admission = fs::read_to_string(crates_dir.join("marrow-run/src/admission.rs"))
        .expect("read admission module");
    assert!(
        admission.contains("validate_commit_record"),
        "the admission ladder must validate the sealed commit record at open",
    );
    for scan in [
        "verify_readable",
        "verify_structural_digests",
        "verify_data_cells_seek_reachable",
        "verify_store_completeness",
    ] {
        assert!(
            !admission.contains(scan),
            "the admission ladder must not run the O(N) `{scan}` data scan at open; \
             it moved to the run's per-root touch check and the deep re-walk",
        );
    }
}

#[test]
fn sealed_open_is_called_only_by_the_admission_module() {
    let crates_dir = workspace_crates_dir();
    let allowed = [
        // The constructor's definition.
        crates_dir.join("marrow-store/src/sealed.rs"),
        // The admission owner: open_read/open_write/open_create are the sole production
        // callers, so every durable handle in the runtime and CLI originates there.
        crates_dir.join("marrow-run/src/admission.rs"),
    ];
    let offenders = scan(&crates_dir, "SealedStore::open", &allowed);
    assert!(
        offenders.is_empty(),
        "SealedStore::open may be called only by the admission module; found:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn stage_one_sealed_holders_are_the_blessed_set() {
    let crates_dir = workspace_crates_dir();
    // Each entry holds a stage-1 SealedStore without admitting it, for the stated reason.
    // A new file appearing here means a new path skips the identity/lock ladder: extend
    // this list only with a rationale, never to silence the scan.
    let allowed = [
        // Defines the type.
        crates_dir.join("marrow-store/src/sealed.rs"),
        // Re-exports the type at the crate root.
        crates_dir.join("marrow-store/src/lib.rs"),
        // The admission owner: mints sealed handles and consumes them in admit.
        crates_dir.join("marrow-run/src/admission.rs"),
        // Stage-1 holds: the pre-check accepted-catalog inspection (the catalog read is
        // what produces the program identity admission needs), the unclean-shutdown
        // recovery replay, pre-admission seeding, the dry run over a pending durable
        // identity (no accepted identity exists to admit against), and the isolated
        // dry-run copy of an already-admitted store.
        crates_dir.join("marrow-run/src/project_session.rs"),
        // Pre-program catalog inspection reads for check and the read-only render verbs.
        crates_dir.join("marrow/src/main.rs"),
        // Doctor diagnoses stores that would fail admission.
        crates_dir.join("marrow/src/cmd_doctor.rs"),
        // `data recover` repairs stores before any program can admit them; the data
        // inspection context also reads backup mounts that have no live identity.
        crates_dir.join("marrow/src/cmd_data.rs"),
        // Restore writes a store body ahead of any checked identity.
        crates_dir.join("marrow/src/cmd_restore.rs"),
        // Evolve apply/preview establish identity; apply runs its own witness and fence.
        crates_dir.join("marrow/src/cmd_evolve/store.rs"),
        // Backup archives a store whatever its identity state; the lock-root witness
        // runs through the shared owner before anything is archived.
        crates_dir.join("marrow/src/cmd_backup.rs"),
        // In-source test module mints a corruption fixture through the sealed open;
        // cfg(test)-only, no production path.
        crates_dir.join("marrow-json/src/surface.rs"),
    ];
    // Type inference can hold a sealed handle without ever spelling the type, so the
    // scan also matches the mint spellings: the admission opens (the only source of a
    // SealedStore outside the admission module, since the constructor scan pins
    // `SealedStore::open` there) and the CLI's inspection helper that forwards one.
    let mut offenders = Vec::new();
    for pattern in [
        "SealedStore",
        "admission::open_",
        "open_store_for_inspection(",
    ] {
        offenders.extend(scan(&crates_dir, pattern, &allowed));
    }
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "a new module holds a stage-1 SealedStore without admission; \
         add it here only with a rationale:\n{}",
        offenders.join("\n")
    );
}
