//! `marrow run <export> [--store <path>] [--format jsonl] [-- <args>...]`.
//!
//! The production run path: capture the project at the working directory, compile
//! it to canonical image bytes, verify them into a sealed image, resolve the named
//! export, and execute it on the VM. Each of the four failure families surfaces as
//! its own typed [`Record`]; the value or the first failure sets the exit code.
//!
//! Durable execution (opening a store for an export with nonempty demand) is wired
//! in with the durable slices; at this slice every admitted export is read-only.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use marrow_compile::{ExportEntry, ExportId, compile};
use marrow_kernel::codec::value::ScalarKind;
use marrow_kernel::durable::{
    DurableStore, ExportDemand, FieldSchema, InvocationGrant, SessionError, SiteSpec,
    SiteTarget as KernelSiteTarget, StoreSchema,
};
use marrow_verify::{
    ImageType, Scalar, SealedEnumType, SealedRecordType, SealedSiteTarget, VerifiedImage,
};
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
                &[],
                &[],
                ExitCode::FAILURE,
            );
        }
    };

    // Family 1: source diagnostics.
    let compiled = match compile(&project) {
        Ok(compiled) => compiled,
        Err(diagnostics) => {
            let records: Vec<Record> = diagnostics
                .iter()
                .map(|diagnostic| Record::Diagnostic {
                    code: diagnostic.code,
                    line: diagnostic.line,
                    column: diagnostic.column,
                })
                .collect();
            return emit(args.format, &records, &[], &[], ExitCode::FAILURE);
        }
    };

    // Resolve the caller-supplied name to a stable id through the compiler's export
    // directory, before verification, so no source string reaches the image. The VM
    // dispatches only on this verified id.
    let export_id = match resolve_export(&compiled.exports, &args.export) {
        Ok(id) => id,
        Err(message) => return usage(&message),
    };

    // Family 2: artifact decode/verify rejection. The compiler cannot mint a
    // verified image — only `marrow_verify::verify` can.
    let image = match marrow_verify::verify(&compiled.image.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            return emit(
                args.format,
                &[Record::ArtifactRejected {
                    code: rejection.code(),
                }],
                &[],
                &[],
                ExitCode::FAILURE,
            );
        }
    };

    let Some(export) = image.export_by_id(export_id) else {
        // The directory named an id the verified image does not carry: a compiler
        // bug, since the same draft produced both.
        eprintln!("internal error: export directory and image disagree");
        return ExitCode::FAILURE;
    };
    let func_index = export.function();
    let demand = export.demand();

    // Positional call arguments are decoded against the verified export signature.
    let function = image.function(func_index);
    let call_args = match decode_args(function.params(), &args.call_args) {
        Ok(values) => values,
        Err(message) => return usage(&message),
    };

    // Family 3: source-mapped runtime fault, or the value. A durable export (nonempty
    // demand) runs against an in-process store opened here (interim; dies at D00).
    let record = if demand.is_empty() {
        run_storeless(&image, func_index, call_args)
    } else {
        let Some(store_path) = &args.store else {
            return usage("this export reads or writes durable data; pass `--store <path>`");
        };
        run_durable(&image, func_index, call_args, store_path, demand)
    };

    let exit = match &record {
        Record::Value(_) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    };
    emit(
        args.format,
        &[record],
        image.record_types(),
        image.enums(),
        exit,
    )
}

/// Resolve a caller-supplied export path to its [`ExportId`] through the compiler's
/// export directory. A `module.item` path (one containing a `.`) matches a module
/// and item exactly; a bare item matches by item name and is an error when more
/// than one module exports it.
fn resolve_export(directory: &[ExportEntry], query: &str) -> Result<ExportId, String> {
    if let Some((module, item)) = query.rsplit_once('.') {
        return directory
            .iter()
            .find(|entry| entry.module == module && entry.item == item)
            .map(|entry| entry.id)
            .ok_or_else(|| format!("no exported function `{query}` in this project"));
    }
    let mut matching = directory.iter().filter(|entry| entry.item == query);
    let first = matching
        .next()
        .ok_or_else(|| format!("no exported function `{query}` in this project"))?;
    if matching.next().is_some() {
        return Err(format!(
            "`{query}` is exported by more than one module; qualify it as `module.{query}`"
        ));
    }
    Ok(first.id)
}

/// Run a storeless export.
fn run_storeless(image: &VerifiedImage, func_index: u16, call_args: Vec<Value>) -> Record {
    match marrow_vm::run(image, func_index, call_args) {
        Ok(value) => Record::Value(value),
        Err(fault) => Record::Fault {
            code: fault.code(),
            line: fault.line(),
            column: fault.column(),
            detail: fault.detail().map(str::to_owned),
        },
    }
}

/// Open the store in-process, resolve authority, and run the durable export.
fn run_durable(
    image: &VerifiedImage,
    func_index: u16,
    call_args: Vec<Value>,
    store_path: &Path,
    demand: marrow_verify::Demand,
) -> Record {
    let schema = build_schema(image);
    let sites = build_sites(image);
    let mut store = match DurableStore::open(store_path, schema, sites) {
        Ok(store) => store,
        Err(error) => return Record::OperationalError { code: error.code() },
    };
    let grant = InvocationGrant::full_store();
    let kernel_demand = ExportDemand {
        read: demand.read,
        write: demand.write,
    };
    // A mutating export drives a transaction session; a read-only export a read
    // session over a pinned snapshot.
    if demand.write {
        match store.txn_session(grant, kernel_demand) {
            Ok(mut session) => run_session(image, func_index, call_args, &mut session),
            Err(error) => session_error_record(image, func_index, error),
        }
    } else {
        match store.read_session(grant, kernel_demand) {
            Ok(mut session) => run_session(image, func_index, call_args, &mut session),
            Err(error) => session_error_record(image, func_index, error),
        }
    }
}

