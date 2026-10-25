//! Stage 1, configuration: the files written into snapshot 1 and the
//! owner's home, then the initrd.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tetsu_tx::store::snapshot::FIRST_COUNTER;
use tetsu_tx::tx::nspawn::Nspawn;
use tetsu_tx::tx::steps::{build_initrd, kernel_version};

use crate::agent::AgentBackend;
use crate::error::InstallError;
use crate::keyboard::validate_timezone;
use crate::user::{Password, validate_full_name, validate_username};

use super::config::{self, OWNER_ID};
use super::exec::Executor;
use super::plan::Action;
use super::target::{ESP_LABEL, MAPPER_NAME};

const SECS_PER_DAY: u64 = 86_400;

/// Write `content` to `path` with `mode`, creating missing parents.
pub(super) fn write_file(path: &Path, content: &str, mode: u32) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| InstallError::io(parent, e))?;
    }
    let io = |e| InstallError::io(path, e);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(io)?;
    file.write_all(content.as_bytes()).map_err(io)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io)
}

/// A text file that may not exist yet.
fn read_or_empty(path: &Path) -> Result<String, InstallError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(InstallError::io(path, e)),
    }
}

fn invalid(what: &str, why: String) -> InstallError {
    InstallError::Failed(format!("invalid {what}: {why}"))
}

