//! Companion-runner discovery and release verification for the persistent terminal path.
//!
//! A persistent `marrow run --store` runs the program in a stock `marrow-runner` attached to
//! the store. The terminal must not run an arbitrary executable found on `PATH`, the working
//! directory, or an environment variable: it locates the companion only at a fixed relative
//! path beside the terminal binary itself, and verifies it against the release manifest the
//! installer wrote there before spawning it. A missing manifest, a release mismatch, or a
//! companion whose bytes do not match its recorded release identity is installation damage —
//! reported with an actionable repair message, never worked around.
//!
//! The manifest is a small line-oriented file (`marrow-companions`) beside the terminal:
//!
//! ```text
//! marrow companions v0
//! release <toolchain version>
//! runner <relative-name> <64 lowercase-hex release id>
//! end
//! ```
//!
//! `<relative-name>` is a single path component (no separators, never absolute, never `..`),
//! resolved against the terminal's own directory.

use std::path::{Path, PathBuf};

use marrow_image::{CompanionReleaseId, companion_release_id};

/// The fixed manifest filename beside the terminal binary.
const MANIFEST_NAME: &str = "marrow-companions";

/// The manifest is tiny; anything larger is malformed rather than read unbounded.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// A generous ceiling on the companion binary read for hashing (law 9: bound before
/// allocating). A stock runner is a few megabytes; a file past this is refused as damaged.
const MAX_COMPANION_BYTES: u64 = 1 << 30;

/// Why the companion could not be located and verified. Every variant is installation damage
/// the terminal reports as `cli.installation_damaged`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CompanionError {
    /// The terminal's own path could not be determined, so the fixed relative location is
    /// unknown.
    TerminalPathUnknown,
    /// The release manifest is absent beside the terminal.
    ManifestMissing,
    /// The manifest is present but malformed, oversized, or names an unsafe path.
    ManifestMalformed,
    /// The manifest names a different toolchain release than this terminal.
    ReleaseMismatch,
    /// The companion binary is absent, unreadable, or oversized.
    CompanionMissing,
    /// The companion's bytes do not match the release identity the manifest records.
    CompanionMismatch,
}

impl CompanionError {
    /// An actionable one-line repair message. It names the installation problem and the fix
    /// without any runner/wire/lifecycle vocabulary.
    pub(crate) fn message(&self) -> &'static str {
        match self {
            CompanionError::TerminalPathUnknown => {
                "cannot locate the Marrow installation directory; reinstall the toolchain"
            }
            CompanionError::ManifestMissing => {
                "the Marrow installation is incomplete (its release manifest is missing); \
                 reinstall the toolchain"
            }
            CompanionError::ManifestMalformed => {
                "the Marrow release manifest is damaged; reinstall the toolchain"
            }
            CompanionError::ReleaseMismatch => {
                "the Marrow release manifest is from a different version than this command; \
                 reinstall the toolchain so its parts match"
            }
            CompanionError::CompanionMissing => {
                "the Marrow installation is incomplete (a required component is missing); \
                 reinstall the toolchain"
            }
            CompanionError::CompanionMismatch => {
                "a Marrow installation component is damaged or altered; reinstall the toolchain"
            }
        }
    }
}

/// Discover and verify the companion runner beside this terminal binary. Returns the
/// verified companion's absolute path, ready to spawn.
pub(crate) fn discover_companion() -> Result<PathBuf, CompanionError> {
    let exe = std::env::current_exe().map_err(|_| CompanionError::TerminalPathUnknown)?;
    let dir = exe
        .parent()
        .ok_or(CompanionError::TerminalPathUnknown)?
        .to_path_buf();
    discover_companion_in(&dir, this_release())
}

/// The toolchain release this terminal was built as; the manifest must name the same one.
fn this_release() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Discover and verify the companion under `dir` against the expected `release`. Split from
/// [`discover_companion`] so a test drives a constructed installation layout without touching
/// the process's own executable path.
pub(crate) fn discover_companion_in(dir: &Path, release: &str) -> Result<PathBuf, CompanionError> {
    let manifest = read_manifest(dir)?;
    if manifest.release != release {
        return Err(CompanionError::ReleaseMismatch);
    }
    let companion = resolve_component(dir, &manifest.runner_name)?;
    let bytes = read_companion(&companion)?;
    let actual = companion_release_id(&bytes);
    if actual != manifest.runner_id {
        return Err(CompanionError::CompanionMismatch);
    }
    Ok(companion)
}

/// The three facts the manifest carries.
struct Manifest {
    release: String,
    runner_name: String,
    runner_id: CompanionReleaseId,
}

fn read_manifest(dir: &Path) -> Result<Manifest, CompanionError> {
    let path = dir.join(MANIFEST_NAME);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return Err(CompanionError::ManifestMissing),
    };
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(CompanionError::ManifestMalformed);
    }
    let text = std::fs::read_to_string(&path).map_err(|_| CompanionError::ManifestMissing)?;
    parse_manifest(&text)
}

