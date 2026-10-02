//! Local UNIX-socket JSON line protocol (LuCI reaches it through the rpcd shim).
//!
//! * transport: `/var/run/wifisync/wifisync.sock`, mode 0600;
//! * protocol: one JSON request per line `{"id":1,"method":"status","params":{}}`,
//!   answered with one line `{"id":1,"ok":true,"result":{...}}`.
//!
//! HTTP is deliberately avoided: it drops the `hyper`/`httparse` dependencies and one layer of
//! protocol parsing surface.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use wifisync_sys::error::SysResult;

/// Client call.
pub fn call(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| {
        format!(
            "cannot connect to the wifisync service ({}): {}",
            socket.display(),
            e
        )
    })?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .ok();
    let request = json!({ "id": 1, "method": method, "params": params });
    let mut line = request.to_string();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| format!("writing the request failed: {}", e))?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|e| format!("reading the response failed: {}", e))?;
    if response.trim().is_empty() {
        return Err("the service returned nothing".to_string());
    }
    let value: Value = serde_json::from_str(response.trim())
        .map_err(|e| format!("the response is not valid JSON: {}", e))?;
    if value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string())
    }
}

/// Server.
pub struct Server {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl Server {
    /// Bind the socket (cleaning up any leftover socket file), and tighten its permissions.
    pub fn bind(socket_path: &Path) -> SysResult<Self> {
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }
        let listener = UnixListener::bind(socket_path)?;
        let mut perms = std::fs::metadata(socket_path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(socket_path, perms)?;
        Ok(Self {
            listener,
            socket_path: socket_path.to_path_buf(),
        })
    }

    /// Start the accept loop (blocks the current thread).
    pub fn serve<F>(&self, handler: F) -> SysResult<()>
    where
        F: Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let handler = std::sync::Arc::new(handler);
        for stream in self.listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(e) => {
                    // A single failed connection must not stop the service
                    crate::log_warn!("accepting the connection failed: {}", e);
                    continue;
                }
            };
            let handler = handler.clone();
            std::thread::spawn(move || {
                if let Err(e) = handle_connection(stream, handler.as_ref()) {
                    crate::log_warn!("handling the connection failed: {}", e);
                }
            });
        }
        Ok(())
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

fn handle_connection<F>(stream: UnixStream, handler: &F) -> SysResult<()>
where
    F: Fn(&str, &Value) -> Result<Value, String>,
{
    let reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(e) => {
                let response = json!({ "id": 0, "ok": false, "error": format!("the request is not valid JSON: {}", e) });
                writeln!(writer, "{}", response)?;
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(json!(0));
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let response = match handler(&method, &params) {
            Ok(result) => json!({ "id": id, "ok": true, "result": result }),
            Err(error) => json!({ "id": id, "ok": false, "error": error }),
        };
        writeln!(writer, "{}", response)?;
        writer.flush()?;
    }
    Ok(())
}

/// Whether the service is running (the socket is connectable).
pub fn is_running(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).is_ok()
}

/// Convenience: turn a `SysError` into a string.
pub fn sys_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}
