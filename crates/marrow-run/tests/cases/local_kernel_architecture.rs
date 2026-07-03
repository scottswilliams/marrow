//! Enforcement artifacts for the one-kernel / typed-address invariant (lane L7).
//!
//! Local collections are the in-memory `Sequence`/`LocalTree` kernel, addressed by
//! the validated `Position`/`CollectionKey` newtypes minted at a single runtime
//! boundary — `crates/marrow-run/src/collection/local.rs`. These scans keep the old
//! shape from returning: the bespoke free-floating dispatch module is gone, and no
//! production code outside the boundary mints an address from a raw integer or key
//! tuple.

use std::fs;
use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

const BOUNDARY: &str = "collection/local.rs";

/// The production part of a source file: everything before its first `#[cfg(test)]`
/// item. Test modules live at the end of a file by convention here and legitimately
/// construct kernel values to exercise them, so an address-construction scan targets
/// only the production prefix.
fn production_source(source: &str) -> &str {
    source.split("#[cfg(test)]").next().unwrap_or(source)
}

fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|err| panic!("read {}: {err}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn the_bespoke_local_collection_dispatch_module_is_gone() {
    let obsolete = src_dir().join("local_collection.rs");
    assert!(
        !obsolete.exists(),
        "the bespoke local-collection dispatch module has returned at {}; its dispatch \
         belongs on the Sequence/LocalTree kernel behind {BOUNDARY}",
        obsolete.display()
    );
    assert!(
        src_dir().join(BOUNDARY).exists(),
        "the local-collection kernel boundary {BOUNDARY} is missing"
    );
}

#[test]
fn kernel_addresses_are_minted_only_at_the_one_boundary() {
    let src = src_dir();
    let boundary = src.join(BOUNDARY);
    let mut files = Vec::new();
    rust_sources(&src, &mut files);

    let mut leaks = Vec::new();
    for file in &files {
        if *file == boundary {
            continue;
        }
        let source = fs::read_to_string(file).unwrap_or_else(|err| {
            panic!("read {}: {err}", file.display());
        });
        let production = production_source(&source);
        for constructor in ["Position::new(", "CollectionKey::new("] {
            if production.contains(constructor) {
                let relative = file.strip_prefix(&src).unwrap_or(file);
                leaks.push(format!(
                    "{} mints an address via `{constructor}`",
                    relative.display()
                ));
            }
        }
    }

    assert!(
        leaks.is_empty(),
        "a local-collection address is constructed outside the {BOUNDARY} boundary:\n{}",
        leaks.join("\n")
    );

    // Positive control: the boundary itself must actually mint both address kinds, so
    // this scan cannot pass by the constructors having been renamed away.
    let boundary_source = fs::read_to_string(&boundary).expect("read boundary module");
    let boundary_production = production_source(&boundary_source);
    assert!(
        boundary_production.contains("Position::new(")
            && boundary_production.contains("CollectionKey::new("),
        "the {BOUNDARY} boundary no longer mints Position/CollectionKey addresses"
    );
}
