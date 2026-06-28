//! The opt-in statement debugger hook (`StepHook` / `Frame` /
//! `run_entry_with_debugger`): statement and managed-write observation, depth,
//! live store handle, terminate-by-Err, and debug value previews.

use crate::support;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use support::*;

use marrow_check::{
    AnalysisSnapshot, CheckedDebugExpression, CheckedRuntimeProgram, ProjectSources,
    analyze_project, tooling,
};
use marrow_run::{
    CheckedEntryCall, DEBUG_VALUE_DEFAULT_PAGE_LIMIT, DEBUG_VALUE_MAX_PAGE_LIMIT,
    DebugFrameSnapshot, DebugValue, DebugValueFilter, DebugValuePage, Frame, Host, RunOutput,
    RuntimeError, Sequence, StepHook, Value, WriteTarget,
};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};
use marrow_syntax::SourceSpan;

fn run_entry_with_debugger(
    store: &TreeStore,
    host: &Host,
    hook: &mut dyn StepHook,
    call: CheckedEntryCall<'_>,
) -> Result<RunOutput, marrow_run::RuntimeError> {
    let mut output = |_text: &str| {};
    marrow_run::run_entry_with_debugger(store, host, hook, &call, &mut output)
}

fn analyzed_runtime_program(
    source: &str,
) -> (AnalysisSnapshot, CheckedRuntimeProgram, PathBuf, String) {
    let root = TempDir::new("marrow-run-debug-expression").expect("create project");
    analyzed_runtime_program_at(root.path(), source)
}

fn analyzed_runtime_program_at(
    root: &Path,
    source: &str,
) -> (AnalysisSnapshot, CheckedRuntimeProgram, PathBuf, String) {
    let (relative, text) = checked_source_file(source, &[]);
    write_temp_source(root, &relative, &text);
    let path = root.join(&relative);
    let config = test_project_config();
    let mut sources = ProjectSources::new();
    sources.insert(&path, text.clone());
    let snapshot = analyze_project(root, &config, &sources, None, None).expect("analyze project");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let runtime = snapshot.program.runtime();
    (snapshot, runtime, path, text)
}

fn analyzed_runtime_program_with_configured_test_at(
    root: &Path,
    source: &str,
    test_source: &str,
) -> (AnalysisSnapshot, CheckedRuntimeProgram, PathBuf, String) {
    let (relative, text) = checked_source_file(source, &[]);
    let test_relative = PathBuf::from("tests/smoke.mw");
    write_temp_source(root, &relative, &text);
    write_temp_source(root, &test_relative, test_source);
    let path = root.join(&relative);
    let mut config = test_project_config();
    config.tests = vec!["tests".to_string()];
    let mut sources = ProjectSources::new();
    sources.insert(&path, text.clone());
    sources.insert(root.join(&test_relative), test_source.to_string());
    let snapshot = analyze_project(root, &config, &sources, None, None).expect("analyze project");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let runtime = snapshot.program.runtime();
    (snapshot, runtime, path, text)
}

fn stop_span(source: &str, needle: &str) -> SourceSpan {
    let start_byte = source.find(needle).expect("stop marker is present");
    let before = &source[..start_byte];
    SourceSpan {
        start_byte,
        end_byte: start_byte + needle.len(),
        line: before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1,
        column: before
            .rsplit_once('\n')
            .map_or(before.len(), |(_, line)| line.len()) as u32
            + 1,
    }
}

/// A test hook that records, for each statement it is offered, the statement's
/// line and the sorted `name=display_debug` of its visible locals plus the
/// activation depth. Optionally aborts at a given line to exercise the
/// terminate-by-Err contract.
#[derive(Default)]
struct Recorder {
    steps: Vec<(u32, Vec<String>, usize)>,
    abort_at_line: Option<u32>,
}

impl StepHook for Recorder {
    fn before_statement(
        &mut self,
        span: SourceSpan,
        frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError> {
        let mut locals: Vec<String> = frame
            .locals()
            .map(|(name, value)| format!("{name}={}", value.display_debug()))
            .collect();
        locals.sort();
        self.steps.push((span.line, locals, frame.depth()));
        if self.abort_at_line == Some(span.line) {
            return Err(RuntimeError::fatal(
                marrow_run::RUN_UNSUPPORTED,
                "debugger terminate",
                span,
            ));
        }
        Ok(())
    }
}

#[test]
fn hook_observes_each_statement_with_its_locals_and_depth() {
    // Three statements on consecutive lines, each adding one local; the hook is
    // offered each before it runs, so it sees the locals bound by earlier ones.
    let program = checked_program(
        "pub fn compute(a: int): int\n\
         \x20\x20\x20\x20const b = a + 1\n\
         \x20\x20\x20\x20var c = b * 2\n\
         \x20\x20\x20\x20return c\n",
    );
    let store = TreeStore::memory();
    let mut hook = Recorder::default();
    let outcome = run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::compute", Value::Int(10)),
    )
    .expect("debugged run completes");
    assert_eq!(outcome.value, Some(Value::Int(22)));

    // The helper writes snippets as `module test`, so the body statements land
    // on lines 4..=6 in the checked source file.
    assert_eq!(
        hook.steps,
        vec![
            (4, vec!["a=10".to_string()], 1),
            (5, vec!["a=10".to_string(), "b=11".to_string()], 1),
            (
                6,
                vec!["a=10".to_string(), "b=11".to_string(), "c=22".to_string()],
                1,
            ),
        ],
    );
}

#[test]
fn hook_depth_tracks_nested_activations() {
    // A call deepens the activation; the callee's statements report a greater
    // depth than the caller's, so step-over/step-out can be expressed by depth.
    let program = checked_program(
        "pub fn inner(): int\n\
         \x20\x20\x20\x20return 1\n\
         \n\
         pub fn outer(): int\n\
         \x20\x20\x20\x20const x = inner()\n\
         \x20\x20\x20\x20return x\n",
    );
    let store = TreeStore::memory();
    let mut hook = Recorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::outer"),
    )
    .expect("debugged run completes");

    let depths: Vec<usize> = hook.steps.iter().map(|(_, _, depth)| *depth).collect();
    // outer's `const x = inner()` (depth 1), inner's `return 1` (depth 2 during
    // the nested call), then outer's `return x` (back at depth 1).
    assert_eq!(depths, vec![1, 2, 1]);
}

