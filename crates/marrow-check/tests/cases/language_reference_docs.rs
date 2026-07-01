use crate::support;
use marrow_check::check_project;

use support::{config, temp_project, write};

const MIN_DOCUMENTED_MODULE_EXAMPLES: usize = 5;

struct MwBlock {
    file_name: String,
    index: usize,
    source: String,
}

fn language_docs_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language")
}

fn docs_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Pull the dotted codes documented in a single `### \`family.*\`` table from
/// `error-codes.md`. A code row starts with `| \`code\` |`, so the codes are read
/// directly from the rendered reference, never from prose.
fn documented_codes_in_family(family_heading: &str) -> std::collections::BTreeSet<String> {
    let text =
        std::fs::read_to_string(docs_dir().join("error-codes.md")).expect("read error-codes");
    let mut codes = std::collections::BTreeSet::new();
    let mut in_section = false;

    for line in text.lines() {
        if line.starts_with("### ") {
            in_section = line.contains(family_heading);
            continue;
        }
        if !in_section {
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("| `")
            && let Some(code) = rest.split('`').next()
        {
            codes.insert(code.to_string());
        }
    }

    codes
}

fn mw_blocks(file_name: &str) -> Vec<MwBlock> {
    let path = language_docs_dir().join(file_name);
    let text = std::fs::read_to_string(path).expect("read language doc");
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut index = 0usize;
    let mut source = String::new();

    for line in text.lines() {
        if line.trim() == "```mw" {
            in_block = true;
            index += 1;
            source.clear();
            continue;
        }
        if line.trim() == "```" && in_block {
            blocks.push(MwBlock {
                file_name: file_name.to_string(),
                index,
                source: source.clone(),
            });
            in_block = false;
            continue;
        }
        if in_block {
            source.push_str(line);
            source.push('\n');
        }
    }

    blocks
}

fn all_mw_blocks() -> Vec<MwBlock> {
    let mut files = std::fs::read_dir(language_docs_dir())
        .expect("read language docs")
        .map(|entry| entry.expect("language doc entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();

    files
        .into_iter()
        .flat_map(|path| {
            let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
            mw_blocks(&file_name)
        })
        .collect()
}

fn source_path_for_module(source: &str) -> String {
    let module_line = source
        .lines()
        .find(|line| line.starts_with("module "))
        .expect("documented example must be a complete module");
    let module = module_line.trim_start_matches("module ").trim();
    format!("src/{}.mw", module.replace("::", "/"))
}

/// Implementation-only storage and identity vocabulary that must never surface in
/// the language reference. Each token is matched case-insensitively as a substring,
/// so spacing and hyphenation variants of the same engine concept are all caught.
const FORBIDDEN_VOCABULARY: &[&str] = &[
    "marrow.catalog.json",
    "catalog",
    "epoch",
    "structural signature",
    "shape signature",
    "source digest",
    "shape digest",
    "catalog digest",
    "engine-profile digest",
    "opaque id",
    "opaque stable id",
    "stable-id annotation",
    "never-reuse",
    "id ledger",
    "identity ledger",
    "ledger",
    "commit stamp",
    "id stamp",
    "engine-profile",
    "engine profile",
    "value-codec",
    "value codec",
    "tree-cell key",
    "fence",
];

/// Public saved-data vocabulary the storage reference must keep, so the rewrite
/// cannot pass by deleting the section instead of restating it for developers.
const REQUIRED_VOCABULARY: &[&str] = &[
    "marrow.lock",
    "stale lock",
    "pending evolution",
    "backup",
    "restore",
    "rename",
    "retire",
];

#[test]
fn language_docs_use_public_vocabulary_only() {
    let mut files = std::fs::read_dir(language_docs_dir())
        .expect("read language docs")
        .map(|entry| entry.expect("language doc entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();

    let mut violations: Vec<(String, &str, usize)> = Vec::new();
    for path in &files {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).expect("read language doc");
        for (line_index, line) in text.to_lowercase().lines().enumerate() {
            for token in FORBIDDEN_VOCABULARY {
                if line.contains(token) {
                    violations.push((file_name.clone(), token, line_index + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "language docs leak implementation-only vocabulary: {violations:#?}"
    );

    let storage_doc = std::fs::read_to_string(language_docs_dir().join("resources-and-storage.md"))
        .expect("read resources-and-storage doc")
        .to_lowercase();
    let missing: Vec<&str> = REQUIRED_VOCABULARY
        .iter()
        .copied()
        .filter(|term| !storage_doc.contains(term))
        .collect();
    assert!(
        missing.is_empty(),
        "resources-and-storage.md must state the public saved-data model, missing: {missing:#?}"
    );
}

#[test]
fn documented_module_examples_check_clean() {
    let mut checked = 0usize;

    for block in all_mw_blocks()
        .into_iter()
        .filter(|block| block.source.trim_start().starts_with("module "))
    {
        let relative_path = source_path_for_module(&block.source);
        let root = temp_project("docs-module-example", |root| {
            write(root, &relative_path, &block.source);
        });
        let (report, _program) = check_project(&root, &config()).expect("check");

        assert!(
            report.diagnostics.is_empty(),
            "{} block {} produced checker diagnostics: {:#?}",
            block.file_name,
            block.index,
            report.diagnostics
        );
        checked += 1;
    }

    assert!(
        checked >= MIN_DOCUMENTED_MODULE_EXAMPLES,
        "expected at least {MIN_DOCUMENTED_MODULE_EXAMPLES} documented module examples, found {checked}"
    );
}

/// The `exists()`-narrowing example in `types.md` must check clean through the
/// production pipeline. `exists(place)` narrows the guarded read to present, so the
/// body reads the narrowed path directly; a redundant inner `if const` over the
/// already-present value is rejected. The example is a bare statement snippet, so it
/// is wrapped in the smallest module that gives `^books(id).subtitle` a saved sparse
/// field before checking.
#[test]
fn types_doc_exists_narrowing_example_checks_clean() {
    let block = mw_blocks("types.md")
        .into_iter()
        .find(|block| block.source.contains("if exists(^books(id).subtitle)"))
        .expect("types.md documents the exists() narrowing example");

    let mut module = String::from(
        "module main\n\n\
         resource Book\n    required title: string\n    subtitle: string\n\n\
         store ^books(id: int): Book\n\n\
         fn show(id: int)\n",
    );
    for line in block.source.lines() {
        module.push_str("    ");
        module.push_str(line);
        module.push('\n');
    }

    let root = temp_project("docs-exists-narrowing", |root| {
        write(root, "src/main.mw", &module);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report.diagnostics.is_empty(),
        "types.md exists() narrowing example produced checker diagnostics: {:#?}",
        report.diagnostics
    );
}

/// Recursively collect the repo-relative paths of files under `dir`, skipping build
/// output, version-control state, and any hidden entry, so the scan sees the tracked
/// source tree and never a stray artifact dir.
fn tracked_files(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("read repo dir");
    for entry in entries {
        let path = entry.expect("repo dir entry").path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            tracked_files(&path, root, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("path under root")
                    .to_path_buf(),
            );
        }
    }
}

/// Whole-repo absence gate: the removed `marrow.catalog.json` artifact name must not survive
/// anywhere in the source tree except the two places that legitimately spell it — this gate's
/// own allowlist and the L8 forbidden-vocabulary token (here, in `language_reference_docs.rs`)
/// and the run-path negative assertion that proves the run projects a lock and never the
/// removed artifact (`run_cli_fence.rs`). The store is the saved-data identity authority and
/// `marrow.lock` its committed projection; reintroducing the file name fails this gate.
#[test]
fn marrow_catalog_json_appears_only_in_allowed_places() {
    const ARTIFACT: &str = "marrow.catalog.json";
    let allowed: [&std::path::Path; 2] = [
        std::path::Path::new("crates/marrow-check/tests/cases/language_reference_docs.rs"),
        std::path::Path::new("crates/marrow-run/tests/cases/run_cli_fence.rs"),
    ];

    let root = repo_root();
    let mut files = Vec::new();
    tracked_files(&root, &root, &mut files);
    files.sort();

    let mut violations: Vec<String> = Vec::new();
    for relative in &files {
        if allowed.contains(&relative.as_path()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(relative)) else {
            continue;
        };
        for (line_index, line) in text.lines().enumerate() {
            if line.contains(ARTIFACT) {
                violations.push(format!("{}:{}", relative.display(), line_index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "the removed `{ARTIFACT}` artifact name reappeared outside its allowlist: {violations:#?}"
    );
}

/// The substrings inside backtick pairs, left to right. Markdown fences its inline code and
/// table cells in backticks, so this reads a heading's `family.*` tokens or a kind cell without a
/// regex dependency.
fn backtick_tokens(text: &str) -> Vec<String> {
    text.split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, part)| part.to_string())
        .collect()
}

/// The first dotted segments a family heading covers, e.g. `config` and `project` for the shared
/// `config.*`/`project.*` section.
fn family_segments(heading: &str) -> Vec<String> {
    backtick_tokens(heading)
        .into_iter()
        .filter_map(|token| token.strip_suffix(".*").map(str::to_string))
        .collect()
}

/// Every `(first segment, kind)` pair the reference states: the "How `kind` Is Assigned" summary
/// table and each `### family.*` section heading. The guard holds all of them against
/// `kind_for_code`, so the documented category can never disagree with the runtime classifier.
fn documented_kind_assignments(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_summary_table = false;

    for line in text.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            in_summary_table = section.contains("How `kind` Is Assigned");
            continue;
        }
        if let Some(heading) = line.strip_prefix("### ") {
            in_summary_table = false;
            if let Some((_, kind_cell)) = heading.split_once("kind `") {
                let kind = kind_cell.split('`').next().unwrap_or_default().to_string();
                pairs.extend(
                    family_segments(heading)
                        .into_iter()
                        .map(|segment| (segment, kind.clone())),
                );
            }
            continue;
        }
        if in_summary_table && line.trim_start().starts_with('|') {
            let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
            if let [segment_cell, kind_cell] = cells.as_slice()
                && let Some(kind) = backtick_tokens(kind_cell).into_iter().next()
            {
                pairs.extend(
                    backtick_tokens(segment_cell)
                        .into_iter()
                        .map(|segment| (segment, kind.clone())),
                );
            }
        }
    }

    pairs
}

fn schema_family_codes() -> Vec<String> {
    [
        marrow_schema::SCHEMA_DUPLICATE_MEMBER,
        marrow_schema::SCHEMA_CATEGORY_LEAF,
        marrow_schema::SCHEMA_PARENT_NOT_CATEGORY,
        marrow_check::SCHEMA_DUPLICATE_ROOT_OWNER,
        marrow_schema::SCHEMA_UNKNOWN_IN_SAVED,
        marrow_schema::SCHEMA_OPTIONAL_IN_SAVED,
        marrow_schema::SCHEMA_KEY_MEMBER_COLLISION,
        marrow_schema::SCHEMA_UNKNOWN_INDEX_ARG,
        marrow_schema::SCHEMA_UNORDERABLE_KEY,
        marrow_schema::SCHEMA_NONSCALAR_KEY,
        marrow_schema::SCHEMA_NON_ENUM_NAMED_FIELD,
        marrow_schema::SCHEMA_INDEX_MISSING_IDENTITY_KEYS,
        marrow_schema::SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
        marrow_schema::SCHEMA_NESTED_INDEX_ARG,
    ]
    .iter()
    .map(|code| code.to_string())
    .collect()
}

/// Every active error-code family documented in `error-codes.md`, mapped to the exact codes it
/// emits. `catalog` and `schema` are pinned to their exported `&str` constants, so a documented
/// code the crate never defines — or an emitted constant left undocumented — fails the guard. The
/// remaining families, whose codes are still inline string literals scattered across the pipeline,
/// are pinned to the reviewed reference set until the code registry centralizes them; the guard
/// still fails the moment a family's table drifts from this list.
fn error_code_families() -> Vec<(&'static str, Vec<String>)> {
    let owned = |codes: &[&str]| {
        codes
            .iter()
            .map(|code| code.to_string())
            .collect::<Vec<_>>()
    };
    vec![
        ("`parse.*`", owned(&["parse.syntax"])),
        ("`fmt.*`", owned(&["fmt.comment_loss"])),
        (
            "`check.*`",
            owned(&[
                "check.failed",
                "check.module_path",
                "check.default_entry",
                "check.duplicate_module",
                "check.multiple_scripts",
                "check.duplicate_declaration",
                "check.builtin_collision",
                "check.surface_collision",
                "check.surface_target",
                "check.surface_field",
                "check.surface_action",
                "check.surface_computed_read",
                "check.unresolved_import",
                "check.unknown_type",
                "check.recursive_keyed_entry",
                "check.return_value",
                "check.missing_return",
                "check.operator_type",
                "check.condition_type",
                "check.call_argument",
                "check.return_type",
                "check.assignment_type",
                "check.lossy_round_trip",
                "check.required_absent",
                "check.uninitialized_var",
                "check.commit_amplification",
                "check.untyped_value",
                "check.key_type",
                "check.sequence_position",
                "check.unresolved_name",
                "check.unknown_field",
                "check.layer_not_value",
                "check.unresolved_call",
                "check.private_function",
                "check.ambiguous_call",
                "check.next_id_requires_single_int",
                "check.next_id_collision",
                "check.rejected_surface",
                "check.catalog_intent",
                "check.lock_corrupt",
                "check.lock_missing",
                "check.stale_lock",
                "check.stale_client",
                "check.durable_store_required",
                "check.unresolved_optional",
                "check.unannotated_absent",
                "check.literal_range",
                "check.string_escape",
                "check.bytes_escape",
                "check.loop_control_flow",
                "check.catch_type",
                "check.throw_type",
                "check.match_requires_enum",
                "check.unknown_enum_member",
                "check.duplicate_match_arm",
                "check.nonexhaustive_match",
                "check.ambiguous_match_arm",
                "check.scrutinee_qualified_match_arm",
                "check.ambiguous_member",
                "check.category_not_selectable",
                "check.is_requires_enum",
                "check.is_type",
                "check.invalid_assign_target",
                "check.non_constant_const",
                "check.loop_mutates_traversed_layer",
                "check.neighbor_unsupported",
                "check.key_requires_single_key",
                "check.range",
                "check.range_value",
                "check.collection_unsupported",
                "check.read_only_expression_context",
                "check.read_only_expression_write",
                "check.read_only_expression_host_effect",
                "check.read_only_expression_unindexed_lookup",
                "check.private_enum",
                "check.exposed_private_enum",
                "check.nesting_limit",
                "check.evolve_target",
                "check.evolve_type",
                "check.evolve_transform",
            ]),
        ),
        ("`schema.*`", schema_family_codes()),
        (
            "`catalog.*`",
            owned(&[
                marrow_catalog::CATALOG_INVALID,
                marrow_catalog::LOCK_CORRUPT,
            ]),
        ),
        (
            "`doctor.*`",
            owned(&[
                "doctor.config_invalid",
                "doctor.lock_corrupt",
                "doctor.check_failed",
                "doctor.store_locked",
                "doctor.store_recovery_required",
                "doctor.store_unavailable",
                "doctor.populated_unstamped",
                "doctor.catalog_collision",
                "doctor.store_lock_epoch_mismatch",
                "doctor.stale_lock",
                "doctor.fence_mismatch",
                "doctor.integrity_sample_failed",
            ]),
        ),
        (
            "`run.*`",
            owned(&[
                "run.type",
                "run.unbound_name",
                "run.overflow",
                "run.decimal_overflow",
                "run.temporal_overflow",
                "run.divide_by_zero",
                "run.no_enclosing_loop",
                "run.unknown_function",
                "run.ambiguous_function",
                "run.private_function",
                "run.entry_argument",
                "run.entry_surface",
                "run.no_value",
                "run.absent_element",
                "run.store",
                "run.unsupported",
                "run.capability",
                "run.transaction_host_effect",
                "run.assertion",
                "run.uncaught_error",
                "run.traversal",
                "run.depth",
                "run.no_entry",
                "run.durable_store_required",
                "run.dry_run_isolation",
                "run.store_evolved",
                "run.store_behind",
                "run.schema_drift",
                "run.engine_profile",
                "run.store_unstamped",
            ]),
        ),
        ("`value.*`", owned(&["value.range"])),
        (
            "`write.*`",
            owned(&[
                "write.required_absent",
                "write.type_mismatch",
                "write.identity_mismatch",
                "write.invalid_data",
                "write.store",
                "write.unknown_field",
                "write.unique_conflict",
                "write.unknown_layer",
                "write.not_a_leaf_layer",
                "write.not_a_group_layer",
                "write.layer_key_arity",
                "write.id_overflow",
                "write.next_id_unsupported",
                "write.required_field",
                "write.requires_maintenance",
                "write.transaction_too_large",
            ]),
        ),
        (
            "`store.*`",
            owned(&[
                "store.io",
                "store.permission_denied",
                "store.locked",
                "store.format_version",
                "store.corruption",
                "store.recovery_required",
                "store.limit",
                "store.cursor",
                "store.transaction",
                "store.read_only",
            ]),
        ),
        (
            "`io.*`",
            owned(&["io.read", "io.listen", "io.thread", "io.write"]),
        ),
        (
            "`config.*` and `project.*`",
            owned(&[
                "config.missing",
                "config.not_a_project",
                "config.invalid",
                "config.data_dir",
                "config.client_without_surface",
                "project.source_root",
            ]),
        ),
        (
            "`data.*`",
            owned(&[
                "data.decode",
                "data.key_type",
                "data.dangling_ref",
                "data.incomplete",
                "data.orphan",
                "data.unknown_path",
            ]),
        ),
        (
            "`evolve.*`",
            owned(&[
                "evolve.no_accepted_catalog",
                "evolve.repair_required",
                "evolve.drift",
                "evolve.catalog_drift",
                "evolve.maintenance_required",
                "evolve.approval_required",
                "evolve.approval_mismatch",
                "evolve.approval_target_unknown",
                "evolve.requires_backup",
                "evolve.backup_path_managed",
                "evolve.transform_faulted",
            ]),
        ),
        ("`test.*`", owned(&["test.none"])),
        (
            "`backup.*`",
            owned(&[
                "backup.catalog_serialization",
                "backup.cell_too_large",
                "backup.manifest_serialization",
                "backup.store_uid_missing",
            ]),
        ),
        (
            "`restore.*`",
            owned(&[
                "restore.format_version",
                "restore.corrupt_chunk",
                "restore.not_empty",
                "restore.engine_recompile_required",
                "restore.source_mismatch",
                "restore.catalog_mismatch",
                "restore.data_invalid",
            ]),
        ),
        (
            "`surface.*`",
            owned(&[
                "surface.request",
                "surface.auth",
                "surface.absent",
                "surface.cursor",
                "surface.stale_cursor",
                "surface.abi_mismatch",
                "surface.invalid_data",
                "surface.limit",
                "surface.conflict",
                "surface.write",
                "surface.action",
                "surface.computed",
                "surface.integrity",
                "surface.store",
            ]),
        ),
    ]
}

/// Code-truth guard: every documented error-code family lists exactly the codes that family emits.
/// A phantom documented code the crate never produces, or an emitted code left undocumented, fails
/// here. `catalog` and `schema` are pinned to their exported constants; the deleted
/// `catalog.merge_conflict` stays absent because it is not among those constants.
#[test]
fn error_codes_doc_documents_every_family() {
    for (heading, expected_codes) in error_code_families() {
        let documented = documented_codes_in_family(heading);
        let expected: std::collections::BTreeSet<String> = expected_codes.into_iter().collect();
        assert_eq!(
            documented, expected,
            "error-codes.md `{heading}` family drifted: the documented codes must equal the codes the pipeline emits"
        );
    }
}

/// Code-truth guard for the `kind` column: every family's documented `kind`, in both the summary
/// table and the section headings, must equal what `kind_for_code` derives from its first segment.
#[test]
fn documented_error_kinds_match_kind_for_code() {
    let text =
        std::fs::read_to_string(docs_dir().join("error-codes.md")).expect("read error-codes");
    let assignments = documented_kind_assignments(&text);

    for (segment, kind) in &assignments {
        assert_eq!(
            marrow_check::kind_for_code(&format!("{segment}.example")),
            kind,
            "error-codes.md documents `{segment}.*` as kind `{kind}`, but kind_for_code disagrees"
        );
    }

    assert!(
        assignments.len() >= 30,
        "expected the kind contract to cover every family plus the summary table, found {}",
        assignments.len()
    );
}

/// Absence gate: the throwaway root fixtures a checker session once left behind must not reappear
/// anywhere in the tracked tree. They are scratch parser inputs, not source, examples, or part of
/// the conformance corpus.
#[test]
fn stray_root_scratch_fixtures_are_absent() {
    const STRAY: [&str; 3] = ["bare_param.mw", "colon_no_ret.mw", "no_annot.mw"];

    let root = repo_root();
    let mut files = Vec::new();
    tracked_files(&root, &root, &mut files);

    let found: Vec<String> = files
        .iter()
        .filter(|relative| {
            relative
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| STRAY.contains(&name))
        })
        .map(|relative| relative.display().to_string())
        .collect();

    assert!(
        found.is_empty(),
        "stray scratch fixtures must stay deleted, found: {found:#?}"
    );
}
