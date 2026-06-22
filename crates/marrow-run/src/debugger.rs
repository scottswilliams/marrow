use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::value::{Value, saved_key_preview_with_text_limit, truncate_preview_chars};

pub const DEBUG_VALUE_DEFAULT_PAGE_LIMIT: usize = 100;
pub const DEBUG_VALUE_MAX_PAGE_LIMIT: usize = 1000;

const VALUE_PREVIEW_CHARS: usize = 256;
const RESOURCE_FIELD_PREVIEW_COUNT: usize = 12;
const IDENTITY_KEY_PREVIEW_COUNT: usize = 8;
const DEBUG_VALUE_SNAPSHOT_NODE_LIMIT: usize = 4096;
const DEBUG_VALUE_SNAPSHOT_DEPTH_LIMIT: usize = 8;

/// A bounded child page for runtime value inspection. `start` is a zero-based
/// offset into the child list; child labels still use Marrow's one-based
/// sequence and identity positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugValuePage {
    start: usize,
    limit: usize,
}

impl DebugValuePage {
    pub fn new(start: usize, limit: usize) -> Self {
        Self {
            start,
            limit: limit.min(DEBUG_VALUE_MAX_PAGE_LIMIT),
        }
    }

    pub fn default_from(start: usize) -> Self {
        Self::new(start, DEBUG_VALUE_DEFAULT_PAGE_LIMIT)
    }

    pub fn start(self) -> usize {
        self.start
    }

    pub fn limit(self) -> usize {
        self.limit
    }
}

impl Default for DebugValuePage {
    fn default() -> Self {
        Self::default_from(0)
    }
}

/// Which class of children a debugger client wants from a runtime value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugValueFilter {
    All,
    Named,
    Indexed,
}

impl DebugValueFilter {
    fn allows_named(self) -> bool {
        matches!(self, Self::All | Self::Named)
    }

    fn allows_indexed(self) -> bool {
        matches!(self, Self::All | Self::Indexed)
    }
}

/// Child counts for debugger clients that split named and indexed children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugValueChildCounts {
    pub named: Option<usize>,
    pub indexed: Option<usize>,
}

