//! Controller accounts: the credentials AP and Gateway nodes use to join the Controller.
//!
//! The password never travels and is never stored. The store keeps a PBKDF2-HMAC-SHA256 key plus
//! its salt; the link handshake (`link.rs`) proves knowledge of that key with HMAC. The key is
//! therefore password-equivalent for the link protocol, so the file is root-only (0600).
//! Accounts are managed on the Controller only (CLI / LuCI); there are no default accounts.

use crate::secrets::{hex, random_bytes, unhex};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::PathBuf;
use std::sync::Mutex;
use wifisync_core::link::{check_password, valid_username};
use wifisync_sys::state::write_atomic;

type HmacSha256 = Hmac<Sha256>;

/// PBKDF2 iterations for new accounts; a few ms on an x86 router, tolerable on small MIPS.
pub const DEFAULT_ITERATIONS: u32 = 4096;
const MAX_ACCOUNTS: usize = 64;

/// PBKDF2-HMAC-SHA256 with a single 32-byte output block.
pub fn derive_key(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let base =
        <HmacSha256 as Mac>::new_from_slice(password).expect("HMAC accepts keys of any length");
    let mut mac = base.clone();
    mac.update(salt);
    mac.update(&1u32.to_be_bytes());
    let mut block = mac.finalize().into_bytes();
    let mut out = block;
    for _ in 1..iterations.max(1) {
        let mut mac = base.clone();
        mac.update(&block);
        block = mac.finalize().into_bytes();
        for (o, b) in out.iter_mut().zip(block.iter()) {
            *o ^= b;
        }
    }
    out.into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    username: String,
    salt: String,
    iterations: u32,
    key: String,
    created_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    accounts: Vec<Record>,
}

/// What the handshake needs to authenticate a user.
#[derive(Debug, Clone)]
pub struct AccountKey {
    pub salt: Vec<u8>,
    pub iterations: u32,
    pub key: [u8; 32],
}

/// Listing entry (never contains key material).
#[derive(Debug, Clone, Serialize)]
pub struct AccountSummary {
    pub username: String,
    pub created_at: i64,
}

pub struct AccountStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AccountStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Result<File, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("the account file is corrupt: {}", e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
            Err(e) => Err(format!("reading the account file failed: {}", e)),
        }
    }

    fn save(&self, file: &File) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        write_atomic(&self.path, text.as_bytes()).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("restricting the account file failed: {}", e))
    }

    pub fn list(&self) -> Result<Vec<AccountSummary>, String> {
        let _guard = self.lock.lock().unwrap();
        Ok(self
            .load()?
            .accounts
            .into_iter()
            .map(|r| AccountSummary {
                username: r.username,
                created_at: r.created_at,
            })
            .collect())
    }

    fn make_record(username: &str, password: &str, now: i64) -> Result<Record, String> {
        check_password(password)?;
        let salt = random_bytes(16).map_err(|e| e.to_string())?;
        let key = derive_key(password.as_bytes(), &salt, DEFAULT_ITERATIONS);
        Ok(Record {
            username: username.to_string(),
            salt: hex(&salt),
            iterations: DEFAULT_ITERATIONS,
            key: hex(&key),
            created_at: now,
        })
    }

    pub fn add(&self, username: &str, password: &str, now: i64) -> Result<(), String> {
        if !valid_username(username) {
            return Err(
                "the account name must be 1-32 characters of letters, digits and - _ . :"
                    .to_string(),
            );
        }
        let _guard = self.lock.lock().unwrap();
        let mut file = self.load()?;
        if file.accounts.iter().any(|r| r.username == username) {
            return Err(format!("account `{}` already exists", username));
        }
        if file.accounts.len() >= MAX_ACCOUNTS {
            return Err(format!("at most {} accounts are supported", MAX_ACCOUNTS));
        }
        file.accounts
            .push(Self::make_record(username, password, now)?);
        self.save(&file)
    }

    pub fn set_password(&self, username: &str, password: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let mut file = self.load()?;
        let record = file
            .accounts
            .iter_mut()
            .find(|r| r.username == username)
            .ok_or_else(|| format!("account `{}` does not exist", username))?;
        let created_at = record.created_at;
        *record = Self::make_record(username, password, created_at)?;
        self.save(&file)
    }

    pub fn remove(&self, username: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let mut file = self.load()?;
        let before = file.accounts.len();
        file.accounts.retain(|r| r.username != username);
        if file.accounts.len() == before {
            return Err(format!("account `{}` does not exist", username));
        }
        self.save(&file)
    }

    /// Key material for the handshake; `None` for an unknown user or a damaged record.
    pub fn lookup(&self, username: &str) -> Option<AccountKey> {
        let _guard = self.lock.lock().unwrap();
        let file = self.load().ok()?;
        let record = file.accounts.into_iter().find(|r| r.username == username)?;
        let key: [u8; 32] = unhex(&record.key)?.try_into().ok()?;
        Some(AccountKey {
            salt: unhex(&record.salt)?,
            iterations: record.iterations,
            key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn store(tag: &str) -> (AccountStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wifisync-accounts-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        (AccountStore::new(dir.join("accounts.json")), dir)
    }

    #[test]
    fn pbkdf2_matches_the_known_vectors() {
        let one = derive_key(b"password", b"salt", 1);
        assert_eq!(
            hex(&one),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        let two = derive_key(b"password", b"salt", 2);
        assert_eq!(
            hex(&two),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
        let many = derive_key(b"password", b"salt", 4096);
        assert_eq!(
            hex(&many),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn account_lifecycle() {
        let (accounts, dir) = store("lifecycle");
        assert!(accounts.list().unwrap().is_empty());
        assert!(accounts.lookup("ap1").is_none());

        accounts.add("ap1", "correct horse", 10).unwrap();
        assert!(accounts.add("ap1", "another one", 11).is_err(), "duplicate");
        assert!(accounts.add("bad name", "correct horse", 11).is_err());
        assert!(accounts.add("ap2", "short", 11).is_err());
        assert_eq!(accounts.list().unwrap().len(), 1);

        let found = accounts.lookup("ap1").unwrap();
        assert_eq!(
            found.key,
            derive_key(b"correct horse", &found.salt, found.iterations)
        );

        accounts.set_password("ap1", "battery staple").unwrap();
        let changed = accounts.lookup("ap1").unwrap();
        assert_eq!(
            changed.key,
            derive_key(b"battery staple", &changed.salt, changed.iterations)
        );
        assert_eq!(accounts.list().unwrap()[0].created_at, 10);
        assert!(accounts.set_password("nobody", "battery staple").is_err());

        accounts.remove("ap1").unwrap();
        assert!(accounts.remove("ap1").is_err());
        assert!(accounts.lookup("ap1").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_account_file_is_private_and_holds_no_password() {
        let (accounts, dir) = store("mode");
        accounts.add("ap1", "super secret pw", 1).unwrap();
        let path = dir.join("accounts.json");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("super secret pw"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
