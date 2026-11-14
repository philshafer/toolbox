//! Request statistics: the last `--stats-size` requests in memory for
//! `/stats`, and an optional JSON-lines log (`--log`).
//!
//! Privacy: `/stats` shows a client only its own requests.  The view
//! of every client is for loopback clients, that is, someone on the
//! server itself.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::pages::esc;

/// What a handler learned about upstream; filled in as it goes.
#[derive(Clone, Debug, Default)]
pub struct Stat {
    /// Registrable domain of the page or image.
    pub site: Option<String>,
    pub upstream: Option<u16>,
    pub origin_bytes: u64,
    pub origin_type: Option<String>,
    pub cache_hit: bool,
    /// Until the response headers arrived (DNS, connect, TLS, first
    /// byte), reading the body, and rendering.
    pub wait_ms: Option<u64>,
    pub download_ms: Option<u64>,
    pub render_ms: Option<u64>,
}

impl Stat {
    /// Record what an upstream fetch returned.
    pub fn note_fetch(&mut self, f: &crate::fetch::Fetched) {
        self.upstream = Some(f.status);
        self.origin_bytes += f.body.len() as u64;
        self.origin_type = Some(f.content_type.clone()).filter(|t| !t.is_empty());
        self.wait_ms = Some(f.wait.as_millis() as u64);
        self.download_ms = Some(f.download.as_millis() as u64);
    }
}

pub struct Row {
    /// Unix seconds.
    pub time: u64,
    pub client: Option<IpAddr>,
    pub method: String,
    pub target: String,
    /// read, article, headlines, img, list, stats, ...
    pub kind: &'static str,
    pub status: u16,
    pub sent: u64,
    pub total_ms: u64,
    pub stat: Stat,
}

impl Row {
    fn json(&self) -> String {
        let s = &self.stat;
        serde_json::json!({
            "time": self.time,
            "client": self.client.map(|c| c.to_string()),
            "method": self.method,
            "target": self.target,
            "kind": self.kind,
            "status": self.status,
            "sent": self.sent,
            "total_ms": self.total_ms,
            "site": s.site,
            "upstream": s.upstream,
            "origin_bytes": s.origin_bytes,
            "origin_type": s.origin_type,
            "cache_hit": s.cache_hit,
            "wait_ms": s.wait_ms,
            "download_ms": s.download_ms,
            "render_ms": s.render_ms,
        })
        .to_string()
    }

    fn is_page(&self) -> bool {
        matches!(self.kind, "read" | "article" | "headlines") && self.status == 200
    }
}

pub struct Stats {
    rows: Mutex<VecDeque<Arc<Row>>>,
    cap: usize,
    log: Option<Mutex<File>>,
}

impl Stats {
    /// `log` is opened for append, created readable only by its owner.
    pub fn new(cap: usize, log: Option<&Path>) -> Result<Stats, String> {
        let log = match log {
            Some(p) => {
                let f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .mode(0o600)
                    .open(p)
                    .map_err(|e| format!("--log {}: {e}", p.display()))?;
                Some(Mutex::new(f))
            }
            None => None,
        };
        Ok(Stats { rows: Mutex::new(VecDeque::with_capacity(cap.min(4096))), cap, log })
    }

    pub fn record(&self, row: Row) {
        if let Some(log) = &self.log {
            let mut f = log.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writeln!(f, "{}", row.json());
        }
        if self.cap == 0 {
            return;
        }
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        if rows.len() >= self.cap {
            rows.pop_front();
        }
        rows.push_back(Arc::new(row));
    }

    /// Newest first; only `client`'s rows unless it is None.
    pub fn rows(&self, client: Option<IpAddr>) -> Vec<Arc<Row>> {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        rows.iter().rev().filter(|r| client.is_none() || r.client == client).cloned().collect()
    }
}

/// The value at fraction `p` of sorted `v`.
fn pct(v: &[u64], p: f64) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    Some(v[i])
}

fn ms(v: Option<u64>) -> String {
    v.map(|m| format!("{m} ms")).unwrap_or_else(|| "-".into())
}

fn size(b: u64) -> String {
    if b >= 10 * 1024 * 1024 {
        format!("{} MB", b / (1024 * 1024))
    } else if b >= 1024 * 1024 {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    } else if b >= 1024 {
        format!("{} KB", b / 1024)
    } else {
        format!("{b} B")
    }
}

fn hit_rate(rows: &[&Arc<Row>]) -> String {
    if rows.is_empty() {
        return "-".into();
    }
    let hits = rows.iter().filter(|r| r.stat.cache_hit).count();
    format!("{}% of {}", hits * 100 / rows.len(), rows.len())
}

