//! One-shot logical transfer commands. Lifecycle owns all admission and effects.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_local_wire::{Json, encode};

use super::{ReportFormat, read_image_bytes, validate_store_output, write_receipt};

pub(super) enum Command {
    Backup {
        image: PathBuf,
        store: PathBuf,
        output: PathBuf,
        format: ReportFormat,
    },
    Restore {
        input: PathBuf,
        store: PathBuf,
        format: ReportFormat,
    },
}

pub(super) fn parse(name: &str, mut args: impl Iterator<Item = String>) -> Option<Command> {
    let mut image = None;
    let mut store = None;
    let mut input = None;
    let mut output = None;
    let mut format = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--image" if name == "backup" && image.is_none() => {
                image = Some(PathBuf::from(args.next()?))
            }
            "--store" if store.is_none() => store = Some(PathBuf::from(args.next()?)),
            "--from" if name == "restore" && input.is_none() => {
                input = Some(PathBuf::from(args.next()?))
            }
            "--out" if name == "backup" && output.is_none() => {
                output = Some(PathBuf::from(args.next()?))
            }
            "--format" if format.is_none() => {
                format = Some(match args.next()?.as_str() {
                    "text" => ReportFormat::Text,
                    "jsonl" => ReportFormat::Jsonl,
                    _ => return None,
                })
            }
            _ => return None,
        }
    }
    let store = store?;
    let format = format.unwrap_or(ReportFormat::Text);
    match name {
        "backup" => Some(Command::Backup {
            image: image?,
            store,
            output: output?,
            format,
        }),
        "restore" => Some(Command::Restore {
            input: input?,
            store,
            format,
        }),
        _ => None,
    }
}

