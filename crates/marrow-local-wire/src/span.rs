//! The source span a runtime fault maps to.

use crate::json::Json;

/// A one-based source line and column. The runner fills it from a VM fault's
/// source mapping; the client surfaces it beside the fault code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub column: u32,
}

impl Span {
    /// The canonical `{"column":C,"line":L}` object (keys sorted by [`Json`]).
    pub(crate) fn to_json(self) -> Json {
        Json::Object(vec![
            ("column".to_string(), Json::Int(i64::from(self.column))),
            ("line".to_string(), Json::Int(i64::from(self.line))),
        ])
    }
}
