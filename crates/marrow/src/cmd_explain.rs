//! `marrow explain`: statically explain a target — a saved `^path` or a name —
//! with no run. It surfaces the same resolution the checker and runtime use, so
//! it can never disagree with them.

use std::process::ExitCode;

use marrow_check::resolve::{resolve_resource_by_name_any, resolve_resource_by_root};
use marrow_check::{
    CheckedProgram, Def, DefItem, IndexSchema, Resolution, ResolvableKind, ResourceSchema, resolve,
};
use marrow_run::{SavedPathClass, classify_saved_path};
use marrow_store::path::{PathSegment, display_path, encode_path, parse_path};
use serde_json::json;

use crate::{CheckFormat, load_checked_project, write_json};

/// Parse `explain`'s arguments: a project directory, a target (a `^path` or a
/// name), and an optional `--format`. Rejects options and a wrong positional
/// count, matching the `data get` grammar.
fn explain_args(args: &[String]) -> Result<(String, String, CheckFormat), ExitCode> {
    let mut positionals = Vec::new();
    let mut format = CheckFormat::Text;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --format");
                    return Err(ExitCode::from(2));
                };
                format = CheckFormat::parse(value).ok_or_else(|| {
                    eprintln!("unknown format: {value}");
                    ExitCode::from(2)
                })?;
            }
            "--help" | "-h" => {
                print!(
                    "Usage:\n  marrow explain [--format text|json|jsonl] <projectdir> <target>\n"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with("--") => {
                eprintln!("unknown explain option: {value}");
                return Err(ExitCode::from(2));
            }
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    match positionals.as_slice() {
        [dir, target] => Ok((dir.clone(), target.clone(), format)),
        [] | [_] => {
            eprintln!("marrow explain requires a project directory and a target");
            Err(ExitCode::from(2))
        }
        _ => {
            eprintln!("marrow explain accepts one project directory and one target");
            Err(ExitCode::from(2))
        }
    }
}

/// `marrow explain <projectdir> <target>`: statically explain a target with no
/// run. A `^path` reports its path/index plan; a name reports its resolution.
pub(crate) fn explain(args: &[String]) -> ExitCode {
    let (dir, target, format) = match explain_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (_, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    // A leading `^` marks a saved path, exactly as the runtime distinguishes one;
    // anything else is a name to resolve.
    if target.starts_with('^') {
        explain_saved_path(&program, &target, format)
    } else {
        explain_name(&program, &target, format);
        ExitCode::SUCCESS
    }
}

/// Explain a saved `^path`: parse it, classify it against the schema, and report
/// the root/resource, the resolved class (scalar type, index marker, key-type
/// mismatch, or orphan), and the indexes a field path participates in. A
/// malformed path is a usage error, like `data get`.
fn explain_saved_path(program: &CheckedProgram, target: &str, format: CheckFormat) -> ExitCode {
    let segments = match parse_path(target) {
        Ok(segments) => segments,
        Err(error) => {
            eprintln!("marrow explain: {}", error.message);
            return ExitCode::from(2);
        }
    };
    let class = classify_saved_path(program, &segments);
    // The root is the first segment; a field path's terminal name is its last
    // named member. Together they pick the resource and the indexes that name
    // covers.
    let root = root_of(&segments);
    let resource = root.and_then(|root| resolve_resource_by_root(program, root));
    let field = terminal_field(&segments);
    let indexes = match (resource, field) {
        (Some(resource), Some(field)) => indexes_covering(resource, field),
        _ => Vec::new(),
    };
    let encoded = encode_path(&segments);

    match format {
        CheckFormat::Text => {
            print!("{}", display_path(&encoded));
            match &class {
                SavedPathClass::Scalar(ty) => {
                    print!(" resolves to");
                    if let Some(resource) = resource {
                        print!(" {} of resource {}", member_phrase(field), resource.name);
                    }
                    println!(", type {}", ty.name());
                    if indexes.is_empty() {
                        println!("index plan: no index covers this field");
                    } else {
                        println!("index plan: {}", index_phrase(&indexes));
                    }
                }
                SavedPathClass::IndexMarker => {
                    println!(" is a generated index entry");
                }
                SavedPathClass::KeyTypeMismatch { expected, found } => {
                    println!(
                        " has a {} key where the schema declares {}",
                        found.name(),
                        expected.name()
                    );
                }
                SavedPathClass::Orphan => {
                    println!(" is an orphan: under no declared root, or an undeclared member");
                }
            }
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(saved_path_record(
                &encoded, &class, root, resource, field, &indexes,
            ));
        }
    }
    ExitCode::SUCCESS
}

/// The saved root name (the path's first segment), or `None` for a path that does
/// not start with a root.
fn root_of(segments: &[PathSegment]) -> Option<&str> {
    match segments.first() {
        Some(PathSegment::Root(name)) => Some(name.as_str()),
        _ => None,
    }
}

/// The terminal named member of a saved path — its field/leaf/index name — or
/// `None` when the path ends at the root or a record key (a bare record path).
fn terminal_field(segments: &[PathSegment]) -> Option<&str> {
    segments.iter().rev().find_map(|segment| match segment {
        PathSegment::Field(name) => Some(name.as_str()),
        _ => None,
    })
}

/// The declared indexes whose key arguments include `field` — the indexes a write
/// to that field keeps coherent.
fn indexes_covering<'r>(resource: &'r ResourceSchema, field: &str) -> Vec<&'r IndexSchema> {
    resource
        .indexes
        .iter()
        .filter(|index| index.args.iter().any(|arg| arg == field))
        .collect()
}

