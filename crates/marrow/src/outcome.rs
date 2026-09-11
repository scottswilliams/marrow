//! The typed CLI outcome owner.
//!
//! One typed [`Record`] preserves all four failure families as distinct variants;
//! its JSONL projection is a canonical one-object-per-line surface the differential
//! harness and (later) `marrow test` consume. The four families never collapse: a
//! source diagnostic, an artifact rejection, a source-mapped runtime fault, and an
//! owner-local operational error are distinct records.

use std::fmt::{self, Write};

use marrow_verify::{SealedEnumType, SealedRecordType};
use marrow_vm::Value;

/// Raw UTF-8 bytes admitted for stdin and a returned bare string. JSON escaping
/// can expand each byte sixfold; this is not an encoded-record bound.
pub(crate) const MAX_TEXT_BYTES: usize = 64 * 1024;

/// The existing JSON admission bound for bytes and aggregate `data` values.
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
    /// The invocation did not return. The source-mapped fault and durable
    /// commit state are orthogonal typed facts.
    Incomplete {
        code: &'static str,
        durable: marrow_vm::DurableCommitState,
        line: u32,
        column: u32,
    },
    /// Family 4: an owner-local operational error (CLI/store/io). `detail` is the
    /// typed human message (e.g. the file and reason a `.marrow/ids` read was
    /// rejected), surfaced in text output only; the KAT-frozen JSONL surface stays
    /// the code alone.
    OperationalError {
        code: &'static str,
        detail: Option<String>,
    },
    /// A durable call was dispatched but no exact valid correlated reply could
    /// be accepted. The cause kind and its stable diagnostic code remain
    /// orthogonal to this outcome and never imply a retry.
    OutcomeUnknown {
        cause: &'static str,
        cause_code: &'static str,
    },
    /// Family 4 specialization: an aggregate compiler resource-limit outcome. Unlike a
    /// bare operational error it carries the typed kind — which fixed bound was
    /// exhausted — so a caller (or a bound-raise audit) can bisect which limit fired
    /// without re-running. Holding the kind rather than one rendering of it lets the
    /// JSONL surface keep the frozen
    /// [`detail`](marrow_compile::ResourceLimitKind::detail) identifier while text
    /// output reads as prose. The record still carries no numeric limit and no source
    /// location. The code is always `cli.compiler_resource_limit`.
    CompilerResourceLimit {
        kind: marrow_compile::ResourceLimitKind,
    },
}

impl Record {
    /// The plain-text rendering for the default (non-JSONL) format. `types` supplies
    /// the field names of a returned record value; it is empty for the non-value
    /// families, which never render a record.
    pub(crate) fn to_text(
        &self,
        types: &[SealedRecordType],
        enums: &[SealedEnumType],
    ) -> Result<String, ()> {
        Ok(match self {
            Record::Value(Some(Value::Text(text))) if text.len() > MAX_TEXT_BYTES => return Err(()),
            // Aggregate text has no byte ceiling; the bare-string limit is checked above.
            Record::Value(Some(value)) => {
                marrow_vm::render::value_text(value, types, enums, usize::MAX).map_err(|_| ())?
            }
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
            Record::Incomplete {
                code,
                durable,
                line,
                column,
            } => format!(
                "{code} at {line}:{column}: invocation incomplete; durable state {}",
                durable_state_name(*durable),
            ),
            Record::ArtifactRejected { code } => code.to_string(),
            Record::OperationalError { code, detail } => match detail {
                Some(text) => format!("{code}: {text}"),
                None => code.to_string(),
            },
            Record::OutcomeUnknown { cause, cause_code } => format!(
                "{}: the call was dispatched but no exact valid reply could be accepted, so its \
                 outcome is unknown and it was not retried; run a read-only export to observe \
                 the store's current state (cause: {cause}, {cause_code})",
                marrow_codes::Code::RunOutcomeUnknown.as_str(),
            ),
            Record::CompilerResourceLimit { kind } => format!(
                "{}: {}",
                marrow_codes::Code::CliCompilerResourceLimit.as_str(),
                kind.description()
            ),
        })
    }

