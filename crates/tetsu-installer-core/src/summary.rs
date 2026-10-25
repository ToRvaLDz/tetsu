//! Summary step: every answer is shown, any step can be redone, and nothing
//! is written before the final confirmation.

use crate::answers::{Answers, Step};
use crate::driver::redo;
use crate::env::Env;
use crate::error::InstallError;
use crate::frontend::{Frontend, choose_from};
use crate::preflight::Preflight;

const INSTALL: &str = "Looks right: install";

/// Loop until the user confirms; returns the final answers.
pub fn run(
    fe: &mut dyn Frontend,
    env: &Env<'_>,
    pf: &Preflight,
    mut answers: Answers,
) -> Result<Answers, InstallError> {
    let mut options = vec![INSTALL.to_owned()];
    options.extend(Step::ALL.iter().map(|s| format!("Change: {}", s.label())));
    let refs: Vec<&str> = options.iter().map(String::as_str).collect();
    loop {
        fe.message(&answers.summary_lines().join("\n"));
        let i = choose_from(fe, "Summary", &refs)?;
        if i == 0 {
            let prompt = format!("Wipe {} and install?", answers.disk.name);
            if fe.confirm(&prompt)? {
                return Ok(answers);
            }
            continue;
        }
        answers = redo(Step::ALL[i - 1], fe, env, pf, answers)?;
    }
}
