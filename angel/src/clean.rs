//! Pre-clean pass, run on the parsed page before readability and
//! headline extraction:
//!
//! - drop scripts (except JSON-LD), styles, forms, hidden elements
//! - drop consent banners, ad slots, newsletter/subscribe promos
//! - resolve lazy-load images and `<picture>`/`srcset` to one `src`
//! - use `<noscript>` image copies when the real image is missing
//! - drop tiny images and images from ad/tracker hosts
//! - turn `<video>`, `<audio>`, and `<iframe>` into plain links

use std::sync::OnceLock;

use dom_query::{Document, NodeRef};
use url::Url;

use crate::pages::esc;

pub fn preclean(doc: &Document, base: &Url) {
    doc.select("script:not([type=\"application/ld+json\"]), style, link, template, \
                form, button, input, select, textarea, dialog, object, embed, \
                [hidden]")
        .remove();
    remove_display_none(doc);
    remove_junk(doc);
    noscript_images(doc);
    lazy_images(doc, base);
    media_links(doc, base);
    drop_bad_images(doc, base);
}

fn remove_display_none(doc: &Document) {
    for n in doc.select("[style]").nodes() {
        let style: String = n
            .attr_or("style", "")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        if style.contains("display:none") || style.contains("visibility:hidden") {
            n.remove_from_parent();
        }
    }
}

/// id/class fragments that mark page furniture.  Matched as a
/// substring of each lowercased id or class token.
const JUNK_SUBSTR: &[&str] = &[
    "cookie-banner", "cookie-consent", "cookieconsent", "cookie-notice", "cookie_notice",
    "consent-banner", "consent_banner", "gdpr", "onetrust", "truste", "sp_message",
    "sp-message", "didomi", "qc-cmp", "usercentrics", "newsletter", "subscribe",
    "paywall-promo", "ad-slot", "adslot", "ad-container", "ad_container", "adcontainer",
    "advert", "ad-wrapper", "ad-unit", "adunit", "dfp-", "google-ad", "sponsored",
    "taboola", "outbrain", "social-share", "share-bar", "sharebar", "share-tools",
    "social-icons", "skip-link", "breaking-news-banner",
];

/// Whole-token matches (too short to match as substrings safely).
const JUNK_TOKEN: &[&str] = &["ad", "ads", "advertisement", "cmp", "consent", "share", "sharing"];

/// Called for thousands of elements per page, so: lowercase once, and
/// try at each byte only the fragments that start with it (none holds
/// whitespace, so searching the whole string equals searching each
/// token).
fn is_junk_name(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    if low.split_whitespace().any(|t| JUNK_TOKEN.contains(&t)) {
        return true;
    }
    static BY_FIRST: OnceLock<Vec<Vec<&'static [u8]>>> = OnceLock::new();
    let by_first = BY_FIRST.get_or_init(|| {
        let mut v = vec![Vec::new(); 256];
        for j in JUNK_SUBSTR {
            v[j.as_bytes()[0] as usize].push(j.as_bytes());
        }
        v
    });
    let b = low.as_bytes();
    (0..b.len()).any(|i| by_first[b[i] as usize].iter().any(|j| b[i..].starts_with(j)))
}

fn chars(n: &NodeRef) -> usize {
    n.text().chars().filter(|c| !c.is_whitespace()).count()
}

fn remove_junk(doc: &Document) {
    let total = doc.body().map(|b| chars(&b)).unwrap_or(0).max(1);
    let sel = doc.select("div[id], div[class], section[id], section[class], aside[id], aside[class], \
                          ins, p[class], ul[class], span[class]");
    for n in sel.nodes() {
        // An earlier removal may already have detached this node.
        if n.parent().is_none() {
            continue;
        }
        let name = format!("{} {}", n.attr_or("id", ""), n.attr_or("class", ""));
        if !is_junk_name(&name) {
            continue;
        }
        // Safety: never remove what is most of the page.
        if chars(n) * 10 > total * 4 {
            continue;
        }
        n.remove_from_parent();
    }
}

/// `<noscript>` holds what a non-JS browser would see.  Depending on
/// the parser's scripting flag its content is either elements or raw
/// text; handle both.  Keep it only when it holds an image that is not
/// already next to it.
fn noscript_images(doc: &Document) {
    for n in doc.select("noscript").nodes() {
        let has_el = n.descendants_it().any(|d| d.has_name("img"));
        let text = if has_el { String::new() } else { n.text().to_string() };
        if !has_el && !text.to_ascii_lowercase().contains("<img") {
            n.remove_from_parent();
            continue;
        }
        let sibling_img = n
            .prev_element_sibling()
            .is_some_and(|s| s.has_name("img") || s.has_name("picture"))
            && n.prev_element_sibling().and_then(|s| real_src_of(&s)).is_some();
        if sibling_img {
            n.remove_from_parent();
        } else if has_el {
            unwrap(n);
        } else {
            n.replace_with_html(text);
        }
    }
}

