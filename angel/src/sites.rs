//! Per-site settings: `data/angel-sites.toml`, for the few sites that
//! need something special.  Each table is keyed by a site-list style
//! entry (it covers that host and its subdomains; wildcards allowed)
//! and the most specific matching key wins:
//!
//! ```toml
//! ["washingtonpost.com"]
//! cookies = false                  # fresh visitor every time
//! user_agent = "..."               # override the User-Agent
//! cookie = { consent = "yes" }     # fixed cookies sent on every fetch
//! ```
//!
//! Like the list, the file is re-read when its mtime changes.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::sitelist;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Site {
    /// `cookies = false`: do not use the shared jar for this site.
    pub fresh: bool,
    pub user_agent: Option<String>,
    /// Fixed cookies, set before every fetch.
    pub cookies: Vec<(String, String)>,
}

/// The parsed file: (key, settings), plus problems to report.
pub fn parse(text: &str) -> (Vec<(String, Site)>, Vec<String>) {
    let mut out = Vec::new();
    let mut bad = Vec::new();
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            bad.push(format!("not valid TOML: {e}"));
            return (out, bad);
        }
    };
    for (key, v) in table {
        let entry = match sitelist::normalize(&key) {
            Ok(e) => e,
            Err(e) => {
                bad.push(format!("[{key}]: {e}"));
                continue;
            }
        };
        let Some(t) = v.as_table() else {
            bad.push(format!("{key}: expected a [\"{key}\"] table"));
            continue;
        };
        let mut site = Site::default();
        for (field, fv) in t {
            match (field.as_str(), fv) {
                ("cookies", toml::Value::Boolean(b)) => site.fresh = !b,
                ("user_agent", toml::Value::String(s)) if !s.trim().is_empty() && !s.contains(['\r', '\n']) => {
                    site.user_agent = Some(s.trim().to_string())
                }
                ("cookie", toml::Value::Table(c)) => {
                    for (name, val) in c {
                        match val.as_str() {
                            Some(val) if cookie_ok(name, val) => site.cookies.push((name.clone(), val.to_string())),
                            _ => bad.push(format!("[{key}] cookie {name}: not a usable cookie value")),
                        }
                    }
                }
                _ => bad.push(format!("[{key}] {field}: unknown setting or wrong type")),
            }
        }
        out.push((entry, site));
    }
    (out, bad)
}

/// Cookie names are tokens; values must not break the header.
fn cookie_ok(name: &str, val: &str) -> bool {
    let token = |c: char| c.is_ascii_graphic() && !"()<>@,;:\\\"/[]?={}".contains(c);
    !name.is_empty() && name.chars().all(token) && val.chars().all(|c| c.is_ascii_graphic() && !";,\\\"".contains(c))
}

/// The settings for `host` from parsed entries: the longest matching
/// key wins, as the most specific.
pub fn lookup(entries: &[(String, Site)], host: &str) -> Option<Site> {
    entries
        .iter()
        .filter(|(k, _)| sitelist::entry_matches(k, host))
        .max_by_key(|(k, _)| k.len())
        .map(|(_, s)| s.clone())
}

pub struct Sites {
    path: PathBuf,
    state: Mutex<State>,
}

struct State {
    entries: Arc<Vec<(String, Site)>>,
    stamp: Option<(SystemTime, u64)>,
}

impl Sites {
    /// A missing file just means no settings.
    pub fn open(path: PathBuf) -> Sites {
        let s = Sites { path, state: Mutex::new(State { entries: Arc::new(Vec::new()), stamp: None }) };
        s.entries();
        s
    }

    fn entries(&self) -> Arc<Vec<(String, Site)>> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = sitelist::stamp(&self.path);
        if now != st.stamp {
            st.stamp = now;
            match fs::read_to_string(&self.path) {
                Ok(text) => {
                    let (entries, bad) = parse(&text);
                    for b in bad {
                        eprintln!("angel: {}: {b} (ignored)", self.path.display());
                    }
                    st.entries = Arc::new(entries);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => st.entries = Arc::new(Vec::new()),
                Err(e) => eprintln!("angel: {}: {e}", self.path.display()),
            }
        }
        Arc::clone(&st.entries)
    }

    pub fn count(&self) -> usize {
        self.entries().len()
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Settings for `host`, or the defaults.
    pub fn for_host(&self, host: &str) -> Site {
        lookup(&self.entries(), host).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
        ["washingtonpost.com"]
        cookies = false

        ["wikipedia.org"]
        user_agent = "angel/0.1 (household reader)"

        ["en.wikipedia.org"]
        cookie = { consent = "yes", "bad cookie" = "x" }

        ["example.com"]
        colour = "blue"

        ["no thanks"]
        cookies = true
    "#;

    #[test]
    fn parsing() {
        let (e, bad) = parse(FILE);
        assert_eq!(e.len(), 4);
        assert_eq!(bad.len(), 3, "{bad:?}");
        assert!(lookup(&e, "www.washingtonpost.com").unwrap().fresh);
        let wiki = lookup(&e, "de.wikipedia.org").unwrap();
        assert_eq!(wiki.user_agent.as_deref(), Some("angel/0.1 (household reader)"));
        // The more specific key wins, and settings are not merged.
        let en = lookup(&e, "en.wikipedia.org").unwrap();
        assert_eq!(en.cookies, [("consent".to_string(), "yes".to_string())]);
        assert_eq!(en.user_agent, None);
        assert!(lookup(&e, "cnn.com").is_none());
    }

    #[test]
    fn bad_toml() {
        let (e, bad) = parse("[unclosed");
        assert!(e.is_empty());
        assert_eq!(bad.len(), 1);
    }
}
