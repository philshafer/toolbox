//! The site list: `data/angel.list`, one host, domain, or fnmatch(3)
//! pattern per line.
//!
//! An entry matches a host when it matches the whole host name or any
//! parent of it at a label boundary, so `cnn.com` covers
//! `www.cnn.com` but not `notcnn.com`, and `ad*.cnn.com` covers
//! `foo.ads.cnn.com`.  `*` also matches `.`, as in fnmatch without
//! flags.
//!
//! Comments and blank lines survive a rewrite.  Edits are atomic
//! (temp file + rename) and the file is re-read whenever its mtime
//! changes, so a hand edit or `git pull` takes effect without a
//! restart.  Angel never runs git.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// fnmatch(3) without flags, on ASCII bytes: `*`, `?`, and `[...]`
/// with `!`/`^` negation and ranges.  An unclosed `[` is literal.
pub fn fnmatch(pat: &[u8], s: &[u8]) -> bool {
    let (mut p, mut i) = (0, 0);
    // Where the last `*` was, and the string position it is retried at.
    let mut star: Option<(usize, usize)> = None;
    while i < s.len() {
        let step = match pat.get(p) {
            Some(b'*') => {
                star = Some((p, i));
                p += 1;
                continue;
            }
            Some(b'?') => Some(1),
            Some(b'[') => match bracket(&pat[p..], s[i]) {
                Some((true, n)) => Some(n),
                Some((false, _)) => None,
                None => (s[i] == b'[').then_some(1),
            },
            Some(&c) => (c == s[i]).then_some(1),
            None => None,
        };
        match (step, star) {
            (Some(n), _) => {
                p += n;
                i += 1;
            }
            (None, Some((sp, si))) => {
                p = sp + 1;
                i = si + 1;
                star = Some((sp, si + 1));
            }
            (None, None) => return false,
        }
    }
    pat[p..].iter().all(|&c| c == b'*')
}

/// Match `c` against the bracket expression at the start of `pat`:
/// (matched, length of the expression), or None if it is unclosed.
fn bracket(pat: &[u8], c: u8) -> Option<(bool, usize)> {
    let mut j = 1;
    let neg = matches!(pat.get(j), Some(b'!' | b'^'));
    if neg {
        j += 1;
    }
    let mut hit = false;
    let mut first = true;
    loop {
        let &lo = pat.get(j)?;
        if lo == b']' && !first {
            return Some((hit != neg, j + 1));
        }
        first = false;
        match (pat.get(j + 1), pat.get(j + 2)) {
            (Some(b'-'), Some(&hi)) if hi != b']' => {
                hit |= lo <= c && c <= hi;
                j += 3;
            }
            _ => {
                hit |= lo == c;
                j += 1;
            }
        }
    }
}

const META: &[char] = &['*', '?', '[', ']'];

/// Does `entry` (normalized) cover `host`?  Tries the host and each
/// parent: `a.b.c`, `b.c`, `c`.
pub fn entry_matches(entry: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let mut h = host.as_str();
    loop {
        if fnmatch(entry.as_bytes(), h.as_bytes()) {
            return true;
        }
        match h.split_once('.') {
            Some((_, rest)) => h = rest,
            None => return false,
        }
    }
}

pub fn is_pattern(entry: &str) -> bool {
    entry.contains(META)
}

/// Check and normalize an entry: lower case, no leading or trailing
/// dots, IDN labels as punycode, and only host-name characters plus
/// fnmatch metacharacters.
pub fn normalize(s: &str) -> Result<String, String> {
    let s = s.trim().trim_matches('.').to_lowercase();
    if s.is_empty() {
        return Err("The entry is empty.".into());
    }
    let mut labels = Vec::new();
    for l in s.split('.') {
        if l.is_empty() {
            return Err(format!("\"{s}\" has an empty label (two dots in a row)."));
        }
        if l.is_ascii() {
            labels.push(l.to_string());
            continue;
        }
        if l.contains(META) {
            return Err(format!("\"{s}\": a non-ASCII label cannot use wildcards."));
        }
        match url::Host::parse(l) {
            Ok(url::Host::Domain(d)) => labels.push(d),
            _ => return Err(format!("\"{s}\" is not a valid host name.")),
        }
    }
    let e = labels.join(".");
    if e.len() > 253 {
        return Err("The entry is too long.".into());
    }
    if let Some(c) = e.chars().find(|c| !(c.is_ascii_alphanumeric() || "-_.*?[]!^".contains(*c))) {
        return Err(format!("\"{e}\": '{c}' is not allowed in a host name or pattern."));
    }
    if !e.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("\"{e}\" would match every site."));
    }
    Ok(e)
}

/// The registrable domain for a host (`www.bbc.co.uk` -> `bbc.co.uk`),
/// from the Public Suffix List.  IP addresses and names the list does
/// not know come back unchanged.
pub fn registrable(host: &str) -> String {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if h.parse::<std::net::IpAddr>().is_ok() || h.starts_with('[') {
        return h;
    }
    match psl::domain_str(&h) {
        Some(d) => d.to_string(),
        None => h,
    }
}