/// Summary figures over `rows`.
fn summary(rows: &[Arc<Row>]) -> String {
    let pages: Vec<&Arc<Row>> = rows.iter().filter(|r| r.is_page()).collect();
    let imgs: Vec<&Arc<Row>> = rows.iter().filter(|r| r.kind == "img").collect();
    let sorted = |it: &mut dyn Iterator<Item = u64>| {
        let mut v: Vec<u64> = it.collect();
        v.sort_unstable();
        v
    };
    let fetched = sorted(&mut pages.iter().filter(|r| !r.stat.cache_hit).map(|r| r.total_ms));
    let cached = sorted(&mut pages.iter().filter(|r| r.stat.cache_hit).map(|r| r.total_ms));
    let renders = sorted(&mut pages.iter().filter_map(|r| r.stat.render_ms));
    let origin: u64 = pages.iter().filter(|r| !r.stat.cache_hit).map(|r| r.stat.origin_bytes).sum();
    let sent: u64 = pages.iter().filter(|r| !r.stat.cache_hit).map(|r| r.sent).sum();
    let img_origin: u64 = imgs.iter().map(|r| r.stat.origin_bytes).sum();
    let img_sent: u64 = imgs.iter().map(|r| r.sent).sum();

    let mut s = String::from("<table class=\"summary\">\n");
    let mut row = |k: &str, v: String| s.push_str(&format!("<tr><th>{k}</th><td>{}</td></tr>\n", esc(&v)));
    row("Requests", rows.len().to_string());
    row("Pages from cache", hit_rate(&pages));
    row("Images from cache", hit_rate(&imgs));
    row(
        "Page time, fetched",
        format!("median {}, 95% {}", ms(pct(&fetched, 0.5)), ms(pct(&fetched, 0.95))),
    );
    row("Page time, cached", format!("median {}", ms(pct(&cached, 0.5))));
    row("Angel's own time per page", format!("median {}, 95% {}", ms(pct(&renders, 0.5)), ms(pct(&renders, 0.95))));
    row("Fetched pages", format!("{} from sites, {} sent", size(origin), size(sent)));
    row("Images", format!("{} from sites, {} sent", size(img_origin), size(img_sent)));
    s.push_str("</table>\n");
    s
}

