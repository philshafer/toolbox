//! Embedded-data fallbacks for pages where readability comes up
//! short: JSON-LD `articleBody` and the AMP alternate.  Gathered from
//! the page before the pre-clean pass removes scripts and links.

use dom_query::Document;
use serde_json::Value;
use url::Url;

use crate::pages::esc;

#[derive(Default)]
pub struct Embedded {
    /// Longest JSON-LD articleBody, plain text.
    pub article_body: Option<String>,
    /// `<link rel="amphtml">` target.
    pub amp: Option<Url>,
}

pub fn gather(doc: &Document, base: &Url) -> Embedded {
    let mut e = Embedded::default();

    for n in doc.select("script[type=\"application/ld+json\"]").nodes() {
        let text = n.text();
        let Ok(v) = serde_json::from_str::<Value>(text.trim()) else { continue };
        let mut best = e.article_body.take();
        walk(&v, &mut best);
        e.article_body = best;
    }

    e.amp = doc
        .select("link[rel=\"amphtml\"]")
        .attr("href")
        .and_then(|h| base.join(h.trim()).ok())
        .filter(|u| u.scheme() == "http" || u.scheme() == "https");
    e
}

fn walk(v: &Value, best: &mut Option<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                if k == "articleBody" {
                    if let Value::String(s) = v
                        && best.as_ref().is_none_or(|b| s.len() > b.len()) {
                            *best = Some(s.clone());
                        }
                } else {
                    walk(v, best);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|v| walk(v, best)),
        _ => {}
    }
}

/// Turn a plain-text body into paragraphs.  Bodies use blank lines,
/// single newlines, or nothing at all between paragraphs.
pub fn body_html(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let sep = if text.contains("\n\n") { "\n\n" } else { "\n" };
    let mut out = String::new();
    for p in text.split(sep) {
        let p = p.split_whitespace().collect::<Vec<_>>().join(" ");
        if !p.is_empty() {
            out.push_str("<p>");
            out.push_str(&esc(&p));
            out.push_str("</p>\n");
        }
    }
    out
}

pub fn text_len(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_ld() {
        let html = r#"<html><head>
            <script type="application/ld+json">{"@type":"WebPage","name":"x"}</script>
            <script type="application/ld+json">[{"@graph":[{"@type":"NewsArticle",
              "articleBody":"First para.\n\nSecond <para>."}]}]</script>
            <link rel="amphtml" href="/amp/story">
            </head><body></body></html>"#;
        let base = Url::parse("https://example.com/story").unwrap();
        let e = gather(&Document::from(html), &base);
        assert_eq!(e.article_body.as_deref(), Some("First para.\n\nSecond <para>."));
        assert_eq!(e.amp.unwrap().as_str(), "https://example.com/amp/story");
        assert_eq!(body_html("First para.\n\nSecond <para>."),
                   "<p>First para.</p>\n<p>Second &lt;para&gt;.</p>\n");
    }
}
