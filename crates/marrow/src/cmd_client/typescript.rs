use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::cmd_check::CHECK_STALE_LOCK;
use crate::{CheckFormat, ClientFreshness, report_simple_error};

const COMMAND: &str = "client typescript";
const HELP: &str = "\
Usage:
  marrow client typescript [--out <path>] <projectdir>

Generate a self-contained TypeScript client from the checked surface ABI.

Options:
  --out  Write the client to <path>, resolved against the current directory;
         prints the written path. When omitted, refresh the marrow.json
         `client` path if one is declared, otherwise print to stdout.
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

    // A committed lock the source has outrun is the `check.stale_lock` condition: the surface this
    // client projects may not reflect the accepted catalog the lock carries, so emitting one
    // silently would hand the developer a client whose shape they cannot trust. Warn loudly and
    // name the run that re-projects the lock, mirroring the read-only advisory `check` reports.
    if lock
        .as_ref()
        .is_some_and(|lock| lock.source_digest != snapshot.program.source_digest())
    {
        report_simple_error(
            CHECK_STALE_LOCK,
            &format!(
                "marrow.lock is behind the current source, so this client may not reflect the \
                 accepted catalog; run marrow run {dir} to re-project the lock before generating a \
                 client"
            ),
            CheckFormat::Text,
        );
    }

    // An explicit `--out` is the ad-hoc escape hatch: resolve it against the process cwd (POSIX
    // convention) and report the written path. With no `--out`, a declared `client` path refreshes
    // write-if-changed through the shared owner that run, serve, and evolve use, while a project
    // with no declared client falls back to stdout.
    if let Some(out) = out {
        return match render(&snapshot.program) {
            Ok(client) => write_explicit_out(out, &client),
            Err(error) => render_failure(&error),
        };
    }

    if let Some(rel) = config.client.as_deref() {
        let path = Path::new(dir).join(rel);
        let existed = path.exists();
        return match crate::write_declared_client_if_changed(
            dir,
            &config,
            &snapshot.program,
            CheckFormat::Text,
        ) {
            Ok(verdict) => {
                report_declared_refresh(&path, existed, verdict);
                ExitCode::SUCCESS
            }
            Err(code) => code,
        };
    }

    match render(&snapshot.program) {
        Ok(client) => {
            print!("{client}");
            ExitCode::SUCCESS
        }
        Err(error) => render_failure(&error),
    }
}

/// Render the typed TypeScript client from a checked program through the surface-ABI render owner.
/// The declared-client refresh has its own write-if-changed owner; this serves the `--out` and
/// stdout paths that emit the freshly rendered text directly.
fn render(
    program: &marrow_check::CheckedProgram,
) -> Result<String, marrow_json::surface::SurfaceClientRenderError> {
    let abi = marrow_json::surface::SurfaceAbiJson::from_program(program);
    let routes = marrow_json::surface::SurfaceRouteManifestJson::from_abi(&abi);
    marrow_json::surface::render_typescript_client(&abi, &routes)
}

/// Report the outcome of refreshing the declared client to stderr, leaving stdout clean. A
/// configured client without a surface reuses the shared surfaceless diagnostic the write owner
/// already raises; otherwise the freshness verdict drives a wrote/updated/unchanged line.
fn report_declared_refresh(path: &Path, existed: bool, verdict: ClientFreshness) {
    match verdict {
        ClientFreshness::Rewritten if existed => eprintln!("updated {}", path.display()),
        ClientFreshness::Rewritten => eprintln!("wrote {}", path.display()),
        ClientFreshness::AlreadyFresh => eprintln!("unchanged {}", path.display()),
        ClientFreshness::SurfacelessConfigured => report_simple_error(
            crate::CLIENT_WITHOUT_SURFACE_CODE,
            crate::CLIENT_WITHOUT_SURFACE_MESSAGE,
            CheckFormat::Text,
        ),
        ClientFreshness::NotConfigured => {}
    }
}

fn render_failure(error: &impl std::fmt::Display) -> ExitCode {
    report_simple_error(
        "surface.abi_mismatch",
        &format!("surface client render failed: {error}"),
        CheckFormat::Text,
    );
    ExitCode::FAILURE
}

/// Write the rendered client to an explicit `--out` path, resolving a relative path against the
/// process cwd as a POSIX CLI does. Success prints the resolved path so the write is never
/// invisible.
fn write_explicit_out(out: &str, client: &str) -> ExitCode {
    let path = PathBuf::from(out);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
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
        Ok(()) => {
            let resolved = path.canonicalize().unwrap_or_else(|_| {
                std::env::current_dir().map_or(path.clone(), |cwd| cwd.join(&path))
            });
            eprintln!("wrote {}", resolved.display());
            ExitCode::SUCCESS
        }
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
