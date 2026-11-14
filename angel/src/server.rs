//! HTTP front end: a fixed pool of worker threads, each pulling
//! requests off one shared tiny_http server, plus routing.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::net::{IpAddr, SocketAddr};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use percent_encoding::percent_decode_str;
use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

use crate::cache::{self, Cache};
use crate::fetch::{self, FetchError, Fetcher};
use crate::image::{self, Verdict};
use crate::render::Mode;
use crate::sitelist::{self, SiteList};
use crate::sites::Sites;
use crate::stats::{self, Row, Stat, Stats};
use crate::{Opts, clean, pages, render, rewrite, target};

struct App {
    opts: Opts,
    fetcher: Fetcher,
    pages: Cache<Page>,
    images: Cache<Img>,
    list: SiteList,
    sites: Sites,
    stats: Stats,
}

/// The parts of a request the handlers look at.
struct Req {
    method: Method,
    /// Raw request-target: path and query.
    target: String,
    host: Option<String>,
    origin: Option<String>,
    referer: Option<String>,
    fetch_site: Option<String>,
    /// POST body (form data), capped at `BODY_LIMIT`.
    body: Vec<u8>,
}

const BODY_LIMIT: u64 = 16 * 1024;

/// A cached `/read/` result.
enum Page {
    /// Rendered HTML (bar slot still unfilled), the render info, and
    /// the page's final URL (after redirects).
    Html { html: String, info: String, orig: Url },
    /// Not HTML: send the browser to the original.
    Redirect { to: String, note: String },
}

/// A cached `/img/` result, including drops and upstream failures so
/// they are not fetched again.
struct Img {
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
    drop: Option<&'static str>,
    note: String,
}

const MIB: usize = 1024 * 1024;
const HOUR: Duration = Duration::from_secs(3600);

/// What a handler produces; `note` goes in the log line.
struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    headers: Vec<(&'static str, String)>,
    note: String,
    /// Boxed: most replies carry only the default.
    stat: Box<Stat>,
}

impl Reply {
    fn html(status: u16, body: String) -> Reply {
        Reply {
            status,
            content_type: "text/html; charset=utf-8",
            body: body.into_bytes(),
            headers: Vec::new(),
            note: String::new(),
            stat: Box::default(),
        }
    }

    fn error(status: u16, msg: &str, orig: Option<&Url>) -> Reply {
        let mut r = Reply::html(status, pages::error(status, msg, orig));
        r.note = msg.to_string();
        r
    }

    fn redirect(to: &str) -> Reply {
        let mut r = Reply::html(302, String::new());
        r.headers.push(("Location", to.to_string()));
        r
    }

    fn note(mut self, s: impl Into<String>) -> Reply {
        self.note = s.into();
        self
    }

    fn stat(mut self, st: Stat) -> Reply {
        self.stat = Box::new(st);
        self
    }
}

