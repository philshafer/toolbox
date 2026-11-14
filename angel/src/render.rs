//! Turn a fetched HTML body into the page we serve.  Shared by the
//! server and the `--render` command used for fixture tests.
//!
//! Flow: decode, parse, gather embedded data (JSON-LD, AMP link),
//! pre-clean, collect headlines, run readability, then choose:
//!
//! - headlines, when asked for or when the page looks like an index
//! - the readability article
//! - JSON-LD articleBody, when readability is short or much shorter
//! - the AMP alternate, when there is still nothing
//! - the fallback page

use std::fs;
use std::process::ExitCode;
use std::time::Instant;

use dom_query::Document;
use url::Url;

use crate::extract::{self, MIN_TEXT, Outcome};
use crate::pages::{self, Alt, ArticleView};
use crate::{charset, clean, embedded, index, rewrite, sanitize};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Auto,
    Article,
    Headlines,
}

pub struct Rendered {
    pub html: String,
    /// What we showed: article, jsonld, amp, headlines, fallback.
    pub kind: &'static str,
    /// Measurements behind the choice, for the log.
    pub info: String,
}

/// Fetch the AMP alternate: (body, content type, final URL).
pub type AmpFetch<'a> = &'a dyn Fn(&Url) -> Option<(Vec<u8>, String, Url)>;

/// Fewest headlines for the headlines view to be worth offering.
const MIN_HEADS: usize = 8;

struct Parsed {
    title: Option<String>,
    site: Option<String>,
    /// og:image, used as the lead image when the article has none.
    lead: Option<String>,
    emb: embedded::Embedded,
    ix: index::Index,
    art: Outcome,
}

fn parse(html: &str, url: &Url) -> Parsed {
    let doc = Document::from(html);
    let title = doc.select("head title").first().text().to_string();
    let title = Some(extract::one_line(&title)).filter(|t| !t.is_empty());
    let site = doc
        .select("meta[property=\"og:site_name\"]")
        .attr("content")
        .map(|s| extract::one_line(&s))
        .filter(|s| !s.is_empty());
    let lead = doc
        .select("meta[property=\"og:image\"]")
        .attr("content")
        .and_then(|s| url.join(s.trim()).ok())
        .filter(|u| u.host_str().is_some_and(|h| !clean::is_tracker_host(h)))
        .map(|u| u.to_string());
    let emb = embedded::gather(&doc, url);
    clean::preclean(&doc, url);
    let ix = index::headlines(&doc, url);
    let art = extract::extract(doc, url);
    Parsed { title, site, lead, emb, ix, art }
}

pub fn render(body: &[u8], content_type: &str, url: &Url, mode: Mode, amp: Option<AmpFetch>) -> Rendered {
    let html = charset::decode(body, content_type);
    let p = parse(&html, url);

    let art_len = match &p.art {
        Outcome::Article(a) => a.text_len,
        Outcome::Failed { .. } => 0,
    };
    let root = is_front(url);
    let link_ratio = p.ix.link_chars as f64 / p.ix.body_chars.max(1) as f64;
    // Article pages carry big related-story rails, so off the front
    // page it takes a failed extraction, or headlines that swamp the
    // article on a page that is mostly links.
    let looks_index = p.ix.count >= MIN_HEADS
        && (root || art_len < MIN_TEXT || (link_ratio >= 0.7 && p.ix.head_chars > 2 * art_len));
    let info = format!(
        "art={art_len} heads={} headchars={} body={} links={:.2} root={root} jsonld={} amp={}",
        p.ix.count,
        p.ix.head_chars,
        p.ix.body_chars,
        link_ratio,
        p.emb.article_body.as_deref().map(embedded::text_len).unwrap_or(0),
        p.emb.amp.is_some(),
    );

    let want_heads = match mode {
        Mode::Headlines => p.ix.count > 0,
        Mode::Article => false,
        Mode::Auto => looks_index,
    };
    let mut r = if want_heads {
        headlines_page(&p, url)
    } else {
        article_chain(p, url, amp)
    };
    r.info = format!("{} {info}", r.kind);
    r
}

/// A home or section front: `/`, or one short word-like segment
/// (`/us`, `/news`, `/world-news`), with no query.
fn is_front(url: &Url) -> bool {
    if url.query().is_some() {
        return false;
    }
    let segs: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [] => true,
        [s] => {
            s.len() <= 24
                && s.matches('-').count() <= 2
                && s.chars().all(|c| c.is_ascii_alphabetic() || c == '-' || c == '_')
        }
        _ => false,
    }
}

fn headlines_page(p: &Parsed, url: &Url) -> Rendered {
    let mut list = String::new();
    let page_key = index::site_key(url.host_str().unwrap_or(""));
    for s in &p.ix.sections {
        if let Some(n) = &s.name {
            list.push_str(&format!("<h2>{}</h2>\n", pages::esc(n)));
        }
        list.push_str("<ul>\n");
        for h in &s.heads {
            let host = h.url.host_str().unwrap_or("");
            let tag = if index::site_key(host) != page_key {
                format!(" <span class=\"host\">{}</span>", pages::esc(host.trim_start_matches("www.")))
            } else {
                String::new()
            };
            list.push_str(&format!(
                "<li><a href=\"{}\">{}</a>{tag}</li>\n",
                pages::esc(h.url.as_str()),
                pages::esc(&h.text)
            ));
        }
        list.push_str("</ul>\n");
    }
    let content = sanitize::clean(&list, url);
    let title = p.title.as_deref().or(p.site.as_deref()).unwrap_or(url.host_str().unwrap_or(""));
    let alt = Alt { label: "view as article", href: rewrite::mode_url("article", url) };
    Rendered { html: pages::headlines(url, title, &content, Some(alt)), kind: "headlines", info: String::new() }
}

