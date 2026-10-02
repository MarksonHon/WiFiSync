//! External command execution wrapper.
//!
//! We deliberately "call native OpenWrt tools" instead of reimplementing them in Rust:
//! * uci write semantics (`commit`, section creation) are guaranteed by `uci`;
//! * bridge bring-up is delegated to netifd (`ubus call network reload`), not netlink;
//! * status reads go through sysfs, without relying on `ip -j` (busybox's ip has no JSON).

use crate::error::{SysError, SysResult};
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

/// Look up an executable in PATH.
pub fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return if path.is_file() { Some(path) } else { None };
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Run a command, allowing a non-zero exit code (the result is returned).
pub fn run(program: &str, args: &[&str]) -> SysResult<CmdOutput> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => SysError::MissingProgram(program.to_string()),
            _ => SysError::Command {
                program: program.to_string(),
                message: e.to_string(),
            },
        })?;

    Ok(CmdOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Run a command and require success.
pub fn run_ok(program: &str, args: &[&str]) -> SysResult<CmdOutput> {
    let output = run(program, args)?;
    if !output.success() {
        return Err(SysError::Command {
            program: program.to_string(),
            message: format!(
                "exit code {} / stderr: {}",
                output.status,
                output.stderr.trim()
            ),
        });
    }
    Ok(output)
}

/// Whether a command exists (used for capability probing and degradation).
pub fn has(program: &str) -> bool {
    which(program).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_captures_stdout() {
        let out = run_ok("echo", &["hello"]).unwrap();
        assert_eq!(out.stdout_trimmed(), "hello");
    }

    #[test]
    fn missing_program_is_reported() {
        let err = run("definitely-not-a-real-binary-xyz", &[]).unwrap_err();
        assert!(matches!(err, SysError::MissingProgram(_)));
    }

    #[test]
    fn non_zero_exit_is_error_for_run_ok() {
        assert!(run_ok("false", &[]).is_err());
        let out = run("false", &[]).unwrap();
        assert!(!out.success());
    }

    #[test]
    fn which_finds_echo() {
        assert!(which("echo").is_some());
        assert!(which("/definitely/not/here").is_none());
    }
}
