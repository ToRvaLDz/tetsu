//! The interface every installer frontend implements. The core never
//! prints or reads a terminal itself.

use crate::error::InstallError;

/// Validation callback: `Err` carries the message shown to the user.
pub type Validate<'a> = &'a dyn Fn(&str) -> Result<(), String>;

/// Interaction surface of the installer. Object-safe, so steps take
/// `&mut dyn Frontend`.
pub trait Frontend {
    /// Ask for a line of text. Implementations should re-prompt while
    /// `validate` fails; the core re-validates the result regardless.
    fn ask(&mut self, prompt: &str, validate: Validate<'_>) -> Result<String, InstallError>;

    /// Ask for a secret without echoing it.
    fn ask_secret(&mut self, prompt: &str) -> Result<String, InstallError>;

    /// Pick one of `options`; returns its index.
    fn choose(&mut self, prompt: &str, options: &[String]) -> Result<usize, InstallError>;

    /// Ask a yes/no question.
    fn confirm(&mut self, prompt: &str) -> Result<bool, InstallError>;

    /// Report progress of `step`.
    fn progress(&mut self, step: &str, msg: &str);

    /// Show plain information or a validation message.
    fn message(&mut self, text: &str);

    /// Show `payload` as a QR code.
    fn show_qr(&mut self, payload: &str) -> Result<(), InstallError>;
}

/// Ask until `validate` accepts the answer, reporting each rejection.
pub fn ask_valid(
    fe: &mut dyn Frontend,
    prompt: &str,
    validate: Validate<'_>,
) -> Result<String, InstallError> {
    loop {
        let answer = fe.ask(prompt, validate)?;
        match validate(&answer) {
            Ok(()) => return Ok(answer),
            Err(msg) => fe.message(&msg),
        }
    }
}

/// Pick from `options` given as string slices.
pub fn choose_from(
    fe: &mut dyn Frontend,
    prompt: &str,
    options: &[&str],
) -> Result<usize, InstallError> {
    let owned: Vec<String> = options.iter().map(|s| (*s).to_owned()).collect();
    loop {
        let index = fe.choose(prompt, &owned)?;
        if index < owned.len() {
            return Ok(index);
        }
        fe.message("invalid choice");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Script;

    fn not_empty(s: &str) -> Result<(), String> {
        if s.is_empty() {
            Err("empty".into())
        } else {
            Ok(())
        }
    }

    #[test]
    fn ask_valid_repeats_until_accepted() {
        let mut fe = Script::new().text("").text("ok");
        assert_eq!(ask_valid(&mut fe, "p", &not_empty).unwrap(), "ok");
        assert_eq!(fe.messages, ["empty"]);
    }

    #[test]
    fn choose_from_rejects_out_of_range_index() {
        let mut fe = Script::new().pick(5).pick(1);
        assert_eq!(choose_from(&mut fe, "p", &["a", "b"]).unwrap(), 1);
        assert_eq!(fe.messages, ["invalid choice"]);
    }

    #[test]
    fn frontend_is_object_safe_and_scripted() {
        let mut script = Script::new().secret("pw").yes(true);
        let fe: &mut dyn Frontend = &mut script;
        assert_eq!(fe.ask_secret("s").unwrap(), "pw");
        assert!(fe.confirm("c").unwrap());
        fe.progress("step", "msg");
        fe.show_qr("payload").unwrap();
        assert_eq!(script.qrs, ["payload"]);
        assert_eq!(script.progress, [("step".to_owned(), "msg".to_owned())]);
    }
}