#[test]
fn frame_debug_snapshot_owns_visible_locals_and_runtime_value_children() {
    struct SnapshotRecorder {
        snapshots: Vec<DebugFrameSnapshot>,
    }

    impl StepHook for SnapshotRecorder {
        fn before_statement(
            &mut self,
            _span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            self.snapshots
                .push(frame.debug_snapshot(DebugValuePage::default(), DebugValueFilter::All));
            Ok(())
        }
    }

    let program = checked_program(
        "pub fn inspect(): int\n\
         \x20   const title: string = \"Dune\"\n\
         \x20   var tags: sequence[string]\n\
         \x20   tags(1) = \"classic\"\n\
         \x20   tags(2) = \"space\"\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   return 0\n",
    );
    let store = TreeStore::memory();
    let mut hook = SnapshotRecorder {
        snapshots: Vec::new(),
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::inspect"),
    )
    .expect("debugged run completes");

    let snapshot = hook.snapshots.last().expect("return statement snapshot");
    assert_eq!(snapshot.depth, 1);
    assert_eq!(snapshot.span.line, 11);
    assert!(
        snapshot
            .file
            .as_ref()
            .is_some_and(|path| path.ends_with("test.mw"))
    );
    let names: Vec<&str> = snapshot
        .locals
        .iter()
        .map(|local| local.name.as_str())
        .collect();
    assert_eq!(snapshot.visible_local_count, 3);
    assert!(!snapshot.locals_truncated);
    assert_eq!(names, ["title", "tags", "scores"]);

    let title = snapshot
        .locals
        .iter()
        .find(|local| local.name == "title")
        .expect("title local");
    assert_eq!(title.value.preview(), "Dune");
    assert_eq!(title.value.child_counts(), None);

    let tags = snapshot
        .locals
        .iter()
        .find(|local| local.name == "tags")
        .expect("tags local");
    assert_eq!(tags.value.preview(), "sequence[2]");
    assert_eq!(tags.value.child_counts().unwrap().indexed, Some(2));
    let children = tags
        .value
        .children(DebugValuePage::default(), DebugValueFilter::All);
    assert_eq!(children[0].name, "[1]");
    assert_eq!(children[0].value.preview(), "classic");
    assert_eq!(children[1].name, "[2]");
    assert_eq!(children[1].value.preview(), "space");

    let scores = snapshot
        .locals
        .iter()
        .find(|local| local.name == "scores")
        .expect("scores local");
    assert_eq!(scores.value.preview(), "tree[2]");
    assert_eq!(scores.value.child_counts().unwrap().named, Some(2));
    let score_children = scores
        .value
        .children(DebugValuePage::default(), DebugValueFilter::Named);
    assert_eq!(score_children[0].name, "(p1)");
    assert_eq!(score_children[0].value.preview(), "10");
    assert_eq!(score_children[1].name, "(p2)");
    assert_eq!(score_children[1].value.preview(), "20");
    assert!(
        scores
            .value
            .children(DebugValuePage::default(), DebugValueFilter::Indexed)
            .is_empty()
    );
}

#[test]
fn frame_debug_snapshot_resolves_shadowed_locals_to_the_visible_value() {
    struct SnapshotRecorder {
        snapshots: Vec<DebugFrameSnapshot>,
    }

    impl StepHook for SnapshotRecorder {
        fn before_statement(
            &mut self,
            _span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            self.snapshots
                .push(frame.debug_snapshot(DebugValuePage::default(), DebugValueFilter::All));
            Ok(())
        }
    }

    let program = checked_program(
        "pub fn shadow(): int\n\
         \x20   const value: int = 1\n\
         \x20   if true\n\
         \x20       const value: int = 2\n\
         \x20       return value\n\
         \x20   return value\n",
    );
    let store = TreeStore::memory();
    let mut hook = SnapshotRecorder {
        snapshots: Vec::new(),
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::shadow"),
    )
    .expect("debugged run completes");

    let snapshot = hook.snapshots.last().expect("inner return snapshot");
    assert_eq!(snapshot.locals.len(), 1);
    assert_eq!(snapshot.locals[0].name, "value");
    assert_eq!(snapshot.locals[0].value.preview(), "2");
}

#[test]
fn frame_debug_snapshot_reports_a_bounded_local_page() {
    struct SnapshotRecorder {
        snapshots: Vec<DebugFrameSnapshot>,
    }

    impl StepHook for SnapshotRecorder {
        fn before_statement(
            &mut self,
            _span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            self.snapshots
                .push(frame.debug_snapshot(DebugValuePage::new(1, 1), DebugValueFilter::All));
            Ok(())
        }
    }

    let program = checked_program(
        "pub fn inspect(): int\n\
         \x20   const a: int = 1\n\
         \x20   const b: int = 2\n\
         \x20   const c: int = 3\n\
         \x20   return a + b + c\n",
    );
    let store = TreeStore::memory();
    let mut hook = SnapshotRecorder {
        snapshots: Vec::new(),
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::inspect"),
    )
    .expect("debugged run completes");

    let snapshot = hook.snapshots.last().expect("return statement snapshot");
    assert_eq!(snapshot.visible_local_count, 3);
    assert!(snapshot.locals_truncated);
    assert_eq!(snapshot.locals.len(), 1);
    assert_eq!(snapshot.locals[0].name, "b");
    assert_eq!(snapshot.locals[0].value.preview(), "2");
}

#[test]
fn frame_evaluates_checked_debug_expression_from_live_shadowed_locals() {
    let (snapshot, program, path, source) = analyzed_runtime_program(
        "pub fn inspect(a: int): int\n\
         \x20   const shadow = a + 1\n\
         \x20   if true\n\
         \x20       const shadow = a + 2\n\
         \x20       const marker = shadow * 2\n\
         \x20       return marker\n\
         \x20   return shadow\n",
    );
    let stop = stop_span(&source, "return marker");
    let expression = snapshot
        .checked_debug_expression(&path, stop, "shadow + marker")
        .expect("debug expression checks against the stop scope");

    struct Evaluator {
        stop_line: u32,
        expression: CheckedDebugExpression,
        values: Vec<DebugValue>,
    }

    impl StepHook for Evaluator {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == self.stop_line {
                self.values
                    .push(frame.evaluate_debug_expression(&self.expression)?);
            }
            Ok(())
        }
    }

    let store = TreeStore::memory();
    let mut hook = Evaluator {
        stop_line: stop.line,
        expression,
        values: Vec::new(),
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::inspect", Value::Int(10)),
    )
    .expect("debugged run completes");

    assert_eq!(hook.values.len(), 1);
    assert_eq!(hook.values[0].preview(), "36");
}

