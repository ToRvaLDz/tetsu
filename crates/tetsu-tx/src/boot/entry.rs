//! File-name grammar of the Tetsu UKIs in `EFI/Linux`.
//!
//! * `tetsu-<n>.efi`                 blessed (Good)
//! * `tetsu-<n>+<left>.efi`          never booted (Untried)
//! * `tetsu-<n>+<left>-<done>.efi`   booted at least once (Trying / Bad)
//! * `tetsu-<n>.efi.tmp`             staged, not yet committed

const PREFIX: &str = "tetsu-";
const EXT: &str = ".efi";
const TMP_EXT: &str = ".tmp";

/// Boot-counting suffix: attempts left and attempts done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tries {
    pub left: u32,
    pub done: u32,
}

/// A committed entry name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryName {
    pub counter: u64,
    pub tries: Option<Tries>,
}

/// Boot-counting state of a committed entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootState {
    Untried,
    Trying,
    Bad,
    Good,
}

/// What a file name in `EFI/Linux` means to Tetsu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Entry(EntryName),
    /// Staged `tetsu-<n>.efi.tmp`.
    Tmp(u64),
}

impl EntryName {
    /// An entry that has just been committed with `left` tries.
    pub fn untried(counter: u64, left: u32) -> Self {
        Self {
            counter,
            tries: Some(Tries { left, done: 0 }),
        }
    }

    pub fn state(&self) -> BootState {
        match self.tries {
            None => BootState::Good,
            Some(t) if t.left == 0 => BootState::Bad,
            Some(t) if t.done == 0 => BootState::Untried,
            Some(_) => BootState::Trying,
        }
    }

    pub fn file_name(&self) -> String {
        match self.tries {
            None => format!("{PREFIX}{}{EXT}", self.counter),
            Some(Tries { left, done: 0 }) => format!("{PREFIX}{}+{left}{EXT}", self.counter),
            Some(Tries { left, done }) => {
                format!("{PREFIX}{}+{left}-{done}{EXT}", self.counter)
            }
        }
    }
}

/// Name of the staged file for `counter`.
pub fn tmp_file_name(counter: u64) -> String {
    format!("{PREFIX}{counter}{EXT}{TMP_EXT}")
}

/// Classify a file name; `None` for anything that is not Tetsu's.
pub fn parse_file_name(name: &str) -> Option<FileKind> {
    let rest = name.strip_prefix(PREFIX)?;
    if let Some(num) = rest.strip_suffix(TMP_EXT) {
        let num = num.strip_suffix(EXT)?;
        return canonical(num).map(FileKind::Tmp);
    }
    let body = rest.strip_suffix(EXT)?;
    let (counter, suffix) = match body.split_once('+') {
        Some((c, s)) => (c, Some(s)),
        None => (body, None),
    };
    let counter = canonical(counter)?;
    let tries = match suffix {
        None => None,
        Some(s) => {
            let (left, done) = match s.split_once('-') {
                Some((l, d)) => (canonical(l)?, canonical(d)?),
                None => (canonical(s)?, 0),
            };
            Some(Tries {
                left: u32::try_from(left).ok()?,
                done: u32::try_from(done).ok()?,
            })
        }
    };
    Some(FileKind::Entry(EntryName { counter, tries }))
}

/// Digits only, no sign and no leading zeros, so every name has one spelling.
fn canonical(s: &str) -> Option<u64> {
    let ok =
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    if ok { s.parse().ok() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> EntryName {
        match parse_file_name(name) {
            Some(FileKind::Entry(e)) => e,
            other => panic!("{name}: expected entry, got {other:?}"),
        }
    }

    #[test]
    fn parses_every_state() {
        assert_eq!(entry("tetsu-7.efi").state(), BootState::Good);
        assert_eq!(entry("tetsu-7+3.efi").state(), BootState::Untried);
        assert_eq!(entry("tetsu-7+2-1.efi").state(), BootState::Trying);
        assert_eq!(entry("tetsu-7+0-3.efi").state(), BootState::Bad);
    }

    #[test]
    fn parses_fields() {
        let e = entry("tetsu-12+2-1.efi");
        assert_eq!(e.counter, 12);
        assert_eq!(e.tries, Some(Tries { left: 2, done: 1 }));
        assert_eq!(entry("tetsu-12.efi").tries, None);
        assert_eq!(
            entry("tetsu-5+3.efi").tries,
            Some(Tries { left: 3, done: 0 })
        );
    }

    #[test]
    fn exhausted_without_attempts_is_bad() {
        assert_eq!(entry("tetsu-7+0.efi").state(), BootState::Bad);
    }

    #[test]
    fn format_round_trips() {
        for name in [
            "tetsu-1.efi",
            "tetsu-1+3.efi",
            "tetsu-10+2-1.efi",
            "tetsu-10+0-3.efi",
            "tetsu-0.efi",
        ] {
            assert_eq!(entry(name).file_name(), name);
        }
    }

    #[test]
    fn untried_constructor_formats() {
        assert_eq!(EntryName::untried(9, 3).file_name(), "tetsu-9+3.efi");
        assert_eq!(EntryName::untried(9, 3).state(), BootState::Untried);
    }

    #[test]
    fn tmp_names() {
        assert_eq!(tmp_file_name(4), "tetsu-4.efi.tmp");
        assert_eq!(parse_file_name("tetsu-4.efi.tmp"), Some(FileKind::Tmp(4)));
    }

    #[test]
    fn foreign_names_are_ignored() {
        for name in [
            "",
            "fedora-6.1.efi",
            "tetsu-.efi",
            "tetsu-x.efi",
            "tetsu-1.EFI",
            "tetsu-1.efi.bak",
            "tetsu-1+.efi",
            "tetsu-1+3-.efi",
            "tetsu-1+a.efi",
            "tetsu-1+3-1-1.efi",
            "tetsu-+3.efi",
            "tetsu-1.tmp",
            "tetsu-1+3.efi.tmp",
            "tetsu-+1.efi",
            "tetsu--1.efi",
            "xtetsu-1.efi",
            "tetsu-01.efi",
            "tetsu-1+03.efi",
        ] {
            assert_eq!(parse_file_name(name), None, "{name:?}");
        }
    }
}
