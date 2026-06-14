use std::process::ExitCode;

use marrow_store::tree::TreeStore;

use crate::{CheckFormat, open_store_for_inspection, report_simple_error, resolve_store_path};

pub(super) fn preview_store(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<TreeStore, ExitCode> {
    Ok(match open_store_for_inspection(dir, config, format)? {
        Some(store) => store,
        None => TreeStore::memory(),
    })
}

pub(super) fn apply_store(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<TreeStore, ExitCode> {
    match resolve_store_path(dir, config, format)? {
        Some(path) => {
            let store = match TreeStore::open(&path) {
                Ok(store) => store,
                Err(error) => {
                    report_simple_error(error.code(), &error.to_string(), format);
                    return Err(ExitCode::FAILURE);
                }
            };
            Ok(store)
        }
        None => Ok(TreeStore::memory()),
    }
}
