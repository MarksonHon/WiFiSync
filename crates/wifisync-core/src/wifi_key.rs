//! Wi-Fi key (PSK) rules, shared by the Controller (which ships the key) and the AP (which
//! applies it).
//!
//! The key is deliberately **not** part of the network profile: the profile travels through LuCI
//! (`profile.publish`), so key material in it would end up in a browser. It is shipped as a
//! separate secret section over the encrypted Controller link and stored in
//! `/etc/wifisync/secrets/` (root only) on the node instead.

use serde::{Deserialize, Serialize};

/// Shortest accepted passphrase (WPA-PSK / SAE).
pub const MIN_PASSPHRASE_LEN: usize = 8;
/// Longest accepted passphrase (WPA-PSK / SAE).
pub const MAX_PASSPHRASE_LEN: usize = 63;
/// A raw PSK may be given as 64 hex digits instead of a passphrase.
pub const RAW_PSK_LEN: usize = 64;

/// Why a key cannot be used. The front end renders it through [`KeyProblem::message_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyProblem {
    /// The auth mode needs a key but none was provided.
    Missing,
    /// Shorter than [`MIN_PASSPHRASE_LEN`].
    TooShort,
    /// Longer than [`MAX_PASSPHRASE_LEN`] without being a 64-digit raw PSK.
    TooLong,
    /// 64 characters long but not hexadecimal.
    NotHex,
    /// Contains control characters.
    Unprintable,
}

impl KeyProblem {
    /// Message key for the UI. The backend never sends finished sentences.
    pub fn message_key(self) -> &'static str {
        match self {
            KeyProblem::Missing => "wifi_key.missing",
            KeyProblem::TooShort => "wifi_key.too_short",
            KeyProblem::TooLong => "wifi_key.too_long",
            KeyProblem::NotHex => "wifi_key.not_hex",
            KeyProblem::Unprintable => "wifi_key.unprintable",
        }
    }
}

/// Whether this auth mode needs a key at all. `none` (and an empty mode) means an open network.
pub fn requires_key(auth: &str) -> bool {
    !auth.is_empty() && !auth.eq_ignore_ascii_case("none")
}

/// Validate a key against its auth mode.
///
/// An open network must not carry a key; every other mode must carry a usable one — a node
/// refusing to apply a half-configured (encrypted but keyless) wireless config is the point of
/// this check.
pub fn check(auth: &str, key: Option<&str>) -> Result<(), KeyProblem> {
    if !requires_key(auth) {
        return Ok(());
    }

    let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) else {
        return Err(KeyProblem::Missing);
    };
    if key.chars().any(char::is_control) {
        return Err(KeyProblem::Unprintable);
    }
    if key.len() == RAW_PSK_LEN {
        return if key.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(())
        } else {
            Err(KeyProblem::NotHex)
        };
    }
    if key.len() < MIN_PASSPHRASE_LEN {
        return Err(KeyProblem::TooShort);
    }
    if key.len() > MAX_PASSPHRASE_LEN {
        return Err(KeyProblem::TooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_network_needs_no_key() {
        assert!(!requires_key("none"));
        assert!(!requires_key(""));
        assert!(check("none", None).is_ok());
        assert!(check("none", Some("")).is_ok());
    }

    #[test]
    fn an_encrypted_network_without_a_key_is_rejected() {
        assert!(requires_key("sae-mixed"));
        assert_eq!(check("sae-mixed", None), Err(KeyProblem::Missing));
        assert_eq!(check("sae-mixed", Some("   ")), Err(KeyProblem::Missing));
    }

    #[test]
    fn passphrase_length_is_enforced() {
        assert_eq!(check("psk2", Some("1234567")), Err(KeyProblem::TooShort));
        assert!(check("psk2", Some("12345678")).is_ok());
        // `z` is not a hex digit, so the raw-PSK branch cannot swallow these.
        assert!(check("psk2", Some(&"z".repeat(MAX_PASSPHRASE_LEN))).is_ok());
        // 64 characters is the raw-PSK form, so the first length that is simply too long is 65.
        assert_eq!(
            check("psk2", Some(&"z".repeat(RAW_PSK_LEN + 1))),
            Err(KeyProblem::TooLong)
        );
    }

    #[test]
    fn a_raw_psk_must_be_hexadecimal() {
        assert!(check("psk2", Some(&"0a".repeat(32))).is_ok());
        assert_eq!(
            check("psk2", Some(&"zz".repeat(32))),
            Err(KeyProblem::NotHex)
        );
    }

    #[test]
    fn control_characters_are_rejected() {
        assert_eq!(
            check("psk2", Some("good\tpass")),
            Err(KeyProblem::Unprintable)
        );
    }

    #[test]
    fn every_problem_has_a_message_key() {
        for problem in [
            KeyProblem::Missing,
            KeyProblem::TooShort,
            KeyProblem::TooLong,
            KeyProblem::NotHex,
            KeyProblem::Unprintable,
        ] {
            assert!(problem.message_key().starts_with("wifi_key."));
        }
    }
}
