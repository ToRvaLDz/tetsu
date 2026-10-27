//! The access policy of the facade (#28): which calls are gated, where each
//! one is routed, and how a refusal is named and logged.

use serde_json::json;
use tetsu_bus::access::{
    MethodKind, Route, Scope, log_line, method_kind, permission_denied, route,
};
use tetsu_bus::peer::Peer;

const OWN: u32 = 1000;
const USER: &str = "/user.slice/user-1000.slice/user@1000.service";

fn peer(uid: u32, agent: bool) -> Peer {
    let cgroup = if agent {
        format!("{USER}/tetsu.slice/tetsu-agent.slice/run-u1.service")
    } else {
        format!("{USER}/app.slice/app-foot-1.scope")
    };
    Peer {
        uid,
        gid: uid,
        pid: 4242,
        cgroup,
        agent,
        comm: "x".to_owned(),
    }
}

// method_kind

#[test]
fn reads_and_the_varlink_service_are_free() {
    for method in [
        "org.tetsu.tx.v1.GetStatus",
        "org.tetsu.tx.v1.ListTransactions",
        "org.tetsu.cap.v1.WatchCapabilities",
        "org.tetsu.notify.v1.Get",
        "org.varlink.service.GetInfo",
        "org.varlink.service.GetInterfaceDescription",
        "org.varlink.service.Anything",
    ] {
        assert_eq!(method_kind(method), MethodKind::Free, "{method}");
    }
}

#[test]
fn everything_else_is_gated() {
    for method in [
        "org.tetsu.tx.v1.Getaway",
        "org.tetsu.tx.v1.Listen",
        "org.tetsu.tx.v1.Watcher",
        "org.tetsu.tx.v1.Install",
        "org.tetsu.notify.v1.Show",
        "org.tetsu.tx.v1.getStatus",
        "org.tetsu.tx.v1.Unknown",
        "org.varlinkx.service.Install",
        "GetStatus",
        "Install",
        "",
        "org.tetsu.tx.v1.",
    ] {
        assert_eq!(method_kind(method), MethodKind::Gated, "{method:?}");
    }
}

// route

#[test]
fn a_session_peer_of_another_user_is_denied_even_on_a_read() {
    for kind in [MethodKind::Free, MethodKind::Gated] {
        for agent in [false, true] {
            let found = route(Scope::Session, &peer(1001, agent), kind, OWN, true);
            assert_eq!(found, Route::Deny, "{kind:?} agent={agent}");
        }
    }
}

#[test]
fn a_session_peer_of_the_same_user_or_root_passes_unless_it_is_the_agent() {
    for uid in [OWN, 0] {
        for kind in [MethodKind::Free, MethodKind::Gated] {
            let found = route(Scope::Session, &peer(uid, false), kind, OWN, false);
            assert_eq!(found, Route::Pass, "uid {uid} {kind:?}");
        }
    }
}

#[test]
fn a_session_agent_is_asked_to_capd_with_the_session_owner() {
    let found = route(
        Scope::Session,
        &peer(OWN, true),
        MethodKind::Gated,
        OWN,
        false,
    );
    assert_eq!(found, Route::AskCapd { uid: OWN });
}

#[test]
fn a_session_agent_reads_freely_without_capd() {
    let found = route(
        Scope::Session,
        &peer(OWN, true),
        MethodKind::Free,
        OWN,
        false,
    );
    assert_eq!(found, Route::Pass);
}

#[test]
fn a_system_read_is_free_for_everyone() {
    for uid in [0, OWN, 1001] {
        for agent in [false, true] {
            for wheel in [false, true] {
                let found = route(
                    Scope::System,
                    &peer(uid, agent),
                    MethodKind::Free,
                    OWN,
                    wheel,
                );
                assert_eq!(found, Route::Pass, "uid {uid} agent {agent} wheel {wheel}");
            }
        }
    }
}

