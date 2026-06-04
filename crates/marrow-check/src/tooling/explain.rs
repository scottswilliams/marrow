use crate::resolve::resolve_store_by_root;
use crate::{
    CheckedProgram, DefItem, Resolution, ResolvableKind, StorePathClass, classify_store_path,
    display_path, parse_path, resolve,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPathExplanation {
    pub target: String,
    pub class: StorePathClass,
    pub root: Option<String>,
    pub resource: Option<String>,
    pub field: Option<String>,
    pub indexes: Vec<IndexExplanation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexExplanation {
    pub name: String,
    pub args: Vec<String>,
    pub unique: bool,
}

pub fn explain_saved_path(
    program: &CheckedProgram,
    target: &str,
) -> Result<SavedPathExplanation, crate::PathParseError> {
    let segments = parse_path(target)?;
    let class = classify_store_path(program, &segments);
    let root = root_of(&segments).map(str::to_string);
    let store_resource = root
        .as_deref()
        .and_then(|root| resolve_store_by_root(program, root));
    let resource = store_resource.map(|store| store.resource.name.clone());
    let field = terminal_field(&segments).map(str::to_string);
    let indexes = match (store_resource, field.as_deref()) {
        (Some(store), Some(field)) => store
            .store
            .indexes
            .iter()
            .filter(|index| index.args.iter().any(|arg| arg == field))
            .map(|index| IndexExplanation {
                name: index.name.clone(),
                args: index.args.clone(),
                unique: index.unique,
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(SavedPathExplanation {
        target: display_path(&segments),
        class,
        root,
        resource,
        field,
        indexes,
    })
}

fn root_of(segments: &[crate::PathSegment]) -> Option<&str> {
    match segments.first() {
        Some(crate::PathSegment::Root(name)) => Some(name.as_str()),
        _ => None,
    }
}

fn terminal_field(segments: &[crate::PathSegment]) -> Option<&str> {
    segments.iter().rev().find_map(|segment| match segment {
        crate::PathSegment::Field(name) => Some(name.as_str()),
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameExplanation {
    pub target: String,
    pub resolution: NameResolutionExplanation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameResolutionExplanation {
    Found { module: String, kind: &'static str },
    Ambiguous { candidates: Vec<String> },
    NotVisible { name: String },
    Unresolved,
}

pub fn explain_name(program: &CheckedProgram, target: &str) -> NameExplanation {
    let segments: Vec<String> = target.split("::").map(str::to_string).collect();
    let resolution = match resolve(program, "", &segments, ResolvableKind::Function) {
        Resolution::Unresolved => resolve(program, "", &segments, ResolvableKind::Resource),
        resolution => resolution,
    };
    let resolution = match resolution {
        Resolution::Found(def) => NameResolutionExplanation::Found {
            module: def.module.name.clone(),
            kind: match def.item {
                DefItem::Function(_) => "function",
                DefItem::Resource(_) => "resource",
            },
        },
        Resolution::Ambiguous(candidates) => NameResolutionExplanation::Ambiguous {
            candidates: candidates.clone(),
        },
        Resolution::NotVisible(name) => NameResolutionExplanation::NotVisible {
            name: name.to_string(),
        },
        Resolution::Unresolved => NameResolutionExplanation::Unresolved,
    };
    NameExplanation {
        target: target.to_string(),
        resolution,
    }
}