struct Line {
    raw: String,
    /// The normalized entry, for a line that has one.
    entry: Option<String>,
}

/// The parsed file, every line kept so a rewrite changes only what
/// was edited.
pub struct List {
    lines: Vec<Line>,
}

impl List {
    /// Parse the file text.  Lines that are not valid entries are kept
    /// as they are and reported in the second value.
    pub fn parse(text: &str) -> (List, Vec<String>) {
        let mut bad = Vec::new();
        let lines = text
            .lines()
            .enumerate()
            .map(|(n, raw)| {
                let body = raw.split('#').next().unwrap_or("").trim();
                let entry = if body.is_empty() {
                    None
                } else {
                    match normalize(body) {
                        Ok(e) => Some(e),
                        Err(e) => {
                            bad.push(format!("line {}: {e}", n + 1));
                            None
                        }
                    }
                };
                Line { raw: raw.to_string(), entry }
            })
            .collect();
        (List { lines }, bad)
    }

    pub fn text(&self) -> String {
        let mut s: String = self.lines.iter().map(|l| format!("{}\n", l.raw)).collect();
        if s.is_empty() {
            s.push('\n');
        }
        s
    }

    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().filter_map(|l| l.entry.as_deref())
    }

    pub fn contains(&self, entry: &str) -> bool {
        self.entries().any(|e| e == entry)
    }

    /// The entries that cover `host`.
    pub fn matching(&self, host: &str) -> Vec<String> {
        self.entries().filter(|e| entry_matches(e, host)).map(str::to_string).collect()
    }

    /// Append `entry` (normalized) after a `# added <date>` comment.
    /// False if it is already there.
    pub fn add(&mut self, entry: &str, date: &str) -> bool {
        if self.contains(entry) {
            return false;
        }
        self.lines.push(Line { raw: format!("# added {date}"), entry: None });
        self.lines.push(Line { raw: entry.to_string(), entry: Some(entry.to_string()) });
        true
    }

    /// Remove every line holding `entry`, along with a `# added`
    /// comment right above it.  False if it was not there.
    pub fn remove(&mut self, entry: &str) -> bool {
        let mut found = false;
        let mut i = self.lines.len();
        while i > 0 {
            i -= 1;
            if self.lines[i].entry.as_deref() != Some(entry) {
                continue;
            }
            found = true;
            self.lines.remove(i);
            if i > 0 && self.lines[i - 1].raw.starts_with("# added ") {
                self.lines.remove(i - 1);
                i -= 1;
            }
        }
        found
    }
}

/// The list file, shared by all workers.
pub struct SiteList {
    path: PathBuf,
    state: Mutex<State>,
}

struct State {
    list: List,
    /// (mtime, length) of the file as last read or written.
    stamp: Option<(SystemTime, u64)>,
}

pub(crate) fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl SiteList {
    /// Load the list.  A missing file is an empty list; the first add
    /// creates it.
    pub fn open(path: PathBuf) -> SiteList {
        let sl = SiteList { path, state: Mutex::new(State { list: List { lines: Vec::new() }, stamp: None }) };
        if !sl.path.exists() {
            eprintln!("angel: {}: not found; starting with an empty list", sl.path.display());
        }
        let mut st = sl.lock();
        sl.refresh(&mut st);
        drop(st);
        sl
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Re-read the file if it changed on disk.
    fn refresh(&self, st: &mut State) {
        let now = stamp(&self.path);
        if now == st.stamp {
            return;
        }
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                eprintln!("angel: {}: {e}", self.path.display());
                return;
            }
        };
        let (list, bad) = List::parse(&text);
        for b in bad {
            eprintln!("angel: {}: {b} (ignored)", self.path.display());
        }
        st.list = list;
        st.stamp = now;
    }

    /// Look at the current list.
    pub fn with<R>(&self, f: impl FnOnce(&List) -> R) -> R {
        let mut st = self.lock();
        self.refresh(&mut st);
        f(&st.list)
    }

    /// Change the list and save it if `f` says it changed.  The file
    /// is re-read first, so a hand edit made since is not lost.
    pub fn edit(&self, f: impl FnOnce(&mut List) -> bool) -> Result<bool, String> {
        let mut st = self.lock();
        self.refresh(&mut st);
        if !f(&mut st.list) {
            return Ok(false);
        }
        let text = st.list.text();
        self.save(&text).map_err(|e| format!("could not save {}: {e}", self.path.display()))?;
        st.stamp = stamp(&self.path);
        Ok(true)
    }

    fn save(&self, text: &str) -> io::Result<()> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(format!(".tmp{}", std::process::id()));
        let tmp = PathBuf::from(tmp);
        let r = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r
    }
}

/// Today's UTC date as YYYY-MM-DD.
pub fn today() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    civil(secs as i64 / 86400)
}

