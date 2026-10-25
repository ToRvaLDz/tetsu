//! A fake machine for the executor tests: a runner that records every
//! call, fakes the tools' file outputs inside a tempdir and snapshots the
//! private files the tools were pointed at.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tetsu_tx::boot::BootError;
use tetsu_tx::boot::space::SpaceProbe;
use tetsu_tx::boot::tool::Runner;

use crate::entropy::fake::Counter;
use crate::frontend::Frontend;
use crate::testing::Script;

use super::exec::{Deps, Executor};
use super::plan::InstallPlan;
use super::target::InstallTarget;
use crate::answers::Answers;
use crate::disk::DiskChoice;
use crate::keyboard::Locale;
use crate::secret::Secret;
use crate::unlock::{Unlock, UnlockLevel};
use crate::user::{Password, UserAccount};

pub const LUKS_UUID: &str = "0b9c7f3e-1111-4222-8333-444455556666";
pub const KERNEL: &str = "6.12.1-arch1-1";
pub const FIRMWARE_BEFORE: &str =
    "BootCurrent: 0001\nBootOrder: 0001,0000\nBoot0000* Windows\nBoot0001* Fedora\n";
pub const FIRMWARE_AFTER: &str = "BootCurrent: 0001\nBootOrder: 0002,0001,0000\nBoot0000* Windows\nBoot0001* Fedora\nBoot0002* Tetsu\n";

pub fn answers(level: UnlockLevel) -> Answers {
    Answers {
        locale: Locale {
            keyboard: "it".into(),
            language: "it_IT.UTF-8".into(),
            timezone: "Europe/Rome".into(),
            rtc_utc: true,
        },
        disk: DiskChoice {
            name: "sda".into(),
            path: "/dev/sda".into(),
            by_id: None,
            size: 512 << 30,
            model: None,
            os: None,
        },
        unlock: Unlock {
            level,
            passphrase: Secret::new("correct horse battery staple"),
            recovery_key: Secret::new("cccccccc-dddddddd-eeeeeeee-ffffffff"),
        },
        user: UserAccount {
            username: "marco".into(),
            full_name: Some("Marco".into()),
            password: Password::Plain(Secret::new("hunter2-hunter2")),
        },
        hostname: "tetsu-ab12".into(),
        agent: None,
    }
}

/// A file a tool was handed inside the run directory, as it was at call time.
#[derive(Clone, Debug)]
pub struct Seen {
    pub name: String,
    pub content: String,
    pub mode: u32,
}

pub struct Machine {
    run_dir: PathBuf,
    pub calls: RefCell<Vec<Vec<String>>>,
    pub seen: RefCell<Vec<Seen>>,
    /// `efibootmgr` output before and after `--create`.
    pub boot_output: RefCell<(String, String)>,
    /// A call whose joined line starts with one of these fails.
    pub fail: RefCell<Vec<String>>,
}

impl Machine {
    pub fn new(run_dir: &Path) -> Self {
        Self {
            run_dir: run_dir.to_path_buf(),
            calls: RefCell::default(),
            seen: RefCell::default(),
            fail: RefCell::default(),
            boot_output: RefCell::new((FIRMWARE_BEFORE.to_owned(), FIRMWARE_AFTER.to_owned())),
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|c| c.join(" ")).collect()
    }

    pub fn fail_on(&self, prefix: &str) {
        self.fail.borrow_mut().push(prefix.to_owned());
    }

    fn snapshot_private_files(&self, args: &[String]) {
        for arg in args {
            let path = Path::new(arg);
            if path.starts_with(&self.run_dir) && path.is_file() {
                let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
                self.seen.borrow_mut().push(Seen {
                    name: path.file_name().unwrap().to_string_lossy().into_owned(),
                    content: fs::read_to_string(path).unwrap_or_default(),
                    mode,
                });
            }
        }
    }

    fn write(path: &str, content: &str) {
        let path = Path::new(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        let i = args.iter().position(|a| a == flag)?;
        args.get(i + 1).map(String::as_str)
    }

    fn glued<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().find_map(|a| a.strip_prefix(flag))
    }

