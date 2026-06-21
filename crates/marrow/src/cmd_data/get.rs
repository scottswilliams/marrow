use std::process::ExitCode;

use marrow_check::parse_path;
use marrow_check::tooling::{
    DataPresence, DataReadResult, StampedData, ToolingError, render_data_path_value,
    resolve_source_text_data_path, stamped_read_data_path,
};
use serde_json::json;

use crate::{CheckFormat, write_json};

pub(super) fn data_get(args: &[String]) -> ExitCode {
    let (dir, path_text, format, backup) =
        match super::dir_and_path_args_with_backup("data get", "path", args) {
            Ok(parsed) => parsed,
            Err(code) => return code,
        };
    let parsed_segments = match parse_path(&path_text) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("marrow data get: {}", error.message);
            return ExitCode::from(2);
        }
    };
    let super::DataReadTarget { program, store, .. } =
        match super::load_data_read_target(dir, format, backup) {
            Ok(target) => target,
            Err(code) => return code,
        };
    let path = match resolve_source_text_data_path(&program, &parsed_segments) {
        Ok(Some(path)) => path,
        // Durable identity that was never committed — a never-run project or a
        // pending member — has no stored value, so the read is absent, not a fault.
        Ok(None) => {
            match format {
                CheckFormat::Text => println!("(absent)"),
                CheckFormat::Json | CheckFormat::Jsonl => write_json(json!({
                    "path": path_text,
                    "presence": DataPresence::Absent.as_label(),
                    "value_b64": serde_json::Value::Null,
                    "store_snapshot": serde_json::Value::Null,
                })),
            }
            return ExitCode::SUCCESS;
        }
        // A malformed path is a usage error; a corrupt checked catalog id is a
        // store fault and must report under the store code, not as usage.
        Err(ToolingError::Path(error)) => {
            eprintln!("marrow data get: {error}");
            return ExitCode::from(2);
        }
        Err(ToolingError::Store(error)) => return super::report_store_error(error, format),
    };
    let (result, store_snapshot) = match &store {
        Some(store) => match stamped_read_data_path(&program, store, &path) {
            Ok(StampedData { data, stamp }) => (data, Some(stamp)),
            Err(error) => return super::report_store_error(error, format),
        },
        None => (
            DataReadResult {
                payload: None,
                presence: DataPresence::Absent,
            },
            None,
        ),
    };
    match format {
        CheckFormat::Text => match &result.payload {
            Some(payload) => println!(
                "{}",
                render_data_path_value(&program, &path, payload.as_bytes())
            ),
            None => match result.presence {
                DataPresence::ChildrenOnly => println!("(no value; has children)"),
                _ => println!("(absent)"),
            },
        },
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "path": path.path(),
                "presence": result.presence.as_label(),
                "value_b64": result
                    .payload
                    .as_ref()
                    .map(|payload| marrow_run::base64::encode(payload.as_bytes())),
                "store_snapshot": store_snapshot
                    .as_ref()
                    .map(marrow_json::data_generation_stamp_to_json),
            }));
        }
    }
    ExitCode::SUCCESS
}
