//! uci 读写。**唯一**允许写 `/etc/config` 的入口，且只接收 `wifisync-core` 产出的写入计划。

use crate::error::{SysError, SysResult};
use crate::exec;
use crate::paths::Paths;
use wifisync_core::config::{WifisyncConfig, UCI_FILE};
use wifisync_core::plan::{UciOp, UciOpKind};
use wifisync_core::uci_file;

pub struct Uci {
    paths: Paths,
}

impl Uci {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// 系统上有 uci 可用吗（宿主机的单测环境里往往没有）。
    pub fn available(&self) -> bool {
        exec::has("uci")
    }

    /// `uci -q get <key>`；键不存在或系统上没有 uci 时返回 `None`。
    pub fn get(&self, key: &str) -> SysResult<Option<String>> {
        let out = match exec::run("uci", &["-q", "get", key]) {
            Ok(out) => out,
            // 宿主机/精简系统上没有 uci 时退化为“读不到”，由调用方回退到直接读文件
            Err(SysError::MissingProgram(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        if !out.success() {
            return Ok(None);
        }
        Ok(Some(out.stdout_trimmed().to_string()))
    }

    /// 直接把某个配置文件读成文本（备份与解析用，不调用 uci）。
    pub fn read_config_file(&self, pkg: &str) -> SysResult<Option<String>> {
        let path = self.paths.uci_file(pkg);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(std::fs::read_to_string(path)?))
    }

    /// `uci export <pkg>`（规范化输出）。
    pub fn export(&self, pkg: &str) -> SysResult<String> {
        let out = exec::run("uci", &["-q", "export", pkg])?;
        Ok(out.stdout)
    }

    /// 读取 WifiSync 自身配置。优先读文件（无 uci 也能工作，便于离线诊断）。
    pub fn load_config(&self) -> SysResult<WifisyncConfig> {
        match self.read_config_file(UCI_FILE)? {
            Some(text) => Ok(WifisyncConfig::from_sections(&uci_file::parse(&text))),
            None => Ok(WifisyncConfig::default()),
        }
    }

    /// 执行写入计划并 commit。返回实际执行的 uci 命令（用于审计与 dry-run 对比）。
    pub fn apply_ops(&self, ops: &[UciOp]) -> SysResult<Vec<String>> {
        let mut executed = Vec::new();
        let mut touched: Vec<String> = Vec::new();

        for op in ops {
            let (subcommand, expr) = match op.kind {
                // `uci set file.section=type` 用于创建 section（option 为 None）
                UciOpKind::Set => (
                    "set",
                    format!("{}={}", op.key(), op.value.clone().unwrap_or_default()),
                ),
                UciOpKind::AddList => (
                    "add_list",
                    format!("{}={}", op.key(), op.value.clone().unwrap_or_default()),
                ),
                UciOpKind::Delete => ("delete", op.key()),
            };

            exec::run_ok("uci", &[subcommand, &expr])?;
            executed.push(format!("uci {} {}", subcommand, expr));
            if !touched.contains(&op.file) {
                touched.push(op.file.clone());
            }
        }

        if !executed.is_empty() {
            for file in &touched {
                exec::run_ok("uci", &["commit", file])?;
                executed.push(format!("uci commit {}", file));
            }
        }
        Ok(executed)
    }

    /// 删除由 wifisync 创建的 section（恢复用）。
    pub fn delete_section(&self, file: &str, section: &str) -> SysResult<()> {
        let out = exec::run("uci", &["-q", "delete", &format!("{}.{}", file, section)])?;
        if !out.success() {
            return Err(SysError::Command {
                program: "uci".to_string(),
                message: format!("删除 {}.{} 失败: {}", file, section, out.stderr.trim()),
            });
        }
        Ok(())
    }
}
