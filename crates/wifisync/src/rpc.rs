//! 本地 UNIX socket JSON 行协议（LuCI 通过 rpcd shim 访问它）。
//!
//! * 传输：`/var/run/wifisync/wifisync.sock`，权限 0600；
//! * 协议：一行一个 JSON 请求 `{"id":1,"method":"status","params":{}}`，
//!   回一行 `{"id":1,"ok":true,"result":{...}}`。
//!
//! 刻意不用 HTTP：省掉 `hyper`/`httparse` 依赖，也少一层协议解析面。

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use wifisync_sys::error::SysResult;

/// 客户端调用。
pub fn call(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| format!("无法连接 wifisync 服务（{}）: {}", socket.display(), e))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .ok();
    let request = json!({ "id": 1, "method": method, "params": params });
    let mut line = request.to_string();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| format!("写入请求失败: {}", e))?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|e| format!("读取响应失败: {}", e))?;
    if response.trim().is_empty() {
        return Err("服务未返回任何内容".to_string());
    }
    let value: Value =
        serde_json::from_str(response.trim()).map_err(|e| format!("响应不是合法 JSON: {}", e))?;
    if value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("未知错误")
            .to_string())
    }
}

/// 服务端。
pub struct Server {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl Server {
    /// 绑定 socket（自动清理上次遗留的 socket 文件），并收紧权限。
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

    /// 启动接受循环（阻塞当前线程）。
    pub fn serve<F>(&self, handler: F) -> SysResult<()>
    where
        F: Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let handler = std::sync::Arc::new(handler);
        for stream in self.listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(e) => {
                    // 单个连接失败不应终止服务
                    crate::log_warn!("接受连接失败: {}", e);
                    continue;
                }
            };
            let handler = handler.clone();
            std::thread::spawn(move || {
                if let Err(e) = handle_connection(stream, handler.as_ref()) {
                    crate::log_warn!("处理连接出错: {}", e);
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
                let response =
                    json!({ "id": 0, "ok": false, "error": format!("请求不是合法 JSON: {}", e) });
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

/// 服务是否在运行（socket 可连接）。
pub fn is_running(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).is_ok()
}

/// 便捷：把 `SysError` 转成字符串。
pub fn sys_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}
