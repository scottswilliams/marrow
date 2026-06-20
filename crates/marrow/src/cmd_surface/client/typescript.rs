use std::path::Path;
use std::process::ExitCode;

use crate::{CheckFormat, report_simple_error};

const HELP: &str = "\
Usage:
  marrow surface client typescript <projectdir>

Generate a self-contained TypeScript client from the checked surface ABI.
";

pub(crate) fn typescript(args: &[String]) -> ExitCode {
    let mut target = None;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                return crate::unknown_option("surface client typescript", value);
            }
            value => {
                if let Err(code) = crate::take_single_target(
                    &mut target,
                    value,
                    "surface client typescript",
                    "project directory",
                ) {
                    return code;
                }
            }
        }
    }

    let Some(target) = target else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    if let Err(code) = crate::reject_bare_file_target("surface client typescript", &target) {
        return code;
    }
    render_client(&target)
}

fn render_client(dir: &str) -> ExitCode {
    let config = match crate::load_config_with_format(dir, CheckFormat::Text) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let accepted = match crate::read_accepted_catalog_artifact(dir, CheckFormat::Text) {
        Ok(accepted) => accepted,
        Err(code) => return code,
    };
    let snapshot = match marrow_check::analyze_project(
        Path::new(dir),
        &config,
        &marrow_check::ProjectSources::new(),
        accepted.as_ref(),
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
        Ok(client) => {
            print!("{client}");
            ExitCode::SUCCESS
        }
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