#[test]
fn frame_rejects_checked_debug_expression_from_a_different_source_context() {
    let (first_snapshot, _first_program, first_path, first_source) = analyzed_runtime_program(
        "pub fn inspect(): int\n\
         \x20   const n = 1\n\
         \x20   return n\n",
    );
    let first_stop = stop_span(&first_source, "return n");
    let expression = first_snapshot
        .checked_debug_expression(&first_path, first_stop, "n")
        .expect("debug expression checks in the first source context");

    let (_second_snapshot, second_program, _second_path, second_source) = analyzed_runtime_program(
        "pub fn inspect(): int\n\
         \x20   const n = 2\n\
         \x20   return n\n",
    );
    let second_stop = stop_span(&second_source, "return n");

    struct RejectionProbe {
        stop_line: u32,
        expression: CheckedDebugExpression,
        error_code: Option<&'static str>,
    }

    impl StepHook for RejectionProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == self.stop_line {
                let error = frame
                    .evaluate_debug_expression(&self.expression)
                    .expect_err("stale debug expression is rejected");
                self.error_code = Some(error.code());
            }
            Ok(())
        }
    }

    let store = TreeStore::memory();
    let mut hook = RejectionProbe {
        stop_line: second_stop.line,
        expression,
        error_code: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&second_program, "test::inspect"),
    )
    .expect("debugged run completes after rejecting the expression");

    assert_eq!(hook.error_code, Some(marrow_run::RUN_UNSUPPORTED));
}

#[test]
fn frame_rejects_debug_expression_when_only_the_function_body_changed() {
    let root = TempDir::new("marrow-run-debug-expression-same-path").expect("create project");
    let (first_snapshot, _first_program, first_path, first_source) = analyzed_runtime_program_at(
        root.path(),
        "pub fn inspect(): int\n\
             \x20   const n = 1\n\
             \x20   return n\n",
    );
    let stop = stop_span(&first_source, "return n");
    let expression = first_snapshot
        .checked_debug_expression(&first_path, stop, "n")
        .expect("debug expression checks in the first body");

    let (_second_snapshot, second_program, _second_path, second_source) =
        analyzed_runtime_program_at(
            root.path(),
            "pub fn inspect(): int\n\
             \x20   const n = 2\n\
             \x20   return n\n",
        );
    let second_stop = stop_span(&second_source, "return n");

    struct StaleBodyProbe {
        stop_line: u32,
        expression: CheckedDebugExpression,
        error_code: Option<&'static str>,
    }

    impl StepHook for StaleBodyProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == self.stop_line {
                let error = frame
                    .evaluate_debug_expression(&self.expression)
                    .expect_err("body-only stale debug expression is rejected");
                self.error_code = Some(error.code());
            }
            Ok(())
        }
    }

    let store = TreeStore::memory();
    let mut hook = StaleBodyProbe {
        stop_line: second_stop.line,
        expression,
        error_code: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&second_program, "test::inspect"),
    )
    .expect("debugged run completes after rejecting the expression");

    assert_eq!(hook.error_code, Some(marrow_run::RUN_UNSUPPORTED));
}

#[test]
fn frame_keeps_debug_expression_valid_when_only_configured_tests_change() {
    let root = TempDir::new("marrow-run-debug-expression-test-identity").expect("create project");
    let source = "pub fn inspect(): int\n\
         \x20   const n = 1\n\
         \x20   return n\n";
    let first_test = "fn smoke()\n    test::inspect()\n";
    let second_test = "fn smoke()\n    const marker = 2\n    test::inspect()\n";
    let (first_snapshot, _first_program, first_path, first_source) =
        analyzed_runtime_program_with_configured_test_at(root.path(), source, first_test);
    let stop = stop_span(&first_source, "return n");
    let expression = first_snapshot
        .checked_debug_expression(&first_path, stop, "n")
        .expect("debug expression checks against the source program");
    let (_second_snapshot, second_program, _second_path, _second_source) =
        analyzed_runtime_program_with_configured_test_at(root.path(), source, second_test);

    struct TestOnlyEditProbe {
        stop_line: u32,
        expression: CheckedDebugExpression,
        value: Option<DebugValue>,
        error_code: Option<&'static str>,
    }

    impl StepHook for TestOnlyEditProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == self.stop_line {
                match frame.evaluate_debug_expression(&self.expression) {
                    Ok(value) => self.value = Some(value),
                    Err(error) => self.error_code = Some(error.code()),
                }
            }
            Ok(())
        }
    }

    let store = TreeStore::memory();
    let mut hook = TestOnlyEditProbe {
        stop_line: stop.line,
        expression,
        value: None,
        error_code: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&second_program, "test::inspect"),
    )
    .expect("debugged run completes");

    assert_eq!(hook.error_code, None);
    assert_eq!(
        hook.value.as_ref().map(DebugValue::preview),
        Some("1".to_string())
    );
}

#[test]
fn frame_rejects_debug_expression_checked_for_another_stop_in_the_same_file() {
    let (snapshot, program, path, source) = analyzed_runtime_program(
        "pub fn inspect(): int\n\
         \x20   const first = 1\n\
         \x20   const second = 2\n\
         \x20   return first + second\n",
    );
    let checked_stop = stop_span(&source, "const second = 2");
    let evaluated_stop = stop_span(&source, "return first");
    let expression = snapshot
        .checked_debug_expression(&path, checked_stop, "first")
        .expect("debug expression checks at the earlier stop");

    struct WrongStopProbe {
        stop_line: u32,
        expression: CheckedDebugExpression,
        error_code: Option<&'static str>,
    }

    impl StepHook for WrongStopProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == self.stop_line {
                let error = frame
                    .evaluate_debug_expression(&self.expression)
                    .expect_err("debug expression is rejected at a different stop");
                self.error_code = Some(error.code());
            }
            Ok(())
        }
    }

    let store = TreeStore::memory();
    let mut hook = WrongStopProbe {
        stop_line: evaluated_stop.line,
        expression,
        error_code: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::inspect"),
    )
    .expect("debugged run completes after rejecting the expression");

    assert_eq!(hook.error_code, Some(marrow_run::RUN_UNSUPPORTED));
}