pub fn run(opts: Opts) -> Result<(), String> {
    let server = Server::http(opts.listen).map_err(|e| format!("listen {}: {e}", opts.listen))?;
    let server = Arc::new(server);
    let min = Duration::from_secs(opts.cache_min * 60);
    let app = Arc::new(App {
        fetcher: Fetcher::new(&opts.user_agent, opts.allow_private),
        pages: Cache::new(opts.cache_mb * MIB, min, 24 * HOUR),
        images: Cache::new(opts.image_cache_mb * MIB, min, 7 * 24 * HOUR),
        list: SiteList::open(opts.list.clone()),
        sites: Sites::open(opts.sites.clone()),
        stats: Stats::new(opts.stats_size, opts.log.as_deref())?,
        opts,
    });
    let n = app.list.with(|l| l.entries().count());
    eprintln!("angel: site list {} ({n} entries)", app.list.path().display());
    eprintln!("angel: site settings {} ({} sites)", app.sites.path().display(), app.sites.count());
    if let Some(jar) = app.opts.cookie_jar.clone() {
        match fs::read(&jar) {
            Ok(b) => match app.fetcher.load_cookies(&b) {
                Ok(()) => eprintln!("angel: cookies from {}", jar.display()),
                Err(e) => eprintln!("angel: {}: {e} (starting empty)", jar.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("--cookie-jar {}: {e}", jar.display())),
        }
        let app = Arc::clone(&app);
        thread::Builder::new()
            .name("cookie-saver".into())
            .spawn(move || save_cookies(&app.fetcher, &jar))
            .map_err(|e| format!("spawn: {e}"))?;
    }

    eprintln!(
        "angel: listening on http://{}/ ({} threads, cache {}+{} MB for {} min+{}{})",
        app.opts.listen,
        app.opts.threads,
        app.opts.cache_mb,
        app.opts.image_cache_mb,
        app.opts.cache_min,
        if app.opts.local_only { ", local only" } else { "" },
        if app.opts.allow_private { ", private fetches allowed" } else { "" },
    );

    let workers: Vec<_> = (0..app.opts.threads)
        .map(|i| {
            let server = Arc::clone(&server);
            let app = Arc::clone(&app);
            thread::Builder::new()
                .name(format!("worker-{i}"))
                .spawn(move || worker(&server, &app))
                .expect("spawn worker")
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

/// Every minute, write the jar out if it changed: to a temporary file
/// readable only by its owner, then renamed over the old one.
fn save_cookies(fetcher: &Fetcher, path: &std::path::Path) {
    let mut last = fs::read(path).ok();
    loop {
        thread::sleep(Duration::from_secs(60));
        let json = match fetcher.cookies_json() {
            Ok(j) => j,
            Err(e) => {
                eprintln!("angel: cookie jar: {e}");
                continue;
            }
        };
        if last.as_ref() == Some(&json) {
            continue;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        let r = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| f.write_all(&json).and_then(|()| f.sync_all()))
            .and_then(|()| fs::rename(&tmp, path));
        match r {
            Ok(()) => last = Some(json),
            Err(e) => eprintln!("angel: --cookie-jar {}: {e}", path.display()),
        }
    }
}

fn worker(server: &Server, app: &App) {
    loop {
        match server.recv() {
            Ok(rq) => serve(app, rq),
            Err(e) => {
                eprintln!("angel: recv: {e}");
                return;
            }
        }
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback(),
        IpAddr::V6(a) => a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v| v.is_loopback()),
    }
}

fn serve(app: &App, mut rq: Request) {
    let start = Instant::now();
    let client: Option<SocketAddr> = rq.remote_addr().copied();
    let method = rq.method().clone();
    let path = rq.url().to_string();
    let header = |name: &str| {
        rq.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_string())
    };
    let mut req = Req {
        method: method.clone(),
        target: path.clone(),
        host: header("Host"),
        origin: header("Origin"),
        referer: header("Referer"),
        fetch_site: header("Sec-Fetch-Site"),
        body: Vec::new(),
    };

    let reply = if app.opts.local_only && !client.is_some_and(|c| is_loopback(c.ip())) {
        Reply::error(403, "This angel only serves its own machine.", None)
    } else if method != Method::Get && method != Method::Head && method != Method::Post {
        Reply::error(405, "Only GET and POST are supported.", None)
    } else if method == Method::Post
        && let Err(e) = rq.as_reader().take(BODY_LIMIT).read_to_end(&mut req.body)
    {
        Reply::error(400, &format!("Could not read the request: {e}"), None)
    } else {
        // A panic (say, inside the extractor) must not take the worker
        // thread with it.
        let ip = client.map(|c| c.ip());
        panic::catch_unwind(AssertUnwindSafe(|| route(app, &req, ip)))
            .unwrap_or_else(|_| Reply::error(500, "Angel hit an internal error on this page.", None))
    };

    let status = reply.status;
    let len = reply.body.len();
    let note = reply.note.clone();
    let stat = (*reply.stat).clone();
    if let Err(e) = rq.respond(build(reply)) {
        eprintln!("angel: respond: {e}");
    }

    let total = start.elapsed().as_millis() as u64;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let who = client.map(|c| c.ip().to_string()).unwrap_or_else(|| "-".into());
    eprintln!("{now} {who} {method} {path} {status} {len} {total}ms{}{note}", if note.is_empty() { "" } else { " " });
    app.stats.record(Row {
        time: now,
        client: client.map(|c| c.ip()),
        method: method.to_string(),
        kind: kind(&path),
        target: path,
        status,
        sent: len as u64,
        total_ms: total,
        stat,
    });
}

/// The request's kind for the statistics: its first path word.
fn kind(target: &str) -> &'static str {
    let path = target.split('?').next().unwrap_or("");
    let word = path.trim_start_matches('/').split('/').next().unwrap_or("");
    match word {
        "" => "front",
        "read" => "read",
        "article" => "article",
        "headlines" => "headlines",
        "reload" => "reload",
        "clear" => "clear",
        "img" => "img",
        "add" => "add",
        "remove" => "remove",
        "list" => "list",
        "stats" => "stats",
        "angel.css" => "css",
        _ => "other",
    }
}

fn build(r: Reply) -> Response<Cursor<Vec<u8>>> {
    let mut resp = Response::from_data(r.body).with_status_code(r.status);
    let mut add = |k: &str, v: &str| {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    };
    add("Content-Type", r.content_type);
    add("Content-Security-Policy", pages::CSP);
    add("X-Content-Type-Options", "nosniff");
    let has = |name: &str| r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name));
    let (cache, referrer) = (has("Cache-Control"), has("Referrer-Policy"));
    for (k, v) in &r.headers {
        add(k, v);
    }
    if !referrer {
        add("Referrer-Policy", "no-referrer");
    }
    if !cache {
        add("Cache-Control", "no-store");
    }
    resp
}

