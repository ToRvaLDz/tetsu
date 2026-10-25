//! freedesktop text is shown as plain text: no `body-markup` (#23).

use tetsu_notifyd::markup::plain_text;

#[test]
fn tags_are_removed() {
    assert_eq!(plain_text("<b>bold</b> and <i>it</i>"), "bold and it");
    assert_eq!(plain_text(r#"<a href="https://x.test">link</a>"#), "link");
    assert_eq!(plain_text("<img src=\"x.png\" alt=\"pic\"/>after"), "after");
    assert_eq!(plain_text("line<br/>next"), "line\nnext");
}

#[test]
fn entities_are_decoded_once() {
    assert_eq!(
        plain_text("a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;"),
        "a & b <c> \"d\" 'e'"
    );
    assert_eq!(plain_text("&#65;&#x42;"), "AB");
    assert_eq!(plain_text("&amp;lt;b&amp;gt;"), "&lt;b&gt;");
}

#[test]
fn text_that_only_looks_like_markup_stays() {
    assert_eq!(plain_text("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
    assert_eq!(plain_text("x <= y"), "x <= y");
    assert_eq!(
        plain_text("unknown &nbsp entity & alone"),
        "unknown &nbsp entity & alone"
    );
    assert_eq!(plain_text("<unterminated"), "<unterminated");
}

#[test]
fn decoded_entities_never_become_tags() {
    assert_eq!(plain_text("&lt;b&gt;bold&lt;/b&gt;"), "<b>bold</b>");
}

#[test]
fn control_characters_are_dropped_except_newline_and_tab() {
    assert_eq!(plain_text("a\u{1b}[31mb\tc\nd\u{7}"), "a[31mb\tc\nd");
    assert_eq!(plain_text("&#27;x&#0;"), "x");
}
