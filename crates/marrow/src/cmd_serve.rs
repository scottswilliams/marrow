use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use marrow_check::CheckedProgram;
use marrow_json::surface::{
    SurfaceAbiJson, SurfaceOperationCatalog, SurfaceOperationErrorJson,
    SurfaceOperationRequestJson, SurfaceOperationResponseJson, SurfaceRouteBinding,
    SurfaceRouteBindings, SurfaceRouteManifestJson, execute_project_surface_operation,
    execute_project_surface_operation_read_only,
};
use marrow_run::{
    ProjectSessionError, ProjectSurfaceReadSession, ProjectSurfaceSession, ProjectSurfaceSnapshot,
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_ACTION, SURFACE_COMPUTED, SURFACE_CONFLICT,
    SURFACE_INVALID_DATA, SURFACE_LIMIT, SURFACE_REQUEST, SURFACE_STALE_CURSOR, SURFACE_STORE,
    SURFACE_WRITE,
};

use crate::cmd_run::report_session_open_error;
use crate::{CheckFormat, report_simple_error};

mod cors;
use cors::CorsPolicy;

const DEFAULT_PORT: u16 = 8080;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const STREAM_TIMEOUT: Duration = Duration::from_secs(15);

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

Run a local HTTP surface endpoint. The server accepts one JSON POST per
connection and closes the response on descriptor-derived
/surface/v1/{read|create|update|delete|action}/<operation-tag> routes.

  --write  Expose create/update/delete/action routes and open a writable surface session.
           Defaults to read-only mode, serving read routes including computed reads.
  --cors-origin
           Allow one exact browser Origin such as http://localhost:5173.
           No CORS headers are emitted unless this option is present.
  --addr   Loopback socket address to bind. Defaults to 127.0.0.1:8080.
  --watch  Re-check and rewrite the declared client on a .mw change, then keep serving.
";

pub(crate) fn serve(args: &[String]) -> ExitCode {
    let mut addr = default_addr();
    let mut mode = ServeMode::ReadOnly;
    let mut cors = None;
    let mut saw_addr = false;
    let mut saw_cors_origin = false;
    let mut saw_write = false;
    let mut watch = false;
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
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
                cors = match CorsPolicy::new(value) {
                    Ok(cors) => Some(cors),
                    Err(message) => {
                        eprintln!("{message}");
                        return ExitCode::from(2);
                    }
                };
                saw_cors_origin = true;
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
    if !addr.ip().is_loopback() {
        eprintln!("--addr must use a loopback address");
        return ExitCode::from(2);
    }

    let config = match crate::load_config_with_format(&dir, CheckFormat::Text) {
        Ok(config) => config,
        Err(code) => return code,
    };
    // A `--write` serve replays an unclean shutdown before it inspects the store, so a store left
    // flagged for recovery by a prior signalled writer with no interrupted commit opens clean
    // rather than refusing the read-only catalog read the snapshot needs.
    if matches!(mode, ServeMode::Write)
        && let Err(error) = marrow_run::recover_store_for_write(std::path::Path::new(&dir), &config)
    {
        return report_session_open_error(&dir, error, CheckFormat::Text);
    }
    let snapshot = match ProjectSurfaceSnapshot::open(&dir) {
        Ok(snapshot) => snapshot,
        Err(error) => return report_session_open_error(&dir, error, CheckFormat::Text),
    };
    let session = match SurfaceServeSession::open(&snapshot, mode) {
        Ok(session) => session,
        Err(error) => return report_session_open_error(&dir, error, CheckFormat::Text),
    };
    let routes = match SurfaceRoutes::from_program(session.program(), mode) {
        Ok(routes) => routes,
        Err(message) => {
            report_simple_error(SURFACE_ABI_MISMATCH, &message, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    // Startup regenerates the declared client from the opened program so a fresh `serve` always
    // hands clients the surface ABI it will answer over.
    if let Err(code) =
        crate::sync_declared_client(&dir, &config, session.program(), CheckFormat::Text)
    {
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
            signature,
        }
    });
    run_server(listener, executor, routes, cors.as_ref(), watch)
}

/// The state `serve --watch` re-checks on a source change: where to re-open the snapshot, the
/// declared config to rewrite the client against, the session mode to re-open under, and the last
/// observed source signature. A change in the signature triggers a re-open, a client rewrite, and a
/// swap of the live executor and routes.
struct SurfaceWatch {
    dir: String,
    config: marrow_project::ProjectConfig,
    mode: ServeMode,
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
    cors: Option<&CorsPolicy>,
    mut watch: Option<SurfaceWatch>,
) -> ExitCode {
    // Without `--watch` the accept loop blocks; with it the listener is nonblocking so the loop can
    // recompute the source signature on each idle cadence and re-check when a source file changes.
    if watch.is_some()
        && let Err(error) = listener.set_nonblocking(true)
    {
        eprintln!("io.listen: failed to set surface watch poll: {error}");
        return ExitCode::FAILURE;
    }
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Err(error) = stream.set_nonblocking(false) {
                    eprintln!("io.read: failed to set surface connection blocking: {error}");
                    continue;
                }
                if let Err(error) = stream.set_read_timeout(Some(STREAM_TIMEOUT)) {
                    eprintln!("io.read: failed to set surface read timeout: {error}");
                    continue;
                }
                if let Err(error) = stream.set_write_timeout(Some(STREAM_TIMEOUT)) {
                    eprintln!("io.write: failed to set surface write timeout: {error}");
                    continue;
                }
                let response = handle_connection(&mut stream, &executor, &routes, cors);
                if let Err(error) = write_response(&mut stream, &response) {
                    eprintln!("io.write: failed to write surface response: {error}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(watch) = watch.as_mut() {
                    watch.poll(&mut executor, &mut routes);
                }
                std::thread::sleep(WATCH_POLL);
            }
            Err(error) => {
                eprintln!("io.listen: failed to accept surface connection: {error}");
            }
        }
    }
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
                eprintln!("{SURFACE_STORE}: failed to read project sources for watch: {error}");
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
            eprintln!("{SURFACE_ABI_MISMATCH}: surface re-check failed: {message}");
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
        let fresh_routes = SurfaceRoutes::from_program(snapshot.program(), self.mode)?;
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
}

