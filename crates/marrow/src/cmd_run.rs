//! `marrow run <export> [--store <path>] [--format jsonl] [-- <args>...]`.
//!
//! The production run path: capture the project at the working directory, compile
//! it to canonical image bytes, verify them into a sealed image, resolve the named
//! export, and execute it on the VM. Each of the four failure families surfaces as
//! its own typed [`Record`]; the value or the first failure sets the exit code.
//!
//! Durable execution (opening a store for an export with nonempty demand) is wired
//! in with the durable slices; at this slice every admitted export is read-only.

use std::path::PathBuf;
use std::process::ExitCode;

use marrow_compile::compile;
use marrow_vm::Value;

use crate::outcome::Record;
use crate::project::capture_project;

/// The output format for `marrow run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Jsonl,
}

struct RunArgs {
    export: String,
    #[allow(dead_code)]
    store: Option<PathBuf>,
    format: Format,
    call_args: Vec<String>,
}

pub(crate) fn run(rest: &[String]) -> ExitCode {
    let args = match parse_args(rest) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let project = match capture_project(&PathBuf::from(".")) {
        Ok(project) => project,
        Err(failure) => {
            return emit(
                args.format,
                &[Record::OperationalError { code: failure.code }],
                ExitCode::FAILURE,
            );
        }
    };

    // Family 1: source diagnostics.
    let encoded = match compile(&project) {
        Ok(image) => image,
        Err(diagnostics) => {
            let records: Vec<Record> = diagnostics
                .iter()
                .map(|diagnostic| Record::Diagnostic {
                    code: diagnostic.code,
                    line: diagnostic.line,
                    column: diagnostic.column,
                })
                .collect();
            return emit(args.format, &records, ExitCode::FAILURE);
        }
    };

    // Family 2: artifact decode/verify rejection. The compiler cannot mint a
    // verified image — only `marrow_verify::verify` can.
    let image = match marrow_verify::verify(&encoded.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            return emit(
                args.format,
                &[Record::ArtifactRejected {
                    code: rejection.code(),
                }],
                ExitCode::FAILURE,
            );
        }
    };

    let Some(export) = image.export(&args.export) else {
        eprintln!(
            "no exported function named `{}` in this project",
            args.export
        );
        return ExitCode::from(2);
    };
    let func_index = export.function();

    // Positional call arguments are decoded against the verified export signature.
    // The current subset admits only zero-parameter exports.
    let function = image.function(func_index);
    if !function.params().is_empty() || !args.call_args.is_empty() {
        eprintln!("`marrow run {}` takes no arguments yet", args.export);
        return ExitCode::from(2);
    }

    // Family 3: source-mapped runtime fault, or the value.
    let record = match marrow_vm::run(&image, func_index, Vec::<Value>::new()) {
        Ok(value) => Record::Value(value),
        Err(fault) => Record::Fault {
            code: fault.code(),
            line: fault.line(),
            column: fault.column(),
        },
    };
    let exit = match &record {
        Record::Value(_) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    };
    emit(args.format, &[record], exit)
}

fn parse_args(rest: &[String]) -> Result<RunArgs, ExitCode> {
    let mut export: Option<String> = None;
    let mut store: Option<PathBuf> = None;
    let mut format = Format::Text;
    let mut call_args: Vec<String> = Vec::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" => {
                call_args.extend(iter.by_ref().cloned());
                break;
            }
            "--store" => {
                let Some(path) = iter.next() else {
                    return Err(usage("`--store` needs a path"));
                };
                store = Some(PathBuf::from(path));
            }
            "--format" => match iter.next().map(String::as_str) {
                Some("jsonl") => format = Format::Jsonl,
                Some("text") => format = Format::Text,
                _ => return Err(usage("`--format` must be `text` or `jsonl`")),
            },
            other if other.starts_with('-') => {
                return Err(usage(&format!("unknown run option: {other}")));
            }
            other => {
                if export.replace(other.to_string()).is_some() {
                    return Err(usage("marrow run takes one export name"));
                }
            }
        }
    }
    let Some(export) = export else {
        return Err(usage("marrow run needs an export name"));
    };
    Ok(RunArgs {
        export,
        store,
        format,
        call_args,
    })
}

fn usage(message: &str) -> ExitCode {
    eprintln!("{message}; run marrow --help for usage");
    ExitCode::from(2)
}

/// Emit records in the selected format and return `exit`. JSONL is one canonical
/// object per line (LF-terminated); text prints each record's rendering.
fn emit(format: Format, records: &[Record], exit: ExitCode) -> ExitCode {
    for record in records {
        match format {
            Format::Jsonl => println!("{}", record.to_jsonl()),
            Format::Text => {
                let text = record.to_text();
                if !text.is_empty() {
                    println!("{text}");
                }
            }
        }
    }
    exit
}
