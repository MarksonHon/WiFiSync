**English** | [简体中文](README_zh-cn.md)

# WifiSync

A home LAN (wired + wireless) networking tool for **OpenWrt**. Written in Rust as
a single binary, with a LuCI web interface.

* **Zero intrusion** — gateway and controller roles never write network
  configuration; only a pure AP applies changes.
* **Reversible** — an initialization baseline is captured before start, and the
  previous network is restored on stop.
* **Least responsibility** — controller validates and pushes, gateway identifies
  and probes endpoints, AP applies and reports.
* KVR (802.11k/v/r) is provisioned centrally; there is **no mesh** — Wi-Fi relay
  uses `wpa_supplicant(STA)` upstream plus `hostapd(AP)` downstream.
* Optional failover: apply-guard rollback, AP link watchdog, procd respawn.

## Supported platforms

OpenWrt 25.12 on x86 and ARM64 (7 package architectures). See
[`docs/BUILDING.md`](docs/BUILDING.md).

## Install

```sh
# In an OpenWrt build tree, add this repository as a feed
echo "src-link wifisync /path/to/WifiSync" >> feeds.conf.default
./scripts/feeds update wifisync && ./scripts/feeds install -a -p wifisync
make menuconfig      # Network → wifisync / LuCI → Applications → luci-app-wifisync
make package/wifisync/compile V=s
```

## Usage

```sh
wifisync status                     # capabilities and default roles
wifisync plan                       # what would change (always empty for non-AP roles)
wifisync apply && wifisync confirm  # only a pure AP really writes
wifisync backup list
wifisync backup verify
wifisync restore                    # managed_only (default)
wifisync restore --full             # overwrite from the baseline
wifisync account add <name>         # Controller account for remote APs / Gateways
wifisync link                       # link to the Controller (port 6550 by default)
wifisync lan report                 # LAN bridge / network / DHCP / IPv6 the Gateway reports
```

LuCI: **Services → WifiSync**, with 8 pages: overview / roles / gateway /
controller / bridge / Wi-Fi & KVR / backup & failover / diagnostics.

## Documentation

| Document | Contents |
|----------|----------|
| [`docs/BACKEND.md`](docs/BACKEND.md) | Backend design: requirements, roles, dependencies, lifecycle, milestones, tests ([简体中文](docs/BACKEND_zh-cn.md)) |
| [`docs/FRONTEND.md`](docs/FRONTEND.md) | LuCI design: ubus contract, pages, safety patterns, i18n ([简体中文](docs/FRONTEND_zh-cn.md)) |
| [`docs/BUILDING.md`](docs/BUILDING.md) | Build paths, architecture mapping, CI ([简体中文](docs/BUILDING_zh-cn.md)) |
| [`docs/STEERING.md`](docs/STEERING.md) | Client steering design (**proposed, not implemented**): controller-centric architecture, protocol, safety rails, milestones ([简体中文](docs/STEERING_zh-cn.md)) |

## License

GPL-2.0-only, see [`LICENSE`](LICENSE).
