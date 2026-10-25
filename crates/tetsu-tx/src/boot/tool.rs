//! External commands behind a trait, so argv construction is unit-tested
//! with a recording fake.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::Command;

use super::BootError;

/// Runs an external program to completion.
pub trait Runner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError>;
}

/// Runs programs found through `PATH` with `std::process::Command`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        todo!()
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
    fn absent_binary_is_tool_missing() {
        match SystemRunner.run("tetsu-no-such-tool-xyz", &[]).unwrap_err() {
            BootError::ToolMissing { tool } => assert_eq!(tool, "tetsu-no-such-tool-xyz"),
            other => panic!("{other:?}"),
        }
    }
}
