//! User step: login name, optional full name and a session password that
//! is separate from the disk passphrase. Root stays locked; the owner joins
//! `wheel` (applied by the install plan, not here).

use crate::entropy::Entropy;
use crate::error::InstallError;
use crate::frontend::{Frontend, ask_valid};
use crate::secret::Secret;

/// Longest login name useradd accepts.
pub const MAX_USERNAME: usize = 32;
const MIN_PASSWORD: usize = 8;
const MAX_FULL_NAME: usize = 64;
const HOSTNAME_PREFIX: &str = "tetsu-";
const HOSTNAME_LEN: usize = 4;
const HOSTNAME_ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// System account names a user may not take.
const RESERVED: &[&str] = &[
    "root",
    "daemon",
    "bin",
    "sys",
    "sync",
    "games",
    "man",
    "lp",
    "mail",
    "news",
    "uucp",
    "proxy",
    "www-data",
    "backup",
    "list",
    "irc",
    "nobody",
    "dbus",
    "polkitd",
    "sshd",
    "wheel",
    "adm",
    "users",
    "tetsu",
    "avahi",
    "colord",
    "git",
    "http",
    "ftp",
    "mysql",
    "postgres",
    "rtkit",
    "nm-openvpn",
    "cups",
    "geoclue",
    "dhcpcd",
];

/// How the session password reaches the install plan.
#[derive(Clone, PartialEq, Eq)]
pub enum Password {
    /// Typed by the user; hashed by the install plan.
    Plain(Secret),
    /// A crypt(3) hash given ahead of time (answer files).
    Hash(String),
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(_) => f.write_str("Password::Plain(***)"),
            Self::Hash(_) => f.write_str("Password::Hash(***)"),
        }
    }
}

impl Password {
    /// Accept a `$id$...` crypt hash.
    pub fn from_hash(hash: &str) -> Result<Self, String> {
        let mut parts = hash.split('$');
        let shape = parts.next() == Some("")
            && parts
                .next()
                .is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
            && parts.next().is_some_and(|s| !s.is_empty())
            && !hash.chars().any(|c| c.is_whitespace() || c == ':');
        shape
            .then(|| Self::Hash(hash.to_owned()))
            .ok_or_else(|| "not a crypt hash ($id$salt$hash)".to_owned())
    }
}

/// The account created for the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAccount {
    pub username: String,
    pub full_name: Option<String>,
    pub password: Password,
}

/// useradd rules: `[a-z_][a-z0-9_-]*`, at most 32 characters, no system name.
pub fn validate_username(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    if !first_ok
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(
            "use lowercase letters, digits, '_' and '-', starting with a letter or '_'".into(),
        );
    }
    if name.len() > MAX_USERNAME {
        return Err(format!("at most {MAX_USERNAME} characters"));
    }
    if RESERVED.contains(&name) || name.starts_with("systemd-") {
        return Err(format!("{name} is reserved for the system"));
    }
    Ok(())
}

/// The full name ends up in passwd's GECOS field.
pub fn validate_full_name(name: &str) -> Result<(), String> {
    if name.chars().any(|c| c.is_control() || c == ':' || c == ',') {
        return Err("no ':' or ',' in the name".into());
    }
    if name.chars().count() > MAX_FULL_NAME {
        return Err(format!("at most {MAX_FULL_NAME} characters"));
    }
    Ok(())
}

/// Ask the session password, twice, with the usual restrictions.
pub fn ask_password(
    fe: &mut dyn Frontend,
    username: &str,
    disk_passphrase: &Secret,
) -> Result<Secret, InstallError> {
    loop {
        let first = fe.ask_secret("Session password")?;
        let problem = if first.chars().count() < MIN_PASSWORD {
            Some(format!("use at least {MIN_PASSWORD} characters"))
        } else if first == disk_passphrase.expose() {
            Some("it must differ from the disk passphrase".to_owned())
        } else if first == username {
            Some("it must differ from the user name".to_owned())
        } else {
            None
        };
        if let Some(msg) = problem {
            fe.message(&msg);
            continue;
        }
        if fe.ask_secret("Repeat the session password")? == first {
            return Ok(Secret::new(first));
        }
        fe.message("The passwords do not match.");
    }
}