/// A human phrase naming a field member, e.g. "field `title`". Falls back to a
/// bare "member" when the path carries no named terminal.
fn member_phrase(field: Option<&str>) -> String {
    match field {
        Some(name) => format!("field `{name}`"),
        None => "member".into(),
    }
}

/// A human phrase listing the indexes a field feeds, marking unique ones.
fn index_phrase(indexes: &[&IndexSchema]) -> String {
    let rendered: Vec<String> = indexes
        .iter()
        .map(|index| {
            let unique = if index.unique { " unique" } else { "" };
            format!("`{}`({}){unique}", index.name, index.args.join(", "))
        })
        .collect();
    format!("covered by {}", rendered.join(", "))
}

/// The JSON record for a saved-path explanation: its class, the root/resource it
/// names, the resolved type when scalar, and the indexes it participates in.
fn saved_path_record(
    encoded: &[u8],
    class: &SavedPathClass,
    root: Option<&str>,
    resource: Option<&ResourceSchema>,
    field: Option<&str>,
    indexes: &[&IndexSchema],
) -> serde_json::Value {
    let (class_name, ty) = match class {
        SavedPathClass::Scalar(ty) => ("scalar", Some(ty.name())),
        SavedPathClass::IndexMarker => ("index_marker", None),
        SavedPathClass::KeyTypeMismatch { .. } => ("key_type_mismatch", None),
        SavedPathClass::Orphan => ("orphan", None),
    };
    let index_records: Vec<serde_json::Value> = indexes
        .iter()
        .map(|index| {
            json!({
                "name": index.name,
                "args": index.args,
                "unique": index.unique,
            })
        })
        .collect();
    json!({
        "target": display_path(encoded),
        "kind": "saved_path",
        "class": class_name,
        "type": ty,
        "root": root,
        "resource": resource.map(|resource| resource.name.clone()),
        "field": field,
        "indexes": index_records,
    })
}

