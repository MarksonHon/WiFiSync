//! WifiSync system adaptation layer.
//!
//! The separation of concerns is strict:
//!
//! * **read-only probing**: `sysfs` / `iwinfo` / `board.json` → capabilities and topology,
//!   callable by any role;
//! * **writes**: `uci` + `netifd`, **driven only by the AP role's write plan**
//!   (see `wifisync_core::plan`);
//! * **LAN report** (`lan`): read-only collection of the bridge, network, DHCP and IPv6 policy
//!   a Gateway reports to the Controller;
//! * **runtime actuation**: `hostapd`, the hostapd `ubus` adapter used by client steering
//!   (see `docs/STEERING.md`). It is off by default and is never part of the write plan;
//! * **backup/restore**: `snapshot` / `restore`, establishing the baseline before start and
//!   restoring before stop.

pub mod error;
pub mod exec;
pub mod hostapd;
pub mod iwinfo;
pub mod lan;
pub mod netifd;
pub mod paths;
pub mod restore;
pub mod snapshot;
pub mod state;
pub mod sysfs;
pub mod uci;

pub use error::{SysError, SysResult};
pub use paths::Paths;
