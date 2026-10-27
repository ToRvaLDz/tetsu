//! The command line: pure argument parsing and the text it prints.

use serde_json::json;
use tetsu_agent::cli::{self, Command, USAGE_EXIT};
use tetsu_agent::contract;
use tetsu_agent::discovery::{Catalog, Route};
use tetsu_agent::runner::Outcome;
use tetsu_agent::tools;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn no_subcommand_is_the_rules_only_notice() {
    assert_eq!(cli::parse(&args(&[])).unwrap(), Command::Idle);
}

#[test]
fn tools_takes_an_optional_json_flag() {
    assert_eq!(
        cli::parse(&args(&["tools"])).unwrap(),
        Command::Tools { json: false }
    );
    assert_eq!(
        cli::parse(&args(&["tools", "--json"])).unwrap(),
        Command::Tools { json: true }
    );
    assert!(cli::parse(&args(&["tools", "--nope"])).is_err());
    assert!(cli::parse(&args(&["tools", "extra"])).is_err());
}

#[test]
fn call_takes_a_tool_optional_json_and_a_reason() {
    let call = cli::parse(&args(&["call", "org.tetsu.notify.v1.Show"])).unwrap();
    assert_eq!(
        call,
        Command::Call {
            tool: "org.tetsu.notify.v1.Show".to_owned(),
            input: json!({}),
        }
    );
    let call = cli::parse(&args(&[
        "call",
        "org.tetsu.notify.v1.Show",
        "{\"title\":\"hi\"}",
        "--reason",
        "UAT",
    ]))
    .unwrap();
    assert_eq!(
        call,
        Command::Call {
            tool: "org.tetsu.notify.v1.Show".to_owned(),
            input: json!({ "title": "hi", "_reason": "UAT" }),
        }
    );
    // The flag may come first.
    let call = cli::parse(&args(&["call", "--reason", "UAT", "a.b.v1.C"])).unwrap();
    assert_eq!(
        call,
        Command::Call {
            tool: "a.b.v1.C".to_owned(),
            input: json!({ "_reason": "UAT" }),
        }
    );
}

#[test]
fn bad_usage_is_an_error() {
    for bad in [
        vec!["call"],
        vec!["call", "a.b.v1.C", "not json"],
        vec!["call", "a.b.v1.C", "{}", "{}"],
        vec!["call", "a.b.v1.C", "--reason"],
        vec!["call", "a.b.v1.C", "[1]", "--reason", "x"],
        vec!["frobnicate"],
    ] {
        assert!(cli::parse(&args(&bad)).is_err(), "{bad:?}");
    }
    assert_eq!(USAGE_EXIT, 64);
}

#[test]
fn the_tool_listing_names_id_kind_class_and_destructive() {
    let all = tools::build(&contract::load().unwrap()).unwrap();
    let catalog = Catalog {
        tools: all
            .into_iter()
            .filter(|t| t.interface == "org.tetsu.tx.v1")
            .collect(),
        routes: [("org.tetsu.tx.v1".to_owned(), Route::Scanned("/s".into()))].into(),
        skipped: Vec::new(),
    };
    let text = cli::render_tools(&catalog, false);
    let update = text
        .lines()
        .find(|l| l.starts_with("org.tetsu.tx.v1.Update"))
        .unwrap();
    assert!(
        update.contains("gated")
            && update.contains("system.update")
            && update.contains("destructive")
    );
    let status = text
        .lines()
        .find(|l| l.starts_with("org.tetsu.tx.v1.GetStatus"))
        .unwrap();
    assert!(status.contains("free") && !status.contains("destructive"));
    let json: serde_json::Value = serde_json::from_str(&cli::render_tools(&catalog, true)).unwrap();
    assert!(
        json["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "org.tetsu.tx.v1.Update" && t["destructive"] == true)
    );
}

#[test]
fn an_outcome_prints_as_its_kind() {
    assert_eq!(cli::render_outcome(&Outcome::Denied), "denied");
    assert_eq!(
        cli::render_outcome(&Outcome::Ok(json!({ "a": 1 }))),
        "ok {\"a\":1}"
    );
    assert!(cli::render_outcome(&Outcome::Unavailable("x".to_owned())).starts_with("unavailable"));
    let error = Outcome::Error {
        error: "e.v1.E".to_owned(),
        parameters: json!({}),
    };
    assert_eq!(cli::render_outcome(&error), "error e.v1.E {}");
    assert_eq!(cli::exit_code(&Outcome::Ok(json!({}))), 0);
    assert_eq!(cli::exit_code(&Outcome::Denied), 1);
}
