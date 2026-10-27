use std::collections::HashSet;
use std::os::unix::net::UnixDatagram;

use super::*;

fn caller() -> Caller {
    Caller {
        unit: Some("tetsu-agent.service".into()),
        pid: 4242,
        exe: Some("/usr/bin/tetsu-agent".into()),
        comm: "tetsu-agent".into(),
        agent: true,
    }
}

fn entry(action: Action) -> AuditEntry {
    AuditEntry::new(action, Outcome::Ok, caller())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn an_add_entry_has_the_exact_native_protocol_bytes() {
    let mut e = entry(Action::Add);
    e.secret_id = Some("demo".into());
    let expected = format!(
        "MESSAGE=add secret demo: ok (tetsu-agent.service, pid 4242)\n\
         MESSAGE_ID={}\n\
         PRIORITY=6\n\
         SYSLOG_IDENTIFIER=tetsu-secrets\n\
         TETSU_ACTION=add\n\
         TETSU_OUTCOME=ok\n\
         TETSU_CALLER_UNIT=tetsu-agent.service\n\
         TETSU_CALLER_PID=4242\n\
         TETSU_CALLER_EXE=/usr/bin/tetsu-agent\n\
         TETSU_CALLER_COMM=tetsu-agent\n\
         TETSU_SECRET_ID=demo\n",
        Action::Add.message_id()
    );
    assert_eq!(text(&encode(&e)), expected);
}

#[test]
fn a_use_entry_carries_the_upstream_and_status() {
    let mut e = entry(Action::Use);
    e.secret_id = Some("openai".into());
    e.upstream = Some("api.openai.com".into());
    e.http_status = Some(200);
    let out = text(&encode(&e));
    assert!(out.contains("TETSU_UPSTREAM=api.openai.com\n"));
    assert!(out.contains("TETSU_HTTP_STATUS=200\n"));
    assert!(out.contains("TETSU_ACTION=use\n"));
}

#[test]
fn a_spawn_entry_carries_the_tool() {
    let mut e = entry(Action::Spawn);
    e.tool_id = Some("web-fetch".into());
    assert!(text(&encode(&e)).contains("TETSU_TOOL_ID=web-fetch\n"));
}

#[test]
fn every_action_has_its_own_message_id() {
    let mut seen = HashSet::new();
    let mut words = HashSet::new();
    for action in Action::ALL {
        let id = action.message_id();
        assert_eq!(id.len(), 32, "{action:?}");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
        assert!(seen.insert(id), "{action:?} repeats an id");
        assert!(words.insert(action.as_str()));
        let out = text(&encode(&entry(action)));
        assert!(out.contains(&format!("MESSAGE_ID={id}\n")));
        assert!(out.contains(&format!("TETSU_ACTION={}\n", action.as_str())));
        assert!(out.contains("SYSLOG_IDENTIFIER=tetsu-secrets\n"));
    }
}

#[test]
fn outcomes_set_the_priority() {
    let mut e = entry(Action::Denied);
    e.outcome = Outcome::Denied;
    let out = text(&encode(&e));
    assert!(out.contains("PRIORITY=4\n") && out.contains("TETSU_OUTCOME=denied\n"));
    e.outcome = Outcome::Failed;
    let out = text(&encode(&e));
    assert!(out.contains("PRIORITY=3\n") && out.contains("TETSU_OUTCOME=failed\n"));
}

#[test]
fn a_value_with_a_newline_uses_the_length_prefixed_form() {
    let mut e = entry(Action::Denied);
    e.reason = Some("line one\nTETSU_ACTION=forged".into());
    let bytes = encode(&e);
    let value = b"line one\nTETSU_ACTION=forged";
    let mut field = b"TETSU_REASON\n".to_vec();
    field.extend_from_slice(&(value.len() as u64).to_le_bytes());
    field.extend_from_slice(value);
    field.push(b'\n');
    assert!(bytes.windows(field.len()).any(|w| w == field.as_slice()));
    // The forged line is not a field of its own.
    assert!(
        !bytes
            .windows(26)
            .any(|w| w == b"\nTETSU_ACTION=forged\nPRIOR")
    );
}

#[test]
fn a_comm_with_a_newline_cannot_forge_a_field() {
    let mut e = entry(Action::Add);
    e.caller.comm = "x\nPRIORITY=0".into();
    let bytes = encode(&e);
    assert!(!text_lossy(&bytes).contains("\nPRIORITY=0\n"));
}

fn text_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_caller_without_a_unit_or_exe_omits_those_fields() {
    let mut e = entry(Action::List);
    e.caller.unit = None;
    e.caller.exe = None;
    let out = text(&encode(&e));
    assert!(!out.contains("TETSU_CALLER_UNIT"));
    assert!(!out.contains("TETSU_CALLER_EXE"));
    assert!(out.contains("MESSAGE=list: ok (tetsu-agent, pid 4242)\n"));
}

#[test]
fn the_journal_sink_sends_one_datagram_per_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sock");
    let socket = UnixDatagram::bind(&path).unwrap();
    let sink = JournalSink::new(&path);
    sink.record(&entry(Action::Add)).unwrap();
    sink.record(&entry(Action::Remove)).unwrap();
    let mut buf = vec![0u8; 8192];
    for expected in [Action::Add, Action::Remove] {
        let n = socket.recv(&mut buf).unwrap();
        assert_eq!(buf[..n], encode(&entry(expected))[..]);
    }
}

#[test]
fn a_dead_socket_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let sink = JournalSink::new(dir.path().join("nobody.sock"));
    assert!(sink.record(&entry(Action::Add)).is_err());
}