fn route(app: &App, req: &Req, client: Option<IpAddr>) -> Reply {
    let raw = req.target.as_str();
    let (path, query) = raw.split_once('?').unwrap_or((raw, ""));

    if req.method == Method::Post {
        return match path {
            "/add" | "/remove" if !same_origin(req) => {
                Reply::error(403, "List changes must come from Angel's own confirmation page.", None)
            }
            "/add" => add_post(app, &req.body),
            "/remove" => remove_post(app, &req.body),
            _ => Reply::error(405, "POST is only for /add and /remove.", None),
        };
    }

    for mode in [Mode::Auto, Mode::Article, Mode::Headlines] {
        if let Some(rest) = raw.strip_prefix(&format!("/{}/", mode_name(mode))) {
            return read(app, rest, mode);
        }
        if let Some(rest) = raw.strip_prefix(&format!("/reload/{}/", mode_name(mode))) {
            return reload(app, rest, mode);
        }
        if let Some(rest) = raw.strip_prefix(&format!("/clear/{}/", mode_name(mode))) {
            return clear(app, req, rest, mode);
        }
    }
    if let Some(rest) = raw.strip_prefix("/img/") {
        return image(app, rest);
    }
    if let Some(rest) = raw.strip_prefix("/add/") {
        return add_get(app, rest);
    }
    if let Some(rest) = raw.strip_prefix("/remove/") {
        return remove_get(app, rest);
    }
    match path {
        "/" => Reply::html(200, pages::front()),
        "/read" | "/read/" => {
            if query.is_empty() {
                return Reply::redirect("/");
            }
            match target::from_query(query) {
                Ok(u) => Reply::redirect(&rewrite::read_url(&u)),
                Err(e) => Reply::error(400, &e, None),
            }
        }
        "/add" | "/remove" => {
            let v = url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "url")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            if v.trim().is_empty() {
                return Reply::redirect("/list");
            }
            let v = v.trim().to_string();
            if path == "/add" { add_get(app, &v) } else { remove_get(app, &v) }
        }
        "/list" => {
            if url::form_urlencoded::parse(query.as_bytes()).any(|(k, v)| k == "format" && v == "text") {
                let mut r = Reply::html(200, app.list.with(|l| l.text()));
                r.content_type = "text/plain; charset=utf-8";
                return r;
            }
            let entries = app.list.with(|l| l.entries().map(str::to_string).collect::<Vec<_>>());
            Reply::html(200, pages::list(&entries, &app.list.path().display().to_string()))
        }
        "/stats" => {
            let admin = client.is_some_and(is_loopback);
            let all = admin && url::form_urlencoded::parse(query.as_bytes()).any(|(k, v)| k == "all" && v == "1");
            let rows = app.stats.rows(if all { None } else { client });
            Reply::html(200, pages::stats(&stats::body(&rows, all, admin)))
        }
        "/angel.css" => {
            let mut r = Reply::html(200, pages::CSS.to_string());
            r.content_type = "text/css; charset=utf-8";
            r.headers.push(("Cache-Control", "max-age=3600".into()));
            r
        }
        _ => Reply::error(404, "Nothing here.", None),
    }
}