#[test]
fn frame_rejects_debug_expression_at_wrong_stop_with_shadowed_live_locals() {
    let (snapshot, program, path, source) = analyzed_runtime_program(
        "pub fn inspect(): int\n\
         \x20   const value: int = 1\n\
         \x20   if true\n\
         \x20       const value: string = \"wrong\"\n\
         \x20       return 0\n\
         \x20   return value\n",
    );
    let checked_stop = stop_span(&source, "if true");
    let evaluated_stop = stop_span(&source, "return 0");
    let expression = snapshot
        .checked_debug_expression(&path, checked_stop, "value")
        .expect("debug expression checks before the branch");

    struct ForgedStopProbe {
        stop_line: u32,
        expression: CheckedDebugExpression,
        error_code: Option<&'static str>,
        value: Option<DebugValue>,
    }

    impl StepHook for ForgedStopProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == self.stop_line {
                match frame.evaluate_debug_expression(&self.expression) {
                    Ok(value) => self.value = Some(value),
                    Err(error) => self.error_code = Some(error.code()),
                }
            }
            Ok(())
        }
    }

    let store = TreeStore::memory();
    let mut hook = ForgedStopProbe {
        stop_line: evaluated_stop.line,
        expression,
        error_code: None,
        value: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::inspect"),
    )
    .expect("debugged run completes after rejecting the expression");

    assert_eq!(hook.error_code, Some(marrow_run::RUN_UNSUPPORTED));
    assert_eq!(hook.value, None);
}

#[test]
fn debug_value_pages_and_filters_use_marrow_labels() {
    let sequence = DebugValue::from_value(Value::Sequence(Sequence::dense(vec![
        Value::Int(10),
        Value::Int(20),
        Value::Int(30),
    ])));
    assert_eq!(sequence.preview(), "sequence[3]");
    assert_eq!(sequence.child_counts().unwrap().indexed, Some(3));
    let first_page = sequence.children(DebugValuePage::new(0, 1), DebugValueFilter::Indexed);
    assert_eq!(first_page.len(), 1);
    assert_eq!(first_page[0].name, "[1]");
    assert_eq!(first_page[0].value.preview(), "10");
    let second_page = sequence.children(DebugValuePage::new(1, 1), DebugValueFilter::Indexed);
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].name, "[2]");
    assert_eq!(second_page[0].value.preview(), "20");
    assert!(
        sequence
            .children(DebugValuePage::new(0, 0), DebugValueFilter::Indexed)
            .is_empty()
    );
    assert!(
        sequence
            .children(DebugValuePage::default(), DebugValueFilter::Named)
            .is_empty()
    );

    let large_sequence = DebugValue::from_value(Value::Sequence(Sequence::dense(
        (0..DEBUG_VALUE_DEFAULT_PAGE_LIMIT + 1)
            .map(|value| Value::Int(value as i64))
            .collect(),
    )));
    let debug_text = format!("{large_sequence:?}");
    assert!(debug_text.contains("sequence[101]"), "{debug_text}");
    assert!(!debug_text.contains("Runtime"), "{debug_text}");
    assert!(!debug_text.contains("Sequence(["), "{debug_text}");
    let default_page =
        large_sequence.children(DebugValuePage::default(), DebugValueFilter::Indexed);
    assert_eq!(default_page.len(), DEBUG_VALUE_DEFAULT_PAGE_LIMIT);
    assert_eq!(
        default_page.last().expect("last default child").name,
        format!("[{DEBUG_VALUE_DEFAULT_PAGE_LIMIT}]")
    );

    let resource = DebugValue::from_value(Value::Resource(vec![
        ("title".into(), Value::Str("Dune".into())),
        ("pages".into(), Value::Int(412)),
    ]));
    assert_eq!(resource.preview(), "resource{title, pages}");
    assert_eq!(resource.child_counts().unwrap().named, Some(2));
    let children = resource.children(DebugValuePage::new(1, 1), DebugValueFilter::Named);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "pages");
    assert_eq!(children[0].value.preview(), "412");

    let program = checked_program(
        "resource Book\n\
         \x20   title: string\nstore ^books(author: string, ordinal: int): Book\n\
         \n\
         pub fn bookId(): Id(^books)\n\
         \x20   return Id(^books, \"Ann\", 2)\n",
    );
    let store = TreeStore::memory();
    let output = run_entry(&store, checked_entry!(&program, "test::bookId"))
        .expect("identity entry runs")
        .value
        .expect("identity value");
    let identity = DebugValue::from_value(output);
    assert_eq!(identity.preview(), "^books(Ann, 2)");
    assert_eq!(identity.child_counts().unwrap().indexed, Some(2));
    let first_key = identity.children(DebugValuePage::new(0, 1), DebugValueFilter::Indexed);
    assert_eq!(first_key.len(), 1);
    assert_eq!(first_key[0].name, "[1]");
    assert_eq!(first_key[0].value.preview(), "Ann");
    let second_key = identity.children(DebugValuePage::new(1, 1), DebugValueFilter::Indexed);
    assert_eq!(second_key.len(), 1);
    assert_eq!(second_key[0].name, "[2]");
    assert_eq!(second_key[0].value.preview(), "2");
}

#[test]
fn debug_value_snapshots_bound_nested_child_materialization() {
    let nested = Value::Sequence(Sequence::dense(
        (0..DEBUG_VALUE_MAX_PAGE_LIMIT + 1)
            .map(|value| Value::Int(value as i64))
            .collect(),
    ));
    let root = DebugValue::from_value(Value::Sequence(Sequence::dense(vec![nested])));
    let children = root.children(DebugValuePage::new(0, 1), DebugValueFilter::Indexed);

    assert_eq!(children.len(), 1);
    assert_eq!(
        children[0].value.preview(),
        format!("sequence[{}]", DEBUG_VALUE_MAX_PAGE_LIMIT + 1)
    );
    assert_eq!(
        children[0].value.child_counts().unwrap().indexed,
        Some(DEBUG_VALUE_MAX_PAGE_LIMIT)
    );
    assert!(children[0].value.children_truncated());
}

