//! 密钥存取与节点间认证。
//!
//! 约束（需求 2）：不使用 TLS —— 避免 `openssl-sys` / `ring`（ring 不支持 MIPS）。
//! 因此：
//! * 本地 API 走 UNIX socket（内核权限隔离，socket 权限 0600）；
//! * 节点间心跳用 **HMAC-SHA256** 认证 + **ChaCha20-Poly1305** 加密载荷（全纯 Rust）。

#![allow(dead_code)] // seal/open 供后续「档案加密传输」使用，见 PLAN.md 里程碑 M3
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::Path;
use wifisync_sys::error::{SysError, SysResult};
use wifisync_sys::state::random_hex;

type HmacSha256 = Hmac<Sha256>;

/// 用 PSK 计算 HMAC-SHA256（十六进制字符串形式）。
pub fn hmac_hex(psk: &[u8], message: &[u8]) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(psk).expect("HMAC 接受任意长度密钥");
    mac.update(message);
    hex(&mac.finalize().into_bytes())
}

/// 恒定时间比较（避免计时侧信道）。
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 派生密钥：以 PSK 为根的 SHA-256（保证密钥长度正确）。
fn derive_key(psk: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"wifisync-v1");
    hasher.update(psk);
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

/// 加密：输出 `nonce(12) || ciphertext`。
pub fn seal(psk: &[u8], plaintext: &[u8], aad: &[u8]) -> SysResult<Vec<u8>> {
    let key = derive_key(psk);
    let cipher = ChaCha20Poly1305::new((&key).into());
    let nonce_bytes = random_bytes(12)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| SysError::Parse {
            what: "chacha20poly1305".into(),
            message: "加密失败".into(),
        })?;
    let mut out = nonce_bytes;
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 解密 `nonce(12) || ciphertext`。
pub fn open(psk: &[u8], sealed: &[u8], aad: &[u8]) -> SysResult<Vec<u8>> {
    if sealed.len() <= 12 {
        return Err(SysError::Parse {
            what: "chacha20poly1305".into(),
            message: "密文长度不足".into(),
        });
    }
    let key = derive_key(psk);
    let cipher = ChaCha20Poly1305::new((&key).into());
    let (nonce_bytes, ciphertext) = sealed.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| SysError::Parse {
            what: "chacha20poly1305".into(),
            message: "解密失败（密钥或完整性校验不通过）".into(),
        })
}

fn random_bytes(len: usize) -> SysResult<Vec<u8>> {
    let hex = random_hex(len)?;
    Ok(hex
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| std::str::from_utf8(pair).ok())
        .filter_map(|pair| u8::from_str_radix(pair, 16).ok())
        .collect())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 密钥仓库：`/etc/wifisync/secrets/<ref>`（0600）。
pub struct SecretStore {
    dir: std::path::PathBuf,
}

impl SecretStore {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn put(&self, reference: &str, value: &str) -> SysResult<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(sanitize(reference));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, value.as_bytes())?;
        set_mode_600(&tmp)?;
        std::fs::rename(&tmp, &path)?;
        set_mode_600(&path)?;
        Ok(())
    }

    pub fn get(&self, reference: &str) -> SysResult<Option<String>> {
        let path = self.dir.join(sanitize(reference));
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text.trim().to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SysError::Io(e)),
        }
    }

    pub fn contains(&self, reference: &str) -> bool {
        self.dir.join(sanitize(reference)).exists()
    }
}

fn sanitize(reference: &str) -> String {
    reference
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn set_mode_600(path: &Path) -> SysResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_is_deterministic_and_key_dependent() {
        let a = hmac_hex(b"key-1", b"message");
        assert_eq!(a, hmac_hex(b"key-1", b"message"));
        assert_ne!(a, hmac_hex(b"key-2", b"message"));
    }

    #[test]
    fn seal_open_roundtrip() {
        let sealed = seal(b"psk", b"hello openwrt", b"hb").unwrap();
        assert_ne!(&sealed[12..], b"hello openwrt");
        assert_eq!(open(b"psk", &sealed, b"hb").unwrap(), b"hello openwrt");
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let mut sealed = seal(b"psk", b"payload", b"").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(open(b"psk", &sealed, b"").is_err());
    }

    #[test]
    fn aad_is_authenticated() {
        let sealed = seal(b"psk", b"payload", b"aad-1").unwrap();
        assert!(open(b"psk", &sealed, b"aad-2").is_err());
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn secret_store_roundtrip_with_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join("wifisync-secrets-test");
        let _ = std::fs::remove_dir_all(&dir);
        let store = SecretStore::new(&dir);
        store.put("gateway/psk", "s3cret").unwrap();
        assert_eq!(store.get("gateway/psk").unwrap().as_deref(), Some("s3cret"));
        let meta = std::fs::metadata(dir.join("gateway_psk")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert!(store.contains("gateway/psk"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_strips_path_separators() {
        assert_eq!(sanitize("../../etc/shadow"), "______etc_shadow");
    }
}
