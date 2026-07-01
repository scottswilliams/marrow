use std::collections::BTreeMap;
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use marrow_check::CheckedProgram;
use marrow_json::surface::{
    SurfaceAbiJson, SurfaceOperationCatalog, SurfaceOperationErrorJson,
    SurfaceOperationRequestJson, SurfaceOperationResponseJson, SurfaceRouteBinding,
    SurfaceRouteBindings, SurfaceRouteManifestJson, execute_project_surface_operation,
    execute_project_surface_operation_read_only,
};
use marrow_run::{
    ProjectSessionError, ProjectSurfaceReadSession, ProjectSurfaceSession, ProjectSurfaceSnapshot,
    SURFACE_ABI_MISMATCH, SURFACE_AUTH, SURFACE_STORE,
};

use crate::cmd_run::report_session_open_error;
use crate::term_style::{self, Stream};
use crate::{CheckFormat, report_simple_error};

mod auth;
mod cors;
mod cursor_token;
mod http;
mod shutdown;
use auth::{AuthTokenSource, RemoteAuthToken};
use cors::CorsPolicy;
use cursor_token::{CursorTokenKeySource, RemoteCursorToken};

const DEFAULT_PORT: u16 = 8080;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const STREAM_TIMEOUT: Duration = Duration::from_secs(15);
const READ_POLL_INTERVAL: Duration = Duration::from_millis(250);
const ACCEPT_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy)]
enum ServeMode {
    ReadOnly,
    Write,
}

impl ServeMode {
    fn allows(self, binding: &SurfaceRouteBinding) -> bool {
        match self {
            Self::ReadOnly => binding.kind.is_read(),
            Self::Write => true,
        }
    }
}

const COMMAND: &str = "serve";
const HELP: &str = "\
Usage:
  marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>
  marrow serve --remote --addr <addr> [--write]
    (--auth-token-env NAME | --auth-token-file PATH)
    [--cursor-token-key-id <kid> (--cursor-token-key-env NAME | --cursor-token-key-file PATH)]
    [--remote-cors-origin <origin>] <projectdir>

Run an HTTP surface endpoint. The default profile is loopback-only. The server
accepts one JSON POST per connection and closes the response on descriptor-derived
/surface/v1/{read|create|update|delete|action}/<operation-tag> routes, plus
/surface/v2/read/<operation-tag> range page routes.

  --write  Expose create/update/delete/action routes and open a writable surface session.
           Defaults to read-only mode, serving v1 read routes including computed reads
           and v2 range page read routes.
  --cors-origin
           Allow one exact browser Origin such as http://localhost:5173.
           No CORS headers are emitted unless this option is present.
  --addr   Loopback socket address to bind. Defaults to 127.0.0.1:8080.
  --watch  Re-check and rewrite the declared client on a .mw change, then keep serving.
  --remote
           Allow binding a non-loopback address. Requires explicit --addr and exactly
           one auth token source. --watch is not supported with --remote.
  --auth-token-env NAME
           Read the remote Bearer token from NAME.
  --auth-token-file PATH
           Read the remote Bearer token from a UTF-8 regular file.
  --cursor-token-key-id
           Enable opaque page cursor tokens for the remote profile with this key id.
  --cursor-token-key-env NAME
           Read the remote cursor token key from NAME.
  --cursor-token-key-file PATH
           Read the remote cursor token key from a UTF-8 regular file.
  --remote-cors-origin
           Allow one exact http/https browser Origin for the remote profile.
";

