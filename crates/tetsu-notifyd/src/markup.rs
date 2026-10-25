//! freedesktop text as plain text. notifyd does not advertise `body-markup`,
//! yet many clients send it anyway: tags are dropped, entities decoded once,
//! and control characters removed, so nothing a client sends is ever
//! interpreted.

/// The longest entity accepted, `&#x10FFFF;` included.
const MAX_ENTITY: usize = 10;

/// Strips markup from freedesktop `summary` or `body` text.
pub fn plain_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(c) = rest.chars().next() {
        let consumed = match c {
            '<' => tag(rest).map(|(len, is_break)| {
                if is_break {
                    out.push('\n');
                }
                len
            }),
            '&' => entity(rest).map(|(len, decoded)| {
                push_visible(&mut out, decoded);
                len
            }),
            _ => None,
        };
        let len = consumed.unwrap_or_else(|| {
            push_visible(&mut out, c);
            c.len_utf8()
        });
        rest = &rest[len..];
    }
    out
}

fn push_visible(out: &mut String, c: char) {
    if !c.is_control() || c == '\n' || c == '\t' {
        out.push(c);
    }
}

/// A tag at the start of `s`: its length, and whether it is a line break.
/// A tag starts with a letter (or `/` and a letter) and ends at the first
/// `>` before any other `<`.
fn tag(s: &str) -> Option<(usize, bool)> {
    let inner = &s[1..];
    let name_start = inner.strip_prefix('/').unwrap_or(inner);
    if !name_start.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let end = inner
        .find(['<', '>'])
        .filter(|&i| inner[i..].starts_with('>'))?;
    let name: String = name_start
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    Some((end + 2, name.eq_ignore_ascii_case("br")))
}

/// An entity at the start of `s`: its length and the character it stands for.
fn entity(s: &str) -> Option<(usize, char)> {
    let end = s
        .char_indices()
        .take(MAX_ENTITY + 1)
        .find(|(_, c)| *c == ';')?
        .0;
    let name = &s[1..end];
    let decoded = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((end + 1, decoded))
}
