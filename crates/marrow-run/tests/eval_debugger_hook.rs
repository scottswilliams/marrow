//! The opt-in statement debugger hook (`StepHook` / `Frame` /
//! `run_entry_with_debugger`): statement and managed-write observation, depth,
//! live store handle, terminate-by-Err, and debug value previews.

#[macro_use]
mod support;

use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{
    CheckedEntryCall, Frame, Host, RunOutput, RuntimeError, StepHook, Value, WriteTarget,
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
            return Err(RuntimeError {
                code: marrow_run::RUN_UNSUPPORTED,
                message: "debugger terminate".into(),
                span,
                throw: None,
                origin: None,
            });
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
    assert_eq!(error.code, marrow_run::RUN_UNSUPPORTED);
    assert_eq!(error.message, "debugger terminate");
    // Only the statements up to and including the aborting one were offered.
    let lines: Vec<u32> = hook.steps.iter().map(|(line, _, _)| *line).collect();
    assert_eq!(lines, vec![4, 5]);
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
    // A field write, then a whole-record delete: the field write stages one
    // `Write` (the field). `delete ^books(1)` clears the record's subtree with one
    // `Delete` of the identity path. The hook sees each `PlanStep` as a
    // `before_write` event, in commit order, at the statement's activation depth.
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

    // Bytes and a sequence get a total, never-faulting preview (the normal
    // renderer refuses these).
    assert_eq!(Value::Bytes(vec![1, 2, 3]).display_debug(), "bytes[3]");
    assert_eq!(
        Value::Sequence(vec![Value::Int(1), Value::Int(2)]).display_debug(),
        "sequence[2]"
    );

    // A resource previews its present field names; an identity previews its keys.
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
    assert_eq!(
        output.value.expect("identity").display_debug(),
        "identity(1)"
    );
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
