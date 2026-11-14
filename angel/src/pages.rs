//! HTML templates and the stylesheet.
//!
//! The CSP forbids inline style, so the CSS is served as
//! `/angel.css`.  Nothing here uses JavaScript.

use url::Url;

use crate::sitelist;

pub const CSP: &str = "default-src 'none'; img-src 'self'; style-src 'self'; \
                       base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

pub const CSS: &str = r#"
:root {
  --fg: #1d1d1b; --bg: #fbfaf7; --muted: #6b6860; --rule: #dedbd2;
  --link: #1f4e8c; --visited: #6a3d8f; --bar: #f0eee8;
}
@media (prefers-color-scheme: dark) {
  :root {
    --fg: #e6e3dc; --bg: #191917; --muted: #9a968c; --rule: #3a3833;
    --link: #8db4ea; --visited: #c3a2e0; --bar: #23221f;
  }
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body {
  margin: 0; background: var(--bg); color: var(--fg);
  font: 1.15rem/1.6 Georgia, "Iowan Old Style", "Times New Roman", serif;
}
a { color: var(--link); }
a:visited { color: var(--visited); }
.bar {
  display: flex; flex-wrap: wrap; gap: .4rem 1.2rem; align-items: baseline;
  padding: .5rem 1rem; background: var(--bar); border-bottom: 1px solid var(--rule);
  font: .9rem/1.4 system-ui, -apple-system, "Segoe UI", sans-serif;
}
.bar .home { font-weight: 700; text-decoration: none; color: var(--fg); }
.bar .site { color: var(--muted); overflow-wrap: anywhere; }
.bar .orig { margin-left: auto; }
.bar .age { color: var(--muted); }
.index h2 { font-size: 1.2rem; margin: 2rem 0 .4rem; padding-bottom: .2rem; border-bottom: 1px solid var(--rule); }
.index ul { list-style: none; padding: 0; margin: 0; }
.index li { padding: .35rem 0; border-bottom: 1px solid var(--rule); line-height: 1.35; }
.index li a { text-decoration: none; }
.index li a:hover { text-decoration: underline; }
.index .host { color: var(--muted); font-size: .8rem; margin-left: .4rem; }
main { max-width: 40rem; margin: 0 auto; padding: 1.5rem 1rem 4rem; }
h1 { font-size: 1.9rem; line-height: 1.25; margin: .2rem 0 .6rem; }
h2, h3, h4 { line-height: 1.3; }
.byline { color: var(--muted); font-size: .95rem; margin-top: 0; }
article img, article video { max-width: 100%; height: auto; display: block; margin: 1rem auto; }
figure { margin: 1.2rem 0; }
figcaption { color: var(--muted); font-size: .9rem; }
blockquote { margin: 1rem 0; padding-left: 1rem; border-left: 3px solid var(--rule); }
pre, code { font-size: .9em; }
pre { overflow-x: auto; padding: .6rem; background: var(--bar); }
table { border-collapse: collapse; display: block; overflow-x: auto; max-width: 100%; }
td, th { border: 1px solid var(--rule); padding: .2rem .4rem; }
hr { border: 0; border-top: 1px solid var(--rule); }
.notice { border: 1px solid var(--rule); background: var(--bar); padding: .8rem 1rem; }
.muted { color: var(--muted); }
form.go { display: flex; gap: .5rem; margin: 1.5rem 0; }
form.go input[type=text] {
  flex: 1; min-width: 0; font: inherit; padding: .4rem .6rem;
  border: 1px solid var(--rule); background: var(--bg); color: var(--fg);
}
form.go button { font: inherit; padding: .4rem 1rem; }
form.pick { margin: 1rem 0; }
form.pick label { display: block; padding: .2rem 0; }
form.pick button, form.alt button { font: inherit; padding: .3rem .9rem; margin-top: .6rem; }
.entries { list-style: none; padding: 0; }
.entries li { display: flex; gap: 1rem; padding: .3rem 0; border-bottom: 1px solid var(--rule); }
.entries code { flex: 1; overflow-wrap: anywhere; }
.entries a { font: .9rem system-ui, -apple-system, "Segoe UI", sans-serif; }
.entries code a { font: inherit; }
main:has(.stats-page) { max-width: none; }
.stats-page { font: .9rem/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; }
.stats-page h1 { font-family: Georgia, "Times New Roman", serif; }
table.summary, table.stats { border-collapse: collapse; margin: .8rem 0; }
table.stats { display: block; overflow-x: auto; }
.summary th { text-align: left; font-weight: normal; color: var(--muted); padding: .15rem 1.2rem .15rem 0; }
.stats th, .stats td { padding: .2rem .5rem; border-bottom: 1px solid var(--rule); white-space: nowrap; text-align: left; }
.stats th { font-weight: 600; }
.stats td.num { text-align: right; font-variant-numeric: tabular-nums; }
.stats td.url { max-width: 32rem; overflow: hidden; text-overflow: ellipsis; }
"#;

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

fn shell(title: &str, bar: &str, body: &str) -> String {
    shell_with(title, bar, body, "no-referrer")
}

/// The pages with forms use `same-origin`, so the browser sends an
/// `Origin` on the POST that the server can check (under
/// `no-referrer` it sends `Origin: null`).
fn shell_with(title: &str, bar: &str, body: &str, referrer: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"referrer\" content=\"{referrer}\">\n\
         <title>{}</title>\n<link rel=\"stylesheet\" href=\"/angel.css\">\n</head>\n<body>\n\
         <header class=\"bar\"><a class=\"home\" href=\"/\">angel</a>{bar}</header>\n\
         <main>\n{body}\n</main>\n</body>\n</html>\n",
        esc(title)
    )
}