/// Days since 1970-01-01 to a date (Hinnant's civil_from_days).
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(p: &str, s: &str) -> bool {
        fnmatch(p.as_bytes(), s.as_bytes())
    }

    #[test]
    fn fnmatch_basics() {
        assert!(fm("abc", "abc"));
        assert!(!fm("abc", "abd"));
        assert!(fm("a*c", "abbbc"));
        assert!(fm("a*c", "ac"));
        assert!(fm("*", ""));
        assert!(fm("a?c", "abc"));
        assert!(!fm("a?c", "ac"));
        assert!(fm("ad*.cnn.com", "ad.x.cnn.com"));
        assert!(fm("[abc]x", "bx"));
        assert!(!fm("[!abc]x", "bx"));
        assert!(fm("[^abc]x", "dx"));
        assert!(fm("[a-c]x", "cx"));
        assert!(!fm("[a-c]x", "dx"));
        assert!(fm("[]]", "]"));
        assert!(fm("[a-]", "-"));
        assert!(fm("a[b", "a[b"));
        assert!(fm("*.*.com", "a.b.com"));
        assert!(!fm("*x", "abc"));
    }

    #[test]
    fn matching() {
        assert!(entry_matches("cnn.com", "cnn.com"));
        assert!(entry_matches("cnn.com", "www.cnn.com"));
        assert!(entry_matches("cnn.com", "edition.cnn.com."));
        assert!(entry_matches("cnn.com", "WWW.CNN.COM"));
        assert!(!entry_matches("cnn.com", "notcnn.com"));
        assert!(!entry_matches("cnn.com", "cnn.com.evil.org"));
        assert!(entry_matches("ad*.cnn.com", "ads.cnn.com"));
        assert!(entry_matches("ad*.cnn.com", "ad.x.cnn.com"));
        assert!(entry_matches("ad*.cnn.com", "foo.ads.cnn.com"));
        assert!(!entry_matches("ad*.cnn.com", "www.cnn.com"));
        assert!(entry_matches("*.cnn.com", "www.cnn.com"));
        assert!(!entry_matches("*.cnn.com", "cnn.com"));
    }

    #[test]
    fn normalizing() {
        assert_eq!(normalize(" WWW.CNN.com. ").unwrap(), "www.cnn.com");
        assert_eq!(normalize(".cnn.com").unwrap(), "cnn.com");
        assert_eq!(normalize("ad*.cnn.com").unwrap(), "ad*.cnn.com");
        assert_eq!(normalize("B\u{fc}cher.de").unwrap(), "xn--bcher-kva.de");
        assert!(normalize("").is_err());
        assert!(normalize("a..b").is_err());
        assert!(normalize("cnn.com/news").is_err());
        assert!(normalize("cnn com").is_err());
        assert!(normalize("<b>").is_err());
        assert!(normalize("*").is_err());
        assert!(normalize("*.*").is_err());
    }

    #[test]
    fn registrable_domains() {
        assert_eq!(registrable("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registrable("edition.cnn.com"), "cnn.com");
        assert_eq!(registrable("en.m.wikipedia.org"), "wikipedia.org");
        assert_eq!(registrable("foo.github.io"), "foo.github.io");
        assert_eq!(registrable("10.1.2.3"), "10.1.2.3");
    }

    const FILE: &str = "# Angel site list\n\
                        cnn.com   # news\n\
                        \n\
                        ad*.cnn.com\n\
                        not a host\n\
                        bbc.com\n";

    #[test]
    fn round_trip() {
        let (l, bad) = List::parse(FILE);
        assert_eq!(l.text(), FILE);
        assert_eq!(bad.len(), 1);
        assert_eq!(l.entries().collect::<Vec<_>>(), ["cnn.com", "ad*.cnn.com", "bbc.com"]);
        assert_eq!(l.matching("ads.cnn.com"), ["cnn.com", "ad*.cnn.com"]);
        assert!(l.matching("example.org").is_empty());
    }

    #[test]
    fn add_remove() {
        let (mut l, _) = List::parse(FILE);
        assert!(!l.add("cnn.com", "2026-11-13"));
        assert!(l.add("latimes.com", "2026-11-13"));
        assert!(l.text().ends_with("bbc.com\n# added 2026-11-13\nlatimes.com\n"));
        assert!(l.remove("latimes.com"));
        assert_eq!(l.text(), FILE);
        assert!(l.remove("cnn.com"));
        assert!(!l.remove("cnn.com"));
        assert!(l.text().starts_with("# Angel site list\n\nad*.cnn.com\n"));
    }

    #[test]
    fn file_edits() {
        let dir = std::env::temp_dir().join(format!("angel-list-{}", std::process::id()));
        let path = dir.join("angel.list");
        let _ = fs::remove_dir_all(&dir);
        let sl = SiteList::open(path.clone());
        assert_eq!(sl.with(|l| l.entries().count()), 0);
        assert!(sl.edit(|l| l.add("cnn.com", "2026-11-13")).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# added 2026-11-13\ncnn.com\n");
        // A hand edit is picked up, and survives the next change.
        fs::write(&path, "# mine\ncnn.com\nbbc.com\n").unwrap();
        assert_eq!(sl.with(|l| l.matching("www.bbc.com")), ["bbc.com"]);
        assert!(sl.edit(|l| l.remove("cnn.com")).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# mine\nbbc.com\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dates() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(19_723), "2024-01-01");
        assert_eq!(civil(19_723 + 59), "2024-02-29");
        assert_eq!(civil(-1), "1969-12-31");
    }
}
