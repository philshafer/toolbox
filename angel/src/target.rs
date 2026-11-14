//! Rebuild the target URL from the request path.
//!
//! `/read/<url>` carries the target unencoded, so we work from the
//! raw request-target rather than a decoded path.  Forms we accept:
//!
//! - `/read/https://example.com/a?b=c`  (the usual case)
//! - `/read/https:/example.com/a`       (slashes collapsed by a client)
//! - `/read/example.com/a`              (no scheme: https assumed)
//! - `/read/https%3A%2F%2Fexample.com`  (fully percent-encoded)
//! - `/read?url=...`                    (the front-page form)

use percent_encoding::percent_decode_str;
use url::Url;

/// Parse the part of the request-target after `/read/`.
pub fn from_path(rest: &str) -> Result<Url, String> {
    let lower = rest.get(..8).unwrap_or(rest).to_ascii_lowercase();
    let s = if lower.starts_with("http%3a") || lower.starts_with("https%3a") {
        percent_decode_str(rest).decode_utf8_lossy().into_owned()
    } else {
        rest.to_string()
    };
    parse(&s)
}

/// Parse the `url` parameter of a `/read?url=...` query string.
pub fn from_query(query: &str) -> Result<Url, String> {
    let v = url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "url")
        .map(|(_, v)| v.into_owned())
        .ok_or("missing url parameter")?;
    parse(&v)
}

/// Parse a user-supplied URL string: fix collapsed slashes, default
/// the scheme to https, and allow only http(s) with a host.
pub fn parse(s: &str) -> Result<Url, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty URL".into());
    }

    let mut fixed = s.to_string();
    for scheme in ["https:", "http:"] {
        if fixed.len() > scheme.len() && fixed[..scheme.len()].eq_ignore_ascii_case(scheme) {
            let tail = fixed[scheme.len()..].trim_start_matches('/').to_string();
            fixed = format!("{scheme}//{tail}");
            break;
        }
    }
    if !fixed.contains("://") {
        fixed = format!("https://{fixed}");
    }

    let mut u = Url::parse(&fixed).map_err(|e| format!("bad URL: {e}"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err(format!("unsupported scheme: {}", u.scheme()));
    }
    match u.host_str() {
        Some(h) if !h.is_empty() => {}
        _ => return Err("URL has no host".into()),
    }
    u.set_fragment(None);
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        from_path(s).map(|u| u.to_string()).unwrap_or_else(|e| format!("ERR {e}"))
    }

    #[test]
    fn plain() {
        assert_eq!(p("https://example.com/a/b?c=d&e=f"), "https://example.com/a/b?c=d&e=f");
        assert_eq!(p("http://example.com"), "http://example.com/");
    }

    #[test]
    fn keeps_query() {
        assert_eq!(
            p("https://abcnews.com/US/story?id=137970527"),
            "https://abcnews.com/US/story?id=137970527"
        );
    }

    #[test]
    fn collapsed_slashes() {
        assert_eq!(p("https:/example.com/x"), "https://example.com/x");
        assert_eq!(p("HTTPS:///example.com/x"), "https://example.com/x");
    }

    #[test]
    fn no_scheme() {
        assert_eq!(p("example.com/x"), "https://example.com/x");
    }

    #[test]
    fn encoded() {
        assert_eq!(p("https%3A%2F%2Fexample.com%2Fa%3Fb%3Dc"), "https://example.com/a?b=c");
    }

    #[test]
    fn drops_fragment() {
        assert_eq!(p("https://example.com/a#top"), "https://example.com/a");
    }

    #[test]
    fn rejects() {
        assert!(from_path("").is_err());
        assert!(from_path("ftp://example.com/").is_err());
        assert!(from_path("javascript:alert(1)").is_err());
        assert!(from_path("file:///etc/passwd").is_err());
    }

    #[test]
    fn query_form() {
        let u = from_query("url=https%3A%2F%2Fexample.com%2Fa%3Fb%3Dc").unwrap();
        assert_eq!(u.as_str(), "https://example.com/a?b=c");
        let u = from_query("url=bbc.com").unwrap();
        assert_eq!(u.as_str(), "https://bbc.com/");
        assert!(from_query("x=1").is_err());
    }
}
