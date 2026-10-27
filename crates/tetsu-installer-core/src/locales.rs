//! Locales beyond the ones the payload pregenerates.
//!
//! The payload carries the locales of the keyboard table. Any other
//! `ll_CC.UTF-8` the owner names is checked against glibc's
//! `usr/share/i18n/SUPPORTED` twice: at answer time against the medium's,
//! and at install time against the target's own, which is the one whose
//! `locale-gen` runs.

use std::collections::BTreeSet;
use std::path::Path;

use crate::keyboard;

/// glibc's list of supported locales, relative to a root.
pub const SUPPORTED: &str = "usr/share/i18n/SUPPORTED";

/// The names of the UTF-8 entries of a `SUPPORTED` file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Supported {
    names: BTreeSet<String>,
}

impl Supported {
    /// Entries are `<name> <charset>`; blank lines, comments and lines
    /// without two fields are skipped, a trailing ` \` is tolerated.
    pub fn parse(_text: &str) -> Self {
        Self::default()
    }

    /// Read `<root>/usr/share/i18n/SUPPORTED`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let _ = root.join(SUPPORTED);
        Ok(Self::default())
    }

    /// The `locale.gen` line that generates `language`, if glibc has it.
    pub fn gen_line(&self, _language: &str) -> Option<String> {
        None
    }
}

/// Whether `language` can be installed: well shaped, then pregenerated or
/// listed in `supported`.
pub fn check_language(
    language: &str,
    _supported: Result<&Supported, &String>,
) -> Result<(), String> {
    keyboard::validate_locale(language)
}

/// `text` with `line` active in it; unchanged when it already is.
pub fn with_locale_gen_entry(text: &str, _line: &str) -> String {
    text.to_owned()
}

/// The name `localedef` archives a UTF-8 locale under: `xx_YY.utf8`.
pub fn archive_name(language: &str) -> String {
    language.to_owned()
}

