use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

/// Every file in a store directory with its bytes, keyed by name, so two snapshots are
/// equal exactly when no store artifact changed.
///
/// A published store always holds at least its head, so an empty snapshot means the
/// caller named the wrong directory; it panics rather than let two empty snapshots
/// compare equal.
pub fn store_files(dir: &Path) -> BTreeMap<OsString, Vec<u8>> {
    let files: BTreeMap<_, _> = fs::read_dir(dir)
        .expect("store entries")
        .map(|entry| {
            let entry = entry.expect("store entry");
            (
                entry.file_name(),
                fs::read(entry.path()).expect("store file"),
            )
        })
        .collect();
    assert!(
        !files.is_empty(),
        "no store files under {}: an empty snapshot makes every comparison vacuous",
        dir.display()
    );
    files
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::panic::catch_unwind;

    use super::store_files;
    use crate::Scratch;

    #[test]
    fn an_empty_directory_is_refused_rather_than_snapshotted() {
        let scratch = Scratch::new("snapshot-empty");
        let refused = catch_unwind(|| store_files(scratch.path()));
        assert!(
            refused.is_err(),
            "an empty snapshot compares equal to anything empty"
        );
    }

    #[test]
    fn a_snapshot_sees_every_file_and_every_byte() {
        let scratch = Scratch::new("snapshot-bytes");
        let dir = scratch.path();
        fs::write(dir.join("HEAD"), b"one").expect("write head");
        let before = store_files(dir);
        assert_eq!(before.len(), 1);
        fs::write(dir.join("HEAD"), b"two").expect("rewrite head");
        assert_ne!(
            store_files(dir),
            before,
            "a changed byte changes the snapshot"
        );
        fs::write(dir.join("HEAD"), b"one").expect("restore head");
        fs::write(dir.join("extra"), b"").expect("write extra");
        assert_ne!(
            store_files(dir),
            before,
            "an added file changes the snapshot"
        );
    }
}
