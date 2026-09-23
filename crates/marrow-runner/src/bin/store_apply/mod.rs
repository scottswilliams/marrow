//! Explicit-artifact apply has no project capture, listener or service result.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use marrow_image::CeilingId;
use marrow_lifecycle::ApplyError;
use marrow_local_wire::{Id32, Json};

use super::{
    Receipt, ReportFormat, SharedFlag, deliver, load_image, shared_store_flag,
    validate_store_output,
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

/// What apply did to the store, decided once from the lifecycle result and rendered as a
/// word only at the output boundary. A failure that may have changed persistent metadata,
/// and one whose activation outcome is unknown, are each distinct from a refusal that
/// published nothing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Applied,
    ActivationUncertain,
    MetadataFailed,
    Refused,
}

impl Outcome {
    fn of(result: &Result<marrow_lifecycle::ApplyReceipt, ApplyError>) -> Self {
        match result {
            Ok(_) => Self::Applied,
            Err(ApplyError::Lifecycle(marrow_lifecycle::LifecycleError::ActivationUncertain {
                ..
            })) => Self::ActivationUncertain,
            Err(ApplyError::Lifecycle(marrow_lifecycle::LifecycleError::Metadata(_))) => {
                Self::MetadataFailed
            }
            Err(_) => Self::Refused,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::ActivationUncertain => "activation_uncertain",
            Self::MetadataFailed => "metadata_failed",
            Self::Refused => "refused",
        }
    }
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
    let receipt = Receipt::kind("apply").text("outcome", Outcome::of(&result).word());
    let receipt = match &result {
        Ok(applied) => receipt
            .text("instance", applied.instance.to_hex())
            .text("old_image", applied.old_image.to_hex())
            .text("new_image", applied.new_image.to_hex())
            .text("old_ceiling", applied.old_ceiling.to_hex())
            .text("ceiling", applied.ceiling.to_hex()),
        Err(error) => {
            let _ = writeln!(io::stderr(), "{}: {error}", error.code().as_str());
            let receipt = receipt.text("code", error.code().as_str());
            match error {
                ApplyError::Lifecycle(marrow_lifecycle::LifecycleError::ActivationUncertain {
                    instance,
                    ..
                }) => receipt.text("instance", instance.to_hex()),
                ApplyError::CeilingUnaccepted {
                    old,
                    proposed,
                    added,
                } => receipt
                    .text("old_ceiling", old.to_hex())
                    .text("ceiling", proposed.to_hex())
                    .field(
                        "added_effects",
                        Json::Array(
                            added
                                .iter()
                                .map(|effect| {
                                    Receipt::default()
                                        .text("export", effect.export.clone())
                                        .text("effect", effect.effect.word())
                                        .field(
                                            "path",
                                            effect.path.clone().map_or(Json::Null, Json::Str),
                                        )
                                        .into_json()
                                })
                                .collect(),
                        ),
                    ),
                _ => receipt,
            }
        }
    };
    deliver(
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        &receipt.into_json(),
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

    fn receipt(outcome: Outcome) -> Json {
        Json::Object(vec![
            ("kind".into(), Json::Str("apply".into())),
            ("outcome".into(), Json::Str(outcome.word().into())),
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
                    ("path".into(), Json::Null),
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
                 added_effects: [{{\"effect\":\"write\",\"export\":\"main.bump\",\"path\":null}}]\n",
                "ab".repeat(32)
            )
        );
    }

    #[test]
    fn receipt_delivery_failure_retains_the_known_outcome() {
        for outcome in [
            Outcome::Applied,
            Outcome::ActivationUncertain,
            Outcome::MetadataFailed,
            Outcome::Refused,
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
