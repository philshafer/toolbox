//! Sanitize extracted article HTML with ammonia, rewriting links and
//! images on the way through.

use std::borrow::Cow;

use url::Url;

use crate::rewrite;

pub fn clean(html: &str, base: &Url) -> String {
    let base = base.clone();
    let mut b = ammonia::Builder::default();
    b.add_tags(&["figure", "figcaption", "picture", "section", "article", "time"])
        .add_tag_attributes("img", &["src", "alt", "title"])
        .rm_tag_attributes("img", &["width", "height", "align"])
        .add_tag_attributes("a", &["id"])
        .add_generic_attributes(&["id"])
        .add_allowed_classes("span", &["host"])
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(move |el, attr, value| -> Option<Cow<'_, str>> {
            match (el, attr) {
                ("a", "href") => rewrite::link(&base, value).map(Cow::Owned),
                ("img", "src") => rewrite::image(&base, value).map(Cow::Owned),
                _ => Some(Cow::Borrowed(value)),
            }
        });
    b.clean(html).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_and_strips() {
        let base = Url::parse("https://example.com/a/").unwrap();
        let out = clean(
            "<p onclick=x>Hi <a href=\"b?x=1&amp;y=2\">b</a> \
             <a href=\"javascript:alert(1)\">j</a>\
             <img src=\"/i.png\" width=900 alt=pic>\
             <script>bad()</script><iframe src=\"//e\"></iframe></p>",
            &base,
        );
        assert_eq!(
            out,
            "<p>Hi <a href=\"/read/https://example.com/a/b?x=1&amp;y=2\">b</a> \
             <a>j</a><img src=\"/img/https://example.com/i.png\" alt=\"pic\"></p>"
        );
    }
}
