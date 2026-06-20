use crate::support;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
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
\x20\x20\x20\x20update author\n\
\x20\x20\x20\x20collection ^books.byAuthor as byAuthor\n\
\x20\x20\x20\x20action retitle\n";

struct SurfaceFixture {
    _root: support::TempProject,
    report: Value,
}

struct ServeProcess {
    child: Child,
    _stdout: BufReader<ChildStdout>,
    _stderr: ChildStderr,
}

struct HttpResponse {
    status: u16,
    body: Value,
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn help_advertises_surface_serve_without_restoring_top_level_serve() {
    let output = marrow(&["--help"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("marrow surface serve [--write] [--addr <loopback:port>] <projectdir>"),
        "{stdout}"
    );
    assert!(
        !stdout.contains(&format!("marrow {} ", "serve")),
        "top-level serve must stay removed: {stdout}"
    );

    let serve_help = marrow(&["surface", "serve", "--help"]);
    assert_eq!(serve_help.status.code(), Some(0), "{serve_help:?}");
    let serve_stdout = String::from_utf8(serve_help.stdout).expect("serve stdout utf8");
    assert!(serve_stdout.contains("--write"), "{serve_stdout}");
    assert!(
        serve_stdout.contains("/surface/v1/{read|update|action}/<operation-tag>"),
        "{serve_stdout}"
    );
}

#[test]
fn surface_serve_rejects_non_loopback_before_project_load() {
    let dir = support::temp_dir("surface-serve-non-loopback");
    write(&dir, "marrow.json", support::native_config());
    write(&dir, "src/app.mw", "module app\npub fn broken(\n");

    let output = marrow(&[
        "surface",
        "serve",
        "--addr",
        "0.0.0.0:0",
        dir.to_str().unwrap(),
    ]);

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
fn surface_serve_fails_closed_on_request_shape_mismatches() {
    let fixture = seeded_surface_fixture("surface-serve-strict");
    let point_route = route_by_alias(&fixture.report, "get");
    let update_route = route_by_alias(&fixture.report, "update");
    let store_catalog_id =
        read_descriptor(&fixture.report, &point_route.operation_tag)["store_catalog_id"]
            .as_str()
            .expect("point read store catalog id")
            .to_string();
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
    assert_eq!(tag_mismatch.status, 400, "{:#?}", tag_mismatch.body);
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

struct SurfaceRoute {
    path: String,
    operation_tag: String,
}

fn route_by_alias(report: &Value, alias: &str) -> SurfaceRoute {
    let route = report["surface_routes"]["routes"]
        .as_array()
        .expect("surface routes")
        .iter()
        .find(|route| route["alias"] == alias)
        .unwrap_or_else(|| panic!("route alias {alias} in {report:#?}"));
    SurfaceRoute {
        path: route["path"].as_str().expect("route path").to_string(),
        operation_tag: route["operation_tag"]
            .as_str()
            .expect("operation tag")
            .to_string(),
    }
}

fn read_descriptor<'a>(report: &'a Value, operation_tag: &str) -> &'a Value {
    report["surface_abi"]["surfaces"]
        .as_array()
        .expect("surface descriptors")
        .iter()
        .flat_map(|surface| {
            surface["read"]
                .as_array()
                .expect("surface read descriptors")
                .iter()
        })
        .find(|read| read["operation_tag"] == operation_tag)
        .unwrap_or_else(|| panic!("read descriptor {operation_tag} in {report:#?}"))
}

fn update_descriptor<'a>(report: &'a Value, operation_tag: &str) -> &'a Value {
    report["surface_abi"]["surfaces"]
        .as_array()
        .expect("surface descriptors")
        .iter()
        .filter_map(|surface| surface.get("update"))
        .find(|update| update["operation_tag"] == operation_tag)
        .unwrap_or_else(|| panic!("update descriptor {operation_tag} in {report:#?}"))
}

fn update_field_catalog_id(update: &Value, label: &str) -> String {
    update["fields"]
        .as_array()
        .expect("update fields")
        .iter()
        .find(|field| field["render_label"] == label)
        .and_then(|field| field["member_catalog_id"].as_str())
        .unwrap_or_else(|| panic!("update field {label} in {update:#?}"))
        .to_string()
}

fn spawn_surface_server(root: &Path) -> (ServeProcess, SocketAddr) {
    spawn_surface_server_with_args(root, &[])
}

fn spawn_surface_server_with_args(root: &Path, extra_args: &[&str]) -> (ServeProcess, SocketAddr) {
    let project = root.to_str().expect("project path utf8");
    let mut args = vec!["surface", "serve", "--addr", "127.0.0.1:0"];
    args.extend(extra_args.iter().copied());
    args.push(project);
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn surface server");
    let stdout = child.stdout.take().expect("surface stdout pipe");
    let mut stderr = child.stderr.take().expect("surface stderr pipe");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    if reader.read_line(&mut line).expect("read listen line") == 0 {
        let status = child.wait().expect("wait failed surface server");
        let mut error = String::new();
        stderr
            .read_to_string(&mut error)
            .expect("read server stderr");
        panic!("surface server exited before listening: status={status:?} stderr={error}");
    }
    let addr_text = line
        .trim()
        .strip_prefix("surface serve listening on http://")
        .unwrap_or_else(|| panic!("unexpected listen line: {line:?}"));
    let addr = addr_text.parse().expect("listen address");
    (
        ServeProcess {
            child,
            _stdout: reader,
            _stderr: stderr,
        },
        addr,
    )
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
    HttpResponse {
        status,
        body: serde_json::from_str(body).expect("response json body"),
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