fn fetch_error(e: FetchError, orig: &Url) -> Reply {
    let status = match &e {
        FetchError::Blocked => 403,
        FetchError::TooBig => 413,
        FetchError::Other(s) if s.to_ascii_lowercase().contains("timeout") => 504,
        FetchError::Other(_) => 502,
    };
    Reply::error(status, &e.to_string(), Some(orig))
}

fn is_html(ct: &str) -> bool {
    let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    ct.is_empty() || ct == "text/html" || ct == "application/xhtml+xml"
}

const HTML_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// The path word for a mode: `/read/`, `/article/`, `/headlines/`.
fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Auto => "read",
        Mode::Article => "article",
        Mode::Headlines => "headlines",
    }
}

fn page_key(mode: Mode, url: &Url) -> String {
    format!("{} {url}", mode_name(mode))
}

/// `/reload/<mode>/<url>`: forget the cached copy, then send the
/// browser to the normal address, which fetches a fresh one.
fn reload(app: &App, rest: &str, mode: Mode) -> Reply {
    let url = match target::from_path(rest) {
        Ok(u) => u,
        Err(e) => return Reply::error(400, &e, None),
    };
    app.pages.remove(&page_key(mode, &url));
    Reply::redirect(&rewrite::mode_url(mode_name(mode), &url)).note("reload")
}

/// `/clear/<mode>/<url>`: forget the shared jar's cookies for the
/// page's site (its registrable domain and every name under it).  A
/// GET, like the bar's other links; a link from another site is
/// refused, since it would be clearing cookies behind the user's back.
fn clear(app: &App, req: &Req, rest: &str, mode: Mode) -> Reply {
    if req.fetch_site.as_deref() == Some("cross-site") {
        return Reply::error(403, "Clearing cookies has to start from Angel's own pages.", None);
    }
    let url = match target::from_path(rest) {
        Ok(u) => u,
        Err(e) => return Reply::error(400, &e, None),
    };
    // The cached page knows where redirects ended up (bbc.com goes to
    // www.bbc.com); its site is the one whose cookies matter.
    let orig = match app.pages.get(&page_key(mode, &url)).as_ref().map(|h| &*h.val) {
        Some(Page::Html { orig, .. }) => orig.clone(),
        _ => url.clone(),
    };
    let host = orig.host_str().unwrap_or("");
    let site = sitelist::registrable(host);
    let n = app.fetcher.clear_cookies(host);
    let fixed = !app.sites.for_host(host).cookies.is_empty();
    let here = rewrite::mode_url(mode_name(mode), &url);
    Reply::html(200, pages::cleared(&site, n, fixed, &here, &format!("/reload{here}"))).note(format!("cleared={n} {site}"))
}