/// A bounded snapshot of a runtime value prepared for debugger inspection.
#[derive(Clone, PartialEq, Eq)]
pub struct DebugValue {
    preview: String,
    child_counts: Option<DebugValueChildCounts>,
    children_truncated: bool,
    children: Vec<DebugCapturedChild>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DebugCapturedChild {
    kind: DebugCapturedChildKind,
    name: String,
    value: DebugValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DebugCapturedChildKind {
    Named,
    Indexed,
}

impl DebugCapturedChildKind {
    fn allowed_by(self, filter: DebugValueFilter) -> bool {
        match self {
            Self::Named => filter.allows_named(),
            Self::Indexed => filter.allows_indexed(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DebugValueBudget {
    remaining: usize,
}

impl Default for DebugValueBudget {
    fn default() -> Self {
        Self {
            remaining: DEBUG_VALUE_SNAPSHOT_NODE_LIMIT,
        }
    }
}

impl DebugValueBudget {
    fn reserve_child(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }
}

enum DebugValueSource<'a> {
    Runtime {
        name: DebugChildName<'a>,
        value: &'a Value,
    },
    SavedKey {
        name: DebugChildName<'a>,
        key: &'a SavedKey,
    },
}

impl DebugValueSource<'_> {
    fn name(&self) -> String {
        match self {
            Self::Runtime { name, .. } | Self::SavedKey { name, .. } => name.render(),
        }
    }
}

enum DebugChildName<'a> {
    Field(&'a str),
    Index(usize),
    KeyTuple(&'a [SavedKey]),
}

impl DebugChildName<'_> {
    fn render(&self) -> String {
        match self {
            Self::Field(name) => truncate_preview_chars(name, VALUE_PREVIEW_CHARS),
            Self::Index(index) => index_label(*index),
            Self::KeyTuple(keys) => key_tuple_label(keys),
        }
    }
}

impl DebugValue {
    /// Captures a bounded snapshot of a runtime value for preview and child expansion.
    pub fn from_value(value: Value) -> Self {
        let mut budget = DebugValueBudget::default();
        Self::from_runtime_value(&value, 0, &mut budget)
    }

    fn from_runtime_value(value: &Value, depth: usize, budget: &mut DebugValueBudget) -> Self {
        match value {
            Value::Sequence(items) => Self::from_runtime_children(
                runtime_value_preview(value),
                DebugCapturedChildKind::Indexed,
                items.len(),
                // A sequence holds its populated 1-based positions; the preview labels
                // each child by its stored position so a hole is visible as a gap.
                items
                    .rows()
                    .map(|(position, item)| (DebugChildName::Index((position - 1) as usize), item)),
                depth,
                budget,
            ),
            Value::LocalTree(tree) => Self::from_runtime_children(
                runtime_value_preview(value),
                DebugCapturedChildKind::Named,
                tree.len(),
                tree.rows()
                    .map(|(keys, value)| (DebugChildName::KeyTuple(keys), value)),
                depth,
                budget,
            ),
            Value::Resource(fields) => Self::from_runtime_children(
                runtime_value_preview(value),
                DebugCapturedChildKind::Named,
                fields.len(),
                fields
                    .iter()
                    .map(|(name, value)| (DebugChildName::Field(name), value)),
                depth,
                budget,
            ),
            Value::Identity(identity) => Self::from_key_children(
                runtime_value_preview(value),
                identity.keys().len(),
                identity
                    .keys()
                    .iter()
                    .enumerate()
                    .map(|(index, key)| (DebugChildName::Index(index), key)),
                depth,
                budget,
            ),
            Value::Int(_)
            | Value::Bool(_)
            | Value::Str(_)
            | Value::Instant(_)
            | Value::Date(_)
            | Value::Duration(_)
            | Value::Decimal(_)
            | Value::Bytes(_)
            | Value::Enum(_) => Self::leaf(runtime_value_preview(value)),
        }
    }

    fn from_saved_key(key: &SavedKey) -> Self {
        Self::leaf(saved_key_preview(key))
    }

    fn leaf(preview: String) -> Self {
        Self {
            preview,
            child_counts: None,
            children_truncated: false,
            children: Vec::new(),
        }
    }

    /// A total one-line preview for display. It is not intended to be parsed.
    pub fn preview(&self) -> String {
        self.preview.clone()
    }

    /// Captured child counts for expandable values, split by named and indexed children.
    pub fn child_counts(&self) -> Option<DebugValueChildCounts> {
        self.child_counts
    }

    /// Whether this snapshot omitted children because it reached debugger bounds.
    pub fn children_truncated(&self) -> bool {
        self.children_truncated
    }

    /// Returns one bounded page of captured child values.
    pub fn children(&self, page: DebugValuePage, filter: DebugValueFilter) -> Vec<DebugValueChild> {
        let children = self
            .children
            .iter()
            .filter(|child| child.kind.allowed_by(filter));
        collect_page(children, page, |child| DebugValueChild {
            name: child.name.clone(),
            value: child.value.clone(),
        })
    }

    fn from_runtime_children<'a>(
        preview: String,
        kind: DebugCapturedChildKind,
        total_children: usize,
        children: impl Iterator<Item = (DebugChildName<'a>, &'a Value)>,
        depth: usize,
        budget: &mut DebugValueBudget,
    ) -> Self {
        Self::with_children(
            preview,
            kind,
            total_children,
            children.map(|(name, value)| DebugValueSource::Runtime { name, value }),
            depth,
            budget,
        )
    }

    fn from_key_children<'a>(
        preview: String,
        total_children: usize,
        children: impl Iterator<Item = (DebugChildName<'a>, &'a SavedKey)>,
        depth: usize,
        budget: &mut DebugValueBudget,
    ) -> Self {
        Self::with_children(
            preview,
            DebugCapturedChildKind::Indexed,
            total_children,
            children.map(|(name, key)| DebugValueSource::SavedKey { name, key }),
            depth,
            budget,
        )
    }

    fn with_children<'a>(
        preview: String,
        kind: DebugCapturedChildKind,
        total_children: usize,
        children: impl Iterator<Item = DebugValueSource<'a>>,
        depth: usize,
        budget: &mut DebugValueBudget,
    ) -> Self {
        let mut captured = Vec::new();
        if depth < DEBUG_VALUE_SNAPSHOT_DEPTH_LIMIT {
            for source in children {
                if captured.len() == DEBUG_VALUE_MAX_PAGE_LIMIT || !budget.reserve_child() {
                    break;
                }
                let name = source.name();
                let value = match source {
                    DebugValueSource::Runtime { value, .. } => {
                        Self::from_runtime_value(value, depth + 1, budget)
                    }
                    DebugValueSource::SavedKey { key, .. } => Self::from_saved_key(key),
                };
                captured.push(DebugCapturedChild { kind, name, value });
            }
        }

        let child_count = captured.len();
        let child_counts = match kind {
            DebugCapturedChildKind::Named => DebugValueChildCounts {
                named: Some(child_count),
                indexed: None,
            },
            DebugCapturedChildKind::Indexed => DebugValueChildCounts {
                named: None,
                indexed: Some(child_count),
            },
        };

        Self {
            preview,
            child_counts: Some(child_counts),
            children_truncated: total_children > child_count,
            children: captured,
        }
    }
}

impl From<Value> for DebugValue {
    fn from(value: Value) -> Self {
        Self::from_value(value)
    }
}

impl fmt::Debug for DebugValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebugValue")
            .field("preview", &self.preview)
            .field("child_counts", &self.child_counts())
            .field("children_truncated", &self.children_truncated)
            .finish()
    }
}

/// One child of an expandable debugger value. Sequence and identity child names
/// are rendered as one-based Marrow positions even though pages use zero-based
/// offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugValueChild {
    pub name: String,
    pub value: DebugValue,
}

/// One visible local binding at a stopped frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugLocal {
    pub name: String,
    pub value: DebugValue,
}

