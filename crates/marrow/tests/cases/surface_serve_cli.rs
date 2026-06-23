use crate::support;
use crate::support_surface::{
    create_descriptor, create_field_catalog_id, delete_descriptor, read_descriptor, route_by_alias,
    spawn_surface_server, spawn_surface_server_with_args, update_descriptor,
    update_field_catalog_id, wait_for_client_change,
};
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use support::{marrow, marrow_sub, temp_project, write};

const SURFACE_SOURCE: &str = "module app\n\
 \n\
 resource Book\n\
 \x20\x20\x20\x20required title: string\n\
 \x20\x20\x20\x20author: string\n\
 store ^books(id: int): Book\n\
 \x20\x20\x20\x20index byAuthor(author, id)\n\
 \n\
pub fn seed()\n\
\x20\x20\x20\x20var book: Book\n\
\x20\x20\x20\x20book.title = \"Dune\"\n\
\x20\x20\x20\x20book.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20var sequel: Book\n\
\x20\x20\x20\x20sequel.title = \"Dune Messiah\"\n\
\x20\x20\x20\x20sequel.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(2) = sequel\n\
\n\
pub fn retitle(id: int, title: string): string\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(id).title = title\n\
\x20\x20\x20\x20return title\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title, author\n\
\x20\x20\x20\x20create title, author\n\
\x20\x20\x20\x20update author\n\
\x20\x20\x20\x20delete\n\
\x20\x20\x20\x20collection ^books.byAuthor as byAuthor\n\
\x20\x20\x20\x20action retitle\n";

const SINGLETON_SURFACE_SOURCE: &str = "module app\n\
 \n\
 resource Settings\n\
 \x20\x20\x20\x20required theme: string\n\
 store ^settings: Settings\n\
 \n\
pub fn seed()\n\
\x20\x20\x20\x20var settings: Settings\n\
\x20\x20\x20\x20settings.theme = \"dark\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^settings = settings\n\
\n\
surface SettingsSurface from ^settings\n\
\x20\x20\x20\x20fields theme\n\
\x20\x20\x20\x20delete\n";

const CLIENT_SURFACE_SOURCE: &str = "module app\n\
\n\
resource Book\n\
\x20\x20\x20\x20required title: string\n\
\x20\x20\x20\x20author: string\n\
store ^books(id: int): Book\n\
\x20\x20\x20\x20index byAuthor(author, id)\n\
\n\
pub fn seed()\n\
\x20\x20\x20\x20var book: Book\n\
\x20\x20\x20\x20book.title = \"Dune\"\n\
\x20\x20\x20\x20book.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20var sequel: Book\n\
\x20\x20\x20\x20sequel.title = \"Dune Messiah\"\n\
\x20\x20\x20\x20sequel.author = \"Frank Herbert\"\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(1) = book\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(2) = sequel\n\
\n\
pub fn retitle(id: int, title: string): string\n\
\x20\x20\x20\x20transaction\n\
\x20\x20\x20\x20\x20\x20\x20\x20^books(id).title = title\n\
\x20\x20\x20\x20return title\n\
\n\
pub fn describe(id: int): string\n\
\x20\x20\x20\x20return (^books(id).title ?? \"\") + \"|\" + (^books(id).author ?? \"\")\n\
\n\
surface Books from ^books\n\
\x20\x20\x20\x20fields title, author\n\
\x20\x20\x20\x20create title, author\n\
\x20\x20\x20\x20update author\n\
\x20\x20\x20\x20delete\n\
\x20\x20\x20\x20collection ^books.byAuthor as byAuthor\n\
\x20\x20\x20\x20action retitle\n\
\x20\x20\x20\x20read describe\n";

/// A native-store config that declares a client output path so serve regenerates the TypeScript
/// client write-if-changed at startup and on a `--watch` source change.
fn native_config_with_client() -> String {
    r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#
        .to_string()
}