impl SurfaceRoutes {
    fn from_program(program: &CheckedProgram, mode: ServeMode) -> Result<Self, String> {
        let abi = SurfaceAbiJson::from_program(program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);
        let catalog = SurfaceOperationCatalog::from_abi(&abi).map_err(|error| error.to_string())?;
        let bindings = SurfaceRouteBindings::from_manifest(&manifest, &catalog)
            .map_err(|error| error.to_string())?;
        let routes = bindings
            .iter()
            .filter(|binding| mode.allows(binding))
            .map(|binding| (binding.path.clone(), binding.clone()))
            .collect();
        Ok(Self { routes })
    }

    fn binding_for_path(&self, path: &str) -> Option<&SurfaceRouteBinding> {
        self.routes.get(path)
    }
}

fn handle_connection(
    stream: &mut TcpStream,
    executor: &SurfaceServeExecutor,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
) -> SurfaceHttpResponse {
    match read_http_request(stream) {
        Ok(request) => execute_http_request(request, executor, routes, cors),
        Err(failure) => SurfaceHttpResponse::error(failure.status, failure.error),
    }
}

fn execute_http_request(
    request: HttpRequest,
    executor: &SurfaceServeExecutor,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
) -> SurfaceHttpResponse {
    let cors_origin = match cors_origin_for_request(&request, cors) {
        Ok(origin) => origin,
        Err(response) => return response,
    };
    if request.method == "OPTIONS" && cors.is_some() {
        return execute_cors_preflight(request, routes, cors_origin);
    }
    if request.method != "POST" {
        return SurfaceHttpResponse::error(
            HttpStatus::MethodNotAllowed,
            surface_error(SURFACE_REQUEST, "surface routes accept POST only"),
        )
        .with_cors(cors_origin);
    }
    if request.target.contains('?') {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        )
        .with_cors(cors_origin);
    }
    let Some(route) = routes.binding_for_path(&request.target) else {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        )
        .with_cors(cors_origin);
    };
    if !request.content_type_is_json {
        return SurfaceHttpResponse::error(
            HttpStatus::UnsupportedMediaType,
            surface_error(
                SURFACE_REQUEST,
                "surface request body must be application/json",
            ),
        )
        .with_cors(cors_origin);
    }
    let operation = match serde_json::from_slice::<SurfaceOperationRequestJson>(&request.body) {
        Ok(operation) => operation,
        Err(_) => {
            return SurfaceHttpResponse::error(
                HttpStatus::BadRequest,
                surface_error(
                    SURFACE_REQUEST,
                    "surface request body is not a valid operation",
                ),
            )
            .with_cors(cors_origin);
        }
    };
    if operation.operation_tag != route.operation_tag {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        )
        .with_cors(cors_origin);
    }
    if !route.kind.matches_operation_body(&operation.request) {
        return SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(
                SURFACE_REQUEST,
                "surface operation request body does not match the route",
            ),
        )
        .with_cors(cors_origin);
    }
    match executor.execute(&operation) {
        Ok(response) => SurfaceHttpResponse::json(HttpStatus::Ok, response_value(response))
            .with_cors(cors_origin),
        Err(error) => SurfaceHttpResponse::error(status_for_surface_error(&error.code), error)
            .with_cors(cors_origin),
    }
}