/// Ask username, optional full name and the session password.
pub fn run(fe: &mut dyn Frontend, disk_passphrase: &Secret) -> Result<UserAccount, InstallError> {
    let username = ask_valid(fe, "User name", &validate_username)?;
    let full = ask_valid(fe, "Full name (optional)", &validate_full_name)?;
    let password = ask_password(fe, &username, disk_passphrase)?;
    Ok(UserAccount {
        username,
        full_name: Some(full.trim().to_owned()).filter(|n| !n.is_empty()),
        password: Password::Plain(password),
    })
}

/// `tetsu-` followed by 4 random characters from `[a-z0-9]`.
pub fn hostname(entropy: &dyn Entropy) -> Result<String, InstallError> {
    // Reject bytes above the last full multiple of the alphabet size, so
    // every character is equally likely.
    let limit = 256 - 256 % HOSTNAME_ALPHABET.len();
    let mut suffix = String::new();
    while suffix.len() < HOSTNAME_LEN {
        let mut buf = [0u8; 16];
        entropy.fill(&mut buf)?;
        for b in buf.into_iter().filter(|b| usize::from(*b) < limit) {
            if suffix.len() < HOSTNAME_LEN {
                suffix.push(char::from(
                    HOSTNAME_ALPHABET[usize::from(b) % HOSTNAME_ALPHABET.len()],
                ));
            }
        }
    }
    Ok(format!("{HOSTNAME_PREFIX}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::fake::Counter;
    use crate::testing::Script;

    #[test]
    fn username_rules() {
        for ok in ["marco", "_svc", "a-b_c9", "x"] {
            assert!(validate_username(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Marco",
            "9lives",
            "-x",
            "a b",
            "a.b",
            "root",
            "systemd-resolve",
            "é",
        ] {
            assert!(validate_username(bad).is_err(), "{bad}");
        }
        assert!(validate_username(&"a".repeat(32)).is_ok());
        assert!(validate_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn full_name_rules() {
        assert!(validate_full_name("").is_ok());
        assert!(validate_full_name("Marco Migozzi").is_ok());
        assert!(validate_full_name("a:b").is_err());
        assert!(validate_full_name("a,b").is_err());
    }

    #[test]
    fn password_hash_shape() {
        assert!(Password::from_hash("$6$salt$abcdef").is_ok());
        assert!(Password::from_hash("$y$j9T$salt$hash").is_ok());
        assert!(Password::from_hash("plaintext").is_err());
        assert!(Password::from_hash("$6$").is_err());
        assert!(Password::from_hash("$6$a$b:c").is_err());
    }

    #[test]
    fn run_collects_the_account() {
        let disk = Secret::new("Disk-Pass-Phrase-1");
        let mut fe = Script::new()
            .text("Root")
            .text("marco")
            .text("  Marco M  ")
            .secret("short")
            .secret("Disk-Pass-Phrase-1")
            .secret("session-pw-1")
            .secret("session-pw-2")
            .secret("session-pw-1")
            .secret("session-pw-1");
        let u = run(&mut fe, &disk).unwrap();
        assert_eq!(u.username, "marco");
        assert_eq!(u.full_name.as_deref(), Some("Marco M"));
        assert_eq!(u.password, Password::Plain(Secret::new("session-pw-1")));
        assert!(fe.messages.iter().any(|m| m.contains("at least 8")));
        assert!(
            fe.messages
                .iter()
                .any(|m| m.contains("differ from the disk"))
        );
        assert!(fe.messages.iter().any(|m| m.contains("do not match")));
        assert!(!format!("{u:?}").contains("session-pw"));
        fe.assert_done();
    }

    #[test]
    fn empty_full_name_is_none() {
        let mut fe = Script::new()
            .text("marco")
            .text("")
            .secret("session-pw-1")
            .secret("session-pw-1");
        let u = run(&mut fe, &Secret::new("x")).unwrap();
        assert_eq!(u.full_name, None);
    }

    #[test]
    fn hostname_has_prefix_and_four_chars() {
        let h = hostname(&Counter::default()).unwrap();
        let suffix = h.strip_prefix("tetsu-").unwrap();
        assert_eq!(suffix.len(), 4);
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        // Counter bytes 0, 7, 14, 21 -> a h o v.
        assert_eq!(suffix, "ahov");
    }
}
