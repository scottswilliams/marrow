use crate::support::{marrow, temp_dir, write};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

const ENTRYPOINTS: &[&str] = &[
    "CONTRIBUTING.md",
    "README.md",
    "SECURITY.md",
    "docs/README.md",
    "docs/language/README.md",
    "docs/tools/README.md",
    "docs/operations/README.md",
    "docs/implementation/README.md",
    "docs/vision.md",
    "docs/status.md",
    "docs/future/README.md",
];

const REQUIRED_DOCUMENTATION: &[&str] = &[
    "docs/compatibility.md",
    "docs/error-codes.md",
    "docs/install.md",
    "docs/legacy.md",
    "docs/quickstart.md",
    "docs/language/source-and-syntax.md",
    "docs/language/types-and-values.md",
    "docs/language/modules-and-functions.md",
    "docs/language/resources.md",
    "docs/language/durable-places.md",
    "docs/language/traversal-and-indexes.md",
    "docs/language/control-flow.md",
    "docs/language/errors-and-transactions.md",
    "docs/language/evolution.md",
    "docs/language/builtins.md",
    "docs/language/standard-library.md",
    "docs/language/execution-limits.md",
    "docs/language/grammar.md",
    "docs/language/sample.md",
    "docs/tools/project-file.md",
    "docs/tools/cli.md",
    "docs/tools/data.md",
    "docs/tools/evolution.md",
    "docs/tools/backup-and-restore.md",
    "docs/tools/diagnostics.md",
    "docs/operations/native-store.md",
    "docs/operations/recovery.md",
    "docs/implementation/compiler.md",
    "docs/implementation/legacy.md",
    "docs/implementation/runtime.md",
    "docs/implementation/storage.md",
    "docs/implementation/lifecycle.md",
    "docs/implementation/tooling.md",
    "docs/implementation/testing.md",
    "docs/implementation/syntax.md",
    "docs/future/compiled-programs.md",
    "docs/future/semantic-paths.md",
    "docs/future/admission-and-activation.md",
    "docs/future/source-standard-library.md",
    "docs/future/local-applications.md",
    "docs/future/public-paths-and-authority.md",
    "docs/future/served-execution.md",
];

const REMOVED_DOCUMENTATION: &[&str] = &[
    "docs/design",
    "docs/backend-contract.md",
    "docs/cli.md",
    "docs/data-evolution.md",
    "docs/data-modeling.md",
    "docs/data-tools.md",
    "docs/operations.md",
    "docs/project-config.md",
    "docs/stability.md",
    "docs/surface-abi.md",
    "docs/testing-architecture.md",
    "docs/tooling-surfaces.md",
    "docs/language/cost-model.md",
    "docs/language/control-flow-and-effects.md",
    "docs/language/enums.md",
    "docs/language/modules-functions.md",
    "docs/language/resources-and-storage.md",
    "docs/language/syntax.md",
    "docs/language/types.md",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root sits two levels above crates/marrow")
        .to_path_buf()
}

fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    (level > 0 && trimmed.as_bytes().get(level) == Some(&b' ')).then_some(level)
}

fn fenced_block_after(markdown: &str, heading: &str, language: &str) -> String {
    let requested_level = heading_level(heading).expect("requested heading is Markdown");
    let mut lines = markdown.lines();
    assert!(
        lines.by_ref().any(|line| line.trim() == heading),
        "README section exists"
    );

    let opening = format!("```{language}");
    let mut source = None::<String>;
    for line in lines {
        if source.is_none() {
            if heading_level(line).is_some_and(|level| level <= requested_level) {
                break;
            }
            if line.trim() == opening {
                source = Some(String::new());
            }
            continue;
        }

        if line.trim() == "```" {
            let mut source = source.expect("source fence is open");
            source.pop();
            return source;
        }
        let source = source.as_mut().expect("source fence is open");
        source.push_str(line);
        source.push('\n');
    }

    panic!("section contains a closed {language} fenced block")
}

fn markdown_link_targets(markdown: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }

        let mut remaining = line;
        while let Some((_, after_open)) = remaining.split_once("](") {
            let Some((target, after_close)) = after_open.split_once(')') else {
                break;
            };
            targets.push(
                target
                    .trim()
                    .trim_matches(|character| matches!(character, '<' | '>')),
            );
            remaining = after_close;
        }

        if let Some(target) = reference_definition_target(line) {
            targets.push(target);
        }
    }
    targets
}