    /// The file effects of the tools the executor drives.
    fn effect(&self, program: &str, args: &[String]) -> String {
        match program {
            "btrfs" if args.starts_with(&["subvolume".into(), "create".into()]) => {
                fs::create_dir_all(&args[2]).unwrap();
            }
            "tar" => payload(Path::new(Self::after(args, "-C").unwrap())),
            "cryptsetup" if args.first().map(String::as_str) == Some("luksUUID") => {
                return format!("{LUKS_UUID}\n");
            }
            "systemd-nspawn" if args.iter().any(|a| a == "mkinitcpio") => {
                let root = Self::after(args, "-D").unwrap();
                Self::write(&format!("{root}/usr/lib/tetsu/initrd.img"), "initrd");
            }
            "openssl" if args.first().map(String::as_str) == Some("passwd") => {
                return "$6$salt$hashedhashedhashed\n".to_owned();
            }
            "openssl" => {
                for flag in ["-keyout", "-out"] {
                    if let Some(out) = Self::after(args, flag) {
                        Self::write(out, &format!("{program} {}", args.join(" ")));
                    }
                }
            }
            "ukify" => Self::write(Self::glued(args, "--output=").unwrap(), "uki"),
            "sbsign" => Self::write(Self::glued(args, "--output=").unwrap(), "signed"),
            "cert-to-efi-sig-list" | "sign-efi-sig-list" => {
                Self::write(args.last().unwrap(), program);
            }
            "efibootmgr" if args.is_empty() => return self.boot_output.borrow().0.clone(),
            "efibootmgr" if args.first().map(String::as_str) == Some("--create") => {
                return self.boot_output.borrow().1.clone();
            }
            _ => {}
        }
        String::new()
    }

    fn answer(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        let args: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let mut line = vec![program.to_owned()];
        line.extend(args.iter().cloned());
        self.snapshot_private_files(&args);
        self.calls.borrow_mut().push(line.clone());
        let joined = line.join(" ");
        if self
            .fail
            .borrow()
            .iter()
            .any(|p| joined.starts_with(p.as_str()))
        {
            return Err(BootError::Tool {
                tool: program.to_owned(),
                status: "exit status: 1".to_owned(),
                stderr: "boom".to_owned(),
            });
        }
        Ok(self.effect(program, &args))
    }
}

/// What the payload tarball would unpack into snapshot 1.
fn payload(root: &Path) {
    let put = |rel: &str, content: &str| Machine::write(&root.join(rel).to_string_lossy(), content);
    put(&format!("usr/lib/modules/{KERNEL}/vmlinuz"), "kernel");
    put("usr/lib/os-release", "NAME=\"Tetsu\"\n");
    put(
        "usr/lib/systemd/boot/efi/systemd-bootx64.efi",
        "systemd-boot",
    );
    put("etc/passwd", "root:x:0:0::/root:/bin/bash\n");
    put("etc/shadow", "root:$6$old$oldhash:19000:0:99999:7:::\n");
    put("etc/group", "root:x:0:\nwheel:x:998:\n");
}

impl Runner for Machine {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.answer(program, args).map(drop)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        self.answer(program, args)
    }
}

/// Plenty of room on every ESP.
pub struct Roomy;

impl SpaceProbe for Roomy {
    fn available_bytes(&self, _: &Path) -> Result<u64, BootError> {
        Ok(1 << 40)
    }
}

/// A target whose directories all live in `root`.
pub fn target(root: &Path, secure_boot: crate::preflight::SecureBoot) -> InstallTarget {
    let mut t = InstallTarget::new(
        Path::new("/dev/sda"),
        Path::new("/medium/payload.tar"),
        &root.join("work"),
        secure_boot,
    );
    t.run_dir = root.join("run");
    t.cache_dir = root.join("cache");
    t
}

/// Run `plan` (or only its first stage) on a fresh fake machine.
pub struct Outcome {
    pub machine: Machine,
    pub script: Script,
    pub result: Result<(), crate::error::InstallError>,
    pub log: Vec<String>,
}

pub fn run_plan(
    answers: &Answers,
    target: &InstallTarget,
    plan: &InstallPlan,
    cancelled: &dyn Fn() -> bool,
) -> Outcome {
    run_with(answers, target, plan, cancelled, |_| {})
}

/// [`run_plan`] with a chance to script the machine first.
pub fn run_with(
    answers: &Answers,
    target: &InstallTarget,
    plan: &InstallPlan,
    cancelled: &dyn Fn() -> bool,
    setup: impl FnOnce(&Machine),
) -> Outcome {
    let machine = Machine::new(&target.run_dir);
    setup(&machine);
    let mut script = Script::new();
    let entropy = Counter::default();
    let (result, log) = {
        let deps = Deps {
            runner: &machine,
            probe: &Roomy,
            entropy: &entropy,
            cancelled,
        };
        let fe: &mut dyn Frontend = &mut script;
        let mut exec = Executor::new(deps, answers, target, fe).unwrap();
        let result = exec.run(plan);
        (result, exec.log().lines().to_vec())
    };
    Outcome {
        machine,
        script,
        result,
        log,
    }
}
