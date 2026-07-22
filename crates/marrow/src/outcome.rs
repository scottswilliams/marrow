//! The typed CLI outcome owner (design §H).
//!
//! One typed [`Record`] preserves all four failure families as distinct variants;
//! its JSONL projection is a canonical one-object-per-line surface the differential
//! harness and (later) `marrow test` consume. The four families never collapse: a
//! source diagnostic, an artifact rejection, a source-mapped runtime fault, and an
//! owner-local operational error are distinct records.

use marrow_verify::{SealedEnumType, SealedRecordType};
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
    /// bare operational error it carries the typed kind detail — which fixed bound was
    /// exhausted — so a caller (or a bound-raise audit) can bisect which limit fired
    /// without re-running. `kind_detail` is a frozen identifier from
    /// [`marrow_compile::ResourceLimitKind::detail`]; the record still carries no numeric
    /// limit and no source location. The code is always `cli.compiler_resource_limit`.
    CompilerResourceLimit { kind_detail: &'static str },
}

impl Record {
    /// The plain-text rendering for the default (non-JSONL) format. `types` supplies
    /// the field names of a returned record value; it is empty for the non-value
    /// families, which never render a record.
    pub(crate) fn to_text(&self, types: &[SealedRecordType], enums: &[SealedEnumType]) -> String {
        match self {
            Record::Value(Some(value)) => render_value_text(value, types, enums),
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
            Record::CompilerResourceLimit { kind_detail } => format!(
                "{}: {kind_detail}",
                marrow_codes::Code::CliCompilerResourceLimit.as_str()
            ),
        }
    }

