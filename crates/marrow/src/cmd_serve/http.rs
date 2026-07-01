use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Instant;

use marrow_json::surface::{
    SurfaceOperationErrorJson, SurfaceOperationRequestJson, SurfaceOperationResponseJson,
};
use marrow_run::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_ACTION, SURFACE_COMPUTED, SURFACE_CONFLICT,
    SURFACE_INVALID_DATA, SURFACE_LIMIT, SURFACE_REQUEST, SURFACE_STALE_CURSOR, SURFACE_STORE,
    SURFACE_WRITE,
};

use super::auth::RemoteAuthToken;
use super::cors::{CorsMatch, CorsPolicy};
use super::{
    MAX_BODY_BYTES, MAX_HEADER_BYTES, READ_POLL_INTERVAL, STREAM_TIMEOUT, SURFACE_AUTH,
    SurfaceRoutes, SurfaceServeExecutor, server_code_message, shutdown, surface_error,
};

pub(super) fn serve_connection(
    mut stream: TcpStream,
    executor: &SurfaceServeExecutor,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
    remote_auth: Option<&RemoteAuthToken>,
    shutdown: &shutdown::Shutdown,
) {
    if let Err(error) = stream.set_nonblocking(false) {
        eprintln!(
            "{}",
            server_code_message(
                "io.read",
                format!("failed to set surface connection blocking: {error}")
            )
        );
        return;
    }
    if let Err(error) = stream.set_read_timeout(Some(READ_POLL_INTERVAL)) {
        eprintln!(
            "{}",
            server_code_message(
                "io.read",
                format!("failed to set surface read timeout: {error}")
            )
        );
        return;
    }
    if let Err(error) = stream.set_write_timeout(Some(STREAM_TIMEOUT)) {
        eprintln!(
            "{}",
            server_code_message(
                "io.write",
                format!("failed to set surface write timeout: {error}")
            )
        );
        return;
    }
    let response = handle_connection(&mut stream, executor, routes, cors, remote_auth, shutdown);
    if let Err(error) = write_response(&mut stream, &response) {
        eprintln!(
            "{}",
            server_code_message(
                "io.write",
                format!("failed to write surface response: {error}")
            )
        );
    }
}

fn handle_connection(
    stream: &mut TcpStream,
    executor: &SurfaceServeExecutor,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
    remote_auth: Option<&RemoteAuthToken>,
    shutdown: &shutdown::Shutdown,
) -> SurfaceHttpResponse {
    let header_read_mode = if remote_auth.is_some() {
        HeaderReadMode::Exact
    } else {
        HeaderReadMode::Buffered
    };
    let partial = match read_http_request_head(stream, shutdown, header_read_mode) {
        Ok(partial) => partial,
        Err(failure) => {
            return SurfaceHttpResponse::error(failure.status, failure.error).with_cors_vary(cors);
        }
    };
    let (route, cors_match) = match decide_http_head(&partial.head, routes, cors, remote_auth) {
        HeadDecision::Respond(response) => return response,
        HeadDecision::ReadBody { route, cors_match } => (route, cors_match),
    };
    match read_http_request_body(stream, partial, shutdown) {
        Ok(request) => execute_http_request_body(request, executor, &route, cors_match),
        Err(failure) => {
            SurfaceHttpResponse::error(failure.status, failure.error).with_cors_match(cors_match)
        }
    }
}

enum HeadDecision {
    Respond(SurfaceHttpResponse),
    ReadBody {
        route: marrow_json::surface::SurfaceRouteBinding,
        cors_match: Option<CorsMatch>,
    },
}

fn cors_origin_for_request(
    request: &ParsedHead,
    cors: Option<&CorsPolicy>,
) -> Result<Option<CorsMatch>, SurfaceHttpResponse> {
    let Some(cors) = cors else {
        return Ok(None);
    };
    match request.origin.single() {
        Ok(Some(origin)) => {
            if let Some(matched) = cors.match_origin(origin) {
                Ok(Some(matched))
            } else {
                Err(SurfaceHttpResponse::error(
                    HttpStatus::Forbidden,
                    surface_error(SURFACE_REQUEST, "surface CORS origin is not allowed"),
                )
                .with_cors_vary(Some(cors)))
            }
        }
        Ok(None) => Ok(None),
        Err(()) => Err(SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(
                SURFACE_REQUEST,
                "surface request must contain at most one Origin",
            ),
        )
        .with_cors_vary(Some(cors))),
    }
}

