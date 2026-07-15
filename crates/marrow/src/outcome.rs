//! The typed CLI outcome owner (design §H).
//!
//! One typed [`Record`] preserves all four failure families as distinct variants;
//! its JSONL projection is a canonical one-object-per-line surface the differential
//! harness and (later) `marrow test` consume. The four families never collapse: a
//! source diagnostic, an artifact rejection, a source-mapped runtime fault, and an
//! owner-local operational error are distinct records.

use marrow_vm::Value;

/// The maximum rendered `data` size before an overflow becomes an operational
/// error rather than a truncated record (design §H).
const MAX_DATA_BYTES: usize = 64 * 1024;

/// A single run outcome record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Record {
    /// A successful value (or `None` for a Unit return).
    Value(Option<Value>),
    /// Family 1: a source diagnostic (parse/check).
    Diagnostic {
        code: &'static str,
        line: u32,
        column: u32,
    },
    /// Family 2: an image decode/verify rejection.
    ArtifactRejected { code: &'static str },
    /// Family 3: a source-mapped runtime fault. `detail` is the static author text
    /// of an `unreachable("...")` fault, surfaced in text output only; the typed
    /// JSONL surface stays the code and span.
    Fault {
        code: &'static str,
        line: u32,
        column: u32,
        detail: Option<String>,
    },
    /// Family 4: an owner-local operational error (CLI/store/io).
    OperationalError { code: &'static str },
}

impl Record {
    /// The plain-text rendering for the default (non-JSONL) format.
    pub(crate) fn to_text(&self) -> String {
        match self {
            Record::Value(Some(value)) => render_value_text(value),
            Record::Value(None) => String::new(),
            Record::Diagnostic { code, line, column } => format!("{code} at {line}:{column}"),
            Record::Fault {
                code,
                line,
                column,
                detail,
            } => match detail {
                Some(text) => format!("{code} at {line}:{column}: {text}"),
                None => format!("{code} at {line}:{column}"),
            },
            Record::ArtifactRejected { code } | Record::OperationalError { code } => {
                code.to_string()
            }
        }
    }

    /// The canonical single-line JSONL projection: one object, keys in ascending
    /// byte order, LF added by the caller.
    pub(crate) fn to_jsonl(&self) -> String {
        match self {
            Record::Value(value) => match render_data(value.as_ref()) {
                Ok(data) => format!(r#"{{"data":{data},"kind":"run","outcome":"value"}}"#),
                Err(()) => Record::OperationalError {
                    code: marrow_codes::Code::IoWrite.as_str(),
                }
                .to_jsonl(),
            },
            Record::Diagnostic { code, line, column } => format!(
                r#"{{"code":{},"kind":"run","outcome":"diagnostic","span":{}}}"#,
                json_string(code),
                span_object(*line, *column)
            ),
            Record::ArtifactRejected { code } => format!(
                r#"{{"code":{},"kind":"run","outcome":"artifact_rejected"}}"#,
                json_string(code)
            ),
            Record::Fault {
                code, line, column, ..
            } => format!(
                r#"{{"code":{},"kind":"run","outcome":"fault","span":{}}}"#,
                json_string(code),
                span_object(*line, *column)
            ),
            Record::OperationalError { code } => format!(
                r#"{{"code":{},"kind":"run","outcome":"error"}}"#,
                json_string(code)
            ),
        }
    }
}

/// The classified outcome of running one `test` declaration: it passed, an
/// `assert` condition was false (`run.assert` — a test failure), or any other
/// runtime fault errored it. A failure and an error stay distinct families: a
/// failure is the test's own assertion, an error is an unexpected fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TestOutcome {
    Passed,
    Failed {
        code: &'static str,
        line: u32,
        column: u32,
    },
    Errored {
        code: &'static str,
        line: u32,
        column: u32,
    },
}

/// One reported test: its report name, the source file it lives in, its
/// declaration position (for the passed span), and its classified outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TestRecord {
    pub(crate) name: String,
    pub(crate) file: String,
    pub(crate) decl_line: u32,
    pub(crate) decl_column: u32,
    pub(crate) outcome: TestOutcome,
}

impl TestRecord {
    /// The canonical single-line JSONL projection: one `kind: "test"` object, keys
    /// in ascending byte order. A pass carries the declaration span; a failure or
    /// error carries its fault code and span.
    pub(crate) fn to_jsonl(&self) -> String {
        match &self.outcome {
            TestOutcome::Passed => format!(
                r#"{{"file":{},"kind":"test","name":{},"outcome":"passed","span":{}}}"#,
                json_string(&self.file),
                json_string(&self.name),
                span_object(self.decl_line, self.decl_column),
            ),
            TestOutcome::Failed { code, line, column } => self.fault_jsonl("failed", code, *line, *column),
            TestOutcome::Errored { code, line, column } => {
                self.fault_jsonl("errored", code, *line, *column)
            }
        }
    }

    fn fault_jsonl(&self, outcome: &str, code: &str, line: u32, column: u32) -> String {
        format!(
            r#"{{"code":{},"file":{},"kind":"test","name":{},"outcome":"{outcome}","span":{}}}"#,
            json_string(code),
            json_string(&self.file),
            json_string(&self.name),
            span_object(line, column),
        )
    }

    /// The plain-text rendering for the default format.
    pub(crate) fn to_text(&self) -> String {
        match &self.outcome {
            TestOutcome::Passed => format!("ok    {}", self.name),
            TestOutcome::Failed { code, line, column } => {
                format!("FAIL  {} ({code} at {line}:{column})", self.name)
            }
            TestOutcome::Errored { code, line, column } => {
                format!("ERROR {} ({code} at {line}:{column})", self.name)
            }
        }
    }
}

