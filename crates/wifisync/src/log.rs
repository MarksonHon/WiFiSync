//! Minimal logging: writes to stderr (collected into syslog by procd/logd, readable via `logread`).
//!
//! No logging framework is pulled in, to keep the binary size under control.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static JSON_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_json_mode(enabled: bool) {
    JSON_MODE.store(enabled, Ordering::Relaxed);
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_epoch(secs as i64)
}

/// Format epoch seconds as `YYYY-MM-DDTHH:MM:SSZ` (without depending on chrono).
pub fn format_epoch(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs_of_day = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Compute the calendar date from the day count (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn emit(level: &str, message: &str) {
    let line = if JSON_MODE.load(Ordering::Relaxed) {
        let payload = serde_json::json!({
            "ts": timestamp(),
            "level": level,
            "msg": message,
        });
        payload.to_string()
    } else {
        format!(
            "{} wifisync[{}]: {}",
            timestamp(),
            std::process::id(),
            message
        )
    };
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "{}", line);
}

pub fn info(message: impl AsRef<str>) {
    emit("info", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    emit("warn", message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
    emit("error", message.as_ref());
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::log::info(format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::log::warn(format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::log::error(format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_is_1970() {
        assert_eq!(format_epoch(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp_formats() {
        // 2024-01-01T00:00:00Z
        assert_eq!(format_epoch(1_704_067_200), "2024-01-01T00:00:00Z");
        // 2026-09-28T12:34:56Z
        assert_eq!(format_epoch(1_790_598_896), "2026-09-28T12:34:56Z");
    }
}
