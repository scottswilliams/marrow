//! Root-relative canonical file identities and the module names derived from
//! them.
//!
//! A project's source lives under one fixed root directory, [`SOURCE_ROOT`]. A
//! captured file's identity is its canonical root-relative path — always
//! forward-slash separated, under `src`, a `.mw` file, with no empty, `.`, or
//! `..` segment. The module name a file declares is derived once from that path:
//! `src/foo/bar.mw` names module `foo.bar`. There is no in-source module header
//! and no single-file fallback; the path is the sole source of module identity.

/// The fixed directory every project's source lives under. A captured identity
/// that does not begin here is outside the source root and cannot name a module.
pub const SOURCE_ROOT: &str = "src";

/// The extension every Marrow source file carries.
pub const SOURCE_EXTENSION: &str = "mw";

/// The canonical root-relative identity of a captured source file, such as
/// `src/foo/bar.mw`. Independent of where the project is located on disk, so a
/// relocated project yields byte-identical identities. Constructed only through
/// [`FileIdentity::validate`], so every value is canonical.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileIdentity(String);

impl FileIdentity {
    /// The canonical root-relative path string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate a caller-supplied root-relative path and derive its identity and
    /// module name, or report why the path cannot name a contained module.
    ///
    /// The path must be relative, forward-slash separated, rooted at
    /// [`SOURCE_ROOT`], carry the `.mw` extension, name at least one path segment
    /// under the root, contain no empty, `.`, or `..` segment, and contain no NUL
    /// or ASCII control character.
    ///
    /// This validates the *path* domain only. The full module-name character and
    /// Unicode-normalization domain (which spellings may name a module, and
    /// whether NFC/NFD-distinct segments are the same identity) lands with the
    /// module-name semantic owner; control characters are rejected here today
    /// because they are wrong under any future domain.
    pub fn validate(path: &str) -> Result<(FileIdentity, ModuleName), SourcePathReason> {
        if path.is_empty() {
            return Err(SourcePathReason::NonCanonical);
        }
        if path.contains('\\') || path.chars().any(|c| c.is_ascii_control()) {
            return Err(SourcePathReason::NonCanonical);
        }
        if path.starts_with('/') {
            return Err(SourcePathReason::Absolute);
        }

        let segments: Vec<&str> = path.split('/').collect();
        for segment in &segments {
            match *segment {
                "" | "." => return Err(SourcePathReason::NonCanonical),
                ".." => return Err(SourcePathReason::Escapes),
                _ => {}
            }
        }

        // At least the root plus one file segment: `src/x.mw`.
        if segments.len() < 2 || segments[0] != SOURCE_ROOT {
            return Err(SourcePathReason::OutsideSourceRoot);
        }

        let under_root = &segments[1..];
        let file_segment = *under_root.last().expect("at least one segment under root");
        let stem = file_segment
            .strip_suffix(&format!(".{SOURCE_EXTENSION}"))
            .filter(|stem| !stem.is_empty())
            .ok_or(SourcePathReason::NotMarrowSource)?;

        let mut module_segments: Vec<&str> = under_root[..under_root.len() - 1].to_vec();
        module_segments.push(stem);
        let module = ModuleName(module_segments.join("."));

        Ok((FileIdentity(path.to_string()), module))
    }

    /// The identity's path lowercased, used to detect two identities that differ
    /// only in case and would collide on a case-insensitive filesystem.
    pub(crate) fn case_fold(&self) -> String {
        self.0.to_lowercase()
    }
}

/// The dotted module name a file declares, such as `foo.bar`, derived from its
/// path under the source root. Constructed only alongside a [`FileIdentity`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModuleName(String);

impl ModuleName {
    /// The dotted module name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a caller-supplied path cannot name a contained source module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourcePathReason {
    /// The path is absolute; identities are project-root-relative.
    Absolute,
    /// The path contains a `..` segment that would escape the source root.
    Escapes,
    /// The path is empty, uses a backslash separator, contains a NUL or ASCII
    /// control character, or has an empty or `.` segment.
    NonCanonical,
    /// The path is not under the fixed `src` source root.
    OutsideSourceRoot,
    /// The path is under the source root but is not a `.mw` file with a
    /// non-empty stem.
    NotMarrowSource,
}
