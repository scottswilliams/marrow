use std::fs;
use std::path::PathBuf;

fn src_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(relative)
}

#[test]
fn saved_place_shape_has_single_executable_owner() {
    let obsolete_patterns = [
        ("infer.rs", "fn is_saved_path_expression"),
        ("infer.rs", "fn is_saved_path_callee"),
        ("infer.rs", "fn saved_leaf_type"),
        ("infer.rs", "fn singleton_saved_leaf_type"),
        ("infer.rs", "fn saved_index_identity_type"),
        ("infer.rs", "fn saved_resource_type"),
        ("infer.rs", "fn saved_group_entry_type"),
        ("infer.rs", "fn saved_layer_chain"),
        ("checks/collections.rs", "fn saved_layer_chain"),
        ("checks/collections.rs", "fn saved_leaf_type"),
        ("checks/collections.rs", "fn saved_group_entry_type"),
        ("checks/saved_keys.rs", "fn saved_layer_chain"),
        ("checks/calls.rs", "fn is_exists_target_arg"),
        ("binding.rs", "fn saved_member_ref"),
        ("binding.rs", "fn saved_layer_base"),
        ("presence/keys.rs", "struct SavedPathParts"),
        ("presence/keys.rs", "fn saved_path_parts"),
        ("rejected_surface.rs", "fn declared_saved_member_or_index"),
        ("rejected_surface.rs", "fn member_chain_after_root"),
        ("checks/saved_keys.rs", "fn checked_index_key_type"),
        ("checks/saved_keys.rs", "fn checked_key_param_type"),
        ("presence/target.rs", "fn saved_path_parts"),
        ("presence/target.rs", "fn resolve_store_by_root"),
        ("presence/target.rs", "fn node_for_path"),
    ];

    let mut findings = Vec::new();
    for (file, pattern) in obsolete_patterns {
        let path = src_path(file);
        let source = fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("failed to read {}: {err}", path.display());
        });
        for (line_index, line) in source.lines().enumerate() {
            if line.contains(pattern) {
                findings.push(format!("{file}:{} contains `{pattern}`", line_index + 1));
            }
        }
    }

    assert!(
        findings.is_empty(),
        "obsolete saved-place classifier family still exists:\n{}",
        findings.join("\n")
    );
}
