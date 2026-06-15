use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use marrow_schema::{Node, NodeKind, ResourceSchema};

use crate::resolve::StoreResource;
use crate::{
    CHECK_REQUIRED_ABSENT, CheckDiagnostic, CheckedProgram, DiagnosticPayload, MarrowType,
    resolve_resource_type, resource_type_name,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalResourceState {
    Tracked {
        resource: String,
        missing: BTreeSet<String>,
    },
    Untracked,
}

#[derive(Debug)]
pub(crate) struct RequiredFieldAssignments {
    active: bool,
    frames: Vec<HashMap<String, LocalResourceState>>,
}

impl RequiredFieldAssignments {
    pub(crate) fn new() -> Self {
        Self {
            active: true,
            frames: Vec::new(),
        }
    }

    pub(crate) fn inactive() -> Self {
        Self {
            active: false,
            frames: Vec::new(),
        }
    }

    pub(crate) fn push_frame(&mut self) {
        if self.active {
            self.frames.push(HashMap::new());
        }
    }

    pub(crate) fn pop_frame(&mut self) {
        if self.active {
            self.frames.pop();
        }
    }

    pub(crate) fn bind_statement(
        &mut self,
        program: &CheckedProgram,
        statement: &marrow_syntax::Statement,
        name: &str,
        ty: &MarrowType,
    ) {
        if !self.active {
            return;
        }
        let state = match statement {
            marrow_syntax::Statement::Var {
                keys, value: None, ..
            } if keys.is_empty() => tracked_resource(program, ty),
            _ => None,
        }
        .unwrap_or(LocalResourceState::Untracked);
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.to_string(), state);
        }
    }

    pub(crate) fn assign_target(&mut self, target: &marrow_syntax::Expression) {
        if !self.active {
            return;
        }
        if let Some(name) = bare_name(target) {
            self.mark_untracked(name);
            return;
        }
        let Some((local, field)) = direct_local_field(target) else {
            return;
        };
        if let Some(LocalResourceState::Tracked { missing, .. }) = self.lookup_mut(local) {
            mark_assigned_field(missing, field);
        }
    }

    pub(crate) fn invalidate_all(&mut self) {
        if !self.active {
            return;
        }
        for frame in &mut self.frames {
            frame.clear();
        }
    }

    pub(crate) fn check_whole_root_write(
        &self,
        file: &Path,
        value: &marrow_syntax::Expression,
        store: StoreResource<'_>,
        diagnostics: &mut Vec<CheckDiagnostic>,
    ) {
        if !self.active {
            return;
        }
        let Some(local) = bare_name(value) else {
            return;
        };
        let Some(LocalResourceState::Tracked { resource, missing }) = self.lookup(local) else {
            return;
        };
        if missing.is_empty() {
            return;
        }
        let target_resource = resource_type_name(&store.module.name, &store.resource.name);
        if resource != &target_resource {
            return;
        }
        diagnostics.push(
            CheckDiagnostic::error(
                CHECK_REQUIRED_ABSENT,
                file,
                value.span(),
                format!(
                    "local resource `{local}` is missing required {} when written to `^{}`",
                    field_list(missing),
                    store.store.root
                ),
            )
            .with_payload(DiagnosticPayload::RequiredAbsent {
                local: local.to_string(),
                resource: resource.clone(),
                store_root: store.store.root.clone(),
                missing_field_paths: missing.iter().cloned().collect(),
            }),
        );
    }

    fn lookup(&self, name: &str) -> Option<&LocalResourceState> {
        self.frames.iter().rev().find_map(|frame| frame.get(name))
    }

    fn lookup_mut(&mut self, name: &str) -> Option<&mut LocalResourceState> {
        self.frames
            .iter_mut()
            .rev()
            .find_map(|frame| frame.get_mut(name))
    }

    fn mark_untracked(&mut self, name: &str) {
        if let Some(state) = self.lookup_mut(name) {
            *state = LocalResourceState::Untracked;
        }
    }
}

fn mark_assigned_field(missing: &mut BTreeSet<String>, field: &str) {
    missing.remove(field);
    let prefix = format!("{field}.");
    missing.retain(|path| !path.starts_with(&prefix));
}

fn tracked_resource(program: &CheckedProgram, ty: &MarrowType) -> Option<LocalResourceState> {
    let MarrowType::Resource(resource) = ty else {
        return None;
    };
    let (schema, _) = resolve_resource_type(program, resource)?;
    let missing = required_plain_field_paths(schema);
    if missing.is_empty() {
        return None;
    }
    Some(LocalResourceState::Tracked {
        resource: resource.clone(),
        missing,
    })
}

fn required_plain_field_paths(resource: &ResourceSchema) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    collect_required_plain_field_paths(&resource.members, &mut Vec::new(), &mut fields);
    fields
}

fn collect_required_plain_field_paths(
    nodes: &[Node],
    prefix: &mut Vec<String>,
    fields: &mut BTreeSet<String>,
) {
    for node in nodes {
        if !node.key_params.is_empty() {
            continue;
        }
        match &node.kind {
            NodeKind::Slot { required: true, .. } => {
                let mut path = prefix.clone();
                path.push(node.name.clone());
                fields.insert(path.join("."));
            }
            NodeKind::Group => {
                prefix.push(node.name.clone());
                collect_required_plain_field_paths(&node.members, prefix, fields);
                prefix.pop();
            }
            NodeKind::Slot {
                required: false, ..
            } => {}
        }
    }
}

fn bare_name(expr: &marrow_syntax::Expression) -> Option<&str> {
    let marrow_syntax::Expression::Name { segments, .. } = expr else {
        return None;
    };
    let [name] = segments.as_slice() else {
        return None;
    };
    Some(name)
}

fn direct_local_field(expr: &marrow_syntax::Expression) -> Option<(&str, &str)> {
    let marrow_syntax::Expression::Field { base, name, .. } = expr else {
        return None;
    };
    let local = bare_name(base)?;
    Some((local, name))
}

fn field_list(fields: &BTreeSet<String>) -> String {
    let names: Vec<String> = fields.iter().map(|field| format!("`{field}`")).collect();
    match names.as_slice() {
        [one] => format!("field {one}"),
        _ => format!("fields {}", names.join(", ")),
    }
}
