use marrow_codes::Code;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use marrow_json::surface::{
    SURFACE_OPERATION_PROFILE_VERSION, SurfaceCursorJson, SurfaceCursorTokenCodec,
    SurfaceCursorTokenError, SurfaceCursorTokenErrorKind, SurfaceOperationErrorJson,
    SurfaceOperationRequestJson, SurfaceOperationResponseJson,
};
use marrow_run::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_ACTION, SURFACE_COMPUTED, SURFACE_CONFLICT,
    SURFACE_CURSOR, SURFACE_INVALID_DATA, SURFACE_LIMIT, SURFACE_REQUEST, SURFACE_STALE_CURSOR,
    SURFACE_STORE, SURFACE_WRITE,
};
use serde_json::Value as Json;

use super::auth::RemoteAuthToken;
use super::cors::{CorsMatch, CorsPolicy};
use super::cursor_token::RemoteCursorToken;
use super::{
    MAX_BODY_BYTES, MAX_HEADER_BYTES, POLL_INTERVAL, STREAM_TIMEOUT, SURFACE_AUTH, SurfaceRoutes,
    SurfaceServeExecutor, server_code_message, shutdown, surface_error,
};

pub(super) fn serve_connection(
    mut stream: TcpStream,
    executor: &SurfaceServeExecutor,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
    remote_auth: Option<&RemoteAuthToken>,
    remote_cursor_token: Option<&RemoteCursorToken>,
    shutdown: &shutdown::Shutdown,
) {
    let started = Instant::now();
    if let Err(error) = stream.set_nonblocking(false) {
        eprintln!(
            "{}",
            server_code_message(
                Code::IoRead.as_str(),
                format!("failed to set surface connection blocking: {error}")
            )
        );
        return;
    }
    if let Err(error) = stream.set_read_timeout(Some(POLL_INTERVAL)) {
        eprintln!(
            "{}",
            server_code_message(
                Code::IoRead.as_str(),
                format!("failed to set surface read timeout: {error}")
            )
        );
        return;
    }
    // A short write timeout, not STREAM_TIMEOUT: a stalled write must wake every
    // POLL_INTERVAL so the response write re-checks the shutdown signal and its total
    // deadline rather than blocking on a slow reader for the full stream timeout.
    if let Err(error) = stream.set_write_timeout(Some(POLL_INTERVAL)) {
        eprintln!(
            "{}",
            server_code_message(
                Code::IoWrite.as_str(),
                format!("failed to set surface write timeout: {error}")
            )
        );
        return;
    }
    let outcome = handle_connection(
        &mut stream,
        executor,
        routes,
        cors,
        remote_auth,
        remote_cursor_token,
        shutdown,
    );
    let status = outcome.response.status;
    match write_response(&mut stream, &outcome.response, shutdown) {
        Ok(()) => {
            if let Some(remaining) = outcome.drain_body {
                drain_request_body(&mut stream, remaining, shutdown);
            }
        }
        Err(error) => {
            eprintln!(
                "{}",
                server_code_message(
                    Code::IoWrite.as_str(),
                    format!("failed to write surface response: {error}")
                )
            );
        }
    }
    eprintln!(
        "{}",
        request_log_line(&outcome.log, status, started.elapsed())
    );
}

/// The single access-log line's fields for one connection: the routing identity, never any request
/// or response payload. The status and latency are supplied at emit time. `method` and `target` are
/// absent only when the request head could not be parsed; `operation_tag` is present only once a
/// route matched, distinguishing a confirmed operation from a rejected or non-surface path.
struct RequestLog {
    method: Option<String>,
    target: Option<String>,
    operation_tag: Option<String>,
}

impl RequestLog {
    fn unparsed() -> Self {
        Self {
            method: None,
            target: None,
            operation_tag: None,
        }
    }

    fn from_head(head: &ParsedHead) -> Self {
        // Log the path only, dropping any query string, so a rejected target can never carry data
        // into the log.
        let path = head.target.split('?').next().unwrap_or_default();
        Self {
            method: Some(head.method.clone()),
            target: Some(path.to_string()),
            operation_tag: None,
        }
    }

    fn with_operation_tag(mut self, tag: &str) -> Self {
        self.operation_tag = Some(tag.to_string());
        self
    }
}

fn request_log_line(log: &RequestLog, status: HttpStatus, elapsed: Duration) -> String {
    format!(
        "serve {} {} {} {}ms op={}",
        log.method.as_deref().unwrap_or("-"),
        log.target.as_deref().unwrap_or("-"),
        status.code(),
        elapsed.as_millis(),
        log.operation_tag.as_deref().unwrap_or("-"),
    )
}

