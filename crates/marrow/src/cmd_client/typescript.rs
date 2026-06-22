use std::fs;
use std::path::Path;
use std::process::ExitCode;

use crate::{CheckFormat, report_simple_error};

const COMMAND: &str = "client typescript";
const HELP: &str = "\
Usage:
  marrow client typescript [--out <path>] <projectdir>

Generate a self-contained TypeScript client from the checked surface ABI.

Options:
  --out  Write the client to <path>; prints to stdout when omitted.
";

pub(crate) fn typescript(args: &[String]) -> ExitCode {
    let mut target = None;
    let mut out: Option<String> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--out" => {
                let Some(value) = args.next() else {
                    eprintln!("marrow {COMMAND} --out requires a path");
                    return ExitCode::from(2);
                };
                if out.replace(value.clone()).is_some() {
                    eprintln!("marrow {COMMAND} accepts one --out path");
                    return ExitCode::from(2);
                }
            }
            value if value.starts_with('-') => {
                return crate::unknown_option(COMMAND, value);
            }
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut target, value, COMMAND, "project directory")
                {
                    return code;
                }
            }
        }
    }

    let Some(target) = target else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    if let Err(code) = crate::reject_bare_file_target(COMMAND, &target) {
        return code;
    }
    render_client(&target, out.as_deref())
}

fn render_client(dir: &str, out: Option<&str>) -> ExitCode {
    let config = match crate::load_config_with_format(dir, CheckFormat::Text) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let lock = match crate::read_committed_lock(dir, CheckFormat::Text) {
        Ok(lock) => lock,
        Err(code) => return code,
    };
    // Match `check`'s accepted-authority binding: a readable live store owns the accepted
    // surface contract, while the committed lock drives first-run or unreadable-store projection.
    let authority = crate::read_accepted_store_catalog_lenient(dir, &config);
    let snapshot = match marrow_check::analyze_project(
        Path::new(dir),
        &config,
        &marrow_check::ProjectSources::new(),
        authority.snapshot(),
        lock.as_ref(),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            report_simple_error(
                error.code,
                &format!("{}: {}", error.path.display(), error.message),
                CheckFormat::Text,
            );
            return ExitCode::FAILURE;
        }
    };

    if snapshot.report.has_errors() {
        crate::report_project(dir, &snapshot.report, CheckFormat::Text);
        return ExitCode::FAILURE;
    }

    let abi = marrow_json::surface::SurfaceAbiJson::from_program(&snapshot.program);
    let routes = marrow_json::surface::SurfaceRouteManifestJson::from_abi(&abi);
    match marrow_json::surface::render_typescript_client(&abi, &routes) {
        Ok(client) => match out {
            Some(out) => write_client(dir, out, &client),
            None => {
                print!("{client}");
                ExitCode::SUCCESS
            }
        },
        Err(error) => {
            report_simple_error(
                "surface.abi_mismatch",
                &format!("surface client render failed: {error}"),
                CheckFormat::Text,
            );
            ExitCode::FAILURE
        }
    }
}

/// Write the rendered client to `out`. A relative path resolves under the
/// project directory; an absolute path is honored as given, the ad-hoc escape
/// hatch for emitting the client outside the project tree.
fn write_client(dir: &str, out: &str, client: &str) -> ExitCode {
    let path = Path::new(out);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(dir).join(path)
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        report_simple_error(
            "io.write",
            &format!("failed to create {}: {error}", parent.display()),
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    }
    match fs::write(&path, client) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report_simple_error(
                "io.write",
                &format!("failed to write {}: {error}", path.display()),
                CheckFormat::Text,
            );
            ExitCode::FAILURE
        }
    }
}