fn parse_manifest(text: &str) -> Result<Manifest, CompanionError> {
    let mut lines = text.lines();
    if lines.next() != Some("marrow companions v0") {
        return Err(CompanionError::ManifestMalformed);
    }
    let release = lines
        .next()
        .and_then(|line| line.strip_prefix("release "))
        .map(str::to_owned)
        .ok_or(CompanionError::ManifestMalformed)?;
    let runner_line = lines
        .next()
        .and_then(|line| line.strip_prefix("runner "))
        .ok_or(CompanionError::ManifestMalformed)?;
    let (runner_name, id_hex) = runner_line
        .split_once(' ')
        .ok_or(CompanionError::ManifestMalformed)?;
    let runner_id =
        CompanionReleaseId::from_hex(id_hex).ok_or(CompanionError::ManifestMalformed)?;
    if lines.next() != Some("end") {
        return Err(CompanionError::ManifestMalformed);
    }
    if release.is_empty() || runner_name.is_empty() {
        return Err(CompanionError::ManifestMalformed);
    }
    Ok(Manifest {
        release,
        runner_name: runner_name.to_owned(),
        runner_id,
    })
}

/// Resolve a manifest-named component to a path under `dir`, refusing any name that is not a
/// single plain component (a separator, a parent reference, or an absolute path could escape
/// the installation directory).
fn resolve_component(dir: &Path, name: &str) -> Result<PathBuf, CompanionError> {
    let mut components = Path::new(name).components();
    let only = components.next();
    if components.next().is_some() {
        return Err(CompanionError::ManifestMalformed);
    }
    match only {
        Some(std::path::Component::Normal(component)) if component == name => {
            Ok(dir.join(component))
        }
        _ => Err(CompanionError::ManifestMalformed),
    }
}

fn read_companion(path: &Path) -> Result<Vec<u8>, CompanionError> {
    let metadata = std::fs::metadata(path).map_err(|_| CompanionError::CompanionMissing)?;
    if !metadata.is_file() || metadata.len() > MAX_COMPANION_BYTES {
        return Err(CompanionError::CompanionMissing);
    }
    std::fs::read(path).map_err(|_| CompanionError::CompanionMissing)
}

/// Render the manifest line for `runner_name` given the companion binary's `bytes`. The
/// installer (and the test harness) writes this beside the terminal.
#[cfg(test)]
pub(crate) fn manifest_text(release: &str, runner_name: &str, companion_bytes: &[u8]) -> String {
    let id = companion_release_id(companion_bytes).to_hex();
    format!("marrow companions v0\nrelease {release}\nrunner {runner_name} {id}\nend\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(dir: &Path, release: &str, runner_bytes: &[u8]) {
        std::fs::create_dir_all(dir).expect("dir");
        std::fs::write(dir.join("marrow-runner"), runner_bytes).expect("runner");
        std::fs::write(
            dir.join(MANIFEST_NAME),
            super::manifest_text(release, "marrow-runner", runner_bytes),
        )
        .expect("manifest");
    }

    fn scratch(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "marrow-companion-{tag}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn a_matching_layout_verifies() {
        let dir = scratch("ok");
        layout(&dir, "9.9.9", b"stock runner bytes");
        assert_eq!(
            discover_companion_in(&dir, "9.9.9"),
            Ok(dir.join("marrow-runner")),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_manifest_is_damage() {
        let dir = scratch("missing");
        std::fs::create_dir_all(&dir).expect("dir");
        assert_eq!(
            discover_companion_in(&dir, "9.9.9"),
            Err(CompanionError::ManifestMissing),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_release_mismatch_is_damage() {
        let dir = scratch("release");
        layout(&dir, "1.0.0", b"stock runner bytes");
        assert_eq!(
            discover_companion_in(&dir, "9.9.9"),
            Err(CompanionError::ReleaseMismatch),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_altered_companion_is_rejected() {
        let dir = scratch("altered");
        layout(&dir, "9.9.9", b"stock runner bytes");
        // Overwrite the companion after the manifest recorded its identity.
        std::fs::write(dir.join("marrow-runner"), b"tampered bytes").expect("tamper");
        assert_eq!(
            discover_companion_in(&dir, "9.9.9"),
            Err(CompanionError::CompanionMismatch),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every installation-damage message is actionable and free of runner/wire/lifecycle
    /// mechanism vocabulary — a user sees an install problem and a repair, never the spawn
    /// mechanism (exit-gate property: ordinary output and guidance carry no such vocabulary).
    #[test]
    fn damage_messages_are_actionable_and_free_of_mechanism_vocabulary() {
        let all = [
            CompanionError::TerminalPathUnknown,
            CompanionError::ManifestMissing,
            CompanionError::ManifestMalformed,
            CompanionError::ReleaseMismatch,
            CompanionError::CompanionMissing,
            CompanionError::CompanionMismatch,
        ];
        let forbidden = [
            "runner",
            "wire",
            "socket",
            "nonce",
            "lifecycle",
            "attach",
            "spawn",
        ];
        for error in all {
            let message = error.message().to_lowercase();
            for word in forbidden {
                assert!(
                    !message.contains(word),
                    "message leaks `{word}`: {}",
                    error.message(),
                );
            }
            assert!(
                message.contains("reinstall"),
                "message must name a repair action: {}",
                error.message(),
            );
        }
    }

    #[test]
    fn a_traversing_runner_name_is_refused() {
        let dir = scratch("traverse");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join(MANIFEST_NAME),
            "marrow companions v0\nrelease 9.9.9\nrunner ../evil 0000000000000000000000000000000000000000000000000000000000000000\nend\n",
        )
        .expect("manifest");
        assert_eq!(
            discover_companion_in(&dir, "9.9.9"),
            Err(CompanionError::ManifestMalformed),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
