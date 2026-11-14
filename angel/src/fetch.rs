//! Upstream fetching: one shared `ureq` agent with limits and an SSRF
//! guard, plus the shared cookie jar and the redirect loop that uses
//! it.
//!
//! The guard is a resolver wrapper.  Every connection ureq makes,
//! including each redirect hop, resolves through it, and it drops
//! loopback, private, link-local, and other non-public addresses.
//! A host that resolves only to such addresses fails to connect.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cookie_store::{CookieStore, RawCookie};
use ureq::Agent;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::Url;

use crate::sitelist;
use crate::sites::Site;

/// Largest HTML page we will read (CNN pages run 5-8 MB).
pub const HTML_LIMIT: u64 = 25 * 1024 * 1024;
/// Largest image we will pass through.
pub const IMAGE_LIMIT: u64 = 10 * 1024 * 1024;

pub const BLOCKED: &str = "address not allowed";

#[derive(Debug)]
struct GuardResolver {
    inner: DefaultResolver,
}

impl Resolver for GuardResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let mut addrs = self.inner.resolve(uri, config, timeout)?;
        let mut keep = 0;
        for i in 0..addrs.len() {
            if is_public(addrs[i].ip()) {
                addrs[keep] = addrs[i];
                keep += 1;
            }
        }
        if keep == 0 {
            return Err(ureq::Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, BLOCKED)));
        }
        addrs.truncate(keep);
        Ok(addrs)
    }
}

/// True when `ip` is a globally routable unicast address.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => is_public_v4(a),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            is_public_v6(a)
        }
    }
}

fn is_public_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_broadcast()
        || a.is_documentation()
        || a.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64)       // 100.64/10 shared
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)    // 192.0.0/24
        || (o[0] == 198 && (o[1] & 0xfe) == 18)       // 198.18/15 benchmark
        || o[0] >= 240)                               // reserved
}

fn is_public_v6(a: Ipv6Addr) -> bool {
    let s = a.segments();
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00                  // fc00::/7 unique local
        || (s[0] & 0xffc0) == 0xfe80                  // fe80::/10 link local
        || (s[0] & 0xffc0) == 0xfec0                  // fec0::/10 site local
        || (s[0] == 0x2001 && s[1] == 0x0db8)         // documentation
        || (s[0] == 0x64 && s[1] == 0xff9b))          // NAT64 (embeds v4)
}

/// What came back from upstream.
pub struct Fetched {
    pub status: u16,
    /// URL after redirects.
    pub url: Url,
    pub content_type: String,
    /// Empty when `skipped`.
    pub body: Vec<u8>,
    /// The content type was not one the caller wanted, so the body
    /// was not read.
    pub skipped: bool,
    /// Freshness the origin asked for (`s-maxage` or `max-age`), in
    /// seconds.  The cache uses it only to keep things longer than its
    /// own minimum.
    pub max_age: Option<u64>,
    /// Until the response headers arrived, and reading the body.
    pub wait: Duration,
    pub download: Duration,
}

/// `s-maxage` if present, else `max-age`.
pub fn parse_max_age(cc: &str) -> Option<u64> {
    let mut max_age = None;
    let mut shared = None;
    for d in cc.split(',') {
        let (k, v) = d.split_once('=').unwrap_or((d, ""));
        let v = v.trim().trim_matches('"').parse::<u64>().ok();
        match k.trim().to_ascii_lowercase().as_str() {
            "s-maxage" => shared = v,
            "max-age" => max_age = v,
            _ => {}
        }
    }
    shared.or(max_age)
}

#[derive(Debug)]
pub enum FetchError {
    Blocked,
    TooBig,
    Other(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Blocked => write!(f, "that address is not allowed (private network)"),
            FetchError::TooBig => write!(f, "the response is too large"),
            FetchError::Other(s) => write!(f, "{s}"),
        }
    }
}

fn classify(e: ureq::Error) -> FetchError {
    match &e {
        ureq::Error::Io(io) if io.to_string().contains(BLOCKED) => FetchError::Blocked,
        ureq::Error::BodyExceedsLimit(_) => FetchError::TooBig,
        _ => FetchError::Other(e.to_string()),
    }
}

/// Redirects followed before giving up.
const MAX_REDIRECTS: usize = 10;

/// Angel's one cookie jar.  Angel keeps it itself rather than leaving
/// it to ureq so that it can see each cookie's domain (to clear one
/// site) and save session cookies too.
type Jar = Mutex<CookieStore>;

#[derive(Clone)]
pub struct Fetcher {
    agent: Agent,
    /// Shared by every client: the household is one "browser".
    jar: Arc<Jar>,
}