    /// The canonical single-line JSONL projection: one object, keys in ascending
    /// byte order, LF added by the caller. A data refusal remains an error so the
    /// emitter cannot mistake a failed rendering for a successful invocation.
    pub(crate) fn to_jsonl(
        &self,
        types: &[SealedRecordType],
        enums: &[SealedEnumType],
    ) -> Result<String, ()> {
        Ok(match self {
            Record::Value(value) => {
                let data = render_data(value.as_ref(), types, enums)?;
                format!(r#"{{"data":{data},"kind":"run","outcome":"value"}}"#)
            }
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
            Record::Incomplete {
                code,
                durable,
                line,
                column,
            } => format!(
                r#"{{"code":{},"durable":{},"kind":"run","outcome":"incomplete","span":{}}}"#,
                json_string(code),
                json_string(durable_state_name(*durable)),
                span_object(*line, *column),
            ),
            Record::OperationalError { code, .. } => format!(
                r#"{{"code":{},"kind":"run","outcome":"error"}}"#,
                json_string(code)
            ),
            Record::OutcomeUnknown { cause, cause_code } => format!(
                r#"{{"cause":{},"cause_code":{},"code":{},"kind":"run","outcome":"outcome_unknown"}}"#,
                json_string(cause),
                json_string(cause_code),
                json_string(marrow_codes::Code::RunOutcomeUnknown.as_str()),
            ),
            Record::CompilerResourceLimit { kind } => format!(
                r#"{{"code":{},"kind":"run","kind_detail":{},"outcome":"error"}}"#,
                json_string(marrow_codes::Code::CliCompilerResourceLimit.as_str()),
                json_string(kind.detail()),
            ),
        })
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
    Incomplete {
        code: &'static str,
        durable: marrow_vm::DurableCommitState,
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
            TestOutcome::Failed { code, line, column } => {
                self.fault_jsonl("failed", code, *line, *column)
            }
            TestOutcome::Errored { code, line, column } => {
                self.fault_jsonl("errored", code, *line, *column)
            }
            TestOutcome::Incomplete {
                code,
                durable,
                line,
                column,
            } => format!(
                r#"{{"code":{},"durable":{},"file":{},"kind":"test","name":{},"outcome":"incomplete","span":{}}}"#,
                json_string(code),
                json_string(durable_state_name(*durable)),
                json_string(&self.file),
                json_string(&self.name),
                span_object(*line, *column),
            ),
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
            TestOutcome::Incomplete {
                code,
                durable,
                line,
                column,
            } => format!(
                "ERROR {} ({code} at {line}:{column}; incomplete, durable {})",
                self.name,
                durable_state_name(*durable),
            ),
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
    fn selected(self) -> usize {
        self.passed + self.failed + self.errored
    }

    /// The canonical JSONL summary object, keys in ascending byte order.
    pub(crate) fn to_jsonl(self) -> String {
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
    pub(crate) fn to_text(self) -> String {
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

fn span_object(line: u32, column: u32) -> String {
    format!(r#"{{"column":{column},"line":{line}}}"#)
}

fn durable_state_name(state: marrow_vm::DurableCommitState) -> &'static str {
    match state {
        marrow_vm::DurableCommitState::KnownOld => "known_old",
        marrow_vm::DurableCommitState::KnownNew => "known_new",
        marrow_vm::DurableCommitState::Unknown => "unknown",
    }
}

/// Render a value as the JSONL `data` field, or `Err` when it exceeds the data
/// bound (the caller turns that into an operational error, never a truncation). A
/// record renders as a JSON object with field names, keys in ascending byte order.
fn render_data(
    value: Option<&Value>,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
) -> Result<String, ()> {
    // Bare strings and bytes retain their raw-text/unquoted-hex policies. An
    // enclosing aggregate instead charges all nested encoding to its own limit.
    let max_bytes = match value {
        Some(Value::Optional(Some(inner))) => return render_data(Some(inner), types, enums),
        Some(Value::Text(_)) => MAX_TEXT_BYTES * 6 + 2,
        Some(Value::Bytes(_)) => MAX_DATA_BYTES + 2,
        _ => MAX_DATA_BYTES,
    };
    let mut data = JsonData::new(max_bytes);
    data.value(value, types, enums)?;
    Ok(data.output)
}

/// One JSON data destination. Every append checks the remaining encoded bytes;
/// recursive values never retain separately rendered child strings.
struct JsonData {
    output: String,
    max_bytes: usize,
}

impl JsonData {
    fn new(max_bytes: usize) -> Self {
        Self {
            output: String::new(),
            max_bytes,
        }
    }

    fn append(&mut self, text: &str) -> Result<(), ()> {
        if text.len() > self.max_bytes - self.output.len() {
            return Err(());
        }
        self.output.push_str(text);
        Ok(())
    }

    fn string(&mut self, text: &str) -> Result<(), ()> {
        write_json_string(self, text).map_err(|_| ())
    }

    fn record(
        &mut self,
        idx: u16,
        slots: &[Option<Value>],
        types: &[SealedRecordType],
        enums: &[SealedEnumType],
    ) -> Result<(), ()> {
        let fields = types.get(idx as usize).map(SealedRecordType::fields);
        let mut entries: Vec<_> = slots
            .iter()
            .enumerate()
            .map(|(position, slot)| {
                let name = fields
                    .and_then(|fields| fields.get(position))
                    .map(|field| field.name.as_ref())
                    .unwrap_or("");
                (name, slot.as_ref())
            })
            .collect();
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        self.append("{")?;
        for (position, (name, value)) in entries.into_iter().enumerate() {
            if position > 0 {
                self.append(",")?;
            }
            self.string(name)?;
            self.append(":")?;
            self.value(value, types, enums)?;
        }
        self.append("}")
    }

    fn enum_value(
        &mut self,
        idx: u16,
        variant: u16,
        payload: &[Value],
        types: &[SealedRecordType],
        enums: &[SealedEnumType],
    ) -> Result<(), ()> {
        let enum_def = enums.get(idx as usize);
        let variant_def = enum_def.and_then(|e| e.variants().get(variant as usize));
        self.append(r#"{"enum":"#)?;
        self.string(enum_def.map(SealedEnumType::name).unwrap_or(""))?;
        self.append(r#","member":"#)?;
        self.string(variant_def.map(|v| v.name.as_ref()).unwrap_or(""))?;
        self.append(r#","payload":["#)?;
        for (position, value) in payload.iter().enumerate() {
            if position > 0 {
                self.append(",")?;
            }
            self.value(Some(value), types, enums)?;
        }
        self.append("]}")
    }

    fn value(
        &mut self,
        value: Option<&Value>,
        types: &[SealedRecordType],
        enums: &[SealedEnumType],
    ) -> Result<(), ()> {
        match value {
            None | Some(Value::Optional(None)) => self.append("null"),
            Some(Value::Int(v)) => write!(self, "{v}").map_err(|_| ()),
            Some(Value::Bool(v)) => self.append(if *v { "true" } else { "false" }),
            Some(Value::Text(v)) => {
                if v.len() > MAX_TEXT_BYTES {
                    return Err(());
                }
                self.string(v)
            }
            Some(Value::Bytes(v)) => {
                let hex = marrow_vm::render::hex_bytes(v, MAX_DATA_BYTES).map_err(|_| ())?;
                self.string(&hex)
            }
            Some(Value::Date(v)) => self.string(&marrow_vm::render::date_text(*v)),
            Some(Value::Instant(v)) => self.string(&marrow_vm::render::instant_text(*v)),
            Some(Value::Duration(v)) => self.string(&marrow_temporal::format_duration(*v)),
            Some(Value::Optional(Some(inner))) => self.value(Some(inner), types, enums),
            Some(Value::Record(idx, slots)) => self.record(*idx, slots, types, enums),
            Some(Value::Enum(idx, variant, payload)) => {
                self.enum_value(*idx, *variant, payload, types, enums)
            }
            Some(Value::List(_, _, items)) => {
                self.append("[")?;
                for (position, item) in items.iter().enumerate() {
                    if position > 0 {
                        self.append(",")?;
                    }
                    self.value(Some(item), types, enums)?;
                }
                self.append("]")
            }
            Some(Value::Map(_, _, entries)) => {
                self.append("{")?;
                for (position, (key, value)) in entries.iter().enumerate() {
                    if position > 0 {
                        self.append(",")?;
                    }
                    let key = marrow_vm::render::key_text(key, MAX_DATA_BYTES).map_err(|_| ())?;
                    self.string(&key)?;
                    self.append(":")?;
                    self.value(Some(value), types, enums)?;
                }
                self.append("}")
            }
            Some(Value::Id(_, keys)) => {
                let text = marrow_vm::render::id_text(keys, MAX_DATA_BYTES).map_err(|_| ())?;
                self.string(&text)
            }
        }
    }
}

impl fmt::Write for JsonData {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.append(text).map_err(|_| fmt::Error)
    }
}

/// Encode a string as canonical JSON: `\"`,
/// `\\`, `\b`, `\t`, `\n`, `\f`, `\r`, other C0 as lowercase `\u00XX`, everything
/// else (including `/` and all non-ASCII) passed through as UTF-8.
fn write_json_string(out: &mut dyn fmt::Write, text: &str) -> fmt::Result {
    out.write_char('"')?;
    for ch in text.chars() {
        match ch {
            '"' => out.write_str("\\\""),
            '\\' => out.write_str("\\\\"),
            '\u{08}' => out.write_str("\\b"),
            '\t' => out.write_str("\\t"),
            '\n' => out.write_str("\\n"),
            '\u{0C}' => out.write_str("\\f"),
            '\r' => out.write_str("\\r"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32),
            c => out.write_char(c),
        }?;
    }
    out.write_char('"')
}

#[expect(clippy::expect_used, reason = "writing to a String is infallible")]
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    write_json_string(&mut out, text).expect("writing to a String is infallible");
    out
}

#[cfg(test)]
mod tests {
    use super::{JsonData, MAX_DATA_BYTES, Record, json_string, render_data};
    use marrow_vm::Value;

    #[test]
    fn value_record_is_canonical_jsonl() {
        assert_eq!(
            Record::Value(Some(Value::Int(42)))
                .to_jsonl(&[], &[])
                .expect("record renders"),
            r#"{"data":42,"kind":"run","outcome":"value"}"#
        );
        assert_eq!(
            Record::Value(Some(Value::Bool(true)))
                .to_jsonl(&[], &[])
                .expect("record renders"),
            r#"{"data":true,"kind":"run","outcome":"value"}"#
        );
        assert_eq!(
            Record::Value(None)
                .to_jsonl(&[], &[])
                .expect("record renders"),
            r#"{"data":null,"kind":"run","outcome":"value"}"#
        );
    }

    /// A lost-reply outcome renders as a distinct typed state (term 13): a distinct JSONL
    /// outcome tag, a stable code, and text that tells the user the outcome is unknown, that
    /// it was not retried, and that a read-only refresh observes the current state — never a
    /// generic timeout and never a replay/exactly-once claim.
    #[test]
    fn outcome_unknown_is_a_distinct_typed_state() {
        assert_eq!(
            Record::OutcomeUnknown {
                cause: "wire",
                cause_code: "wire.malformed",
            }
            .to_jsonl(&[], &[])
            .expect("record renders"),
            r#"{"cause":"wire","cause_code":"wire.malformed","code":"run.outcome_unknown","kind":"run","outcome":"outcome_unknown"}"#,
        );
        let text = Record::OutcomeUnknown {
            cause: "wire",
            cause_code: "wire.malformed",
        }
        .to_text(&[], &[])
        .expect("record renders");
        assert!(
            text.contains("run.outcome_unknown"),
            "carries the code: {text}"
        );
        assert!(text.contains("wire.malformed"));
        assert!(
            text.contains("outcome is unknown"),
            "names the state: {text}"
        );
        assert!(
            text.contains("not retried"),
            "states no automatic replay occurred: {text}"
        );
        assert!(
            text.contains("read-only"),
            "points at a read-only refresh: {text}"
        );
        assert!(
            !text.to_lowercase().contains("timed out") && !text.to_lowercase().contains("timeout"),
            "is not a generic timeout: {text}"
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
            .to_jsonl(&[], &[])
            .expect("record renders")
            .contains(r#""outcome":"diagnostic""#)
        );
        assert!(
            Record::ArtifactRejected {
                code: "image.function"
            }
            .to_jsonl(&[], &[])
            .expect("record renders")
            .contains(r#""outcome":"artifact_rejected""#)
        );
        assert!(
            Record::Fault {
                code: "run.overflow",
                line: 1,
                column: 1,
                detail: None,
            }
            .to_jsonl(&[], &[])
            .expect("record renders")
            .contains(r#""outcome":"fault""#)
        );
        assert_eq!(
            Record::Incomplete {
                code: "run.commit",
                durable: marrow_vm::DurableCommitState::KnownOld,
                line: 7,
                column: 9,
            }
            .to_jsonl(&[], &[])
            .expect("record renders"),
            r#"{"code":"run.commit","durable":"known_old","kind":"run","outcome":"incomplete","span":{"column":9,"line":7}}"#,
        );
        assert!(
            Record::OperationalError {
                code: "store.io",
                detail: None,
            }
            .to_jsonl(&[], &[])
            .expect("record renders")
            .contains(r#""outcome":"error""#)
        );
    }

    /// A typed operational message names the file and reason in text output but never
    /// reaches the KAT-frozen JSONL surface, which stays the code alone.
    #[test]
    fn operational_detail_is_text_only() {
        let record = Record::OperationalError {
            code: "project.ids_corrupt",
            detail: Some(".marrow/ids: unresolved Git conflict markers".to_string()),
        };
        assert_eq!(
            record.to_text(&[], &[]).expect("record renders"),
            "project.ids_corrupt: .marrow/ids: unresolved Git conflict markers"
        );
        assert_eq!(
            record.to_jsonl(&[], &[]).expect("record renders"),
            r#"{"code":"project.ids_corrupt","kind":"run","outcome":"error"}"#
        );
    }

    /// The compiler resource-limit record projects one typed kind two ways: the frozen
    /// `kind_detail` identifier on the JSONL surface (keys in ascending byte order:
    /// `code`, `kind`, `kind_detail`, `outcome`), and the kind's own words in text, so
    /// which aggregate bound fired is legible to a tool and to a person without
    /// re-running — and no Rust variant name reaches the terminal.
    #[test]
    fn compiler_resource_limit_projects_the_kind_for_each_surface() {
        let record = Record::CompilerResourceLimit {
            kind: marrow_compile::ResourceLimitKind::Exports,
        };
        assert_eq!(
            record.to_jsonl(&[], &[]).expect("record renders"),
            r#"{"code":"cli.compiler_resource_limit","kind":"run","kind_detail":"Exports","outcome":"error"}"#
        );
        assert_eq!(
            record.to_text(&[], &[]).expect("record renders"),
            "cli.compiler_resource_limit: the export table is full"
        );
    }

    /// Keys within an object are in ascending byte order, including the
    /// nested span object (`column` before `line`).
    #[test]
    fn keys_are_in_ascending_byte_order() {
        let line = Record::Fault {
            code: "run.overflow",
            line: 7,
            column: 2,
            detail: None,
        }
        .to_jsonl(&[], &[])
        .expect("record renders");
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

    #[test]
    fn json_data_writer_retains_its_prefix_on_refusal() {
        let mut data = JsonData::new(3);
        assert_eq!(data.append("é"), Ok(()));
        assert_eq!(data.append("a"), Ok(()));
        assert_eq!(data.output, "éa");
        assert_eq!(data.append("b"), Err(()));
        assert_eq!(data.output, "éa");
        assert_eq!(data.append(""), Ok(()));

        for text in ["a\"b\\c", "\u{08}\t\n\u{0C}\r", "\u{01}", "café ☕/"] {
            let expected = json_string(text);
            let mut exact = JsonData::new(expected.len());
            assert_eq!(exact.string(text), Ok(()));
            assert_eq!(exact.output, expected);
            let mut short = JsonData::new(expected.len() - 1);
            assert_eq!(short.string(text), Err(()));
            assert!(!short.output.is_empty());
            assert!(expected.starts_with(&short.output));
            assert!(short.output.len() < expected.len());
        }
    }

    #[test]
    fn json_value_forms_share_the_encoded_destination() {
        use marrow_kernel::codec::key::KeyScalar;
        use std::rc::Rc;

        let cases = [
            (Value::Int(i64::MIN), "-9223372036854775808"),
            (Value::Bool(false), "false"),
            (Value::Text("é\n".into()), r#""é\n""#),
            (Value::Bytes(vec![0, 171, 255].into()), r#""0x00abff""#),
            (Value::Date(0), r#""1970-01-01""#),
            (Value::Instant(0), r#""1970-01-01T00:00:00Z""#),
            (Value::Duration(0), r#""PT0S""#),
            (Value::Optional(None), "null"),
            (Value::Optional(Some(Box::new(Value::Bool(true)))), "true"),
            (Value::list(0, Rc::new(Vec::new())), "[]"),
            (Value::map(0, Rc::new(Vec::new())), "{}"),
            (
                Value::map(
                    0,
                    Rc::new(vec![
                        (
                            KeyScalar::Str("a\"\n".to_string()),
                            Value::Text("é\\".into()),
                        ),
                        (KeyScalar::Str("z".to_string()), Value::Optional(None)),
                    ]),
                ),
                r#"{"a\"\n":"é\\","z":null}"#,
            ),
            // Missing metadata keeps the existing empty-name fallbacks and stable
            // field order when those names compare equal.
            (
                Value::Record(0, vec![Some(Value::Int(7)), None].into_boxed_slice()),
                r#"{"":7,"":null}"#,
            ),
            (
                Value::Enum(
                    0,
                    0,
                    vec![Value::list(
                        0,
                        Rc::new(vec![Value::Text("é\n".into()), Value::Optional(None)]),
                    )]
                    .into_boxed_slice(),
                ),
                r#"{"enum":"","member":"","payload":[["é\n",null]]}"#,
            ),
            (
                Value::Id(
                    0,
                    vec![KeyScalar::Str("é\n".to_string()), KeyScalar::Int(-2)].into(),
                ),
                r#""Id(é\n, -2)""#,
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(
                render_data(Some(&value), &[], &[]),
                Ok(expected.to_string())
            );
            let mut exact = JsonData::new(expected.len());
            assert_eq!(exact.value(Some(&value), &[], &[]), Ok(()));
            assert_eq!(exact.output, expected);
            let mut short = JsonData::new(expected.len() - 1);
            assert_eq!(short.value(Some(&value), &[], &[]), Err(()));
            assert!(expected.starts_with(&short.output));
            assert!(short.output.len() < expected.len());
        }
    }

    #[test]
    fn outer_and_nested_json_limits_preserve_their_units() {
        use marrow_kernel::codec::key::KeyScalar;
        use std::rc::Rc;

        let raw = Value::Text("\0".repeat(MAX_DATA_BYTES).into());
        let optional = Value::Optional(Some(Box::new(raw.clone())));
        let encoded = render_data(Some(&optional), &[], &[]).expect("transparent raw text limit");
        assert_eq!(encoded.len(), MAX_DATA_BYTES * 6 + 2);
        let nested = Value::list(0, Rc::new(vec![raw]));
        assert_eq!(render_data(Some(&nested), &[], &[]), Err(()));
        assert_eq!(
            Record::Value(Some(nested)).to_text(&[], &[]),
            Ok(format!("[{}]", "\0".repeat(MAX_DATA_BYTES))),
        );

        for (extra, accepted) in [(0, true), (1, false)] {
            let list = Value::list(
                0,
                Rc::new(vec![Value::Text(
                    "a".repeat(MAX_DATA_BYTES - 4 + extra).into(),
                )]),
            );
            let id = Value::Id(
                0,
                vec![KeyScalar::Str("a".repeat(MAX_DATA_BYTES - 6 + extra))].into(),
            );
            for value in [list, id] {
                let result = render_data(Some(&value), &[], &[]);
                if accepted {
                    assert_eq!(result.expect("exact encoded limit").len(), MAX_DATA_BYTES);
                } else {
                    assert_eq!(result, Err(()));
                }
            }
        }

        let bytes = Value::Bytes(vec![0; (MAX_DATA_BYTES - 2) / 2].into());
        assert_eq!(
            render_data(
                Some(&Value::Optional(Some(Box::new(bytes.clone())))),
                &[],
                &[]
            )
            .expect("transparent unquoted hex limit")
            .len(),
            MAX_DATA_BYTES + 2,
        );
        let list = Value::list(0, Rc::new(vec![bytes]));
        assert_eq!(render_data(Some(&list), &[], &[]), Err(()));
        let exact = Value::list(
            0,
            Rc::new(vec![Value::Bytes(vec![0; (MAX_DATA_BYTES - 6) / 2].into())]),
        );
        assert_eq!(
            render_data(Some(&exact), &[], &[])
                .expect("nested exact hex limit")
                .len(),
            MAX_DATA_BYTES,
        );
    }

    #[test]
    fn bare_string_bounds_precede_text_and_json_rendering() {
        for input in ["é".repeat(32_768), "\0".repeat(65_536)] {
            let record = Record::Value(Some(Value::Text(input.as_str().into())));
            assert_eq!(record.to_text(&[], &[]).expect("exact raw limit"), input);
            let json = record
                .to_jsonl(&[], &[])
                .expect("escaping remains admitted");
            let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
            assert_eq!(parsed["data"], input);
        }
        let record = Record::Value(Some(Value::Text("a".repeat(65_537).into())));
        assert_eq!(record.to_text(&[], &[]), Err(()));
        assert_eq!(record.to_jsonl(&[], &[]), Err(()));
    }

    #[test]
    fn json_bytes_limit_counts_unquoted_hex() {
        let value = Value::Bytes(vec![0; (MAX_DATA_BYTES - 2) / 2].into());
        let data = render_data(Some(&value), &[], &[]).expect("exact unquoted hex limit");
        assert_eq!(data.len(), MAX_DATA_BYTES + 2);
        assert!(data.starts_with("\"0x"));
        assert!(data.ends_with('"'));
        assert!(
            data.as_bytes()[3..data.len() - 1]
                .iter()
                .all(|byte| *byte == b'0')
        );

        let excess = Value::Bytes(vec![0; MAX_DATA_BYTES / 2].into());
        assert_eq!(render_data(Some(&excess), &[], &[]), Err(()));
    }

    #[test]
    fn json_aggregate_refuses_before_crossing_data_limit() {
        use std::rc::Rc;

        let nested = |width| {
            let inner = Value::list(0, Rc::new(vec![Value::Text("".into()); width]));
            Value::list(1, Rc::new(vec![inner; width]))
        };
        assert_eq!(
            Record::Value(Some(nested(2))).to_jsonl(&[], &[]),
            Ok(r#"{"data":[["",""],["",""]],"kind":"run","outcome":"value"}"#.to_string()),
        );

        let value = nested(256);
        let Value::List(outer_type, outer_bytes, items) = &value else {
            panic!("the result is a list");
        };
        assert_eq!((*outer_type, *outer_bytes, items.len()), (1, 256, 256));
        assert_eq!(value.structural_bytes(), 257);
        assert!(marrow_vm::collection_within_limits(
            items.len(),
            *outer_bytes
        ));
        let Value::List(_, _, first) = &items[0] else {
            panic!("the first item is a list");
        };
        for item in items.iter() {
            let Value::List(inner_type, inner_bytes, leaves) = item else {
                panic!("every item is a list");
            };
            assert_eq!((*inner_type, *inner_bytes, leaves.len()), (0, 0, 256));
            assert!(marrow_vm::collection_within_limits(
                leaves.len(),
                *inner_bytes
            ));
            assert!(Rc::ptr_eq(first, leaves));
            assert!(
                leaves
                    .iter()
                    .all(|leaf| matches!(leaf, Value::Text(text) if text.is_empty()))
            );
        }

        assert_eq!(render_data(Some(&value), &[], &[]), Err(()));
        let mut data = JsonData::new(MAX_DATA_BYTES);
        assert_eq!(data.value(Some(&value), &[], &[]), Err(()));
        assert_eq!(Record::Value(Some(value)).to_jsonl(&[], &[]), Err(()));
        let inner_json = format!("[{}]", vec![r#""""#; 256].join(","));
        let expected = format!("[{}]", vec![inner_json; 256].join(","));
        assert_eq!(expected.len(), 197_121);
        assert!(!data.output.is_empty());
        assert!(expected.starts_with(&data.output));
        assert!(data.output.len() <= MAX_DATA_BYTES);
    }
}
