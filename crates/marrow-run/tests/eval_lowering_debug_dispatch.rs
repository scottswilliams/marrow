//! Saved-path lowering corners, the opt-in statement debugger hook, module-aware
//! call dispatch, enum match / is, cross-module file ids, and typed identity refs.

#[macro_use]
mod support;

use support::*;

use marrow_check::{CheckedRuntimeProgram, FileId};
use marrow_run::{
    CheckedEntryCall, Host, RUN_DIVIDE_BY_ZERO, RUN_UNCAUGHT_THROW, RunOutput, Value, WriteTarget,
};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{TreeStore, decode_tree_enum_member};
use marrow_store::value::{SavedValue, ScalarType, decode_value};
use marrow_syntax::parse_source;
use std::path::PathBuf;

fn run_entry_with_debugger(
    store: &TreeStore,
    host: &Host,
    hook: &mut dyn StepHook,
    call: CheckedEntryCall<'_>,
) -> Result<RunOutput, marrow_run::RuntimeError> {
    marrow_run::run_entry_with_debugger(store, host, hook, &call)
}

// --- Saved-path lowering (the one `lower` pass) ---
//
// These pin the equivalence-risk corners of the unified lowering: the identity
// splice versus raw keys, the keyed-root arity message, the unkeyed-group hop
// versus keyed-layer distinction, and the index-branch terminal classification.

/// A single-key store identity splices in as the whole key, addressing the same
/// record a bare int key does.
#[test]
fn an_identity_argument_splices_in_as_the_record_key() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn save()\n    const id = nextId(^books)\n    ^books(id).title = \"a\"\n\n\
         pub fn read(): string\n    return ^books(1).title\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Str("a".into()))
    );
}

#[test]
fn a_wrong_typed_key_faults_at_lowering_and_does_not_write() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn save(bad: string)\n    ^books(bad).title = \"a\"\n",
        "check.key_type",
    );
}

/// A single-key identity from a string-keyed store cannot be spliced into an
/// int-keyed root; lowering rejects the scalar mismatch before writing.
#[test]
fn a_wrong_scalar_spliced_identity_faults_and_does_not_write() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         resource Magazine at ^magazines(issn: string)\n    required title: string\n\n\
         pub fn seed()\n    ^magazines(\"issn\").title = \"m\"\n\n\
         pub fn save()\n    for id in ^magazines\n        ^books(id).title = \"a\"\n",
        "check.key_type",
    );
}

/// A single-key identity produced from the target store still writes through the
/// saved path lowering.
#[test]
fn a_single_key_store_identity_splice_still_writes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn seed()\n    ^books(7).title = \"seed\"\n\n\
         pub fn save()\n    for id in ^books\n        ^books(id).title = \"a\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::save"))
        .expect("store identity splice writes");
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(7)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("a".into()))
    );
}

/// A composite identity cannot be one component among raw keys: `^pairs(id, 5)`
/// mixing the spliced identity with a trailing raw key is rejected as unsupported.
#[test]
fn an_identity_mixed_with_a_raw_key_is_rejected() {
    checker_rejects(
        "resource Pair at ^pairs(a: int, b: int)\n    required title: string\n\n\
         pub fn seed()\n    ^pairs(7, 8).title = \"seed\"\n\n\
         pub fn save()\n    for id in ^pairs\n        ^pairs(id, 5).title = \"a\"\n",
        "check.key_type",
    );
}

/// Addressing a keyed root without an identity is a type error naming the
/// expected key count, not a silent read of the keyless path.
#[test]
fn a_keyed_root_without_an_identity_is_a_type_error() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn read(): string\n    return ^books.title\n",
        "check.untyped_value",
    );
}