#[test]
fn hook_store_handle_sees_prior_writes() {
    // `Frame::store()` is the live handle, so a write made by an earlier statement
    // is visible to the hook when it inspects a later one (read-your-writes).
    let program = checked_program(
        "resource Account\n\
         \x20\x20\x20\x20balance: int\nstore ^accts(id: int): Account\n\
         \n\
         pub fn seed(): int\n\
         \x20\x20\x20\x20^accts(1).balance = 7\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();

    struct StorePeeker<'a> {
        program: &'a CheckedRuntimeProgram,
        balance_seen_at_return: Option<i64>,
    }
    // The synthesized `module test` header shifts the source body down, so the
    // `return 0` statement lands on this line of the checked module. By the time the
    // hook sees it the balance write has run, so the live store must reflect it.
    const RETURN_LINE: u32 = 9;
    impl StepHook for StorePeeker<'_> {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == RETURN_LINE {
                self.balance_seen_at_return = match read_data_value(
                    self.program,
                    frame.store(),
                    "accts",
                    &[SavedKey::Int(1)],
                    &data_path(self.program, "accts", &["balance"]),
                    ScalarType::Int,
                ) {
                    Some(SavedValue::Int(n)) => Some(n),
                    _ => None,
                };
            }
            Ok(())
        }
    }

    let mut hook = StorePeeker {
        program: &program,
        balance_seen_at_return: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");
    assert_eq!(hook.balance_seen_at_return, Some(7));
}

#[test]
fn frame_debug_data_reads_the_paused_run_store() {
    let program = checked_program(
        "resource Account\n\
         \x20\x20\x20\x20balance: int\nstore ^accts(id: int): Account\n\
         \n\
         pub fn seed(): int\n\
         \x20\x20\x20\x20^accts(1).balance = 7\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();

    struct DebugDataProbe<'a> {
        program: &'a CheckedRuntimeProgram,
        snapshot: Option<tooling::DataSnapshotStamp>,
        preview: Option<tooling::StampedData<tooling::DataPreviewReadResult>>,
        children: Option<tooling::StampedData<tooling::DataChildrenPage>>,
    }

    const RETURN_LINE: u32 = 9;
    impl StepHook for DebugDataProbe<'_> {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == RETURN_LINE {
                self.snapshot = Some(frame.debug_data_snapshot().expect("debug data snapshot"));
                let record = [
                    tooling::DataPathSegment::Root("accts".into()),
                    tooling::DataPathSegment::Key(SavedKey::Int(1)),
                ];
                let balance = [
                    tooling::DataPathSegment::Root("accts".into()),
                    tooling::DataPathSegment::Key(SavedKey::Int(1)),
                    tooling::DataPathSegment::Field("balance".into()),
                ];
                self.children = Some(
                    frame
                        .debug_data_children(
                            &[tooling::DataPathSegment::Root("accts".into())],
                            10,
                            None,
                        )
                        .expect("debug data children read"),
                );
                self.preview = frame
                    .debug_data_preview(&balance, 64)
                    .expect("debug data preview read");
                assert_eq!(
                    frame
                        .debug_data_preview(&record, 64)
                        .expect("record preview read")
                        .expect("record path resolves")
                        .data
                        .presence,
                    tooling::DataPresence::ChildrenOnly
                );
            }
            Ok(())
        }
    }

    let mut hook = DebugDataProbe {
        program: &program,
        snapshot: None,
        preview: None,
        children: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");

    let preview = hook.preview.expect("preview captured");
    let snapshot = hook.snapshot.expect("snapshot captured");
    assert_eq!(snapshot.checked_source_digest, hook.program.source_digest());
    assert_eq!(snapshot.store_commit, preview.stamp.store_commit);
    assert_eq!(snapshot.open_transaction, None);
    assert_eq!(
        preview.stamp.checked_source_digest,
        hook.program.source_digest()
    );
    let commit = preview
        .stamp
        .store_commit
        .as_ref()
        .expect("debug data read is stamped with the live run commit");
    assert_eq!(commit.commit_id, 1);
    assert_eq!(commit.source_digest, hook.program.source_digest());
    assert_eq!(preview.stamp.open_transaction, None);
    assert_eq!(preview.data.presence, tooling::DataPresence::ValueOnly);
    let value = preview.data.preview.expect("value preview");
    assert_eq!(value.text, "7");
    assert!(!value.truncated);

    let children = hook.children.expect("children captured");
    assert_eq!(
        children.stamp.checked_source_digest,
        hook.program.source_digest()
    );
    assert_eq!(children.stamp.store_commit, preview.stamp.store_commit);
    assert_eq!(children.stamp.open_transaction, None);
    assert_eq!(
        children.data.children,
        vec![tooling::DataChild::Key(SavedKey::Int(1))]
    );
    assert!(!children.data.truncated);
}

#[test]
fn frame_debug_data_reads_keyed_member_entries() {
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed(): int\n\
         \x20\x20\x20\x20^books(1).title = \"root\"\n\
         \x20\x20\x20\x20^books(1).versions(2).note = \"second\"\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();

    struct KeyedProbe {
        keys: Option<tooling::StampedData<tooling::DataChildrenPage>>,
        preview: Option<tooling::StampedData<tooling::DataPreviewReadResult>>,
    }

    const RETURN_LINE: u32 = 12;
    impl StepHook for KeyedProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == RETURN_LINE {
                let versions = [
                    tooling::DataPathSegment::Root("books".into()),
                    tooling::DataPathSegment::Key(SavedKey::Int(1)),
                    tooling::DataPathSegment::Layer("versions".into()),
                ];
                let note = [
                    tooling::DataPathSegment::Root("books".into()),
                    tooling::DataPathSegment::Key(SavedKey::Int(1)),
                    tooling::DataPathSegment::Layer("versions".into()),
                    tooling::DataPathSegment::Key(SavedKey::Int(2)),
                    tooling::DataPathSegment::Field("note".into()),
                ];
                self.keys = Some(
                    frame
                        .debug_data_children(&versions, 10, None)
                        .expect("keyed member children read"),
                );
                self.preview = frame
                    .debug_data_preview(&note, 64)
                    .expect("keyed member preview read");
            }
            Ok(())
        }
    }

    let mut hook = KeyedProbe {
        keys: None,
        preview: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");

    assert_eq!(
        hook.keys.expect("keys captured").data.children,
        vec![tooling::DataChild::Key(SavedKey::Int(2))]
    );
    let preview = hook
        .preview
        .expect("preview captured")
        .data
        .preview
        .expect("value preview");
    assert_eq!(preview.text, "\"second\"");
    assert!(!preview.truncated);
}

