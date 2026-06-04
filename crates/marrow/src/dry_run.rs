//! The dry-run hook and report for `run --dry-run`.
//!
//! A dry run executes the entry inside one outer store savepoint that is always
//! rolled back, so the saved data is left byte-for-byte unchanged. It collects the
//! managed writes the run *would* have committed through the same write-observation
//! seam the trace uses, then reports them. When `--trace` is also set, it forwards
//! each event to a [`TraceHook`] so the two compose: observe and discard.
//!
//! Only saved data is rewound. Side effects outside the store — a `std::io` file
//! write, a `std::log` line — are not rolled back, so the report covers managed
//! writes alone.

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{Frame, RuntimeError, StepHook, WriteOp, WriteTarget};
use marrow_syntax::SourceSpan;
use serde_json::json;

use crate::CheckFormat;
use crate::trace::{TraceHook, WriteTargetNames, render_write_target, write_target_json};

/// One planned managed operation the run would have committed: its kind, the human
/// path, and the value bytes for a write (a delete has none).
pub(crate) struct PlannedWrite {
    op: WriteOp,
    target: WriteTarget,
    value: Option<Vec<u8>>,
}

/// Collects the managed writes a dry run would commit, optionally forwarding each
/// observed statement and write to a [`TraceHook`] when `--dry-run --trace`
/// compose. It is purely observational — the run's writes still stage inside the
/// outer savepoint and are discarded when the savepoint is rolled back.
pub(crate) struct DryRunHook {
    planned: Vec<PlannedWrite>,
    trace: Option<TraceHook>,
}

impl DryRunHook {
    pub(crate) fn new(trace: Option<TraceHook>) -> Self {
        Self {
            planned: Vec::new(),
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
        self.planned.push(PlannedWrite {
            op,
            target: target.clone(),
            value: value.map(<[u8]>::to_vec),
        });
    }
}

/// Report the planned writes a dry run collected. The run's own `print`/`write`
/// output is handled by the caller; this renders only the dry-run report, on
/// standard error under text format (off the program's stdout stream) or as a JSON
/// object under `json`/`jsonl`.
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
            eprintln!("dry run: {writes} write(s), {deletes} delete(s) (rolled back)");
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            let records: Vec<serde_json::Value> = planned
                .iter()
                .map(|step| planned_record(step, &names))
                .collect();
            crate::write_json(json!({
                "committed": false,
                "writes": writes,
                "deletes": deletes,
                "planned": records,
            }));
        }
    }
}

/// One planned write as a JSON record: its op, the human path, and base64 of the
/// value bytes for a write (a delete has none).
fn planned_record(step: &PlannedWrite, names: &WriteTargetNames) -> serde_json::Value {
    let op = match step.op {
        WriteOp::Write => "write",
        WriteOp::Delete => "delete",
    };
    json!({
        "op": op,
        "target": write_target_json(&step.target, names),
        "path": render_write_target(&step.target, names),
        "value_b64": step.value.as_ref().map(|bytes| marrow_run::base64::encode(bytes)),
    })
}