/// An unkeyed group hop (`^patients(id).name.first`) lowers `name` as a zero-key
/// group layer, landing the field under a `ChildLayer`, not as a top-level field.
#[test]
fn an_unkeyed_group_hop_lowers_to_a_child_layer() {
    let program = checked_program(
        "resource Patient at ^patients(id: int)\n    mrn: string\n    name\n        first: string\n\n\
         pub fn save()\n    ^patients(1).name.first = \"Sam\"\n\n\
         pub fn read(): string\n    return ^patients(1)?.name?.first ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Str("Sam".into()))
    );
    // The field landed under the `name` group layer, not beside `mrn`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "patients",
            &[SavedKey::Int(1)],
            &data_path(&program, "patients", &["name", "first"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Sam".into()))
    );
}

// ---------------------------------------------------------------------------
// Opt-in statement debugger hook (`StepHook` / `Frame` / `run_entry_with_debugger`).
// ---------------------------------------------------------------------------

use marrow_run::{Frame, RuntimeError, StepHook};
use marrow_syntax::SourceSpan;

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
        "resource Account at ^accts(id: int)\n\
         \x20\x20\x20\x20balance: int\n\
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
    const RETURN_LINE: u32 = 8;
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
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
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
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \n\
         \x20\x20\x20\x20details\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20meta\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20label: string\n\
         \n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
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
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20shelf: string\n\
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
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
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
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20title: string\n\
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

// Module-aware, visibility-aware runtime call dispatch.

/// Build a checked program from `(module_name, source)` pairs, one source file
/// per pair, so module-aware dispatch and file ids use the production checker
/// artifact.
fn multi_module_program(modules: &[(&str, &str)]) -> CheckedRuntimeProgram {
    let files: Vec<(PathBuf, String)> = modules
        .iter()
        .map(|(name, source)| {
            let path = module_source_path(name);
            let text = format!("module {name}\n\n{source}");
            let parsed = parse_source(&text);
            assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
            (path, text)
        })
        .collect();
    checked_program_files(&files)
}

#[test]
fn bare_call_resolves_in_own_module_not_a_foreign_one() {
    // Two modules each declare `fn greet` returning a distinct value. `zzz::run`
    // calls a bare `greet()`. A bare name resolves in its own module first, so it
    // must run `zzz::greet` (2), never the foreign `aaa::greet` (1).
    let program = multi_module_program(&[
        ("aaa", "pub fn greet(): int\n    return 1\n"),
        (
            "zzz",
            "fn greet(): int\n    return 2\npub fn run(): int\n    return greet()\n",
        ),
    ]);
    assert_eq!(
        run(checked_entry!(&program, "zzz::run")),
        Ok(Some(Value::Int(2))),
        "a bare call must run the calling module's own function"
    );
}

#[test]
fn cross_module_call_to_a_private_fn_is_a_visibility_error() {
    checker_rejects_sources(
        &[
            "module aaa\nfn secret(): int\n    return 1\n",
            "module zzz\nfn run(): int\n    return aaa::secret()\n",
        ],
        "check.private_function",
    );
}

/// An index branch is not an assignable place: `inout ^books.byShelf(s)` is
/// rejected, the same unsupported-path classification the lowering gives it.
#[test]
fn an_index_branch_is_not_an_assignable_place() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    required title: string\n    shelf: string\n\n    index byShelf(shelf, id)\n\n\
         pub fn give(inout s: string)\n    s = \"x\"\n\n\
         pub fn run_it()\n    give(inout ^books.byShelf(\"a\"))\n",
        "check.untyped_value",
    );
}

