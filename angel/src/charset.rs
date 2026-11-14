//! Decode an HTML body to a String.
//!
//! Order: byte-order mark, then the Content-Type charset, then a
//! `<meta>` charset in the first 1024 bytes, then UTF-8 if the bytes
//! are valid UTF-8, and finally a chardetng guess.

use encoding_rs::{Encoding, UTF_8};

pub fn decode(body: &[u8], content_type: &str) -> String {
    let enc = pick(body, content_type);
    let (text, _, _) = enc.decode(body);
    text.into_owned()
}

fn pick(body: &[u8], content_type: &str) -> &'static Encoding {
    if let Some((enc, _)) = Encoding::for_bom(body) {
        return enc;
    }
    if let Some(enc) = header_charset(content_type).and_then(label) {
        return enc;
    }
    if let Some(enc) = meta_charset(&body[..body.len().min(1024)]).and_then(label) {
        return enc;
    }
    if std::str::from_utf8(body).is_ok() {
        return UTF_8;
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(body, true);
    det.guess(None, chardetng::Utf8Detection::Allow)
}

/// Look up a label; a page cannot really be UTF-16 if we found an
/// ASCII-compatible label in it, so treat that as UTF-8 (per WHATWG).
fn label(s: String) -> Option<&'static Encoding> {
    let enc = Encoding::for_label(s.as_bytes())?;
    Some(enc.output_encoding())
}

fn header_charset(ct: &str) -> Option<String> {
    for part in ct.split(';').skip(1) {
        let part = part.trim();
        if part.len() > 8 && part[..8].eq_ignore_ascii_case("charset=") {
            return Some(part[8..].trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

/// Find `charset=` inside a `<meta` tag.  Covers both
/// `<meta charset="x">` and the http-equiv content form.
fn meta_charset(head: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(head).to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = s[from..].find("<meta") {
        let start = from + i;
        let end = s[start..].find('>').map(|j| start + j).unwrap_or(s.len());
        let tag = &s[start..end];
        if let Some(j) = tag.find("charset=") {
            let v = tag[j + 8..].trim_start_matches(['"', '\'', ' ']);
            let v: String = v
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || "-_:.".contains(*c))
                .collect();
            if !v.is_empty() {
                return Some(v);
            }
        }
        from = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_wins_over_guess() {
        let body = b"<p>caf\xe9</p>";
        assert_eq!(decode(body, "text/html; charset=ISO-8859-1"), "<p>caf\u{e9}</p>");
    }

    #[test]
    fn meta_forms() {
        assert_eq!(meta_charset(b"<meta charset=\"windows-1252\">").as_deref(), Some("windows-1252"));
        assert_eq!(
            meta_charset(b"<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=Shift_JIS\">")
                .as_deref(),
            Some("shift_jis")
        );
        assert_eq!(meta_charset(b"<meta name=x>"), None);
    }

    #[test]
    fn utf8_default() {
        assert_eq!(decode("<p>caf\u{e9}</p>".as_bytes(), "text/html"), "<p>caf\u{e9}</p>");
    }

    #[test]
    fn bom() {
        assert_eq!(decode(b"\xef\xbb\xbfhi", "text/html; charset=latin1"), "hi");
    }

    #[test]
    fn utf16_label_means_utf8() {
        let body = "<meta charset=utf-16><p>caf\u{e9}</p>".as_bytes();
        assert_eq!(decode(body, ""), "<meta charset=utf-16><p>caf\u{e9}</p>");
    }
}
