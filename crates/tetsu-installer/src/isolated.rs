//! A runner whose tools sit in their own process group. The terminal sends
//! Ctrl+C to the foreground group only, so it reaches the installer (which
//! turns it into the "abort?" question between two actions) and never kills
//! a running `cryptsetup`, `tar` or `efi-updatevar` halfway.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::process::Command;

use tetsu_tx::boot::BootError;
use tetsu_tx::boot::tool::{Runner, SystemRunner};

/// [`SystemRunner`] with every tool in a new process group.
#[derive(Clone, Copy, Debug, Default)]
pub struct IsolatedRunner;

impl IsolatedRunner {
    fn capture(program: &str, args: &[OsString]) -> Result<String, BootError> {
        let out = match Command::new(program).args(args).process_group(0).output() {
            Ok(o) => o,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(BootError::ToolMissing {
                    tool: program.to_owned(),
                });
            }
            Err(e) => {
                return Err(BootError::Io {
                    path: program.into(),
                    source: e,
                });
            }
        };
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        Err(BootError::Tool {
            tool: program.to_owned(),
            status: out.status.to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

impl Runner for IsolatedRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        Self::capture(program, args).map(drop)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        Self::capture(program, args)
    }

    fn check_tool(&self, program: &str) -> Result<(), BootError> {
        SystemRunner.check_tool(program)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// The process group in `/proc/<id>/stat` (field 5, after the command
    /// name, which may hold spaces and parentheses).
    fn pgrp(stat: &str) -> u32 {
        let after = stat.rsplit_once(')').unwrap().1;
        after.split_whitespace().nth(2).unwrap().parse().unwrap()
    }

    fn arg(s: &str) -> OsString {
        s.into()
    }

    #[test]
    fn the_tool_leaves_the_process_group_of_the_installer() {
        let ours = pgrp(&fs::read_to_string("/proc/self/stat").unwrap());
        let args = [arg("-c"), arg("cat /proc/$$/stat")];
        let theirs = pgrp(&IsolatedRunner.output("sh", &args).unwrap());
        assert_ne!(ours, theirs);
        // The plain runner shares the group: the property is the wrapper's.
        let same = pgrp(&SystemRunner.output("sh", &args).unwrap());
        assert_eq!(ours, same);
    }

    #[test]
    fn results_map_like_the_system_runner() {
        assert_eq!(
            IsolatedRunner
                .output("sh", &[arg("-c"), arg("echo hi; echo no >&2")])
                .unwrap(),
            "hi\n"
        );
        match IsolatedRunner
            .run("sh", &[arg("-c"), arg("echo boom >&2; exit 3")])
            .unwrap_err()
        {
            BootError::Tool { status, stderr, .. } => {
                assert!(status.contains('3'), "{status}");
                assert_eq!(stderr, "boom");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            IsolatedRunner.run("tetsu-no-such-tool-xyz", &[]),
            Err(BootError::ToolMissing { .. })
        ));
        assert!(IsolatedRunner.check_tool("sh").is_ok());
        assert!(IsolatedRunner.check_tool("tetsu-no-such-tool-xyz").is_err());
    }
}