/// A head-level error responds before the request body is read. The response is written first, then
/// the client's declared body is drained so closing the connection sends a normal FIN rather than a
/// TCP RST, which would discard the response the client has not yet consumed.
struct ConnectionOutcome {
    response: SurfaceHttpResponse,
    drain_body: Option<usize>,
    log: RequestLog,
}

fn handle_connection(
    stream: &mut TcpStream,
    executor: &SurfaceServeExecutor,
    routes: &SurfaceRoutes,
    cors: Option<&CorsPolicy>,
    remote_auth: Option<&RemoteAuthToken>,
    remote_cursor_token: Option<&RemoteCursorToken>,
    shutdown: &shutdown::Shutdown,
) -> ConnectionOutcome {
    let header_read_mode = if remote_auth.is_some() {
        HeaderReadMode::Exact
    } else {
        HeaderReadMode::Buffered
    };
    let partial = match read_http_request_head(stream, shutdown, header_read_mode) {
        Ok(partial) => partial,
        Err(failure) => {
            return ConnectionOutcome {
                response: SurfaceHttpResponse::error(failure.status, failure.error)
                    .with_cors_vary(cors),
                drain_body: None,
                log: RequestLog::unparsed(),
            };
        }
    };
    let log = RequestLog::from_head(&partial.head);
    // The readiness probe is answered before routing and auth so an orchestrator can poll it
    // directly; it never reflects request data and never reaches a surface session.
    if partial.head.target == HEALTH_PATH {
        return ConnectionOutcome {
            drain_body: pending_body_len(&partial),
            response: health_response(&partial.head.method, executor.is_ready()),
            log,
        };
    }
    let (route, cors_match) = match decide_http_head(&partial.head, routes, cors, remote_auth) {
        HeadDecision::Respond(response) => {
            return ConnectionOutcome {
                drain_body: pending_body_len(&partial),
                response,
                log,
            };
        }
        HeadDecision::ReadBody { route, cors_match } => (route, cors_match),
    };
    let log = log.with_operation_tag(&route.operation_tag);
    let response = match read_http_request_body(stream, partial, shutdown) {
        Ok(request) => {
            execute_http_request_body(request, executor, &route, cors_match, remote_cursor_token)
        }
        Err(failure) => {
            SurfaceHttpResponse::error(failure.status, failure.error).with_cors_match(cors_match)
        }
    };
    ConnectionOutcome {
        response,
        drain_body: None,
        log,
    }
}

const HEALTH_PATH: &str = "/health";

