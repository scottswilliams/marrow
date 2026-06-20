use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use marrow_check::CheckedProgram;
use marrow_json::surface::{
    SurfaceAbiJson, SurfaceOperationErrorJson, SurfaceOperationRequestJson,
    SurfaceOperationResponseJson, SurfaceRouteManifestJson, SurfaceRouteRequestJson,
    execute_project_surface_operation, execute_project_surface_operation_read_only,
};
use marrow_run::{
    ProjectSessionError, ProjectSurfaceReadSession, ProjectSurfaceSession, SURFACE_ABI_MISMATCH,
    SURFACE_ABSENT, SURFACE_ACTION, SURFACE_CONFLICT, SURFACE_INVALID_DATA, SURFACE_LIMIT,
    SURFACE_REQUEST, SURFACE_STALE_CURSOR, SURFACE_STORE, SURFACE_WRITE,
};

use crate::cmd_run::report_session_open_error;
use crate::{CheckFormat, report_simple_error};

const DEFAULT_PORT: u16 = 8080;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const STREAM_TIMEOUT: Duration = Duration::from_secs(15);

const HELP: &str = "\
Usage:
  marrow surface serve [--write] [--addr <loopback:port>] <projectdir>

Run a local HTTP surface endpoint. The server accepts one JSON POST per
connection and closes the response on descriptor-derived
/surface/v1/{read|update|action}/<operation-tag> routes.

  --write  Expose update/action routes and open a writable surface session.
           Defaults to read-only mode, serving read routes only.
  --addr   Loopback socket address to bind. Defaults to 127.0.0.1:8080.
";

#[derive(Clone, Copy)]
enum ServeMode {
    ReadOnly,
    Write,
}

impl ServeMode {
    fn allows(self, request: SurfaceRouteRequestJson) -> bool {
        match self {
            Self::ReadOnly => request.is_read(),
            Self::Write => true,
        }
    }
}

pub(crate) fn serve(args: &[String]) -> ExitCode {
    let mut addr = default_addr();
    let mut mode = ServeMode::ReadOnly;
    let mut saw_addr = false;
    let mut saw_write = false;
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
                return crate::unknown_option("surface serve", value);
            }
            value => {
                if let Err(code) =
                    crate::take_single_target(&mut dir, value, "surface serve", "project directory")
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

    let session = match SurfaceServeSession::open(&dir, mode) {
        Ok(session) => session,
        Err(error) => return report_session_open_error(&dir, error, CheckFormat::Text),
    };
    let routes = SurfaceRoutes::from_program(session.program(), mode);
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
    println!("surface serve listening on http://{bound_addr}");
    if let Err(error) = std::io::stdout().flush() {
        report_simple_error(
            "io.write",
            &format!("failed to write surface server status: {error}"),
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    }

    run_server(listener, &session, &routes)
}

fn default_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))
}

fn run_server(
    listener: TcpListener,
    session: &SurfaceServeSession,
    routes: &SurfaceRoutes,
) -> ExitCode {
    for connection in listener.incoming() {
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("io.listen: failed to accept surface connection: {error}");
                continue;
            }
        };
        if let Err(error) = stream.set_read_timeout(Some(STREAM_TIMEOUT)) {
            eprintln!("io.read: failed to set surface read timeout: {error}");
            continue;
        }
        if let Err(error) = stream.set_write_timeout(Some(STREAM_TIMEOUT)) {
            eprintln!("io.write: failed to set surface write timeout: {error}");
            continue;
        }
        let response = handle_connection(&mut stream, session, routes);
        if let Err(error) = write_response(&mut stream, &response) {
            eprintln!("io.write: failed to write surface response: {error}");
        }
    }
    ExitCode::SUCCESS
}

enum SurfaceServeSession {
    ReadOnly(ProjectSurfaceReadSession),
    Write(ProjectSurfaceSession),
}

