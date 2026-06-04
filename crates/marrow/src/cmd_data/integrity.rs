use std::io::{self, Write};
use std::process::ExitCode;

use marrow_check::{CheckedProgram, StoreLeafKind, identity_leaf_key_mismatch};
use marrow_store::StoreError;
use marrow_store::key::decode_identity_payload_arity;
use marrow_store::tree::{TreeStore, decode_tree_enum_member};
use marrow_store::value::decode_value;
use marrow_syntax::Diagnose;
use serde_json::json;

use crate::{CheckFormat, envelope, load_checked_project, write_json};

use super::inspect::{DataRecord, visit_data_records};

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
    // One snapshot spans every traversal — the declared-cell walk, the orphan scan,
    // and the report pass — so the whole verdict describes one coherent store.
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

pub(crate) fn count_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<(usize, usize), StoreError> {
    let mut problems = 0usize;
    let mut records = 0usize;
    visit_integrity_problems(store, program, |outcome| {
        if let Outcome::Record = outcome.kind {
            records += 1;
        }
        if outcome.problem.is_some() {
            problems = problems.checked_add(1).ok_or(StoreError::LimitExceeded {
                limit: "data integrity problem count",
            })?;
        }
        Ok(())
    })?;
    Ok((records, problems))
}

/// Count the declared records and the schema problems among them, walking only the
/// declared-record pass — decode and key-type checks — and never the orphan scan.
/// Restore verifies that the data it just replayed decodes under the schema; a
/// store may carry orphan debris (a faithfully copied cell under a dropped root or
/// field) without the restored declared data being invalid, so restore must not
/// reject a faithful backup on orphan-freeness. The orphan and corruption findings
/// belong to `data integrity`, reported through [`count_integrity_problems`].
pub(crate) fn declared_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<(usize, usize), StoreError> {
    let mut records = 0usize;
    let mut problems = 0usize;
    visit_data_records(program, store, |record| {
        records += 1;
        if check_record(program, &record).is_some() {
            problems = problems.checked_add(1).ok_or(StoreError::LimitExceeded {
                limit: "data integrity problem count",
            })?;
        }
        Ok(())
    })?;
    Ok((records, problems))
}

/// Whether an integrity outcome came from walking a declared record or from
/// scanning the store's actual cells for orphans. Only declared records are
/// counted toward the verified record total.
enum Outcome {
    Record,
    StoredCell,
}

struct IntegrityOutcome {
    kind: Outcome,
    problem: Option<IntegrityProblem>,
}

/// Visit both integrity passes against one snapshot: the declared-cell walk (which
/// yields decode and key-type findings) and the orphan scan over the store's actual
/// cells (which yields orphan and corruption findings). Each visited cell is one
/// `IntegrityOutcome`, carrying a problem when the cell is unhealthy.
fn visit_integrity_problems(
    store: &TreeStore,
    program: &CheckedProgram,
    mut visit: impl FnMut(IntegrityOutcome) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    visit_data_records(program, store, |record| {
        visit(IntegrityOutcome {
            kind: Outcome::Record,
            problem: check_record(program, &record),
        })
    })?;
    super::orphan::visit_orphans(store, program, |orphan| {
        visit(IntegrityOutcome {
            kind: Outcome::StoredCell,
            problem: Some(IntegrityProblem {
                code: orphan.code,
                path: orphan.path,
                message: orphan.message,
            }),
        })
    })
}

struct IntegrityProblem {
    code: &'static str,
    path: String,
    message: String,
}

impl Diagnose for IntegrityProblem {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
}

fn check_record(program: &CheckedProgram, record: &DataRecord) -> Option<IntegrityProblem> {
    if let Some(mismatch) = &record.key_mismatch {
        return Some(IntegrityProblem {
            code: "data.key_type",
            path: record.path.clone(),
            message: format!(
                "stored key is a {} where the schema declares {}",
                mismatch.found.name(),
                mismatch.expected.name()
            ),
        });
    }
    match &record.leaf {
        StoreLeafKind::Scalar(ty) => {
            decode_value(&record.value, *ty)
                .is_none()
                .then(|| IntegrityProblem {
                    code: "data.decode",
                    path: record.path.clone(),
                    message: format!("stored value is not a canonical {} form", ty.name()),
                })
        }
        StoreLeafKind::Identity { store_root, arity } => {
            check_identity_leaf(program, record, store_root, *arity)
        }
        StoreLeafKind::Enum { enum_id } => check_enum_leaf(program, record, *enum_id),
    }
}

fn check_identity_leaf(
    program: &CheckedProgram,
    record: &DataRecord,
    store_root: &str,
    arity: usize,
) -> Option<IntegrityProblem> {
    let Some(keys) = decode_identity_payload_arity(&record.value, arity) else {
        return Some(IntegrityProblem {
            code: "data.decode",
            path: record.path.clone(),
            message: format!("stored value is not a canonical `Id(^{store_root})` encoding"),
        });
    };
    identity_leaf_key_mismatch(program, store_root, &keys).map(|(expected, found)| {
        IntegrityProblem {
            code: "data.key_type",
            path: record.path.clone(),
            message: format!(
                "stored `Id(^{store_root})` reference has a {} key where the schema declares {}",
                found.name(),
                expected.name()
            ),
        }
    })
}

fn check_enum_leaf(
    program: &CheckedProgram,
    record: &DataRecord,
    enum_id: marrow_check::EnumId,
) -> Option<IntegrityProblem> {
    let enum_fact = program.facts.enum_(enum_id)?;
    let stored = decode_tree_enum_member(&record.value).ok();
    let Some(stored) = stored else {
        return Some(enum_decode_problem(record, &enum_fact.name));
    };
    if stored.enum_id().as_str() != enum_fact.catalog_id {
        return Some(enum_decode_problem(record, &enum_fact.name));
    }
    let valid_member = program.facts.enum_members().iter().any(|member| {
        member.enum_id == enum_id
            && member.catalog_id == stored.member_id().as_str()
            && program.facts.enum_member_is_selectable(member.id)
    });
    (!valid_member).then(|| enum_decode_problem(record, &enum_fact.name))
}

fn enum_decode_problem(record: &DataRecord, enum_name: &str) -> IntegrityProblem {
    IntegrityProblem {
        code: "data.decode",
        path: record.path.clone(),
        message: format!("stored value is not a catalog-backed `{enum_name}` member"),
    }
}

fn report_integrity(
    dir: &str,
    records: usize,
    problems: usize,
    store: &TreeStore,
    program: &CheckedProgram,
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
    program: &CheckedProgram,
) -> Result<(), StoreError> {
    visit_integrity_problems(store, program, |outcome| {
        if let Some(problem) = outcome.problem {
            eprintln!("{}: {}: {}", problem.path, problem.code, problem.message);
        }
        Ok(())
    })
}

fn write_integrity_problems_jsonl(
    store: &TreeStore,
    program: &CheckedProgram,
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
    program: &CheckedProgram,
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
    envelope(problem, json!({ "path": problem.path }), None, None)
}
