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