fn reference_definition_target(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('[') || trimmed.starts_with("[^") {
        return None;
    }
    let label_end = trimmed.find("]:")?;
    if label_end < 2 {
        return None;
    }
    let destination = trimmed[label_end + 2..].trim_start();
    if let Some(destination) = destination.strip_prefix('<') {
        destination.split_once('>').map(|(target, _)| target)
    } else {
        destination.split_whitespace().next()
    }
}

fn markdown_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read documentation directory") {
        let path = entry.expect("documentation entry").path();
        if path.is_dir() {
            markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn heading_anchors(markdown: &str) -> Vec<String> {
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut in_fence = false;
    markdown
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                return None;
            }
            if in_fence {
                return None;
            }
            heading_level(line).map(|level| trimmed[level..].trim())
        })
        .map(|heading| {
            let base: String = heading
                .chars()
                .flat_map(char::to_lowercase)
                .filter_map(|character| {
                    if character.is_alphanumeric() || matches!(character, '-' | '_') {
                        Some(character)
                    } else if character.is_whitespace() {
                        Some('-')
                    } else {
                        None
                    }
                })
                .collect();
            let occurrence = occurrences.entry(base.clone()).or_default();
            let anchor = if *occurrence == 0 {
                base
            } else {
                format!("{base}-{occurrence}")
            };
            *occurrence += 1;
            anchor
        })
        .collect()
}

fn assert_example_checks(source: &str, name: &str) {
    let project = temp_dir(name);
    write(
        &project,
        "marrow.json",
        r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"}}"#,
    );
    let module = source
        .lines()
        .find_map(|line| line.strip_prefix("module "))
        .expect("a checked documentation example starts with a module declaration")
        .trim();
    let source_path = format!("src/{}.mw", module.replace("::", "/"));
    write(&project, &source_path, source);

    let output = marrow(&[
        "check",
        project.to_str().expect("temporary project path is UTF-8"),
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "documentation example must check through the production CLI: {output:?}"
    );
}

fn mw_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut source = None::<String>;
    for line in markdown.lines() {
        if line.trim() == "```mw" {
            assert!(source.is_none(), "documentation nests an mw fence");
            source = Some(String::new());
            continue;
        }
        if line.trim() == "```" && source.is_some() {
            blocks.push(source.take().expect("source fence is open"));
            continue;
        }
        if let Some(source) = source.as_mut() {
            source.push_str(line);
            source.push('\n');
        }
    }
    assert!(source.is_none(), "documentation leaves an mw fence open");
    blocks
}

#[test]
fn root_readme_example_checks_through_the_cli() {
    let root = repo_root();
    let readme = fs::read_to_string(root.join("README.md")).expect("read root README");
    let source = fenced_block_after(&readme, "## Example", "mw");
    assert_example_checks(&source, "root-readme-example");
}

#[test]
fn language_readme_example_checks_through_the_cli() {
    let root = repo_root();
    let readme =
        fs::read_to_string(root.join("docs/language/README.md")).expect("read language README");
    let source = fenced_block_after(&readme, "## First Look", "mw");
    assert_example_checks(&source, "language-readme-example");
}

#[test]
fn every_current_mw_fence_is_a_complete_checked_module() {
    let root = repo_root();
    let mut files = vec![
        root.join("CONTRIBUTING.md"),
        root.join("README.md"),
        root.join("SECURITY.md"),
    ];
    markdown_files(&root.join("docs"), &mut files);
    files.sort();

    for file in files {
        let relative = file.strip_prefix(&root).expect("file is in repository");
        if relative.starts_with("docs/future") {
            continue;
        }
        let markdown = fs::read_to_string(&file).expect("read documentation");
        for (index, source) in mw_blocks(&markdown).into_iter().enumerate() {
            assert!(
                source.trim_start().starts_with("module "),
                "{} mw block {} is an unchecked fragment; use a complete module or a text fence",
                relative.display(),
                index + 1
            );
            assert_example_checks(&source, "documentation-module-example");
        }
    }
}

