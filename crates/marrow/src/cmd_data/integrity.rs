use std::io::{self, Write};
use std::process::ExitCode;

use marrow_check::tooling::{IntegrityProblem, count_integrity_problems, visit_integrity_problems};
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::{CheckFormat, envelope, load_checked_project, write_json};

pub(super) fn data_integrity(args: &[String]) -> ExitCode {
    let (dir, format) = match super::one_positional_with_format("data integrity", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let store = match super::open_tree_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let _snapshot = match &store {
        Some(store) => match store.read_snapshot() {
            Ok(snapshot) => Some(snapshot),
            Err(error) => return super::report_store_error(error, format),
        },
        None => None,
    };
    let (records, problems) = match &store {
        Some(store) => match count_integrity_problems(store, &program) {
            Ok(counts) => counts,
            Err(error) => return super::report_store_error(error, format),
        },
        None => (0, 0),
    };

    if let Some(store) = &store {
        if let Err(error) = report_integrity(&dir, records, problems, store, &program, format) {
            return super::report_store_error(error, format);
        }
    } else {
        report_empty_integrity(&dir, format);
    }
    if problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn report_integrity(
    dir: &str,
    records: usize,
    problems: usize,
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<(), StoreError> {
    match format {
        CheckFormat::Text => {
            if problems == 0 {
                println!("ok: {dir} integrity verified ({records} records)");
            } else {
                write_integrity_problems_text(store, program)?;
            }
        }
        CheckFormat::Json => write_integrity_json(dir, records, store, program)?,
        CheckFormat::Jsonl => {
            write_integrity_problems_jsonl(store, program)?;
            write_json(json!({
                "kind": "summary",
                "records": records,
                "problems": problems,
            }));
        }
    }
    Ok(())
}

fn report_empty_integrity(dir: &str, format: CheckFormat) {
    match format {
        CheckFormat::Text => println!("ok: {dir} integrity verified (0 records)"),
        CheckFormat::Json => write_json(json!({
            "project": dir,
            "records": 0,
            "problems": [],
        })),
        CheckFormat::Jsonl => write_json(json!({
            "kind": "summary",
            "records": 0,
            "problems": 0,
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
            write_json(integrity_record(&problem));
        }
        Ok(())
    })
}

fn write_integrity_json(
    dir: &str,
    records: usize,
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
) -> Result<(), StoreError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "{{\"project\":").expect("write integrity JSON");
    serde_json::to_writer(&mut out, dir).expect("serialize project path");
    write!(out, ",\"records\":{records},\"problems\":[").expect("write integrity JSON");
    let mut first = true;
    visit_integrity_problems(store, program, |outcome| {
        if let Some(problem) = outcome.problem {
            if !first {
                write!(out, ",").expect("write integrity JSON separator");
            }
            first = false;
            serde_json::to_writer(&mut out, &integrity_record(&problem))
                .expect("serialize integrity problem");
        }
        Ok(())
    })?;
    writeln!(out, "]}}").expect("write integrity JSON");
    Ok(())
}

fn integrity_record(problem: &IntegrityProblem) -> serde_json::Value {
    envelope(
        problem,
        json!({ "path": problem.path }),
        None,
        Some(problem.help),
    )
}