/// Pages per site, busiest first.
fn per_site(rows: &[Arc<Row>]) -> String {
    let mut by: HashMap<&str, (usize, usize, u64, u64)> = HashMap::new();
    for r in rows.iter().filter(|r| r.is_page()) {
        let e = by.entry(r.stat.site.as_deref().unwrap_or("-")).or_default();
        e.0 += 1;
        e.1 += usize::from(r.stat.cache_hit);
        e.2 += r.stat.origin_bytes;
        e.3 += r.sent;
    }
    if by.is_empty() {
        return String::new();
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
    let mut s = String::from(
        "<h2>Sites</h2>\n<table class=\"stats\">\n<tr><th>Site</th><th>Pages</th><th>Cached</th>\
         <th>From site</th><th>Sent</th></tr>\n",
    );
    for (site, (n, hit, origin, sent)) in v.into_iter().take(20) {
        s.push_str(&format!(
            "<tr><td>{}</td><td class=\"num\">{n}</td><td class=\"num\">{hit}</td>\
             <td class=\"num\">{}</td><td class=\"num\">{}</td></tr>\n",
            esc(site),
            size(origin),
            size(sent)
        ));
    }
    s.push_str("</table>\n");
    s
}

fn clock(t: u64) -> String {
    let d = t % 86_400;
    format!("{:02}:{:02}:{:02}", d / 3600, d / 60 % 60, d % 60)
}

/// What to show for a request-target: the target URL for the reader
/// endpoints, the path otherwise.
fn shown(target: &str) -> &str {
    for p in ["/read/", "/article/", "/headlines/", "/img/", "/reload/", "/clear/"] {
        if let Some(rest) = target.strip_prefix(p) {
            return rest;
        }
    }
    target
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

/// The `/stats` body.  `all`: rows from every client (with a client
/// column); `admin`: the viewer may switch between the two views.
pub fn body(rows: &[Arc<Row>], all: bool, admin: bool) -> String {
    let mut s = String::from("<div class=\"stats-page\">\n<h1>Statistics</h1>\n");
    let who = if all { "every client" } else { "your requests" };
    let switch = match (admin, all) {
        (true, false) => " (<a href=\"/stats?all=1\">all clients</a>)",
        (true, true) => " (<a href=\"/stats\">only mine</a>)",
        _ => "",
    };
    s.push_str(&format!(
        "<p class=\"muted\">The last {} requests from {who}{switch}, newest first.  Times are UTC.</p>\n",
        rows.len()
    ));
    s.push_str(&summary(rows));
    s.push_str(&per_site(rows));
    s.push_str("<h2>Requests</h2>\n<table class=\"stats\">\n<tr><th>Time</th>");
    if all {
        s.push_str("<th>Client</th>");
    }
    s.push_str(
        "<th>Kind</th><th>Status</th><th>Site status</th><th>URL</th><th>Type</th><th>From site</th>\
         <th>Sent</th><th>Cache</th><th>Wait</th><th>Download</th><th>Render</th><th>Total</th></tr>\n",
    );
    for r in rows {
        let st = &r.stat;
        let url = shown(&r.target);
        let link = r.method == "GET" && matches!(r.kind, "read" | "article" | "headlines");
        let url_cell = if link {
            format!("<a href=\"{}\" title=\"{}\">{}</a>", esc(&r.target), esc(url), esc(&short(url, 70)))
        } else {
            format!("<span title=\"{}\">{}</span>", esc(url), esc(&short(url, 70)))
        };
        let num = |v: Option<u64>| v.map(|m| m.to_string()).unwrap_or_default();
        s.push_str(&format!("<tr><td>{}</td>", clock(r.time)));
        if all {
            s.push_str(&format!("<td>{}</td>", r.client.map(|c| c.to_string()).unwrap_or_default()));
        }
        s.push_str(&format!(
            "<td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"url\">{url_cell}</td>\
             <td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td>{}</td><td class=\"num\">{}</td>\
             <td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>\n",
            r.kind,
            r.status,
            num(st.upstream.map(u64::from)),
            esc(st.origin_type.as_deref().map(|t| t.split(';').next().unwrap_or("")).unwrap_or("")),
            if st.origin_bytes > 0 { size(st.origin_bytes) } else { String::new() },
            size(r.sent),
            if st.cache_hit { "hit" } else { "" },
            num(st.wait_ms),
            num(st.download_ms),
            num(st.render_ms),
            r.total_ms,
        ));
    }
    s.push_str("</table>\n<p class=\"muted\">Wait is DNS, connect, TLS and first byte; times in ms.</p>\n</div>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(client: &str, kind: &'static str, total: u64, hit: bool) -> Row {
        Row {
            time: 3661,
            client: Some(client.parse().unwrap()),
            method: "GET".into(),
            target: format!("/{kind}/https://www.bbc.com/news"),
            kind,
            status: 200,
            sent: 1000,
            total_ms: total,
            stat: Stat { site: Some("bbc.com".into()), cache_hit: hit, origin_bytes: 5000, ..Stat::default() },
        }
    }

    #[test]
    fn ring_and_privacy() {
        let s = Stats::new(3, None).unwrap();
        for (i, c) in ["10.0.0.1", "10.0.0.2", "10.0.0.1", "10.0.0.1"].iter().enumerate() {
            s.record(row(c, "read", i as u64, false));
        }
        assert_eq!(s.rows(None).len(), 3);
        let mine = s.rows(Some("10.0.0.1".parse().unwrap()));
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].total_ms, 3, "newest first");
        assert!(s.rows(Some("10.0.0.3".parse().unwrap())).is_empty());
    }

    #[test]
    fn figures() {
        assert_eq!(pct(&[1, 2, 3, 4, 5], 0.5), Some(3));
        assert_eq!(pct(&[1, 2, 3, 4, 100], 0.95), Some(100));
        assert_eq!(pct(&[], 0.5), None);
        assert_eq!(clock(3661), "01:01:01");
        assert_eq!(size(2048), "2 KB");
        let rows: Vec<Arc<Row>> =
            vec![row("::1", "read", 300, false), row("::1", "read", 1, true), row("::1", "img", 5, false)]
                .into_iter()
                .map(Arc::new)
                .collect();
        let b = body(&rows, false, true);
        assert!(b.contains("50% of 2"), "{b}");
        assert!(b.contains("all clients"));
        assert!(b.contains("<td>bbc.com</td><td class=\"num\">2</td><td class=\"num\">1</td>"));
    }

    #[test]
    fn json_line() {
        let j = row("10.0.0.1", "read", 7, true).json();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["client"], "10.0.0.1");
        assert_eq!(v["cache_hit"], true);
        assert_eq!(v["wait_ms"], serde_json::Value::Null);
    }
}