#[test]
fn serve_watch_rewrites_client_on_source_change() {
    let root = temp_project("serve-watch-client", |root| {
        write(root, "marrow.json", &native_config_with_client());
        write(root, "src/app.mw", CLIENT_SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "{seed:?}");
    let out = root.join("generated/marrow.ts");
    let before = std::fs::read_to_string(&out).expect("client present after seed");
    let (_server, _addr) = spawn_surface_server_with_args(&root, &["--write", "--watch"]);
    let changed = CLIENT_SURFACE_SOURCE.replace(
        "    read describe\n",
        "    read describe\n    read describe as summary\n",
    );
    write(&root, "src/app.mw", &changed);
    let after = wait_for_client_change(&out, &before, std::time::Duration::from_secs(8));
    assert_ne!(
        after, before,
        "serve --watch must rewrite the client on a surface change"
    );
}

#[test]
fn serve_startup_writes_declared_client() {
    let root = temp_project("serve-writes-client", |root| {
        write(
            root,
            "marrow.json",
            r#"{"sourceRoots":["src"],"store":{"backend":"native","dataDir":".data"},"client":"generated/marrow.ts"}"#,
        );
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let seed = marrow(&["run", "--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let out = root.join("generated/marrow.ts");
    std::fs::remove_file(&out).ok();

    let (_server, _addr) = spawn_surface_server(&root);
    assert!(
        out.exists(),
        "serve startup must regenerate the declared client"
    );
}

#[test]
fn serve_write_holds_the_store_lock_for_its_lifetime() {
    let root = temp_project("serve-write-lock", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let (_server, _addr) = spawn_surface_server_with_args(&root, &["--write"]);

    // A concurrent write-capable run must be refused while the write server owns the store.
    assert_store_locked(
        support::marrow_bounded(
            &["run", "--entry", "app::seed", project],
            std::time::Duration::from_secs(15),
        ),
        "racing run",
    );
    // A read-only inspection must also be refused: the write server excludes any other open.
    assert_store_locked(
        support::marrow_bounded(
            &["data", "stats", project],
            std::time::Duration::from_secs(15),
        ),
        "racing stats",
    );
    // A second write server must fail fast rather than coexisting; bind to an ephemeral port so a
    // regression that lets it listen forever surfaces as a bound timeout, not a passing test.
    assert_store_locked(
        support::marrow_bounded(
            &["serve", "--write", "--addr", "127.0.0.1:0", project],
            std::time::Duration::from_secs(15),
        ),
        "second serve --write",
    );
}

#[test]
fn read_only_serve_blocks_a_writer() {
    let root = temp_project("serve-read-blocks-writer", |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let project = root.to_str().expect("project path utf8");
    let seed = marrow(&["run", "--entry", "app::seed", project]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let (_server, _addr) = spawn_surface_server(&root);

    // A read-only serve holds a native read-only open, which excludes a write-capable command.
    assert_store_locked(
        support::marrow_bounded(
            &["run", "--entry", "app::seed", project],
            std::time::Duration::from_secs(15),
        ),
        "writer racing a read-only serve",
    );
}

/// Assert a CLI command was refused because the serve process holds the cross-process store lock.
/// The dotted code is read from the shared stderr fault line, the CLI's single owner of "which
/// stderr line is the fault" across run, data, and serve.
fn assert_store_locked(output: std::process::Output, what: &str) {
    assert_eq!(output.status.code(), Some(1), "{what}: {output:?}");
    let fault = support::last_fault(&output.stderr);
    let segments: Vec<&str> = fault.split(": ").collect();
    let (_, code) = support::find_code_segment(&segments);
    assert_eq!(
        code, "store.locked",
        "{what} must be refused store.locked: {output:?}"
    );
}

struct SurfaceFixture {
    _root: support::TempProject,
    report: Value,
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[test]
fn help_advertises_top_level_serve() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains(
            "marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>"
        ),
        "{stdout}"
    );
    assert!(
        !stdout.contains("surface serve"),
        "root help should not advertise removed surface commands: {stdout}"
    );

    let serve_help = marrow(&["serve", "--help"]);
    assert_eq!(serve_help.status.code(), Some(0), "{serve_help:?}");
    let serve_stdout = String::from_utf8(serve_help.stdout).expect("serve stdout utf8");
    assert!(
        serve_stdout.contains(
            "marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>"
        ),
        "{serve_stdout}"
    );
    assert!(serve_stdout.contains("--write"), "{serve_stdout}");
    assert!(serve_stdout.contains("--watch"), "{serve_stdout}");
    assert!(serve_stdout.contains("--cors-origin"), "{serve_stdout}");
    assert!(
        serve_stdout.contains("/surface/v1/{read|create|update|delete|action}/<operation-tag>"),
        "{serve_stdout}"
    );
}

#[test]
fn surface_serve_rejects_non_loopback_before_project_load() {
    let dir = support::temp_dir("surface-serve-non-loopback");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");

    let output = marrow(&["serve", "--addr", "0.0.0.0:0", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "usage failure should not write stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("loopback"), "{stderr}");
    assert!(
        !stderr.contains("parse."),
        "bind validation should fail before source loading: {stderr}"
    );
}

#[test]
fn surface_serve_rejects_non_loopback_cors_origin_before_project_load() {
    let dir = support::temp_dir("surface-serve-non-loopback-cors-origin");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");

    let output = marrow(&[
        "serve",
        "--cors-origin",
        "https://example.com",
        dir.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "usage failure should not write stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("loopback origin"), "{stderr}");
    assert!(
        !stderr.contains("parse."),
        "CORS origin validation should fail before source loading: {stderr}"
    );
}

#[test]
fn surface_serve_executes_manifest_point_read_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-point-read");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());

    let response = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": point_route.operation_tag,
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id.clone(),
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.body["profile_version"], "surface.operation.v1");
    assert_eq!(response.body["operation_tag"], point_route.operation_tag);
    assert_eq!(response.body["result"]["kind"], "record");
    let record = &response.body["result"]["record"];
    assert_eq!(
        field_value(record, "title"),
        json!({ "kind": "string", "value": "Dune" })
    );
    assert_eq!(
        field_value(record, "author"),
        json!({ "kind": "string", "value": "Frank Herbert" })
    );
}

#[test]
fn surface_serve_cors_origin_allows_exact_local_browser_origin() {
    let fixture = seeded_surface_fixture("surface-serve-cors-origin");
    let point_route = route_by_alias(&fixture.report, "get");
    let origin = "http://localhost:5173";
    let (_server, addr) =
        spawn_surface_server_with_args(fixture.root(), &["--cors-origin", origin]);

    let preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(preflight.status, 204, "{:#?}", preflight.body);
    assert_eq!(
        preflight.header("access-control-allow-origin"),
        Some(origin)
    );
    assert_eq!(
        preflight.header("access-control-allow-methods"),
        Some("POST, OPTIONS")
    );
    assert_eq!(
        preflight.header("access-control-allow-headers"),
        Some("Content-Type")
    );
    assert_eq!(preflight.header("vary"), Some("Origin"));

    let non_empty_preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 2\r\n\r\n{{}}",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        non_empty_preflight.status, 400,
        "{:#?}",
        non_empty_preflight.body
    );
    assert_eq!(non_empty_preflight.body["code"], "surface.request");
    assert_eq!(
        non_empty_preflight.header("access-control-allow-origin"),
        Some(origin)
    );

    let blocked_preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://example.com\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        blocked_preflight.status, 403,
        "{:#?}",
        blocked_preflight.body
    );
    assert_eq!(blocked_preflight.body["code"], "surface.request");
    assert_eq!(
        blocked_preflight.header("access-control-allow-origin"),
        None
    );

    let blocked_post = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[
            ("Content-Type", "application/json"),
            ("Origin", "http://example.com"),
        ],
    );
    assert_eq!(blocked_post.status, 403, "{:#?}", blocked_post.body);
    assert_eq!(blocked_post.body["code"], "surface.request");
    assert_eq!(blocked_post.header("access-control-allow-origin"), None);

    let response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json"), ("Origin", origin)],
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.header("access-control-allow-origin"), Some(origin));
    assert_eq!(response.header("vary"), Some("Origin"));
}

#[test]
fn surface_serve_cors_origin_echoes_configured_origin_for_casing_variant() {
    let fixture = seeded_surface_fixture("surface-serve-cors-origin-casing");
    let point_route = route_by_alias(&fixture.report, "get");
    let origin = "http://localhost:5173";
    let request_origin = "HTTP://LoCaLhOsT:5173";
    let (_server, addr) =
        spawn_surface_server_with_args(fixture.root(), &["--cors-origin", origin]);

    let preflight = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {request_origin}\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(preflight.status, 204, "{:#?}", preflight.body);
    assert_eq!(
        preflight.header("access-control-allow-origin"),
        Some(origin)
    );

    let response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[
            ("Content-Type", "application/json"),
            ("Origin", request_origin),
        ],
    );
    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.header("access-control-allow-origin"), Some(origin));

    let blocked = raw_http(
        addr,
        format!(
            "OPTIONS {} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://example.com\r\nAccess-Control-Request-Method: POST\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(blocked.status, 403, "{:#?}", blocked.body);
    assert_eq!(blocked.header("access-control-allow-origin"), None);
}

#[test]
fn surface_serve_fails_closed_on_request_shape_mismatches() {
    let fixture = seeded_surface_fixture("surface-serve-strict");
    let point_route = route_by_alias(&fixture.report, "get");
    let create_route = route_by_alias(&fixture.report, "create");
    let delete_route = route_by_alias(&fixture.report, "delete");
    let update_route = route_by_alias(&fixture.report, "update");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let create = create_descriptor(&fixture.report, &create_route.operation_tag);
    let title_catalog_id = create_field_catalog_id(create, "title");
    let author_catalog_id = create_field_catalog_id(create, "author");
    let (_server, addr) = spawn_surface_server(fixture.root());
    let good_body = json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": point_route.operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": "1" }]
                }
            }
        }
    });

    let missing_content_type = post_json(addr, &point_route.path, good_body.clone(), &[]);
    assert_eq!(
        missing_content_type.status, 415,
        "{:#?}",
        missing_content_type.body
    );
    assert_eq!(missing_content_type.body["code"], "surface.request");

    let tag_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "request": good_body["request"].clone()
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(tag_mismatch.status, 404, "{:#?}", tag_mismatch.body);
    assert_eq!(tag_mismatch.body["code"], "surface.abi_mismatch");

    let kind_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": point_route.operation_tag,
            "request": { "kind": "singleton_read" }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(kind_mismatch.status, 400, "{:#?}", kind_mismatch.body);
    assert_eq!(kind_mismatch.body["code"], "surface.request");

    let write_route = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": []
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(write_route.status, 404, "{:#?}", write_route.body);
    assert_eq!(write_route.body["code"], "surface.abi_mismatch");

    let create_route_response = post_json(
        addr,
        &create_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": create_route.operation_tag,
            "request": {
                "kind": "point_create",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "3" }]
                    },
                    "fields": [
                        {
                            "catalog_id": title_catalog_id,
                            "value": { "kind": "string", "value": "Children of Dune" }
                        },
                        {
                            "catalog_id": author_catalog_id,
                            "value": { "kind": "string", "value": "Frank Herbert" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(
        create_route_response.status, 404,
        "{:#?}",
        create_route_response.body
    );
    assert_eq!(create_route_response.body["code"], "surface.abi_mismatch");

    let delete_route_response = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": {
                "kind": "point_delete",
                "request": {
                    "identity": {
                        "store_catalog_id": delete_descriptor(&fixture.report, &delete_route.operation_tag)["store_catalog_id"],
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(
        delete_route_response.status, 404,
        "{:#?}",
        delete_route_response.body
    );
    assert_eq!(delete_route_response.body["code"], "surface.abi_mismatch");
}

#[test]
fn surface_serve_reports_abi_mismatch_as_not_found() {
    let fixture = seeded_surface_fixture("surface-serve-abi-mismatch-404");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());

    // An operation tag the route no longer serves is the wrong-route/stale-client class: 404.
    let tag_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(tag_mismatch.status, 404, "{:#?}", tag_mismatch.body);
    assert_eq!(tag_mismatch.body["code"], "surface.abi_mismatch");

    // A stale profile version surfaces from the runtime executor as abi_mismatch: also 404.
    let profile_mismatch = post_json(
        addr,
        &point_route.path,
        json!({
            "profile_version": "surface.operation.v0",
            "operation_tag": point_route.operation_tag,
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(profile_mismatch.status, 404, "{:#?}", profile_mismatch.body);
    assert_eq!(profile_mismatch.body["code"], "surface.abi_mismatch");
}

#[test]
fn surface_serve_rejects_unknown_json_fields_without_mutation() {
    let fixture = seeded_surface_fixture("surface-serve-unknown-json");
    let point_route = route_by_alias(&fixture.report, "get");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let update_route = route_by_alias(&fixture.report, "update");
    let update = update_descriptor(&fixture.report, &update_route.operation_tag);
    let store_catalog_id = update["store_catalog_id"]
        .as_str()
        .expect("update store catalog id")
        .to_string();
    let author_catalog_id = update_field_catalog_id(update, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let mut top_level = point_read_request(&fixture.report, &point_route.operation_tag, 1);
    top_level["smuggled"] = json!(true);
    let response = post_json(
        addr,
        &point_route.path,
        top_level,
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");

    let mut request = point_read_request(&fixture.report, &point_route.operation_tag, 1);
    request["request"]["request"]["smuggled"] = json!("ignored-if-not-strict");
    let response = post_json(
        addr,
        &point_route.path,
        request,
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");

    let mut identity = point_read_request(&fixture.report, &point_route.operation_tag, 1);
    identity["request"]["request"]["identity"]["smuggled"] = json!("wrong-store");
    let response = post_json(
        addr,
        &point_route.path,
        identity,
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");

    let first_page = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 1
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    let mut cursor = first_page.body["result"]["page"]["next"].clone();
    assert!(
        cursor.is_object(),
        "expected page cursor: {:#?}",
        first_page.body
    );
    cursor["smuggled"] = json!("old-boundary");
    let cursor_response = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 10,
                    "cursor": cursor
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(cursor_response.status, 400, "{:#?}", cursor_response.body);
    assert_eq!(cursor_response.body["code"], "surface.request");

    let mut boundary_cursor = first_page.body["result"]["page"]["next"].clone();
    boundary_cursor["boundary"]["smuggled"] = json!("wrong-anchor");
    let boundary_response = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 10,
                    "cursor": boundary_cursor
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(
        boundary_response.status, 400,
        "{:#?}",
        boundary_response.body
    );
    assert_eq!(boundary_response.body["code"], "surface.request");

    let update_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": { "kind": "string", "value": "Ursula Le Guin" },
                        "smuggled": { "catalog_id": author_catalog_id, "value": { "kind": "string", "value": "wrong" } }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(update_response.status, 400, "{:#?}", update_response.body);
    assert_eq!(update_response.body["code"], "surface.request");

    let value_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": {
                            "kind": "string",
                            "value": "Ursula Le Guin",
                            "smuggled": "alternate"
                        }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(value_response.status, 400, "{:#?}", value_response.body);
    assert_eq!(value_response.body["code"], "surface.request");

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "author"),
        json!({ "kind": "string", "value": "Frank Herbert" })
    );
}

#[test]
fn surface_serve_non_post_method_names_post() {
    let fixture = seeded_surface_fixture("surface-serve-non-post");
    let point_route = route_by_alias(&fixture.report, "get");
    let (_server, addr) = spawn_surface_server(fixture.root());

    let response = raw_http(
        addr,
        format!("GET {} HTTP/1.1\r\nHost: {addr}\r\n\r\n", point_route.path).into_bytes(),
        &[],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    assert_eq!(response.body["code"], "surface.request");
    let message = response.body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("POST"),
        "a non-POST request must be told POST is required: {message}"
    );
}

#[test]
fn surface_serve_negative_page_limit_reports_limit_must_be_positive() {
    let fixture = seeded_surface_fixture("surface-serve-negative-limit");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let (_server, addr) = spawn_surface_server(fixture.root());

    let response = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": -1
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(response.status, 400, "{:#?}", response.body);
    let message = response.body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("greater than zero"),
        "a negative page limit must route through the limit-must-be-greater-than-zero branch: {message}"
    );
}

#[test]
fn surface_serve_write_mode_executes_sparse_update_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-write-update");
    let point_route = route_by_alias(&fixture.report, "get");
    let update_route = route_by_alias(&fixture.report, "update");
    let update = update_descriptor(&fixture.report, &update_route.operation_tag);
    let store_catalog_id = update["store_catalog_id"]
        .as_str()
        .expect("update store catalog id")
        .to_string();
    let author_catalog_id = update_field_catalog_id(update, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let kind_mismatch = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_read",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(kind_mismatch.status, 400, "{:#?}", kind_mismatch.body);
    assert_eq!(kind_mismatch.body["code"], "surface.request");

    let update_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": { "kind": "string", "value": "Brian Herbert" }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(update_response.status, 200, "{:#?}", update_response.body);
    assert_eq!(update_response.body["result"]["kind"], "updated");

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "author"),
        json!({ "kind": "string", "value": "Brian Herbert" })
    );
}

#[test]
fn surface_serve_write_mode_executes_create_and_delete_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-write-create-delete");
    let point_route = route_by_alias(&fixture.report, "get");
    let create_route = route_by_alias(&fixture.report, "create");
    let delete_route = route_by_alias(&fixture.report, "delete");
    let create = create_descriptor(&fixture.report, &create_route.operation_tag);
    let delete = delete_descriptor(&fixture.report, &delete_route.operation_tag);
    let store_catalog_id = create["store_catalog_id"]
        .as_str()
        .expect("create store catalog id")
        .to_string();
    assert_eq!(
        delete["store_catalog_id"]
            .as_str()
            .expect("delete store catalog id"),
        store_catalog_id
    );
    let title_catalog_id = create_field_catalog_id(create, "title");
    let author_catalog_id = create_field_catalog_id(create, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let create_response = post_json(
        addr,
        &create_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": create_route.operation_tag,
            "request": {
                "kind": "point_create",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "3" }]
                    },
                    "fields": [
                        {
                            "catalog_id": title_catalog_id,
                            "value": { "kind": "string", "value": "Children of Dune" }
                        },
                        {
                            "catalog_id": author_catalog_id,
                            "value": { "kind": "string", "value": "Frank Herbert" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(create_response.status, 200, "{:#?}", create_response.body);
    assert_eq!(create_response.body["result"]["kind"], "created");
    assert_eq!(
        field_value(&create_response.body["result"]["record"], "title"),
        json!({ "kind": "string", "value": "Children of Dune" })
    );

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 3),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "author"),
        json!({ "kind": "string", "value": "Frank Herbert" })
    );

    let delete_response = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": {
                "kind": "point_delete",
                "request": {
                    "identity": {
                        "store_catalog_id": delete["store_catalog_id"],
                        "keys": [{ "kind": "int", "value": "3" }]
                    }
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(delete_response.status, 200, "{:#?}", delete_response.body);
    assert_eq!(delete_response.body["result"]["kind"], "deleted");

    let absent_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 3),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(absent_response.status, 404, "{:#?}", absent_response.body);
    assert_eq!(absent_response.body["code"], "surface.absent");
}

#[test]
fn surface_serve_rejects_garbage_singleton_bodies_without_mutation() {
    let fixture = seeded_singleton_fixture("surface-serve-singleton-strict");
    let read_route = route_by_alias(&fixture.report, "get");
    let delete_route = route_by_alias(&fixture.report, "delete");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    // The empty closed object is the valid singleton-read request body.
    let valid_read = post_json(
        addr,
        &read_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": read_route.operation_tag,
            "request": { "kind": "singleton_read", "request": {} }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(valid_read.status, 200, "{:#?}", valid_read.body);
    assert_eq!(
        field_value(&valid_read.body["result"]["record"], "theme"),
        json!({ "kind": "string", "value": "dark" })
    );

    let garbage_bodies = [
        json!({ "kind": "singleton_read", "request": { "unexpected": true } }),
        json!({ "kind": "singleton_read", "request": "garbage" }),
        json!({ "kind": "singleton_read", "request": [] }),
        json!({ "kind": "singleton_read" }),
    ];
    for body in garbage_bodies {
        let response = post_json(
            addr,
            &read_route.path,
            json!({
                "profile_version": "surface.operation.v1",
                "operation_tag": read_route.operation_tag,
                "request": body,
            }),
            &[("Content-Type", "application/json")],
        );
        assert_eq!(response.status, 400, "{body:#?} -> {:#?}", response.body);
        assert_eq!(response.body["code"], "surface.request", "{body:#?}");
    }

    // A garbage-body singleton delete must be rejected before it can delete.
    let garbage_delete = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": { "kind": "singleton_delete", "request": { "unexpected": true } }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(garbage_delete.status, 400, "{:#?}", garbage_delete.body);
    assert_eq!(garbage_delete.body["code"], "surface.request");

    let still_present = post_json(
        addr,
        &read_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": read_route.operation_tag,
            "request": { "kind": "singleton_read", "request": {} }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(still_present.status, 200, "{:#?}", still_present.body);
    assert_eq!(
        field_value(&still_present.body["result"]["record"], "theme"),
        json!({ "kind": "string", "value": "dark" })
    );

    // The valid empty delete body removes the singleton.
    let valid_delete = post_json(
        addr,
        &delete_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": delete_route.operation_tag,
            "request": { "kind": "singleton_delete", "request": {} }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(valid_delete.status, 200, "{:#?}", valid_delete.body);
    assert_eq!(valid_delete.body["result"]["kind"], "deleted");
}

#[test]
fn surface_serve_write_mode_kill_leaves_a_recoverable_store() {
    let fixture = seeded_surface_fixture("surface-serve-write-idle-kill");
    let create_route = route_by_alias(&fixture.report, "create");
    let create = create_descriptor(&fixture.report, &create_route.operation_tag);
    let store_catalog_id = create["store_catalog_id"]
        .as_str()
        .expect("create store catalog id")
        .to_string();
    let title_catalog_id = create_field_catalog_id(create, "title");
    let author_catalog_id = create_field_catalog_id(create, "author");
    let (server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let create_response = post_json(
        addr,
        &create_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": create_route.operation_tag,
            "request": {
                "kind": "point_create",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "3" }]
                    },
                    "fields": [
                        {
                            "catalog_id": title_catalog_id,
                            "value": { "kind": "string", "value": "Children of Dune" }
                        },
                        {
                            "catalog_id": author_catalog_id,
                            "value": { "kind": "string", "value": "Frank Herbert" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(create_response.status, 200, "{:#?}", create_response.body);

    // A write serve holds the native writer lock for its whole lifetime, so a SIGKILL skips redb's
    // clean-shutdown marker: the store is left needing a write-capable recovery, not torn. The
    // committed create must survive the replay.
    drop(server);

    let root = fixture.root().to_str().unwrap();
    let locked_dump = marrow_sub("data", &["dump", "--format", "json", root]);
    assert_eq!(
        locked_dump.status.code(),
        Some(1),
        "a read-only open after a killed write serve must report recovery, not open: {locked_dump:?}"
    );
    assert_eq!(
        support::json(locked_dump.stdout)["code"],
        json!("store.recovery_required"),
        "a killed write serve must leave a recoverable store"
    );

    let recover = marrow_sub("data", &["recover", root]);
    assert_eq!(recover.status.code(), Some(0), "data recover: {recover:?}");

    let dump = marrow_sub("data", &["dump", "--format", "json", root]);
    assert_eq!(
        dump.status.code(),
        Some(0),
        "data dump must open the store cleanly after recovery: stdout={} stderr={}",
        String::from_utf8_lossy(&dump.stdout),
        String::from_utf8_lossy(&dump.stderr)
    );
    let dumped: Value = support::json(dump.stdout);
    let titles: Vec<&str> = dumped["cells"]
        .as_array()
        .expect("dump cells")
        .iter()
        .filter(|cell| cell["path"] == "^books(3).title")
        .filter_map(|cell| cell["value_b64"].as_str())
        .collect();
    assert_eq!(
        titles,
        ["Q2hpbGRyZW4gb2YgRHVuZQ=="],
        "committed record must survive idle serve shutdown: {dumped:#?}"
    );
}

#[test]
fn surface_serve_write_mode_executes_action_over_http() {
    let fixture = seeded_surface_fixture("surface-serve-write-action");
    let point_route = route_by_alias(&fixture.report, "get");
    let action_route = route_by_alias(&fixture.report, "retitle");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let action_response = post_json(
        addr,
        &action_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": action_route.operation_tag,
            "request": {
                "kind": "action",
                "request": {
                    "arguments": [
                        {
                            "name": "id",
                            "value": { "kind": "int", "value": "1" }
                        },
                        {
                            "name": "title",
                            "value": { "kind": "string", "value": "Dune HTTP" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(action_response.status, 200, "{:#?}", action_response.body);
    assert_eq!(action_response.body["result"]["kind"], "action");
    assert_eq!(action_response.body["result"]["result"]["output"], "");
    assert_eq!(
        action_response.body["result"]["result"]["value"],
        json!({ "kind": "string", "value": "Dune HTTP" })
    );

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "title"),
        json!({ "kind": "string", "value": "Dune HTTP" })
    );
}

#[test]
fn surface_serve_write_mode_executes_startup_source_snapshot() {
    let fixture = seeded_surface_fixture("surface-serve-write-startup-snapshot");
    let point_route = route_by_alias(&fixture.report, "get");
    let action_route = route_by_alias(&fixture.report, "retitle");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);
    let edited_source = SURFACE_SOURCE.replace(
        "        ^books(id).title = title\n    return title\n",
        "        ^books(id).title = \"Edited Source\"\n    return \"Edited Source\"\n",
    );
    assert_ne!(edited_source, SURFACE_SOURCE);
    write(fixture.root(), "src/app.mw", &edited_source);

    let action_response = post_json(
        addr,
        &action_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": action_route.operation_tag,
            "request": {
                "kind": "action",
                "request": {
                    "arguments": [
                        {
                            "name": "id",
                            "value": { "kind": "int", "value": "1" }
                        },
                        {
                            "name": "title",
                            "value": { "kind": "string", "value": "Dune Startup" }
                        }
                    ]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(action_response.status, 200, "{:#?}", action_response.body);
    assert_eq!(
        action_response.body["result"]["result"]["value"],
        json!({ "kind": "string", "value": "Dune Startup" })
    );

    let read_response = post_json(
        addr,
        &point_route.path,
        point_read_request(&fixture.report, &point_route.operation_tag, 1),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(read_response.status, 200, "{:#?}", read_response.body);
    assert_eq!(
        field_value(&read_response.body["result"]["record"], "title"),
        json!({ "kind": "string", "value": "Dune Startup" })
    );
}

#[test]
fn surface_serve_write_mode_reports_stale_cursor_as_conflict() {
    let fixture = seeded_surface_fixture("surface-serve-write-stale-cursor");
    let page_route = route_by_alias(&fixture.report, "byAuthor");
    let update_route = route_by_alias(&fixture.report, "update");
    let update = update_descriptor(&fixture.report, &update_route.operation_tag);
    let store_catalog_id = update["store_catalog_id"]
        .as_str()
        .expect("update store catalog id")
        .to_string();
    let author_catalog_id = update_field_catalog_id(update, "author");
    let (_server, addr) = spawn_surface_server_with_args(fixture.root(), &["--write"]);

    let first_page = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 1
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(first_page.status, 200, "{:#?}", first_page.body);
    let cursor = first_page.body["result"]["page"]["next"].clone();
    assert!(
        cursor.is_object(),
        "first page must return a cursor: {:#?}",
        first_page.body
    );

    let update_response = post_json(
        addr,
        &update_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": update_route.operation_tag,
            "request": {
                "kind": "point_update",
                "request": {
                    "identity": {
                        "store_catalog_id": store_catalog_id,
                        "keys": [{ "kind": "int", "value": "1" }]
                    },
                    "fields": [{
                        "catalog_id": author_catalog_id,
                        "value": { "kind": "string", "value": "Brian Herbert" }
                    }]
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );
    assert_eq!(update_response.status, 200, "{:#?}", update_response.body);

    let stale_page = post_json(
        addr,
        &page_route.path,
        json!({
            "profile_version": "surface.operation.v1",
            "operation_tag": page_route.operation_tag,
            "request": {
                "kind": "page",
                "request": {
                    "exact_keys": [{ "kind": "string", "value": "Frank Herbert" }],
                    "limit": 10,
                    "cursor": cursor
                }
            }
        }),
        &[("Content-Type", "application/json")],
    );

    assert_eq!(stale_page.status, 409, "{:#?}", stale_page.body);
    assert_eq!(stale_page.body["code"], "surface.stale_cursor");
}

#[test]
fn surface_serve_rejects_smuggled_or_unbounded_http_shapes() {
    let fixture = seeded_surface_fixture("surface-serve-http-shapes");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());
    let body = serde_json::to_vec(&json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": point_route.operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": "1" }]
                }
            }
        }
    }))
    .expect("request json");

    let duplicate_length = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(duplicate_length.status, 400, "{:#?}", duplicate_length.body);
    assert_eq!(duplicate_length.body["code"], "surface.request");

    let duplicate_content_type = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        duplicate_content_type.status, 400,
        "{:#?}",
        duplicate_content_type.body
    );
    assert_eq!(duplicate_content_type.body["code"], "surface.request");

    let transfer_encoding = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(
        transfer_encoding.status, 400,
        "{:#?}",
        transfer_encoding.body
    );
    assert_eq!(transfer_encoding.body["code"], "surface.request");

    let oversized_header = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nX-Fill: {}\r\nContent-Length: 0\r\n\r\n",
            point_route.path,
            "a".repeat(16 * 1024)
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(oversized_header.status, 431, "{:#?}", oversized_header.body);
    assert_eq!(oversized_header.body["code"], "surface.limit");

    let mut pipelined = body.clone();
    pipelined.extend_from_slice(b"GET /surface/v1/read/unused HTTP/1.1\r\n\r\n");
    let pipelined = raw_http(
        addr,
        format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            point_route.path,
            body.len()
        )
        .into_bytes(),
        &pipelined,
    );
    assert_eq!(pipelined.status, 400, "{:#?}", pipelined.body);
    assert_eq!(pipelined.body["code"], "surface.request");

    let wrong_method = raw_http(
        addr,
        format!(
            "GET {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            point_route.path
        )
        .into_bytes(),
        &[],
    );
    assert_eq!(wrong_method.status, 405, "{:#?}", wrong_method.body);
    assert_eq!(wrong_method.body["code"], "surface.request");
}

#[test]
fn surface_serve_processes_at_most_one_paced_request_per_connection() {
    let fixture = seeded_surface_fixture("surface-serve-paced-pipeline");
    let point_route = route_by_alias(&fixture.report, "get");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
    let (_server, addr) = spawn_surface_server(fixture.root());
    let body = json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": point_route.operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": "1" }]
                }
            }
        }
    });
    let response = paced_pipeline(
        addr,
        &point_route.path,
        body,
        b"POST /surface/v1/read/unused HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(response.body["result"]["kind"], "record");
}

impl SurfaceFixture {
    fn root(&self) -> &Path {
        &self._root
    }
}

fn seeded_surface_fixture(name: &str) -> SurfaceFixture {
    let root = temp_project(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SURFACE_SOURCE);
    });
    let seed = marrow_sub("run", &["--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let checked = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);
    assert_eq!(checked.status.code(), Some(0), "check: {checked:?}");
    SurfaceFixture {
        _root: root,
        report: support::json(checked.stdout),
    }
}

fn seeded_singleton_fixture(name: &str) -> SurfaceFixture {
    let root = temp_project(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SINGLETON_SURFACE_SOURCE);
    });
    let seed = marrow_sub("run", &["--entry", "app::seed", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    let checked = marrow_sub("check", &["--format", "json", root.to_str().unwrap()]);
    assert_eq!(checked.status.code(), Some(0), "check: {checked:?}");
    SurfaceFixture {
        _root: root,
        report: support::json(checked.stdout),
    }
}

fn point_read_request(report: &Value, operation_tag: &str, id: i64) -> Value {
    let store_catalog_id = read_descriptor(report, operation_tag)["store_catalog_id"]
        .as_str()
        .expect("point read store catalog id")
        .to_string();
    json!({
        "profile_version": "surface.operation.v1",
        "operation_tag": operation_tag,
        "request": {
            "kind": "point_read",
            "request": {
                "identity": {
                    "store_catalog_id": store_catalog_id,
                    "keys": [{ "kind": "int", "value": id.to_string() }]
                }
            }
        }
    })
}

fn post_json(addr: SocketAddr, path: &str, body: Value, headers: &[(&str, &str)]) -> HttpResponse {
    let body = serde_json::to_vec(&body).expect("request json");
    let mut stream = TcpStream::connect(addr).expect("connect surface server");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\n",
        body.len()
    )
    .expect("write request line");
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").expect("write request header");
    }
    stream.write_all(b"\r\n").expect("finish headers");
    stream.write_all(&body).expect("write body");
    stream.flush().expect("flush request");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("finish request");

    parse_response(&read_response(stream))
}

fn raw_http(addr: SocketAddr, mut head: Vec<u8>, body: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).expect("connect surface server");
    head.extend_from_slice(body);
    stream.write_all(&head).expect("write raw request");
    stream.flush().expect("flush raw request");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("finish raw request");
    parse_response(&read_response(stream))
}

fn paced_pipeline(addr: SocketAddr, path: &str, body: Value, delayed_extra: &[u8]) -> HttpResponse {
    let body = serde_json::to_vec(&body).expect("request json");
    let mut stream = TcpStream::connect(addr).expect("connect surface server");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .expect("write paced request headers");
    stream.write_all(&body).expect("write paced body");
    stream.flush().expect("flush paced request");
    std::thread::sleep(Duration::from_millis(25));
    let _ = stream.write_all(delayed_extra);
    let _ = stream.flush();
    let raw = read_response(stream);
    let response_count = String::from_utf8_lossy(&raw)
        .match_indices("HTTP/1.1 ")
        .count();
    assert_eq!(
        response_count,
        1,
        "surface server must emit one response per connection: {}",
        String::from_utf8_lossy(&raw)
    );
    parse_response(&raw)
}

fn read_response(mut stream: TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::ConnectionReset && !raw.is_empty() => {}
        Err(error) => panic!("read response: {error}"),
    }
    raw
}

fn parse_response(raw: &[u8]) -> HttpResponse {
    let text = String::from_utf8(raw.to_vec()).expect("response utf8");
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("response missing header terminator: {text:?}"));
    let status = head
        .lines()
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.to_string(), value.trim().to_string()))
        })
        .collect();
    HttpResponse {
        status,
        headers,
        body: if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body).expect("response json body")
        },
    }
}

fn field_value(record: &Value, label: &str) -> Value {
    record["fields"]
        .as_array()
        .expect("record fields")
        .iter()
        .find(|field| field["render_label"] == label)
        .and_then(|field| field["value"].as_object().map(|_| field["value"].clone()))
        .unwrap_or_else(|| panic!("field {label} in {record:#?}"))
}
