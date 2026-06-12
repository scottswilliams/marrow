//! The dry-run hook and report for `run --dry-run`.
//!
//! A dry run executes the entry against an isolated run store, then reports the
//! managed writes the run *would* have committed. Only saved data is isolated:
//! side effects outside the store (a `std::io` write, a `std::log` line) are not,
//! so the report covers managed writes alone. With `--trace` set, each event also
//! forwards to a [`TraceHook`].

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{Frame, RuntimeError, StepHook, WriteOp, WriteTarget};
use marrow_syntax::SourceSpan;
use serde_json::json;

use crate::CheckFormat;
use crate::trace::{TraceHook, WriteTargetNames, render_write_target, write_target_json};

/// One managed operation a dry run would have committed; `value` is `None` for a
/// delete.
pub(crate) struct PlannedWrite {
    op: WriteOp,
    target: WriteTarget,
    value: Option<Vec<u8>>,
}

/// Collects the managed writes a dry run would commit, optionally forwarding each
/// event to a [`TraceHook`] under `--dry-run --trace`.
pub(crate) struct DryRunHook {
    planned: Vec<PlannedWrite>,
    transaction_buffer: Vec<PlannedWrite>,
    transaction_depth: usize,
    discarding_transaction: bool,
    trace: Option<TraceHook>,
}

impl DryRunHook {
    pub(crate) fn new(trace: Option<TraceHook>) -> Self {
        Self {
            planned: Vec::new(),
            transaction_buffer: Vec::new(),
            transaction_depth: 0,
            discarding_transaction: false,
            trace,
        }
    }

    pub(crate) fn into_report(self) -> (Vec<PlannedWrite>, Option<TraceHook>) {
        (self.planned, self.trace)
    }
}

impl StepHook for DryRunHook {
    fn before_statement(
        &mut self,
        span: SourceSpan,
        frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError> {
        // Forward to the trace when composing; a dry run without `--trace` ignores
        // statements and records only the writes it would commit.
        match &mut self.trace {
            Some(trace) => trace.before_statement(span, frame),
            None => Ok(()),
        }
    }

    fn before_write(
        &mut self,
        op: WriteOp,
        target: &WriteTarget,
        value: Option<&[u8]>,
        depth: usize,
    ) {
        if let Some(trace) = &mut self.trace {
            trace.before_write(op, target, value, depth);
        }
        let planned = PlannedWrite {
            op,
            target: target.clone(),
            value: value.map(<[u8]>::to_vec),
        };
        if self.transaction_depth == 0 {
            self.planned.push(planned);
        } else if !self.discarding_transaction {
            self.transaction_buffer.push(planned);
        }
    }

    fn transaction_begin(&mut self, transaction_depth: usize) {
        if transaction_depth == 1 {
            self.transaction_buffer.clear();
            self.discarding_transaction = false;
        }
        self.transaction_depth = transaction_depth;
    }

    fn transaction_commit(&mut self, transaction_depth: usize) {
        if transaction_depth == 1 {
            if self.discarding_transaction {
                self.transaction_buffer.clear();
            } else {
                self.planned.append(&mut self.transaction_buffer);
            }
            self.transaction_depth = 0;
            self.discarding_transaction = false;
        } else {
            self.transaction_depth = transaction_depth.saturating_sub(1);
        }
    }

    fn transaction_rollback(&mut self, transaction_depth: usize) {
        self.transaction_buffer.clear();
        if transaction_depth == 1 {
            self.transaction_depth = 0;
            self.discarding_transaction = false;
        } else {
            self.transaction_depth = transaction_depth.saturating_sub(1);
            self.discarding_transaction = true;
        }
    }
}

/// Report the planned writes a dry run collected, on standard error to stay off
/// the program's own stdout stream.
pub(crate) fn report(
    planned: &[PlannedWrite],
    format: CheckFormat,
    program: &CheckedRuntimeProgram,
) {
    let names = WriteTargetNames::from_program(program);
    let writes = planned
        .iter()
        .filter(|step| matches!(step.op, WriteOp::Write))
        .count();
    let deletes = planned.len() - writes;
    match format {
        CheckFormat::Text => {
            for step in planned {
                match (step.op, &step.value) {
                    (WriteOp::Write, Some(value)) => eprintln!(
                        "would write {}\t{}",
                        render_write_target(&step.target, &names),
                        names.render_leaf_value(&step.target, value)
                    ),
                    (WriteOp::Write, None) => {
                        eprintln!("would write {}", render_write_target(&step.target, &names))
                    }
                    (WriteOp::Delete, _) => {
                        eprintln!("would delete {}", render_write_target(&step.target, &names))
                    }
                }
            }
            eprintln!("dry run: {writes} write(s), {deletes} delete(s) (not committed)");
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            let records: Vec<serde_json::Value> = planned
                .iter()
                .map(|step| planned_record(step, &names))
                .collect();
            crate::write_json_err(json!({
                "committed": false,
                "writes": writes,
                "deletes": deletes,
                "planned": records,
            }));
        }
    }
}

/// Render one planned write as a JSON record with op, path, and base64 value
/// (or null for deletes).
fn planned_record(step: &PlannedWrite, names: &WriteTargetNames) -> serde_json::Value {
    let op = match step.op {
        WriteOp::Write => "write",
        WriteOp::Delete => "delete",
    };
    json!({
        "op": op,
        "target": write_target_json(&step.target, names),
        "path": render_write_target(&step.target, names),
        "value_b64": step.value.as_deref().map(marrow_run::base64::encode),
    })
}
