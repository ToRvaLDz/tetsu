//! Window rules: app_id and title patterns in, a decision out.

use tetsu_comp::rules::{Decision, Pattern, Rule, resolve};

fn pattern(source: &str) -> Pattern {
    Pattern::new(source).unwrap()
}

fn app(source: &str) -> Rule {
    Rule {
        app_id: Some(pattern(source)),
        ..Rule::default()
    }
}

fn title(source: &str) -> Rule {
    Rule {
        title: Some(pattern(source)),
        ..Rule::default()
    }
}

#[test]
fn an_app_id_rule_matches_on_the_app_id_only() {
    let rules = [Rule {
        floating: Some(true),
        ..app("^pavucontrol$")
    }];
    let hit = resolve(&rules, Some("pavucontrol"), Some("Volume"));
    assert_eq!(hit.floating, Some(true));
    assert_eq!(
        resolve(&rules, Some("foot"), Some("pavucontrol")),
        Decision::default()
    );
}

#[test]
fn a_title_rule_matches_on_the_title_only() {
    let rules = [Rule {
        workspace: Some(3),
        ..title("Picture-in-Picture")
    }];
    assert_eq!(
        resolve(
            &rules,
            Some("firefox"),
            Some("Firefox - Picture-in-Picture")
        )
        .workspace,
        Some(3)
    );
    assert_eq!(
        resolve(&rules, Some("Picture-in-Picture"), Some("x")),
        Decision::default()
    );
}

#[test]
fn both_matchers_must_match() {
    let rules = [Rule {
        title: Some(pattern("^Settings$")),
        floating: Some(true),
        ..app("^foot$")
    }];
    assert_eq!(
        resolve(&rules, Some("foot"), Some("Settings")).floating,
        Some(true)
    );
    assert_eq!(
        resolve(&rules, Some("foot"), Some("shell")),
        Decision::default()
    );
    assert_eq!(
        resolve(&rules, Some("kitty"), Some("Settings")),
        Decision::default()
    );
}

#[test]
fn patterns_are_unanchored_unless_asked() {
    let loose = [Rule {
        floating: Some(true),
        ..app("foot")
    }];
    assert_eq!(
        resolve(&loose, Some("footclient"), None).floating,
        Some(true)
    );
    let anchored = [Rule {
        floating: Some(true),
        ..app("^foot$")
    }];
    assert_eq!(
        resolve(&anchored, Some("footclient"), None),
        Decision::default()
    );
    assert_eq!(resolve(&anchored, Some("foot"), None).floating, Some(true));
}

#[test]
fn matching_rules_merge_in_order_and_the_later_one_wins_per_field() {
    let rules = [
        Rule {
            floating: Some(true),
            size: Some((800, 600)),
            workspace: Some(2),
            ..app("foot")
        },
        Rule {
            floating: Some(false),
            output: Some("HEADLESS-2".into()),
            ..app("^foot$")
        },
        Rule {
            workspace: Some(9),
            ..app("^other$")
        },
    ];
    assert_eq!(
        resolve(&rules, Some("foot"), None),
        Decision {
            floating: Some(false),
            workspace: Some(2),
            output: Some("HEADLESS-2".into()),
            size: Some((800, 600)),
        }
    );
}

#[test]
fn a_missing_field_never_matches_a_matcher() {
    let rules = [
        Rule {
            floating: Some(true),
            ..app(".*")
        },
        Rule {
            floating: Some(true),
            ..title("")
        },
    ];
    assert_eq!(resolve(&rules, None, None), Decision::default());
    // An empty title is a title, an absent one is not.
    assert_eq!(resolve(&rules, None, Some("")).floating, Some(true));
}

#[test]
fn a_rule_without_a_matcher_never_matches() {
    let rules = [Rule {
        floating: Some(true),
        ..Rule::default()
    }];
    assert_eq!(
        resolve(&rules, Some("foot"), Some("x")),
        Decision::default()
    );
}

#[test]
fn a_non_matching_rule_leaves_the_decision_empty() {
    let rules = [Rule {
        floating: Some(true),
        size: Some((1, 1)),
        ..app("^nothing$")
    }];
    assert_eq!(
        resolve(&rules, Some("foot"), Some("x")),
        Decision::default()
    );
    assert_eq!(resolve(&[], Some("foot"), Some("x")), Decision::default());
}

#[test]
fn a_bad_pattern_is_an_error_and_patterns_compare_by_source() {
    assert!(Pattern::new("(unclosed").is_err());
    assert_eq!(pattern("a+"), pattern("a+"));
    assert_ne!(pattern("a+"), pattern("a*"));
    assert_eq!(pattern("a+").as_str(), "a+");
}