/// Whether `localedef --list-archive` output names `language`.
pub fn archive_lists(_list_output: &str, _language: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXCERPT: &str = "# comment\n\nes_MX.UTF-8 UTF-8\nes_MX ISO-8859-1\naa_ER UTF-8\nsr_RS@latin UTF-8\nbroken\nit_IT.UTF-8 UTF-8 \\\n";

    fn fixture() -> Supported {
        Supported::parse(EXCERPT)
    }

    #[test]
    fn parse_keeps_the_utf8_entries_only() {
        let s = fixture();
        for name in ["es_MX.UTF-8", "aa_ER", "sr_RS@latin", "it_IT.UTF-8"] {
            assert!(s.names.contains(name), "{name}");
        }
        assert!(!s.names.contains("es_MX"));
        assert!(!s.names.contains("broken"));
        assert_eq!(s.names.len(), 4);
    }

    #[test]
    fn gen_line_prefers_the_named_codeset() {
        let s = fixture();
        assert_eq!(s.gen_line("es_MX.UTF-8").unwrap(), "es_MX.UTF-8 UTF-8");
        assert_eq!(s.gen_line("aa_ER.UTF-8").unwrap(), "aa_ER UTF-8");
        assert_eq!(s.gen_line("xx_YY.UTF-8"), None);
    }

    #[test]
    fn load_reads_the_file_under_the_root() {
        let t = tempfile::tempdir().unwrap();
        assert!(Supported::load(t.path()).is_err());
        let file = t.path().join(SUPPORTED);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, EXCERPT).unwrap();
        assert_eq!(Supported::load(t.path()).unwrap(), fixture());
    }

    #[test]
    fn a_pregenerated_language_needs_no_file() {
        let none = Err("no file".to_owned());
        assert!(check_language("it_IT.UTF-8", none.as_ref()).is_ok());
    }

    #[test]
    fn a_supported_language_is_accepted() {
        let s = fixture();
        assert!(check_language("es_MX.UTF-8", Ok(&s)).is_ok());
        assert!(check_language("aa_ER.UTF-8", Ok(&s)).is_ok());
    }

    #[test]
    fn an_unknown_language_is_refused_without_echoing_it() {
        let s = fixture();
        let err = check_language("xx_YY.UTF-8", Ok(&s)).unwrap_err();
        assert!(err.contains("SUPPORTED"), "{err}");
        assert!(!err.contains("xx_YY"), "{err}");
    }

    #[test]
    fn an_unreadable_list_refuses_what_it_cannot_check() {
        let none = Err("No such file".to_owned());
        let err = check_language("es_MX.UTF-8", none.as_ref()).unwrap_err();
        assert!(err.contains("cannot check"), "{err}");
        assert!(err.contains("No such file"), "{err}");
    }

    #[test]
    fn the_shape_is_checked_first() {
        let s = fixture();
        assert!(check_language("sr_RS@latin", Ok(&s)).is_err());
        assert!(check_language("es_MX.ISO", Ok(&s)).is_err());
        assert!(check_language("es_MX", Ok(&s)).is_err());
    }

    #[test]
    fn locale_gen_entry_is_appended_once() {
        let line = "es_MX.UTF-8 UTF-8";
        assert_eq!(
            with_locale_gen_entry("it_IT.UTF-8 UTF-8\n", line),
            "it_IT.UTF-8 UTF-8\nes_MX.UTF-8 UTF-8\n"
        );
        assert_eq!(with_locale_gen_entry("a", line), "a\nes_MX.UTF-8 UTF-8\n");
        let once = with_locale_gen_entry("a\n", line);
        assert_eq!(with_locale_gen_entry(&once, line), once);
    }

    #[test]
    fn a_commented_entry_does_not_count_as_present() {
        let text = "#es_MX.UTF-8 UTF-8\n";
        assert_eq!(
            with_locale_gen_entry(text, "es_MX.UTF-8 UTF-8"),
            "#es_MX.UTF-8 UTF-8\nes_MX.UTF-8 UTF-8\n"
        );
    }

    #[test]
    fn the_archive_name_is_normalized() {
        assert_eq!(archive_name("es_MX.UTF-8"), "es_MX.utf8");
        assert_eq!(archive_name("aa_ER.UTF-8"), "aa_ER.utf8");
    }

    #[test]
    fn the_archive_listing_matches_whole_lines() {
        assert!(archive_lists("C.utf8\nes_MX.utf8\n", "es_MX.UTF-8"));
        assert!(!archive_lists("es_MX.utf8@x\n", "es_MX.UTF-8"));
        assert!(!archive_lists("", "es_MX.UTF-8"));
    }

    #[test]
    fn the_pregenerated_set_has_the_21_locales() {
        let set = keyboard::pregenerated_locales();
        assert_eq!(set.len(), 21, "{set:?}");
        assert!(set.contains("en_US.UTF-8"));
        assert!(keyboard::is_pregenerated("it_IT.UTF-8"));
        assert!(!keyboard::is_pregenerated("es_MX.UTF-8"));
    }

    /// `image/build` greps the same literals out of keyboard.rs to decide
    /// what the payload generates; this keeps the two in step.
    #[test]
    fn the_pregenerated_set_is_what_image_build_derives() {
        let mut found: BTreeSet<&str> = BTreeSet::new();
        for part in include_str!("keyboard.rs").split('"') {
            if is_locale_literal(part) {
                found.insert(part);
            }
        }
        found.insert("en_US.UTF-8");
        assert_eq!(found, keyboard::pregenerated_locales());
    }

    fn is_locale_literal(s: &str) -> bool {
        let Some(base) = s.strip_suffix(".UTF-8") else {
            return false;
        };
        base.split_once('_').is_some_and(|(l, c)| {
            (2..=3).contains(&l.len())
                && l.bytes().all(|b| b.is_ascii_lowercase())
                && c.len() == 2
                && c.bytes().all(|b| b.is_ascii_uppercase())
        })
    }
}