fn run_session(
    image: &VerifiedImage,
    func_index: u16,
    call_args: Vec<Value>,
    session: &mut dyn marrow_kernel::durable::Durable,
) -> Record {
    match marrow_vm::run_durable(image, func_index, call_args, session) {
        Ok(value) => Record::Value(value),
        Err(fault) => Record::Fault {
            code: fault.code(),
            line: fault.line(),
            column: fault.column(),
            detail: fault.detail().map(str::to_owned),
        },
    }
}

/// Map a session-setup failure to a typed record. An authority denial is a
/// source-uncatchable fault at the export entry; a profile mismatch or engine
/// failure is an operational error.
fn session_error_record(image: &VerifiedImage, func_index: u16, error: SessionError) -> Record {
    match error {
        SessionError::Denied => {
            let (line, column) = image.function(func_index).span_at(0).unwrap_or((1, 1));
            Record::Fault {
                code: marrow_codes::Code::RunAuthority.as_str(),
                line,
                column,
                detail: None,
            }
        }
        SessionError::ProfileMismatch => Record::OperationalError {
            code: marrow_codes::Code::StoreCorruption.as_str(),
        },
        SessionError::Engine(store) => Record::OperationalError { code: store.code() },
    }
}

/// The kernel store schema derived from the verified image's single root.
fn build_schema(image: &VerifiedImage) -> StoreSchema {
    let root = &image.roots()[0];
    let record = image.record_type(root.record());
    StoreSchema {
        root_name: root.name().to_string(),
        key: scalar_kind(root.key()),
        fields: record
            .fields()
            .iter()
            .map(|field| FieldSchema {
                name: field.name.to_string(),
                // A durable root record is verified to carry only scalar fields.
                kind: match field.ty {
                    ImageType::Scalar { scalar, .. } => scalar_kind(scalar),
                    _ => unreachable!("a durable field is a scalar"),
                },
                required: field.required,
            })
            .collect(),
    }
}

/// The kernel site specs derived from the verified image's site table.
fn build_sites(image: &VerifiedImage) -> Vec<SiteSpec> {
    image
        .sites()
        .iter()
        .map(|site| SiteSpec {
            target: match site.target {
                SealedSiteTarget::Entry => KernelSiteTarget::Entry,
                SealedSiteTarget::Field(field) => KernelSiteTarget::Field(field),
            },
        })
        .collect()
}

/// The kernel scalar kind for an image scalar type.
fn scalar_kind(scalar: Scalar) -> ScalarKind {
    match scalar {
        Scalar::Int => ScalarKind::Int,
        Scalar::Bool => ScalarKind::Bool,
        Scalar::Text => ScalarKind::Str,
        Scalar::Bytes => ScalarKind::Bytes,
    }
}

/// Decode positional CLI arguments against the export's parameter types. A scalar
/// parameter decodes from its text; a record (`struct`) parameter has no
/// command-line spelling, so an export taking one cannot be run from the terminal.
fn decode_args(params: &[ImageType], args: &[String]) -> Result<Vec<Value>, String> {
    if params.len() != args.len() {
        return Err(format!(
            "this export takes {} argument(s), found {}",
            params.len(),
            args.len()
        ));
    }
    params
        .iter()
        .zip(args)
        .map(|(param, text)| match param {
            ImageType::Scalar {
                scalar,
                optional: false,
            } => decode_arg(*scalar, text),
            _ => Err("a struct argument cannot be passed on the command line".to_string()),
        })
        .collect()
}

fn decode_arg(scalar: Scalar, text: &str) -> Result<Value, String> {
    match scalar {
        Scalar::Int => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("`{text}` is not an integer")),
        Scalar::Bool => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("`{text}` is not a boolean (true/false)")),
        },
        Scalar::Text => Ok(Value::Text(Rc::from(text))),
        // A `bytes` argument is a `0x`-prefixed even-length lowercase-hex string,
        // matching how a `bytes` value renders back out.
        Scalar::Bytes => decode_hex_bytes(text)
            .map(|bytes| Value::Bytes(Rc::from(bytes.as_slice())))
            .ok_or_else(|| format!("`{text}` is not `0x`-prefixed lowercase hex")),
    }
}

/// Decode a `0x`-prefixed even-length lowercase-hex string to bytes.
fn decode_hex_bytes(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("0x")?;
    if hex.len() % 2 != 0
        || hex
            .bytes()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
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
fn emit(
    format: Format,
    records: &[Record],
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    exit: ExitCode,
) -> ExitCode {
    for record in records {
        match format {
            Format::Jsonl => println!("{}", record.to_jsonl(types, enums)),
            Format::Text => {
                let text = record.to_text(types, enums);
                if !text.is_empty() {
                    println!("{text}");
                }
            }
        }
    }
    exit
}
