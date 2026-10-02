//! WifiSync 核心纯逻辑。
//!
//! 这一层**不做任何系统调用**（不读写 `/etc/config`、不执行外部命令），
//! 因此可以在宿主机上完整单测。所有“要不要动网络、动哪些”的判断都集中在这里，
//! 这是本项目「默认零侵入」原则的落点：
//!
//! * [`role`]：角色集合与建桥策略（`enable_bridge = ap && !gateway && !controller`）
//! * [`bridge`]：网桥规划（列表结构，预留多网桥 / VLAN）
//! * [`wifi_source`]：Wi-Fi 信息三来源（控制器自身 / 网关 / 自定义）
//! * [`profile`]：`NetworkProfile`（网络档案，Controller 下发给 AP）
//! * [`admission`]：新 AP 准入状态机
//! * [`backup`]：初始基线 / 快照 / 恢复计划
//! * [`failsafe`]：死手定时器 + 心跳看门狗
//! * [`plan`]：把上面所有东西合成一份「写入计划」（非 AP 角色恒为空）

pub mod admission;
pub mod backup;
pub mod bridge;
pub mod capability;
pub mod config;
pub mod error;
pub mod failsafe;
pub mod plan;
pub mod profile;
pub mod role;
pub mod uci_file;
pub mod wifi_source;

pub use error::{CoreError, CoreResult};

/// 程序版本（由 Cargo 注入，用于写入备份清单与状态接口）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 缺省网桥名；代码中不假设只有一个网桥，此值仅作为默认值。
pub const DEFAULT_BRIDGE: &str = "br-lan";

/// 统一的时间取值入口：核心层不依赖系统时钟实现，调用方传入秒级时间戳。
pub type Timestamp = i64;