/// An enum-typed field round-trips through saved data and reads back to the same
/// member. The test observes the language behavior instead of the storage codec.
#[test]
fn an_enum_field_round_trips_through_saved_data() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         pub fn seed(id: int)\n    ^orders(id).state = Status::archived\n\n\
         pub fn matches_archived(id: int): bool\n    return (^orders(id).state ?? Status::active) == Status::archived\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(7)),
    )
    .expect("seed");
    let bytes = read_data_bytes(
        &program,
        &store,
        "orders",
        &[SavedKey::Int(7)],
        &data_path(&program, "orders", &["state"]),
    )
    .expect("enum leaf is written");
    assert!(
        decode_value(&bytes, ScalarType::Int).is_none(),
        "enum leaves must not use the old scalar ordinal codec"
    );
    let stored = decode_tree_enum_member(&bytes).expect("enum leaf uses tree enum member codec");
    assert_eq!(stored.enum_id(), &enum_catalog_id(&program, "Status"));
    assert_eq!(
        stored.member_id(),
        &enum_member_catalog_id(&program, "Status", "archived")
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::matches_archived", Value::Int(7))
        )
        .expect("compare")
        .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn an_entry_enum_argument_uses_checked_catalog_identity() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         pub fn give(): Status\n    return Status::active\n\n\
         pub fn save(state: Status)\n    ^orders(1).state = state\n\n\
         pub fn read(): bool\n    return (^orders(1).state ?? Status::archived) == Status::active\n",
    );
    let store = TreeStore::memory();
    let state = run_entry(&store, checked_entry!(&program, "test::give"))
        .expect("give")
        .value
        .expect("enum value");

    run_entry(&store, checked_entry!(&program, "test::save", state)).expect("save enum argument");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn an_enum_index_uses_catalog_member_keys() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n    index byState(state, id)\n\n\
         pub fn seed()\n    ^orders(1).state = Status::archived\n    ^orders(2).state = Status::active\n    ^orders(3).state = Status::archived\n\n\
         pub fn countArchived(): int\n    var count = 0\n    for id in ^orders.byState(Status::archived)\n        count = count + 1\n    return count\n\n\
         pub fn countActive(): int\n    var count = 0\n    for id in ^orders.byState(Status::active)\n        count = count + 1\n    return count\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed enum-index fixture");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countArchived"))
            .expect("count archived")
            .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countActive"))
            .expect("count active")
            .value,
        Some(Value::Int(1))
    );
}

#[test]
fn a_singleton_keyed_enum_leaf_can_be_matched_after_read() {
    let program = checked_program(
        "enum Kind\n    number\n    plus\n\n\
         resource Session at ^session\n    required cursor: int\n    kinds(pos: int): Kind\n\n\
         pub fn readBack(): int\n    \
         ^session.cursor = 1\n    \
         ^session.kinds(1) = Kind::plus\n    \
         match ^session.kinds(1) ?? Kind::number\n        number\n            return 0\n        plus\n            return 1\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::readBack"))
            .expect("keyed enum leaf match runs")
            .value,
        Some(Value::Int(1))
    );
}

/// Nominal `==` on enum values: comparing the same member is true, comparing two
/// different members of the same enum is false.
#[test]
fn enum_equality_is_true_for_the_same_member_and_false_otherwise() {
    let program = checked_program(
        "enum Color\n    red\n    green\n    blue\n\n\
         pub fn same(): bool\n    return Color::green == Color::green\n\n\
         pub fn different(): bool\n    return Color::green == Color::blue\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `match` dispatches to the arm for the scrutinee's member. Each arm returns a
/// distinct marker, so the returned value names the chosen arm.
#[test]
fn match_dispatches_to_the_arm_for_the_scrutinees_member() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         pub fn label(s: Status): int\n    \
         match s\n        active\n            return 10\n        \
         archived\n            return 20\n        banned\n            return 30\n\n\
         pub fn labelActive(): int\n    return label(Status::active)\n\n\
         pub fn labelArchived(): int\n    return label(Status::archived)\n\n\
         pub fn labelBanned(): int\n    return label(Status::banned)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelActive")).unwrap(),
        Some(Value::Int(10))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelArchived")).unwrap(),
        Some(Value::Int(20))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelBanned")).unwrap(),
        Some(Value::Int(30))
    );
}

/// `match` dispatches by the scrutinee's resolved enum, not by the arm member
/// set. Two enums share member names `x` and `y` but in opposite declaration
/// order, so dispatching by the arm set alone would pick `A` and invert the
/// result for `B`.
#[test]
fn match_dispatches_by_the_scrutinees_enum_not_the_arm_set() {
    let program = checked_program(
        "enum A\n    x\n    y\n\n\
         enum B\n    y\n    x\n\n\
         pub fn label(s: B): int\n    \
         match s\n        x\n            return 1\n        y\n            return 2\n\n\
         pub fn labelX(): int\n    return label(B::x)\n\n\
         pub fn labelY(): int\n    return label(B::y)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelX")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelY")).unwrap(),
        Some(Value::Int(2))
    );
}

