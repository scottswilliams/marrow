use std::process::ExitCode;

use marrow_run::SystemNondeterminism;
use marrow_store::SealedStore;
use marrow_store::tree::TreeStore;

use crate::backup::ensure_store_uid;
use crate::{CheckFormat, open_store_for_inspection, report_simple_error, resolve_store_path};

pub(super) fn preview_store(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<TreeStore, ExitCode> {
    Ok(match open_store_for_inspection(dir, config, format)? {
        Some(store) => store.into_store(),
        None => TreeStore::memory(),
    })
}

/// Open the apply write store, stamping its durable uid before returning. The uid is the
/// store's stable identity; it must already be present before the apply path commits any
/// baseline or catalog, so the `(store_uid, commit_id)` pair that names committed accepted
/// state never has an absent uid — even if the process dies between the baseline commit and a
/// later stamp. This mirrors the run path, which stamps the uid as it opens the write store.
/// Apply is the identity-establishing flow, so it holds the stage-1 [`SealedStore`] and runs
/// its own witness, fence, and activation machinery rather than the run-path admission.
pub(super) fn apply_store(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<TreeStore, ExitCode> {
    match resolve_store_path(dir, config, format)? {
        Some(path) => {
            let store: SealedStore = match marrow_run::admission::open_create(&path) {
                Ok(store) => store,
                Err(error) => {
                    report_simple_error(error.code(), &error.to_string(), format);
                    return Err(ExitCode::FAILURE);
                }
            };
            let mut nondeterminism = SystemNondeterminism::new();
            if let Err(error) = ensure_store_uid(&store, &mut nondeterminism) {
                report_simple_error(error.code(), &error.to_string(), format);
                return Err(ExitCode::FAILURE);
            }
            Ok(store.into_store())
        }
        None => Ok(TreeStore::memory()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::apply_store;
    use crate::CheckFormat;

    /// The apply write store must already carry its durable uid the moment it is opened,
    /// before the apply path establishes any baseline. If the uid were stamped only after the
    /// baseline commit, a crash in that window would leave a durably committed accepted catalog
    /// whose `(store_uid, commit_id)` identity has an absent uid. Opening the store stamps the
    /// uid, so a fresh open already reads `Some`.
    #[test]
    fn apply_store_stamps_uid_before_returning_on_fresh_store() {
        let dir = std::env::temp_dir().join(format!(
            "marrow-apply-store-uid-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).expect("create fixture project dir");
        let config = marrow_project::parse_config(
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        )
        .expect("parse native config");

        let store = apply_store(
            dir.to_str().expect("utf8 fixture dir"),
            &config,
            CheckFormat::Text,
        )
        .expect("open apply store");

        let uid = store.read_store_uid().expect("read store uid");
        assert!(
            uid.is_some(),
            "apply_store must stamp the durable uid before any baseline commit"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
