//! The execution-trace hook shared by `run --trace` and `test --trace`.
//!
//! A [`TraceHook`] is a [`StepHook`] that observes each statement and each
//! managed write as the run executes, reporting them in execution order. Under
//! text format it prints an indented, depth-aware stream to standard error,
//! leaving the program's own `print`/`write` on standard out; under `json`/`jsonl`
//! it emits one `step`/`write` record per event on standard out. It never alters
//! the run: it only observes, so a traced run does exactly what an untraced one
//! does, plus the trace.

use marrow_run::{Frame, RuntimeError, StepHook, WriteOp};
use marrow_store::path::display_path;
use marrow_syntax::SourceSpan;
use serde_json::json;

use crate::CheckFormat;
use crate::cmd_data::render_value_bytes;

/// Observes a run and reports each statement and managed write. The `label`
/// prefixes every event under text format and is carried on each JSON record, so
/// `test --trace` can attribute a trace to the test it belongs to; a plain `run`
/// passes an empty label.
pub(crate) struct TraceHook {
    format: CheckFormat,
    label: String,
    records: Vec<serde_json::Value>,
}

impl TraceHook {
    pub(crate) fn new(format: CheckFormat, label: impl Into<String>) -> Self {
        Self {
            format,
            label: label.into(),
            records: Vec::new(),
        }
    }

    /// Emit the collected JSON records (for `json`/`jsonl`), then reset them. Text
    /// traces print as they happen and leave nothing to flush. `json` wraps the
    /// records in one object; `jsonl` streams them followed by a summary line.
    pub(crate) fn flush(&mut self) {
        let records = std::mem::take(&mut self.records);
        match self.format {
            CheckFormat::Text => {}
            CheckFormat::Json => crate::write_json(json!({
                "trace": self.label,
                "events": records,
            })),
            CheckFormat::Jsonl => {
                for record in &records {
                    crate::write_json(record.clone());
                }
                crate::write_json(json!({
                    "kind": "summary",
                    "trace": self.label,
                    "events": records.len(),
                }));
            }
        }
    }

    /// The text prefix for an event: the label (when tracing a named test) and an
    /// indent of two spaces per nested call, so the stream reads as a call tree.
    fn text_indent(&self, depth: usize) -> String {
        let mut prefix = String::new();
        if !self.label.is_empty() {
            prefix.push_str(&self.label);
            prefix.push_str(": ");
        }
        // Depth 1 is the entry activation, so indent by the depth past it.
        prefix.push_str(&"  ".repeat(depth.saturating_sub(1)));
        prefix
    }
}

impl StepHook for TraceHook {
    fn before_statement(
        &mut self,
        span: SourceSpan,
        frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError> {
        let depth = frame.depth();
        let file = frame
            .file()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let locals: Vec<(String, String)> = frame
            .locals()
            .map(|(name, value)| (name.to_string(), value.display_debug()))
            .collect();
        match self.format {
            CheckFormat::Text => {
                let locals_text = locals
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join("  ");
                let location = format!("{file}:{}", span.line);
                if locals_text.is_empty() {
                    eprintln!("{}{location}", self.text_indent(depth));
                } else {
                    eprintln!("{}{location}\t{locals_text}", self.text_indent(depth));
                }
            }
            CheckFormat::Json | CheckFormat::Jsonl => {
                let locals_json: Vec<serde_json::Value> = locals
                    .iter()
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect();
                self.records.push(json!({
                    "kind": "step",
                    "trace": self.label,
                    "file": file,
                    "line": span.line,
                    "depth": depth,
                    "locals": locals_json,
                }));
            }
        }
        Ok(())
    }

    fn before_write(&mut self, op: WriteOp, path: &[u8], value: Option<&[u8]>, depth: usize) {
        let op_name = match op {
            WriteOp::Write => "write",
            WriteOp::Delete => "delete",
        };
        let rendered_path = display_path(path);
        match self.format {
            CheckFormat::Text => {
                let line = match value {
                    Some(bytes) => {
                        format!("{op_name} {rendered_path} = {}", render_value_bytes(bytes))
                    }
                    None => format!("{op_name} {rendered_path}"),
                };
                // A write is caused by the statement at the current depth; indent it
                // one level past that statement so it nests under its cause.
                eprintln!("{}{line}", self.text_indent(depth + 1));
            }
            CheckFormat::Json | CheckFormat::Jsonl => {
                self.records.push(json!({
                    "kind": "write",
                    "trace": self.label,
                    "op": op_name,
                    "path": rendered_path,
                    "value_b64": value.map(marrow_run::base64::encode),
                    "depth": depth,
                }));
            }
        }
    }
}