pub(super) fn run(command: Command) -> io::Result<ExitCode> {
    match command {
        Command::Backup {
            image,
            store,
            output,
            format,
        } => {
            let store_text = validate_store_output(&store)?;
            let destination = validate_destination(&output)?;
            let bytes = match read_image_bytes(&image) {
                Ok(bytes) => bytes,
                Err(code) => return Ok(code),
            };
            let result = marrow_lifecycle::backup(&store, &bytes, &output);
            let mut fields = base("backup", store_text, destination);
            match &result {
                Ok(backup) => {
                    success(&mut fields, &backup.audit);
                    fields.push(("backup_digest".into(), Json::Str(backup.digest.to_hex())));
                }
                Err(error) => {
                    failure(&mut fields, error.code());
                    if let Some(path) = &error.unpublished {
                        retained(&mut fields, path)?;
                    }
                    if error.cleanup.is_some() {
                        fields.push(("cleanup_failed".into(), Json::Bool(true)));
                    }
                    let _ = writeln!(io::stderr(), "{}: {error}", error.code());
                }
            }
            deliver(
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
                fields,
                format,
            )?;
            Ok(if result.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Command::Restore {
            input,
            store,
            format,
        } => {
            let destination = validate_destination(&store)?;
            let input_text = validate_store_output(&input)?;
            let mut file = match std::fs::File::open(&input) {
                Ok(file) => file,
                Err(error) => {
                    let _ = writeln!(
                        io::stderr(),
                        "{}: {error}",
                        marrow_codes::Code::IoRead.as_str()
                    );
                    return Ok(ExitCode::FAILURE);
                }
            };
            let result = marrow_lifecycle::restore(&mut file, &store);
            let mut fields = base("restore", destination, input_text);
            match &result {
                Ok(restored) => success(&mut fields, &restored.audit),
                Err(error) => {
                    failure(&mut fields, error.code());
                    if let Some(path) = &error.stage {
                        retained(&mut fields, path)?;
                    }
                    if let Some(instance) = error.published_instance() {
                        fields.push(("instance".into(), Json::Str(instance.to_hex())));
                    }
                    if let Some(outcome) = error.batch_outcome() {
                        fields.push((
                            "batch_outcome".into(),
                            Json::Str(
                                match outcome {
                                    marrow_lifecycle::RestoreBatchOutcome::Aborted => "aborted",
                                    marrow_lifecycle::RestoreBatchOutcome::Indeterminate => {
                                        "indeterminate"
                                    }
                                }
                                .into(),
                            ),
                        ));
                    }
                    let _ = writeln!(io::stderr(), "{}: {error}", error.code());
                }
            }
            deliver(
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
                fields,
                format,
            )?;
            Ok(if result.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

fn validate_destination(path: &Path) -> io::Result<&str> {
    let text = validate_store_output(path)?;
    // A retained private sibling may have a longer basename than the destination.
    // Reserve the filesystem owner's full basename bound before any effect.
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if validate_store_output(parent)?.len() + 256 > marrow_local_wire::MAX_STRING_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination parent exceeds the retained-path output bound",
        ));
    }
    Ok(text)
}

fn base(kind: &str, store: &str, artifact: &str) -> Vec<(String, Json)> {
    vec![
        ("kind".into(), Json::Str(kind.into())),
        ("store".into(), Json::Str(store.into())),
        ("backup".into(), Json::Str(artifact.into())),
    ]
}

fn success(fields: &mut Vec<(String, Json)>, audit: &marrow_lifecycle::StoreAudit) {
    fields.extend([
        ("outcome".into(), Json::Str("complete".into())),
        ("instance".into(), Json::Str(audit.instance.to_hex())),
        ("image".into(), Json::Str(audit.image_id.to_hex())),
        ("content_digest".into(), Json::Str(audit.digest.to_hex())),
    ]);
}

fn failure(fields: &mut Vec<(String, Json)>, code: &str) {
    fields.extend([
        ("outcome".into(), Json::Str("error".into())),
        ("code".into(), Json::Str(code.into())),
    ]);
}

fn retained(fields: &mut Vec<(String, Json)>, path: &Path) -> io::Result<()> {
    fields.push((
        "unpublished".into(),
        Json::Str(validate_store_output(path)?.into()),
    ));
    Ok(())
}

fn deliver(
    output: &mut dyn Write,
    diagnostic: &mut dyn Write,
    fields: Vec<(String, Json)>,
    format: ReportFormat,
) -> io::Result<()> {
    let receipt = Json::Object(fields);
    let result = match format {
        ReportFormat::Jsonl => write_receipt(output, &encode(&receipt)),
        ReportFormat::Text => write_text(output, &receipt),
    };
    if result.is_err() {
        // Delivery failure does not undo lifecycle effects. Retain the known
        // result in a bounded diagnostic if that channel is still writable.
        let _ = writeln!(
            diagnostic,
            "{}: transfer receipt delivery failed; known lifecycle result follows",
            marrow_codes::Code::IoWrite.as_str()
        );
        let _ = write_receipt(diagnostic, &encode(&receipt));
    }
    result
}

fn write_text(output: &mut dyn Write, receipt: &Json) -> io::Result<()> {
    if let Json::Object(fields) = receipt {
        for (key, value) in fields {
            if let Json::Str(value) = value {
                writeln!(output, "{key}: {value}")?;
            } else {
                writeln!(output, "{key}: {}", encode(value))?;
            }
        }
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_inputs_are_disjoint_and_unique() {
        let parse_words =
            |name, words: &str| parse(name, words.split_whitespace().map(str::to_owned));
        assert!(matches!(
            parse_words("backup", "--out b --store s --image i"),
            Some(Command::Backup { .. })
        ));
        assert!(matches!(
            parse_words("restore", "--from b --store s"),
            Some(Command::Restore { .. })
        ));
        for (name, words) in [
            ("restore", "--from b --store s --image i"),
            ("restore", "--store s"),
            ("backup", "--store s --out b"),
            ("backup", "--image i --store s --from b"),
            ("restore", "--from b --store s --store other"),
            ("backup", "--image i --store s --out a --out b"),
            ("restore", "--from b --store s --format text --format jsonl"),
            ("restore", "--from b --store s --format invalid"),
        ] {
            assert!(parse_words(name, words).is_none(), "{name} {words}");
        }
    }

    #[derive(Clone, Copy)]
    enum Failure {
        Write,
        Flush,
    }
    struct Sink(Failure);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            match self.0 {
                Failure::Write => Err(io::ErrorKind::BrokenPipe.into()),
                Failure::Flush => Ok(bytes.len()),
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn write_and_flush_failure_keep_known_result_without_claiming_delivery() {
        for format in [ReportFormat::Text, ReportFormat::Jsonl] {
            for failure_at in [Failure::Write, Failure::Flush] {
                let mut fields = base("restore", "published", "input");
                fields.push(("outcome".into(), Json::Str("complete".into())));
                fields.push(("instance".into(), Json::Str("01".repeat(16))));
                let expected = encode(&Json::Object(fields.clone()));
                let mut diagnostic = Vec::new();
                assert_eq!(
                    deliver(
                        &mut Sink(failure_at),
                        &mut diagnostic,
                        fields.clone(),
                        format
                    )
                    .unwrap_err()
                    .kind(),
                    io::ErrorKind::BrokenPipe
                );
                assert!(
                    String::from_utf8(diagnostic)
                        .unwrap()
                        .lines()
                        .any(|line| line == expected)
                );
                assert_eq!(
                    deliver(
                        &mut Sink(failure_at),
                        &mut Sink(Failure::Write),
                        fields,
                        format
                    )
                    .unwrap_err()
                    .kind(),
                    io::ErrorKind::BrokenPipe
                );
            }
        }
    }
}
