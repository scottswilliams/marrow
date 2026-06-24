use std::io;
use std::process::ExitCode;

use marrow_check::tooling::{count_integrity_problems, visit_integrity_problems};
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::{CheckFormat, write_json};

pub(super) fn data_integrity(args: &[String]) -> ExitCode {
    let target = match super::load_data_read_target_from_args("data integrity", args) {
        Ok(target) => target,
        Err(code) => return code,
    };
    // The committed lock is the independent witness for a store rolled back below its
    // committed roots, which the in-store anchors cannot see once they roll back with the
    // data. Cross-check it before the family passes so a total drop — including an absent
    // store while the lock records committed roots — fails closed rather than verifying
    // vacuously. A backup mount is self-contained, so its inspection ignores the live lock.
    if let Err(code) = super::verify_lock_roots_present(&target) {
        return code;
    }
    let super::DataReadTarget {
        dir,
        format,
        program,
        store,
        from_backup: _,
    } = target;
    let _snapshot = match super::pin_snapshot(&store, format) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };
    let (cells, problems) = match &store {
        Some(store) => match count_integrity_problems(store, &program) {
            Ok((cells, problems)) => (cells, problems),
            Err(error) => return super::report_store_error(error, format),
        },
        None => (0, 0),
    };
    let store_snapshot = match (&store, format) {
        (Some(store), CheckFormat::Json | CheckFormat::Jsonl) => {
            match super::data_snapshot_stamp(&program, store) {
                Ok(stamp) => Some(stamp),
                Err(error) => return super::report_store_error(error, format),
            }
        }
        _ => None,
    };

    if let Some(store) = &store {
        if let Err(error) = report_integrity(
            &dir,
            cells,
            problems,
            store,
            &program,
            format,
            store_snapshot.as_ref(),
        ) {
            return super::report_data_output_error(error, format);
        }
    } else {
        report_empty_integrity(&dir, format, None);
    }
    if problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn report_integrity(
    dir: &str,
    cells: usize,
    problems: usize,
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
    store_snapshot: Option<&marrow_check::tooling::DataSnapshotStamp>,
) -> Result<(), super::DataOutputError> {
    match format {
        CheckFormat::Text => {
            if problems == 0 {
                println!("ok: {dir} integrity verified ({cells} cells)");
            } else {
                write_integrity_problems_text(store, program)
                    .map_err(super::DataOutputError::from)?;
            }
        }
        CheckFormat::Json => write_integrity_json(dir, cells, store, program, store_snapshot)?,
        CheckFormat::Jsonl => {
            write_integrity_problems_jsonl(store, program).map_err(super::DataOutputError::from)?;
            write_json(json!({
                "kind": "summary",
                "cells": cells,
                "problems": problems,
                "store_snapshot": store_snapshot
                    .map(marrow_json::data_generation_stamp_to_json),
            }));
        }
    }
    Ok(())
}

fn report_empty_integrity(
    dir: &str,
    format: CheckFormat,
    store_snapshot: Option<&marrow_check::tooling::DataSnapshotStamp>,
) {
    match format {
        CheckFormat::Text => println!("ok: {dir} integrity verified (0 cells)"),
        CheckFormat::Json => write_json(json!({
            "project": crate::project_json_path(dir),
            "cells": 0,
            "problems": [],
            "store_snapshot": store_snapshot
                .map(marrow_json::data_generation_stamp_to_json),
        })),
        CheckFormat::Jsonl => write_json(json!({
            "kind": "summary",
            "cells": 0,
            "problems": 0,
            "store_snapshot": store_snapshot
                .map(marrow_json::data_generation_stamp_to_json),
        })),
    }
}

fn write_integrity_problems_text(
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
) -> Result<(), StoreError> {
    visit_integrity_problems(store, program, |outcome| {
        if let Some(problem) = outcome.problem {
            eprintln!("{}: {}: {}", problem.path, problem.code, problem.message);
            if let Some(help) = problem.help {
                eprintln!("help: {help}");
            }
        }
        Ok(())
    })
}

fn write_integrity_problems_jsonl(
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
) -> Result<(), StoreError> {
    visit_integrity_problems(store, program, |outcome| {
        if let Some(problem) = outcome.problem {
            write_json(marrow_json::saved_data::integrity_problem_record_to_json(
                &problem,
            ));
        }
        Ok(())
    })
}

fn write_integrity_json(
    dir: &str,
    cells: usize,
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
    store_snapshot: Option<&marrow_check::tooling::DataSnapshotStamp>,
) -> Result<(), super::DataOutputError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    super::write_json_array_envelope(
        &mut out,
        |out| {
            write!(out, "\"project\":")?;
            serde_json::to_writer(&mut *out, &crate::project_json_path(dir))
                .map_err(super::DataOutputError::from_json)?;
            write!(out, ",\"cells\":{cells}")?;
            write!(out, ",\"store_snapshot\":")?;
            serde_json::to_writer(
                &mut *out,
                &store_snapshot.map(marrow_json::data_generation_stamp_to_json),
            )
            .map_err(super::DataOutputError::from_json)?;
            Ok(())
        },
        "problems",
        |emit| {
            let mut output_error = None;
            let result = visit_integrity_problems(store, program, |outcome| {
                if let Some(problem) = outcome.problem {
                    super::stop_on_output_error(
                        &mut output_error,
                        emit(&marrow_json::saved_data::integrity_problem_record_to_json(
                            &problem,
                        )),
                    )?;
                }
                Ok(())
            });
            super::finish_output_visit(result, output_error)
        },
    )
}