pub(crate) fn serve(args: &[String]) -> ExitCode {
    let mut addr = default_addr();
    let mut mode = ServeMode::ReadOnly;
    let mut cors = None;
    let mut remote_cors = None;
    let mut remote_auth_source = None;
    let mut cursor_token_key_id = None;
    let mut cursor_token_key_source = None;
    let mut saw_addr = false;
    let mut saw_cors_origin = false;
    let mut saw_remote_cors_origin = false;
    let mut saw_cursor_token_flag = false;
    let mut saw_write = false;
    let mut remote = false;
    let mut watch = false;
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--remote" => {
                if remote {
                    eprintln!("duplicate --remote");
                    return ExitCode::from(2);
                }
                remote = true;
            }
            "--write" => {
                if saw_write {
                    eprintln!("duplicate --write");
                    return ExitCode::from(2);
                }
                mode = ServeMode::Write;
                saw_write = true;
            }
            "--watch" => {
                if watch {
                    eprintln!("duplicate --watch");
                    return ExitCode::from(2);
                }
                watch = true;
            }
            "--cors-origin" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --cors-origin");
                    return ExitCode::from(2);
                };
                if saw_cors_origin {
                    eprintln!("duplicate --cors-origin");
                    return ExitCode::from(2);
                }
                cors = match CorsPolicy::local(value) {
                    Ok(cors) => Some(cors),
                    Err(message) => {
                        eprintln!("{message}");
                        return ExitCode::from(2);
                    }
                };
                saw_cors_origin = true;
            }
            "--remote-cors-origin" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --remote-cors-origin");
                    return ExitCode::from(2);
                };
                if saw_remote_cors_origin {
                    eprintln!("duplicate --remote-cors-origin");
                    return ExitCode::from(2);
                }
                remote_cors = match CorsPolicy::remote(value) {
                    Ok(cors) => Some(cors),
                    Err(message) => {
                        eprintln!("{message}");
                        return ExitCode::from(2);
                    }
                };
                saw_remote_cors_origin = true;
            }
            "--auth-token-env" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --auth-token-env");
                    return ExitCode::from(2);
                };
                if remote_auth_source.is_some() {
                    eprintln!("remote serve requires exactly one auth token source");
                    return ExitCode::from(2);
                }
                remote_auth_source = Some(AuthTokenSource::Env(value.clone()));
            }
            "--auth-token-file" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --auth-token-file");
                    return ExitCode::from(2);
                };
                if remote_auth_source.is_some() {
                    eprintln!("remote serve requires exactly one auth token source");
                    return ExitCode::from(2);
                }
                remote_auth_source = Some(AuthTokenSource::File(PathBuf::from(value)));
            }
            "--cursor-token-key-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --cursor-token-key-id");
                    return ExitCode::from(2);
                };
                if cursor_token_key_id.replace(value.clone()).is_some() {
                    eprintln!("duplicate --cursor-token-key-id");
                    return ExitCode::from(2);
                }
                saw_cursor_token_flag = true;
            }
            "--cursor-token-key-env" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --cursor-token-key-env");
                    return ExitCode::from(2);
                };
                if cursor_token_key_source.is_some() {
                    eprintln!(
                        "remote serve cursor token mode requires exactly one cursor token key source"
                    );
                    return ExitCode::from(2);
                }
                cursor_token_key_source = Some(CursorTokenKeySource::Env(value.clone()));
                saw_cursor_token_flag = true;
            }
            "--cursor-token-key-file" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --cursor-token-key-file");
                    return ExitCode::from(2);
                };
                if cursor_token_key_source.is_some() {
                    eprintln!(
                        "remote serve cursor token mode requires exactly one cursor token key source"
                    );
                    return ExitCode::from(2);
                }
                cursor_token_key_source = Some(CursorTokenKeySource::File(PathBuf::from(value)));
                saw_cursor_token_flag = true;
            }
            "--addr" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --addr");
                    return ExitCode::from(2);
                };
                if saw_addr {
                    eprintln!("duplicate --addr");
                    return ExitCode::from(2);
                }
                addr = match value.parse() {
                    Ok(addr) => addr,
                    Err(error) => {
                        eprintln!("invalid --addr: {error}");
                        return ExitCode::from(2);
                    }
                };
                saw_addr = true;
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                return crate::unknown_option(COMMAND, value);
            }
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut dir, value, COMMAND, "project directory")
                {
                    return code;
                }
            }
        }
        index += 1;
    }

    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    if remote {
        if !saw_addr {
            eprintln!("--remote requires an explicit --addr");
            return ExitCode::from(2);
        }
        if watch {
            eprintln!("--remote does not support --watch in this release");
            return ExitCode::from(2);
        }
        if saw_cors_origin {
            eprintln!("--cors-origin is local-only; use --remote-cors-origin with --remote");
            return ExitCode::from(2);
        }
        if saw_cursor_token_flag {
            if cursor_token_key_id.is_none() {
                eprintln!("remote serve cursor token mode requires --cursor-token-key-id");
                return ExitCode::from(2);
            }
            if cursor_token_key_source.is_none() {
                eprintln!(
                    "remote serve cursor token mode requires exactly one cursor token key source"
                );
                return ExitCode::from(2);
            }
        }
    } else {
        if remote_auth_source.is_some() {
            eprintln!("auth token sources require --remote");
            return ExitCode::from(2);
        }
        if saw_remote_cors_origin {
            eprintln!("--remote-cors-origin requires --remote");
            return ExitCode::from(2);
        }
        if saw_cursor_token_flag {
            eprintln!("cursor-token flags require --remote");
            return ExitCode::from(2);
        }
    }
    let remote_auth = if remote {
        let Some(source) = remote_auth_source else {
            eprintln!("--remote requires exactly one auth token source");
            return ExitCode::from(2);
        };
        match RemoteAuthToken::load(&source) {
            Ok(token) => Some(token),
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };
    let remote_cursor_token = if saw_cursor_token_flag {
        match RemoteCursorToken::load(
            cursor_token_key_id
                .as_deref()
                .expect("cursor token key id checked"),
            cursor_token_key_source
                .as_ref()
                .expect("cursor token key source checked"),
        ) {
            Ok(token) => Some(token),
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };
    let cors = if remote {
        remote_cors.as_ref()
    } else {
        cors.as_ref()
    };
    if !remote && !addr.ip().is_loopback() {
        eprintln!("--addr must use a loopback address");
        return ExitCode::from(2);
    }

    let config = match crate::load_config_with_format(&dir, CheckFormat::Text) {
        Ok(config) => config,
        Err(code) => return code,
    };
    // A `dataDir` occupied by a non-directory is the same configuration fault `run` and the
    // inspections classify, so serve guards it first rather than letting either session open leak a
    // raw `ENOTDIR` as a generic `store.io` fault. Both modes open the store, so both are covered.
    if let Err(error) = marrow_check::guard_data_dir(std::path::Path::new(&dir), &config) {
        report_simple_error(error.code(), &error.message(), CheckFormat::Text);
        return ExitCode::FAILURE;
    }
    let shutdown = match shutdown::install() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            report_simple_error(
                "io.signal",
                &format!("failed to install surface shutdown handler: {error}"),
                CheckFormat::Text,
            );
            return ExitCode::FAILURE;
        }
    };
    // A `--write` serve replays an unclean shutdown before it inspects the store, so a store left
    // flagged for recovery by a prior signalled writer with no interrupted commit opens clean
    // rather than refusing the read-only catalog read the snapshot needs.
    if matches!(mode, ServeMode::Write)
        && let Err(error) = marrow_run::recover_store_for_write(std::path::Path::new(&dir), &config)
    {
        return report_session_open_error(&dir, error, CheckFormat::Text);
    }
    if let Some(code) = shutdown_exit_code(&shutdown) {
        return code;
    }
    let snapshot = match ProjectSurfaceSnapshot::open(&dir) {
        Ok(snapshot) => snapshot,
        Err(error) => return report_session_open_error(&dir, error, CheckFormat::Text),
    };
    if let Some(code) = shutdown_exit_code(&shutdown) {
        return code;
    }
    let session = match SurfaceServeSession::open(&snapshot, mode) {
        Ok(session) => session,
        Err(error) => return report_session_open_error(&dir, error, CheckFormat::Text),
    };
    if let Some(code) = shutdown_exit_code(&shutdown) {
        return code;
    }
    // A `--write` serve over a fresh checkout seeds an empty store from the committed lock; announce
    // it loudly at startup, exactly as the run path prints the seed notice, so it is never silent.
    for notice in session.notices() {
        eprintln!("{}", notice.message());
    }
    let routes = match SurfaceRoutes::from_program(session.program(), mode, remote) {
        Ok(routes) => routes,
        Err(message) => {
            report_simple_error(SURFACE_ABI_MISMATCH, &message, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    if let Some(code) = shutdown_exit_code(&shutdown) {
        return code;
    }
    // Startup regenerates the declared client from the opened program so a fresh `serve` always
    // hands clients the surface ABI it will answer over.
    if let Err(code) =
        crate::sync_declared_client(&dir, &config, session.program(), CheckFormat::Text)
    {
        return code;
    }
    if let Some(code) = shutdown_exit_code(&shutdown) {
        return code;
    }
    // The session holds the native store lock for the whole server lifetime: a `--write` serve owns
    // the writer lock and excludes any other writer or inspection, and a read-only serve holds a
    // read-only open that excludes a writer. Requests reuse this held session rather than reopening.
    let executor = SurfaceServeExecutor::new(session);
    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(error) => {
            report_simple_error(
                "io.listen",
                &format!("failed to bind surface server at {addr}: {error}"),
                CheckFormat::Text,
            );
            return ExitCode::FAILURE;
        }
    };
    let bound_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(error) => {
            report_simple_error(
                "io.listen",
                &format!("failed to read surface server address: {error}"),
                CheckFormat::Text,
            );
            return ExitCode::FAILURE;
        }
    };
    println!("serve listening on http://{bound_addr}");
    if let Err(error) = std::io::stdout().flush() {
        report_simple_error(
            "io.write",
            &format!("failed to write surface server status: {error}"),
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    }

    let watch = watch.then(|| {
        let signature = source_signature(&dir, &config).unwrap_or(0);
        SurfaceWatch {
            dir,
            config,
            mode,
            remote,
            signature,
        }
    });
    let server_http = SurfaceServerHttp {
        cors,
        remote_auth: remote_auth.as_ref(),
        remote_cursor_token: remote_cursor_token.as_ref(),
    };
    run_server(listener, executor, routes, server_http, watch, &shutdown)
}

struct SurfaceServerHttp<'a> {
    cors: Option<&'a CorsPolicy>,
    remote_auth: Option<&'a RemoteAuthToken>,
    remote_cursor_token: Option<&'a RemoteCursorToken>,
}

