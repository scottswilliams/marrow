//! The one private URI and document-identity owner.
//!
//! It performs `file` URI → [`SelectedRoot`] at initialization and `file` URI +
//! [`SelectedRoot`] → [`DocumentKey`] for documents, and it re-encodes a snapshot
//! [`FileIdentity`] back to a diagnostic URI over the retained root spelling. It admits
//! only `file` URIs with empty authority, no query or fragment, and one absolute
//! decoded UTF-8 path; it percent-decodes exactly once and rejects malformed escapes,
//! encoded separators, control bytes, and non-canonical components. Case, Unicode
//! normalization, symlink, and hardlink aliases are never coalesced: the physical
//! membership authority is the capture adapter's exact [`FileIdentity`], not this
//! lexical owner.

use std::fmt::Write as _;

use marrow_project_fs::{
    DependencyAlias, DependencyPath, FileIdentity, ProjectInput, SourceOrigin,
};

use crate::capacities::MAX_URI_BYTES;

/// Why a `file` URI was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum UriError {
    /// The URI exceeded [`MAX_URI_BYTES`].
    TooLong,
    /// The scheme was not `file` (with empty authority).
    NotFileScheme,
    /// The URI carried a query or fragment.
    HasQueryOrFragment,
    /// A percent-escape was malformed or decoded to an encoded separator, NUL, or
    /// control byte.
    BadEscape,
    /// The decoded path was empty, relative, or carried an empty, `.`, `..`, or
    /// repeated/trailing-separator component, or a raw backslash.
    NonCanonicalPath,
    /// The decoded bytes were not valid UTF-8.
    NotUtf8,
    /// The URI named a real file under the selected root that is not one of the
    /// project's own source files — anything outside `src`, or not a `.mw` file.
    NotProjectSource,
}

/// The caller-selected project root: its decoded absolute lexical path components. No
/// case, Unicode, symlink, or physical-identity canonicalization is applied — a
/// symlinked or percent-spelled root is retained exactly and re-encoded faithfully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectedRoot {
    /// Decoded absolute path components (no leading empty component).
    components: Vec<String>,
}

impl SelectedRoot {
    /// Admit a `file` URI as the selected root.
    pub(crate) fn from_uri(uri: &str) -> Result<Self, UriError> {
        let components = decode_file_uri_path(uri)?;
        if components.is_empty() {
            return Err(UriError::NonCanonicalPath);
        }
        Ok(Self { components })
    }

    /// The decoded absolute path components.
    pub(crate) fn components(&self) -> &[String] {
        &self.components
    }
}

/// A document identity: the tree it belongs to and its identity in that tree. Two URI
/// spellings that decode to the same admitted path produce one key; filesystem case,
/// Unicode, symlink, and hardlink aliases are never coalesced here.
///
/// A key built from a URI is always [`SourceOrigin::Root`]: the workspace is one project
/// and only its own source is ever opened, overlaid, or formatted. A dependency key
/// exists only for a file the analysis snapshot reports on, which is what makes
/// "dependency files are read-only" a property of the type rather than a rule a caller
/// must remember.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DocumentKey {
    origin: SourceOrigin,
    /// Forward-slash-joined path relative to the origin's own root, e.g. `src/foo.mw`.
    relative: String,
}

impl DocumentKey {
    /// Admit a `file` document URI as one of the root project's own source files. The
    /// document must be a proper descendant of the root — a sibling sharing a name
    /// prefix is not containment — and must be a canonical captured source spelling, so
    /// a file the project does not compile never enters the document ledger or the
    /// capture overlay.
    pub(crate) fn from_uri(uri: &str, root: &SelectedRoot) -> Result<Self, UriError> {
        let components = decode_file_uri_path(uri)?;
        let root_len = root.components.len();
        if components.len() <= root_len {
            return Err(UriError::NonCanonicalPath);
        }
        if components[..root_len] != root.components[..] {
            return Err(UriError::NonCanonicalPath);
        }
        let relative = components[root_len..].join("/");
        FileIdentity::check(&relative).map_err(|_| UriError::NotProjectSource)?;
        Ok(Self {
            origin: SourceOrigin::Root,
            relative,
        })
    }

    /// The forward-slash-joined path relative to this document's own tree.
    pub(crate) fn relative(&self) -> &str {
        &self.relative
    }