fn cors_origin_for_request(
    request: &HttpRequest,
    cors: Option<&CorsPolicy>,
) -> Result<Option<String>, SurfaceHttpResponse> {
    let Some(origin) = &request.origin else {
        return Ok(None);
    };
    let Some(cors) = cors else {
        return Ok(None);
    };
    if let Some(configured) = cors.matched_origin(origin) {
        return Ok(Some(configured.to_string()));
    }
    Err(SurfaceHttpResponse::error(
        HttpStatus::Forbidden,
        surface_error(SURFACE_REQUEST, "surface CORS origin is not allowed"),
    ))
}

fn execute_cors_preflight(
    request: HttpRequest,
    routes: &SurfaceRoutes,
    cors_origin: Option<String>,
) -> SurfaceHttpResponse {
    let Some(cors_origin) = cors_origin else {
        return SurfaceHttpResponse::error(
            HttpStatus::Forbidden,
            surface_error(
                SURFACE_REQUEST,
                "surface CORS preflight origin is not allowed",
            ),
        );
    };
    if request.target.contains('?') {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        )
        .with_cors(Some(cors_origin));
    }
    if routes.binding_for_path(&request.target).is_none() {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        )
        .with_cors(Some(cors_origin));
    }
    if request.access_control_request_method.as_deref() != Some("POST") {
        return SurfaceHttpResponse::error(
            HttpStatus::MethodNotAllowed,
            surface_error(SURFACE_REQUEST, "surface CORS preflight must request POST"),
        )
        .with_cors(Some(cors_origin));
    }
    if !request.body.is_empty() {
        return SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(SURFACE_REQUEST, "surface CORS preflight body must be empty"),
        )
        .with_cors(Some(cors_origin));
    }
    SurfaceHttpResponse::empty(HttpStatus::NoContent).with_cors(Some(cors_origin))
}

fn status_for_surface_error(code: &str) -> HttpStatus {
    match code {
        SURFACE_ABSENT | SURFACE_ABI_MISMATCH => HttpStatus::NotFound,
        SURFACE_CONFLICT | SURFACE_STALE_CURSOR => HttpStatus::Conflict,
        SURFACE_LIMIT => HttpStatus::PayloadTooLarge,
        SURFACE_ACTION | SURFACE_COMPUTED | SURFACE_INVALID_DATA | SURFACE_STORE
        | SURFACE_WRITE => HttpStatus::InternalServerError,
        SURFACE_REQUEST => HttpStatus::BadRequest,
        _ => HttpStatus::BadRequest,
    }
}