/// The state `serve --watch` re-checks on a source change: where to re-open the snapshot, the
/// declared config to rewrite the client against, the session mode to re-open under, and the last
/// observed source signature. A change in the signature triggers a re-open, a client rewrite, and a
/// swap of the live executor and routes.
struct SurfaceWatch {
    dir: String,
    config: marrow_project::ProjectConfig,
    mode: ServeMode,
    remote: bool,
    signature: u64,
}

/// A cheap signature over the project's source-root `.mw` files: the set of paths plus each file's
/// modification time. A change to any tracked source — content saved, file added, file removed —
/// moves the signature, which is all `--watch` needs to decide whether to re-check. It deliberately
/// does not read file bodies; mtime is enough to detect an edit between poll cycles.
fn source_signature(dir: &str, config: &marrow_project::ProjectConfig) -> std::io::Result<u64> {
    use std::hash::{Hash, Hasher};

    let modules = marrow_project::discover_modules(std::path::Path::new(dir), config)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for module in &modules {
        module.path.hash(&mut hasher);
        let modified = std::fs::metadata(&module.path)?.modified()?;
        modified.hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn default_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))
}

/// The cadence at which a `--watch` server recomputes the source signature between accepts. Short
/// enough that an editor save reflects quickly, long enough not to busy-poll the filesystem.
const WATCH_POLL: Duration = Duration::from_millis(200);