    /// The tree this document belongs to.
    pub(crate) fn origin(&self) -> &SourceOrigin {
        &self.origin
    }

    /// The document key for one captured file: the tree it came from and its identity
    /// there.
    pub(crate) fn captured(origin: &SourceOrigin, identity: &FileIdentity) -> Self {
        Self {
            origin: origin.clone(),
            relative: identity.as_str().to_owned(),
        }
    }
}

/// Where each captured tree sits, as the selected root plus each dependency's declared
/// relative location. The declared spelling is the manifest's, so this owner joins two
/// facts it is given and canonicalizes no path of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OriginRoots {
    dependencies: Vec<(DependencyAlias, DependencyPath)>,
}

impl OriginRoots {
    /// The declared locations of one captured project's dependencies.
    pub(crate) fn of(input: &ProjectInput) -> Self {
        Self {
            dependencies: input
                .origins()
                .iter()
                .filter_map(|origin| {
                    let alias = origin.alias()?;
                    let path = input.dependency_path(origin)?;
                    Some((alias.clone(), path.clone()))
                })
                .collect(),
        }
    }

    /// The absolute path components of one origin's root: the selected root itself, or
    /// the selected root with the dependency's declared relative path applied — a
    /// leading `..` run pops a component, every other segment descends. `None` when the
    /// alias is not one of these dependencies, or when the declared path would step
    /// above the filesystem root.
    fn root_of(&self, root: &SelectedRoot, origin: &SourceOrigin) -> Option<Vec<String>> {
        let Some(alias) = origin.alias() else {
            return Some(root.components.clone());
        };
        let (_, path) = self
            .dependencies
            .iter()
            .find(|(declared, _)| declared == alias)?;
        let mut components = root.components.clone();
        for segment in path.segments() {
            if segment == ".." {
                components.pop()?;
            } else {
                components.push(segment.to_owned());
            }
        }
        Some(components)
    }
}

/// Re-encode one captured document to a `file` URI over the retained selected-root
/// spelling. The client's own document-URI spelling is never echoed; the caller-selected
/// root spelling is deliberately retained and canonically re-encoded. A dependency's file
/// resolves against *its* root, so the URI names the file where it actually lives rather
/// than a path that does not exist in the consuming tree.
pub(crate) fn document_uri(
    root: &SelectedRoot,
    origins: &OriginRoots,
    key: &DocumentKey,
) -> Option<String> {
    let mut uri = String::from("file://");
    for component in origins.root_of(root, &key.origin)? {
        uri.push('/');
        percent_encode_segment(&mut uri, &component);
    }
    for segment in key.relative.split('/') {
        uri.push('/');
        percent_encode_segment(&mut uri, segment);
    }
    Some(uri)
}

/// Decode a `file` URI into its absolute decoded path components, enforcing every
/// canonicalization rule. Returns the components below the leading `/`.
fn decode_file_uri_path(uri: &str) -> Result<Vec<String>, UriError> {
    if uri.len() > MAX_URI_BYTES {
        return Err(UriError::TooLong);
    }
    // Reject a query or fragment before scheme handling: they are never admitted.
    if uri.contains('?') || uri.contains('#') {
        return Err(UriError::HasQueryOrFragment);
    }
    // `file://` with empty authority, then an absolute path beginning with `/`.
    let rest = uri.strip_prefix("file://").ok_or(UriError::NotFileScheme)?;
    // Empty authority: the path must begin immediately at `/`. A non-empty authority
    // (`file://host/...`) is refused.
    if !rest.starts_with('/') {
        return Err(UriError::NotFileScheme);
    }
    let decoded = percent_decode_path(rest)?;
    // Split on `/`; the leading `/` yields an empty first component that must be
    // dropped. Every other component must be canonical.
    let mut components = Vec::new();
    let mut segments = decoded.split('/');
    let first = segments.next();
    debug_assert_eq!(first, Some(""));
    if first != Some("") {
        return Err(UriError::NonCanonicalPath);
    }
    for segment in segments {
        if segment.is_empty() || segment == "." || segment == ".." {
            // Empty (repeated/trailing separator), `.`, or `..` are non-canonical.
            return Err(UriError::NonCanonicalPath);
        }
        if segment.contains('\\') {
            return Err(UriError::NonCanonicalPath);
        }
        components.push(segment.to_owned());
    }
    Ok(components)
}

