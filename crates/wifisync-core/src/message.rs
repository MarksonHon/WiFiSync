//! UI-facing messages: a stable key plus interpolation parameters.
//!
//! The backend never ships pre-rendered sentences to the web UI. It emits a
//! [`Message`] and the front end resolves it through the LuCI language
//! packages (`luci/luci-app-wifisync/po/*`), so no natural language text is
//! baked into the Rust code and the interface follows the user's language
//! preference.
//!
//! Parameters are referenced in the front-end templates as `%{name}`; boolean
//! flags are encoded as `"1"` / `"0"`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A single untranslated UI message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Stable identifier the front end looks up in its translation table.
    pub key: String,
    /// Interpolation parameters; an empty map is omitted from the JSON.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl Message {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            params: BTreeMap::new(),
        }
    }

    /// Adds a parameter, keeping the builder style.
    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    /// Adds a boolean parameter as `"1"` / `"0"`.
    pub fn flag(self, name: impl Into<String>, value: bool) -> Self {
        self.param(name, if value { "1" } else { "0" })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// Renders `key(param=value, ...)`.
    ///
    /// This is only used for logs, the CLI and error strings; the web UI always
    /// receives the structured form and translates it.
    pub fn render(&self) -> String {
        if self.params.is_empty() {
            return self.key.clone();
        }
        let params = self
            .params
            .iter()
            .map(|(name, value)| format!("{}={}", name, value))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({})", self.key, params)
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl From<&str> for Message {
    fn from(key: &str) -> Self {
        Message::new(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_key_without_empty_params() {
        let json = serde_json::to_value(Message::from("example.plain")).unwrap();
        assert_eq!(json, serde_json::json!({ "key": "example.plain" }));
    }

    #[test]
    fn serializes_params_as_object() {
        let msg = Message::from("example.with_params")
            .param("k", "1")
            .flag("r", false);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["key"], "example.with_params");
        assert_eq!(json["params"]["k"], "1");
        assert_eq!(json["params"]["r"], "0");
        assert_eq!(msg.render(), "example.with_params(k=1, r=0)");
    }
}
