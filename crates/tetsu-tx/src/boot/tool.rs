//! External commands behind a trait, so argv construction is unit-tested
//! with a recording fake.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use super::BootError;

/// Runs an external program to completion.
pub trait Runner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError>;

    /// Run to completion and return stdout. Fails like [`Runner::run`].
    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError>;

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
pub struct SystemRunner;

impl SystemRunner {
    /// Run `program` and return its stdout, or the mapped failure.
    fn capture(program: &str, args: &[OsString]) -> Result<String, BootError> {
        let out = match Command::new(program).args(args).output() {
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
}

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        Self::capture(program, args).map(drop)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        Self::capture(program, args)
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
        assert!(SystemRunner.run("true", &[]).is_ok());
    }

    #[test]
    fn failure_carries_tool_status_and_stderr() {
        let args = ["-c".into(), "echo boom >&2; exit 3".into()];
        match SystemRunner.run("sh", &args).unwrap_err() {
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
        assert!(SystemRunner.check_tool("sh").is_ok());
        assert!(SystemRunner.check_tool("/bin/sh").is_ok());
        assert!(matches!(
            SystemRunner.check_tool("tetsu-no-such-tool-xyz"),
            Err(BootError::ToolMissing { .. })
        ));
    }

    #[test]
    fn output_returns_stdout() {
        let args = ["-c".into(), "echo hello; echo noise >&2".into()];
        assert_eq!(SystemRunner.output("sh", &args).unwrap(), "hello\n");
    }

    #[test]
    fn output_failure_carries_tool_status_and_stderr() {
        let args = ["-c".into(), "echo out; echo boom >&2; exit 4".into()];
        match SystemRunner.output("sh", &args).unwrap_err() {
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
            SystemRunner.output("tetsu-no-such-tool-xyz", &[]),
            Err(BootError::ToolMissing { .. })
        ));
    }

    #[test]
    fn absent_binary_is_tool_missing() {
        match SystemRunner.run("tetsu-no-such-tool-xyz", &[]).unwrap_err() {
            BootError::ToolMissing { tool } => assert_eq!(tool, "tetsu-no-such-tool-xyz"),
            other => panic!("{other:?}"),
        }
    }
}
