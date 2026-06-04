//! `marrow debug explain`: render checked target facts for diagnostics/admin use.

use std::process::ExitCode;

use marrow_check::{
    CheckedProgram, StorePathClass,
    tooling::{
        IndexExplanation, NameExplanation, NameResolutionExplanation, SavedPathExplanation,
        explain_name as explain_name_facts, explain_saved_path as explain_saved_path_facts,
    },
};
use serde_json::json;

use crate::{CheckFormat, load_checked_project, write_json};

pub(crate) fn debug(args: &[String]) -> ExitCode {
    let Some((subcommand, rest)) = args.split_first() else {
        eprintln!("missing debug subcommand; expected `explain`");
        eprintln!("run `marrow debug --help` for usage");
        return ExitCode::from(2);
    };
    match subcommand.as_str() {
        "--help" | "-h" => {
            print!(
                "\
Usage:
  marrow debug explain [--format text|json|jsonl] <projectdir> <target>
"
            );
            ExitCode::SUCCESS
        }
        "explain" => explain(rest),
        other => {
            eprintln!("unknown debug subcommand: {other}");
            eprintln!("expected `explain`");
            ExitCode::from(2)
        }
    }
}

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
                    "Usage:\n  marrow debug explain [--format text|json|jsonl] <projectdir> <target>\n"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with("--") => {
                eprintln!("unknown debug explain option: {value}");
                return Err(ExitCode::from(2));
            }
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    match positionals.as_slice() {
        [dir, target] => Ok((dir.clone(), target.clone(), format)),
        [] | [_] => {
            eprintln!("marrow debug explain requires a project directory and a target");
            Err(ExitCode::from(2))
        }
        _ => {
            eprintln!("marrow debug explain accepts one project directory and one target");
            Err(ExitCode::from(2))
        }
    }
}

fn explain(args: &[String]) -> ExitCode {
    let (dir, target, format) = match explain_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (_, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    if target.starts_with('^') {
        explain_saved_path(&program, &target, format)
    } else {
        explain_name(&program, &target, format);
        ExitCode::SUCCESS
    }
}

fn explain_saved_path(program: &CheckedProgram, target: &str, format: CheckFormat) -> ExitCode {
    let facts = match explain_saved_path_facts(program, target) {
        Ok(facts) => facts,
        Err(error) => {
            eprintln!("marrow debug explain: {}", error.message);
            return ExitCode::from(2);
        }
    };

    match format {
        CheckFormat::Text => {
            print!("{}", facts.target);
            match &facts.class {
                StorePathClass::Scalar(ty) => {
                    print!(" resolves to");
                    if let Some(resource) = &facts.resource {
                        print!(
                            " {} of resource {}",
                            member_phrase(facts.field.as_deref()),
                            resource
                        );
                    }
                    println!(", type {}", ty.name());
                    if facts.indexes.is_empty() {
                        println!("indexes: no declared index covers this field");
                    } else {
                        println!("indexes: {}", index_phrase(&facts.indexes));
                    }
                }
                StorePathClass::Identity {
                    store_root: referenced,
                    ..
                } => {
                    print!(" resolves to");
                    if let Some(resource) = &facts.resource {
                        print!(
                            " {} of resource {}",
                            member_phrase(facts.field.as_deref()),
                            resource
                        );
                    }
                    println!(", type Id(^{referenced})");
                }
                StorePathClass::IndexMarker => {
                    println!(" is a generated index entry");
                }
                StorePathClass::KeyTypeMismatch { expected, found } => {
                    println!(
                        " has a {} key where the schema declares {}",
                        found.name(),
                        expected.name()
                    );
                }
                StorePathClass::Orphan => {
                    println!(" is an orphan: under no declared root, or an undeclared member");
                }
            }
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(saved_path_record(&facts));
        }
    }
    ExitCode::SUCCESS
}

fn member_phrase(field: Option<&str>) -> String {
    match field {
        Some(name) => format!("field `{name}`"),
        None => "member".into(),
    }
}

fn index_phrase(indexes: &[IndexExplanation]) -> String {
    let rendered: Vec<String> = indexes
        .iter()
        .map(|index| {
            let unique = if index.unique { " unique" } else { "" };
            format!("`{}`({}){unique}", index.name, index.args.join(", "))
        })
        .collect();
    format!("covered by {}", rendered.join(", "))
}

fn saved_path_record(facts: &SavedPathExplanation) -> serde_json::Value {
    let (class_name, ty) = match &facts.class {
        StorePathClass::Scalar(ty) => ("scalar", Some(ty.name().to_string())),
        StorePathClass::Identity { store_root, .. } => {
            ("identity", Some(format!("Id(^{store_root})")))
        }
        StorePathClass::IndexMarker => ("index_marker", None),
        StorePathClass::KeyTypeMismatch { .. } => ("key_type_mismatch", None),
        StorePathClass::Orphan => ("orphan", None),
    };
    let index_records: Vec<serde_json::Value> = facts
        .indexes
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
        "target": facts.target,
        "kind": "saved_path",
        "class": class_name,
        "type": ty,
        "root": facts.root,
        "resource": facts.resource,
        "field": facts.field,
        "indexes": index_records,
    })
}

fn explain_name(program: &CheckedProgram, target: &str, format: CheckFormat) {
    let facts = explain_name_facts(program, target);
    match format {
        CheckFormat::Text => print!("{}", name_text(&facts)),
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(name_record(&facts));
        }
    }
}

fn name_text(facts: &NameExplanation) -> String {
    match &facts.resolution {
        NameResolutionExplanation::Found { module, kind } => format!(
            "{} resolves to {} `{}` in module {}\n",
            facts.target, kind, facts.target, module
        ),
        NameResolutionExplanation::Ambiguous { candidates } => format!(
            "{} is ambiguous: defined in {}\n",
            facts.target,
            candidates.join(", ")
        ),
        NameResolutionExplanation::NotVisible { name } => {
            format!(
                "{} resolves to `{name}`, which is not visible (not `pub`)\n",
                facts.target
            )
        }
        NameResolutionExplanation::Unresolved => {
            format!("{} resolves to no declaration\n", facts.target)
        }
    }
}

fn name_record(facts: &NameExplanation) -> serde_json::Value {
    match &facts.resolution {
        NameResolutionExplanation::Found { module, kind } => json!({
            "target": facts.target,
            "kind": "name",
            "resolution": "found",
            "module": module,
            "resolved_kind": kind,
        }),
        NameResolutionExplanation::Ambiguous { candidates } => json!({
            "target": facts.target,
            "kind": "name",
            "resolution": "ambiguous",
            "candidates": candidates,
        }),
        NameResolutionExplanation::NotVisible { name } => json!({
            "target": facts.target,
            "kind": "name",
            "resolution": "not_visible",
            "name": name,
        }),
        NameResolutionExplanation::Unresolved => json!({
            "target": facts.target,
            "kind": "name",
            "resolution": "unresolved",
        }),
    }
}