fn real_src_of(n: &NodeRef) -> Option<String> {
    let img = if n.has_name("img") {
        *n
    } else {
        n.descendants_it().find(|d| d.has_name("img"))?
    };
    img.attr("src").map(|s| s.to_string()).filter(|s| !is_placeholder(s))
}

/// Placeholder `src` values used while a lazy loader is pending.
fn is_placeholder(src: &str) -> bool {
    let s = src.trim().to_ascii_lowercase();
    s.is_empty()
        || s.starts_with("data:")
        || s.starts_with("about:")
        || s.contains("placeholder")
        || s.contains("blank.gif")
        || s.contains("spacer.gif")
        || s.contains("transparent.gif")
        || s.contains("lazy-load")
        || s.contains("1x1")
}

const LAZY_SRC: &[&str] = &[
    "data-src", "data-lazy-src", "data-original", "data-hi-res-src", "data-url",
    "data-lazy", "data-img-src", "data-image",
];
const LAZY_SRCSET: &[&str] = &["data-srcset", "data-lazy-srcset", "srcset"];

/// Pick one URL from a srcset: the widest candidate up to 1200w (or
/// up to 2x), else the narrowest above that.
pub fn pick_srcset(srcset: &str) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    let mut over: Option<(f64, String)> = None;
    for part in split_srcset(srcset) {
        let mut it = part.split_whitespace();
        let Some(url) = it.next() else { continue };
        if url.starts_with("data:") {
            continue;
        }
        let w = match it.next() {
            Some(d) if d.ends_with('w') => d[..d.len() - 1].parse::<f64>().unwrap_or(0.0),
            Some(d) if d.ends_with('x') => d[..d.len() - 1].parse::<f64>().unwrap_or(1.0) * 600.0,
            _ => 600.0,
        };
        let slot = if w <= 1200.0 { &mut best } else { &mut over };
        let better = match slot {
            None => true,
            Some((bw, _)) => {
                if w <= 1200.0 {
                    w > *bw
                } else {
                    w < *bw
                }
            }
        };
        if better {
            *slot = Some((w, url.to_string()));
        }
    }
    best.or(over).map(|(_, u)| u)
}

