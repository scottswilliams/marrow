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

use marrow_project::FileIdentity;

use crate::capacities::MAX_URI_BYTES;

/// Why a `file` URI was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UriError {
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
}

/// The caller-selected project root: its decoded absolute lexical path components. No
/// case, Unicode, symlink, or physical-identity canonicalization is applied — a
/// symlinked or percent-spelled root is retained exactly and re-encoded faithfully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedRoot {
    /// Decoded absolute path components (no leading empty component).
    components: Vec<String>,
}

impl SelectedRoot {
    /// Admit a `file` URI as the selected root.
    pub fn from_uri(uri: &str) -> Result<Self, UriError> {
        let components = decode_file_uri_path(uri)?;
        if components.is_empty() {
            return Err(UriError::NonCanonicalPath);
        }
        Ok(Self { components })
    }

    /// The decoded absolute path components.
    pub fn components(&self) -> &[String] {
        &self.components
    }
}

/// A document identity: its root-relative components under the selected root. Two URI
/// spellings that decode to the same admitted path produce one key; filesystem case,
/// Unicode, symlink, and hardlink aliases are never coalesced here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocumentKey {
    /// Forward-slash-joined root-relative path, e.g. `src/foo.mw`.
    relative: String,
}

impl DocumentKey {
    /// Admit a `file` document URI under a selected root. The document must be a
    /// proper descendant of the root — a sibling sharing a name prefix is not
    /// containment.
    pub fn from_uri(uri: &str, root: &SelectedRoot) -> Result<Self, UriError> {
        let components = decode_file_uri_path(uri)?;
        let root_len = root.components.len();
        if components.len() <= root_len {
            return Err(UriError::NonCanonicalPath);
        }
        if components[..root_len] != root.components[..] {
            return Err(UriError::NonCanonicalPath);
        }
        let relative = components[root_len..].join("/");
        Ok(Self { relative })
    }

    /// The forward-slash-joined root-relative path.
    pub fn relative(&self) -> &str {
        &self.relative
    }

    /// The document key for a snapshot file identity (already a canonical root-relative
    /// path such as `src/foo.mw`).
    pub fn from_identity(identity: &FileIdentity) -> Self {
        Self {
            relative: identity.as_str().to_owned(),
        }
    }
}

/// Re-encode a snapshot [`FileIdentity`] to a diagnostic `file` URI over the retained
/// selected-root spelling. The client's own document-URI spelling is never echoed; the
/// caller-selected root spelling is deliberately retained and canonically re-encoded.
pub fn diagnostic_uri(root: &SelectedRoot, identity: &FileIdentity) -> String {
    let mut uri = String::from("file://");
    for component in &root.components {
        uri.push('/');
        percent_encode_segment(&mut uri, component);
    }
    for segment in identity.as_str().split('/') {
        uri.push('/');
        percent_encode_segment(&mut uri, segment);
    }
    uri
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
        for bad in ["file:///a/./b", "file:///a/../b", "file:///a//b", "file:///a/"] {
            assert_eq!(
                SelectedRoot::from_uri(bad),
                Err(UriError::NonCanonicalPath),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_encoded_separator_and_control() {
        assert_eq!(SelectedRoot::from_uri("file:///a%2Fb"), Err(UriError::BadEscape));
        assert_eq!(SelectedRoot::from_uri("file:///a%00b"), Err(UriError::BadEscape));
    }

    #[test]
    fn rejects_malformed_escape() {
        assert_eq!(SelectedRoot::from_uri("file:///a%zzb"), Err(UriError::BadEscape));
        assert_eq!(SelectedRoot::from_uri("file:///a%2"), Err(UriError::BadEscape));
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
    fn diagnostic_uri_round_trips_identity() {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        let uri = diagnostic_uri(&root, &identity("src/foo.mw"));
        assert_eq!(uri, "file:///proj/src/foo.mw");
        // And it re-parses to the same document key.
        let key = DocumentKey::from_uri(&uri, &root).unwrap();
        assert_eq!(key, DocumentKey::from_identity(&identity("src/foo.mw")));
    }

    #[test]
    fn diagnostic_uri_encodes_space_in_root() {
        let root = SelectedRoot::from_uri("file:///my%20proj").unwrap();
        let uri = diagnostic_uri(&root, &identity("src/foo.mw"));
        assert_eq!(uri, "file:///my%20proj/src/foo.mw");
    }
}
