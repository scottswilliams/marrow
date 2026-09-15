//! Explicit-artifact apply has no project capture, listener or service result.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use marrow_image::CeilingId;
use marrow_lifecycle::ApplyError;
use marrow_local_wire::{Id32, Json};

use super::{
    ReportFormat, SharedFlag, deliver, load_image, shared_store_flag, validate_store_output,
};

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
        if let SharedFlag::Taken = shared_store_flag(&flag, &mut args, &mut store, &mut format)? {
            continue;
        }
        match flag.as_str() {
            "--old-image" if old.is_none() => old = Some(PathBuf::from(args.next()?)),
            "--new-image" if new.is_none() => new = Some(PathBuf::from(args.next()?)),
            "--accept-ceiling" if accepted.is_none() => {
                accepted = Some(CeilingId::from_bytes(
                    *Id32::from_hex(&args.next()?)?.bytes(),
                ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Failure, Sink};
    use marrow_local_wire::encode;

    fn receipt(outcome: &str) -> Json {
        Json::Object(vec![
            ("kind".into(), Json::Str("apply".into())),
            ("outcome".into(), Json::Str(outcome.into())),
        ])
    }

    /// Text output is the default, so its spelling is part of the command's contract: one
    /// `key: value` line per field, strings unquoted, structured values in JSON.
    #[test]
    fn text_output_spells_one_line_per_field_without_quoting_strings() {
        let report = Json::Object(vec![
            ("kind".into(), Json::Str("apply".into())),
            (
                "outcome".into(),
                Json::Str("ceiling_unaccepted_outcome".into()),
            ),
            ("code".into(), Json::Str("store.ceiling_unaccepted".into())),
            ("old_ceiling".into(), Json::Str("ab".repeat(32))),
            (
                "added_effects".into(),
                Json::Array(vec![Json::Object(vec![
                    ("export".into(), Json::Str("main.bump".into())),
                    ("effect".into(), Json::Str("write".into())),
                    ("place".into(), Json::Null),
                ])]),
            ),
        ]);
        let mut rendered = Vec::new();
        deliver(&mut rendered, &mut Vec::new(), &report, ReportFormat::Text)
            .expect("text delivery");
        assert_eq!(
            String::from_utf8(rendered).expect("UTF-8"),
            format!(
                "kind: apply\n\
                 outcome: ceiling_unaccepted_outcome\n\
                 code: store.ceiling_unaccepted\n\
                 old_ceiling: {}\n\
                 added_effects: [{{\"effect\":\"write\",\"export\":\"main.bump\",\"place\":null}}]\n",
                "ab".repeat(32)
            )
        );
    }

    #[test]
    fn receipt_delivery_failure_retains_the_known_outcome() {
        for outcome in [
            "applied",
            "activation_uncertain",
            "metadata_failed",
            "refused",
        ] {
            let report = receipt(outcome);
            for format in [ReportFormat::Text, ReportFormat::Jsonl] {
                for failure in [Failure::WriteAt(0), Failure::Flush] {
                    let mut diagnostic = Vec::new();
                    let error = deliver(&mut Sink::new(failure), &mut diagnostic, &report, format)
                        .expect_err("write or flush fails");
                    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                    assert!(
                        String::from_utf8(diagnostic)
                            .expect("UTF-8")
                            .lines()
                            .any(|line| line == encode(&report))
                    );
                }
            }
        }
    }
}