fn run_server(
    listener: TcpListener,
    mut executor: SurfaceServeExecutor,
    mut routes: SurfaceRoutes,
    server_http: SurfaceServerHttp<'_>,
    mut watch: Option<SurfaceWatch>,
    shutdown: &shutdown::Shutdown,
) -> ExitCode {
    if let Err(error) = listener.set_nonblocking(true) {
        eprintln!(
            "{}",
            server_code_message(
                "io.listen",
                format!("failed to set surface accept poll: {error}")
            )
        );
        return ExitCode::FAILURE;
    }
    let mut next_watch = Instant::now();
    loop {
        if let Some(code) = shutdown_exit_code(shutdown) {
            return code;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(code) = shutdown_exit_code(shutdown) {
                    return code;
                }
                http::serve_connection(
                    stream,
                    &executor,
                    &routes,
                    server_http.cors,
                    server_http.remote_auth,
                    server_http.remote_cursor_token,
                    shutdown,
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(code) = shutdown_exit_code(shutdown) {
                    return code;
                }
                if let Some(watch) = watch.as_mut() {
                    let now = Instant::now();
                    if now >= next_watch {
                        watch.poll(&mut executor, &mut routes);
                        next_watch = now + WATCH_POLL;
                    }
                }
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(error) => {
                eprintln!(
                    "{}",
                    server_code_message(
                        "io.listen",
                        format!("failed to accept surface connection: {error}")
                    )
                );
            }
        }
    }
}

fn shutdown_exit_code(shutdown: &shutdown::Shutdown) -> Option<ExitCode> {
    shutdown.requested().map(signal_exit_code)
}

fn signal_exit_code(signal: i32) -> ExitCode {
    ExitCode::from(128 + signal as u8)
}

fn server_code_message(code: &str, message: impl std::fmt::Display) -> String {
    term_style::code_message(Stream::Stderr, code, message)
}

impl SurfaceWatch {
    /// On a changed source signature, re-check the project, rewrite the declared client, and swap in
    /// the fresh session and routes so the next request answers over the updated surface. The held
    /// session — and the store lock with it — is released before re-checking, because re-checking
    /// re-opens the store and the same process cannot hold the lock against itself; the lock is
    /// re-acquired as part of the swap. A re-check that fails logs a tooling error; the next genuine
    /// edit retriggers it.
    fn poll(&mut self, executor: &mut SurfaceServeExecutor, routes: &mut SurfaceRoutes) {
        let signature = match source_signature(&self.dir, &self.config) {
            Ok(signature) => signature,
            Err(error) => {
                eprintln!(
                    "{}",
                    server_code_message(
                        SURFACE_STORE,
                        format!("failed to read project sources for watch: {error}")
                    )
                );
                return;
            }
        };
        if signature == self.signature {
            return;
        }
        // Advance past the change before attempting the re-check so a source that fails to check is
        // not re-attempted every cadence; the next genuine edit retriggers it.
        self.signature = signature;
        if let Err(message) = self.recheck(executor, routes) {
            eprintln!(
                "{}",
                server_code_message(
                    SURFACE_ABI_MISMATCH,
                    format!("surface re-check failed: {message}")
                )
            );
        }
    }

    fn recheck(
        &self,
        executor: &mut SurfaceServeExecutor,
        routes: &mut SurfaceRoutes,
    ) -> Result<(), String> {
        executor.release();
        let snapshot = ProjectSurfaceSnapshot::open(&self.dir)
            .map_err(|error| format!("{}: {}", error.code(), error.message()))?;
        let fresh_routes = SurfaceRoutes::from_program(snapshot.program(), self.mode, self.remote)?;
        crate::sync_declared_client(
            &self.dir,
            &self.config,
            snapshot.program(),
            CheckFormat::Text,
        )
        .map_err(|_| "declared client rewrite failed".to_string())?;
        executor
            .reopen(&snapshot, self.mode)
            .map_err(|error| format!("{}: {}", error.code(), error.message()))?;
        *routes = fresh_routes;
        Ok(())
    }
}

enum SurfaceServeSession {
    ReadOnly(Box<ProjectSurfaceReadSession>),
    Write(Box<ProjectSurfaceSession>),
}

/// Owns the surface session that holds the native store lock for the server's process lifetime, so
/// every request reuses one open session rather than re-opening (and re-locking) per request. A
/// `--watch` re-check must `release` the session before re-opening the store, then `reopen` once the
/// re-check succeeds; between the two the session is absent and a request reports `surface.store`.
struct SurfaceServeExecutor {
    session: Option<SurfaceServeSession>,
}

impl SurfaceServeExecutor {
    fn new(session: SurfaceServeSession) -> Self {
        Self {
            session: Some(session),
        }
    }

    fn execute(
        &self,
        operation: &SurfaceOperationRequestJson,
    ) -> Result<SurfaceOperationResponseJson, SurfaceOperationErrorJson> {
        let Some(session) = self.session.as_ref() else {
            return Err(surface_error(
                SURFACE_STORE,
                "surface session is not open after a failed re-check",
            ));
        };
        session.execute(operation)
    }

    /// Drop the held session, releasing the native store lock. A re-check re-opens the store, which
    /// the same process cannot do while still holding the lock, so the session is released first.
    fn release(&mut self) {
        self.session = None;
    }

    fn reopen(
        &mut self,
        snapshot: &ProjectSurfaceSnapshot,
        mode: ServeMode,
    ) -> Result<(), ProjectSessionError> {
        self.session = Some(SurfaceServeSession::open(snapshot, mode)?);
        Ok(())
    }
}

impl SurfaceServeSession {
    fn open(
        snapshot: &ProjectSurfaceSnapshot,
        mode: ServeMode,
    ) -> Result<Self, ProjectSessionError> {
        match mode {
            ServeMode::ReadOnly => snapshot.open_read_only().map(Box::new).map(Self::ReadOnly),
            ServeMode::Write => snapshot.open_write().map(Box::new).map(Self::Write),
        }
    }

    fn program(&self) -> &CheckedProgram {
        match self {
            Self::ReadOnly(session) => session.program(),
            Self::Write(session) => session.program(),
        }
    }

    /// Notices raised while opening the session. A read-only serve never seeds, so only a
    /// `--write` serve carries the `SeededFromCommittedLock` notice for a fresh checkout.
    fn notices(&self) -> &[marrow_run::ProjectSessionNotice] {
        match self {
            Self::ReadOnly(_) => &[],
            Self::Write(session) => session.notices(),
        }
    }

    fn execute(
        &self,
        operation: &SurfaceOperationRequestJson,
    ) -> Result<SurfaceOperationResponseJson, SurfaceOperationErrorJson> {
        match self {
            Self::ReadOnly(session) => {
                execute_project_surface_operation_read_only(session, operation)
            }
            Self::Write(session) => execute_project_surface_operation(session, operation),
        }
    }
}

struct SurfaceRoutes {
    routes: BTreeMap<String, SurfaceRouteBinding>,
    mode: ServeMode,
    remote: bool,
}

impl SurfaceRoutes {
    fn from_program(
        program: &CheckedProgram,
        mode: ServeMode,
        remote: bool,
    ) -> Result<Self, String> {
        let abi_v1 = SurfaceAbiJson::from_program(program);
        let manifest_v1 = SurfaceRouteManifestJson::from_abi(&abi_v1);
        let catalog_v1 =
            SurfaceOperationCatalog::from_abi(&abi_v1).map_err(|error| error.to_string())?;
        let bindings_v1 = SurfaceRouteBindings::from_manifest(&manifest_v1, &catalog_v1)
            .map_err(|error| error.to_string())?;
        let mut routes = bindings_v1
            .iter()
            .filter(|binding| remote || mode.allows(binding))
            .map(|binding| (binding.path.clone(), binding.clone()))
            .collect::<BTreeMap<_, _>>();

        let abi_v2 = SurfaceAbiJson::from_program_v2(program);
        let manifest_v2 = SurfaceRouteManifestJson::from_abi_v2(&abi_v2);
        let catalog_v2 =
            SurfaceOperationCatalog::from_abi_v2(&abi_v2).map_err(|error| error.to_string())?;
        let bindings_v2 = SurfaceRouteBindings::from_manifest(&manifest_v2, &catalog_v2)
            .map_err(|error| error.to_string())?;
        for binding in bindings_v2
            .iter()
            .filter(|binding| remote || mode.allows(binding))
        {
            routes.insert(binding.path.clone(), binding.clone());
        }
        Ok(Self {
            routes,
            mode,
            remote,
        })
    }

    fn binding_for_path(&self, path: &str) -> Option<&SurfaceRouteBinding> {
        self.routes.get(path)
    }

    fn mode_allows(&self, binding: &SurfaceRouteBinding) -> bool {
        self.mode.allows(binding)
    }
}

fn surface_error(code: &str, message: &str) -> SurfaceOperationErrorJson {
    SurfaceOperationErrorJson {
        code: code.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use marrow_check::{ProjectConfig, StoreBackend, StoreConfig, check_project};
    use marrow_json::surface::SurfaceOperationKind;
    use marrow_store::tree::TreeStore;

    const RANGE_SURFACE: &str = "\
module test

resource Post
    required title: string
    required category: string
    required publishedOn: date
store ^posts(id: int): Post
    index byCategoryDate(category, publishedOn, id)

surface Posts from ^posts
    fields title, category, publishedOn
    collection ^posts.byCategoryDate as byCategoryDate
    collection ^posts.byCategoryDate range as byCategoryDateRange
";

    #[test]
    fn server_code_message_styles_the_code() {
        assert_eq!(
            term_style::Palette::for_test(true)
                .code_message(SURFACE_ABI_MISMATCH, "surface re-check failed"),
            "\x1b[36msurface.abi_mismatch\x1b[0m: surface re-check failed"
        );
    }

    #[test]
    fn serve_routes_mount_v1_existing_routes_and_v2_range_routes() {
        let project = TempProject::new("marrow-serve-range-routes");
        fs::create_dir(project.path().join("src")).expect("create src");
        fs::write(project.path().join("src/test.mw"), RANGE_SURFACE).expect("write source");
        let config = ProjectConfig {
            source_roots: vec!["src".into()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Native,
                data_dir: Some(".marrow/data".into()),
            },
            tests: Vec::new(),
            client: None,
        };
        let (report, program) = check_project(project.path(), &config).expect("check project");
        assert!(
            !report.has_errors(),
            "route fixture checks cleanly: {:#?}",
            report.diagnostics
        );
        let program = commit_catalog(project.path(), &config, program);

        let routes =
            SurfaceRoutes::from_program(&program, ServeMode::ReadOnly, false).expect("routes");
        let paths = routes.routes.keys().cloned().collect::<Vec<_>>();
        assert!(
            paths
                .iter()
                .any(|path| path.starts_with("/surface/v1/read/")),
            "v1 read route is mounted: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path.starts_with("/surface/v2/read/")),
            "v2 range route is mounted: {paths:?}"
        );
        assert!(
            !paths
                .iter()
                .any(|path| path.starts_with("/surface/v2/update/")),
            "serve only adds v2 range read routes: {paths:?}"
        );
        let range = routes
            .routes
            .values()
            .find(|binding| binding.alias == "byCategoryDateRange")
            .expect("range binding");
        assert_eq!(range.kind, SurfaceOperationKind::RangePage);
        assert_eq!(
            range.operation_profile,
            marrow_json::surface::SurfaceOperationProfile::V2
        );
        let exact = routes
            .routes
            .values()
            .find(|binding| binding.alias == "byCategoryDate")
            .expect("exact binding");
        assert_eq!(exact.kind, SurfaceOperationKind::Page);
        assert_eq!(
            exact.operation_profile,
            marrow_json::surface::SurfaceOperationProfile::V1
        );
    }

    fn commit_catalog(
        root: &Path,
        config: &ProjectConfig,
        program: CheckedProgram,
    ) -> CheckedProgram {
        let store = TreeStore::memory();
        if !marrow_run::evolution::commit_catalog_baseline(&store, &program)
            .expect("commit catalog baseline")
        {
            return program;
        }
        let accepted = store
            .read_catalog_snapshot()
            .expect("read catalog snapshot");
        let (report, program) =
            marrow_check::check_project_with_catalog(root, config, accepted.as_ref())
                .expect("re-check with accepted catalog");
        assert!(
            !report.has_errors(),
            "accepted route fixture checks cleanly: {:#?}",
            report.diagnostics
        );
        program
    }

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(prefix: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
            fs::create_dir(&root).expect("create temp project");
            Self { root }
        }

        fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
