//! uci 配置文件解析（纯函数，用于「只恢复 wifisync 改过的键」）。

/// 一个 uci section。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UciSection {
    pub kind: String,
    pub name: String,
    pub anonymous: bool,
    pub options: Vec<(String, String)>,
    pub lists: Vec<(String, Vec<String>)>,
}

impl UciSection {
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn list(&self, name: &str) -> Vec<&str> {
        self.lists
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }
}

/// 解析 uci 文件文本。忽略注释与未知行，容忍缩进。
pub fn parse(text: &str) -> Vec<UciSection> {
    let mut sections: Vec<UciSection> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = split_uci_tokens(line);
        if parts.is_empty() {
            continue;
        }
        let keyword = parts.remove(0);
        match keyword.as_str() {
            "config" => {
                if parts.is_empty() {
                    continue;
                }
                let kind = parts.remove(0);
                let (name, anonymous) = match parts.first() {
                    Some(name) => (name.clone(), false),
                    None => (format!("@{}[-1]", kind), true),
                };
                sections.push(UciSection {
                    kind,
                    name,
                    anonymous,
                    options: Vec::new(),
                    lists: Vec::new(),
                });
            }
            "option" | "list" => {
                if parts.len() < 2 {
                    continue;
                }
                let key = parts.remove(0);
                let value = parts.join(" ");
                if let Some(section) = sections.last_mut() {
                    if keyword == "option" {
                        section.options.push((key, value));
                    } else {
                        match section.lists.iter_mut().find(|(k, _)| *k == key) {
                            Some((_, values)) => values.push(value),
                            None => section.lists.push((key, vec![value])),
                        }
                    }
                }
            }
            _ => {}
        }
    }
    sections
}

/// 按 shell 风格拆分一行（支持单/双引号）。
fn split_uci_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;

    for ch in line.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    has_token = true;
                } else if ch.is_whitespace() {
                    if has_token || !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                } else {
                    current.push(ch);
                    has_token = true;
                }
            }
        }
    }
    if has_token || !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// 读取某个 section.option 的值。
pub fn get(text: &str, section: &str, option: &str) -> Option<String> {
    parse(text)
        .into_iter()
        .find(|s| s.name == section)
        .and_then(|s| s.option(option).map(|v| v.to_string()))
}

/// 把 `network.lan.ipaddr` 拆成 `(file, section, option)`；`network.lan` ⇒ option 为 `None`。
pub fn split_key(key: &str) -> Option<(String, String, Option<String>)> {
    let mut parts = key.split('.');
    let file = parts.next()?.to_string();
    let section = parts.next()?.to_string();
    let option = parts.next().map(|s| s.to_string());
    if parts.next().is_some() {
        return None;
    }
    Some((file, section, option))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
config interface 'lan'
	option device 'br-lan'
	option proto 'static'
	option ipaddr '192.168.1.1'
	option netmask '255.255.255.0'

config interface 'wan'
	option device 'wan'
	option proto 'dhcp'

config device
	option name 'br-lan'
	option type 'bridge'
	list ports 'lan1'
	list ports 'lan2'
"#;

    #[test]
    fn parses_sections_along_with_keywords() {
        let sections = parse(SAMPLE);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].kind, "interface");
        assert_eq!(sections[0].name, "lan");
        assert!(!sections[0].anonymous);
        assert!(sections[2].anonymous);
    }

    #[test]
    fn reads_options_and_lists() {
        assert_eq!(get(SAMPLE, "lan", "ipaddr").as_deref(), Some("192.168.1.1"));
        let sections = parse(SAMPLE);
        let device = sections.iter().find(|s| s.kind == "device").unwrap();
        assert_eq!(device.list("ports"), vec!["lan1", "lan2"]);
        assert_eq!(device.option("name"), Some("br-lan"));
    }

    #[test]
    fn handles_quoted_values_with_spaces() {
        let text = "config wifi-iface 'default'\n\toption ssid 'My Home WiFi'\n";
        assert_eq!(
            get(text, "default", "ssid").as_deref(),
            Some("My Home WiFi")
        );
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let text = "# comment\n\nconfig x 'y'\n\t# inner\n\toption a 'b'\n";
        let sections = parse(text);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].option("a"), Some("b"));
    }

    #[test]
    fn split_key_forms() {
        assert_eq!(
            split_key("network.lan.ipaddr"),
            Some(("network".into(), "lan".into(), Some("ipaddr".into())))
        );
        assert_eq!(
            split_key("wireless.radio0.disabled"),
            Some(("wireless".into(), "radio0".into(), Some("disabled".into())))
        );
        assert_eq!(split_key("badkey"), None);
        assert_eq!(split_key("a.b.c.d"), None);
    }
}
