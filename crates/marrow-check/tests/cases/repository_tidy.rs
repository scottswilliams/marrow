use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Recursively collect the repo-relative paths of files under `dir`, skipping build
/// output, version-control state, and any hidden entry, so the scan sees the tracked
/// source tree and never a stray artifact dir.
fn tracked_files(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
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
/// anywhere in the source tree except the three places that legitimately spell it — this gate,
/// the forbidden-vocabulary token in `language_reference_docs.rs`, and the run-path negative
/// assertion that proves the run projects a lock and never the removed artifact
/// (`run_cli_fence.rs`). The store is the saved-data identity authority and `marrow.lock` its
/// committed projection; reintroducing the file name fails this gate.
#[test]
fn marrow_catalog_json_appears_only_in_allowed_places() {
    const ARTIFACT: &str = "marrow.catalog.json";
    let allowed: [&Path; 3] = [
        Path::new("crates/marrow-check/tests/cases/language_reference_docs.rs"),
        Path::new("crates/marrow-check/tests/cases/repository_tidy.rs"),
        Path::new("crates/marrow-run/tests/cases/run_cli_fence.rs"),
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

#[test]
fn diagnostic_error_slice_scans_have_one_checkpoint_owner() {
    let root = repo_root();
    let source_root = root.join("crates/marrow-check/src");
    let owner = Path::new("crates/marrow-check/src/checks/diagnostics.rs");
    let mut files = Vec::new();
    tracked_files(&source_root, &root, &mut files);

    let mut violations = Vec::new();
    for relative in files {
        if relative == owner || relative.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(root.join(&relative)).expect("read Rust source");
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains("Severity::Error") {
                continue;
            }
            let start = index.saturating_sub(4);
            if lines[start..=index]
                .iter()
                .any(|candidate| candidate.contains("diagnostics["))
            {
                violations.push(format!("{}:{}", relative.display(), index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "direct diagnostic severity-slice scans must use ErrorCheckpoint: {violations:#?}",
    );
}

fn item_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source
        .find(start)
        .unwrap_or_else(|| panic!("missing `{start}`"));
    let tail = &source[start..];
    let end = tail.find(end).unwrap_or_else(|| panic!("missing `{end}`"));
    &tail[..end]
}

#[test]
fn migrated_admission_boundaries_have_no_legacy_status_or_raw_state_catch_all() {
    let root = repo_root();
    let infer =
        std::fs::read_to_string(root.join("crates/marrow-check/src/infer.rs")).expect("read infer");
    let calls = std::fs::read_to_string(root.join("crates/marrow-check/src/checks/calls.rs"))
        .expect("read calls");
    let saved = std::fs::read_to_string(root.join("crates/marrow-check/src/checks/saved_keys.rs"))
        .expect("read saved keys");
    let operators =
        std::fs::read_to_string(root.join("crates/marrow-check/src/checks/operators.rs"))
            .expect("read operators");
    let enums =
        std::fs::read_to_string(root.join("crates/marrow-check/src/enums.rs")).expect("read enums");

    for legacy in ["SavedKeyArgStatus", "LocalKeyStatus"] {
        assert!(
            !infer.contains(legacy) && !saved.contains(legacy),
            "legacy boundary status `{legacy}` must stay deleted",
        );
    }

    for (source, start, end) in [
        (
            &calls,
            "fn check_assert_equal_args(",
            "fn check_unknown_std_operation(",
        ),
        (
            &calls,
            "fn check_identity_constructor(",
            "pub(crate) fn check_next_id(",
        ),
        (&calls, "pub(crate) fn check_next_id(", "fn check_neighbor("),
        (&calls, "fn check_key(", "fn check_append("),
        (
            &operators,
            "pub(crate) fn check_throw_type(",
            "pub(crate) fn check_return_type(",
        ),
        (
            &saved,
            "pub(crate) fn check_saved_key_args(",
            "fn check_layer_key_args(",
        ),
        (
            &infer,
            "fn local_collection_access_type(",
            "fn key_arg_span(",
        ),
        (
            &enums,
            "pub(crate) fn check_is(",
            "fn enum_visible_in_program(",
        ),
    ] {
        let item = item_between(source, start, end);
        let raw_catch_all = [
            "MarrowType::Dynamic",
            "MarrowType::Invalid",
            "MarrowType::NoValue",
            "MarrowType::Unknown",
        ]
        .iter()
        .all(|variant| item.contains(variant));
        assert!(
            !raw_catch_all,
            "migrated boundary `{start}` must dispatch through typed admission",
        );
    }

    for (path, source) in [("infer.rs", &infer), ("checks/saved_keys.rs", &saved)] {
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains("!= Some(") {
                continue;
            }
            let start = index.saturating_sub(2);
            assert!(
                !lines[start..=index]
                    .iter()
                    .any(|candidate| candidate.contains("type_compatible")),
                "{path}:{} uses raw compatibility as boundary policy",
                index + 1,
            );
        }
    }
}

#[test]
fn no_value_value_boundaries_use_typed_state_dispatch() {
    let root = repo_root();
    let infer =
        std::fs::read_to_string(root.join("crates/marrow-check/src/infer.rs")).expect("read infer");
    let driver = std::fs::read_to_string(root.join("crates/marrow-check/src/checks/driver.rs"))
        .expect("read checker driver");
    let diagnostics = std::fs::read_to_string(root.join("crates/marrow-check/src/diagnostics.rs"))
        .expect("read diagnostics");
    let operators =
        std::fs::read_to_string(root.join("crates/marrow-check/src/checks/operators.rs"))
            .expect("read operators");
    let ranges = std::fs::read_to_string(root.join("crates/marrow-check/src/checks/ranges.rs"))
        .expect("read ranges");
    let statements =
        std::fs::read_to_string(root.join("crates/marrow-check/src/checks/statements.rs"))
            .expect("read statements");
    let enums =
        std::fs::read_to_string(root.join("crates/marrow-check/src/enums.rs")).expect("read enums");
    let typerules = std::fs::read_to_string(root.join("crates/marrow-check/src/typerules.rs"))
        .expect("read typerules");

    let boundaries = [
        (
            &operators,
            "pub(crate) fn check_return_type(",
            "pub(crate) fn check_assignment(",
            "admit_strict_value(",
        ),
        (
            &operators,
            "pub(crate) fn check_assignment(",
            "pub(crate) fn check_unary(",
            "admit_strict_value(",
        ),
        (
            &operators,
            "pub(crate) fn check_unary(",
            "pub(crate) fn check_binary(",
            "disposition(",
        ),
        (
            &operators,
            "pub(crate) fn check_binary(",
            "fn check_equality(",
            "disposition(",
        ),
        (
            &operators,
            "pub(crate) fn check_coalesce(",
            "fn coalesce_base(",
            "disposition(",
        ),
        (
            &enums,
            "fn report_non_enum_match(",
            "fn check_match_coverage(",
            "disposition(",
        ),
        (
            &enums,
            "pub(crate) fn check_is(",
            "fn enum_visible_in_program(",
            "disposition(",
        ),
        (
            &ranges,
            "pub(crate) fn check_range_header(",
            "fn check_date_step_whole_days(",
            "admit_range_step(",
        ),
        (
            &infer,
            "fn binding_type(",
            "/// Record `name`'s type",
            "admit_inferred_binding(",
        ),
        (
            &driver,
            "fn checked_file_prelude(",
            "fn infer_module_const_value(",
            "admit_inferred_binding(",
        ),
        (
            &infer,
            "Expression::Interpolation { parts, .. } => {",
            "Expression::Name { segments, span, .. }",
            "ErrorCheckpoint",
        ),
        (
            &infer,
            "fn infer_field_access(",
            "fn reject_saved_access(",
            "disposition(",
        ),
        (
            &statements,
            "fn check_binding_statement(",
            "fn check_uninitialized_binding(",
            "admit_inferred_binding(",
        ),
        (
            &statements,
            "fn check_for(",
            "fn check_try(",
            "admit_collection_operand(",
        ),
        (
            &statements,
            "fn target_already_blamed(",
            "fn span_contains(",
            "disposition(",
        ),
        (
            &diagnostics,
            "pub(crate) fn accepts(self, source: &MarrowType)",
            "fn join_or_list(",
            "disposition(",
        ),
        (
            &typerules,
            "pub(crate) fn type_renderable_at_runtime(",
            "pub(crate) fn is_ordered(",
            "disposition(",
        ),
    ];
    let grouped = [
        "MarrowType::Dynamic | MarrowType::NoValue",
        "MarrowType::NoValue | MarrowType::Dynamic",
        "MarrowType::Invalid | MarrowType::NoValue",
        "MarrowType::NoValue | MarrowType::Invalid",
        "MarrowType::NoValue | MarrowType::Unknown",
        "MarrowType::Unknown | MarrowType::NoValue",
        "TypeDisposition::Recovery | TypeDisposition::NoValue",
        "TypeDisposition::NoValue | TypeDisposition::Recovery",
        "TypeDisposition::ExplicitDynamic | TypeDisposition::NoValue",
        "TypeDisposition::NoValue | TypeDisposition::ExplicitDynamic",
        "TypeDisposition::Poisoned | TypeDisposition::NoValue",
        "TypeDisposition::NoValue | TypeDisposition::Poisoned",
    ];
    let mut violations = Vec::new();
    for (source, start, end, owner) in boundaries {
        let item = item_between(source, start, end);
        if !item.contains(owner) {
            violations.push(format!("`{start}` does not use `{owner}`"));
        }
        let compact = item.split_whitespace().collect::<Vec<_>>().join(" ");
        for pattern in grouped {
            if compact.contains(pattern) {
                violations.push(format!("`{start}` groups `{pattern}`"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "NoValue value boundaries must use typed, separate state dispatch: {violations:#?}",
    );
}

#[test]
fn public_tooling_renderers_are_checked_program_bound() {
    let root = repo_root();
    let render = std::fs::read_to_string(root.join("crates/marrow-check/src/tooling/render.rs"))
        .expect("read tooling render");

    for (start, end) in [
        (
            "pub fn render_callable_signature(",
            "pub(crate) fn render_callable_signature_with_names(",
        ),
        (
            "pub fn render_callable_shape(",
            "pub(crate) fn render_callable_shape_with_names(",
        ),
        (
            "pub fn render_marrow_type(",
            "pub(crate) fn render_marrow_type_with_names(",
        ),
    ] {
        let item = item_between(&render, start, end);
        assert!(
            item.contains("CheckedProgram"),
            "`{start}` must be snapshot-bound"
        );
        assert!(
            !item.contains("DeclIds"),
            "`{start}` must not expose the compiler's nominal-id recovery view",
        );
    }
}

#[test]
fn internal_type_audit_reuses_one_snapshot_aligned_lexical_cache() {
    let root = repo_root();
    let audit = std::fs::read_to_string(
        root.join("crates/marrow-check/src/analysis/internal_type_audit.rs"),
    )
    .expect("read internal type audit");

    assert_eq!(
        audit.matches("lex_source(").count(),
        1,
        "the developer audit must tokenize each analyzed file only in its aligned cache",
    );
    for owner in [
        "build_binding_index_from_lexed(",
        "PrelexedSourceHover::new(",
        "source_non_type_hover_fact_at_prelexed(",
        "trace_function_recovery_types(",
    ] {
        assert!(audit.contains(owner), "the audit must reuse `{owner}`");
    }
    for duplicate_owner in [
        "build_binding_index(snapshot)",
        "source_callable_hover_fact_at(",
        "source_module_path_hover_fact_at(",
        "store_root_hover_fact_at(",
        "source_schema_hover_fact_at(",
        "saved_place_hover_fact_at(",
        "source_operator_hover_fact_at(",
    ] {
        assert!(
            !audit.contains(duplicate_owner),
            "the audit must not invoke re-lexing owner `{duplicate_owner}`",
        );
    }
    for forbidden_sampler in [
        "type_at(",
        "is_representative_type_probe",
        "for (token_index, token)",
    ] {
        assert!(
            !audit.contains(forbidden_sampler),
            "the audit must derive recovery sites from the checker walk, not `{forbidden_sampler}`",
        );
    }
}

#[test]
fn recursive_poison_preflights_cover_dependent_boundary_families() {
    let root = repo_root();
    let infer =
        std::fs::read_to_string(root.join("crates/marrow-check/src/infer.rs")).expect("read infer");
    let calls = std::fs::read_to_string(root.join("crates/marrow-check/src/checks/calls.rs"))
        .expect("read calls");
    let saved = std::fs::read_to_string(root.join("crates/marrow-check/src/checks/saved_keys.rs"))
        .expect("read saved keys");
    let operators =
        std::fs::read_to_string(root.join("crates/marrow-check/src/checks/operators.rs"))
            .expect("read operators");
    let ranges = std::fs::read_to_string(root.join("crates/marrow-check/src/checks/ranges.rs"))
        .expect("read ranges");
    let statements =
        std::fs::read_to_string(root.join("crates/marrow-check/src/checks/statements.rs"))
            .expect("read statements");
    let enums =
        std::fs::read_to_string(root.join("crates/marrow-check/src/enums.rs")).expect("read enums");

    for (source, start, end) in [
        (
            &operators,
            "pub(crate) fn check_condition(",
            "pub(crate) fn check_throw_type(",
        ),
        (
            &operators,
            "pub(crate) fn check_return_type(",
            "pub(crate) fn check_assignment(",
        ),
        (
            &operators,
            "pub(crate) fn check_assignment(",
            "pub(crate) fn check_unary(",
        ),
        (
            &operators,
            "pub(crate) fn check_unary(",
            "pub(crate) fn check_binary(",
        ),
        (
            &operators,
            "pub(crate) fn check_binary(",
            "fn check_equality(",
        ),
        (
            &operators,
            "pub(crate) fn check_coalesce(",
            "fn coalesce_base(",
        ),
        (&infer, "fn infer_field_access(", "fn reject_saved_access("),
        (
            &infer,
            "fn local_collection_access_type(",
            "fn key_arg_span(",
        ),
        (
            &statements,
            "fn require_optional_if_const_subject(",
            "fn check_while(",
        ),
        (&statements, "fn check_for(", "fn check_try("),
        (
            &enums,
            "fn report_non_enum_match(",
            "fn check_match_coverage(",
        ),
    ] {
        let item = item_between(source, start, end);
        assert!(
            item.contains("contains_invalid")
                || item.contains("disposition(")
                || item.contains("admit_collection_operand("),
            "dependent boundary `{start}` must preflight recursive poison",
        );
    }

    let call = item_between(&calls, "pub(crate) fn check_call(", "fn dispatch_call(");
    let call_preflight = call
        .find("arg_types.iter().any(MarrowType::contains_invalid)")
        .expect("call recursive poison preflight");
    let call_dispatch = call.find("dispatch_call").expect("call dispatch");
    assert!(
        call_preflight < call_dispatch,
        "recursive call poison must return before dispatch",
    );
    assert!(
        !calls.contains("Some(MarrowType::Invalid)"),
        "call-specific top-level poison guards must not bypass the recursive preflight",
    );

    let saved = item_between(
        &saved,
        "pub(crate) fn check_saved_key_args(",
        "fn check_layer_key_args(",
    );
    let saved_preflight = saved
        .find("check.arg_types.iter().any(MarrowType::contains_invalid)")
        .expect("saved-key recursive poison preflight");
    let saved_lowering = saved
        .find("lower_expr_for_file")
        .expect("saved-key target lowering");
    assert!(
        saved_preflight < saved_lowering,
        "saved-key poison must precede name and shape checks",
    );

    let range = item_between(
        &ranges,
        "pub(crate) fn check_range_header(",
        "fn check_date_step_whole_days(",
    );
    assert!(
        range.contains("admit_range_endpoint(") && range.contains("admit_range_step("),
        "range endpoints and step must preflight recursive poison",
    );
}
