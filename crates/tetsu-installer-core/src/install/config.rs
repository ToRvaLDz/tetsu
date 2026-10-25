//! Content of the configuration files written into the new system. Pure
//! functions over strings, so every file is unit-tested without a disk.

use crate::agent::AgentBackend;
use crate::unlock::UnlockLevel;
use crate::unlockfile::UnlockFile;

/// UID and GID of the owner (the first regular user).
pub const OWNER_ID: u32 = 1000;
/// GID given to `wheel` when the image has no such group.
pub const WHEEL_FALLBACK_GID: u32 = 998;
/// Login shell of the owner.
pub const OWNER_SHELL: &str = "/bin/bash";

/// Hardware clock in UTC, no drift correction (the `adjtime` file format).
pub const ADJTIME: &str = "0.0 0 0\n0\nUTC\n";

/// A TOML basic string, quoted and escaped.
pub fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `text` with `line` appended on a line of its own.
pub fn append_line(text: &str, line: &str) -> String {
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    out
}

/// The `passwd` line of the owner. Colons and control characters cannot
/// appear in the GECOS field.
pub fn passwd_line(user: &str, full_name: Option<&str>) -> String {
    let gecos: String = full_name
        .unwrap_or("")
        .chars()
        .filter(|c| *c != ':' && !c.is_control())
        .collect();
    format!("{user}:x:{OWNER_ID}:{OWNER_ID}:{gecos}:/home/{user}:{OWNER_SHELL}")
}

/// The `shadow` line of the owner; `days` is the last change in days since
/// the epoch.
pub fn shadow_line(user: &str, hash: &str, days: u64) -> String {
    format!("{user}:{hash}:{days}:0:99999:7:::")
}