/// Nested enum member paths resolve to distinct concrete members.
#[test]
fn a_nested_member_path_resolves_to_the_right_member() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn bengalMatches(): bool\n    return Cat::bengal == Cat::tiger::bengal\n\n\
         pub fn bengalIsHousecat(): bool\n    return Cat::bengal == Cat::housecat\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalMatches")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsHousecat")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `pet is Cat::tiger` is true for any value at or under `tiger` (a `bengal`),
/// false for one outside it (a `housecat`).
#[test]
fn is_tests_subtree_membership() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn bengalIsTiger(): bool\n    return Cat::bengal is Cat::tiger\n\n\
         pub fn housecatIsTiger(): bool\n    return Cat::housecat is Cat::tiger\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsTiger")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::housecatIsTiger")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `is` against a concrete leaf is exact equality: a `bengal` value is
/// `Cat::bengal` but not `Cat::siberian`.
#[test]
fn is_against_a_leaf_is_exact() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn bengalIsBengal(): bool\n    return Cat::bengal is Cat::bengal\n\n\
         pub fn siberianIsBengal(): bool\n    return Cat::siberian is Cat::bengal\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsBengal")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::siberianIsBengal")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// A `match` runs the category arm for any descendant: a `bengal` or `siberian`
/// value both take the `tiger` arm, a `housecat` takes its own.
#[test]
fn match_runs_the_category_arm_for_any_descendant() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn label(pet: Cat): int\n    \
         match pet\n        tiger\n            return 1\n        \
         housecat\n            return 2\n\n\
         pub fn labelBengal(): int\n    return label(Cat::bengal)\n\n\
         pub fn labelSiberian(): int\n    return label(Cat::siberian)\n\n\
         pub fn labelHousecat(): int\n    return label(Cat::housecat)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelBengal")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelSiberian")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelHousecat")).unwrap(),
        Some(Value::Int(2))
    );
}

/// Two `paw`s under different parents are distinct members: the full member paths
/// `Cat::tiger::paw` and `Cat::lion::paw` are not aliases.
#[test]
fn a_duplicated_member_resolves_by_its_full_path_to_a_distinct_member() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         pub fn sameTigerPaw(): bool\n    return Cat::tiger::paw == Cat::tiger::paw\n\n\
         pub fn differentPaws(): bool\n    return Cat::tiger::paw == Cat::lion::paw\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::sameTigerPaw")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::differentPaws")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// A `match` with qualified arms over duplicated leaves dispatches each value to
/// the correct arm by walking the arm path against the scrutinee enum — the two
/// `paw`s take different arms even though they share a name.
#[test]
fn match_with_qualified_arms_dispatches_each_duplicated_paw_to_its_own_arm() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         pub fn label(pet: Cat): int\n    \
         match pet\n        \
         tiger::bengal\n            return 1\n        \
         tiger::paw\n            return 2\n        \
         lion::paw\n            return 3\n        \
         lion::mane\n            return 4\n\n\
         pub fn labelTigerBengal(): int\n    return label(Cat::tiger::bengal)\n\n\
         pub fn labelTigerPaw(): int\n    return label(Cat::tiger::paw)\n\n\
         pub fn labelLionPaw(): int\n    return label(Cat::lion::paw)\n\n\
         pub fn labelLionMane(): int\n    return label(Cat::lion::mane)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelTigerPaw")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelLionPaw")).unwrap(),
        Some(Value::Int(3))
    );
    // The other leaves still dispatch to their arms.
    assert_eq!(
        run(checked_entry!(&program, "test::labelTigerBengal")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelLionMane")).unwrap(),
        Some(Value::Int(4))
    );
}