/// Where the server puts the "reload" link and cache age: those
/// change per request, while the rest of the page is cached.
pub const BAR_SLOT: &str = "<!--angel-bar-->";

/// The other rendering of this page, offered in the bar.
pub struct Alt {
    pub label: &'static str,
    pub href: String,
}

fn site_bar(orig: &Url, alt: Option<&Alt>) -> String {
    let alt = alt
        .map(|a| format!(" <a class=\"alt\" href=\"{}\">{}</a>", esc(&a.href), esc(a.label)))
        .unwrap_or_default();
    format!(
        " <span class=\"site\">{}</span>{alt}{BAR_SLOT} <a class=\"orig\" href=\"{}\" rel=\"noreferrer\">open original</a>",
        esc(orig.host_str().unwrap_or("")),
        esc(orig.as_str())
    )
}

pub struct ArticleView<'a> {
    pub orig: &'a Url,
    pub title: &'a str,
    pub byline: Option<&'a str>,
    pub site: Option<&'a str>,
    /// Already sanitized.
    pub content: &'a str,
    /// Shown under the byline, e.g. where the text came from.
    pub source_note: Option<&'a str>,
    pub alt: Option<Alt>,
}

pub fn article(v: &ArticleView) -> String {
    let mut meta = Vec::new();
    if let Some(b) = v.byline {
        meta.push(esc(b));
    }
    if let Some(s) = v.site {
        meta.push(esc(s));
    }
    let meta = if meta.is_empty() {
        String::new()
    } else {
        format!("<p class=\"byline\">{}</p>\n", meta.join(" &middot; "))
    };
    let note = v
        .source_note
        .map(|n| format!("<p class=\"muted\">{}</p>\n", esc(n)))
        .unwrap_or_default();
    let body = format!("<article>\n<h1>{}</h1>\n{meta}{note}{}\n</article>", esc(v.title), v.content);
    shell(v.title, &site_bar(v.orig, v.alt.as_ref()), &body)
}

/// Headlines rendering.  `content` is the already sanitized list.
pub fn headlines(orig: &Url, title: &str, content: &str, alt: Option<Alt>) -> String {
    let body = format!("<div class=\"index\">\n<h1>{}</h1>\n{content}\n</div>", esc(title));
    shell(title, &site_bar(orig, alt.as_ref()), &body)
}

/// Extraction found nothing usable.
pub fn fallback(orig: &Url, title: Option<&str>, why: &str, alt: Option<Alt>) -> String {
    let t = title.unwrap_or(orig.host_str().unwrap_or("page"));
    let body = format!(
        "<h1>{}</h1>\n<div class=\"notice\">\n<p>Angel could not extract readable text from this page \
         ({}).  It may need JavaScript, or it may be an index page.</p>\n\
         <p><a href=\"{}\" rel=\"noreferrer\">Open the original</a></p>\n</div>",
        esc(t),
        esc(why),
        esc(orig.as_str())
    );
    shell(t, &site_bar(orig, alt.as_ref()), &body)
}

/// The bar slot's contents: reload (with the age of a cached copy)
/// and add or remove this site.
/// `clear_href` forgets the site's cookies; `cookies` is how many
/// Angel holds for it (shown on hover).
pub fn bar_slot(
    reload_href: &str,
    clear_href: &str,
    cookies: usize,
    age: Option<&str>,
    orig: &Url,
    listed: bool,
) -> String {
    let age = age.map(|a| format!(" <span class=\"age\">cached {}</span>", esc(a))).unwrap_or_default();
    let (verb, label) = if listed { ("remove", "remove site") } else { ("add", "add site") };
    let held = match cookies {
        1 => "1 cookie".to_string(),
        n => format!("{n} cookies"),
    };
    format!(
        " <a class=\"reload\" href=\"{}\">reload</a>{age} \
         <a class=\"clear\" href=\"{}\" title=\"forget this site's cookies ({held})\">clear</a> \
         <a class=\"list\" href=\"/{verb}/{}\">{label}</a>",
        esc(reload_href),
        esc(clear_href),
        esc(orig.as_str())
    )
}

