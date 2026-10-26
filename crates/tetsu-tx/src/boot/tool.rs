//! External commands behind a trait, so argv construction is unit-tested
//! with a recording fake.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use super::BootError;

/// Runs an external program to completion.
pub trait Runner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError>;

    /// Run to completion and return stdout. Fails like [`Runner::run`].
    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError>;

    /// Like [`Runner::output`], with `stdin` written to the tool and then
    /// closed, so a secret never sits in argv or in a file. The default
    /// refuses: a fake that does not model stdin must not drop the bytes.
    fn output_stdin(
        &self,
        program: &str,
        _args: &[OsString],
        _stdin: &[u8],
    ) -> Result<String, BootError> {
        Err(BootError::io(
            program,
            std::io::Error::new(ErrorKind::Unsupported, "this runner has no stdin"),
        ))
    }

    /// Fail with `ToolMissing` when `program` cannot be run. Lets callers
    /// check cheap preconditions before they change anything.
    fn check_tool(&self, _program: &str) -> Result<(), BootError> {
        Ok(())
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Runs programs found through `PATH` with `std::process::Command`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner {
    own_group: bool,
}

impl SystemRunner {
    /// Tools share the process group of the caller.
    pub const fn new() -> Self {
        Self { own_group: false }
    }

    /// Every tool leads a new process group. The terminal sends Ctrl+C to
    /// the foreground group only, so it reaches the caller and never kills
    /// a running tool halfway.
    pub const fn own_group() -> Self {
        Self { own_group: true }
    }

    fn command(&self, program: &str, args: &[OsString], env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(program);
        cmd.args(args).envs(env.iter().copied());
        if self.own_group {
            cmd.process_group(0);
        }
        cmd
    }

    /// Like [`Runner::output`], with extra environment variables, so a
    /// secret reaches the tool without sitting in argv. Values are never
    /// part of an error.
    pub fn output_env(
        &self,
        program: &str,
        args: &[OsString],
        env: &[(&str, &str)],
    ) -> Result<String, BootError> {
        self.capture(program, args, env, None)
    }

    /// Run `program` and return its stdout, or the mapped failure. `stdin`
    /// is written to the tool and closed; without it the tool reads nothing.
    fn capture(
        &self,
        program: &str,
        args: &[OsString],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<String, BootError> {
        let mut cmd = self.command(program, args, env);
        let out = match stdin {
            None => cmd.output(),
            Some(bytes) => Self::output_with_stdin(&mut cmd, bytes),
        };
        let out = match out {
            Ok(o) => o,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(BootError::ToolMissing {
                    tool: program.to_owned(),
                });
            }
            Err(e) => return Err(BootError::io(program, e)),
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

    /// `Command::output` with `bytes` on stdin. A thread feeds the pipe, so
    /// a tool that fills its own output before it reads cannot deadlock.
    fn output_with_stdin(cmd: &mut Command, bytes: &[u8]) -> std::io::Result<Output> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut pipe = child.stdin.take().expect("stdin is piped");
        std::thread::scope(|scope| {
            scope.spawn(move || {
                // A tool that exits without reading closes the pipe: its
                // status, not the broken pipe, tells what went wrong.
                let _ = pipe.write_all(bytes);
            });
            child.wait_with_output()
        })
    }
}

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.capture(program, args, &[], None).map(drop)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        self.capture(program, args, &[], None)
    }

    fn output_stdin(
        &self,
        program: &str,
        args: &[OsString],
        stdin: &[u8],
    ) -> Result<String, BootError> {
        self.capture(program, args, &[], Some(stdin))
    }

    fn check_tool(&self, program: &str) -> Result<(), BootError> {
        let found = if program.contains('/') {
            is_executable(Path::new(program))
        } else {
            std::env::var_os("PATH")
                .is_some_and(|p| std::env::split_paths(&p).any(|d| is_executable(&d.join(program))))
        };
        if found {
            Ok(())
        } else {
            Err(BootError::ToolMissing {
                tool: program.to_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_is_ok() {
        assert!(SystemRunner::new().run("true", &[]).is_ok());
    }

    #[test]
    fn failure_carries_tool_status_and_stderr() {
        let args = ["-c".into(), "echo boom >&2; exit 3".into()];
        match SystemRunner::new().run("sh", &args).unwrap_err() {
            BootError::Tool {
                tool,
                status,
                stderr,
            } => {
                assert_eq!(tool, "sh");
                assert!(status.contains('3'), "{status}");
                assert_eq!(stderr.trim(), "boom");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn check_tool_finds_present_and_flags_absent() {
        assert!(SystemRunner::new().check_tool("sh").is_ok());
        assert!(SystemRunner::new().check_tool("/bin/sh").is_ok());
        assert!(matches!(
            SystemRunner::new().check_tool("tetsu-no-such-tool-xyz"),
            Err(BootError::ToolMissing { .. })
        ));
    }

    #[test]
    fn output_returns_stdout() {
        let args = ["-c".into(), "echo hello; echo noise >&2".into()];
        assert_eq!(SystemRunner::new().output("sh", &args).unwrap(), "hello\n");
    }

    #[test]
    fn output_failure_carries_tool_status_and_stderr() {
        let args = ["-c".into(), "echo out; echo boom >&2; exit 4".into()];
        match SystemRunner::new().output("sh", &args).unwrap_err() {
            BootError::Tool {
                tool,
                status,
                stderr,
            } => {
                assert_eq!(tool, "sh");
                assert!(status.contains('4'), "{status}");
                assert_eq!(stderr, "boom");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn output_of_an_absent_binary_is_tool_missing() {
        assert!(matches!(
            SystemRunner::new().output("tetsu-no-such-tool-xyz", &[]),
            Err(BootError::ToolMissing { .. })
        ));
    }

    #[test]
    fn absent_binary_is_tool_missing() {
        match SystemRunner::new()
            .run("tetsu-no-such-tool-xyz", &[])
            .unwrap_err()
        {
            BootError::ToolMissing { tool } => assert_eq!(tool, "tetsu-no-such-tool-xyz"),
            other => panic!("{other:?}"),
        }
    }

    /// The process group in `/proc/<id>/stat` (field 5, after the command
    /// name, which may hold spaces and parentheses).
    fn pgrp(stat: &str) -> u32 {
        let after = stat.rsplit_once(')').unwrap().1;
        after.split_whitespace().nth(2).unwrap().parse().unwrap()
    }

    #[test]
    fn stdin_bytes_reach_the_tool() {
        let out = SystemRunner::new()
            .output_stdin("cat", &[], b"s3cret\n")
            .unwrap();
        assert_eq!(out, "s3cret\n");
    }

    #[test]
    fn stdin_failure_maps_like_output() {
        let args = ["-c".into(), "cat >/dev/null; echo boom >&2; exit 5".into()];
        match SystemRunner::new()
            .output_stdin("sh", &args, b"x")
            .unwrap_err()
        {
            BootError::Tool { status, stderr, .. } => {
                assert!(status.contains('5'), "{status}");
                assert_eq!(stderr, "boom");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            SystemRunner::new().output_stdin("tetsu-no-such-tool-xyz", &[], b"x"),
            Err(BootError::ToolMissing { .. })
        ));
    }

    #[test]
    fn a_tool_that_ignores_stdin_does_not_break_the_write() {
        let big = vec![b'x'; 1 << 20];
        assert!(SystemRunner::new().output_stdin("true", &[], &big).is_ok());
    }

    #[test]
    fn the_own_group_option_moves_the_tool_to_a_new_group() {
        let ours = pgrp(&std::fs::read_to_string("/proc/self/stat").unwrap());
        let args = ["-c".into(), "cat /proc/$$/stat".into()];
        let alone = SystemRunner::own_group().output("sh", &args).unwrap();
        let theirs = pgrp(&alone);
        assert_ne!(ours, theirs);
        // `process_group(0)` makes the child the leader of its group.
        let pid: u32 = alone.split_whitespace().next().unwrap().parse().unwrap();
        assert_eq!(theirs, pid);
        // The default shares the group of the caller.
        let same = pgrp(&SystemRunner::new().output("sh", &args).unwrap());
        assert_eq!(ours, same);
    }

    #[test]
    fn the_own_group_option_applies_to_stdin_runs_too() {
        let ours = pgrp(&std::fs::read_to_string("/proc/self/stat").unwrap());
        let args = ["-c".into(), "cat >/dev/null; cat /proc/$$/stat".into()];
        let out = SystemRunner::own_group()
            .output_stdin("sh", &args, b"x")
            .unwrap();
        assert_ne!(ours, pgrp(&out));
    }

    #[test]
    fn a_megabyte_on_stdin_round_trips_without_deadlock() {
        // `cat` echoes while it reads: a writer on the calling thread would
        // block on a full stdout pipe the caller never drains.
        let big = vec![b'x'; 1 << 20];
        let out = SystemRunner::new().output_stdin("cat", &[], &big).unwrap();
        assert_eq!(out.len(), 1 << 20);
    }

    #[test]
    fn env_vars_reach_the_tool() {
        let args = ["-c".into(), "printf %s \"$X\"".into()];
        let out = SystemRunner::new()
            .output_env("sh", &args, &[("X", "v4lue")])
            .unwrap();
        assert_eq!(out, "v4lue");
    }

    #[test]
    fn env_runs_honour_the_own_group_option() {
        let ours = pgrp(&std::fs::read_to_string("/proc/self/stat").unwrap());
        let args = ["-c".into(), "cat /proc/$$/stat".into()];
        let alone = SystemRunner::own_group()
            .output_env("sh", &args, &[("X", "1")])
            .unwrap();
        let pid: u32 = alone.split_whitespace().next().unwrap().parse().unwrap();
        assert_eq!(pgrp(&alone), pid);
        let same = SystemRunner::new()
            .output_env("sh", &args, &[("X", "1")])
            .unwrap();
        assert_eq!(pgrp(&same), ours);
    }

    #[test]
    fn env_failures_map_without_leaking_the_env() {
        let args = ["-c".into(), "echo boom >&2; exit 3".into()];
        let err = SystemRunner::new()
            .output_env("sh", &args, &[("NEWPIN", "s3cret-pin")])
            .unwrap_err();
        assert!(matches!(err, BootError::Tool { .. }));
        assert!(!format!("{err} {err:?}").contains("s3cret-pin"));
        assert!(matches!(
            SystemRunner::new().output_env("tetsu-no-such-tool-xyz", &[], &[("K", "s3cret-pin")]),
            Err(BootError::ToolMissing { .. })
        ));
    }

    #[test]
    fn the_default_trait_method_refuses_stdin() {
        struct NoStdin;
        impl Runner for NoStdin {
            fn run(&self, _: &str, _: &[OsString]) -> Result<(), BootError> {
                Ok(())
            }
            fn output(&self, _: &str, _: &[OsString]) -> Result<String, BootError> {
                Ok(String::new())
            }
        }
        assert!(matches!(
            NoStdin.output_stdin("cat", &[], b"x"),
            Err(BootError::Io { .. })
        ));
    }
}