/// `is` with a full member path is exact over the right leaf, and a category right
/// operand is the subtree test — the same `is_descendant` walk, now reachable for
/// a duplicated leaf via its qualifying path.
#[test]
fn is_with_a_full_path_to_a_duplicated_leaf_is_exact() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         pub fn tigerPawIsTigerPaw(): bool\n    return Cat::tiger::paw is Cat::tiger::paw\n\n\
         pub fn lionPawIsTigerPaw(): bool\n    return Cat::lion::paw is Cat::tiger::paw\n\n\
         pub fn tigerPawIsTiger(): bool\n    return Cat::tiger::paw is Cat::tiger\n\n\
         pub fn lionPawIsTiger(): bool\n    return Cat::lion::paw is Cat::tiger\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::tigerPawIsTigerPaw")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lionPawIsTigerPaw")).unwrap(),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::tigerPawIsTiger")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lionPawIsTiger")).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn uncaught_fault_in_entry_module_carries_its_file_id() {
    // A divide-by-zero raised in the entry module's own body stamps that
    // module's file id (index 0), so a renderer can name the file it lives in.
    let program = multi_module_program(&[(
        "a",
        "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
    )]);
    let error = run_expecting_error(checked_entry!(&program, "a::boom"));
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(0)));
    assert!(
        program
            .file_path(FileId(0))
            .is_some_and(|path| path.ends_with("src/a.mw"))
    );
}

#[test]
fn uncaught_fault_in_cross_module_callee_carries_the_callee_file_id() {
    // The entry `a::run` calls `b::boom`, which divides by zero. The fault is
    // uncaught, so its origin must be `b`'s file (index 1), the frame that
    // raised it, not the entry's `a`.
    let program = multi_module_program(&[
        ("a", "pub fn run(): int\n    return b::boom()\n"),
        (
            "b",
            "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
        ),
    ]);
    let error = run_expecting_error(checked_entry!(&program, "a::run"));
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(1)));
    assert!(
        program
            .file_path(FileId(1))
            .is_some_and(|path| path.ends_with("src/b.mw"))
    );
}

#[test]
fn uncaught_throw_from_cross_module_callee_carries_the_raising_frame_file_id() {
    // A language `throw` is catchable, so it re-spans at each call boundary on its
    // way out — the same path catchable faults take. Thrown in callee `b` and
    // never caught, its origin must stay `b`'s file (index 1), the frame that
    // first raised it, not the entry `a` it surfaces through.
    let program = multi_module_program(&[
        ("a", "pub fn run(): int\n    return b::boom()\n"),
        (
            "b",
            "pub fn boom(): int\n    throw Error(code: \"x.y\", message: \"bad\")\n",
        ),
    ]);
    let error = run_expecting_error(checked_entry!(&program, "a::run"));
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    assert_eq!(error.origin, Some(FileId(1)));
}

#[test]
fn checked_program_entry_fault_carries_origin() {
    let error = eval_source(
        "pub fn f(): int\n    const boom = 1 / 0\n    return 0\n",
        "f",
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(0)));
}

#[test]
fn outer_frame_does_not_overwrite_inner_origin() {
    // `a::outer` calls `a::mid` calls `b::boom`. The uncaught fault crosses
    // three frames in two modules; the deepest (`b`, index 1) wins and the outer
    // `a` frames must not overwrite it.
    let program = multi_module_program(&[
        (
            "a",
            "pub fn outer(): int\n    return mid()\n\n\
             fn mid(): int\n    return b::boom()\n",
        ),
        (
            "b",
            "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
        ),
    ]);
    let error = run_expecting_error(checked_entry!(&program, "a::outer"));
    assert_eq!(error.code, RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(1)));
}

/// A program with an `Author` resource and a `Book` whose `authorId` is a typed
/// reference to `Author`. `seed` writes a reference; `read` reads it back.
fn typed_ref_program() -> CheckedRuntimeProgram {
    checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n\
         \n\
         pub fn read(): bool\n\
         \x20   for author in keys(^authors)\n\
         \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
         \x20       return stored == author\n\
         \x20   return false\n",
    )
}

#[test]
fn an_identity_field_round_trips_through_saved_data() {
    // A `Book.authorId: Id(^authors)` field stores an identity and reads it back as
    // the same identity value produced by the author store.
    let program = typed_ref_program();
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn a_stored_identity_field_reads_back_the_identity_value() {
    // The stored leaf carries the referenced identity's key segments, not a plain
    // scalar field encoding.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    // The stored leaf is the canonical identity encoding — the same
    // order-preserving key bytes a unique index entry stores — not a scalar
    // `encode_value`.
    let stored = read_data_bytes(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["authorId"]),
    );
    assert_eq!(
        stored,
        Some(encode_identity_payload(&[SavedKey::Int(1)])),
        "identity field stored as its canonical key encoding"
    );
}

