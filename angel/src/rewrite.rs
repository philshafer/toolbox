//! URL rewriting: every link goes back through `/read/`, every image
//! through `/img/`.
//!
//! Rewritten URLs are root-relative (`/read/https://...`) so they
//! work whatever host name the client used to reach us.

use url::Url;

use crate::clean;

/// Rewrite an `<a href>` found in a page whose URL is `base`.
/// Returns None to drop the attribute (javascript: and friends).
pub fn link(base: &Url, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if href.starts_with('#') {
        return Some(href.to_string());
    }
    let lower = href.get(..7).unwrap_or(href).to_ascii_lowercase();
    if lower.starts_with("mailto:") || lower.starts_with("tel:") {
        return Some(href.to_string());
    }

    let abs = base.join(href).ok()?;
    if abs.scheme() != "http" && abs.scheme() != "https" {
        return None;
    }

    // A link to a spot on this same page stays a bare fragment.
    if let Some(frag) = abs.fragment() {
        let mut a = abs.clone();
        a.set_fragment(None);
        let mut b = base.clone();
        b.set_fragment(None);
        if a == b {
            return Some(format!("#{frag}"));
        }
    }
    // Players and apps are useless through the reader; go direct.
    if abs.host_str().is_some_and(clean::is_direct_host) {
        return Some(abs.to_string());
    }
    Some(read_url(&abs))
}

/// Rewrite an `<img src>`.  Placeholders (data: URIs) are dropped.
pub fn image(base: &Url, src: &str) -> Option<String> {
    let abs = base.join(src.trim()).ok()?;
    if abs.scheme() != "http" && abs.scheme() != "https" {
        return None;
    }
    Some(format!("/img/{}", abs.as_str()))
}

/// `/read/<url>` for an absolute URL.
pub fn read_url(u: &Url) -> String {
    format!("/read/{}", u.as_str())
}

/// Force a rendering: `/article/<url>` or `/headlines/<url>`.
pub fn mode_url(mode: &str, u: &Url) -> String {
    format!("/{mode}/{}", u.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://www.example.com/news/story?id=7").unwrap()
    }

    #[test]
    fn links() {
        let b = base();
        assert_eq!(link(&b, "/a/b").unwrap(), "/read/https://www.example.com/a/b");
        assert_eq!(link(&b, "c?x=1&y=2").unwrap(), "/read/https://www.example.com/news/c?x=1&y=2");
        assert_eq!(link(&b, "//other.org/x").unwrap(), "/read/https://other.org/x");
        assert_eq!(link(&b, "http://other.org/x#s").unwrap(), "/read/http://other.org/x#s");
        assert_eq!(link(&b, "#top").unwrap(), "#top");
        assert_eq!(link(&b, "https://www.example.com/news/story?id=7#p3").unwrap(), "#p3");
        assert_eq!(link(&b, "mailto:a@b.c").unwrap(), "mailto:a@b.c");
        assert_eq!(link(&b, "javascript:void(0)"), None);
        assert_eq!(link(&b, "ftp://x.org/"), None);
        assert_eq!(link(&b, "https://www.youtube.com/watch?v=x").unwrap(), "https://www.youtube.com/watch?v=x");
        assert_eq!(link(&b, ""), None);
    }

    #[test]
    fn images() {
        let b = base();
        assert_eq!(image(&b, "/i/a.jpg").unwrap(), "/img/https://www.example.com/i/a.jpg");
        assert_eq!(image(&b, "data:image/gif;base64,R0lGOD"), None);
    }
}
