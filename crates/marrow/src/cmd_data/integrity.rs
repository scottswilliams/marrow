use std::io;
use std::process::ExitCode;

use marrow_check::tooling::{IntegrityProblem, count_integrity_problems, visit_integrity_problems};
use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::{CheckFormat, envelope, load_checked_project_with_format, write_json};

pub(super) fn data_integrity(args: &[String]) -> ExitCode {
    let (dir, format) = match super::one_positional_with_format("data integrity", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project_with_format(&dir, format) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let store = match crate::open_store_for_inspection(&dir, &config, format) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let _snapshot = match super::pin_snapshot(&store, format) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
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
            "project": crate::project_json_path(dir),
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
    super::write_json_array_envelope(
        &mut out,
        |out| {
            write!(out, "\"project\":").expect("write integrity JSON");
            serde_json::to_writer(&mut *out, &crate::project_json_path(dir))
                .expect("serialize project path");
            write!(out, ",\"records\":{records}").expect("write integrity JSON");
        },
        "problems",
        |emit| {
            visit_integrity_problems(store, program, |outcome| {
                if let Some(problem) = outcome.problem {
                    emit(&integrity_record(&problem));
                }
                Ok(())
            })
        },
    )
}

fn integrity_record(problem: &IntegrityProblem) -> serde_json::Value {
    let mut record = envelope(
        problem,
        json!({ "path": problem.path }),
        None,
        Some(problem.help),
    );
    if let Some(incomplete) = &problem.incomplete {
        let object = record.as_object_mut().expect("integrity record object");
        object.insert(
            "store_catalog_id".into(),
            json!(incomplete.store_catalog_id.as_str()),
        );
        object.insert(
            "record_identity".into(),
            json!(
                incomplete
                    .record_identity
                    .iter()
                    .map(saved_key_json)
                    .collect::<Vec<_>>()
            ),
        );
        object.insert(
            "parent_path".into(),
            json!(
                incomplete
                    .parent_path
                    .iter()
                    .map(data_path_segment_json)
                    .collect::<Vec<_>>()
            ),
        );
        object.insert(
            "missing_member_catalog_id".into(),
            json!(incomplete.missing_member_catalog_id.as_str()),
        );
    }
    if let Some(dangling_ref) = &problem.dangling_ref {
        let object = record.as_object_mut().expect("integrity record object");
        object.insert(
            "containing_identity".into(),
            json!(
                dangling_ref
                    .containing_identity
                    .iter()
                    .map(saved_key_json)
                    .collect::<Vec<_>>()
            ),
        );
        object.insert(
            "field_catalog_id".into(),
            json!(dangling_ref.field_catalog_id.as_str()),
        );
        object.insert(
            "referenced_root".into(),
            json!(dangling_ref.referenced_root),
        );
        object.insert(
            "referenced_identity".into(),
            json!(
                dangling_ref
                    .referenced_identity
                    .iter()
                    .map(saved_key_json)
                    .collect::<Vec<_>>()
            ),
        );
    }
    record
}

fn data_path_segment_json(segment: &DataPathSegment) -> serde_json::Value {
    match segment {
        DataPathSegment::Member(catalog_id) => {
            json!({ "member_catalog_id": catalog_id.as_str() })
        }
        DataPathSegment::Key(key) => json!({ "key": saved_key_json(key) }),
    }
}

fn saved_key_json(key: &SavedKey) -> serde_json::Value {
    match key {
        SavedKey::Int(value) => json!({ "type": "int", "value": value }),
        SavedKey::Bool(value) => json!({ "type": "bool", "value": value }),
        SavedKey::Str(value) => json!({ "type": "string", "value": value }),
        SavedKey::Date(value) => json!({ "type": "date", "days_since_epoch": value }),
        SavedKey::Duration(value) => json!({ "type": "duration", "nanos": value.to_string() }),
        SavedKey::Instant(value) => {
            json!({ "type": "instant", "nanos_since_epoch": value.to_string() })
        }
        SavedKey::Bytes(value) => {
            json!({ "type": "bytes", "value_b64": marrow_run::base64::encode(value) })
        }
    }
}
