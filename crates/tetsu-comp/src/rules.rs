//! Window rules: app_id and title patterns decide floating, workspace,
//! output and size. They work the same in every layout. Niri semantics:
//! a rule matches when every matcher it sets matches, all matching rules
//! merge in order and the later one wins per field.

use regex_lite::Regex;

/// A compiled regex that compares by its source. Unanchored: use `^...$`.
#[derive(Debug, Clone)]
pub struct Pattern(Regex);

impl Pattern {
    /// Compiles a pattern.
    pub fn new(source: &str) -> Result<Self, regex_lite::Error> {
        Regex::new(source).map(Self)
    }

    /// The source text.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text)
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Pattern {}

/// One rule: matchers, then what to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    /// Matches the xdg app_id.
    pub app_id: Option<Pattern>,
    /// Matches the title.
    pub title: Option<Pattern>,
    /// Float (or tile) the window.
    pub floating: Option<bool>,
    /// Open on this workspace of the output, 1..=9.
    pub workspace: Option<u8>,
    /// Open on this output (by name).
    pub output: Option<String>,
    /// Configure the window to this size.
    pub size: Option<(i32, i32)>,
}

/// What the matching rules decided; `None` is "no opinion".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    /// Float or tile.
    pub floating: Option<bool>,
    /// Workspace number, 1..=9.
    pub workspace: Option<u8>,
    /// Output name.
    pub output: Option<String>,
    /// Size in pixels.
    pub size: Option<(i32, i32)>,
}

impl Rule {
    fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        if self.app_id.is_none() && self.title.is_none() {
            return false;
        }
        let ok = |pattern: &Option<Pattern>, text: Option<&str>| match pattern {
            None => true,
            Some(pattern) => text.is_some_and(|t| pattern.is_match(t)),
        };
        ok(&self.app_id, app_id) && ok(&self.title, title)
    }
}

/// Merges the rules that match, in order; the later one wins per field.
pub fn resolve(rules: &[Rule], app_id: Option<&str>, title: Option<&str>) -> Decision {
    let mut decision = Decision::default();
    for rule in rules.iter().filter(|r| r.matches(app_id, title)) {
        decision.floating = rule.floating.or(decision.floating);
        decision.workspace = rule.workspace.or(decision.workspace);
        decision.output = rule.output.clone().or(decision.output);
        decision.size = rule.size.or(decision.size);
    }
    decision
}