#[test]
fn a_system_agent_outside_wheel_is_denied_without_asking_anyone() {
    let found = route(
        Scope::System,
        &peer(OWN, true),
        MethodKind::Gated,
        OWN,
        false,
    );
    assert_eq!(found, Route::Deny);
}

#[test]
fn a_system_agent_in_wheel_goes_to_capd_of_its_own_uid() {
    let found = route(
        Scope::System,
        &peer(1234, true),
        MethodKind::Gated,
        OWN,
        true,
    );
    assert_eq!(found, Route::AskCapd { uid: 1234 });
}

#[test]
fn a_system_non_agent_goes_to_the_authority_never_to_capd() {
    for uid in [0, OWN, 1001] {
        for wheel in [false, true] {
            let found = route(
                Scope::System,
                &peer(uid, false),
                MethodKind::Gated,
                OWN,
                wheel,
            );
            assert_eq!(found, Route::AskAuthority, "uid {uid} wheel {wheel}");
        }
    }
}

// permission_denied

#[test]
fn the_refusal_is_the_permission_denied_of_the_interface() {
    let error = permission_denied("org.tetsu.tx.v1.Install");
    assert_eq!(error.error, "org.tetsu.tx.v1.PermissionDenied");
    assert_eq!(error.parameters, json!({}));
}

#[test]
fn a_method_without_an_interface_is_refused_with_the_varlink_service_name() {
    for method in ["Install", ""] {
        let error = permission_denied(method);
        assert_eq!(
            error.error, "org.varlink.service.PermissionDenied",
            "{method:?}"
        );
        assert_eq!(error.parameters, json!({}));
    }
}

// log_line

#[test]
fn a_refusal_line_names_the_method_the_peer_and_the_reason() {
    let line = log_line("org.tetsu.tx.v1.Install", &peer(OWN, true), "capd said off");
    assert_eq!(
        line,
        "tetsu-bus: refused org.tetsu.tx.v1.Install from pid 4242 (run-u1.service): capd said off"
    );
}

#[test]
fn control_characters_of_the_method_cannot_forge_a_line() {
    let line = log_line("a\nb\r\x1b[31mc\0d\u{85}", &peer(OWN, true), "why");
    assert!(!line.contains(char::is_control), "{line:?}");
    assert!(line.contains("refused ab[31mcd from"), "{line:?}");
}

#[test]
fn a_long_method_is_truncated_with_a_marker() {
    let method = "m".repeat(5000);
    let line = log_line(&method, &peer(OWN, true), "why");
    let kept = "m".repeat(200);
    assert!(line.contains(&format!("refused {kept}... from")), "{line}");
    assert!(line.len() < 400, "{}", line.len());
}

#[test]
fn a_method_of_exactly_the_limit_is_not_marked() {
    let method = "m".repeat(200);
    let line = log_line(&method, &peer(OWN, true), "why");
    assert!(line.contains(&format!("refused {method} from")), "{line}");
}

#[test]
fn a_peer_without_a_unit_is_named_by_its_process() {
    let mut p = peer(OWN, false);
    p.cgroup = "/user.slice".to_owned();
    let line = log_line("x.Y", &p, "why");
    assert!(line.contains("from pid 4242 (x)"), "{line}");
}

// wheel membership

mod wheel {
    use std::fs;

    use tetsu_bus::access::{EtcFiles, Groups, in_wheel};

    const PASSWD: &str = "\
root:x:0:0:root:/root:/bin/bash
alice:x:1000:1000:Alice:/home/alice:/bin/fish
bob:x:1001:1001:Bob:/home/bob:/bin/fish
carol:x:1002:10:Carol:/home/carol:/bin/fish
";

    #[test]
    fn a_user_in_the_member_list_is_in_wheel() {
        let group = "alice:x:1000:\nwheel:x:10:alice,dave\n";
        assert!(in_wheel(PASSWD, group, 1000));
    }