/// The operational readiness probe. It is unauthenticated and CORS-neutral so an orchestrator can
/// poll it directly, and it never reflects request data. GET reports store and catalog readiness;
/// any other method is rejected so the probe cannot be mistaken for a surface route.
fn health_response(method: &str, ready: bool) -> SurfaceHttpResponse {
    if method != "GET" {
        return SurfaceHttpResponse::error(
            HttpStatus::MethodNotAllowed,
            surface_error(SURFACE_REQUEST, "surface health probe accepts GET only"),
        );
    }
    let (status, state) = if ready {
        (HttpStatus::Ok, "ready")
    } else {
        (HttpStatus::ServiceUnavailable, "unavailable")
    };
    SurfaceHttpResponse::json(status, serde_json::json!({ "status": state }))
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
    remote_cursor_token: Option<&RemoteCursorToken>,
) -> SurfaceHttpResponse {
    let operation = match operation_from_http_body(&request.body, route, remote_cursor_token) {
        Ok(operation) => operation,
        Err(error) => {
            return SurfaceHttpResponse::error(status_for_surface_error(&error.code), error)
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
    if operation.profile_version != SURFACE_OPERATION_PROFILE_VERSION {
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
        Ok(response) => match response_value_for_route(response, route, remote_cursor_token) {
            Ok(value) => {
                SurfaceHttpResponse::json(HttpStatus::Ok, value).with_cors_match(cors_match)
            }
            Err(error) => SurfaceHttpResponse::error(status_for_surface_error(&error.code), error)
                .with_cors_match(cors_match),
        },
        Err(error) => SurfaceHttpResponse::error(status_for_surface_error(&error.code), error)
            .with_cors_match(cors_match),
    }
}

fn operation_from_http_body(
    body: &[u8],
    route: &marrow_json::surface::SurfaceRouteBinding,
    remote_cursor_token: Option<&RemoteCursorToken>,
) -> Result<SurfaceOperationRequestJson, SurfaceOperationErrorJson> {
    let Some(cursor_token) = remote_cursor_token else {
        return serde_json::from_slice::<SurfaceOperationRequestJson>(body)
            .map_err(|_| request_body_error());
    };
    if !route.kind.is_page_cursor_operation() {
        return serde_json::from_slice::<SurfaceOperationRequestJson>(body)
            .map_err(|_| request_body_error());
    }

    let mut value = serde_json::from_slice::<Json>(body).map_err(|_| request_body_error())?;
    let Some(operation_tag) = value.get("operation_tag").and_then(Json::as_str) else {
        return serde_json::from_value(value).map_err(|_| request_body_error());
    };
    if operation_tag != route.operation_tag {
        return Err(surface_error(
            SURFACE_ABI_MISMATCH,
            "surface operation is not active",
        ));
    }
    let Some(profile_version) = value.get("profile_version").and_then(Json::as_str) else {
        return serde_json::from_value(value).map_err(|_| request_body_error());
    };
    if profile_version != SURFACE_OPERATION_PROFILE_VERSION {
        return Err(surface_error(
            SURFACE_ABI_MISMATCH,
            "surface operation is not active",
        ));
    }
    if value.pointer("/request/kind").and_then(Json::as_str) != Some("page") {
        return serde_json::from_value(value).map_err(|_| request_body_error());
    }
    decrypt_page_request_cursor(&mut value, &route.operation_tag, cursor_token.codec())?;
    serde_json::from_value(value).map_err(|_| request_body_error())
}

fn decrypt_page_request_cursor(
    value: &mut Json,
    operation_tag: &str,
    codec: &SurfaceCursorTokenCodec,
) -> Result<(), SurfaceOperationErrorJson> {
    let Some(request) = value
        .get_mut("request")
        .and_then(|request| request.get_mut("request"))
    else {
        return Ok(());
    };
    let Some(cursor) = request.get_mut("cursor") else {
        return Ok(());
    };
    if cursor.is_null() {
        return Ok(());
    }
    let Some(token) = cursor.as_str() else {
        return Err(surface_error(
            SURFACE_CURSOR,
            "surface cursor token mode requires cursor strings",
        ));
    };
    let typed = codec
        .decode(operation_tag, token)
        .map_err(cursor_token_surface_error)?;
    *cursor = serde_json::to_value(typed).map_err(|_| {
        surface_error(
            SURFACE_STORE,
            "surface cursor token could not be converted to a typed cursor",
        )
    })?;
    Ok(())
}

fn response_value_for_route(
    response: SurfaceOperationResponseJson,
    route: &marrow_json::surface::SurfaceRouteBinding,
    remote_cursor_token: Option<&RemoteCursorToken>,
) -> Result<Json, SurfaceOperationErrorJson> {
    let Some(cursor_token) = remote_cursor_token else {
        return Ok(response_value(response));
    };
    if !route.kind.is_page_cursor_operation() {
        return Ok(response_value(response));
    }
    let mut value = serde_json::to_value(response)
        .map_err(|_| surface_error(SURFACE_STORE, "surface response could not be encoded"))?;
    encrypt_page_response_cursor(&mut value, &route.operation_tag, cursor_token.codec())?;
    Ok(value)
}

fn encrypt_page_response_cursor(
    value: &mut Json,
    operation_tag: &str,
    codec: &SurfaceCursorTokenCodec,
) -> Result<(), SurfaceOperationErrorJson> {
    let Some(next) = value.pointer_mut("/result/page/next") else {
        return Err(surface_error(
            SURFACE_STORE,
            "surface page response cursor is missing",
        ));
    };
    if next.is_null() {
        return Ok(());
    }
    let cursor = serde_json::from_value::<SurfaceCursorJson>(next.take())
        .map_err(|_| surface_error(SURFACE_STORE, "surface page response cursor is malformed"))?;
    let token = codec
        .encode(operation_tag, &cursor)
        .map_err(cursor_token_surface_error)?;
    *next = Json::String(token);
    Ok(())
}

fn request_body_error() -> SurfaceOperationErrorJson {
    surface_error(
        SURFACE_REQUEST,
        "surface request body is not a valid operation",
    )
}

fn cursor_token_surface_error(error: SurfaceCursorTokenError) -> SurfaceOperationErrorJson {
    match error.kind() {
        SurfaceCursorTokenErrorKind::StaleCursor => {
            surface_error(SURFACE_STALE_CURSOR, "surface cursor token is stale")
        }
        SurfaceCursorTokenErrorKind::Cursor | SurfaceCursorTokenErrorKind::Key => {
            surface_error(SURFACE_CURSOR, "surface cursor token is malformed")
        }
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

/// The number of declared request-body bytes still unread after the head, or `None` when there is
/// nothing to drain. An over-limit declared body yields `None` so it stays undrained and fails
/// closed with the connection reset that request already earns.
fn pending_body_len(partial: &PartialHttpRequest) -> Option<usize> {
    let content_length = partial.head.content_length?;
    if content_length > MAX_BODY_BYTES {
        return None;
    }
    let body_start = partial.header_end + 4;
    let body_end = body_start.checked_add(content_length)?;
    let remaining = body_end.saturating_sub(partial.buffer.len());
    (remaining > 0).then_some(remaining)
}

/// Read and discard the client's already-sent request body after a head-level error response so the
/// connection closes with a normal FIN. Closing a `TcpStream` with an unread body in the receive
/// buffer makes the OS send a TCP RST that discards the response the client has not yet consumed,
/// losing a 401 or any other head-level error. The drain is best effort: it consumes what the client
/// has already streamed and stops as soon as no more is pending, so a client that declared a body
/// but withheld it does not hold the connection open.
fn drain_request_body(stream: &mut TcpStream, mut remaining: usize, shutdown: &shutdown::Shutdown) {
    let mut chunk = [0; 4096];
    while remaining > 0 {
        if shutdown.requested().is_some() {
            return;
        }
        let limit = remaining.min(chunk.len());
        match stream.read(&mut chunk[..limit]) {
            Ok(0) => return,
            Ok(read) => remaining -= read,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
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
    ServiceUnavailable,
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
            Self::ServiceUnavailable => 503,
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
            Self::ServiceUnavailable => "Service Unavailable",
            Self::RequestHeaderFieldsTooLarge => "Request Header Fields Too Large",
        }
    }
}

fn write_response(
    stream: &mut TcpStream,
    response: &SurfaceHttpResponse,
    shutdown: &shutdown::Shutdown,
) -> std::io::Result<()> {
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
    let mut message = Vec::with_capacity(body.len() + 256);
    write!(
        message,
        "HTTP/1.1 {} {}\r\n",
        response.status.code(),
        response.status.reason()
    )?;
    if response.body.is_some() {
        message.extend_from_slice(b"Content-Type: application/json\r\n");
    }
    if let Some(origin) = &response.cors_origin {
        let allow_headers = response.cors_allow_headers.unwrap_or("Content-Type");
        write!(
            message,
            "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: {allow_headers}\r\n"
        )?;
    }
    if let Some(vary) = response.cors_vary {
        write!(message, "Vary: {vary}\r\n")?;
    }
    write!(
        message,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    message.extend_from_slice(&body);
    write_all_with_shutdown_poll(stream, &message, shutdown)?;
    stream.flush()
}

/// Write the whole response, waking every `POLL_INTERVAL` (the socket write timeout) to
/// re-check the shutdown signal and a total write deadline. Symmetric with
/// [`read_with_shutdown_poll`]: a first SIGTERM/SIGINT aborts the write promptly rather
/// than letting a slow-reading client hold a graceful shutdown for the full transfer, and
/// the total deadline bounds a paced reader — which keeps making slow progress, so a
/// progress-reset idle bound would never fire — from head-of-line-blocking other requests.
fn write_all_with_shutdown_poll(
    stream: &mut TcpStream,
    message: &[u8],
    shutdown: &shutdown::Shutdown,
) -> std::io::Result<()> {
    let deadline = Instant::now() + STREAM_TIMEOUT;
    let mut written = 0;
    while written < message.len() {
        match stream.write(&message[written..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    ErrorKind::WriteZero,
                    "surface response write made no progress",
                ));
            }
            Ok(count) => written += count,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                if shutdown.requested().is_some() {
                    return Err(std::io::Error::other("surface server is shutting down"));
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "surface response write exceeded the stream deadline",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
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
    use super::super::cursor_token::CursorTokenKeySource;
    use super::*;
    use marrow_json::surface::{SurfaceOperationKind, SurfaceRouteBinding};

    const VALID_CURSOR_TOKEN_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

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
    fn health_probe_maps_readiness_and_rejects_non_get() {
        let ready = health_response("GET", true);
        assert_eq!(ready.status.code(), 200);
        assert_eq!(ready.body, Some(serde_json::json!({ "status": "ready" })));

        let unavailable = health_response("GET", false);
        assert_eq!(unavailable.status.code(), 503);
        assert_eq!(
            unavailable.body,
            Some(serde_json::json!({ "status": "unavailable" }))
        );

        assert_eq!(health_response("POST", true).status.code(), 405);
    }

    #[test]
    fn request_log_line_carries_identity_status_and_latency() {
        let matched = RequestLog::from_head(&ParsedHead {
            method: "POST".into(),
            target: "/surface/v1/read/tag".into(),
            origin: HeaderOccurrence::Missing,
            access_control_request_method: HeaderOccurrence::Missing,
            access_control_request_headers: HeaderOccurrence::Missing,
            authorization: HeaderOccurrence::Missing,
            content_length: None,
            content_type_is_json: false,
        })
        .with_operation_tag("tag");
        assert_eq!(
            request_log_line(&matched, HttpStatus::Ok, Duration::from_millis(4)),
            "serve POST /surface/v1/read/tag 200 4ms op=tag"
        );

        assert_eq!(
            request_log_line(
                &RequestLog::unparsed(),
                HttpStatus::BadRequest,
                Duration::from_millis(0)
            ),
            "serve - - 400 0ms op=-"
        );
    }

    #[test]
    fn request_log_drops_query_string_from_the_target() {
        let log = RequestLog::from_head(&ParsedHead {
            method: "POST".into(),
            target: "/surface/v1/read/tag?token=secret".into(),
            origin: HeaderOccurrence::Missing,
            access_control_request_method: HeaderOccurrence::Missing,
            access_control_request_headers: HeaderOccurrence::Missing,
            authorization: HeaderOccurrence::Missing,
            content_length: None,
            content_type_is_json: false,
        });
        let line = request_log_line(&log, HttpStatus::NotFound, Duration::from_millis(1));
        assert!(
            !line.contains("secret"),
            "query must not reach the log: {line}"
        );
        assert!(
            line.contains("/surface/v1/read/tag "),
            "path is logged: {line}"
        );
    }

    #[test]
    fn cursor_token_route_body_profile_mismatch_stays_abi_mismatch() {
        let key_path = std::env::temp_dir().join(format!(
            "marrow-cursor-token-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::write(&key_path, VALID_CURSOR_TOKEN_KEY).expect("write cursor token key");
        let remote_cursor_token =
            RemoteCursorToken::load("kid_1", &CursorTokenKeySource::File(key_path.clone()))
                .expect("load cursor token key");
        let _ = std::fs::remove_file(&key_path);
        let route = SurfaceRouteBinding {
            path: "/surface/v1/read/op_tag".into(),
            operation_tag: "op_tag".into(),
            kind: SurfaceOperationKind::RangePage,
            surface_module: "test".into(),
            surface_name: "Posts".into(),
            alias: "byDate".into(),
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "profile_version": "surface.operation.v0",
            "operation_tag": "op_tag",
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [],
                    "range": {
                        "lower": { "kind": "date", "days_since_epoch": 10 },
                        "lower_inclusive": false
                    },
                    "limit": 1,
                    "cursor": "not-a-cursor-token"
                }
            }
        }))
        .expect("encode operation body");

        let error = operation_from_http_body(&body, &route, Some(&remote_cursor_token))
            .expect_err("profile mismatch fails before cursor token decode");

        assert_eq!(error.code, SURFACE_ABI_MISMATCH);
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

    // `Shutdown::test_pending` exists only in the Unix signal implementation.
    #[cfg(unix)]
    #[test]
    fn a_stalled_response_write_aborts_promptly_when_shutdown_is_requested() {
        use std::sync::Arc;
        use std::sync::atomic::Ordering;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("test listener address");
        // The client connects but never reads, so the server's write fills the socket buffers
        // and blocks — the absent/slow-reader case that starved a first-signal graceful shutdown.
        let client = std::net::TcpStream::connect(addr).expect("connect test listener");
        let (mut server, _) = listener.accept().expect("accept test connection");
        server
            .set_write_timeout(Some(POLL_INTERVAL))
            .expect("set test write timeout");

        let (shutdown, signal) = shutdown::Shutdown::test_pending();
        // Request shutdown shortly after the write stalls, mimicking a first SIGTERM mid-response.
        let trigger = {
            let signal = Arc::clone(&signal);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                signal.store(15, Ordering::SeqCst);
            })
        };

        // Far larger than any socket send/receive buffer, so the write cannot fully drain to the
        // idle client and must block until shutdown aborts it rather than run to completion.
        let message = vec![b'x'; 64 * 1024 * 1024];
        let start = std::time::Instant::now();
        let result = write_all_with_shutdown_poll(&mut server, &message, &shutdown);
        let elapsed = start.elapsed();
        trigger.join().expect("join shutdown trigger");
        drop(client);

        assert!(
            result.is_err(),
            "a shutdown mid-write must abort, not complete"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "graceful shutdown must abort a stalled write within a poll interval, took {elapsed:?}",
        );
    }
}
