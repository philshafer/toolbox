//! Headlines ("index") rendering for front pages and aggregators.
//!
//! Collect links whose text reads like a headline, skip navigation
//! and footers, drop duplicates, and group them under the nearest
//! section heading.  Runs on the pre-cleaned page.

use std::collections::HashMap;

use dom_query::{Document, NodeRef};
use url::Url;

use crate::{clean, sitelist};

pub struct Headline {
    pub url: Url,
    pub text: String,
}

pub struct Section {
    pub name: Option<String>,
    pub heads: Vec<Headline>,
}

pub struct Index {
    pub sections: Vec<Section>,
    pub count: usize,
    /// Non-space characters of headline text.
    pub head_chars: usize,
    /// Non-space characters in the page body and inside its links.
    pub body_chars: usize,
    pub link_chars: usize,
}

const MIN_CHARS: usize = 20;
const MIN_WORDS: usize = 4;
const MAX_HEADS: usize = 400;

/// Link text that is page furniture, not a story.
const JUNK_TEXT: &[&str] = &[
    "sign in", "sign up", "log in", "subscribe", "privacy", "terms of", "cookie",
    "advertis", "contact us", "newsletter", "read more", "show more", "load more",
    "see all", "see more", "view all", "skip to", "download the", "manage preferences",
    "do not sell", "your privacy choices", "all rights reserved",
];

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn nchars(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

fn is_headline_text(t: &str) -> bool {
    if t.len() < MIN_CHARS || t.split_whitespace().count() < MIN_WORDS || t.len() > 300 {
        return false;
    }
    let l = t.to_ascii_lowercase();
    !JUNK_TEXT.iter().any(|j| l.starts_with(j) || (l.len() < 40 && l.contains(j)))
}

/// Registrable domain, used to tell on-site links from off-site ones.
pub fn site_key(host: &str) -> String {
    sitelist::registrable(host)
}

/// Navigation-ish containers, by element name, role, or class/id.
/// `<nav>` and menus count anywhere; headers and footers only outside
/// `<main>`/`<article>` (inside, they belong to a story or card).
fn in_furniture(a: &NodeRef) -> bool {
    let mut soft = false;
    for anc in a.ancestors_it(None) {
        if !anc.is_element() {
            continue;
        }
        if anc.has_name("nav") {
            return true;
        }
        if anc.has_name("article") || anc.has_name("main") {
            return false;
        }
        if anc.has_name("header") || anc.has_name("footer") {
            soft = true;
        }
        if let Some(role) = anc.attr("role") {
            let r = role.to_ascii_lowercase();
            if r == "navigation" || r == "menu" || r == "menubar" {
                return true;
            }
            if r == "contentinfo" || r == "banner" {
                soft = true;
            }
        }
        let name = format!("{} {}", anc.attr_or("id", ""), anc.attr_or("class", "")).to_ascii_lowercase();
        for t in name.split_whitespace() {
            if t.contains("menu") || t.contains("navbar") || t.contains("nav-") || t == "nav" || t.contains("breadcrumb") {
                return true;
            }
            if t.contains("footer") || t.contains("masthead") {
                soft = true;
            }
        }
    }
    soft
}

/// Text with a space wherever an element starts or ends, so a kicker
/// in its own span does not run into the headline ("ExplainedWhat").
/// Inline formatting (em, b, ...) is left alone.
fn spaced_text(n: &NodeRef) -> String {
    fn walk(n: &NodeRef, out: &mut String) {
        for c in n.children_it(false) {
            if c.is_text() {
                out.push_str(&c.text());
            } else if c.is_element() {
                let inline = ["em", "i", "b", "strong", "u", "sup", "sub", "small", "mark", "abbr", "q"]
                    .iter()
                    .any(|t| c.has_name(t));
                let sep = if inline { "" } else { " " };
                out.push_str(sep);
                walk(&c, out);
                out.push_str(sep);
            }
        }
    }
    let mut out = String::new();
    walk(n, &mut out);
    one_line(&out)
}

/// The headline for a link: its text, or its `aria-label` when the
/// text is empty (card overlay links) or is a kicker run into the
/// label ("Pete HegsethHegseth calls ...").
fn link_text(a: &NodeRef) -> String {
    let text = spaced_text(a);
    let label = a.attr("aria-label").map(|l| one_line(&l)).unwrap_or_default();
    if !label.is_empty() && is_headline_text(&label) && (!is_headline_text(&text) || text.ends_with(&label)) {
        label
    } else {
        text
    }
}

fn story_url(page: &Url, href: &str) -> Option<Url> {
    let mut u = page.join(href.trim()).ok()?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    u.set_fragment(None);
    let host = u.host_str()?.to_string();
    if clean::is_tracker_host(&host) {
        return None;
    }
    let path = u.path().to_ascii_lowercase();
    if path.contains("/share") || path.contains("/intent/") || path.contains("/sharer") {
        return None;
    }
    let same = site_key(&host) == site_key(page.host_str().unwrap_or(""));
    let mut p = page.clone();
    p.set_fragment(None);
    if u == p {
        return None;
    }
    if same {
        if path == "/" || path.is_empty() {
            return None;
        }
    } else {
        // Off-site links count only when they look like a story, not
        // a home page or a profile.
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let storyish = segs.len() >= 2 || segs.iter().any(|s| s.len() >= 20) || u.query().is_some();
        if !storyish {
            return None;
        }
    }
    Some(u)
}

/// A heading that names a section rather than being a headline.
fn section_name(h: &NodeRef) -> Option<String> {
    let has_story_link = h
        .descendants_it()
        .any(|d| d.has_name("a") && is_headline_text(&one_line(&d.text())));
    if has_story_link {
        return None;
    }
    let t = one_line(&h.text());
    if t.is_empty() || t.len() > 60 {
        return None;
    }
    Some(t)
}

pub fn headlines(doc: &Document, page: &Url) -> Index {
    let body = match doc.body() {
        Some(b) => b,
        None => doc.root(),
    };
    let body_chars = nchars(&body.text());
    let mut link_chars = 0;

    let mut order: Vec<(Option<String>, Headline)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut section: Option<String> = None;

    for n in body.descendants_it() {
        if !n.is_element() {
            continue;
        }
        let name = n.node_name().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        match name.as_str() {
            "h1" | "h2" | "h3" => {
                if let Some(s) = section_name(&n)
                    && !in_furniture(&n) {
                        section = Some(s);
                    }
            }
            "a" => {
                link_chars += nchars(&n.text());
                if n.has_attr(clean::MEDIA_ATTR) {
                    continue;
                }
                let text = link_text(&n);
                if !is_headline_text(&text) || order.len() >= MAX_HEADS {
                    continue;
                }
                let Some(href) = n.attr("href") else { continue };
                let Some(url) = story_url(page, &href) else { continue };
                if in_furniture(&n) {
                    continue;
                }
                let key = url.as_str().to_string();
                if let Some(&i) = seen.get(&key) {
                    if text.len() > order[i].1.text.len() {
                        order[i].1.text = text;
                    }
                    continue;
                }
                seen.insert(key, order.len());
                order.push((section.clone(), Headline { url, text }));
            }
            _ => {}
        }
    }

    // Group by section, keeping first-appearance order of sections.
    let mut sections: Vec<Section> = Vec::new();
    let mut idx: HashMap<Option<String>, usize> = HashMap::new();
    let count = order.len();
    let head_chars = order.iter().map(|(_, h)| nchars(&h.text)).sum();
    for (s, h) in order {
        let i = *idx.entry(s.clone()).or_insert_with(|| {
            sections.push(Section { name: s, heads: Vec::new() });
            sections.len() - 1
        });
        sections[i].heads.push(h);
    }

    Index { sections, count, head_chars, body_chars, link_chars }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_texts() {
        let doc = Document::from(
            "<a id=a href=/x><span>Explained</span>What to know about it all</a>\
             <a id=b href=/y aria-label=\"Senate passes the big bill tonight\"></a>\
             <a id=c href=/z>Make<em>s</em> sense</a>",
        );
        let t = |id: &str| link_text(&doc.select(&format!("#{id}")).nodes()[0]);
        assert_eq!(t("a"), "Explained What to know about it all");
        assert_eq!(t("b"), "Senate passes the big bill tonight");
        assert_eq!(t("c"), "Makes sense");
    }

    #[test]
    fn keys() {
        assert_eq!(site_key("www.bbc.com"), "bbc.com");
        assert_eq!(site_key("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_key("abcnews.com"), "abcnews.com");
        assert_eq!(site_key("en.m.wikipedia.org"), "wikipedia.org");
    }

    #[test]
    fn collects() {
        let html = r#"<body>
          <nav><a href="/politics/long-nav-link-that-reads-like-a-headline">Politics section is long enough here</a></nav>
          <h2>Top stories</h2>
          <h3><a href="/2026/11/10/one">Senate passes the big bill after long night</a></h3>
          <a href="/2026/11/10/one">Senate passes the big bill after a long, long night</a>
          <a href="/about">About us</a>
          <h2><a href="/world">World</a></h2>
          <a href="https://other.org/2026/story-about-things">Other site reports on a thing happening</a>
          <a href="https://other.org/">Other site home page is not a story</a>
          <footer><a href="/terms/a-very-long-terms-page">Terms of service and other legal things</a></footer>
        </body>"#;
        let page = Url::parse("https://www.example.com/").unwrap();
        let ix = headlines(&Document::from(html), &page);
        assert_eq!(ix.count, 2);
        assert_eq!(ix.sections[0].name.as_deref(), Some("Top stories"));
        assert_eq!(ix.sections[0].heads[0].text, "Senate passes the big bill after a long, long night");
        assert_eq!(ix.sections[1].name.as_deref(), Some("World"));
        assert_eq!(ix.sections[1].heads[0].url.as_str(), "https://other.org/2026/story-about-things");
    }
}