fn read(app: &App, rest: &str, mode: Mode) -> Reply {
    let url = match target::from_path(rest) {
        Ok(u) => u,
        Err(e) => return Reply::error(400, &e, None),
    };
    let key = page_key(mode, &url);
    let mut st = Stat { site: url.host_str().map(sitelist::registrable), ..Stat::default() };
    let (page, age) = match app.pages.get(&key) {
        Some(hit) => {
            st.cache_hit = true;
            (hit.val, Some(hit.age))
        }
        None => match fetch_page(app, &url, mode, &mut st) {
            Ok((page, size, ttl)) => (app.pages.put(&key, page, size, ttl), None),
            Err(reply) => return reply.stat(st),
        },
    };
    let cached = age.map(|a| format!(" cache=hit age={}s", a.as_secs())).unwrap_or_default();
    match &*page {
        Page::Html { html, info, orig } => {
            let here = rewrite::mode_url(mode_name(mode), &url);
            let (reload, clear) = (format!("/reload{here}"), format!("/clear{here}"));
            let age = age.map(cache::age_text);
            let host = orig.host_str().unwrap_or("");
            let listed = app.list.with(|l| !l.matching(host).is_empty());
            let slot = pages::bar_slot(&reload, &clear, app.fetcher.cookie_count(host), age.as_deref(), orig, listed);
            let html = html.replace(pages::BAR_SLOT, &slot);
            Reply::html(200, html).note(format!("{info}{cached}")).stat(st)
        }
        Page::Redirect { to, note } => Reply::redirect(to).note(format!("{note}{cached}")).stat(st),
    }
}

/// Fetch and render: the page, its size, and how long to keep it.
/// Errors are returned as the reply to send and are not cached.
fn fetch_page(app: &App, url: &Url, mode: Mode, st: &mut Stat) -> Result<(Page, usize, Duration), Reply> {
    let site = app.sites.for_host(url.host_str().unwrap_or(""));
    let f = app.fetcher.get(url, HTML_ACCEPT, fetch::HTML_LIMIT, is_html, &site).map_err(|e| fetch_error(e, url))?;
    st.note_fetch(&f);
    if f.status >= 400 {
        let msg = format!("The site answered with HTTP status {}.", f.status);
        return Err(Reply::error(f.status, &msg, Some(&f.url)));
    }
    let ttl = app.pages.ttl(f.max_age);
    if f.skipped {
        // Not HTML (PDF, video, ...): hand the browser the original.
        let to = f.url.to_string();
        let size = to.len();
        return Ok((Page::Redirect { to, note: format!("non-html {}", f.content_type) }, size, ttl));
    }

    // The AMP fetch happens inside render; its time is kept apart so
    // render time is Angel's own.
    let amp_time = std::cell::Cell::new(Duration::ZERO);
    let amp_bytes = std::cell::Cell::new(0u64);
    let amp = |u: &Url| -> Option<(Vec<u8>, String, Url)> {
        let t = Instant::now();
        let site = app.sites.for_host(u.host_str().unwrap_or(""));
        let a = app.fetcher.get(u, HTML_ACCEPT, fetch::HTML_LIMIT, is_html, &site);
        amp_time.set(amp_time.get() + t.elapsed());
        let a = a.ok()?;
        amp_bytes.set(amp_bytes.get() + a.body.len() as u64);
        (a.status < 400 && !a.skipped).then_some((a.body, a.content_type, a.url))
    };
    let t = Instant::now();
    let r = render::render(&f.body, &f.content_type, &f.url, mode, Some(&amp));
    let render_time = t.elapsed().saturating_sub(amp_time.get());
    st.render_ms = Some(render_time.as_millis() as u64);
    st.origin_bytes += amp_bytes.get();
    let info = format!("{} fetched={}KB render={}ms", r.info, f.body.len() / 1024, render_time.as_millis());
    let size = r.html.len() + info.len() + f.url.as_str().len();
    Ok((Page::Html { html: r.html, info, orig: f.url }, size, ttl))
}

fn is_image(ct: &str) -> bool {
    let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    // SVG can carry script; leave it out.  An empty or generic type is
    // let through and sniffed.
    ct != "image/svg+xml" && (ct.starts_with("image/") || ct.is_empty() || ct == "application/octet-stream")
}