fn decide_http_head(
    request: &ParsedHead,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
    remote_auth: Option<&RemoteAuthToken>,
) -> HeadDecision {
    if request.method == "OPTIONS" && cors.is_some() {
        let cors_match = match cors_origin_for_request(request, cors) {
            Ok(origin) => origin,
            Err(response) => return HeadDecision::Respond(response),
        };
        return HeadDecision::Respond(execute_cors_preflight(request, routes, cors, cors_match));
    }
    if let Some(auth) = remote_auth
        && !authorized_request(auth, request)
    {
        return HeadDecision::Respond(SurfaceHttpResponse::error(
            HttpStatus::Unauthorized,
            surface_error(SURFACE_AUTH, "surface authorization is required"),
        ));
    }
    let cors_match = match cors_origin_for_request(request, cors) {
        Ok(origin) => origin,
        Err(response) => return HeadDecision::Respond(response),
    };
    if request.method != "POST" {
        if request.content_length.is_none() {
            return HeadDecision::Respond(
                SurfaceHttpResponse::error(
                    HttpStatus::BadRequest,
                    surface_error(
                        SURFACE_REQUEST,
                        "surface routes accept POST only; send the operation as a POST request",
                    ),
                )
                .with_cors_match(cors_match),
            );
        }
        return HeadDecision::Respond(
            SurfaceHttpResponse::error(
                HttpStatus::MethodNotAllowed,
                surface_error(SURFACE_REQUEST, "surface routes accept POST only"),
            )
            .with_cors_match(cors_match),
        );
    }
    let route = match route_for_head(request, routes, cors_match.clone()) {
        Ok(route) => route,
        Err(response) => return HeadDecision::Respond(response),
    };
    if !routes.mode_allows(&route) {
        let status = if routes.remote {
            HttpStatus::Forbidden
        } else {
            HttpStatus::NotFound
        };
        let code = if routes.remote {
            SURFACE_AUTH
        } else {
            SURFACE_ABI_MISMATCH
        };
        return HeadDecision::Respond(
            SurfaceHttpResponse::error(
                status,
                surface_error(
                    code,
                    "surface operation is not available in this serve mode",
                ),
            )
            .with_cors_match(cors_match),
        );
    }
    if !request.content_type_is_json {
        return HeadDecision::Respond(
            SurfaceHttpResponse::error(
                HttpStatus::UnsupportedMediaType,
                surface_error(
                    SURFACE_REQUEST,
                    "surface request body must be application/json",
                ),
            )
            .with_cors_match(cors_match),
        );
    }
    HeadDecision::ReadBody { route, cors_match }
}

fn route_for_head(
    request: &ParsedHead,
    routes: &SurfaceRoutes,
    cors_match: Option<CorsMatch>,
) -> Result<marrow_json::surface::SurfaceRouteBinding, SurfaceHttpResponse> {
    if request.target.contains('?') {
        return Err(SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        )
        .with_cors_match(cors_match));
    }
    routes
        .binding_for_path(&request.target)
        .cloned()
        .ok_or_else(|| {
            SurfaceHttpResponse::error(
                HttpStatus::NotFound,
                surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
            )
            .with_cors_match(cors_match)
        })
}

fn authorized_request(auth: &RemoteAuthToken, request: &ParsedHead) -> bool {
    request
        .authorization
        .single()
        .ok()
        .flatten()
        .is_some_and(|value| auth.matches_bearer(value))
}

fn execute_http_request_body(
    request: HttpRequest,
    executor: &SurfaceServeExecutor,
    route: &marrow_json::surface::SurfaceRouteBinding,
    cors_match: Option<CorsMatch>,
) -> SurfaceHttpResponse {
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
            .with_cors_match(cors_match);
        }
    };
    if operation.operation_tag != route.operation_tag {
        return SurfaceHttpResponse::error(
            HttpStatus::NotFound,
            surface_error(SURFACE_ABI_MISMATCH, "surface operation is not active"),
        )
        .with_cors_match(cors_match);
    }
    if !route.kind.matches_operation_body(&operation.request) {
        return SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(
                SURFACE_REQUEST,
                "surface operation request body does not match the route",
            ),
        )
        .with_cors_match(cors_match);
    }
    match executor.execute(&operation) {
        Ok(response) => SurfaceHttpResponse::json(HttpStatus::Ok, response_value(response))
            .with_cors_match(cors_match),
        Err(error) => SurfaceHttpResponse::error(status_for_surface_error(&error.code), error)
            .with_cors_match(cors_match),
    }
}

