//! Entry-name admission: one normal relative directory-entry component.

use std::fmt;

/// The longest admitted name in bytes (`NAME_MAX` on the qualified platforms).
const MAX_NAME_BYTES: usize = 255;

/// One admitted directory-entry name: a single normal relative component.
///
/// Admission rejects every spelling that could escape or alias the admitted
/// parent before any filesystem call. The grammar is platform-independent, so a
/// name admitted on one qualified platform is admitted identically on every
/// other, and a journal written under one platform's admission survives
/// classification under the other's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryName(String);

impl EntryName {
    /// Admit `name` as one normal relative component, or return the typed
    /// rejection. No filesystem call is made; admission is pure.
    pub fn admit(name: &str) -> Result<Self, EntryNameError> {
        if name.is_empty() {
            return Err(EntryNameError::Empty);
        }
        if name == "." {
            return Err(EntryNameError::Dot);
        }
        if name == ".." {
            return Err(EntryNameError::DotDot);
        }
        if name.contains('\0') {
            return Err(EntryNameError::Nul);
        }
        let bytes = name.as_bytes();
        // Leading separators and drive prefixes are absolute spellings on the
        // platform family that reads them, so they are refused everywhere.
        let drive_prefixed = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        if bytes[0] == b'/' || bytes[0] == b'\\' || drive_prefixed {
            return Err(EntryNameError::Absolute);
        }
        if name.contains('/') || name.contains('\\') {
            return Err(EntryNameError::Separator);
        }
        if name.chars().any(|c| c.is_control()) {
            return Err(EntryNameError::Control);
        }
        if bytes.len() > MAX_NAME_BYTES {
            return Err(EntryNameError::TooLong { len: bytes.len() });
        }
        Ok(Self(name.to_string()))
    }

    /// The admitted spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn admitted_max_bytes() -> usize {
        MAX_NAME_BYTES
    }
}

impl fmt::Display for EntryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a spelling was refused admission as an [`EntryName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryNameError {
    /// The empty string names nothing.
    Empty,
    /// `.` aliases the admitted parent itself.
    Dot,
    /// `..` escapes the admitted parent.
    DotDot,
    /// An interior NUL byte cannot reach a filesystem call.
    Nul,
    /// The spelling is absolute or drive-prefixed rather than relative.
    Absolute,
    /// The spelling contains a path separator, so it names more than one
    /// component.
    Separator,
    /// The spelling contains a control byte, which no qualified platform
    /// accepts in a portable entry name.
    Control,
    /// The spelling exceeds the qualified platforms' name length bound.
    TooLong {
        /// The rejected spelling's length in bytes.
        len: usize,
    },
}

impl fmt::Display for EntryNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("an entry name cannot be empty"),
            Self::Dot => formatter.write_str("`.` aliases the admitted directory itself"),
            Self::DotDot => formatter.write_str("`..` escapes the admitted directory"),
            Self::Nul => formatter.write_str("an entry name cannot contain a NUL byte"),
            Self::Absolute => formatter
                .write_str("an entry name must be relative, not absolute or drive-prefixed"),
            Self::Separator => formatter.write_str("an entry name cannot contain a path separator"),
            Self::Control => formatter.write_str("an entry name cannot contain a control byte"),
            Self::TooLong { len } => write!(
                formatter,
                "an entry name cannot exceed {MAX_NAME_BYTES} bytes (found {len})"
            ),
        }
    }
}

impl std::error::Error for EntryNameError {}