/// Image proxy, so `img-src 'self'` works.  The format is sniffed from
/// the bytes; trackers, tiny images and (by default) animations are
/// replaced with a blank pixel.  Results, drops included, are cached.
fn image(app: &App, rest: &str) -> Reply {
    let url = match target::from_path(rest) {
        Ok(u) => u,
        Err(e) => return Reply::error(400, &e, None),
    };
    let key = url.to_string();
    let mut st = Stat { site: url.host_str().map(sitelist::registrable), ..Stat::default() };
    let (img, cached) = match app.images.get(&key) {
        Some(hit) => {
            st.cache_hit = true;
            (hit.val, format!(" cache=hit age={}s", hit.age.as_secs()))
        }
        None => match fetch_image(app, &url, &mut st) {
            Ok((img, ttl)) => {
                let size = img.body.len() + img.note.len();
                (app.images.put(&key, img, size, ttl), String::new())
            }
            Err(reply) => return reply.stat(st),
        },
    };
    let mut headers = Vec::new();
    if img.status == 200 {
        headers.push(("Cache-Control", "max-age=86400".to_string()));
    }
    if let Some(why) = img.drop {
        headers.push(("X-Angel-Drop", why.to_string()));
    }
    Reply {
        status: img.status,
        content_type: img.mime,
        body: img.body.clone(),
        headers,
        note: format!("{}{cached}", img.note),
        stat: Box::new(st),
    }
}

fn blank(why: &'static str, note: String) -> Img {
    Img { status: 200, mime: "image/gif", body: image::BLANK_GIF.to_vec(), drop: Some(why), note }
}

fn fetch_image(app: &App, url: &Url, st: &mut Stat) -> Result<(Img, Duration), Reply> {
    let ttl = app.images.ttl(None);
    if url.host_str().is_some_and(clean::is_tracker_host) {
        return Ok((blank("tracker", "img drop=tracker".into()), ttl));
    }
    let accept = "image/avif,image/webp,image/png,image/jpeg,image/*;q=0.8";
    let site = app.sites.for_host(url.host_str().unwrap_or(""));
    let f = app.fetcher.get(url, accept, fetch::IMAGE_LIMIT, is_image, &site).map_err(|e| fetch_error(e, url))?;
    st.note_fetch(&f);
    if f.status >= 400 || f.skipped {
        let note = format!("img upstream={} type={}", f.status, f.content_type);
        return Ok((Img { status: 404, mime: "text/plain", body: Vec::new(), drop: None, note }, ttl));
    }
    let ttl = app.images.ttl(f.max_age);
    let len = f.body.len();
    let img = match image::filter(f.body, app.opts.animated) {
        Verdict::Keep { body, mime } => {
            let note = format!("img {mime} {}KB", body.len() / 1024);
            Img { status: 200, mime, body, drop: None, note }
        }
        Verdict::Drop(why) => blank(why, format!("img drop={why} {}KB", len / 1024)),
    };
    Ok((img, ttl))
}

/// A POST is accepted only from Angel's own pages: the `Origin` (or,
/// without one, the `Referer`) must name this server, or the browser
/// must say the request is same-origin.  Forms are served with
/// `Referrer-Policy: same-origin`, so browsers send a real `Origin`.
fn same_origin(req: &Req) -> bool {
    let Some(host) = req.host.as_deref() else { return false };
    let ours = |u: &str| {
        Url::parse(u).ok().is_some_and(|u| {
            let h = u.host_str().unwrap_or("");
            let hp = match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            };
            hp.eq_ignore_ascii_case(host)
        })
    };
    match (&req.origin, &req.referer) {
        (Some(o), _) => ours(o),
        (None, Some(r)) => ours(r),
        (None, None) => req.fetch_site.as_deref() == Some("same-origin"),
    }
}

/// What `/add/...` or `/remove/...` names: a bare domain or pattern,
/// or a page address (whose host is used).
enum Subject {
    Entry(String),
    Page(Url),
}

fn subject(rest: &str) -> Result<Subject, String> {
    let dec = percent_decode_str(rest).decode_utf8_lossy();
    let s = dec.trim();
    if !s.contains('/') && !s.contains(':') {
        return sitelist::normalize(s).map(Subject::Entry);
    }
    target::from_path(rest).map(Subject::Page)
}

/// Where "back to the page" goes: only Angel's own reader pages.
fn safe_back(b: &str) -> Option<&str> {
    ["/read/", "/article/", "/headlines/"].iter().any(|p| b.starts_with(p)).then_some(b)
}