#[test]
fn frame_debug_data_reads_open_transaction_store_with_transaction_stamp() {
    let program = checked_program(
        "resource Account\n\
         \x20\x20\x20\x20balance: int\nstore ^accts(id: int): Account\n\
         \n\
         pub fn init(): int\n\
         \x20\x20\x20\x20^accts(1).balance = 1\n\
         \x20\x20\x20\x20return 0\n\
         \n\
         pub fn seed(): int\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^accts(1).balance = 7\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::init")).expect("initial write commits");

    struct TransactionProbe {
        snapshot: Option<tooling::DataSnapshotStamp>,
        preview: Option<tooling::StampedData<tooling::DataPreviewReadResult>>,
        children: Option<tooling::StampedData<tooling::DataChildrenPage>>,
    }

    const RETURN_LINE: u32 = 14;
    impl StepHook for TransactionProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == RETURN_LINE {
                self.snapshot = Some(
                    frame
                        .debug_data_snapshot()
                        .expect("debug data snapshot inside transaction"),
                );
                let balance = [
                    tooling::DataPathSegment::Root("accts".into()),
                    tooling::DataPathSegment::Key(SavedKey::Int(1)),
                    tooling::DataPathSegment::Field("balance".into()),
                ];
                self.children = Some(
                    frame
                        .debug_data_children(
                            &[tooling::DataPathSegment::Root("accts".into())],
                            10,
                            None,
                        )
                        .expect("debug data children read inside transaction"),
                );
                self.preview = frame
                    .debug_data_preview(&balance, 64)
                    .expect("debug data preview read inside transaction");
            }
            Ok(())
        }
    }

    let mut hook = TransactionProbe {
        snapshot: None,
        preview: None,
        children: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");

    let preview = hook.preview.expect("preview captured");
    let snapshot = hook.snapshot.expect("snapshot captured");
    assert_eq!(snapshot.store_commit, None);
    assert_eq!(
        snapshot.open_transaction.expect("transaction stamp").depth,
        NonZeroUsize::new(1).expect("nonzero depth")
    );
    assert_eq!(preview.data.presence, tooling::DataPresence::ValueOnly);
    assert_eq!(preview.data.preview.expect("value preview").text, "7");
    assert_eq!(preview.stamp.store_commit, None);
    assert_eq!(
        preview
            .stamp
            .open_transaction
            .expect("transaction stamp")
            .depth,
        NonZeroUsize::new(1).expect("nonzero depth")
    );

    let children = hook.children.expect("children captured");
    assert_eq!(
        children.data.children,
        vec![tooling::DataChild::Key(SavedKey::Int(1))]
    );
    assert_eq!(children.stamp.store_commit, None);
    assert_eq!(
        children
            .stamp
            .open_transaction
            .expect("transaction stamp")
            .depth,
        NonZeroUsize::new(1).expect("nonzero depth")
    );
}

#[test]
fn runtime_open_transaction_data_snapshot_requires_open_store_transaction() {
    let program = checked_program(
        "resource Account\n\
         \x20\x20\x20\x20balance: int\nstore ^accts(id: int): Account\n",
    );
    let store = TreeStore::memory();

    let error = tooling::runtime_open_transaction_data_snapshot_stamp(
        &program,
        &store,
        NonZeroUsize::new(1).expect("nonzero depth"),
    )
    .expect_err("open transaction snapshots require an open store transaction");

    assert_eq!(error.code(), "store.transaction");
}

#[test]
fn frame_debug_data_stamps_nested_source_transaction_depth() {
    let program = checked_program(
        "resource Account\n\
         \x20\x20\x20\x20balance: int\nstore ^accts(id: int): Account\n\
         \n\
         pub fn seed(): int\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20^accts(1).balance = 9\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();

    struct NestedProbe {
        preview: Option<tooling::StampedData<tooling::DataPreviewReadResult>>,
        source_depth_at_return: Option<usize>,
        store_depth_at_return: Option<usize>,
    }

    const RETURN_LINE: u32 = 11;
    impl StepHook for NestedProbe {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            if span.line == RETURN_LINE {
                let balance = [
                    tooling::DataPathSegment::Root("accts".into()),
                    tooling::DataPathSegment::Key(SavedKey::Int(1)),
                    tooling::DataPathSegment::Field("balance".into()),
                ];
                self.source_depth_at_return = Some(frame.transaction_depth());
                self.store_depth_at_return = Some(frame.store().transaction_depth());
                self.preview = frame
                    .debug_data_preview(&balance, 64)
                    .expect("nested transaction debug data preview read");
            }
            Ok(())
        }
    }

    let mut hook = NestedProbe {
        preview: None,
        source_depth_at_return: None,
        store_depth_at_return: None,
    };
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");

    assert_eq!(hook.source_depth_at_return, Some(2));
    assert_eq!(hook.store_depth_at_return, Some(1));
    let preview = hook.preview.expect("preview captured");
    assert_eq!(preview.data.preview.expect("value preview").text, "9");
    assert_eq!(
        preview
            .stamp
            .open_transaction
            .expect("transaction stamp")
            .depth,
        NonZeroUsize::new(2).expect("nonzero depth")
    );
}