/// Explain a name: resolve it as each applicable kind through the one resolver the
/// checker and runtime use, and report `found`/`ambiguous`/`not_visible`/
/// `unresolved`. A trailing `::Id` (a `Resource::Id` or `module::Resource::Id`) is
/// a resource-identity request on the name minus that suffix; any other bare or
/// qualified name is tried as a function first, then a resource, so the first
/// concrete declaration wins.
fn explain_name(program: &CheckedProgram, target: &str, format: CheckFormat) {
    let segments: Vec<String> = target.split("::").map(str::to_string).collect();
    let resolution = if segments.last().map(String::as_str) == Some("Id") && segments.len() >= 2 {
        // The name names a resource identity. Strip the `Id` and resolve the
        // remaining `Resource` / `module::Resource` as a resource, reported as the
        // identity. The resolver's own two-segment `Resource::Id` special-case only
        // covers a bare resource, so the qualified form is resolved here instead.
        resolve_resource_name(program, &segments[..segments.len() - 1])
    } else {
        // A name is resolved project-wide (from no particular module) as a function,
        // then, if that fails, as a resource — the same fall-through the runtime's
        // call dispatch uses.
        match resolve(program, "", &segments, ResolvableKind::Function) {
            Resolution::Unresolved => resolve_resource_name(program, &segments),
            resolution => resolution,
        }
    };
    match format {
        CheckFormat::Text => print!("{}", name_text(target, &resolution)),
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(name_record(target, &resolution));
        }
    }
}

/// Resolve a `Resource` / `module::Resource` name to its resource declaration. The
/// shared resolver handles the qualified form and a bare resource in the empty
/// module; a bare resource living in some other module is not reachable by an
/// unqualified name there, so it falls back to the project-wide resource-name
/// lookup the checker uses, reported as the resource identity.
fn resolve_resource_name<'p>(program: &'p CheckedProgram, name: &[String]) -> Resolution<'p> {
    match resolve(program, "", name, ResolvableKind::ResourceIdentity) {
        Resolution::Unresolved if name.len() == 1 => {
            match resolve_resource_by_name_any(program, &name[0]) {
                Some(resource) => resource_identity(program, resource),
                None => Resolution::Unresolved,
            }
        }
        resolution => resolution,
    }
}

/// Wrap a project-wide resource in a `Found` identity resolution, attributing it to
/// the module that declares it (the resolver's `Def` carries the owning module).
fn resource_identity<'p>(
    program: &'p CheckedProgram,
    resource: &'p ResourceSchema,
) -> Resolution<'p> {
    let module = program
        .modules
        .iter()
        .find(|module| module.resources.iter().any(|r| r.name == resource.name))
        .expect("resource came from a program module");
    Resolution::Found(Def {
        module,
        kind: ResolvableKind::ResourceIdentity,
        item: DefItem::Resource(resource),
    })
}

/// The human render of a name resolution.
fn name_text(target: &str, resolution: &Resolution<'_>) -> String {
    match resolution {
        Resolution::Found(def) => format!(
            "{target} resolves to {} `{target}` in module {}\n",
            kind_word(def),
            def.module.name
        ),
        Resolution::Ambiguous(candidates) => format!(
            "{target} is ambiguous: defined in {}\n",
            candidates.join(", ")
        ),
        Resolution::NotVisible(name) => {
            format!("{target} resolves to `{name}`, which is not visible (not `pub`)\n")
        }
        Resolution::Unresolved => format!("{target} resolves to no declaration\n"),
    }
}

/// The JSON record for a name resolution.
fn name_record(target: &str, resolution: &Resolution<'_>) -> serde_json::Value {
    match resolution {
        Resolution::Found(def) => json!({
            "target": target,
            "kind": "name",
            "resolution": "found",
            "module": def.module.name,
            "resolved_kind": kind_word(def),
        }),
        Resolution::Ambiguous(candidates) => json!({
            "target": target,
            "kind": "name",
            "resolution": "ambiguous",
            "candidates": candidates,
        }),
        Resolution::NotVisible(name) => json!({
            "target": target,
            "kind": "name",
            "resolution": "not_visible",
            "name": name,
        }),
        Resolution::Unresolved => json!({
            "target": target,
            "kind": "name",
            "resolution": "unresolved",
        }),
    }
}

/// The kind word for a resolved declaration, matching the `ResolvableKind` it was
/// reached as: a function, or a resource (constructor or identity).
fn kind_word(def: &Def<'_>) -> &'static str {
    match def.item {
        DefItem::Function(_) => "function",
        DefItem::Resource(_) => "resource",
    }
}