fn execute_cors_preflight(
    request: &ParsedHead,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
    cors_match: Option<CorsMatch>,
) -> SurfaceHttpResponse {
    let Some(cors) = cors else {
        return SurfaceHttpResponse::error(
            HttpStatus::Forbidden,
            surface_error(
                SURFACE_REQUEST,
                "surface CORS preflight origin is not allowed",
            ),
        );
    };
    let Some(cors_match) = cors_match else {
        return SurfaceHttpResponse::error(
            HttpStatus::Forbidden,
            surface_error(
                SURFACE_REQUEST,
                "surface CORS preflight origin is not allowed",
            ),
        )
        .with_cors_vary(Some(cors));
    };
    if let Err(response) = route_for_head(request, routes, Some(cors_match.clone())) {
        return response;
    }
    match request.access_control_request_method.single() {
        Ok(Some("POST")) => {}
        Ok(_) => {
            return SurfaceHttpResponse::error(
                HttpStatus::MethodNotAllowed,
                surface_error(SURFACE_REQUEST, "surface CORS preflight must request POST"),
            )
            .with_cors_match(Some(cors_match));
        }
        Err(()) => {
            return SurfaceHttpResponse::error(
                HttpStatus::BadRequest,
                surface_error(
                    SURFACE_REQUEST,
                    "surface request must contain at most one Access-Control-Request-Method",
                ),
            )
            .with_cors_match(Some(cors_match));
        }
    }
    if cors.is_remote() && !remote_cors_headers_match(cors, &request.access_control_request_headers)
    {
        return SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(
                SURFACE_REQUEST,
                "surface CORS preflight must request Content-Type and Authorization headers",
            ),
        )
        .with_cors_match(Some(cors_match));
    }
    if request.content_length.unwrap_or(0) != 0 {
        return SurfaceHttpResponse::error(
            HttpStatus::BadRequest,
            surface_error(SURFACE_REQUEST, "surface CORS preflight body must be empty"),
        )
        .with_cors_match(Some(cors_match));
    }
    SurfaceHttpResponse::empty(HttpStatus::NoContent).with_cors_match(Some(cors_match))
}

fn remote_cors_headers_match(cors: &CorsPolicy, headers: &HeaderOccurrence) -> bool {
    let Ok(Some(headers)) = headers.single() else {
        return false;
    };
    cors.remote_request_headers_match(headers)
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
    body: Vec<u8>,
}

struct PartialHttpRequest {
    head: ParsedHead,
    buffer: Vec<u8>,
    header_end: usize,
}

#[derive(Clone, Copy)]
enum HeaderReadMode {
    Exact,
    Buffered,
}

fn read_http_request_head(
    stream: &mut TcpStream,
    shutdown: &shutdown::Shutdown,
    mode: HeaderReadMode,
) -> Result<PartialHttpRequest, HttpFailure> {
    let mut buffer = Vec::new();
    let header_end = read_until_header_end(stream, &mut buffer, shutdown, mode)?;
    let parsed = parse_head(&buffer[..header_end])?;
    Ok(PartialHttpRequest {
        head: parsed,
        buffer,
        header_end,
    })
}