/// Percent-decode a path exactly once, rejecting malformed escapes and any decoded
/// encoded-separator, NUL, or control byte.
fn percent_decode_path(input: &str) -> Result<String, UriError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let high = bytes.get(index + 1).copied().ok_or(UriError::BadEscape)?;
            let low = bytes.get(index + 2).copied().ok_or(UriError::BadEscape)?;
            let decoded = hex_pair(high, low).ok_or(UriError::BadEscape)?;
            // An encoded separator or a control/NUL byte is rejected: decoding may not
            // introduce a new path separator or an unsafe byte.
            if decoded == b'/' || decoded == b'\\' || decoded < 0x20 || decoded == 0x7f {
                return Err(UriError::BadEscape);
            }
            out.push(decoded);
            index += 3;
        } else if byte < 0x20 || byte == 0x7f {
            return Err(UriError::BadEscape);
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| UriError::NotUtf8)
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    Some(hex_digit(high)? << 4 | hex_digit(low)?)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-encode one path segment for a `file` URI: RFC 3986 unreserved characters
/// pass through, every other byte is `%XX`.
fn percent_encode_segment(out: &mut String, segment: &str) {
    for &byte in segment.as_bytes() {
        if is_unreserved(byte) {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(path: &str) -> FileIdentity {
        FileIdentity::validate(path).unwrap().0
    }

    fn root_key(path: &str) -> DocumentKey {
        DocumentKey::captured(&SourceOrigin::Root, &identity(path))
    }

    fn dependency(alias: &str) -> SourceOrigin {
        SourceOrigin::Dependency(DependencyAlias::parse(alias).unwrap())
    }

    fn origins_for(alias: &str, declared: &str) -> OriginRoots {
        OriginRoots {
            dependencies: vec![(
                DependencyAlias::parse(alias).unwrap(),
                DependencyPath::parse(declared).unwrap(),
            )],
        }
    }

    #[test]
    fn admits_simple_root() {
        let root = SelectedRoot::from_uri("file:///home/user/proj").unwrap();
        assert_eq!(root.components(), &["home", "user", "proj"]);
    }

    #[test]
    fn rejects_non_file_scheme() {
        assert_eq!(
            SelectedRoot::from_uri("http:///x"),
            Err(UriError::NotFileScheme)
        );
        assert_eq!(
            SelectedRoot::from_uri("file://host/x"),
            Err(UriError::NotFileScheme)
        );
    }

    #[test]
    fn rejects_query_and_fragment() {
        assert_eq!(
            SelectedRoot::from_uri("file:///x?y=1"),
            Err(UriError::HasQueryOrFragment)
        );
        assert_eq!(
            SelectedRoot::from_uri("file:///x#frag"),
            Err(UriError::HasQueryOrFragment)
        );
    }

    #[test]
    fn rejects_dot_and_dotdot_and_trailing_separator() {
        for bad in [
            "file:///a/./b",
            "file:///a/../b",
            "file:///a//b",
            "file:///a/",
        ] {
            assert_eq!(
                SelectedRoot::from_uri(bad),
                Err(UriError::NonCanonicalPath),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_encoded_separator_and_control() {
        assert_eq!(
            SelectedRoot::from_uri("file:///a%2Fb"),
            Err(UriError::BadEscape)
        );
        assert_eq!(
            SelectedRoot::from_uri("file:///a%00b"),
            Err(UriError::BadEscape)
        );
    }

    #[test]
    fn rejects_malformed_escape() {
        assert_eq!(
            SelectedRoot::from_uri("file:///a%zzb"),
            Err(UriError::BadEscape)
        );
        assert_eq!(
            SelectedRoot::from_uri("file:///a%2"),
            Err(UriError::BadEscape)
        );
    }

    #[test]
    fn percent_decodes_once() {
        // %20 is a space; a legitimate directory name with a space.
        let root = SelectedRoot::from_uri("file:///my%20proj").unwrap();
        assert_eq!(root.components(), &["my proj"]);
    }

    #[test]
    fn document_must_be_proper_descendant() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        let key = DocumentKey::from_uri("file:///proj/src/foo.mw", &root).unwrap();
        assert_eq!(key.relative(), "src/foo.mw");
    }

    #[test]
    fn sibling_prefix_is_not_containment() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        assert_eq!(
            DocumentKey::from_uri("file:///project/src/foo.mw", &root),
            Err(UriError::NonCanonicalPath)
        );
    }

    #[test]
    fn root_itself_is_not_a_document() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        assert_eq!(
            DocumentKey::from_uri("file:///proj", &root),
            Err(UriError::NonCanonicalPath)
        );
    }

    #[test]
    fn two_spellings_of_same_path_produce_one_key() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        let a = DocumentKey::from_uri("file:///proj/src/foo.mw", &root).unwrap();
        let b = DocumentKey::from_uri("file:///proj/src/%66oo.mw", &root).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_file_outside_the_source_root_is_not_a_document() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        for outside in [
            "file:///proj/README.md",
            "file:///proj/marrow.toml",
            "file:///proj/notes.mw",
            "file:///proj/lib/text/src/text.mw",
        ] {
            assert_eq!(
                DocumentKey::from_uri(outside, &root),
                Err(UriError::NotProjectSource),
                "{outside}"
            );
        }
    }

    #[test]
    fn document_uri_round_trips_a_root_document() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        let key = root_key("src/foo.mw");
        let uri = document_uri(&root, &OriginRoots::default(), &key).unwrap();
        assert_eq!(uri, "file:///proj/src/foo.mw");
        // And it re-parses to the same document key.
        assert_eq!(DocumentKey::from_uri(&uri, &root).unwrap(), key);
    }

    #[test]
    fn document_uri_encodes_space_in_root() {
        let root = SelectedRoot::from_uri("file:///my%20proj").unwrap();
        let uri = document_uri(&root, &OriginRoots::default(), &root_key("src/foo.mw")).unwrap();
        assert_eq!(uri, "file:///my%20proj/src/foo.mw");
    }

    /// A dependency file resolves against the dependency's own root: the declared
    /// relative path is applied to the selected root, so the URI names the file where it
    /// lives rather than a path in the consuming tree.
    #[test]
    fn a_dependency_document_resolves_against_its_own_root() {
        let root = SelectedRoot::from_uri("file:///home/dev/app").unwrap();
        for (declared, expected) in [
            ("../graphtext", "file:///home/dev/graphtext/src/text.mw"),
            (
                "lib/graphtext",
                "file:///home/dev/app/lib/graphtext/src/text.mw",
            ),
            (
                "../../shared/graphtext",
                "file:///home/shared/graphtext/src/text.mw",
            ),
        ] {
            let origins = origins_for("graphtext", declared);
            let key = DocumentKey::captured(&dependency("graphtext"), &identity("src/text.mw"));
            assert_eq!(
                document_uri(&root, &origins, &key).as_deref(),
                Some(expected),
                "{declared}"
            );
        }
    }

    /// Two trees may hold the same identity. The origin, not the identity string, is what
    /// tells the two documents apart.
    #[test]
    fn the_same_identity_in_two_trees_is_two_documents() {
        let root = SelectedRoot::from_uri("file:///home/dev/app").unwrap();
        let origins = origins_for("graphtext", "../graphtext");
        let mine = root_key("src/text.mw");
        let theirs = DocumentKey::captured(&dependency("graphtext"), &identity("src/text.mw"));
        assert_ne!(mine, theirs);
        assert_ne!(
            document_uri(&root, &origins, &mine),
            document_uri(&root, &origins, &theirs)
        );
    }

    /// An alias no capture reported has no root, so no URI is invented for it.
    #[test]
    fn an_unknown_alias_has_no_document_uri() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        let key = DocumentKey::captured(&dependency("absent"), &identity("src/text.mw"));
        assert_eq!(document_uri(&root, &OriginRoots::default(), &key), None);
    }

    /// A declared path that would step above the filesystem root names no directory, so
    /// it yields no URI rather than a truncated one.
    #[test]
    fn a_path_above_the_filesystem_root_has_no_document_uri() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        let origins = origins_for("graphtext", "../../graphtext");
        let key = DocumentKey::captured(&dependency("graphtext"), &identity("src/text.mw"));
        assert_eq!(document_uri(&root, &origins, &key), None);
    }
}
