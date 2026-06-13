//! The execution-trace hook shared by `run --trace` and `test --trace`.
//!
//! A [`TraceHook`] observes each statement and managed write as the run executes
//! and reports them in execution order, on standard error to leave the program's
//! `print` output alone on stdout. It only observes: a traced run does exactly what
//! an untraced one does, plus the trace.

use std::collections::HashMap;

use marrow_check::StoredValueMeaning;
use marrow_check::{CheckedRuntimeProgram, StoreFact, StoreId};
use marrow_run::{Frame, RuntimeError, StepHook, WriteDataSegment, WriteOp, WriteTarget};
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::decode_tree_enum_member;
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};
use marrow_syntax::SourceSpan;
use serde_json::json;
/// Observes a run and reports each statement and managed write. `label` prefixes
/// each event so `test --trace` can attribute a trace to its test; a plain `run`
/// passes an empty label.
pub(crate) struct TraceHook {
    label: String,
    names: WriteTargetNames,
}

impl TraceHook {
    pub(crate) fn new(label: impl Into<String>, program: &CheckedRuntimeProgram) -> Self {
        Self {
            label: label.into(),
            names: WriteTargetNames::from_program(program),
        }
    }

    /// Text traces print as they happen and leave nothing to flush.
    pub(crate) fn flush(&mut self) {}

    /// The text prefix for an event: the label and two spaces per nested call, so
    /// the stream reads as a call tree.
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
}

#[derive(Clone, Default)]
pub(crate) struct WriteTargetNames {
    stores: HashMap<String, String>,
    identity_stores: HashMap<StoreId, StoreFact>,
    members: HashMap<String, String>,
    member_meanings: HashMap<String, StoredValueMeaning>,
    enum_catalogs: HashMap<marrow_check::EnumId, String>,
    enum_members: HashMap<(marrow_check::EnumId, String), String>,
    indexes: HashMap<String, IndexName>,
}

#[derive(Clone)]
struct IndexName {
    root: String,
    name: String,
    key_meanings: Vec<StoredValueMeaning>,
}

impl WriteTargetNames {
    pub(crate) fn from_program(program: &CheckedRuntimeProgram) -> Self {
        let facts = program.facts();
        let mut names = Self::default();
        for store in facts.stores() {
            names.identity_stores.insert(store.id, store.clone());
            if let Some(catalog_id) = &store.catalog_id {
                names.stores.insert(catalog_id.clone(), store.root.clone());
            }
        }
        for enum_fact in facts.enums() {
            if let Some(catalog_id) = &enum_fact.catalog_id {
                names.enum_catalogs.insert(enum_fact.id, catalog_id.clone());
            }
        }
        for member in facts.enum_members() {
            let Some(member_catalog_id) = &member.catalog_id else {
                continue;
            };
            if !names.enum_catalogs.contains_key(&member.enum_id) {
                continue;
            }
            if !facts.enum_member_is_selectable(member.id) {
                continue;
            }
            if let Some(path) = facts.enum_member_catalog_path(member.id) {
                names
                    .enum_members
                    .insert((member.enum_id, member_catalog_id.clone()), path);
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
            names.indexes.insert(
                catalog_id.clone(),
                IndexName {
                    root,
                    name: index.name.clone(),
                    key_meanings: index
                        .keys
                        .iter()
                        .map(|key| key.value_meaning.clone())
                        .collect(),
                },
            );
        }
        names
    }

    /// Render a managed write's leaf value through its declared stored meaning,
    /// falling back to raw bytes when the payload does not match that meaning.
    pub(crate) fn render_leaf_value(&self, target: &WriteTarget, value: &[u8]) -> String {
        match self.leaf_meaning(target) {
            Some(StoredValueMeaning::Scalar(ScalarType::Bool)) => {
                if let Some(SavedValue::Bool(flag)) = decode_value(value, ScalarType::Bool) {
                    return flag.to_string();
                }
            }
            Some(StoredValueMeaning::Enum { enum_id, .. }) => {
                if let Some(rendered) = self.render_enum_leaf(*enum_id, value) {
                    return rendered;
                }
            }
            Some(StoredValueMeaning::Identity {
                store: store_id, ..
            }) => {
                if let Some(store) = self.identity_stores.get(store_id)
                    && let Some(rendered) = render_identity_leaf(store, value)
                {
                    return rendered;
                }
            }
            Some(StoredValueMeaning::Scalar(_)) | None => {}
        }
        crate::render_value_bytes(value)
    }

    fn render_enum_leaf(
        &self,
        expected_enum: marrow_check::EnumId,
        value: &[u8],
    ) -> Option<String> {
        let stored = decode_tree_enum_member(value).ok()?;
        let expected_catalog = self.enum_catalogs.get(&expected_enum)?;
        if expected_catalog != stored.enum_id().as_str() {
            return None;
        }
        self.enum_members
            .get(&(expected_enum, stored.member_id().as_str().to_string()))
            .cloned()
    }

    fn leaf_meaning(&self, target: &WriteTarget) -> Option<&StoredValueMeaning> {
        let member = self.leaf_member(target)?;
        self.member_meanings.get(member)
    }

    fn leaf_member<'a>(&self, target: &'a WriteTarget) -> Option<&'a str> {
        let WriteTarget::Data { path, .. } = target else {
            return None;
        };
        path.iter().rev().find_map(|segment| match segment {
            WriteDataSegment::Member(member) => Some(member.as_str()),
            WriteDataSegment::Key(_) => None,
        })
    }
}