#[test]
fn an_identity_field_assigned_via_next_id_round_trips() {
    // Constructing the reference from `nextId(^authors)` (the first allocated id is
    // `1` on an empty root) round-trips through the saved identity field.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const a = nextId(^authors)\n\
         \x20   ^authors(a).name = \"Ada\"\n\
         \x20   ^books(1).authorId = a\n\
         \n\
         pub fn read(): bool\n\
         \x20   for author in keys(^authors)\n\
         \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
         \x20       return stored == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn a_self_referencing_identity_field_round_trips() {
    // A field of the same resource (`managerId: Id(^people)` on `Person`) is a valid
    // self-reference that stores and reads back like any other typed reference.
    let program = checked_program(
        "resource Person at ^people(id: int)\n\
         \x20   managerId: Id(^people)\n\
         \n\
         pub fn seed(): bool\n\
         \x20   const person = nextId(^people)\n\
         \x20   ^people(person).managerId = person\n\
         \x20   const manager = nextId(^people)\n\
         \x20   ^people(person).managerId = manager\n\
         \x20   return read(manager)\n\
         \n\
         pub fn read(expected: Id(^people)): bool\n\
         \x20   const stored: Id(^people) = ^people(1).managerId ?? expected\n\
         \x20   return stored == expected\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::seed"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn equality_on_two_identities_of_the_same_store_evaluates() {
    // `==` on two identities of the same store is value equality of their keys:
    // equal keys are `true`, differing keys are `false`.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         pub fn same(): bool\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   return author == author\n\
         \n\
         pub fn different(): bool\n\
         \x20   const ada = nextId(^authors)\n\
         \x20   ^authors(ada).name = \"Ada\"\n\
         \x20   const grace = nextId(^authors)\n\
         \x20   ^authors(grace).name = \"Grace\"\n\
         \x20   return ada == grace\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn single_key_store_identity_behaves_like_other_identity_origins() {
    let program = checked_program(
        "resource Doc at ^docs(id: int)\n\
         \x20   title: string\n\
         \n\
         pub fn idValue(): Id(^docs)\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return doc\n\
         \x20   return id\n\
         \n\
         pub fn mixedEq(): bool\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return id == doc\n\
         \x20   return false\n",
    );
    assert_identity_value(
        run(checked_entry!(&program, "test::idValue")).unwrap(),
        "docs",
        &[SavedKey::Int(1)],
    );
    assert_eq!(
        run(checked_entry!(&program, "test::mixedEq")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn unique_index_identity_compares_with_the_allocated_identity() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required isbn: string\n\
         \x20   index byIsbn(isbn) unique\n\
         \n\
         pub fn seed(): bool\n\
         \x20   var b: Book\n\
         \x20   b.title = \"T\"\n\
         \x20   b.isbn = \"I-1\"\n\
         \x20   const id = nextId(^books)\n\
         \x20   ^books(id) = b\n\
         \x20   const found: Id(^books) = ^books.byIsbn(\"I-1\") ?? id\n\
         \x20   return id == found\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seed")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn a_whole_resource_write_with_an_identity_field_round_trips() {
    // A whole-record write `^books(1) = b` carrying an identity-typed field stores
    // the reference, and a whole-record read reads it back.
    let program = checked_program(
        "resource Author at ^authors(id: int)\n\
         \x20   name: string\n\
         \n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   var b: Book\n\
         \x20   b.title = \"Mort\"\n\
         \x20   b.authorId = author\n\
         \x20   ^books(1) = b\n\
         \n\
         pub fn read(): bool\n\
         \x20   if exists(^books(1))\n\
         \x20       const b = ^books(1)\n\
         \x20       for author in keys(^authors)\n\
         \x20           return b.authorId == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}