fn make_agent(user_agent: &str, allow_private: bool) -> Agent {
    // Redirects are followed in `get`, so cookies are handled on every hop.
    let config = Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(30)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .user_agent(user_agent)
        .build();
    if allow_private {
        Agent::new_with_config(config)
    } else {
        let resolver = GuardResolver { inner: DefaultResolver::default() };
        Agent::with_parts(config, DefaultConnector::new(), resolver)
    }
}

fn lock(jar: &Jar) -> MutexGuard<'_, CookieStore> {
    jar.lock().unwrap_or_else(|e| e.into_inner())
}

/// True when `domain` (a cookie's) is `site` or under it.
fn under(domain: &str, site: &str) -> bool {
    let d = domain.trim_start_matches('.');
    d.eq_ignore_ascii_case(site)
        || (d.len() > site.len()
            && d.as_bytes()[d.len() - site.len() - 1] == b'.'
            && d[d.len() - site.len()..].eq_ignore_ascii_case(site))
}

/// Put `site`'s fixed cookies in the jar, for the whole registrable
/// domain of `url`.
fn set_cookies(jar: &Jar, url: &Url, site: &Site) {
    if site.cookies.is_empty() {
        return;
    }
    let Some(host) = url.host_str() else { return };
    let domain = sitelist::registrable(host);
    let mut jar = lock(jar);
    for (name, val) in &site.cookies {
        let text = format!("{name}={val}; Domain={domain}; Path=/");
        if let Err(e) = jar.parse(&text, url) {
            eprintln!("angel: cookie {name} for {domain}: {e}");
        }
    }
}

/// Keep the `Set-Cookie`s from a response to `url`.  A cookie for a
/// whole public suffix (`Domain=co.uk`) is refused, as browsers do.
fn store_cookies(jar: &Jar, url: &Url, resp: &ureq::http::Response<ureq::Body>) {
    let cookies: Vec<RawCookie<'static>> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| RawCookie::parse(v.to_string()).ok())
        .filter(|c| match c.domain() {
            Some(d) => {
                let d = d.trim_start_matches('.');
                psl::suffix_str(d) != Some(d)
            }
            None => true,
        })
        .collect();
    if !cookies.is_empty() {
        lock(jar).store_response_cookies(cookies.into_iter(), url);
    }
}

