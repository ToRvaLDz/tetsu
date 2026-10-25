//! Passphrase strength check: length, character classes and a small list
//! of passwords everybody tries first. No external dependency.

/// Shortest accepted passphrase.
pub const MIN_LEN: usize = 10;
/// From this length on, one or two character classes are enough.
const LONG_LEN: usize = 20;

const COMMON: &[&str] = &[
    "password",
    "passw0rd",
    "p@ssw0rd",
    "password1",
    "password123",
    "123456",
    "1234567",
    "12345678",
    "123456789",
    "1234567890",
    "12345678910",
    "0123456789",
    "qwerty",
    "qwertyuiop",
    "qwerty123",
    "azerty",
    "abc123",
    "abcdef",
    "abcdefgh",
    "iloveyou",
    "letmein",
    "welcome",
    "admin",
    "administrator",
    "changeme",
    "default",
    "master",
    "monkey",
    "dragon",
    "football",
    "baseball",
    "sunshine",
    "princess",
    "trustno1",
    "superman",
    "batman",
    "starwars",
    "hello123",
    "login",
    "secret",
    "linux",
    "archlinux",
    "tetsu",
    "diskpassword",
    "correcthorse",
    "000000",
    "111111",
    "123123",
    "654321",
    "666666",
    "696969",
    "121212",
    "zaq12wsx",
    "1q2w3e4r",
    "1qaz2wsx",
    "q1w2e3r4",
    "asdfgh",
    "asdfghjkl",
    "zxcvbnm",
    "passphrase",
];

fn classes(s: &str) -> usize {
    [
        s.chars().any(|c| c.is_lowercase()),
        s.chars().any(|c| c.is_uppercase()),
        s.chars().any(|c| c.is_ascii_digit()),
        s.chars().any(|c| !c.is_alphanumeric()),
    ]
    .into_iter()
    .filter(|b| *b)
    .count()
}

fn is_common(s: &str) -> bool {
    let lower = s.to_lowercase();
    let stem = lower.trim_end_matches(|c: char| c.is_ascii_digit() || c == '!');
    COMMON.contains(&lower.as_str()) || (!stem.is_empty() && COMMON.contains(&stem))
}

fn is_repetitive(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|first| chars.all(|c| c == first))
}

/// `Ok` when the passphrase is acceptable, else the reason.
pub fn check(s: &str) -> Result<(), String> {
    // The boot prompt cannot type a tab, an escape or a newline.
    if s.chars().any(char::is_control) {
        return Err("no control characters (tab, newline, escape): type plain characters".into());
    }
    let len = s.chars().count();
    if len < MIN_LEN {
        return Err(format!("too short: use at least {MIN_LEN} characters"));
    }
    if is_repetitive(s) || is_common(s) {
        return Err("too common: pick something less guessable".into());
    }
    let needed = if len >= LONG_LEN { 2 } else { 3 };
    if classes(s) < needed {
        return Err(format!(
            "too simple: mix at least {needed} of lowercase, uppercase, digits and symbols, \
             or use {LONG_LEN}+ characters"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_is_rejected() {
        assert!(check("Ab1!").unwrap_err().contains("too short"));
        assert!(check("Abcdef1!x").is_err());
    }

    #[test]
    fn control_characters_are_rejected() {
        for bad in [
            "Correct-Horse-9\tBattery",
            "Correct-Horse-9\nBattery",
            "Correct\u{1b}Horse-9-Battery",
        ] {
            assert!(check(bad).unwrap_err().contains("control"), "{bad:?}");
        }
    }

    #[test]
    fn common_passwords_are_rejected_even_with_decoration() {
        assert!(check("1234567890").unwrap_err().contains("common"));
        assert!(check("Password123").unwrap_err().contains("common"));
        assert!(check("aaaaaaaaaaaa").unwrap_err().contains("common"));
    }

    #[test]
    fn needs_three_classes_below_twenty_chars() {
        assert!(check("onlylowercase").unwrap_err().contains("too simple"));
        assert!(check("Lowercase123").is_ok());
        assert!(check("lower-and-digits9").is_ok());
    }

    #[test]
    fn long_phrase_with_two_classes_is_accepted() {
        assert!(check("blue elephants walk slowly").is_ok());
    }
}