#[test]
fn documentation_links_and_anchors_resolve() {
    let root = repo_root();
    let mut files = vec![
        root.join("CONTRIBUTING.md"),
        root.join("README.md"),
        root.join("SECURITY.md"),
    ];
    markdown_files(&root.join("docs"), &mut files);
    files.sort();
    let mut failures = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .expect("documentation file is inside the repository");
        let markdown = fs::read_to_string(&file).expect("read documentation entrypoint");
        for target in markdown_link_targets(&markdown) {
            if target.starts_with("https://")
                || target.starts_with("http://")
                || target.starts_with("mailto:")
            {
                continue;
            }
            let (path_text, anchor) = target
                .split_once('#')
                .map_or((target, None), |(path, anchor)| (path, Some(anchor)));
            let target_path = if path_text.is_empty() {
                file.clone()
            } else {
                file.parent()
                    .expect("entrypoint has a parent")
                    .join(path_text)
            };
            if !target_path.exists() {
                failures.push(format!(
                    "{}: missing link target {target}",
                    relative.display()
                ));
            } else if let Some(anchor) = anchor {
                let target_file = if target_path.is_dir() {
                    target_path.join("README.md")
                } else {
                    target_path
                };
                let target_markdown =
                    fs::read_to_string(&target_file).expect("read linked Markdown target");
                if !heading_anchors(&target_markdown)
                    .iter()
                    .any(|candidate| candidate == anchor)
                {
                    failures.push(format!(
                        "{}: missing link anchor {target}",
                        relative.display()
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "documentation entrypoint link failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn current_docs_do_not_link_to_retired_authority() {
    let root = repo_root();
    let mut files = vec![root.join("README.md")];
    markdown_files(&root.join("docs"), &mut files);
    let mut failures = Vec::new();
    for file in files {
        let relative = file.strip_prefix(&root).expect("file is in repository");
        if relative.starts_with("docs/future") {
            continue;
        }
        let markdown = fs::read_to_string(&file).expect("read current documentation");
        for target in markdown_link_targets(&markdown) {
            let portal_may_index_future =
                matches!(relative.to_str(), Some("docs/README.md" | "docs/status.md"));
            if (target.contains("future/") && !portal_may_index_future)
                || target.contains("adr/")
                || target.contains("agents-work/")
                || target.contains("roadmaps/")
                || target.contains("design/")
            {
                failures.push(format!("{}: {target}", relative.display()));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "current documentation links to retired authority:\n{}",
        failures.join("\n")
    );
}

#[test]
fn status_defines_the_documentation_states() {
    let status = fs::read_to_string(repo_root().join("docs/status.md")).expect("read status page");
    for state in ["Current", "Legacy", "Future"] {
        assert!(
            status.contains(&format!("| {state} |")),
            "docs/status.md is missing the {state} state"
        );
    }
    assert!(
        !status.contains("Accepted target"),
        "docs/status.md must not recreate a target-contract authority layer"
    );
}

#[test]
fn generated_error_reference_does_not_publish_speculative_codes() {
    let reference =
        fs::read_to_string(repo_root().join("docs/error-codes.md")).expect("read error reference");
    for forbidden in [
        "Reserved And Future Codes",
        "check.surface_decl",
        "check.surface_catalog_pending",
        "check.surface_operation",
        "decode.shape",
        "decode.unknown_member",
        "decode.required_absent",
        "decode.value",
        "surface.integrity",
        "future renderer profile",
    ] {
        assert!(
            !reference.contains(forbidden),
            "generated error reference publishes speculative entry {forbidden:?}"
        );
    }
}

#[test]
fn documentation_tree_has_one_current_reference_and_no_superseded_tree() {
    let root = repo_root();
    let missing = ENTRYPOINTS
        .iter()
        .chain(REQUIRED_DOCUMENTATION)
        .copied()
        .filter(|path| !root.join(path).is_file())
        .collect::<Vec<_>>();
    let surviving = REMOVED_DOCUMENTATION
        .iter()
        .copied()
        .filter(|path| root.join(path).exists())
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "new documentation tree is incomplete: {missing:#?}"
    );
    assert!(
        surviving.is_empty(),
        "superseded documentation paths still exist: {surviving:#?}"
    );

    let expected = ENTRYPOINTS
        .iter()
        .chain(REQUIRED_DOCUMENTATION)
        .copied()
        .filter(|path| path.starts_with("docs/"))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let mut discovered = Vec::new();
    markdown_files(&root.join("docs"), &mut discovered);
    let discovered = discovered
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(&root).expect("documentation is in repo");
            (!matches!(
                relative.to_str(),
                Some("docs/implementation/AGENTS.md" | "docs/implementation/CLAUDE.md")
            ))
            .then(|| relative.to_string_lossy().into_owned())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        discovered, expected,
        "public documentation inventory drifted; classify every added or removed page"
    );
}

#[test]
fn every_public_document_is_reachable_from_the_project_readme() {
    let root = repo_root()
        .canonicalize()
        .expect("canonical repository root");
    let expected = ENTRYPOINTS
        .iter()
        .chain(REQUIRED_DOCUMENTATION)
        .copied()
        .map(|path| {
            root.join(path)
                .canonicalize()
                .expect("canonical public document")
        })
        .collect::<BTreeSet<_>>();

    let start = root.join("README.md");
    let mut queue = VecDeque::from([start.clone()]);
    let mut reached = BTreeSet::new();
    while let Some(file) = queue.pop_front() {
        let file = file.canonicalize().expect("canonical linked document");
        if !reached.insert(file.clone()) {
            continue;
        }
        let markdown = fs::read_to_string(&file).expect("read linked document");
        for target in markdown_link_targets(&markdown) {
            if target.starts_with("https://")
                || target.starts_with("http://")
                || target.starts_with("mailto:")
                || target.starts_with('#')
            {
                continue;
            }
            let path = target.split('#').next().unwrap_or_default();
            if path.is_empty() {
                continue;
            }
            let mut linked = file.parent().expect("document parent").join(path);
            if linked.is_dir() {
                linked = linked.join("README.md");
            }
            if linked
                .extension()
                .is_some_and(|extension| extension == "md")
                && let Ok(linked) = linked.canonicalize()
                && expected.contains(&linked)
            {
                queue.push_back(linked);
            }
        }
    }

    let unreachable = expected.difference(&reached).collect::<Vec<_>>();
    assert!(
        unreachable.is_empty(),
        "public documentation is orphaned from README.md: {unreachable:#?}"
    );
}

#[test]
fn current_reference_has_no_parallel_spec_or_legacy_product_model() {
    let root = repo_root();
    let mut authority_files = vec![root.join("AGENTS.md"), root.join("docs/README.md")];
    markdown_files(&root.join("crates"), &mut authority_files);
    markdown_files(&root.join("docs/language"), &mut authority_files);
    authority_files.push(root.join("docs/implementation/AGENTS.md"));
    authority_files.push(root.join("docs/quickstart.md"));

    let banned_authority = [
        "docs/design",
        "accepted target",
        "target contract",
        "approval packet",
    ];
    let banned_language = [
        "surface declaration",
        "generated client",
        "operation tag",
        "cost model",
        "hidden scan",
        "crud",
    ];
    let mut failures = Vec::new();

    for file in authority_files {
        let relative = file.strip_prefix(&root).expect("file is in repository");
        let text = fs::read_to_string(&file)
            .expect("read documentation authority")
            .to_lowercase();
        for term in banned_authority {
            if text.contains(term) {
                failures.push(format!(
                    "{} contains authority term {term:?}",
                    relative.display()
                ));
            }
        }
        if relative.starts_with("docs/language") || relative == Path::new("docs/quickstart.md") {
            for term in banned_language {
                if text.contains(term) {
                    failures.push(format!(
                        "{} contains legacy product term {term:?}",
                        relative.display()
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "canonical documentation recreates rejected authority or product models:\n{}",
        failures.join("\n")
    );
}

#[test]
fn example_extraction_does_not_fall_through_to_the_next_section() {
    let markdown = "## Example\n\nNo example here.\n\n## Later\n\n```mw\nmodule wrong\n```\n";

    let extraction = std::panic::catch_unwind(|| fenced_block_after(markdown, "## Example", "mw"));
    assert!(extraction.is_err());
}

#[test]
fn reference_style_link_definitions_are_checked_as_targets() {
    let markdown = "See [the retired design][old].\n\n[old]: future/design.md\n";

    assert_eq!(markdown_link_targets(markdown), ["future/design.md"]);
}