fn cookie_header(jar: &Jar, url: &Url) -> Option<String> {
    let jar = lock(jar);
    let v: Vec<String> = jar.get_request_values(url).map(|(n, v)| format!("{n}={v}")).collect();
    (!v.is_empty()).then(|| v.join("; "))
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

impl Fetcher {
    pub fn new(user_agent: &str, allow_private: bool) -> Fetcher {
        Fetcher { agent: make_agent(user_agent, allow_private), jar: Arc::new(Mutex::new(CookieStore::default())) }
    }

    /// The unexpired cookies, session ones included, as JSON, for
    /// `--cookie-jar`.
    pub fn cookies_json(&self) -> Result<Vec<u8>, String> {
        let live = {
            let jar = lock(&self.jar);
            CookieStore::from_cookies(jar.iter_unexpired().map(|c| Ok::<_, ()>(c.clone())), false)
                .map_err(|()| "copying the jar".to_string())?
        };
        let mut out = Vec::new();
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(&live, &mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// Replace the jar with saved JSON.
    pub fn load_cookies(&self, json: &[u8]) -> Result<(), String> {
        let store = cookie_store::serde::json::load_all(json).map_err(|e| e.to_string())?;
        *lock(&self.jar) = store;
        Ok(())
    }

    /// How many cookies the jar holds for `host`'s registrable domain.
    pub fn cookie_count(&self, host: &str) -> usize {
        let site = sitelist::registrable(host);
        let jar = lock(&self.jar);
        jar.iter_unexpired().filter(|c| c.domain.as_cow().is_some_and(|d| under(&d, &site))).count()
    }

    /// Forget every cookie for `host`'s registrable domain and the
    /// names under it; returns how many there were.
    pub fn clear_cookies(&self, host: &str) -> usize {
        let site = sitelist::registrable(host);
        let mut jar = lock(&self.jar);
        let doomed: Vec<(String, String, String)> = jar
            .iter_any()
            .filter_map(|c| {
                let d = c.domain.as_cow()?;
                under(&d, &site).then(|| (d.into_owned(), String::from(&c.path), c.name().to_string()))
            })
            .collect();
        let live = jar.iter_unexpired().filter(|c| c.domain.as_cow().is_some_and(|d| under(&d, &site))).count();
        for (d, p, n) in &doomed {
            jar.remove(d, p, n);
        }
        live
    }

    /// GET `url`, following redirects, reading at most `limit` bytes.
    /// The body is read only when `want(content_type)` is true.
    /// `site` is the per-site settings for `url`'s host.
    pub fn get(
        &self,
        url: &Url,
        accept: &str,
        limit: u64,
        want: impl Fn(&str) -> bool,
        site: &Site,
    ) -> Result<Fetched, FetchError> {
        // A fresh visitor: a jar of its own for this fetch (redirect
        // hops included), dropped after.
        let own;
        let jar: &Jar = if site.fresh {
            own = Mutex::new(CookieStore::default());
            &own
        } else {
            &self.jar
        };
        set_cookies(jar, url, site);

        let start = Instant::now();
        let mut cur = url.clone();
        let mut hops = 0;
        let mut resp = loop {
            let mut req = self
                .agent
                .get(cur.as_str())
                .header("Accept", accept)
                .header("Accept-Language", "en-US,en;q=0.5");
            if let Some(ua) = &site.user_agent {
                req = req.header("User-Agent", ua);
            }
            if let Some(c) = cookie_header(jar, &cur) {
                req = req.header("Cookie", c);
            }
            let resp = req.call().map_err(classify)?;
            store_cookies(jar, &cur, &resp);
            let next = is_redirect(resp.status().as_u16())
                .then(|| resp.headers().get("location"))
                .flatten()
                .and_then(|v| v.to_str().ok())
                .and_then(|loc| cur.join(loc.trim()).ok())
                .filter(|u| matches!(u.scheme(), "http" | "https"));
            match next {
                Some(u) if hops < MAX_REDIRECTS => {
                    hops += 1;
                    cur = u;
                }
                Some(_) => return Err(FetchError::Other("too many redirects".into())),
                None => break resp,
            }
        };
        let wait = start.elapsed();

        let status = resp.status().as_u16();
        let final_url = cur;
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let max_age = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_max_age);
        if !want(&content_type) {
            let body = Vec::new();
            return Ok(Fetched {
                status,
                url: final_url,
                content_type,
                body,
                skipped: true,
                max_age,
                wait,
                download: Duration::ZERO,
            });
        }
        let body = resp
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(classify)?;
        let download = start.elapsed() - wait;

        Ok(Fetched { status, url: final_url, content_type, body, skipped: false, max_age, wait, download })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> bool {
        is_public(s.parse().unwrap())
    }

    #[test]
    fn cookie_domains() {
        assert!(under("bbc.co.uk", "bbc.co.uk"));
        assert!(under(".www.BBC.co.uk", "bbc.co.uk"));
        assert!(!under("notbbc.co.uk", "bbc.co.uk"));
        assert!(!under("co.uk", "bbc.co.uk"));
    }

    #[test]
    fn jar_clear_and_save() {
        let f = Fetcher::new("test", false);
        let u = Url::parse("https://www.bbc.co.uk/news").unwrap();
        let o = Url::parse("https://cnn.com/").unwrap();
        {
            let mut j = lock(&f.jar);
            j.parse("a=1; Path=/", &u).unwrap();
            j.parse("b=2; Domain=bbc.co.uk; Path=/; Max-Age=3600", &u).unwrap();
            j.parse("c=3; Path=/", &o).unwrap();
        }
        assert_eq!(f.cookie_count("bbc.co.uk"), 2);
        // Session cookies are saved too.
        let saved = f.cookies_json().unwrap();
        assert_eq!(f.clear_cookies("news.bbc.co.uk"), 2);
        assert_eq!(f.cookie_count("www.bbc.co.uk"), 0);
        assert_eq!(f.cookie_count("cnn.com"), 1);
        f.load_cookies(&saved).unwrap();
        assert_eq!(f.cookie_count("bbc.co.uk"), 2);
    }

    #[test]
    fn max_age() {
        assert_eq!(parse_max_age("public, max-age=300"), Some(300));
        assert_eq!(parse_max_age("max-age=60, s-maxage=7200"), Some(7200));
        assert_eq!(parse_max_age("no-store"), None);
        assert_eq!(parse_max_age("max-age=\"90\""), Some(90));
    }

    #[test]
    fn public() {
        assert!(ok("8.8.8.8"));
        assert!(ok("151.101.1.67"));
        assert!(ok("2606:4700::1111"));
    }

    #[test]
    fn private() {
        for a in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254",
            "0.0.0.0", "100.64.0.1", "255.255.255.255", "224.0.0.1", "::1", "::",
            "fe80::1", "fd00::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1", "64:ff9b::7f00:1",
        ] {
            assert!(!ok(a), "{a} should be blocked");
        }
    }
}
