//! Root-relative canonical file identities and the module names derived from
//! them.
//!
//! A project's source lives under one fixed root directory, [`SOURCE_ROOT`]. A
//! captured file's identity is its canonical root-relative path — always
//! forward-slash separated, under `src`, a `.mw` file, with no empty, `.`, or
//! `..` segment. A file's module identity is derived once from that path:
//! `src/foo/bar.mw` names module `foo.bar`. An importable source file carries a
//! matching in-source header. A headerless script keeps its path-derived identity
//! for export lookup but cannot be imported.

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
        Self::check(path)?;

        // `check` has established that `path` is a canonical `.mw` file with a
        // non-empty stem under the source root, with no empty, `.`, `..`,
        // backslash, or control segment. Derive the dotted module name from the
        // segments below the root; this allocation is validate's, not check's.
        let mut segments = path.split('/');
        segments.next(); // the source root, already checked.
        let under_root: Vec<&str> = segments.collect();
        let file_segment = *under_root
            .last()
            .expect("check guarantees a file under the root");
        let stem = mw_stem(file_segment).expect("check guarantees a non-empty `.mw` stem");

        let mut module_segments: Vec<&str> = under_root[..under_root.len() - 1].to_vec();
        module_segments.push(stem);
        let module = ModuleName(module_segments.join("."));

        Ok((FileIdentity(path.to_string()), module))
    }

    /// Report why a caller-supplied root-relative path cannot name a contained
    /// source file, in the same reason precedence as [`FileIdentity::validate`],
    /// without allocating.
    ///
    /// This is the one reason owner: `validate` delegates to it before
    /// constructing an identity, and the physical adapter calls it on a borrowed
    /// spelling before committing to any allocation. It traverses `path` as a
    /// borrowed `&str` only — no owned buffer, split collection, or formatting —
    /// so a caller may reject a spelling without paying for one.
    pub fn check(path: &str) -> Result<(), SourcePathReason> {
        if path.is_empty() {
            return Err(SourcePathReason::NonCanonical);
        }
        if path.contains('\\') || path.chars().any(|c| c.is_ascii_control()) {
            return Err(SourcePathReason::NonCanonical);
        }
        if path.starts_with('/') {
            return Err(SourcePathReason::Absolute);
        }

        // First offending segment in source order.
        for segment in path.split('/') {
            match segment {
                "" | "." => return Err(SourcePathReason::NonCanonical),
                ".." => return Err(SourcePathReason::Escapes),
                _ => {}
            }
        }

        // At least the root plus one file segment: `src/x.mw`.
        let mut segments = path.split('/');
        let root = segments
            .next()
            .expect("a non-empty path has a first segment");
        if root != SOURCE_ROOT || segments.next().is_none() {
            return Err(SourcePathReason::OutsideSourceRoot);
        }

        let file_segment = path
            .rsplit('/')
            .next()
            .expect("a non-empty path has a final segment");
        if mw_stem(file_segment).is_none() {
            return Err(SourcePathReason::NotMarrowSource);
        }

        Ok(())
    }

    /// The identity's path lowercased, used to detect two identities that differ
    /// only in case and would collide on a case-insensitive filesystem.
    pub(crate) fn case_fold(&self) -> String {
        self.0.to_lowercase()
    }
}

/// The non-empty stem of a `.mw` file segment, or `None` when the segment is not
/// a `.mw` file with a non-empty name. Allocation-free: stripping the `mw`
/// extension and its `.` separator recovers the stem without materializing the
/// `.mw` suffix string.
fn mw_stem(file_segment: &str) -> Option<&str> {
    file_segment
        .strip_suffix(SOURCE_EXTENSION)
        .and_then(|rest| rest.strip_suffix('.'))
        .filter(|stem| !stem.is_empty())
}

/// The path-derived dotted module name, such as `foo.bar`. Constructed only
/// alongside a [`FileIdentity`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModuleName(String);

impl ModuleName {
    /// The dotted module name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a caller-supplied path cannot name a contained source file.
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