#[test]
fn hook_error_aborts_the_run() {
    // Returning Err from `before_statement` terminates the run; the error
    // surfaces and later statements never execute.
    let program = checked_program(
        "pub fn compute(): int\n\
         \x20\x20\x20\x20const a = 1\n\
         \x20\x20\x20\x20const b = 2\n\
         \x20\x20\x20\x20return a + b\n",
    );
    let store = TreeStore::memory();
    let mut hook = Recorder {
        steps: Vec::new(),
        abort_at_line: Some(5),
    };
    let error = run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::compute"),
    )
    .expect_err("the hook aborts the run");
    assert_eq!(error.code(), marrow_run::RUN_UNSUPPORTED);
    assert_eq!(error.message, "debugger terminate");
    // Only the statements up to and including the aborting one were offered.
    let lines: Vec<u32> = hook.steps.iter().map(|(line, _, _)| *line).collect();
    assert_eq!(lines, vec![4, 5]);
}

#[test]
fn fatal_hook_error_with_throw_is_not_caught_by_try() {
    const ABORT_LINE: u32 = 5;
    const CATCH_RETURN_LINE: u32 = 7;

    struct FatalThrowHook {
        steps: Vec<u32>,
    }

    impl StepHook for FatalThrowHook {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            _frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            self.steps.push(span.line);
            if span.line == ABORT_LINE {
                return Err(RuntimeError::fatal_with_throw(
                    marrow_run::RUN_UNSUPPORTED,
                    "debugger fatal",
                    span,
                    Value::Resource(vec![
                        (
                            marrow_schema::error::CODE.to_string(),
                            Value::Str("debug.fatal".into()),
                        ),
                        (
                            marrow_schema::error::MESSAGE.to_string(),
                            Value::Str("debugger fatal".into()),
                        ),
                    ]),
                ));
            }
            Ok(())
        }
    }

    let program = checked_program(
        "pub fn compute(): int\n\
         \x20\x20\x20\x20try\n\
         \x20\x20\x20\x20\x20\x20\x20\x20const a = 1\n\
         \x20\x20\x20\x20catch err: Error\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return 99\n\
         \x20\x20\x20\x20return 1\n",
    );
    let store = TreeStore::memory();
    let mut hook = FatalThrowHook { steps: Vec::new() };
    let error = run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::compute"),
    )
    .expect_err("fatal hook error escapes the try");

    assert_eq!(error.code(), marrow_run::RUN_UNSUPPORTED);
    assert_eq!(error.message, "debugger fatal");
    assert!(!error.catchable);
    assert!(error.throw.is_some());
    assert!(!hook.steps.contains(&CATCH_RETURN_LINE));
}

#[test]
fn fatal_uncaught_throw_hook_error_in_callee_is_not_caught_by_caller_try() {
    const INNER_ABORT_LINE: u32 = 4;
    const CATCH_RETURN_LINE: u32 = 11;

    struct FatalUncaughtThrowHook {
        steps: Vec<u32>,
    }

    impl StepHook for FatalUncaughtThrowHook {
        fn before_statement(
            &mut self,
            span: SourceSpan,
            _frame: Frame<'_, '_>,
        ) -> Result<(), RuntimeError> {
            self.steps.push(span.line);
            if span.line == INNER_ABORT_LINE {
                return Err(RuntimeError::fatal_with_throw(
                    marrow_run::RUN_UNCAUGHT_THROW,
                    "debugger fatal throw",
                    span,
                    Value::Resource(vec![
                        (
                            marrow_schema::error::CODE.to_string(),
                            Value::Str("debug.fatal".into()),
                        ),
                        (
                            marrow_schema::error::MESSAGE.to_string(),
                            Value::Str("debugger fatal throw".into()),
                        ),
                    ]),
                ));
            }
            Ok(())
        }
    }

    let program = checked_program(
        "pub fn inner(): int\n\
         \x20\x20\x20\x20const a = 1\n\
         \x20\x20\x20\x20return a\n\
         \n\
         pub fn outer(): int\n\
         \x20\x20\x20\x20try\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return inner()\n\
         \x20\x20\x20\x20catch err: Error\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return 99\n\
         \x20\x20\x20\x20return 1\n",
    );
    let store = TreeStore::memory();
    let mut hook = FatalUncaughtThrowHook { steps: Vec::new() };
    let error = run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::outer"),
    )
    .expect_err("fatal callee hook error escapes the caller try");

    assert_eq!(error.code(), marrow_run::RUN_UNCAUGHT_THROW);
    assert_eq!(error.message, "debugger fatal throw");
    assert!(!error.catchable);
    assert!(error.throw.is_some());
    assert!(!hook.steps.contains(&CATCH_RETURN_LINE));
}

/// A hook recording each managed write it is offered: its operation, the human
/// path, whether a value was present, and the activation depth. Statement events
/// are ignored, so the recorded sequence is exactly the run's managed writes in
/// commit order.
#[derive(Default)]
struct WriteRecorder {
    writes: Vec<(marrow_run::WriteOp, WriteTarget, bool, usize)>,
}

impl StepHook for WriteRecorder {
    fn before_statement(
        &mut self,
        _span: SourceSpan,
        _frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn before_write(
        &mut self,
        op: marrow_run::WriteOp,
        target: &WriteTarget,
        value: Option<&[u8]>,
        depth: usize,
    ) {
        self.writes
            .push((op, target.clone(), value.is_some(), depth));
    }
}

#[test]
fn hook_observes_each_managed_write_in_commit_order() {
    // The field write records the whole-record presence before the field cell.
    // `delete ^books(1)` clears the record's subtree with one `Delete` of the identity path.
    // The hook sees each `PlanStep` as a `before_write` event, in commit order, at
    // the statement's activation depth.
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20title: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn seed(): int\n\
         \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20delete ^books(1)\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();
    let mut hook = WriteRecorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::seed"),
    )
    .expect("debugged run completes");

    assert_eq!(
        hook.writes,
        vec![
            (
                marrow_run::WriteOp::Write,
                WriteTarget::Data {
                    store: store_catalog_id(&program, "books").as_str().to_string(),
                    identity: vec![SavedKey::Int(1)],
                    path: Vec::new(),
                },
                false,
                1
            ),
            (
                marrow_run::WriteOp::Write,
                WriteTarget::Data {
                    store: store_catalog_id(&program, "books").as_str().to_string(),
                    identity: vec![SavedKey::Int(1)],
                    path: write_data_path(data_path(&program, "books", &["title"])),
                },
                true,
                1
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store_catalog_id(&program, "books").as_str().to_string(),
                    identity: vec![SavedKey::Int(1)],
                    path: Vec::new(),
                },
                false,
                1
            ),
        ],
    );
}

