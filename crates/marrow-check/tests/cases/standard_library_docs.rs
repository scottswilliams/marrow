use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use marrow_schema::stdlib::{self, ParamType, ReturnType};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    params: Vec<String>,
    result: Option<String>,
}

fn reference_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository root")
        .join("docs/language/standard-library.md")
}

fn scalar_name(scalar: marrow_schema::ScalarType) -> String {
    scalar.name().to_string()
}

fn parameter_name(param: &ParamType) -> String {
    match param {
        ParamType::Scalar(scalar) => scalar_name(*scalar),
        ParamType::ScalarAny => "T".to_string(),
        ParamType::Sequence(scalar) => format!("sequence[{}]", scalar.name()),
        ParamType::Error => "Error".to_string(),
        ParamType::Path => "T?".to_string(),
    }
}

fn result_name(result: &ReturnType) -> Option<String> {
    match result {
        ReturnType::Scalar(scalar) => Some(scalar_name(*scalar)),
        ReturnType::OptionalScalar(scalar) => Some(format!("{}?", scalar.name())),
        ReturnType::Sequence(scalar) => Some(format!("sequence[{}]", scalar.name())),
        ReturnType::Void => None,
    }
}

fn expected_signatures() -> BTreeMap<String, Signature> {
    stdlib::all()
        .iter()
        .map(|op| {
            (
                format!("std::{}::{}", op.module, op.op),
                Signature {
                    params: op.params.iter().map(parameter_name).collect(),
                    result: result_name(&op.ret),
                },
            )
        })
        .collect()
}

fn documented_signatures(markdown: &str) -> BTreeMap<String, Signature> {
    let mut signatures = BTreeMap::new();
    for line in markdown.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("std::") else {
            continue;
        };
        let Some((callee, after_open)) = rest.split_once('(') else {
            continue;
        };
        let (params, suffix) = after_open
            .split_once(')')
            .expect("standard-library signature closes its parameter list");
        let params = if params.is_empty() {
            Vec::new()
        } else {
            params
                .split(',')
                .map(|param| {
                    param
                        .trim()
                        .split_once(": ")
                        .expect("documented parameter has a name and type")
                        .1
                        .to_string()
                })
                .collect()
        };
        let result = suffix.trim().strip_prefix(": ").map(str::to_string);
        let path = format!("std::{callee}");
        assert!(
            signatures
                .insert(path.clone(), Signature { params, result })
                .is_none(),
            "duplicate standard-library signature for {path}"
        );
    }
    signatures
}

fn backtick_tokens(text: &str) -> Vec<String> {
    text.split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, part)| part.to_string())
        .collect()
}

fn documented_capabilities(markdown: &str) -> BTreeMap<String, BTreeSet<String>> {
    let section = markdown
        .split_once("## Host Capabilities")
        .expect("host-capability section")
        .1
        .split_once("\n## ")
        .expect("next reference section")
        .0;
    let mut bullets = Vec::<String>::new();
    for line in section.lines() {
        if line.starts_with("- `") {
            bullets.push(line.trim().to_string());
        } else if line.starts_with("  ")
            && let Some(bullet) = bullets.last_mut()
        {
            bullet.push(' ');
            bullet.push_str(line.trim());
        }
    }
    bullets
        .into_iter()
        .map(|bullet| {
            let tokens = backtick_tokens(&bullet);
            let (capability, paths) = tokens
                .split_first()
                .expect("capability bullet has a capability name");
            (
                capability.clone(),
                paths.iter().cloned().collect::<BTreeSet<_>>(),
            )
        })
        .collect()
}

fn expected_capabilities() -> BTreeMap<String, BTreeSet<String>> {
    let mut capabilities = BTreeMap::<String, BTreeSet<String>>::new();
    for op in stdlib::all() {
        let Some(capability) = op.requires_capability else {
            continue;
        };
        capabilities
            .entry(format!("{capability:?}").to_lowercase())
            .or_default()
            .insert(format!("std::{}::{}", op.module, op.op));
    }
    capabilities
}

#[test]
fn standard_library_reference_matches_the_descriptor_table() {
    let markdown = fs::read_to_string(reference_path()).expect("read standard-library reference");
    assert_eq!(
        documented_signatures(&markdown),
        expected_signatures(),
        "standard-library signatures drifted from marrow-schema descriptors"
    );
    assert_eq!(
        documented_capabilities(&markdown),
        expected_capabilities(),
        "standard-library host capabilities drifted from marrow-schema descriptors"
    );
}
