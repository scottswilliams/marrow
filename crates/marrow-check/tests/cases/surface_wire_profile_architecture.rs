/// The wire surface exposes exactly one operation profile and one route profile. This whole-repo
/// gate fails closed if a structured second-profile identifier reappears in any `.rs`, `.ts`, or
/// `.md` source under `crates/` or `docs/`: a dotted operation or route profile constant, a
/// versioned route prefix, or a second-profile type or constructor name. The needles are assembled
/// at runtime so the scan never matches itself.
#[test]
fn no_second_wire_profile_identifier_survives_in_the_workspace() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let forbidden = [
        format!("surface.operation.{}", "v2"),
        format!("surface.route.{}", "v2"),
        format!("/surface/{}/", "v2"),
        format!("SurfaceOperation{}", "Profile"),
        format!("from_abi_{}", "v2"),
        format!("from_program_{}", "v2"),
    ];
    let mut scanned = 0usize;
    let mut offenders = Vec::new();
    for dir in [repo_root.join("crates"), repo_root.join("docs")] {
        collect_workspace_sources(&dir, &mut |path, text| {
            scanned += 1;
            for needle in &forbidden {
                if text.contains(needle.as_str()) {
                    offenders.push(format!("{} contains `{needle}`", path.display()));
                }
            }
        });
    }
    assert!(scanned > 0, "the workspace source scan reached no files");
    assert!(
        offenders.is_empty(),
        "a second wire profile identifier reappeared:\n{}",
        offenders.join("\n")
    );
}

/// Read every `.rs`, `.ts`, and `.md` file under `dir`, passing each file's path and text to
/// `visit`. Build-output trees never live under the tracked `crates`/`docs` roots, so no target
/// directory is excluded.
fn collect_workspace_sources(dir: &std::path::Path, visit: &mut dyn FnMut(&std::path::Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_workspace_sources(&path, visit);
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("rs" | "ts" | "md")
        ) && let Ok(text) = std::fs::read_to_string(&path)
        {
            visit(&path, &text);
        }
    }
}
