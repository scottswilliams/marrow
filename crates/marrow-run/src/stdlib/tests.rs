use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use marrow_check::{CheckedArg as ExecArg, CheckedRuntimeProgram};
use marrow_schema::stdlib::Capability;
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::env::{Context, Env, TransactionState};
use crate::error::RUN_UNSUPPORTED;
use crate::host::Host;
use crate::host_effects::{eval_clock_capability, eval_env, eval_io, eval_log};
use crate::std_pure::eval_std;
use crate::stdlib::eval_assert;
use crate::value::RunOutputSink;

struct NoProgramOutput;

impl RunOutputSink for NoProgramOutput {
    fn write(&mut self, _text: &str) {}
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
        let ctx = Context {
            program: &program,
            store: &store,
            host: &host,
            transaction: Rc::new(RefCell::new(TransactionState::default())),
        };
        let mut env = Env::new(ctx, Rc::new(RefCell::new(NoProgramOutput)), None, None, 1);
        let result = match entry.requires_capability {
            Some(Capability::Clock) => {
                eval_clock_capability(entry.op, no_args, span, &mut env).map(Some)
            }
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