/// Split on the commas between candidates, not the commas inside
/// URLs (some CDNs put commas in the path).  A candidate boundary is
/// a comma followed by whitespace, or one following a descriptor.
fn split_srcset(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let b = s.as_bytes();
    for (i, c) in s.char_indices() {
        if c == ',' {
            let next_ws = b.get(i + 1).is_some_and(|n| n.is_ascii_whitespace());
            let after_desc = cur.trim_end().rsplit(' ').next().is_some_and(|t| {
                cur.trim().contains(' ') && (t.ends_with('w') || t.ends_with('x'))
            });
            if next_ws || after_desc {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn lazy_images(doc: &Document, base: &Url) {
    for img in doc.select("img").nodes() {
        let src = img.attr("src").map(|s| s.to_string());
        if src.as_deref().is_none_or(is_placeholder) {
            let mut found = LAZY_SRC.iter().find_map(|a| img.attr(a)).map(|s| s.to_string());
            if found.is_none() {
                found = LAZY_SRCSET.iter().find_map(|a| img.attr(a).and_then(|s| pick_srcset(&s)));
            }
            if found.is_none() {
                // <picture><source srcset=...><img></picture>
                if let Some(pic) = img.parent().filter(|p| p.has_name("picture")) {
                    found = pic
                        .children_it(false)
                        .filter(|c| c.has_name("source"))
                        .find_map(|s| s.attr("srcset").or_else(|| s.attr("data-srcset")))
                        .and_then(|s| pick_srcset(&s));
                }
            }
            match found {
                Some(f) => img.set_attr("src", &abs(base, &f)),
                None => {
                    img.remove_from_parent();
                    continue;
                }
            }
        }
        img.remove_attrs(&["srcset", "sizes", "loading", "decoding"]);
        img.remove_attrs(LAZY_SRC);
        img.remove_attrs(LAZY_SRCSET);
    }
    // Unwrap <picture>, leaving its <img>.
    for pic in doc.select("picture").nodes() {
        for s in pic.children_it(false).filter(|c| c.has_name("source")).collect::<Vec<_>>() {
            s.remove_from_parent();
        }
        unwrap(pic);
    }
}

/// Replace `n` with its children.  (dom_query's `unwrap_node` is
/// jQuery's `unwrap`: it removes the node's parent, so call it on the
/// first child.)
fn unwrap(n: &NodeRef) {
    match n.first_child() {
        Some(c) => c.unwrap_node(),
        None => n.remove_from_parent(),
    }
}

fn abs(base: &Url, u: &str) -> String {
    base.join(u.trim()).map(|u| u.to_string()).unwrap_or_else(|_| u.to_string())
}

/// Ad, analytics, and tracking-pixel hosts.  Matched against the
/// host and its parent domains.
const TRACKER_HOSTS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "google-analytics.com", "googletagmanager.com",
    "googleadservices.com", "adservice.google.com", "scorecardresearch.com", "quantserve.com",
    "chartbeat.com", "chartbeat.net", "amazon-adsystem.com", "adsrvr.org", "taboola.com",
    "outbrain.com", "criteo.com", "criteo.net", "pubmatic.com", "rubiconproject.com",
    "casalemedia.com", "moatads.com", "bat.bing.com", "facebook.net", "pixel.wp.com",
    "pixel.quantserve.com", "adnxs.com", "openx.net", "teads.tv", "zemanta.com",
    "parsely.com", "newrelic.com", "nr-data.net", "krxd.net", "bluekai.com",
];

pub fn is_tracker_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    TRACKER_HOSTS.iter().any(|t| h == *t || h.ends_with(&format!(".{t}")))
}

fn attr_px(n: &NodeRef, name: &str) -> Option<u32> {
    let v = n.attr(name)?;
    let digits: String = v.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn drop_bad_images(doc: &Document, base: &Url) {
    for img in doc.select("img").nodes() {
        let src = img.attr_or("src", "").to_string();
        let bad_host = base
            .join(&src)
            .ok()
            .and_then(|u| u.host_str().map(is_tracker_host))
            .unwrap_or(true);
        let (w, h) = (attr_px(img, "width"), attr_px(img, "height"));
        let tiny = match (w, h) {
            (Some(w), Some(h)) => (w < 64 && h < 64) || w <= 4 || h <= 4,
            (Some(x), None) | (None, Some(x)) => x <= 4,
            _ => false,
        };
        if bad_host || tiny {
            img.remove_from_parent();
        }
    }
}

/// Hosts whose pages are players or apps, not something to read;
/// links to them go direct instead of through /read/.
pub const DIRECT_HOSTS: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "player.vimeo.com",
    "tiktok.com", "instagram.com", "twitter.com", "x.com", "facebook.com", "spotify.com",
    "soundcloud.com", "podcasts.apple.com", "dailymotion.com", "rumble.com",
];

pub fn is_direct_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    DIRECT_HOSTS.iter().any(|t| h == *t || h.ends_with(&format!(".{t}")))
}

/// The page a viewer should open for an embedded player URL.
fn watch_url(u: &Url) -> Url {
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    if host.contains("youtube")
        && let Some(id) = u.path().strip_prefix("/embed/") {
            let id = id.split('/').next().unwrap_or(id);
            if let Ok(w) = Url::parse(&format!("https://www.youtube.com/watch?v={id}")) {
                return w;
            }
        }
    if host == "player.vimeo.com"
        && let Some(id) = u.path().strip_prefix("/video/")
            && let Ok(w) = Url::parse(&format!("https://vimeo.com/{id}")) {
                return w;
            }
    u.clone()
}

/// Marks the links made here, so the headlines pass can skip them
/// (the sanitizer drops the attribute).
pub const MEDIA_ATTR: &str = "data-angel-media";

fn media_link(href: &Url, label: &str, poster: Option<String>) -> String {
    let img = poster.map(|p| format!("<img src=\"{}\" alt=\"\"><br>", esc(&p))).unwrap_or_default();
    format!("<p><a href=\"{}\" {MEDIA_ATTR}=\"\">{img}{}</a></p>", esc(href.as_str()), esc(label))
}

fn media_links(doc: &Document, base: &Url) {
    for n in doc.select("iframe").nodes() {
        let src = n.attr("src").or_else(|| n.attr("data-src")).map(|s| s.to_string());
        let Some(u) = src.and_then(|s| base.join(s.trim()).ok()) else {
            n.remove_from_parent();
            continue;
        };
        let host = u.host_str().unwrap_or("").to_string();
        if (u.scheme() != "http" && u.scheme() != "https") || is_tracker_host(&host) {
            n.remove_from_parent();
            continue;
        }
        let title = n.attr("title").map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let label = if is_direct_host(&host) {
            format!("Video: {}", title.unwrap_or_else(|| host.clone()))
        } else {
            format!("Embedded content: {}", title.unwrap_or_else(|| host.clone()))
        };
        n.replace_with_html(media_link(&watch_url(&u), &label, None));
    }

    for (tag, what) in [("video", "Video"), ("audio", "Audio")] {
        for n in doc.select(tag).nodes() {
            let src = n
                .attr("src")
                .map(|s| s.to_string())
                .or_else(|| {
                    n.children_it(false)
                        .filter(|c| c.has_name("source"))
                        .find_map(|c| c.attr("src").map(|s| s.to_string()))
                })
                .and_then(|s| base.join(s.trim()).ok())
                .filter(|u| u.scheme() == "http" || u.scheme() == "https");
            match src {
                Some(u) => {
                    let poster = n.attr("poster").map(|p| abs(base, &p));
                    let title = n.attr("title").or_else(|| n.attr("aria-label")).map(|s| s.to_string());
                    let label = match title {
                        Some(t) if !t.trim().is_empty() => format!("{what}: {}", t.trim()),
                        _ => what.to_string(),
                    };
                    n.replace_with_html(media_link(&u, &label, poster));
                }
                None => n.remove_from_parent(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(html: &str) -> String {
        let doc = Document::from(html);
        preclean(&doc, &Url::parse("https://example.com/a/").unwrap());
        doc.select("body").inner_html().to_string().trim().to_string()
    }

    #[test]
    fn junk_names() {
        assert!(is_junk_name("main-col Ad"));
        assert!(is_junk_name("x Newsletter-Signup"));
        assert!(is_junk_name("wrap dfp-slot"));
        assert!(is_junk_name("onetrust-banner-sdk"));
        assert!(!is_junk_name("header shadow"), "'ad' only as a whole token");
        assert!(!is_junk_name("mw-content-text reflist"));
        assert!(!is_junk_name(""));
    }

    #[test]
    fn srcset() {
        assert_eq!(pick_srcset("a.jpg 320w, b.jpg 800w, c.jpg 1600w").unwrap(), "b.jpg");
        assert_eq!(pick_srcset("c.jpg 1600w, d.jpg 2400w").unwrap(), "c.jpg");
        assert_eq!(pick_srcset("a.jpg 1x, b.jpg 2x").unwrap(), "b.jpg");
        assert_eq!(pick_srcset("https://x/i/w_320,h_200/a.jpg 320w, https://x/i/w_800,h_500/a.jpg 800w").unwrap(),
                   "https://x/i/w_800,h_500/a.jpg");
        assert_eq!(pick_srcset("only.jpg").unwrap(), "only.jpg");
    }

    #[test]
    fn lazy() {
        assert_eq!(
            run("<img src=\"data:image/gif;base64,R0l\" data-src=\"/real.jpg\" alt=x>"),
            "<img src=\"https://example.com/real.jpg\" alt=\"x\">"
        );
        assert_eq!(
            run("<picture><source srcset=\"/p-400.webp 400w, /p-900.webp 900w\"><img alt=p></picture>"),
            "<img alt=\"p\" src=\"https://example.com/p-900.webp\">"
        );
        assert_eq!(run("<img alt=gone>"), "");
    }

    #[test]
    fn noscript() {
        assert_eq!(
            run("<div><noscript><img src=\"/n.jpg\"></noscript></div>"),
            "<div><img src=\"/n.jpg\"></div>"
        );
        assert_eq!(
            run("<div><img src=\"/a.jpg\"><noscript><img src=\"/a.jpg\"></noscript></div>"),
            "<div><img src=\"/a.jpg\"></div>"
        );
        assert_eq!(run("<p>x<noscript>Enable JavaScript</noscript></p>"), "<p>x</p>");
    }

    #[test]
    fn junk() {
        assert_eq!(
            run("<p>keep this text, it is the article and it is long enough to matter</p>\
                 <div class=\"newsletter-signup\">Sign up</div><div id=\"onetrust-banner\">ok</div>\
                 <aside class=\"ad\">buy</aside><div style=\"display: none\">hid</div>"),
            "<p>keep this text, it is the article and it is long enough to matter</p>"
        );
    }

    #[test]
    fn bad_images() {
        assert_eq!(run("<img src=\"https://ad.doubleclick.net/p.gif\">"), "");
        assert_eq!(run("<img src=\"/i.png\" width=1 height=1>"), "");
        assert_eq!(run("<img src=\"/i.png\" width=\"32\" height=\"32\">"), "");
        assert_eq!(run("<img src=\"/i.png\" width=\"640\">"), "<img src=\"/i.png\" width=\"640\">");
    }

    #[test]
    fn media() {
        assert_eq!(
            run("<iframe src=\"https://www.youtube.com/embed/abc123?rel=0\" title=\"Clip\"></iframe>"),
            "<p><a href=\"https://www.youtube.com/watch?v=abc123\" data-angel-media=\"\">Video: Clip</a></p>"
        );
        assert_eq!(
            run("<video poster=\"/p.jpg\"><source src=\"/v.mp4\"></video>"),
            "<p><a href=\"https://example.com/v.mp4\" data-angel-media=\"\"><img src=\"https://example.com/p.jpg\" alt=\"\"><br>Video</a></p>"
        );
        assert_eq!(run("<video></video><iframe></iframe>"), "");
    }
}