/// `shadow` with the root password locked (`!`).
pub fn lock_root(shadow: &str) -> String {
    let mut found = false;
    let mut out = String::new();
    for line in shadow.lines() {
        if line.split(':').next() == Some("root") {
            let rest: Vec<&str> = line.split(':').skip(2).collect();
            out.push_str("root:!");
            if !rest.is_empty() {
                out.push(':');
                out.push_str(&rest.join(":"));
            }
            found = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        out.push_str("root:!::0:99999:7:::\n");
    }
    out
}

/// `group` with the owner's private group added and `user` made a member
/// of `wheel` (created when missing).
pub fn add_owner_groups(group: &str, user: &str) -> String {
    let mut out = String::new();
    let mut wheel_seen = false;
    for line in group.lines() {
        let fields: Vec<&str> = line.splitn(4, ':').collect();
        if fields.first() == Some(&"wheel") && fields.len() == 4 {
            wheel_seen = true;
            let members = match fields[3] {
                "" => user.to_owned(),
                m => format!("{m},{user}"),
            };
            out.push_str(&format!("wheel:{}:{}:{members}", fields[1], fields[2]));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !wheel_seen {
        out.push_str(&format!("wheel:x:{WHEEL_FALLBACK_GID}:{user}\n"));
    }
    out.push_str(&format!("{user}:x:{OWNER_ID}:\n"));
    out
}

pub fn vconsole(keymap: &str) -> String {
    format!("KEYMAP={keymap}\n")
}

pub fn locale_conf(language: &str) -> String {
    format!("LANG={language}\n")
}

pub fn hostname(name: &str) -> String {
    format!("{name}\n")
}

/// Mounts of the shared subvolumes and the ESP; the root itself comes from
/// the kernel command line.
pub fn fstab(mapper: &str, esp_label: &str) -> String {
    let mut out = String::from("# Shared subvolumes and the ESP; the root comes from the UKI.\n");
    for name in ["var", "home", "root"] {
        out.push_str(&format!(
            "{mapper}  /{name}  btrfs  subvol=@{name},noatime  0 0\n"
        ));
    }
    out.push_str(&format!(
        "PARTLABEL={esp_label}  /efi  vfat  umask=0077  0 2\n"
    ));
    out
}

/// The base kernel command line. The snapshot's own `rootflags=subvol=`
/// is appended per UKI by the builder, so it is not baked in here.
pub fn cmdline(luks_uuid: &str, mapper_name: &str, console_args: &[String]) -> String {
    let mut line =
        format!("rd.luks.name={luks_uuid}={mapper_name} root=/dev/mapper/{mapper_name} rw");
    for arg in console_args {
        line.push(' ');
        line.push_str(arg);
    }
    line.push('\n');
    line
}

/// `etc/tetsu/agent-backend.toml`; the API key itself is staged elsewhere.
pub fn agent_backend(backend: &AgentBackend) -> String {
    match backend {
        AgentBackend::Api { provider, .. } => {
            format!("backend = \"api\"\nprovider = \"{}\"\n", provider.id())
        }
        AgentBackend::Cli(tool) => format!("backend = \"cli\"\ntool = \"{}\"\n", tool.id()),
    }
}

/// The staged API key file, picked up by the agent on first use.
pub fn staged_api_key(provider: &str, key: &str) -> String {
    format!(
        "provider = {}\nkey = {}\n",
        toml_string(provider),
        toml_string(key)
    )
}

/// `etc/tetsu/unlock-level.toml`. Paranoid needs no TPM seal, so it is
/// done from the start; the others wait for the first boot.
pub fn unlock_level(level: UnlockLevel) -> String {
    UnlockFile::initial(level).render()
}

/// `loader/loader.conf` of systemd-boot.
pub fn loader_conf() -> &'static str {
    "timeout 3\neditor no\nconsole-mode keep\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ApiProvider, CliTool};
    use crate::secret::Secret;

    #[test]
    fn toml_strings_are_escaped() {
        assert_eq!(toml_string("plain"), "\"plain\"");
        assert_eq!(toml_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(toml_string("\u{7}"), "\"\\u0007\"");
    }

    #[test]
    fn append_line_adds_the_missing_newline() {
        assert_eq!(append_line("a\n", "b"), "a\nb\n");
        assert_eq!(append_line("a", "b"), "a\nb\n");
        assert_eq!(append_line("", "b"), "b\n");
    }

    #[test]
    fn passwd_and_shadow_lines() {
        assert_eq!(
            passwd_line("marco", Some("Marco: M\n")),
            "marco:x:1000:1000:Marco M:/home/marco:/bin/bash"
        );
        assert_eq!(
            passwd_line("marco", None),
            "marco:x:1000:1000::/home/marco:/bin/bash"
        );
        assert_eq!(
            shadow_line("marco", "$6$s$h", 20000),
            "marco:$6$s$h:20000:0:99999:7:::"
        );
    }

    #[test]
    fn root_gets_locked_keeping_the_other_fields() {
        let shadow = "root:$6$x$y:19000:0:99999:7:::\nbin:!!:19000::::::\n";
        assert_eq!(
            lock_root(shadow),
            "root:!:19000:0:99999:7:::\nbin:!!:19000::::::\n"
        );
        assert_eq!(
            lock_root("bin:!!:1::::::\n"),
            "bin:!!:1::::::\nroot:!::0:99999:7:::\n"
        );
    }

    #[test]
    fn owner_joins_wheel_and_gets_a_private_group() {
        let group = "root:x:0:\nwheel:x:998:\n";
        assert_eq!(
            add_owner_groups(group, "marco"),
            "root:x:0:\nwheel:x:998:marco\nmarco:x:1000:\n"
        );
        let group = "wheel:x:10:alice\n";
        assert_eq!(
            add_owner_groups(group, "marco"),
            "wheel:x:10:alice,marco\nmarco:x:1000:\n"
        );
        assert_eq!(
            add_owner_groups("root:x:0:\n", "marco"),
            "root:x:0:\nwheel:x:998:marco\nmarco:x:1000:\n"
        );
    }

    #[test]
    fn small_files() {
        assert_eq!(vconsole("it"), "KEYMAP=it\n");
        assert_eq!(locale_conf("it_IT.UTF-8"), "LANG=it_IT.UTF-8\n");
        assert_eq!(hostname("tetsu-ab12"), "tetsu-ab12\n");
        assert_eq!(ADJTIME.lines().last(), Some("UTC"));
        assert!(loader_conf().contains("editor no"));
    }

    #[test]
    fn cmdline_has_no_crypttab_and_no_root_subvolume() {
        assert_eq!(
            cmdline("1234-uuid", "root", &[]),
            "rd.luks.name=1234-uuid=root root=/dev/mapper/root rw\n"
        );
    }

    #[test]
    fn cmdline_keeps_the_consoles_in_order() {
        let consoles = ["console=tty0".to_owned(), "console=ttyS0,115200".to_owned()];
        assert_eq!(
            cmdline("1234-uuid", "root", &consoles),
            "rd.luks.name=1234-uuid=root root=/dev/mapper/root rw console=tty0 console=ttyS0,115200\n"
        );
    }

    #[test]
    fn fstab_mounts_the_shared_subvolumes_and_the_esp() {
        let f = fstab("/dev/mapper/root", "TETSU-ESP");
        for name in ["var", "home", "root"] {
            assert!(f.contains(&format!("/dev/mapper/root  /{name}  btrfs  subvol=@{name}")));
        }
        assert!(f.contains("PARTLABEL=TETSU-ESP  /efi  vfat"));
    }

    #[test]
    fn agent_files() {
        let api = AgentBackend::Api {
            provider: ApiProvider::OpenAi,
            key: Secret::new("sk-secret-key"),
        };
        let text = agent_backend(&api);
        assert_eq!(text, "backend = \"api\"\nprovider = \"openai\"\n");
        assert!(!text.contains("sk-secret-key"));
        assert_eq!(
            agent_backend(&AgentBackend::Cli(CliTool::ClaudeCode)),
            "backend = \"cli\"\ntool = \"claude-code\"\n"
        );
        assert_eq!(
            staged_api_key("openai", "sk-a\"b"),
            "provider = \"openai\"\nkey = \"sk-a\\\"b\"\n"
        );
    }

    #[test]
    fn unlock_level_state_depends_on_the_tpm() {
        assert_eq!(
            unlock_level(UnlockLevel::Comfort),
            "level = \"comfort\"\nstate = \"pending\"\n"
        );
        assert_eq!(
            unlock_level(UnlockLevel::Balanced),
            "level = \"balanced\"\nstate = \"pending\"\n"
        );
        assert_eq!(
            unlock_level(UnlockLevel::Paranoid),
            "level = \"paranoid\"\nstate = \"sealed\"\n"
        );
    }
}