impl SurfaceServeSession {
    fn open(root: impl AsRef<Path>, mode: ServeMode) -> Result<Self, ProjectSessionError> {
        match mode {
            ServeMode::ReadOnly => ProjectSurfaceReadSession::open(root).map(Self::ReadOnly),
            ServeMode::Write => ProjectSurfaceSession::open(root).map(Self::Write),
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

struct SurfaceRouteBinding {
    operation_tag: String,
    request: SurfaceRouteRequestJson,
}

impl SurfaceRoutes {
    fn from_program(program: &CheckedProgram, mode: ServeMode) -> Self {
        let abi = SurfaceAbiJson::from_program(program);
        let manifest = SurfaceRouteManifestJson::from_abi(&abi);
        let routes = manifest
            .routes
            .into_iter()
            .filter(|route| mode.allows(route.request))
            .map(|route| {
                (
                    route.path,
                    SurfaceRouteBinding {
                        operation_tag: route.operation_tag,
                        request: route.request,
                    },
                )
            })
            .collect();
        Self { routes }
    }

    fn binding_for_path(&self, path: &str) -> Option<&SurfaceRouteBinding> {
        self.routes.get(path)
    }
}

fn handle_connection(
    stream: &mut TcpStream,
    session: &SurfaceServeSession,
    routes: &SurfaceRoutes,
) -> SurfaceHttpResponse {
    match read_http_request(stream) {
        Ok(request) => execute_http_request(request, session, routes),
        Err(failure) => SurfaceHttpResponse::error(failure.status, failure.error),
    }
}

fn execute_http_request(
    request: HttpRequest,
    session: &SurfaceServeSession,
    routes: &SurfaceRoutes,
) -> SurfaceHttpResponse {
    if request.method != "POST" {
        return SurfaceHttpResponse::error(
            HttpStatus::MethodNotAllowed,
            surface_error(SURFACE_REQUEST, "surface routes accept POST only"),
        );
    }
    if request.target.contains('?') {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        );
    }
    let Some(route) = routes.binding_for_path(&request.target) else {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        );
    };
    if !request.content_type_is_json {
        return SurfaceHttpResponse::error(
            HttpStatus::UnsupportedMediaType,
            surface_error(
                SURFACE_REQUEST,
                "surface request body must be application/json",
            ),
        );
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
            );
        }
    };
    if operation.operation_tag != route.operation_tag {
        return SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        );
    }
    if !route.request.matches_operation_body(&operation.request) {
        return SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(
                SURFACE_REQUEST,
                "surface operation request body does not match the route",
            ),
        );
    }
    match session.execute(&operation) {
        Ok(response) => SurfaceHttpResponse::json(HttpStatus::Ok, response_value(response)),
        Err(error) => SurfaceHttpResponse::error(status_for_surface_error(&error.code), error),
    }
}

fn status_for_surface_error(code: &str) -> HttpStatus {
    match code {
        SURFACE_ABSENT => HttpStatus::NotFound,
        SURFACE_CONFLICT | SURFACE_STALE_CURSOR => HttpStatus::Conflict,
        SURFACE_LIMIT => HttpStatus::PayloadTooLarge,
        SURFACE_ACTION | SURFACE_INVALID_DATA | SURFACE_STORE | SURFACE_WRITE => {
            HttpStatus::InternalServerError
        }
        SURFACE_ABI_MISMATCH | SURFACE_REQUEST => HttpStatus::BadRequest,
        _ => HttpStatus::BadRequest,
    }
}

struct HttpRequest {
    method: String,
    target: String,
    content_type_is_json: bool,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, HttpFailure> {
    let mut buffer = Vec::new();
    let header_end = read_until_header_end(stream, &mut buffer)?;
    let parsed = parse_head(&buffer[..header_end])?;
    if parsed.content_length > MAX_BODY_BYTES {
        return Err(HttpFailure::new(
            HttpStatus::PayloadTooLarge,
            SURFACE_LIMIT,
            "surface request body is too large",
        ));
    }
    let body_start = header_end + 4;
    let body_end = body_start
        .checked_add(parsed.content_length)
        .ok_or_else(|| {
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
    content_length: usize,
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
        content_length: content_length.ok_or_else(|| {
            request_failure("surface request must contain exactly one Content-Length")
        })?,
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
    body: serde_json::Value,
}

impl SurfaceHttpResponse {
    fn json(status: HttpStatus, body: serde_json::Value) -> Self {
        Self { status, body }
    }

    fn error(status: HttpStatus, error: SurfaceOperationErrorJson) -> Self {
        Self {
            status,
            body: error_value(error),
        }
    }
}

#[derive(Clone, Copy)]
enum HttpStatus {
    Ok,
    BadRequest,
    Conflict,
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
            Self::BadRequest => 400,
            Self::Conflict => 409,
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
            Self::BadRequest => "Bad Request",
            Self::Conflict => "Conflict",
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
    let body = serde_json::to_vec(&response.body).unwrap_or_else(|_| {
        b"{\"code\":\"surface.store\",\"message\":\"surface response could not be encoded\"}"
            .to_vec()
    });
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status.code(),
        response.status.reason(),
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