/// The end-of-run summary over the selected tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestSummary {
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) errored: usize,
    pub(crate) total: usize,
}

impl TestSummary {
    /// The number of tests actually run (selected by any filter).
    fn selected(&self) -> usize {
        self.passed + self.failed + self.errored
    }

    /// The canonical JSONL summary object, keys in ascending byte order.
    pub(crate) fn to_jsonl(&self) -> String {
        format!(
            r#"{{"errored":{},"failed":{},"kind":"summary","passed":{},"selected":{},"total":{}}}"#,
            self.errored,
            self.failed,
            self.passed,
            self.selected(),
            self.total,
        )
    }

    /// The plain-text summary line.
    pub(crate) fn to_text(&self) -> String {
        format!(
            "{} passed, {} failed, {} errored ({}/{} selected)",
            self.passed,
            self.failed,
            self.errored,
            self.selected(),
            self.total,
        )
    }
}

/// Render bytes as `0x`-prefixed lowercase hex, the canonical `bytes` rendering.
fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn span_object(line: u32, column: u32) -> String {
    format!(r#"{{"column":{column},"line":{line}}}"#)
}

fn render_value_text(value: &Value) -> String {
    match value {
        Value::Int(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Text(v) => v.to_string(),
        Value::Bytes(v) => hex_bytes(v),
        Value::Optional(None) => "absent".to_string(),
        Value::Optional(Some(inner)) => render_value_text(inner),
        // Record returns are rejected by the verifier, so a record never surfaces
        // as an export result; render defensively rather than panicking.
        Value::Record(..) => String::new(),
    }
}

/// Render a value as the JSONL `data` field, or `Err` when a text value exceeds the
/// data bound (the caller turns that into an operational error, never a truncation).
fn render_data(value: Option<&Value>) -> Result<String, ()> {
    Ok(match value {
        None | Some(Value::Optional(None)) => "null".to_string(),
        Some(Value::Int(v)) => v.to_string(),
        Some(Value::Bool(v)) => v.to_string(),
        Some(Value::Text(v)) => {
            if v.len() > MAX_DATA_BYTES {
                return Err(());
            }
            json_string(v)
        }
        Some(Value::Bytes(v)) => {
            if v.len() * 2 + 2 > MAX_DATA_BYTES {
                return Err(());
            }
            json_string(&hex_bytes(v))
        }
        Some(Value::Optional(Some(inner))) => render_data(Some(inner))?,
        // A record cannot be an export result (verifier-rejected return); no record
        // wire format is minted here.
        Some(Value::Record(..)) => return Err(()),
    })
}

/// Encode a string as a canonical JSON string (design §H escaping rules): `\"`,
/// `\\`, `\b`, `\t`, `\n`, `\f`, `\r`, other C0 as lowercase `\u00XX`, everything
/// else (including `/` and all non-ASCII) passed through as UTF-8.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::{Record, json_string};
    use marrow_vm::Value;

    #[test]
    fn value_record_is_canonical_jsonl() {
        assert_eq!(
            Record::Value(Some(Value::Int(42))).to_jsonl(),
            r#"{"data":42,"kind":"run","outcome":"value"}"#
        );
        assert_eq!(
            Record::Value(Some(Value::Bool(true))).to_jsonl(),
            r#"{"data":true,"kind":"run","outcome":"value"}"#
        );
        assert_eq!(
            Record::Value(None).to_jsonl(),
            r#"{"data":null,"kind":"run","outcome":"value"}"#
        );
    }

    #[test]
    fn each_family_projects_a_distinct_outcome() {
        assert!(
            Record::Diagnostic {
                code: "check.type",
                line: 3,
                column: 5
            }
            .to_jsonl()
            .contains(r#""outcome":"diagnostic""#)
        );
        assert!(
            Record::ArtifactRejected {
                code: "image.function"
            }
            .to_jsonl()
            .contains(r#""outcome":"artifact_rejected""#)
        );
        assert!(
            Record::Fault {
                code: "run.overflow",
                line: 1,
                column: 1,
                detail: None,
            }
            .to_jsonl()
            .contains(r#""outcome":"fault""#)
        );
        assert!(
            Record::OperationalError { code: "store.io" }
                .to_jsonl()
                .contains(r#""outcome":"error""#)
        );
    }

    /// Keys within an object are in ascending byte order (design §H), including the
    /// nested span object (`column` before `line`).
    #[test]
    fn keys_are_in_ascending_byte_order() {
        let line = Record::Fault {
            code: "run.overflow",
            line: 7,
            column: 2,
            detail: None,
        }
        .to_jsonl();
        assert_eq!(
            line,
            r#"{"code":"run.overflow","kind":"run","outcome":"fault","span":{"column":2,"line":7}}"#
        );
    }

    /// The escaping rules: the seven short escapes, C0 as lowercase `\u00XX`, and no
    /// escaping of `/` or non-ASCII.
    #[test]
    fn json_string_escapes_per_contract() {
        assert_eq!(json_string("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(json_string("\u{08}\t\n\u{0C}\r"), r#""\b\t\n\f\r""#);
        assert_eq!(json_string("\u{01}"), r#""\u0001""#);
        assert_eq!(json_string("a/b"), r#""a/b""#);
        assert_eq!(json_string("café ☕"), "\"café ☕\"");
    }
}
