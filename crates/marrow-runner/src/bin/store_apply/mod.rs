//! Explicit-artifact apply has no project capture, listener or service result.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use marrow_image::CeilingId;
use marrow_lifecycle::ApplyError;
use marrow_local_wire::{Id32, Json, encode};

use super::{ReportFormat, load_image, validate_store_output, write_receipt};

pub(super) struct Command {
    old: PathBuf,
    new: PathBuf,
    store: PathBuf,
    accepted: Option<CeilingId>,
    format: ReportFormat,
}

pub(super) fn parse(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let (mut old, mut new, mut store, mut accepted, mut format) = (None, None, None, None, None);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--old-image" if old.is_none() => old = Some(PathBuf::from(args.next()?)),
            "--new-image" if new.is_none() => new = Some(PathBuf::from(args.next()?)),
            "--store" if store.is_none() => store = Some(PathBuf::from(args.next()?)),
            "--accept-ceiling" if accepted.is_none() => {
                accepted = Some(CeilingId::from_bytes(
                    *Id32::from_hex(&args.next()?)?.bytes(),
                ));
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
    Some(Command {
        old: old?,
        new: new?,
        store: store?,
        accepted,
        format: format.unwrap_or(ReportFormat::Text),
    })
}

pub(super) fn run(command: Command) -> io::Result<ExitCode> {
    validate_store_output(&command.store)?;
    // Each loader drops its raw buffer before the next artifact is read.
    let old = match load_image(&command.old) {
        Ok(image) => marrow_lifecycle::prepare(image),
        Err(code) => return Ok(code),
    };
    let new = match load_image(&command.new) {
        Ok(image) => marrow_lifecycle::prepare(image),
        Err(code) => return Ok(code),
    };
    let result = marrow_lifecycle::apply(&command.store, old, new, command.accepted);
    let mut fields = vec![("kind".into(), Json::Str("apply".into()))];
    let text = |value: String| Json::Str(value);
    match &result {
        Ok(receipt) => fields.extend([
            ("outcome".into(), text("applied".into())),
            ("instance".into(), text(receipt.instance.to_hex())),
            ("old_image".into(), text(receipt.old_image.to_hex())),
            ("new_image".into(), text(receipt.new_image.to_hex())),
            ("old_ceiling".into(), text(receipt.old_ceiling.to_hex())),
            ("ceiling".into(), text(receipt.ceiling.to_hex())),
        ]),
        Err(error) => {
            let outcome = match error {
                ApplyError::Lifecycle(marrow_lifecycle::LifecycleError::ActivationUncertain {
                    ..
                }) => "activation_uncertain",
                ApplyError::Lifecycle(marrow_lifecycle::LifecycleError::Metadata(_)) => {
                    "metadata_failed"
                }
                _ => "refused",
            };
            fields.push(("outcome".into(), text(outcome.into())));
            fields.push(("code".into(), text(error.code().into())));
            if let ApplyError::Lifecycle(marrow_lifecycle::LifecycleError::ActivationUncertain {
                instance,
                ..
            }) = error
            {
                fields.push(("instance".into(), text(instance.to_hex())));
            }
            if let ApplyError::CeilingUnaccepted {
                old,
                proposed,
                added,
            } = error
            {
                fields.push(("old_ceiling".into(), text(old.to_hex())));
                fields.push(("ceiling".into(), text(proposed.to_hex())));
                fields.push((
                    "added_effects".into(),
                    Json::Array(
                        added
                            .iter()
                            .map(|effect| {
                                Json::Object(vec![
                                    ("export".into(), text(effect.export.clone())),
                                    ("effect".into(), text(effect.effect.word().into())),
                                    (
                                        "place".into(),
                                        effect.place.clone().map_or(Json::Null, text),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ));
            }
            let _ = writeln!(io::stderr(), "{}: {error}", error.code());
        }
    }
    deliver(
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        &Json::Object(fields),
        command.format,
    )?;
    Ok(if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn deliver(
    output: &mut dyn Write,
    diagnostic: &mut dyn Write,
    report: &Json,
    format: ReportFormat,
) -> io::Result<()> {
    let receipt = encode(report);
    let delivery = match format {
        ReportFormat::Jsonl => write_receipt(output, &receipt),
        ReportFormat::Text => {
            let Json::Object(fields) = report else {
                unreachable!("object constructed above")
            };
            let rendered = fields
                .iter()
                .map(|(key, value)| format!("{key}: {}", encode(value)))
                .collect::<Vec<_>>()
                .join("\n");
            write_receipt(output, &rendered)
        }
    };
    if delivery.is_err() {
        let _ = write_receipt(diagnostic, &receipt);
    }
    delivery
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenOutput {
        fail_write: bool,
    }

    impl Write for BrokenOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(bytes.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn receipt_delivery_failure_retains_the_known_outcome() {
        for outcome in [
            "applied",
            "activation_uncertain",
            "metadata_failed",
            "refused",
        ] {
            let report = Json::Object(vec![
                ("kind".into(), Json::Str("apply".into())),
                ("outcome".into(), Json::Str(outcome.into())),
            ]);
            for format in [ReportFormat::Text, ReportFormat::Jsonl] {
                for fail_write in [true, false] {
                    let mut diagnostic = Vec::new();
                    let error = deliver(
                        &mut BrokenOutput { fail_write },
                        &mut diagnostic,
                        &report,
                        format,
                    )
                    .expect_err("write or flush fails");
                    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                    assert_eq!(diagnostic, format!("{}\n", encode(&report)).as_bytes());
                }
            }
        }
    }
}