    /// The canonical single-line JSONL projection: one object, keys in ascending
    /// byte order, LF added by the caller.
    pub(crate) fn to_jsonl(&self, types: &[SealedRecordType], enums: &[SealedEnumType]) -> String {
        match self {
            Record::Value(value) => match render_data(value.as_ref(), types, enums) {
                Ok(data) => format!(r#"{{"data":{data},"kind":"run","outcome":"value"}}"#),
                Err(()) => Record::OperationalError {
                    code: marrow_codes::Code::IoWrite.as_str(),
                    detail: None,
                }
                .to_jsonl(types, enums),
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
            Record::CompilerResourceLimit { kind_detail } => format!(
                r#"{{"code":{},"kind":"run","kind_detail":{},"outcome":"error"}}"#,
                json_string(marrow_codes::Code::CliCompilerResourceLimit.as_str()),
                json_string(kind_detail),
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

/// The canonical text of a returned value. `run` delegates to
/// [`marrow_vm::render::value_text`], which renders every value shape (scalars, enums,
/// identities, records, lists, maps, optionals).
fn render_value_text(
    value: &Value,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
) -> String {
    marrow_vm::render::value_text(value, types, enums)
}

/// Render a value as the JSONL `data` field, or `Err` when it exceeds the data
/// bound (the caller turns that into an operational error, never a truncation). A
/// record renders as a JSON object with field names, keys in ascending byte order.
fn render_data(
    value: Option<&Value>,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
) -> Result<String, ()> {
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
            json_string(&marrow_vm::render::hex_bytes(v))
        }
        // Temporal values render as their canonical text in a JSON string, like bytes.
        Some(Value::Date(v)) => json_string(&marrow_vm::render::date_text(*v)),
        Some(Value::Instant(v)) => json_string(&marrow_vm::render::instant_text(*v)),
        Some(Value::Duration(v)) => json_string(&marrow_temporal::format_duration(*v)),
        Some(Value::Optional(Some(inner))) => render_data(Some(inner), types, enums)?,
        Some(Value::Record(idx, slots)) => {
            let fields = types.get(*idx as usize).map(SealedRecordType::fields);
            let mut entries: Vec<(&str, String)> = Vec::with_capacity(slots.len());
            for (position, slot) in slots.iter().enumerate() {
                let name = fields
                    .and_then(|fields| fields.get(position))
                    .map(|field| field.name.as_ref())
                    .unwrap_or("");
                entries.push((name, render_data(slot.as_ref(), types, enums)?));
            }
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            let mut out = String::from("{");
            for (position, (name, rendered)) in entries.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(name));
                out.push(':');
                out.push_str(rendered);
            }
            out.push('}');
            if out.len() > MAX_DATA_BYTES {
                return Err(());
            }
            out
        }
        // An enum value renders `{"enum": ..., "member": ..., "payload": [...]}`,
        // keys in ascending byte order.
        Some(Value::Enum(enum_idx, variant, payload)) => {
            let enum_def = enums.get(*enum_idx as usize);
            let variant_def = enum_def.and_then(|e| e.variants().get(*variant as usize));
            let enum_name = enum_def.map(SealedEnumType::name).unwrap_or("");
            let member = variant_def.map(|v| v.name.as_ref()).unwrap_or("");
            let mut items = Vec::with_capacity(payload.len());
            for value in payload.iter() {
                items.push(render_data(Some(value), types, enums)?);
            }
            let out = format!(
                r#"{{"enum":{},"member":{},"payload":[{}]}}"#,
                json_string(enum_name),
                json_string(member),
                items.join(",")
            );
            if out.len() > MAX_DATA_BYTES {
                return Err(());
            }
            out
        }
        // A list renders as a JSON array in insertion order.
        Some(Value::List(_, _, items)) => {
            let mut rendered = Vec::with_capacity(items.len());
            for item in items.iter() {
                rendered.push(render_data(Some(item), types, enums)?);
            }
            let out = format!("[{}]", rendered.join(","));
            if out.len() > MAX_DATA_BYTES {
                return Err(());
            }
            out
        }
        // A map renders as a JSON object with string-rendered keys in ascending key
        // order (entries are stored sorted).
        Some(Value::Map(_, _, entries)) => {
            let mut out = String::from("{");
            for (position, (key, value)) in entries.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(&marrow_vm::render::key_text(key)));
                out.push(':');
                out.push_str(&render_data(Some(value), types, enums)?);
            }
            out.push('}');
            if out.len() > MAX_DATA_BYTES {
                return Err(());
            }
            out
        }
        // An entry identity renders as its `Id(k0, k1)` text in a JSON string.
        Some(Value::Id(_, keys)) => {
            let out = json_string(&marrow_vm::render::id_text(keys));
            if out.len() > MAX_DATA_BYTES {
                return Err(());
            }
            out
        }
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
            Record::Value(Some(Value::Int(42))).to_jsonl(&[], &[]),
            r#"{"data":42,"kind":"run","outcome":"value"}"#
        );
        assert_eq!(
            Record::Value(Some(Value::Bool(true))).to_jsonl(&[], &[]),
            r#"{"data":true,"kind":"run","outcome":"value"}"#
        );
        assert_eq!(
            Record::Value(None).to_jsonl(&[], &[]),
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
            .to_jsonl(&[], &[]),
            r#"{"cause":"wire","cause_code":"wire.malformed","code":"run.outcome_unknown","kind":"run","outcome":"outcome_unknown"}"#,
        );
        let text = Record::OutcomeUnknown {
            cause: "wire",
            cause_code: "wire.malformed",
        }
        .to_text(&[], &[]);
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
            .contains(r#""outcome":"diagnostic""#)
        );
        assert!(
            Record::ArtifactRejected {
                code: "image.function"
            }
            .to_jsonl(&[], &[])
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
            .contains(r#""outcome":"fault""#)
        );
        assert_eq!(
            Record::Incomplete {
                code: "run.commit",
                durable: marrow_vm::DurableCommitState::KnownOld,
                line: 7,
                column: 9,
            }
            .to_jsonl(&[], &[]),
            r#"{"code":"run.commit","durable":"known_old","kind":"run","outcome":"incomplete","span":{"column":9,"line":7}}"#,
        );
        assert!(
            Record::OperationalError {
                code: "store.io",
                detail: None,
            }
            .to_jsonl(&[], &[])
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
            record.to_text(&[], &[]),
            "project.ids_corrupt: .marrow/ids: unresolved Git conflict markers"
        );
        assert_eq!(
            record.to_jsonl(&[], &[]),
            r#"{"code":"project.ids_corrupt","kind":"run","outcome":"error"}"#
        );
    }

    /// The compiler resource-limit record carries the typed kind detail on the frozen
    /// JSONL surface (keys in ascending byte order: `code`, `kind`, `kind_detail`,
    /// `outcome`) and in text output, so which aggregate bound fired is legible without
    /// re-running.
    #[test]
    fn compiler_resource_limit_carries_the_kind_detail() {
        let record = Record::CompilerResourceLimit {
            kind_detail: "Exports",
        };
        assert_eq!(
            record.to_jsonl(&[], &[]),
            r#"{"code":"cli.compiler_resource_limit","kind":"run","kind_detail":"Exports","outcome":"error"}"#
        );
        assert_eq!(
            record.to_text(&[], &[]),
            "cli.compiler_resource_limit: Exports"
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
        .to_jsonl(&[], &[]);
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
