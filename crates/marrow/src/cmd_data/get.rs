use std::process::ExitCode;

use marrow_check::parse_path;
use marrow_check::tooling::{
    DataPresence, data_presence_name, read_data_query, resolve_source_text_data_query,
};
use serde_json::json;

use crate::{CheckFormat, load_checked_project, write_json};

use super::render_value_bytes;

pub(super) fn data_get(args: &[String]) -> ExitCode {
    let (dir, path_text, format) = match crate::dir_and_path_args("data get", "path", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let parsed_segments = match parse_path(&path_text) {
        Ok(segments) => segments,
        Err(error) => {
            eprintln!("marrow data get: {}", error.message);
            return ExitCode::from(2);
        }
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let query = match resolve_source_text_data_query(&program, &parsed_segments) {
        Ok(query) => query,
        Err(message) => {
            eprintln!("marrow data get: {message}");
            return ExitCode::from(2);
        }
    };
    let store = match super::open_tree_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let (value, presence) = match &store {
        Some(store) => match read_data_query(store, &query) {
            Ok(result) => result,
            Err(error) => return super::report_store_error(error, format),
        },
        None => (None, DataPresence::Absent),
    };
    match format {
        CheckFormat::Text => match &value {
            Some(payload) => println!("{}", render_value_bytes(payload.as_bytes())),
            None => match presence {
                DataPresence::ChildrenOnly => println!("(no value; has children)"),
                _ => println!("(absent)"),
            },
        },
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "path": query.path(),
                "presence": data_presence_name(presence),
                "value_b64": value
                    .as_ref()
                    .map(|payload| marrow_run::base64::encode(payload.as_bytes())),
            }));
        }
    }
    ExitCode::SUCCESS
}
