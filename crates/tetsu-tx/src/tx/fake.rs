//! Scripted command runner for the step tests: rules match on program plus
//! an argv prefix, every call is recorded, and a rule can run a side effect
//! (create a file) so stand-ins for ukify or mkinitcpio produce their output.

use std::cell::RefCell;
use std::ffi::OsString;

use crate::boot::BootError;
use crate::boot::tool::Runner;

/// What a matching call returns.
#[derive(Clone, Debug)]
pub(crate) enum Reply {
    /// Success with this stdout.
    Out(String),
    /// Failure with this stderr.
    Fail(String),
}

type Effect = Box<dyn Fn(&[String])>;

struct Rule {
    prefix: Vec<String>,
    reply: Reply,
    effect: Option<Effect>,
    /// Consumed by its first match.
    once: bool,
}

/// A runner that answers from a script. A call no rule matches succeeds
/// with empty stdout, so tests only script what they care about.
#[derive(Default)]
pub(crate) struct ScriptedRunner {
    rules: RefCell<Vec<Rule>>,
    calls: RefCell<Vec<Vec<String>>>,
    missing: RefCell<Vec<String>>,
}

impl ScriptedRunner {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, prefix: &[&str], reply: Reply, effect: Option<Effect>, once: bool) {
        self.rules.borrow_mut().push(Rule {
            prefix: prefix.iter().map(|s| (*s).to_owned()).collect(),
            reply,
            effect,
            once,
        });
    }

    /// Answer every call that starts with `prefix` (program first).
    pub fn on(&self, prefix: &[&str], reply: Reply) -> &Self {
        self.push(prefix, reply, None, false);
        self
    }

    /// Like [`ScriptedRunner::on`], for the first matching call only.
    pub fn on_once(&self, prefix: &[&str], reply: Reply) -> &Self {
        self.push(prefix, reply, None, true);
        self
    }

    /// Run `effect` with the full command line, then succeed.
    pub fn on_effect(&self, prefix: &[&str], effect: impl Fn(&[String]) + 'static) -> &Self {
        self.push(
            prefix,
            Reply::Out(String::new()),
            Some(Box::new(effect)),
            false,
        );
        self
    }

    /// `check_tool` fails for this program.
    pub fn without_tool(&self, program: &str) -> &Self {
        self.missing.borrow_mut().push(program.to_owned());
        self
    }

    /// Every call so far, as one space-joined string each.
    pub fn lines(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|c| c.join(" ")).collect()
    }

    fn answer(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        let mut line = vec![program.to_owned()];
        line.extend(args.iter().map(|a| a.to_string_lossy().into_owned()));
        self.calls.borrow_mut().push(line.clone());
        let (reply, run_effect) = {
            let mut rules = self.rules.borrow_mut();
            let Some(i) = rules.iter().position(|r| line.starts_with(&r.prefix)) else {
                return Ok(String::new());
            };
            let reply = rules[i].reply.clone();
            let effect = rules[i].effect.is_some();
            if rules[i].once {
                rules.remove(i);
                (reply, None)
            } else {
                (reply, effect.then_some(i))
            }
        };
        if let Some(i) = run_effect {
            let rules = self.rules.borrow();
            if let Some(effect) = &rules[i].effect {
                effect(&line);
            }
        }
        match reply {
            Reply::Out(out) => Ok(out),
            Reply::Fail(stderr) => Err(BootError::Tool {
                tool: program.to_owned(),
                status: "exit status: 1".to_owned(),
                stderr,
            }),
        }
    }
}

impl Runner for ScriptedRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.answer(program, args).map(drop)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        self.answer(program, args)
    }

    fn check_tool(&self, program: &str) -> Result<(), BootError> {
        if self.missing.borrow().iter().any(|m| m == program) {
            return Err(BootError::ToolMissing {
                tool: program.to_owned(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn matches_on_program_and_argv_prefix_and_records_calls() {
        let r = ScriptedRunner::new();
        r.on(&["pacman", "-Qi"], Reply::Out("vim 9\n".into()));
        assert_eq!(
            r.output("pacman", &args(&["-Qi", "vim"])).unwrap(),
            "vim 9\n"
        );
        assert_eq!(r.output("pacman", &args(&["-S", "vim"])).unwrap(), "");
        assert_eq!(r.output("other", &args(&["-Q"])).unwrap(), "");
        assert_eq!(r.lines(), ["pacman -Qi vim", "pacman -S vim", "other -Q"]);
    }

    #[test]
    fn first_matching_rule_wins_and_once_is_consumed() {
        let r = ScriptedRunner::new();
        r.on_once(&["x"], Reply::Fail("first".into()));
        r.on(&["x"], Reply::Out("later".into()));
        assert!(r.run("x", &[]).is_err());
        assert_eq!(r.output("x", &[]).unwrap(), "later");
    }

    #[test]
    fn failure_maps_to_a_tool_error_with_stderr() {
        let r = ScriptedRunner::new();
        r.on(&["pacman"], Reply::Fail("boom".into()));
        match r.run("pacman", &[]).unwrap_err() {
            BootError::Tool { tool, stderr, .. } => {
                assert_eq!(tool, "pacman");
                assert_eq!(stderr, "boom");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn effect_runs_with_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let target = out.clone();
        let r = ScriptedRunner::new();
        r.on_effect(&["mkinitcpio"], move |line| {
            std::fs::write(&target, line.join(" ")).unwrap();
        });
        r.run("mkinitcpio", &args(&["-k", "6.1"])).unwrap();
        assert_eq!(std::fs::read_to_string(out).unwrap(), "mkinitcpio -k 6.1");
    }

    #[test]
    fn missing_tools_fail_the_check() {
        let r = ScriptedRunner::new();
        r.without_tool("dkms");
        assert!(r.check_tool("pacman").is_ok());
        assert!(matches!(
            r.check_tool("dkms"),
            Err(BootError::ToolMissing { .. })
        ));
    }
}
