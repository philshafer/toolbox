//! Readability extraction (dom_smoothie).

use url::Url;

/// Below this much article text we call extraction a failure and
/// show the fallback page instead.
pub const MIN_TEXT: usize = 200;

pub struct Article {
    pub title: String,
    pub byline: Option<String>,
    pub site_name: Option<String>,
    /// Article HTML, not yet sanitized.
    pub content: String,
    pub text_len: usize,
}

pub enum Outcome {
    Article(Article),
    /// Nothing readable; carries the page title if we found one.
    Failed { title: Option<String>, why: String },
}

/// Run readability over an already parsed (and pre-cleaned) page.
pub fn extract(doc: dom_query::Document, url: &Url) -> Outcome {
    let mut r = match dom_smoothie::Readability::with_document(doc, Some(url.as_str()), None) {
        Ok(r) => r,
        Err(e) => return Outcome::Failed { title: None, why: e.to_string() },
    };
    let a = match r.parse() {
        Ok(a) => a,
        Err(e) => return Outcome::Failed { title: None, why: e.to_string() },
    };

    let title = one_line(&a.title);
    let text_len = a.text_content.chars().filter(|c| !c.is_whitespace()).count();
    if text_len < MIN_TEXT {
        let title = if title.is_empty() { None } else { Some(title) };
        return Outcome::Failed { title, why: format!("only {text_len} characters of text found") };
    }

    Outcome::Article(Article {
        title,
        byline: a.byline.map(|s| one_line(&s)).filter(|s| !s.is_empty()),
        site_name: a.site_name.map(|s| one_line(&s)).filter(|s| !s.is_empty()),
        content: a.content.to_string(),
        text_len,
    })
}

pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
