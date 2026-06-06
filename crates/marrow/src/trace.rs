//! The execution-trace hook shared by `run --trace` and `test --trace`.
//!
//! A [`TraceHook`] is a [`StepHook`] that observes each statement and each
//! managed write as the run executes, reporting them in execution order. Under
//! text format it prints an indented, depth-aware stream to standard error,
//! leaving the program's own `print`/`write` on standard out; under `json`/`jsonl`
//! it emits one `step`/`write` record per event on standard out. It never alters
//! the run: it only observes, so a traced run does exactly what an untraced one
//! does, plus the trace.

use std::collections::HashMap;

use marrow_check::CheckedRuntimeProgram;
use marrow_check::StoredValueMeaning;
use marrow_run::{Frame, RuntimeError, StepHook, WriteDataSegment, WriteOp, WriteTarget};
use marrow_store::key::SavedKey;
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};
use marrow_syntax::SourceSpan;
use serde_json::json;

use crate::CheckFormat;

/// Observes a run and reports each statement and managed write. The `label`
/// prefixes every event under text format and is carried on each JSON record, so
/// `test --trace` can attribute a trace to the test it belongs to; a plain `run`
/// passes an empty label.
pub(crate) struct TraceHook {
    format: CheckFormat,
    label: String,
    names: WriteTargetNames,
    records: Vec<serde_json::Value>,
}