struct HttpRequest {
    method: String,
    target: String,
    origin: Option<String>,
    access_control_request_method: Option<String>,
    content_type_is_json: bool,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, HttpFailure> {
    let mut buffer = Vec::new();
    let header_end = read_until_header_end(stream, &mut buffer)?;
    let parsed = parse_head(&buffer[..header_end])?;
    let content_length = match parsed.content_length {
        Some(content_length) => content_length,
        None if parsed.method == "OPTIONS" => 0,
        // A non-POST method reaches here before the route-level method check because it
        // carries no Content-Length, so the bare Content-Length error would hide the real
        // cause. Name the method requirement instead so the developer switches to POST.
        None if parsed.method != "POST" => {
            return Err(request_failure(
                "surface routes accept POST only; send the operation as a POST request",
            ));
        }
        None => {
            return Err(request_failure(
                "surface request must contain exactly one Content-Length",
            ));
        }
    };
    if content_length > MAX_BODY_BYTES {
        return Err(HttpFailure::new(
            HttpStatus::PayloadTooLarge,
            SURFACE_LIMIT,
            "surface request body is too large",
        ));
    }
    let body_start = header_end + 4;
    let body_end = body_start.checked_add(content_length).ok_or_else(|| {
        HttpFailure::new(
            HttpStatus::PayloadTooLarge,
            SURFACE_LIMIT,
            "surface request body is too large",
        )
    })?;
    if buffer.len() > body_end {
        return Err(HttpFailure::new(
            HttpStatus::BadRequest,
            SURFACE_REQUEST,
            "surface request contains trailing bytes after the declared body",
        ));
    }
    while buffer.len() < body_end {
        let remaining = body_end - buffer.len();
        let mut chunk = [0; 4096];
        let limit = remaining.min(chunk.len());
        let read = stream
            .read(&mut chunk[..limit])
            .map_err(|_| request_failure("surface request body could not be read"))?;
        if read == 0 {
            return Err(request_failure("surface request body ended early"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(HttpRequest {
        method: parsed.method,
        target: parsed.target,
        origin: parsed.origin,
        access_control_request_method: parsed.access_control_request_method,
        content_type_is_json: parsed.content_type_is_json,
        body: buffer[body_start..body_end].to_vec(),
    })
}

fn read_until_header_end(
    stream: &mut TcpStream,
    buffer: &mut Vec<u8>,
) -> Result<usize, HttpFailure> {
    loop {
        if let Some(index) = find_header_end(buffer) {
            if index > MAX_HEADER_BYTES {
                return Err(HttpFailure::new(
                    HttpStatus::RequestHeaderFieldsTooLarge,
                    SURFACE_LIMIT,
                    "surface request headers are too large",
                ));
            }
            return Ok(index);
        }
        if buffer.len() > MAX_HEADER_BYTES {
            return Err(HttpFailure::new(
                HttpStatus::RequestHeaderFieldsTooLarge,
                SURFACE_LIMIT,
                "surface request headers are too large",
            ));
        }
        let mut chunk = [0; 1024];
        let read = stream
            .read(&mut chunk)
            .map_err(|_| request_failure("surface request headers could not be read"))?;
        if read == 0 {
            return Err(request_failure(
                "surface request ended before headers completed",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

struct ParsedHead {
    method: String,
    target: String,
    origin: Option<String>,
    access_control_request_method: Option<String>,
    content_length: Option<usize>,
    content_type_is_json: bool,
}

fn parse_head(head: &[u8]) -> Result<ParsedHead, HttpFailure> {
    let head = std::str::from_utf8(head)
        .map_err(|_| request_failure("surface request headers must be UTF-8"))?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| request_failure("surface request line is missing"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| request_failure("surface request method is missing"))?
        .to_string();
    let target = request_parts
        .next()
        .ok_or_else(|| request_failure("surface request target is missing"))?
        .to_string();
    let version = request_parts
        .next()
        .ok_or_else(|| request_failure("surface request version is missing"))?;
    if request_parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(request_failure("surface request line is malformed"));
    }

    let mut content_length = None;
    let mut origin = None;
    let mut access_control_request_method = None;
    let mut saw_content_type = false;
    let mut content_type_is_json = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(request_failure("surface request headers must not fold"));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(request_failure("surface request header is malformed"));
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "content-length" => {
                if content_length.is_some() {
                    return Err(request_failure(
                        "surface request must contain exactly one Content-Length",
                    ));
                }
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| request_failure("surface Content-Length is malformed"))?;
                content_length = Some(parsed);
            }
            "content-type" => {
                if saw_content_type {
                    return Err(request_failure(
                        "surface request must contain at most one Content-Type",
                    ));
                }
                saw_content_type = true;
                content_type_is_json = is_json_content_type(value);
            }
            "origin" => {
                if origin.is_some() {
                    return Err(request_failure(
                        "surface request must contain at most one Origin",
                    ));
                }
                origin = Some(value.to_string());
            }
            "access-control-request-method" => {
                if access_control_request_method.is_some() {
                    return Err(request_failure(
                        "surface request must contain at most one Access-Control-Request-Method",
                    ));
                }
                access_control_request_method = Some(value.to_ascii_uppercase());
            }
            "transfer-encoding" => {
                return Err(request_failure(
                    "surface request must not use Transfer-Encoding",
                ));
            }
            _ => {}
        }
    }
    Ok(ParsedHead {
        method,
        target,
        origin,
        access_control_request_method,
        content_length,
        content_type_is_json,
    })
}

fn is_json_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
}

fn request_failure(message: &'static str) -> HttpFailure {
    HttpFailure::new(HttpStatus::BadRequest, SURFACE_REQUEST, message)
}

struct HttpFailure {
    status: HttpStatus,
    error: SurfaceOperationErrorJson,
}

impl HttpFailure {
    fn new(status: HttpStatus, code: &str, message: &str) -> Self {
        Self {
            status,
            error: surface_error(code, message),
        }
    }
}

struct SurfaceHttpResponse {
    status: HttpStatus,
    body: Option<serde_json::Value>,
    cors_origin: Option<String>,
}

impl SurfaceHttpResponse {
    fn json(status: HttpStatus, body: serde_json::Value) -> Self {
        Self {
            status,
            body: Some(body),
            cors_origin: None,
        }
    }

    fn empty(status: HttpStatus) -> Self {
        Self {
            status,
            body: None,
            cors_origin: None,
        }
    }

    fn error(status: HttpStatus, error: SurfaceOperationErrorJson) -> Self {
        Self {
            status,
            body: Some(error_value(error)),
            cors_origin: None,
        }
    }

    fn with_cors(mut self, origin: Option<String>) -> Self {
        self.cors_origin = origin;
        self
    }
}

#[derive(Clone, Copy)]
enum HttpStatus {
    Ok,
    NoContent,
    BadRequest,
    Conflict,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    PayloadTooLarge,
    UnsupportedMediaType,
    InternalServerError,
    RequestHeaderFieldsTooLarge,
}

impl HttpStatus {
    fn code(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::NoContent => 204,
            Self::BadRequest => 400,
            Self::Conflict => 409,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
            Self::PayloadTooLarge => 413,
            Self::UnsupportedMediaType => 415,
            Self::InternalServerError => 500,
            Self::RequestHeaderFieldsTooLarge => 431,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::NoContent => "No Content",
            Self::BadRequest => "Bad Request",
            Self::Conflict => "Conflict",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "Not Found",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::PayloadTooLarge => "Payload Too Large",
            Self::UnsupportedMediaType => "Unsupported Media Type",
            Self::InternalServerError => "Internal Server Error",
            Self::RequestHeaderFieldsTooLarge => "Request Header Fields Too Large",
        }
    }
}

fn write_response(stream: &mut TcpStream, response: &SurfaceHttpResponse) -> std::io::Result<()> {
    let body = response
        .body
        .as_ref()
        .map(|body| {
            serde_json::to_vec(body).unwrap_or_else(|_| {
                b"{\"code\":\"surface.store\",\"message\":\"surface response could not be encoded\"}"
                    .to_vec()
            })
        })
        .unwrap_or_default();
    write!(
        stream,
        "HTTP/1.1 {} {}\r\n",
        response.status.code(),
        response.status.reason()
    )?;
    if response.body.is_some() {
        stream.write_all(b"Content-Type: application/json\r\n")?;
    }
    if let Some(origin) = &response.cors_origin {
        write!(
            stream,
            "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nVary: Origin\r\n"
        )?;
    }
    write!(
        stream,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()
}

fn response_value(value: SurfaceOperationResponseJson) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or_else(|_| {
        serde_json::json!({
            "code": SURFACE_STORE,
            "message": "surface response could not be encoded"
        })
    })
}

fn error_value(value: SurfaceOperationErrorJson) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or_else(|_| {
        serde_json::json!({
            "code": SURFACE_STORE,
            "message": "surface response could not be encoded"
        })
    })
}

fn surface_error(code: &str, message: &str) -> SurfaceOperationErrorJson {
    SurfaceOperationErrorJson {
        code: code.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computed_read_execution_faults_are_server_faults() {
        assert_eq!(status_for_surface_error(SURFACE_COMPUTED).code(), 500);
        assert_eq!(status_for_surface_error(SURFACE_REQUEST).code(), 400);
    }

    #[test]
    fn abi_mismatch_is_the_not_found_wrong_route_class() {
        assert_eq!(status_for_surface_error(SURFACE_ABI_MISMATCH).code(), 404);
        assert_eq!(status_for_surface_error(SURFACE_ABSENT).code(), 404);
    }
}