fn render_identity_leaf(store: &StoreFact, value: &[u8]) -> Option<String> {
    let keys = decode_identity_payload_arity(value, store.identity_keys.len())?;
    if !store.identity_keys_match(&keys) {
        return Some(format!("0x{}", crate::hex_string(value)));
    }
    Some(format!("^{}({})", store.root, render_keys(&keys)))
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
            Some(info) => format!(
                "index:^{}.{name}({}) -> ({})",
                info.root,
                render_index_keys(keys, &info.key_meanings, names),
                render_keys(identity),
                name = info.name
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
                .map(|info| format!("^{}.{}", info.root, info.name))
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

fn render_index_keys(
    keys: &[SavedKey],
    meanings: &[StoredValueMeaning],
    names: &WriteTargetNames,
) -> String {
    keys.iter()
        .enumerate()
        .map(|(index, key)| match meanings.get(index) {
            Some(meaning) => render_index_key(key, meaning, names),
            None => render_key(key),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_index_key(
    key: &SavedKey,
    meaning: &StoredValueMeaning,
    names: &WriteTargetNames,
) -> String {
    match (meaning, key) {
        (StoredValueMeaning::Enum { enum_id, .. }, SavedKey::Str(member_catalog_id)) => names
            .enum_members
            .get(&(*enum_id, member_catalog_id.clone()))
            .cloned()
            .unwrap_or_else(|| render_key(key)),
        _ => render_key(key),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use marrow_check::{ModuleId, ResourceId, StoreIdentityKeyFact};
    use marrow_store::key::{SavedKey, encode_identity_payload};

    #[test]
    fn trace_leaf_renderer_rejects_type_wrong_identity_payloads() {
        let mut names = WriteTargetNames::default();
        names.identity_stores.insert(
            StoreId(0),
            StoreFact {
                id: StoreId(0),
                module: ModuleId(0),
                root: "authors".to_string(),
                resource: ResourceId(0),
                identity_keys: vec![StoreIdentityKeyFact {
                    name: "id".to_string(),
                    value_meaning: Some(StoredValueMeaning::Scalar(ScalarType::Int)),
                }],
                next_id_shape: "int".to_string(),
                catalog_id: Some("store-authors".to_string()),
                span: SourceSpan::default(),
            },
        );
        names.member_meanings.insert(
            "member-author".to_string(),
            StoredValueMeaning::Identity {
                store: StoreId(0),
                root: "authors".to_string(),
                store_catalog_id: Some("store-authors".to_string()),
                arity: 1,
                key_scalars: vec![ScalarType::Int],
            },
        );
        let target = WriteTarget::Data {
            store: "store-books".to_string(),
            identity: vec![SavedKey::Int(1)],
            path: vec![WriteDataSegment::Member("member-author".to_string())],
        };
        let bytes = encode_identity_payload(&[SavedKey::Str("not-an-int".to_string())]);

        let rendered = names.render_leaf_value(&target, &bytes);

        assert!(
            rendered.starts_with("0x"),
            "type-wrong identity payload must render as raw bytes: {rendered:?}"
        );
        assert!(
            !rendered.contains("^authors"),
            "type-wrong identity payload must not render as a rooted identity: {rendered:?}"
        );
    }
}
