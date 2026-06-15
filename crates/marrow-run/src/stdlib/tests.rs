use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use marrow_check::{
    CheckedArg as ExecArg, CheckedCallTarget, CheckedExpr, CheckedLiteralKind,
    CheckedRuntimeProgram, CheckedStdCall,
};
use marrow_schema::ReturnPresence;
use marrow_schema::stdlib::Capability;
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::env::{Context, Env, TransactionState};
use crate::error::{RUN_UNSUPPORTED, RuntimeError};
use crate::host::{Host, RunContext};
use crate::host_effects::{eval_clock_capability, eval_context, eval_env, eval_io, eval_log};
use crate::std_pure::eval_std;
use crate::stdlib::eval_assert;
use crate::value::RunOutputSink;

struct NoProgramOutput;

impl RunOutputSink for NoProgramOutput {
    fn write(&mut self, _text: &str) {}
}

fn test_env<'a>(
    program: &'a CheckedRuntimeProgram,
    store: &'a TreeStore,
    host: &'a Host,
) -> Env<'a> {
    let ctx = Context {
        program,
        store,
        host,
        transaction: Rc::new(RefCell::new(TransactionState::default())),
    };
    Env::new(ctx, Rc::new(RefCell::new(NoProgramOutput)), None, None, 1)
}

fn string_expr(text: &str) -> CheckedExpr {
    CheckedExpr::Literal {
        kind: CheckedLiteralKind::String,
        text: format!("{text:?}"),
        span: SourceSpan::default(),
    }
}

fn string_arg(text: &str) -> ExecArg {
    ExecArg {
        name: None,
        value: string_expr(text),
    }
}

fn std_call_arg(
    module: &'static str,
    op: &'static str,
    args: Vec<ExecArg>,
    capability: Capability,
) -> ExecArg {
    let span = SourceSpan::default();
    ExecArg {
        name: None,
        value: CheckedExpr::Call {
            callee: Box::new(CheckedExpr::Name {
                segments: vec!["std".into(), module.into(), op.into()],
                enum_member: None,
                span,
            }),
            args,
            target: CheckedCallTarget::Std(CheckedStdCall {
                module,
                op,
                presence: ReturnPresence::Always,
                requires_capability: Some(capability),
            }),
            write_fallibility: None,
            place: None,
            span,
        },
    }
}

fn assert_unsupported<T>(result: Result<T, RuntimeError>) {
    assert_eq!(result.err().map(|error| error.code), Some(RUN_UNSUPPORTED));
}

fn assert_unknown_host_ops(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    host: &Host,
    args: &[ExecArg],
) {
    let span = SourceSpan::default();

    let mut env = test_env(program, store, host);
    assert_unsupported(eval_clock_capability("missing", args, span, &mut env));
    let mut env = test_env(program, store, host);
    assert_unsupported(eval_env("missing", args, span, &mut env));
    let mut env = test_env(program, store, host);
    assert_unsupported(eval_context("missing", args, span, &mut env));
    let mut env = test_env(program, store, host);
    assert_unsupported(eval_log("missing", args, span, &mut env));
    let mut env = test_env(program, store, host);
    assert_unsupported(eval_io("missing", args, span, &mut env));
}

#[test]
fn every_table_row_reaches_a_live_handler() {
    let program = CheckedRuntimeProgram::default();
    let store = TreeStore::memory();
    let host = Host::new()
        .with_clock(0)
        .with_environment(HashMap::new())
        .with_log_sink(Rc::new(RefCell::new(String::new())))
        .with_filesystem();
    let span = SourceSpan::default();
    let no_args: &[ExecArg] = &[];

    for entry in marrow_schema::stdlib::all() {
        let mut env = test_env(&program, &store, &host);
        let result = match entry.requires_capability {
            Some(Capability::Clock) => {
                eval_clock_capability(entry.op, no_args, span, &mut env).map(Some)
            }
            Some(Capability::Context) => eval_context(entry.op, no_args, span, &mut env).map(Some),
            Some(Capability::Environment) => eval_env(entry.op, no_args, span, &mut env).map(Some),
            Some(Capability::Log) => eval_log(entry.op, no_args, span, &mut env),
            Some(Capability::Filesystem) => eval_io(entry.op, no_args, span, &mut env),
            Some(Capability::Maintenance) => {
                unreachable!("the stdlib table has no maintenance helper")
            }
            None if entry.module == "assert" => eval_assert(entry.op, no_args, span, &mut env),
            None => eval_std(entry.module, entry.op, no_args, span, &mut env).map(Some),
        };
        if let Err(error) = result {
            assert_ne!(
                error.code, RUN_UNSUPPORTED,
                "std::{}::{} has a descriptor row but no runtime handler",
                entry.module, entry.op
            );
        }
    }
}

#[test]
fn host_capability_handlers_reject_unknown_ops() {
    let program = CheckedRuntimeProgram::default();
    let store = TreeStore::memory();
    let log = Rc::new(RefCell::new(String::new()));
    let host = Host::new()
        .with_clock(0)
        .with_environment(HashMap::new())
        .with_run_context(RunContext::new())
        .with_log_sink(Rc::clone(&log))
        .with_filesystem();
    let no_capability_host = Host::new();
    let span = SourceSpan::default();
    let no_args: &[ExecArg] = &[];

    assert_unknown_host_ops(&program, &store, &host, no_args);
    assert_eq!(log.borrow().as_str(), "");

    let stray_args = vec![string_arg("unused")];
    assert_unknown_host_ops(&program, &store, &host, &stray_args);
    assert_unknown_host_ops(&program, &store, &no_capability_host, no_args);

    let log_arg = vec![std_call_arg(
        "log",
        "info",
        vec![string_arg("should not run")],
        Capability::Log,
    )];
    let mut env = test_env(&program, &store, &host);
    assert_unsupported(eval_log("missing", &log_arg, span, &mut env));
    assert_eq!(log.borrow().as_str(), "");

    let path = std::env::temp_dir().join(format!("marrow-unknown-io-arg-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let path_text = path.to_string_lossy().into_owned();
    let io_arg = vec![std_call_arg(
        "io",
        "writeText",
        vec![string_arg(&path_text), string_arg("should not run")],
        Capability::Filesystem,
    )];
    let mut env = test_env(&program, &store, &host);
    assert_unsupported(eval_io("missing", &io_arg, span, &mut env));
    assert!(!path.exists());
    let _ = std::fs::remove_file(path);
}