    #[test]
    fn a_user_whose_primary_group_is_wheel_is_in_wheel() {
        let group = "wheel:x:10:\n";
        assert!(in_wheel(PASSWD, group, 1002));
    }

    #[test]
    fn a_user_in_neither_is_not_in_wheel() {
        let group = "wheel:x:10:alice\nbob:x:1001:\n";
        assert!(!in_wheel(PASSWD, group, 1001));
    }

    #[test]
    fn a_member_name_must_match_whole() {
        let group = "wheel:x:10:alice2,xbob,bobby\n";
        assert!(!in_wheel(PASSWD, group, 1001));
        assert!(!in_wheel(PASSWD, group, 1000));
    }

    #[test]
    fn without_a_wheel_group_nobody_is_in_wheel() {
        let group = "alice:x:1000:alice\nwheelers:x:10:alice\n";
        assert!(!in_wheel(PASSWD, group, 1000));
        assert!(!in_wheel(PASSWD, "", 1002));
    }

    #[test]
    fn malformed_lines_are_skipped_and_the_decision_stays_right() {
        let passwd = format!("garbage\n::::\nx:x:notanumber:5:\n{PASSWD}short:x:1\n");
        let group = "nonsense\nwheel:x:abc:alice\nwheel:x:10:alice\n:::\n";
        assert!(in_wheel(&passwd, group, 1000));
        assert!(!in_wheel(&passwd, group, 1001));
    }

    #[test]
    fn an_unknown_uid_is_not_in_wheel() {
        let group = "wheel:x:10:alice,ghost\n";
        assert!(!in_wheel(PASSWD, group, 4000));
    }

    #[test]
    fn a_wheel_line_with_an_unreadable_gid_does_not_match_by_group() {
        // Only the member list could match; the primary gid cannot.
        let group = "wheel:x:oops:\n";
        assert!(!in_wheel(PASSWD, group, 1002));
    }

    fn etc(passwd: Option<&str>, group: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("etc")).unwrap();
        if let Some(text) = passwd {
            fs::write(dir.path().join("etc/passwd"), text).unwrap();
        }
        if let Some(text) = group {
            fs::write(dir.path().join("etc/group"), text).unwrap();
        }
        dir
    }

    #[test]
    fn etc_files_reads_both_files_under_its_root() {
        let dir = etc(Some(PASSWD), Some("wheel:x:10:alice\n"));
        let groups = EtcFiles::new(dir.path());
        assert!(groups.in_wheel(1000));
        assert!(!groups.in_wheel(1001));
    }

    #[test]
    fn missing_files_mean_not_in_wheel() {
        for dir in [
            etc(None, Some("wheel:x:10:alice\n")),
            etc(Some(PASSWD), None),
            etc(None, None),
        ] {
            assert!(!EtcFiles::new(dir.path()).in_wheel(1000));
        }
        assert!(!EtcFiles::new("/nonexistent-root").in_wheel(1000));
    }

    #[test]
    fn unreadable_text_means_not_in_wheel() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("etc")).unwrap();
        fs::write(dir.path().join("etc/passwd"), [0xff, 0xfe, b'\n']).unwrap();
        fs::write(dir.path().join("etc/group"), "wheel:x:10:alice\n").unwrap();
        assert!(!EtcFiles::new(dir.path()).in_wheel(1000));
    }
}

// expected servers

mod servers {
    use super::{USER, peer};
    use tetsu_bus::access::{ExpectedServer, capd_path};
    use tetsu_bus::peer::Peer;

    fn capd(uid: u32) -> ExpectedServer {
        ExpectedServer::UserUnit {
            uid,
            unit: "tetsu-capd.service".to_owned(),
        }
    }

    fn at(uid: u32, cgroup: &str) -> Peer {
        Peer {
            cgroup: cgroup.to_owned(),
            ..peer(uid, false)
        }
    }