fn read_http_request_body(
    stream: &mut TcpStream,
    partial: PartialHttpRequest,
    shutdown: &shutdown::Shutdown,
) -> Result<HttpRequest, HttpFailure> {
    let content_length = match partial.head.content_length {
        Some(content_length) => content_length,
        None if partial.head.method == "OPTIONS" => 0,
        None if partial.head.method != "POST" => {
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
    let mut buffer = partial.buffer;
    let body_start = partial.header_end + 4;
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
        let read = read_with_shutdown_poll(
            stream,
            &mut chunk[..limit],
            shutdown,
            "surface request body could not be read",
        )?;
        if read == 0 {
            return Err(request_failure("surface request body ended early"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(HttpRequest {
        body: buffer[body_start..body_end].to_vec(),
    })
}

fn read_until_header_end(
    stream: &mut TcpStream,
    buffer: &mut Vec<u8>,
    shutdown: &shutdown::Shutdown,
    mode: HeaderReadMode,
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
        let read_limit = match mode {
            HeaderReadMode::Exact => 1,
            HeaderReadMode::Buffered => chunk.len(),
        };
        let read = read_with_shutdown_poll(
            stream,
            &mut chunk[..read_limit],
            shutdown,
            "surface request headers could not be read",
        )?;
        if read == 0 {
            return Err(request_failure(
                "surface request ended before headers completed",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn read_with_shutdown_poll(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    shutdown: &shutdown::Shutdown,
    failure: &'static str,
) -> Result<usize, HttpFailure> {
    let idle_start = Instant::now();
    loop {
        if shutdown.requested().is_some() {
            return Err(request_failure("surface server is shutting down"));
        }
        match stream.read(buffer) {
            Ok(read) => return Ok(read),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) && idle_start.elapsed() < STREAM_TIMEOUT => {}
            Err(_) => return Err(request_failure(failure)),
        }
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

struct ParsedHead {
    method: String,
    target: String,
    origin: HeaderOccurrence,
    access_control_request_method: HeaderOccurrence,
    access_control_request_headers: HeaderOccurrence,
    authorization: HeaderOccurrence,
    content_length: Option<usize>,
    content_type_is_json: bool,
}

enum HeaderOccurrence {
    Missing,
    Single(String),
    Duplicate,
}

impl HeaderOccurrence {
    fn insert(&mut self, value: &str) {
        *self = match self {
            Self::Missing => Self::Single(value.to_string()),
            Self::Single(_) | Self::Duplicate => Self::Duplicate,
        };
    }

    fn single(&self) -> Result<Option<&str>, ()> {
        match self {
            Self::Missing => Ok(None),
            Self::Single(value) => Ok(Some(value)),
            Self::Duplicate => Err(()),
        }
    }
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
    let mut origin = HeaderOccurrence::Missing;
    let mut access_control_request_method = HeaderOccurrence::Missing;
    let mut access_control_request_headers = HeaderOccurrence::Missing;
    let mut authorization = HeaderOccurrence::Missing;
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
                origin.insert(value);
            }
            "access-control-request-method" => {
                access_control_request_method.insert(&value.to_ascii_uppercase());
            }
            "access-control-request-headers" => {
                access_control_request_headers.insert(value);
            }
            "authorization" => {
                authorization.insert(value);
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
        access_control_request_headers,
        authorization,
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
    cors_allow_headers: Option<&'static str>,
    cors_vary: Option<&'static str>,
}

impl SurfaceHttpResponse {
    fn json(status: HttpStatus, body: serde_json::Value) -> Self {
        Self {
            status,
            body: Some(body),
            cors_origin: None,
            cors_allow_headers: None,
            cors_vary: None,
        }
    }

    fn empty(status: HttpStatus) -> Self {
        Self {
            status,
            body: None,
            cors_origin: None,
            cors_allow_headers: None,
            cors_vary: None,
        }
    }

    fn error(status: HttpStatus, error: SurfaceOperationErrorJson) -> Self {
        Self {
            status,
            body: Some(error_value(error)),
            cors_origin: None,
            cors_allow_headers: None,
            cors_vary: None,
        }
    }

    fn with_cors_match(mut self, cors: Option<CorsMatch>) -> Self {
        if let Some(cors) = cors {
            self.cors_origin = Some(cors.origin);
            self.cors_allow_headers = Some(cors.allow_headers);
            self.cors_vary = Some(cors.vary);
        }
        self
    }

    fn with_cors_vary(mut self, cors: Option<&CorsPolicy>) -> Self {
        if let Some(cors) = cors {
            self.cors_vary = Some(cors.vary());
        }
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
    Unauthorized,
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
            Self::Unauthorized => 401,
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
            Self::Unauthorized => "Unauthorized",
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
        let allow_headers = response.cors_allow_headers.unwrap_or("Content-Type");
        write!(
            stream,
            "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: {allow_headers}\r\n"
        )?;
    }
    if let Some(vary) = response.cors_vary {
        write!(stream, "Vary: {vary}\r\n")?;
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

    #[test]
    fn request_head_reader_stops_at_header_terminator() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("test listener address");
        let writer = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(addr).expect("connect test listener");
            stream
                .write_all(b"POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nabcde")
                .expect("write coalesced request");
        });
        let (mut stream, _) = listener.accept().expect("accept test request");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set test read timeout");
        let shutdown = shutdown::install().expect("install test shutdown handle");
        let partial = match read_http_request_head(&mut stream, &shutdown, HeaderReadMode::Exact) {
            Ok(partial) => partial,
            Err(_) => panic!("request head reader rejected the coalesced test request"),
        };

        assert_eq!(partial.buffer.len(), partial.header_end + 4);
        assert_eq!(&partial.buffer[partial.header_end..], b"\r\n\r\n");
        writer.join().expect("join request writer");
    }
}