/// Put the page's og:image in front when the article has no image of
/// its own (lead photos often sit in a hero block outside the text).
fn with_lead(content: &str, lead: Option<&str>) -> String {
    match lead {
        Some(src) if !content.contains("<img") => {
            format!("<figure><img src=\"{}\" alt=\"\"></figure>\n{content}", pages::esc(src))
        }
        _ => content.to_string(),
    }
}

fn heads_alt(p: &Parsed, url: &Url) -> Option<Alt> {
    (p.ix.count >= MIN_HEADS)
        .then(|| Alt { label: "view as headlines", href: rewrite::mode_url("headlines", url) })
}

fn article_chain(p: Parsed, url: &Url, amp: Option<AmpFetch>) -> Rendered {
    let alt = heads_alt(&p, url);
    let jsonld = p.emb.article_body.as_deref().filter(|b| embedded::text_len(b) >= MIN_TEXT);

    if let Outcome::Article(a) = &p.art
        && jsonld.is_none_or(|b| embedded::text_len(b) < a.text_len * 2) {
            let content = sanitize::clean(&with_lead(&a.content, p.lead.as_deref()), url);
            let html = pages::article(&ArticleView {
                orig: url,
                title: &a.title,
                byline: a.byline.as_deref(),
                site: a.site_name.as_deref(),
                content: &content,
                source_note: None,
                alt,
            });
            return Rendered { html, kind: "article", info: String::new() };
        }

    let (title, byline, site) = match &p.art {
        Outcome::Article(a) => (Some(a.title.clone()), a.byline.clone(), a.site_name.clone()),
        Outcome::Failed { title, .. } => (title.clone(), None, None),
    };
    let title = title.or(p.title.clone()).unwrap_or_else(|| url.host_str().unwrap_or("").to_string());

    if let Some(body) = jsonld {
        let content = sanitize::clean(&with_lead(&embedded::body_html(body), p.lead.as_deref()), url);
        let html = pages::article(&ArticleView {
            orig: url,
            title: &title,
            byline: byline.as_deref(),
            site: site.as_deref().or(p.site.as_deref()),
            content: &content,
            source_note: Some("Text from the page's embedded article data; inline images and links are not included."),
            alt,
        });
        return Rendered { html, kind: "jsonld", info: String::new() };
    }

    if let (Some(amp_url), Some(fetch)) = (&p.emb.amp, amp)
        && let Some((body, ct, final_url)) = fetch(amp_url) {
            let html = charset::decode(&body, &ct);
            let q = parse(&html, &final_url);
            if let Outcome::Article(a) = &q.art {
                let content = sanitize::clean(&with_lead(&a.content, q.lead.as_deref().or(p.lead.as_deref())), &final_url);
                let html = pages::article(&ArticleView {
                    orig: url,
                    title: &a.title,
                    byline: a.byline.as_deref(),
                    site: a.site_name.as_deref(),
                    content: &content,
                    source_note: Some("Text from the page's AMP version."),
                    alt,
                });
                return Rendered { html, kind: "amp", info: String::new() };
            }
        }

    let why = match &p.art {
        Outcome::Failed { why, .. } => why.clone(),
        Outcome::Article(_) => "no usable text".into(),
    };
    Rendered {
        html: pages::fallback(url, Some(&title), &why, alt),
        kind: "fallback",
        info: String::new(),
    }
}

/// `angel --render FILE [--url URL] [--mode M] [--explain] [--bench N]`.
/// Fixture convention: foo.html sits next to foo.url (used when --url
/// is absent) and foo.headers (supplies the Content-Type, for the
/// charset).  No network access: the AMP fallback is skipped.
/// `bench`: render N more times and print timings to stderr instead
/// of the page.
pub fn render_file(file: &str, url: Option<&str>, mode: Mode, explain: bool, bench: usize) -> ExitCode {
    let body = match fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("angel: {file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let stem = file.strip_suffix(".html").unwrap_or(file);
    let url = match url {
        Some(u) => u.to_string(),
        None => {
            let side = format!("{stem}.url");
            match fs::read_to_string(&side) {
                Ok(u) => u.trim().to_string(),
                Err(e) => {
                    eprintln!("angel: need --url ({side}: {e})");
                    return ExitCode::from(2);
                }
            }
        }
    };
    let url = match Url::parse(&url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("angel: bad --url: {e}");
            return ExitCode::from(2);
        }
    };
    let ct = header_content_type(&format!("{stem}.headers")).unwrap_or_default();
    let r = render(&body, &ct, &url, mode, None);
    if bench > 0 {
        let mut ms: Vec<f64> = (0..bench)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(render(&body, &ct, &url, mode, None));
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        let at = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
        eprintln!(
            "{file}: {} KB {} min {:.1} median {:.1} p95 {:.1} max {:.1} ms",
            body.len() / 1024,
            r.kind,
            ms[0],
            at(0.5),
            at(0.95),
            ms[ms.len() - 1]
        );
        return ExitCode::SUCCESS;
    }
    if explain {
        eprintln!("{file}: {}", r.info);
    }
    print!("{}", r.html.replace(pages::BAR_SLOT, ""));
    ExitCode::SUCCESS
}

/// Last Content-Type in a saved `curl -D` headers file (the last one
/// is the final response after redirects).
fn header_content_type(path: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.trim().to_string())
        .next()
}