impl TraceHook {
    pub(crate) fn new(
        format: CheckFormat,
        label: impl Into<String>,
        program: &CheckedRuntimeProgram,
    ) -> Self {
        Self {
            format,
            label: label.into(),
            names: WriteTargetNames::from_program(program),
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

    fn before_write(
        &mut self,
        op: WriteOp,
        target: &WriteTarget,
        value: Option<&[u8]>,
        depth: usize,
    ) {
        let op_name = match op {
            WriteOp::Write => "write",
            WriteOp::Delete => "delete",
        };
        let rendered_target = render_write_target(target, &self.names);
        match self.format {
            CheckFormat::Text => {
                let line = match value {
                    Some(bytes) => {
                        format!(
                            "{op_name} {rendered_target} = {}",
                            self.names.render_leaf_value(target, bytes)
                        )
                    }
                    None => format!("{op_name} {rendered_target}"),
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
                    "target": write_target_json(target, &self.names),
                    "path": rendered_target,
                    "value_b64": value.map(marrow_run::base64::encode),
                    "depth": depth,
                }));
            }
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct WriteTargetNames {
    stores: HashMap<String, String>,
    members: HashMap<String, String>,
    member_meanings: HashMap<String, StoredValueMeaning>,
    indexes: HashMap<String, (String, String)>,
}

impl WriteTargetNames {
    pub(crate) fn from_program(program: &CheckedRuntimeProgram) -> Self {
        let facts = program.facts();
        let mut names = Self::default();
        for store in facts.stores() {
            if let Some(catalog_id) = &store.catalog_id {
                names.stores.insert(catalog_id.clone(), store.root.clone());
            }
        }
        for member in facts.resource_members() {
            let Some(catalog_id) = &member.catalog_id else {
                continue;
            };
            names
                .members
                .insert(catalog_id.clone(), member.name.clone());
            if let Some(meaning) = &member.value_meaning {
                names
                    .member_meanings
                    .insert(catalog_id.clone(), meaning.clone());
            }
        }
        for index in facts.store_indexes() {
            let Some(catalog_id) = &index.catalog_id else {
                continue;
            };
            let root = facts.store(index.store).root.clone();
            names
                .indexes
                .insert(catalog_id.clone(), (root, index.name.clone()));
        }
        names
    }

    /// Render a managed write's leaf value as its declared typed scalar, looked up
    /// by the write's leaf member. The canonical saved text of every non-bool scalar
    /// is its stored bytes (an int as its digits, a date as `YYYY-MM-DD`, raw bytes
    /// as `0x<hex>`), so only a `bool` needs decoding to read `true`/`false`; every
    /// other value renders straight from its bytes with no decode/encode round-trip.
    /// A value whose meaning is not a scalar (an identity reference or enum member)
    /// also renders from its raw bytes. The JSON `value_b64` field stays the raw bytes.
    pub(crate) fn render_leaf_value(&self, target: &WriteTarget, value: &[u8]) -> String {
        if let Some(StoredValueMeaning::Scalar(ScalarType::Bool)) = self.leaf_meaning(target)
            && let Some(SavedValue::Bool(flag)) = decode_value(value, ScalarType::Bool)
        {
            return flag.to_string();
        }
        crate::render_value_bytes(value)
    }

    fn leaf_meaning(&self, target: &WriteTarget) -> Option<&StoredValueMeaning> {
        let WriteTarget::Data { path, .. } = target else {
            return None;
        };
        let member = path.iter().rev().find_map(|segment| match segment {
            WriteDataSegment::Member(member) => Some(member),
            WriteDataSegment::Key(_) => None,
        })?;
        self.member_meanings.get(member)
    }
}

pub(crate) fn render_write_target(target: &WriteTarget, names: &WriteTargetNames) -> String {
    match target {
        WriteTarget::Data {
            store,
            identity,
            path,
        } => {
            let mut rendered = names
                .stores
                .get(store)
                .map(|root| format!("^{root}"))
                .unwrap_or_else(|| format!("data:{store}"));
            if !identity.is_empty() {
                rendered.push('(');
                rendered.push_str(&render_keys(identity));
                rendered.push(')');
            }
            for segment in path {
                match segment {
                    WriteDataSegment::Member(member) => {
                        rendered.push('.');
                        rendered.push_str(names.members.get(member).map_or(member, String::as_str));
                    }
                    WriteDataSegment::Key(key) => {
                        rendered.push('(');
                        rendered.push_str(&render_key(key));
                        rendered.push(')');
                    }
                }
            }
            rendered
        }
        WriteTarget::Index {
            index,
            keys,
            identity,
        } => match names.indexes.get(index) {
            Some((root, name)) => format!(
                "index:^{root}.{name}({}) -> ({})",
                render_keys(keys),
                render_keys(identity)
            ),
            None => format!(
                "index:{index}({}) -> ({})",
                render_keys(keys),
                render_keys(identity)
            ),
        },
        WriteTarget::Meta { catalog_epoch } => format!("meta:catalog-epoch={catalog_epoch}"),
    }
}

pub(crate) fn write_target_json(
    target: &WriteTarget,
    names: &WriteTargetNames,
) -> serde_json::Value {
    match target {
        WriteTarget::Data {
            store,
            identity,
            path,
        } => json!({
            "kind": "data",
            "store": names.stores.get(store).map_or(store.as_str(), String::as_str),
            "identity": identity.iter().map(render_key).collect::<Vec<_>>(),
            "path": path.iter().map(|segment| write_data_segment_json(segment, names)).collect::<Vec<_>>(),
        }),
        WriteTarget::Index {
            index,
            keys,
            identity,
        } => json!({
            "kind": "index",
            "index": names
                .indexes
                .get(index)
                .map(|(root, name)| format!("^{root}.{name}"))
                .unwrap_or_else(|| index.clone()),
            "keys": keys.iter().map(render_key).collect::<Vec<_>>(),
            "identity": identity.iter().map(render_key).collect::<Vec<_>>(),
        }),
        WriteTarget::Meta { catalog_epoch } => json!({
            "kind": "meta",
            "catalogEpoch": catalog_epoch,
        }),
    }
}

fn write_data_segment_json(
    segment: &WriteDataSegment,
    names: &WriteTargetNames,
) -> serde_json::Value {
    match segment {
        WriteDataSegment::Member(member) => {
            json!({ "member": names.members.get(member).map_or(member.as_str(), String::as_str) })
        }
        WriteDataSegment::Key(key) => json!({ "key": render_key(key) }),
    }
}

fn render_keys(keys: &[SavedKey]) -> String {
    keys.iter().map(render_key).collect::<Vec<_>>().join(", ")
}

fn render_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => format!("{value:?}"),
        SavedKey::Date(value) => render_temporal_key(SavedValue::Date(*value)),
        SavedKey::Duration(value) => render_temporal_key(SavedValue::Duration(*value)),
        SavedKey::Instant(value) => render_temporal_key(SavedValue::Instant(*value)),
        SavedKey::Bytes(value) => format!("bytes:{}", marrow_run::base64::encode(value)),
    }
}

fn render_temporal_key(value: SavedValue) -> String {
    encode_value(&value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| format!("{value:?}"))
}