/// After `/clear`: what was forgotten, and the way back.
pub fn cleared(site: &str, n: usize, fixed: bool, back: &str, reload: &str) -> String {
    let what = match n {
        0 => format!("Angel held no cookies for {site}."),
        1 => format!("Cleared 1 cookie for {site}."),
        n => format!("Cleared {n} cookies for {site}."),
    };
    let fixed = if fixed {
        "<p class=\"muted\">The fixed cookies for this site in the site settings file are sent again \
         on the next fetch.</p>\n"
    } else {
        ""
    };
    let body = format!(
        "<h1>Cookies</h1>\n<p class=\"notice\">{}</p>\n{fixed}\
         <p class=\"muted\">Pages already in Angel's cache are not refetched until they expire or are reloaded.</p>\n\
         <p><a href=\"{}\">Reload the page</a> &middot; <a href=\"{}\">Back to the page</a></p>",
        esc(&what),
        esc(reload),
        esc(back)
    );
    shell("Cookies cleared", "", &body)
}

/// `/remove/<entry>` link for one list entry; `?`, `[` and friends
/// are escaped so the entry survives the trip through the path.
pub fn remove_href(entry: &str) -> String {
    let mut h = String::from("/remove/");
    for b in entry.bytes() {
        if b.is_ascii_alphanumeric() || b"-._*".contains(&b) {
            h.push(b as char);
        } else {
            h.push_str(&format!("%{b:02X}"));
        }
    }
    h
}

const LIST_NOTE: &str = "<p class=\"muted\">Each entry covers that host and every name under it \
    (<code>cnn.com</code> covers <code>www.cnn.com</code>); <code>*</code>, <code>?</code> and \
    <code>[...]</code> are wildcards.  In reader mode every page is cleaned up whether or not it \
    is listed; the list decides which sites proxy mode rewrites.</p>";

/// `/list`.
pub fn list(entries: &[String], path: &str) -> String {
    let mut items = String::new();
    for e in entries {
        // A plain name is a site to read; a pattern is not one place.
        let name = if sitelist::is_pattern(e) {
            format!("<code>{}</code>", esc(e))
        } else {
            format!("<code><a href=\"/read/https://{}/\">{}</a></code>", esc(e), esc(e))
        };
        items.push_str(&format!("<li>{name} <a href=\"{}\">remove</a></li>\n", esc(&remove_href(e))));
    }
    let items = if entries.is_empty() {
        "<p>The list is empty.</p>".to_string()
    } else {
        format!("<ul class=\"entries\">\n{items}</ul>")
    };
    let body = format!(
        "<h1>Site list</h1>\n{LIST_NOTE}\n\
         <form class=\"go\" action=\"/add\" method=\"get\">\n\
         <input type=\"text\" name=\"url\" placeholder=\"example.com or a page address\" \
         autocomplete=\"off\" autocapitalize=\"off\" spellcheck=\"false\">\n\
         <button type=\"submit\">Add</button>\n</form>\n{items}\n\
         <p class=\"muted\">{} {} in <code>{}</code> (<a href=\"/list?format=text\">as text</a>).</p>",
        entries.len(),
        if entries.len() == 1 { "entry" } else { "entries" },
        esc(path)
    );
    shell("Site list", LIST_BAR, &body)
}

const LIST_BAR: &str = " <a class=\"site\" href=\"/list\">site list</a>";

fn hidden(name: &str, value: &str) -> String {
    format!("<input type=\"hidden\" name=\"{name}\" value=\"{}\">", esc(value))
}

