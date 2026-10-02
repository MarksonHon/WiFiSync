//! WifiSync 系统适配层。
//!
//! 分工非常明确：
//!
//! * **只读探测**：`sysfs` / `iwinfo` / `board.json` → 能力与拓扑，任何角色都允许调用；
//! * **写入**：`uci` + `netifd`，**只由 AP 角色的写入计划驱动**（见 `wifisync_core::plan`）；
//! * **备份/恢复**：`snapshot` / `restore`，服务启动前建基线、停止前还原。

pub mod error;
pub mod exec;
pub mod iwinfo;
pub mod netifd;
pub mod paths;
pub mod restore;
pub mod snapshot;
pub mod state;
pub mod sysfs;
pub mod uci;

pub use error::{SysError, SysResult};
pub use paths::Paths;