impl Executor<'_> {
    fn etc(&self) -> PathBuf {
        self.target.layout().etc(FIRST_COUNTER)
    }

    fn snapshot_root(&self) -> PathBuf {
        self.target.layout().root(FIRST_COUNTER)
    }

    fn home(&self) -> PathBuf {
        let user = &self.answers.user.username;
        self.target.layout().shared("@home").join(user)
    }

    pub(super) fn system_action(&mut self, action: Action) -> Result<(), InstallError> {
        match action {
            Action::HashPassword => self.hash_password(),
            Action::WriteUser => self.write_user(),
            Action::LockRoot => self.lock_root(),
            Action::WriteLocale => self.write_locale(),
            Action::WriteHostname => self.write_hostname(),
            Action::WriteFstab => self.write_fstab(),
            Action::WriteCmdline => self.write_cmdline(),
            Action::WriteAgentBackend => self.write_agent_backend(),
            Action::StageApiKey => self.stage_api_key(),
            Action::WriteUnlockLevel => self.write_unlock_level(),
            Action::BuildInitrd => self.build_initrd(),
            other => Err(InstallError::Failed(format!(
                "{other:?} is not implemented yet"
            ))),
        }
    }

    /// `openssl passwd -6` reads the password from a 0600 file, so it
    /// never shows up in argv.
    fn hash_password(&mut self) -> Result<(), InstallError> {
        let Password::Plain(plain) = &self.answers.user.password else {
            return Ok(());
        };
        let file = self
            .run
            .secret("session-password", &format!("{}\n", plain.expose()))?;
        let hash = self.tool_output("openssl", &[&"passwd", &"-6", &"-in", &file.path()])?;
        file.remove()?;
        let hash = hash.trim();
        if !hash.starts_with("$6$") || hash.contains(':') {
            return Err(InstallError::Failed(
                "openssl produced an unusable password hash".into(),
            ));
        }
        self.state.password_hash = Some(hash.to_owned());
        Ok(())
    }

    fn password_hash(&self) -> Result<String, InstallError> {
        match &self.answers.user.password {
            Password::Hash(h) => Ok(h.clone()),
            Password::Plain(_) => self
                .state
                .password_hash
                .clone()
                .ok_or_else(|| InstallError::Failed("the password was not hashed".into())),
        }
    }

    fn write_user(&mut self) -> Result<(), InstallError> {
        let user = &self.answers.user;
        validate_username(&user.username).map_err(|w| invalid("user name", w))?;
        if let Some(name) = &user.full_name {
            validate_full_name(name).map_err(|w| invalid("full name", w))?;
        }
        let hash = self.password_hash()?;
        let etc = self.etc();
        let days = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / SECS_PER_DAY);

        let passwd = etc.join("passwd");
        let line = config::passwd_line(&user.username, user.full_name.as_deref());
        write_file(
            &passwd,
            &config::append_line(&read_or_empty(&passwd)?, &line),
            0o644,
        )?;

        let shadow = etc.join("shadow");
        let line = config::shadow_line(&user.username, &hash, days);
        write_file(
            &shadow,
            &config::append_line(&read_or_empty(&shadow)?, &line),
            0o600,
        )?;

        let group = etc.join("group");
        let text = config::add_owner_groups(&read_or_empty(&group)?, &user.username);
        write_file(&group, &text, 0o644)?;

        self.create_home()
    }

    /// An empty home in `@home`, owned by the owner.
    fn create_home(&self) -> Result<(), InstallError> {
        let home = self.home();
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&home)
            .map_err(|e| InstallError::io(&home, e))?;
        self.chown_home()
    }

    fn chown_home(&self) -> Result<(), InstallError> {
        let owner = format!("{OWNER_ID}:{OWNER_ID}");
        self.tool("chown", &[&"-R", &owner, &self.home()])
    }

    fn lock_root(&mut self) -> Result<(), InstallError> {
        let shadow = self.etc().join("shadow");
        let text = config::lock_root(&read_or_empty(&shadow)?);
        write_file(&shadow, &text, 0o600)
    }

    fn write_locale(&mut self) -> Result<(), InstallError> {
        let l = &self.answers.locale;
        validate_timezone(&l.timezone).map_err(|w| invalid("timezone", w))?;
        let etc = self.etc();
        write_file(
            &etc.join("vconsole.conf"),
            &config::vconsole(&l.keyboard),
            0o644,
        )?;
        write_file(
            &etc.join("locale.conf"),
            &config::locale_conf(&l.language),
            0o644,
        )?;
        write_file(&etc.join("adjtime"), config::ADJTIME, 0o644)?;
        let link = etc.join("localtime");
        match fs::remove_file(&link) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(InstallError::io(&link, e)),
        }
        let zone = format!("/usr/share/zoneinfo/{}", l.timezone);
        symlink(&zone, &link).map_err(|e| InstallError::io(&link, e))
    }

    fn write_hostname(&mut self) -> Result<(), InstallError> {
        let name = &self.answers.hostname;
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(invalid("hostname", name.clone()));
        }
        write_file(&self.etc().join("hostname"), &config::hostname(name), 0o644)
    }

    fn write_fstab(&mut self) -> Result<(), InstallError> {
        let root = self.snapshot_root();
        for mountpoint in ["var", "home", "root", "efi"] {
            let dir = root.join(mountpoint);
            fs::create_dir_all(&dir).map_err(|e| InstallError::io(&dir, e))?;
        }
        let mapper = format!("/dev/mapper/{MAPPER_NAME}");
        let text = config::fstab(&mapper, ESP_LABEL);
        write_file(&self.etc().join("fstab"), &text, 0o644)
    }

    /// The base command line; the per-snapshot `rootflags=subvol=` is added
    /// by the UKI builder.
    fn write_cmdline(&mut self) -> Result<(), InstallError> {
        let uuid = self
            .state
            .luks_uuid
            .clone()
            .ok_or_else(|| InstallError::Failed("the LUKS UUID is not known".into()))?;
        let text = config::cmdline(&uuid, MAPPER_NAME);
        write_file(&self.etc().join("kernel/cmdline"), &text, 0o644)?;
        self.state.cmdline = Some(text.trim().to_owned());
        Ok(())
    }

    fn write_agent_backend(&mut self) -> Result<(), InstallError> {
        let Some(backend) = &self.answers.agent else {
            return Ok(());
        };
        let path = self.etc().join("tetsu/agent-backend.toml");
        write_file(&path, &config::agent_backend(backend), 0o644)
    }

    /// The API key waits in the owner's home until the agent first runs.
    fn stage_api_key(&mut self) -> Result<(), InstallError> {
        let Some(AgentBackend::Api { provider, key }) = &self.answers.agent else {
            return Ok(());
        };
        let dir = self.home().join(".local/share/tetsu");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| InstallError::io(&dir, e))?;
        let text = config::staged_api_key(provider.id(), key.expose());
        write_file(&dir.join("staged-api-key"), &text, 0o600)?;
        self.chown_home()
    }

    fn write_unlock_level(&mut self) -> Result<(), InstallError> {
        let path = self.etc().join("tetsu/unlock-level.toml");
        write_file(
            &path,
            &config::unlock_level(self.answers.unlock.level),
            0o644,
        )
    }

    /// Build the initrd offline in a container over snapshot 1.
    fn build_initrd(&mut self) -> Result<(), InstallError> {
        let root = self.snapshot_root();
        let version = kernel_version(&root)?;
        let scratch = self.run.subdir("initrd-var")?;
        fs::create_dir_all(&self.target.cache_dir)
            .map_err(|e| InstallError::io(&self.target.cache_dir, e))?;
        let ns = Nspawn::new(&root, &scratch, &self.target.cache_dir);
        build_initrd(self.deps.runner, &ns, &version)?;
        self.state.kernel_version = Some(version);
        Ok(())
    }
}