/// `GET /add/...`: the user confirms (and may edit) the entry.
/// `alt` is the narrower full host name, offered as a second choice.
pub fn add_confirm(entry: &str, alt: Option<&str>, covered: &[String], back: Option<&str>) -> String {
    let back_field = back.map(|b| hidden("back", b)).unwrap_or_default();
    let covered = if covered.is_empty() {
        String::new()
    } else {
        let c: Vec<String> = covered.iter().map(|e| format!("<code>{}</code>", esc(e))).collect();
        format!("<p class=\"notice\">Already covered by {}.</p>\n", c.join(", "))
    };
    let alt = alt
        .map(|a| {
            format!(
                "<form class=\"alt\" action=\"/add\" method=\"post\">{}{back_field}\
                 <span class=\"muted\">Or only this host:</span> <button type=\"submit\">Add {}</button></form>\n",
                hidden("entry", a),
                esc(a)
            )
        })
        .unwrap_or_default();
    let body = format!(
        "<h1>Add to the site list</h1>\n{covered}\
         <form class=\"go\" action=\"/add\" method=\"post\">\n\
         <input type=\"text\" name=\"entry\" value=\"{}\" autocomplete=\"off\" autocapitalize=\"off\" \
         spellcheck=\"false\">\n{back_field}\n<button type=\"submit\">Add</button>\n</form>\n{alt}{LIST_NOTE}",
        esc(entry)
    );
    shell_with("Add to the site list", LIST_BAR, &body, "same-origin")
}

/// `GET /remove/...`: the entries covering the host, to pick from.
pub fn remove_confirm(subject: &str, entries: &[String], back: Option<&str>) -> String {
    let body = if entries.is_empty() {
        format!(
            "<h1>Remove from the site list</h1>\n<p class=\"notice\">Nothing on the list covers \
             <code>{}</code>.</p>\n<p><a href=\"/list\">Show the list</a></p>",
            esc(subject)
        )
    } else {
        let check = if entries.len() == 1 { " checked" } else { "" };
        let boxes: String = entries
            .iter()
            .map(|e| {
                format!(
                    "<label><input type=\"checkbox\" name=\"entry\" value=\"{}\"{check}> <code>{}</code></label>\n",
                    esc(e),
                    esc(e)
                )
            })
            .collect();
        format!(
            "<h1>Remove from the site list</h1>\n<p>These entries cover <code>{}</code>:</p>\n\
             <form class=\"pick\" action=\"/remove\" method=\"post\">\n{boxes}{}\n\
             <button type=\"submit\">Remove</button>\n</form>",
            esc(subject),
            back.map(|b| hidden("back", b)).unwrap_or_default()
        )
    };
    shell_with("Remove from the site list", LIST_BAR, &body, "same-origin")
}

/// After a POST: what happened, and where to go next.
pub fn list_done(msg: &str, back: Option<&str>) -> String {
    let back = back
        .map(|b| format!("<p><a href=\"{}\">Back to the page</a></p>\n", esc(b)))
        .unwrap_or_default();
    let body = format!(
        "<h1>Site list</h1>\n<p class=\"notice\">{}</p>\n{back}<p><a href=\"/list\">Show the list</a></p>",
        esc(msg)
    );
    shell("Site list", LIST_BAR, &body)
}

/// Something went wrong; `orig` is the page we were trying to show.
pub fn error(status: u16, msg: &str, orig: Option<&Url>) -> String {
    let link = orig
        .map(|u| format!("<p><a href=\"{}\" rel=\"noreferrer\">Open the original</a></p>\n", esc(u.as_str())))
        .unwrap_or_default();
    let body = format!(
        "<h1>{}</h1>\n<div class=\"notice\">\n<p>{}</p>\n{link}</div>",
        status_text(status),
        esc(msg)
    );
    let bar = orig.map(|o| site_bar(o, None)).unwrap_or_default();
    shell(status_text(status), &bar, &body)
}

pub fn front() -> String {
    let body = "<h1>angel</h1>\n\
        <p class=\"muted\">Type or paste an address to read it without the clutter.</p>\n\
        <form class=\"go\" action=\"/read\" method=\"get\">\n\
        <input type=\"text\" name=\"url\" placeholder=\"example.com/some/article\" \
        autofocus autocomplete=\"off\" autocapitalize=\"off\" spellcheck=\"false\">\n\
        <button type=\"submit\">Read</button>\n</form>\n\
        <p class=\"muted\">Tip: put <code>/read/</code> in front of any address, as in \
        <code>/read/https://www.bbc.com/news</code>.</p>\n\
        <p><a href=\"/list\">Site list</a> &middot; <a href=\"/stats\">Statistics</a></p>";
    shell("angel", "", body)
}

/// `/stats`, around the body from `stats::body`.
pub fn stats(body: &str) -> String {
    shell("Statistics", " <a class=\"site\" href=\"/stats\">statistics</a>", body)
}

pub fn status_text(status: u16) -> &'static str {
    match status {
        400 => "Bad request",
        403 => "Forbidden",
        404 => "Not found",
        405 => "Method not allowed",
        413 => "Too large",
        502 => "Could not fetch the page",
        504 => "Timed out",
        s if s >= 500 => "Server error",
        _ => "Error",
    }
}