fn list_reply(html: String) -> Reply {
    let mut r = Reply::html(200, html);
    r.headers.push(("Referrer-Policy", "same-origin".into()));
    r
}

fn add_get(app: &App, rest: &str) -> Reply {
    let (entry, alt, covered, back) = match subject(rest) {
        Err(e) => return Reply::error(400, &e, None),
        Ok(Subject::Entry(e)) => {
            let covered = app.list.with(|l| {
                if sitelist::is_pattern(&e) {
                    l.entries().filter(|x| *x == e).map(str::to_string).collect()
                } else {
                    l.matching(&e)
                }
            });
            (e, None, covered, None)
        }
        Ok(Subject::Page(u)) => {
            let host = u.host_str().unwrap_or("").to_string();
            let domain = sitelist::registrable(&host);
            let alt = (domain != host).then_some(host.clone());
            let covered = app.list.with(|l| l.matching(&host));
            (domain, alt, covered, Some(rewrite::read_url(&u)))
        }
    };
    list_reply(pages::add_confirm(&entry, alt.as_deref(), &covered, back.as_deref()))
}

fn remove_get(app: &App, rest: &str) -> Reply {
    let (name, entries, back) = match subject(rest) {
        Err(e) => return Reply::error(400, &e, None),
        Ok(Subject::Entry(e)) => {
            let entries = app.list.with(|l| {
                let mut v: Vec<String> = l.entries().filter(|x| *x == e).map(str::to_string).collect();
                if !sitelist::is_pattern(&e) {
                    v.extend(l.matching(&e).into_iter().filter(|x| *x != e));
                }
                v
            });
            (e, entries, None)
        }
        Ok(Subject::Page(u)) => {
            let host = u.host_str().unwrap_or("").to_string();
            let entries = app.list.with(|l| l.matching(&host));
            (host, entries, Some(rewrite::read_url(&u)))
        }
    };
    list_reply(pages::remove_confirm(&name, &entries, back.as_deref()))
}

fn form(body: &[u8]) -> (Vec<String>, Option<String>) {
    let mut entries = Vec::new();
    let mut back = None;
    for (k, v) in url::form_urlencoded::parse(body) {
        match &*k {
            "entry" => entries.push(v.into_owned()),
            "back" => back = safe_back(&v).map(str::to_string),
            _ => {}
        }
    }
    (entries, back)
}

fn add_post(app: &App, body: &[u8]) -> Reply {
    let (entries, back) = form(body);
    let Some(raw) = entries.first() else {
        return Reply::error(400, "No entry given.", None);
    };
    let entry = match sitelist::normalize(raw) {
        Ok(e) => e,
        Err(e) => return Reply::error(400, &e, None),
    };
    let date = sitelist::today();
    let msg = match app.list.edit(|l| l.add(&entry, &date)) {
        Ok(true) => format!("Added {entry} to the site list."),
        Ok(false) => format!("{entry} is already on the site list."),
        Err(e) => return Reply::error(500, &e, None),
    };
    Reply::html(200, pages::list_done(&msg, back.as_deref())).note(format!("list add {entry}"))
}

fn remove_post(app: &App, body: &[u8]) -> Reply {
    let (raw, back) = form(body);
    if raw.is_empty() {
        return Reply::error(400, "Nothing was selected to remove.", None);
    }
    let mut entries = Vec::new();
    for r in &raw {
        match sitelist::normalize(r) {
            Ok(e) => entries.push(e),
            Err(e) => return Reply::error(400, &e, None),
        }
    }
    let mut gone = Vec::new();
    let r = app.list.edit(|l| {
        for e in &entries {
            if l.remove(e) {
                gone.push(e.clone());
            }
        }
        !gone.is_empty()
    });
    if let Err(e) = r {
        return Reply::error(500, &e, None);
    }
    let msg = if gone.is_empty() {
        "Those entries were no longer on the site list.".to_string()
    } else {
        format!("Removed {} from the site list.", gone.join(", "))
    };
    Reply::html(200, pages::list_done(&msg, back.as_deref())).note(format!("list remove {}", gone.join(" ")))
}