/// The owned debugger view of a stopped runtime frame. Locals are listed by
/// first visible binding order, with shadowed names resolved to the innermost
/// value. `locals` is the captured page; `visible_local_count` is the total
/// visible names before paging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugFrameSnapshot {
    pub span: SourceSpan,
    pub file: Option<PathBuf>,
    pub depth: usize,
    pub visible_local_count: usize,
    pub locals_truncated: bool,
    pub locals: Vec<DebugLocal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DebugLocalsSnapshot {
    pub visible_local_count: usize,
    pub locals_truncated: bool,
    pub locals: Vec<DebugLocal>,
}

pub(crate) fn snapshot_locals<'a>(
    locals: impl Iterator<Item = (&'a str, &'a Value)>,
    page: DebugValuePage,
    filter: DebugValueFilter,
) -> DebugLocalsSnapshot {
    let mut order = Vec::new();
    let mut latest = HashMap::new();
    for (name, value) in locals {
        if !latest.contains_key(name) {
            order.push(name);
        }
        latest.insert(name, value);
    }

    let visible_local_count = order.len();
    let locals = if filter.allows_named() {
        let locals = order
            .into_iter()
            .filter_map(|name| latest.get(name).map(|value| (name, *value)));
        let mut budget = DebugValueBudget::default();
        collect_page(locals, page, |(name, value)| DebugLocal {
            name: name.to_string(),
            value: DebugValue::from_runtime_value(value, 0, &mut budget),
        })
    } else {
        Vec::new()
    };
    DebugLocalsSnapshot {
        locals_truncated: locals.len() < visible_local_count,
        visible_local_count,
        locals,
    }
}

fn collect_page<T, R>(
    iter: impl Iterator<Item = T>,
    page: DebugValuePage,
    mut render: impl FnMut(T) -> R,
) -> Vec<R> {
    iter.skip(page.start())
        .take(page.limit())
        .map(&mut render)
        .collect()
}

fn runtime_value_preview(value: &Value) -> String {
    match value {
        Value::Int(_)
        | Value::Bool(_)
        | Value::Instant(_)
        | Value::Date(_)
        | Value::Duration(_)
        | Value::Decimal(_)
        | Value::Bytes(_)
        | Value::Enum(_)
        | Value::Sequence(_)
        | Value::LocalTree(_) => {
            truncate_preview_chars(&value.display_debug(), VALUE_PREVIEW_CHARS)
        }
        Value::Str(text) => truncate_preview_chars(text, VALUE_PREVIEW_CHARS),
        Value::Resource(fields) => resource_preview(fields),
        Value::Identity(identity) => identity_preview(identity),
    }
}

fn resource_preview(fields: &[(String, Value)]) -> String {
    let mut names: Vec<String> = fields
        .iter()
        .take(RESOURCE_FIELD_PREVIEW_COUNT)
        .map(|(name, _)| truncate_preview_chars(name, VALUE_PREVIEW_CHARS))
        .collect();
    if fields.len() > RESOURCE_FIELD_PREVIEW_COUNT {
        names.push(format!(
            "... {} more",
            fields.len() - RESOURCE_FIELD_PREVIEW_COUNT
        ));
    }
    format!("resource{{{}}}", names.join(", "))
}

fn identity_preview(identity: &crate::value::IdentityValue) -> String {
    let root = truncate_preview_chars(identity.root(), VALUE_PREVIEW_CHARS);
    let mut keys: Vec<String> = identity
        .keys()
        .iter()
        .take(IDENTITY_KEY_PREVIEW_COUNT)
        .map(saved_key_preview)
        .collect();
    if identity.keys().len() > IDENTITY_KEY_PREVIEW_COUNT {
        keys.push(format!(
            "... {} more",
            identity.keys().len() - IDENTITY_KEY_PREVIEW_COUNT
        ));
    }
    format!("^{root}({})", keys.join(", "))
}

fn key_tuple_label(keys: &[SavedKey]) -> String {
    let mut rendered: Vec<String> = keys
        .iter()
        .take(IDENTITY_KEY_PREVIEW_COUNT)
        .map(saved_key_preview)
        .collect();
    if keys.len() > IDENTITY_KEY_PREVIEW_COUNT {
        rendered.push(format!(
            "... {} more",
            keys.len() - IDENTITY_KEY_PREVIEW_COUNT
        ));
    }
    format!("({})", rendered.join(", "))
}

fn index_label(index: usize) -> String {
    format!("[{}]", index + 1)
}

fn saved_key_preview(key: &SavedKey) -> String {
    saved_key_preview_with_text_limit(key, VALUE_PREVIEW_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::LocalTree;

    #[test]
    fn local_tree_child_labels_bound_key_tuple_arity() {
        let keys: Vec<SavedKey> = (0..IDENTITY_KEY_PREVIEW_COUNT + 1)
            .map(|value| SavedKey::Int(value as i64))
            .collect();
        let mut local_tree = LocalTree::default();
        local_tree.insert(keys, Value::Int(42));
        let tree = DebugValue::from_value(Value::LocalTree(local_tree));

        let children = tree.children(DebugValuePage::default(), DebugValueFilter::Named);

        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "(0, 1, 2, 3, 4, 5, 6, 7, ... 1 more)");
        assert_eq!(children[0].value.preview(), "42");
    }
}