#[test]
fn hook_observes_each_managed_delete_shape() {
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20title: string\n\
         \n\
         \x20\x20\x20\x20details\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20meta\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20label: string\n\
         \n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20^books(1).details.note = \"top\"\n\
         \x20\x20\x20\x20^books(2).details.meta.label = \"nested group\"\n\
         \x20\x20\x20\x20^books(3).details.meta.label = \"nested scalar\"\n\
         \x20\x20\x20\x20^books(4).versions(1).note = \"entry\"\n\
         \n\
         pub fn dropAll()\n\
         \x20\x20\x20\x20delete ^books(1).details\n\
         \x20\x20\x20\x20delete ^books(2).details.meta\n\
         \x20\x20\x20\x20delete ^books(3).details.meta.label\n\
         \x20\x20\x20\x20delete ^books(4).versions(1)\n",
    );
    let store = TreeStore::memory();
    run_entry_with_host(&store, &Host::new(), checked_entry!(&program, "test::seed"))
        .expect("seed runs");
    let mut hook = WriteRecorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new(),
        &mut hook,
        checked_entry!(&program, "test::dropAll"),
    )
    .expect("delete run completes");

    let store = store_catalog_id(&program, "books").as_str().to_string();
    assert_eq!(
        hook.writes,
        vec![
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store.clone(),
                    identity: vec![SavedKey::Int(1)],
                    path: write_data_path(data_path(&program, "books", &["details"])),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store.clone(),
                    identity: vec![SavedKey::Int(2)],
                    path: write_data_path(data_path(&program, "books", &["details", "meta"])),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store.clone(),
                    identity: vec![SavedKey::Int(3)],
                    path: write_data_path(data_path(
                        &program,
                        "books",
                        &["details", "meta", "label"],
                    )),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store,
                    identity: vec![SavedKey::Int(4)],
                    path: write_data_path(keyed_data_path(
                        &program,
                        "books",
                        &[("versions", vec![SavedKey::Int(1)])],
                        &[],
                    )),
                },
                false,
                1,
            ),
        ],
    );
}

#[test]
fn hook_observes_maintenance_whole_root_deletes() {
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20shelf: string\nstore ^books(id: int): Book\n\
         \n\
         \x20\x20\x20\x20index byShelf(shelf, id)\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20^books(1).shelf = \"fiction\"\n\
         \n\
         pub fn dropRoot()\n\
         \x20\x20\x20\x20delete ^books\n",
    );
    let store = TreeStore::memory();
    run_entry_with_host(&store, &Host::new(), checked_entry!(&program, "test::seed"))
        .expect("seed runs");
    let mut hook = WriteRecorder::default();
    run_entry_with_debugger(
        &store,
        &Host::new().with_maintenance(),
        &mut hook,
        checked_entry!(&program, "test::dropRoot"),
    )
    .expect("maintenance root drop runs");

    assert_eq!(
        hook.writes,
        vec![
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Data {
                    store: store_catalog_id(&program, "books").as_str().to_string(),
                    identity: Vec::new(),
                    path: Vec::new(),
                },
                false,
                1,
            ),
            (
                marrow_run::WriteOp::Delete,
                WriteTarget::Index {
                    index: index_catalog_id(&program, "books", "byShelf")
                        .as_str()
                        .to_string(),
                    keys: Vec::new(),
                    identity: Vec::new(),
                },
                false,
                1,
            ),
        ],
    );
}

#[test]
fn no_hook_run_pays_no_write_observation() {
    // Regression: a write with no hook installed runs through the non-debugged
    // entry exactly as before. The default `before_write` is never reached, so the
    // managed write still lands and the run completes.
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20title: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn seed(): int\n\
         \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20return 0\n",
    );
    let store = TreeStore::memory();
    run_entry_with_host(&store, &Host::new(), checked_entry!(&program, "test::seed"))
        .expect("run completes");
    let title = match read_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["title"]),
        ScalarType::Str,
    ) {
        Some(SavedValue::Str(s)) => Some(s),
        _ => None,
    };
    assert_eq!(title, Some("Mort".to_string()));
}

#[test]
fn display_debug_renders_scalars_and_structured_previews() {
    // Scalars render like the normal renderer.
    assert_eq!(Value::Int(42).display_debug(), "42");
    assert_eq!(Value::Bool(true).display_debug(), "true");
    assert_eq!(Value::Str("hi".into()).display_debug(), "hi");

    // Bytes render as canonical `0x`-hex so distinct values look distinct; a
    // sequence gets a total, never-faulting structural preview.
    assert_eq!(Value::Bytes(vec![1, 2, 3]).display_debug(), "0x010203");
    assert_eq!(
        Value::Sequence(Sequence::dense(vec![Value::Int(1), Value::Int(2)])).display_debug(),
        "sequence[2]"
    );

    // A resource previews its present field names; an identity previews its rooted keys.
    assert_eq!(
        Value::Resource(vec![
            ("title".into(), Value::Str("v2".into())),
            ("pages".into(), Value::Int(3)),
        ])
        .display_debug(),
        "resource{title, pages}"
    );
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20title: string\nstore ^books(id: int): Book\n\
         \n\
         pub fn nextBookId(): Id(^books)\n\
         \x20\x20\x20\x20return nextId(^books)\n",
    );
    let store = TreeStore::memory();
    let output =
        run_entry(&store, checked_entry!(&program, "test::nextBookId")).expect("next identity");
    assert_eq!(output.value.expect("identity").display_debug(), "^books(1)");
}

#[test]
fn an_ordinary_run_with_host_is_unchanged_by_the_hook_machinery() {
    // Sanity: the same program through the non-debugged entry behaves exactly as
    // before — no hook installed, no behavior change.
    let program = checked_program(
        "pub fn compute(a: int): int\n\
         \x20\x20\x20\x20const b = a + 1\n\
         \x20\x20\x20\x20return b\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry_with_host(
        &store,
        &Host::new(),
        checked_entry!(&program, "test::compute", Value::Int(4)),
    )
    .expect("run completes");
    assert_eq!(outcome.value, Some(Value::Int(5)));
}
