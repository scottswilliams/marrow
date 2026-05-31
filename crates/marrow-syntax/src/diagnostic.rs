//! The diagnostic surface shared across the toolchain: error/warning records,
//! the severity scale, the `Diagnose` trait the CLI renders, and source spans.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub kind: &'static str,
    pub severity: Severity,
    pub message: String,
    pub help: Option<String>,
    pub span: SourceSpan,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}: {}",
            self.span.line,
            self.span.column,
            self.severity.as_str(),
            self.code,
            self.message
        )
    }
}

impl Diagnose for Diagnostic {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn severity(&self) -> Severity {
        self.severity
    }
    fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }
    // A parse diagnostic stores its kind verbatim (always "parse"); return it
    // rather than deriving it, so the rendered kind never depends on the map.
    fn kind(&self) -> &str {
        self.kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// The common error surface every diagnostic that reaches the CLI envelope shares:
/// a dotted code, a broad kind, a human message, a severity, and optional help.
/// The CLI renders any of these uniformly over `&dyn Diagnose`; the source-span
/// shape stays per source, since a parse span, a project line/column, and a
/// path-located finding are not the same object.
pub trait Diagnose {
    fn code(&self) -> &str;
    fn message(&self) -> &str;
    fn severity(&self) -> Severity {
        Severity::Error
    }
    fn help(&self) -> Option<&str> {
        None
    }
    fn kind(&self) -> &str {
        kind_for_code(self.code())
    }
}

/// The broad `kind` category for a dotted error code, derived from the code's
/// first segment. The prefix is not always the kind name
/// (`run.*` is `runtime`, `store.*` is `storage`), so the mapping is explicit.
pub fn kind_for_code(code: &str) -> &'static str {
    match code.split('.').next().unwrap_or("") {
        "parse" => "parse",
        "check" | "schema" => "check",
        "run" | "value" => "runtime",
        "store" => "storage",
        "io" => "io",
        "protocol" => "protocol",
        // Configuration and project-discovery failures are tooling errors.
        _ => "tooling",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: u32,
    pub column: u32,
}

#[cfg(test)]
mod tests {
    use super::kind_for_code;

    #[test]
    fn value_codes_are_runtime_diagnostics() {
        assert_eq!(kind_for_code("value.range"), "runtime");
    }
}
