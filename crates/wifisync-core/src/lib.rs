//! WifiSync core pure logic.
//!
//! This layer makes **no system calls** (it does not read/write `/etc/config` and does not run
//! external commands), so it can be fully unit-tested on a host. Every decision about "whether to
//! touch the network, and which parts" lives here:
//!
//! * [`role`]: role set and bridging policy (`enable_bridge = ap && !gateway && !controller`)
//! * [`bridge`]: bridge planning (list structure, reserving room for multi-bridge / VLAN)
//! * [`wifi_source`]: the three Wi-Fi information sources (controller itself / gateway / custom)
//! * [`profile`]: `NetworkProfile` (network profile pushed by the Controller to the AP)
//! * [`admission`]: new AP admission state machine
//! * [`backup`]: initial baseline / snapshots / restore plan
//! * [`failsafe`]: dead-man timer + heartbeat watchdog
//! * [`plan`]: combine all of the above into a single "write plan" (always empty for non-AP roles)

pub mod admission;
pub mod backup;
pub mod bridge;
pub mod capability;
pub mod config;
pub mod error;
pub mod failsafe;
pub mod message;
pub mod plan;
pub mod profile;
pub mod role;
pub mod uci_file;
pub mod wifi_source;

pub use error::{CoreError, CoreResult};
pub use message::Message;

/// Program version (injected by Cargo, written into backup manifests and the status interface).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default bridge name; the code does not assume a single bridge, so this value is only a default.
pub const DEFAULT_BRIDGE: &str = "br-lan";

/// Unified time entry point: the core layer does not depend on a system clock, and the caller
/// passes a second-resolution timestamp.
pub type Timestamp = i64;
