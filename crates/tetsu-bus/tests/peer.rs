//! Peer identity rules of the bus access policy (#28): the cgroup decides
//! whether a peer is the agent, and the procfs readings it rests on fail
//! closed.

use tetsu_bus::peer::{is_agent_cgroup, parse_fdinfo_pid, parse_proc_cgroup, unit_name};

const USER: &str = "/user.slice/user-1000.slice/user@1000.service";

#[test]
fn the_agent_service_is_an_agent_peer() {
    let path = format!("{USER}/tetsu.slice/tetsu-agent.slice/tetsu-agent.service");
    assert!(is_agent_cgroup(&path));
}

#[test]
fn a_transient_child_unit_of_the_agent_slice_is_an_agent_peer() {
    let path = format!("{USER}/tetsu.slice/tetsu-agent.slice/run-u42.service");
    assert!(is_agent_cgroup(&path));
}

#[test]
fn a_nested_slice_under_the_agent_slice_is_an_agent_peer() {
    let path = format!(
        "{USER}/tetsu.slice/tetsu-agent.slice/tetsu-agent-tools.slice/run-u7.scope"
    );
    assert!(is_agent_cgroup(&path));
}

#[test]
fn another_user_unit_is_not_an_agent_peer() {
    let path = format!("{USER}/app.slice/app-foot-1234.scope");
    assert!(!is_agent_cgroup(&path));
}

#[test]
fn a_unit_merely_named_like_the_agent_is_not_an_agent_peer() {
    let path = format!("{USER}/app.slice/tetsu-agent.service");
    assert!(!is_agent_cgroup(&path));
    let path = format!("{USER}/app.slice/not-tetsu-agent.slice/x.scope");
    assert!(!is_agent_cgroup(&path));
}

#[test]
fn the_agent_slice_anywhere_in_the_path_counts() {
    // Being stricter is safe: an agent peer only loses rights.
    assert!(is_agent_cgroup("/system.slice/tetsu-agent.slice/x.service"));
}

#[test]
fn cgroup_v2_line_is_read() {
    let content = format!("0::{USER}/app.slice/a.scope\n");
    assert_eq!(
        parse_proc_cgroup(&content).as_deref(),
        Some(format!("{USER}/app.slice/a.scope").as_str())
    );
}

#[test]
fn hybrid_hierarchy_uses_the_unified_line() {
    let content = format!("1:name=systemd:/legacy\n0::{USER}/a.scope\n");
    assert_eq!(
        parse_proc_cgroup(&content).as_deref(),
        Some(format!("{USER}/a.scope").as_str())
    );
}

#[test]
fn missing_or_relative_cgroup_fails_closed() {
    assert_eq!(parse_proc_cgroup(""), None);
    assert_eq!(parse_proc_cgroup("1:cpu:/x\n"), None);
    assert_eq!(parse_proc_cgroup("0::relative\n"), None);
}

#[test]
fn fdinfo_pid_is_read() {
    let content = "pos:\t0\nflags:\t02000002\nmnt_id:\t15\nino:\t1066\nPid:\t4242\nNSpid:\t4242\n";
    assert_eq!(parse_fdinfo_pid(content), Some(4242));
}

#[test]
fn an_exited_or_unreadable_pidfd_has_no_pid() {
    assert_eq!(parse_fdinfo_pid("Pid:\t-1\n"), None);
    assert_eq!(parse_fdinfo_pid("Pid:\t0\n"), None);
    assert_eq!(parse_fdinfo_pid("flags:\t0\n"), None);
    assert_eq!(parse_fdinfo_pid("Pid:\tx\n"), None);
}

#[test]
fn unit_name_is_the_innermost_service_or_scope() {
    let path = format!("{USER}/tetsu.slice/tetsu-agent.slice/tetsu-agent.service");
    assert_eq!(unit_name(&path), Some("tetsu-agent.service"));
    let path = format!("{USER}/app.slice/app-foot-1234.scope");
    assert_eq!(unit_name(&path), Some("app-foot-1234.scope"));
    assert_eq!(unit_name("/user.slice"), None);
}
