//! The dry-run hook and report for `run --dry-run`.
//!
//! A dry run executes the entry against an isolated run store, then reports the
//! managed writes the run *would* have committed. Only saved data is isolated:
//! side effects outside the store (a `std::io` write, a `std::log` line) are not,
//! so the report covers managed writes alone. With `--trace` set, each event also
//! forwards to a [`TraceHook`].

use std::collections::BTreeMap;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{Frame, RuntimeError, StepHook, WriteOp, WriteTarget};
use marrow_syntax::SourceSpan;
use serde_json::json;

use crate::trace::{TraceHook, WriteTargetNames, render_write_target, write_target_json};

/// One managed operation a dry run would have committed; `value` is `None` for a
/// delete.
pub(crate) struct PlannedWrite {
    op: WriteOp,
    target: WriteTarget,
    value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PreviewActions {
    pub(crate) would_freeze: bool,
    pub(crate) would_apply: bool,
    pub(crate) would_fence: bool,
    pub(crate) messages: Vec<String>,
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

#[derive(Clone, Copy)]
pub(crate) enum ReportFormat {
    Text,
    Json,
}

/// Report the planned writes a dry run collected, on standard error to stay off
/// the program's own stdout stream.
pub(crate) fn report(
    planned: &[PlannedWrite],
    format: ReportFormat,
    program: &CheckedRuntimeProgram,
    actions: &PreviewActions,
) {
    let names = WriteTargetNames::from_program(program);
    let write_counts = write_counts(planned, &names);
    let (writes, deletes) = write_counts.totals();
    match format {
        ReportFormat::Text => {
            for message in &actions.messages {
                eprintln!("{message}");
            }
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
            write_counts.render_text();
            eprintln!("dry run: {writes} write(s), {deletes} delete(s) (not committed)");
        }
        ReportFormat::Json => {
            let records: Vec<serde_json::Value> = planned
                .iter()
                .map(|step| planned_record(step, &names))
                .collect();
            crate::write_json_err(json!({
                "committed": false,
                "would_freeze": actions.would_freeze,
                "would_apply": actions.would_apply,
                "would_fence": actions.would_fence,
                "messages": actions.messages,
                "writes": writes,
                "deletes": deletes,
                "write_counts": write_counts.to_json(),
                "planned": records,
            }));
        }
    }
}

#[derive(Default)]
struct WriteCounts {
    roots: BTreeMap<String, TargetCounts>,
    indexes: BTreeMap<String, TargetCounts>,
}

#[derive(Default)]
struct TargetCounts {
    creates: usize,
    writes: usize,
    deletes: usize,
}

impl WriteCounts {
    /// The headline `write`/`delete` totals, summed from the per-target counts so
    /// they reconcile with the per-root and per-index breakdown the report renders.
    fn totals(&self) -> (usize, usize) {
        self.roots.values().chain(self.indexes.values()).fold(
            (0, 0),
            |(writes, deletes), counts| {
                (
                    writes + counts.creates + counts.writes,
                    deletes + counts.deletes,
                )
            },
        )
    }

    fn render_text(&self) {
        for (root, counts) in &self.roots {
            eprintln!(
                "would touch root {root}: {} create(s), {} write(s), {} delete(s)",
                counts.creates, counts.writes, counts.deletes
            );
        }
        for (index, counts) in &self.indexes {
            eprintln!(
                "would touch index {index}: {} create(s), {} write(s), {} delete(s)",
                counts.creates, counts.writes, counts.deletes
            );
        }
    }

    fn to_json(&self) -> serde_json::Value {
        json!({
            "roots": counts_map_json(&self.roots),
            "indexes": counts_map_json(&self.indexes),
        })
    }
}

fn counts_map_json(counts: &BTreeMap<String, TargetCounts>) -> serde_json::Value {
    serde_json::Value::Object(
        counts
            .iter()
            .map(|(target, counts)| {
                (
                    target.clone(),
                    json!({
                        "creates": counts.creates,
                        "writes": counts.writes,
                        "deletes": counts.deletes,
                    }),
                )
            })
            .collect(),
    )
}

fn write_counts(planned: &[PlannedWrite], names: &WriteTargetNames) -> WriteCounts {
    let mut counts = WriteCounts::default();
    for step in planned {
        match &step.target {
            WriteTarget::Data { store, path, .. } => {
                let entry = counts
                    .roots
                    .entry(names.root_display(store).to_string())
                    .or_default();
                // Only a record that did not already exist reaches the observer as an
                // empty-path node write, so each such write is one record create. A
                // re-established existing record is filtered upstream and never counted.
                match step.op {
                    WriteOp::Write if path.is_empty() => entry.creates += 1,
                    WriteOp::Write => entry.writes += 1,
                    WriteOp::Delete => entry.deletes += 1,
                }
            }
            WriteTarget::Index { index, .. } => {
                let entry = counts
                    .indexes
                    .entry(names.index_display(index))
                    .or_default();
                match step.op {
                    WriteOp::Write => entry.writes += 1,
                    WriteOp::Delete => entry.deletes += 1,
                }
            }
            WriteTarget::Meta { .. } => {}
        }
    }
    counts
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
