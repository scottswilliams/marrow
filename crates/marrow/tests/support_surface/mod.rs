use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

pub(crate) struct ServeProcess {
    child: Child,
    stopped: bool,
    _stdout: BufReader<ChildStdout>,
    _stderr: ChildStderr,
}

pub(crate) struct SurfaceRoute {
    pub(crate) path: String,
    pub(crate) operation_tag: String,
}

pub(crate) fn route_by_alias(report: &Value, alias: &str) -> SurfaceRoute {
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

pub(crate) fn read_descriptor<'a>(report: &'a Value, operation_tag: &str) -> &'a Value {
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

pub(crate) fn update_descriptor<'a>(report: &'a Value, operation_tag: &str) -> &'a Value {
    report["surface_abi"]["surfaces"]
        .as_array()
        .expect("surface descriptors")
        .iter()
        .filter_map(|surface| surface.get("update"))
        .find(|update| update["operation_tag"] == operation_tag)
        .unwrap_or_else(|| panic!("update descriptor {operation_tag} in {report:#?}"))
}

pub(crate) fn create_descriptor<'a>(report: &'a Value, operation_tag: &str) -> &'a Value {
    report["surface_abi"]["surfaces"]
        .as_array()
        .expect("surface descriptors")
        .iter()
        .filter_map(|surface| surface.get("create"))
        .find(|create| create["operation_tag"] == operation_tag)
        .unwrap_or_else(|| panic!("create descriptor {operation_tag} in {report:#?}"))
}

pub(crate) fn delete_descriptor<'a>(report: &'a Value, operation_tag: &str) -> &'a Value {
    report["surface_abi"]["surfaces"]
        .as_array()
        .expect("surface descriptors")
        .iter()
        .filter_map(|surface| surface.get("delete"))
        .find(|delete| delete["operation_tag"] == operation_tag)
        .unwrap_or_else(|| panic!("delete descriptor {operation_tag} in {report:#?}"))
}

pub(crate) fn update_field_catalog_id(update: &Value, label: &str) -> String {
    update["fields"]
        .as_array()
        .expect("update fields")
        .iter()
        .find(|field| field["render_label"] == label)
        .and_then(|field| field["member_catalog_id"].as_str())
        .unwrap_or_else(|| panic!("update field {label} in {update:#?}"))
        .to_string()
}

pub(crate) fn create_field_catalog_id(create: &Value, label: &str) -> String {
    create["fields"]
        .as_array()
        .expect("create fields")
        .iter()
        .find(|field| field["render_label"] == label)
        .and_then(|field| field["member_catalog_id"].as_str())
        .unwrap_or_else(|| panic!("create field {label} in {create:#?}"))
        .to_string()
}

impl ServeProcess {
    /// Stop the server with SIGTERM — the documented foreground stop — and wait for it to exit.
    /// SIGTERM skips the process's destructors exactly as a real operator stop does, so the
    /// store is left in whatever on-disk state the running server held it in.
    pub(crate) fn stop_with_sigterm(mut self) {
        let pid = self.child.id().to_string();
        let _ = Command::new("kill").args(["-TERM", &pid]).status();
        let _ = self.child.wait();
        // The wait above reaps the child; clear it so Drop does not signal a stale pid.
        self.stopped = true;
    }
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Poll a generated client file until its contents differ from `before` or the deadline passes,
/// returning the final contents. The bound is generous so `serve --watch`'s poll cadence and the
/// re-check it triggers have room to land before the assertion runs.
pub(crate) fn wait_for_client_change(path: &Path, before: &str, deadline: Duration) -> String {
    let start = std::time::Instant::now();
    loop {
        let current = std::fs::read_to_string(path).unwrap_or_default();
        if current != before || start.elapsed() >= deadline {
            return current;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(crate) fn spawn_surface_server(root: &Path) -> (ServeProcess, SocketAddr) {
    spawn_surface_server_with_args(root, &[])
}

pub(crate) fn spawn_surface_server_with_args(
    root: &Path,
    extra_args: &[&str],
) -> (ServeProcess, SocketAddr) {
    let project = root.to_str().expect("project path utf8");
    let mut args = vec!["serve", "--addr", "127.0.0.1:0"];
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
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader.read_line(&mut line);
        let _ = tx.send((reader, line, result));
    });
    let (reader, line, result) = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            let mut error = String::new();
            stderr
                .read_to_string(&mut error)
                .expect("read timed-out server stderr");
            panic!("surface server did not print a listen line within 10s; stderr={error}");
        }
    };
    if result.expect("read listen line") == 0 {
        let status = child.wait().expect("wait failed surface server");
        let mut error = String::new();
        stderr
            .read_to_string(&mut error)
            .expect("read server stderr");
        panic!("surface server exited before listening: status={status:?} stderr={error}");
    }
    let addr_text = line
        .trim()
        .strip_prefix("serve listening on http://")
        .unwrap_or_else(|| panic!("unexpected listen line: {line:?}"));
    (
        ServeProcess {
            child,
            stopped: false,
            _stdout: reader,
            _stderr: stderr,
        },
        addr_text.parse().expect("listen address"),
    )
}