    #[test]
    fn the_unit_under_the_users_manager_matches() {
        let cgroup = format!("{USER}/session.slice/tetsu-capd.service");
        assert!(capd(1000).matches(&at(1000, &cgroup)));
        let cgroup = format!("{USER}/tetsu-capd.service");
        assert!(capd(1000).matches(&at(1000, &cgroup)));
    }

    #[test]
    fn another_uid_does_not_match() {
        let cgroup = format!("{USER}/session.slice/tetsu-capd.service");
        assert!(!capd(1000).matches(&at(1001, &cgroup)));
        assert!(!capd(1001).matches(&at(1000, &cgroup)));
    }

    #[test]
    fn the_same_unit_under_another_users_manager_does_not_match() {
        let other =
            "/user.slice/user-1001.slice/user@1001.service/session.slice/tetsu-capd.service";
        assert!(!capd(1000).matches(&at(1000, other)));
        assert!(!capd(1000).matches(&at(1001, other)));
    }

    #[test]
    fn a_unit_merely_containing_the_name_does_not_match() {
        for name in [
            "x-tetsu-capd.service",
            "tetsu-capd.service.d",
            "tetsu-capd.scope",
        ] {
            let cgroup = format!("{USER}/session.slice/{name}");
            assert!(!capd(1000).matches(&at(1000, &cgroup)), "{name}");
        }
    }

    #[test]
    fn a_child_unit_inside_the_service_does_not_match() {
        let cgroup = format!("{USER}/session.slice/tetsu-capd.service/run-u1.scope");
        assert!(!capd(1000).matches(&at(1000, &cgroup)));
    }

    #[test]
    fn a_system_unit_or_a_session_scope_does_not_match() {
        assert!(!capd(1000).matches(&at(1000, "/system.slice/tetsu-capd.service")));
        let session = "/user.slice/user-1000.slice/session-2.scope";
        assert!(!capd(1000).matches(&at(1000, session)));
        let not_the_manager = "/user.slice/user-1000.slice/user@1000.service.d/tetsu-capd.service";
        assert!(!capd(1000).matches(&at(1000, not_the_manager)));
    }

    #[test]
    fn dot_segments_do_not_match() {
        let cgroup = format!("{USER}/../../../system.slice/tetsu-capd.service");
        assert!(!capd(1000).matches(&at(1000, &cgroup)));
    }

    #[test]
    fn an_agent_peer_never_matches() {
        let cgroup = format!("{USER}/tetsu.slice/tetsu-agent.slice/tetsu-capd.service");
        let mut agent = at(1000, &cgroup);
        agent.agent = true;
        assert!(!capd(1000).matches(&agent));
        // The cgroup alone is enough, whatever the flag says.
        agent.agent = false;
        assert!(!capd(1000).matches(&agent));
    }

    #[test]
    fn a_cgroup_expectation_is_an_equality() {
        let cgroup = format!("{USER}/session.slice/tetsu-capd.service");
        let expected = ExpectedServer::Cgroup(cgroup.clone());
        assert!(expected.matches(&at(1000, &cgroup)));
        assert!(!expected.matches(&at(1000, &format!("{cgroup}/x"))));
        assert!(!expected.matches(&at(1000, "/system.slice/tetsu-capd.service")));
    }

    #[test]
    fn capd_lives_at_a_fixed_path_per_user() {
        assert_eq!(
            capd_path(1000),
            std::path::PathBuf::from("/run/user/1000/tetsu/org.tetsu.cap.v1")
        );
        assert_eq!(
            capd_path(0),
            std::path::PathBuf::from("/run/user/0/tetsu/org.tetsu.cap.v1")
        );
    }
}

#[test]
fn control_characters_of_the_reason_cannot_forge_a_line_either() {
    let line = log_line("x.Y", &peer(OWN, true), "no\nFORGED\x1b[0m");
    assert!(!line.contains(char::is_control), "{line:?}");
    assert!(line.ends_with(": noFORGED[0m"), "{line:?}");
}
