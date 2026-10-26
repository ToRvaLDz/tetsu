//! Running a tool with environment variables, which [`Runner`] cannot do.
//! The Balanced PIN reaches `systemd-cryptenroll` as `NEWPIN`, never in
//! argv.

use std::ffi::OsString;

use tetsu_tx::boot::BootError;
use tetsu_tx::boot::tool::{Runner, SystemRunner};

/// A [`Runner`] that can also pass environment variables.
pub trait SealRunner: Runner {
    fn run_env(
        &self,
        program: &str,
        args: &[OsString],
        env: &[(&str, &str)],
    ) -> Result<(), BootError>;
}

impl SealRunner for SystemRunner {
    fn run_env(
        &self,
        program: &str,
        args: &[OsString],
        env: &[(&str, &str)],
    ) -> Result<(), BootError> {
        self.output_env(program, args, env).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_reaches_the_tool_and_failures_are_mapped() {
        let ok: [OsString; 2] = ["-c".into(), "test \"$NEWPIN\" = 1234".into()];
        assert!(
            SystemRunner::new()
                .run_env("sh", &ok, &[("NEWPIN", "1234")])
                .is_ok()
        );
        assert!(
            SystemRunner::new()
                .run_env("sh", &ok, &[("NEWPIN", "9")])
                .is_err()
        );
        let boom: [OsString; 2] = ["-c".into(), "echo boom >&2; exit 3".into()];
        match SystemRunner::new().run_env("sh", &boom, &[]).unwrap_err() {
            BootError::Tool { stderr, .. } => assert_eq!(stderr, "boom"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            SystemRunner::new().run_env("tetsu-no-such-tool-xyz", &[], &[]),
            Err(BootError::ToolMissing { .. })
        ));
    }
}
