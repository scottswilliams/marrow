use crate::support::{marrow, temp_dir, write};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const ENTRYPOINTS: &[&str] = &[
    "README.md",
    "docs/README.md",
    "docs/language/README.md",
    "docs/vision.md",
    "docs/status.md",
    "docs/design/README.md",
    "docs/future/README.md",
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
            if path.file_name().is_some_and(|name| name == "future") {
                continue;
            }
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
    write(&project, "src/app/tasks.mw", source);

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
fn documentation_entrypoint_links_resolve() {
    let root = repo_root();
    let mut failures = Vec::new();
    for relative in ENTRYPOINTS {
        let file = root.join(relative);
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
                failures.push(format!("{relative}: missing link target {target}"));
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
                    failures.push(format!("{relative}: missing link anchor {target}"));
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
        let markdown = fs::read_to_string(&file).expect("read current documentation");
        for target in markdown_link_targets(&markdown) {
            if target.contains("future/")
                || target.contains("adr/")
                || target.contains("agents-work/")
                || target.contains("roadmaps/")
            {
                let relative = file.strip_prefix(&root).expect("file is in repository");
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
    for state in [
        "Current",
        "Legacy",
        "Designed",
        "Accepted target",
        "Research",
    ] {
        assert!(
            status.contains(&format!("| {state} |")),
            "docs/status.md is missing the {state} state"
        );
    }
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
