//! The journal entries: encoding and the datagram.

use std::collections::BTreeMap;
use std::os::unix::net::UnixDatagram;
use std::time::Duration;

use tetsu_capd::audit::{AuditEntry, AuditSink, Event, JournalSink, RecordingSink};

/// Parses the native protocol: `KEY=value\n`, or `KEY\n` + u64 LE length +
/// value + `\n`. Every field is kept, in order.
fn parse(mut data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut fields = Vec::new();
    while !data.is_empty() {
        let nl = data.iter().position(|b| *b == b'\n').expect("a newline");
        let line = &data[..nl];
        if let Some(eq) = line.iter().position(|b| *b == b'=') {
            let key = String::from_utf8(line[..eq].to_vec()).unwrap();
            fields.push((key, line[eq + 1..].to_vec()));
            data = &data[nl + 1..];
        } else {
            let key = String::from_utf8(line.to_vec()).unwrap();
            let len = u64::from_le_bytes(data[nl + 1..nl + 9].try_into().unwrap()) as usize;
            let value = data[nl + 9..nl + 9 + len].to_vec();
            assert_eq!(
                data[nl + 9 + len],
                b'\n',
                "framed value ends with a newline"
            );
            fields.push((key, value));
            data = &data[nl + 10 + len..];
        }
    }
    fields
}

fn map(data: &[u8]) -> BTreeMap<String, String> {
    parse(data)
        .into_iter()
        .map(|(k, v)| (k, String::from_utf8(v).unwrap()))
        .collect()
}

fn full() -> AuditEntry {
    let mut entry = AuditEntry::new(Event::Check);
    entry.method = Some("org.tetsu.tx.v1.Install".into());
    entry.class = Some("packages.install".into());
    entry.scope = Some("work".into());
    entry.state = Some("ask".into());
    entry.layer = Some("managed".into());
    entry.decision = Some("deny".into());
    entry.denied_by = Some("user".into());
    entry.cancelled = true;
    entry.peer_pid = Some(4242);
    entry.peer_unit = Some("tetsu-agent.service".into());
    entry.caller_unit = Some("tetsu-tx.service".into());
    entry.args = Some("{\"packages\":[\"htop\"]}".into());
    entry.reason = Some("to build".into());
    entry
}

#[test]
fn plain_fields_are_encoded_as_key_value_lines() {
    let fields = map(&full().encode());
    let expect = [
        ("SYSLOG_IDENTIFIER", "tetsu-capd"),
        ("TETSU_METHOD", "org.tetsu.tx.v1.Install"),
        ("TETSU_CLASS", "packages.install"),
        ("TETSU_SCOPE", "work"),
        ("TETSU_STATE", "ask"),
        ("TETSU_LAYER", "managed"),
        ("TETSU_DECISION", "deny"),
        ("TETSU_DENIED_BY", "user"),
        ("TETSU_CANCELLED", "true"),
        ("TETSU_PEER_PID", "4242"),
        ("TETSU_PEER_UNIT", "tetsu-agent.service"),
        ("TETSU_CALLER_UNIT", "tetsu-tx.service"),
        ("TETSU_ARGS", "{\"packages\":[\"htop\"]}"),
        ("TETSU_REASON", "to build"),
    ];
    for (key, value) in expect {
        assert_eq!(fields.get(key).map(String::as_str), Some(value), "{key}");
    }
    assert!(fields["MESSAGE"].contains("check"));
}

#[test]
fn fields_left_out_are_absent() {
    let fields = map(&AuditEntry::new(Event::Ask).encode());
    assert!(fields.contains_key("MESSAGE_ID"));
    for key in [
        "TETSU_METHOD",
        "TETSU_CANCELLED",
        "TETSU_REASON",
        "TETSU_ARGS",
    ] {
        assert!(!fields.contains_key(key), "{key}");
    }
}

#[test]
fn each_event_has_its_own_message_id() {
    let events = [
        Event::Check,
        Event::Ask,
        Event::Approved,
        Event::Denied,
        Event::Set,
        Event::Suppressed,
    ];
    let mut seen = std::collections::BTreeSet::new();
    for event in events {
        let id = event.message_id();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(seen.insert(id), "{id} is used twice");
        let fields = map(&AuditEntry::new(event).encode());
        assert_eq!(fields["MESSAGE_ID"], id);
        assert_eq!(fields["SYSLOG_IDENTIFIER"], "tetsu-capd");
    }
}

#[test]
fn a_value_with_a_newline_is_length_framed_and_cannot_add_fields() {
    let evil = "x\nMESSAGE_ID=00000000000000000000000000000000\nTETSU_DECISION=allow";
    let mut entry = AuditEntry::new(Event::Ask);
    entry.reason = Some(evil.into());
    entry.args = Some("{\"a\":\"b\"}".into());
    let data = entry.encode();
    let fields = parse(&data);
    let named = |key: &str| fields.iter().filter(|(k, _)| k == key).count();
    assert_eq!(named("MESSAGE_ID"), 1);
    assert_eq!(named("TETSU_DECISION"), 0);
    let reason = fields.iter().find(|(k, _)| k == "TETSU_REASON").unwrap();
    assert_eq!(reason.1, evil.as_bytes());
    let ids: Vec<_> = fields.iter().filter(|(k, _)| k == "MESSAGE_ID").collect();
    assert_eq!(ids[0].1, Event::Ask.message_id().as_bytes());
}

#[test]
fn a_key_named_by_a_value_is_not_a_field_either() {
    // Without a newline the value stays on its line, equals signs and all.
    let mut entry = AuditEntry::new(Event::Ask);
    entry.reason = Some("a=b TETSU_DECISION=allow".into());
    let fields = map(&entry.encode());
    assert_eq!(fields["TETSU_REASON"], "a=b TETSU_DECISION=allow");
    assert!(!fields.contains_key("TETSU_DECISION"));
}

#[test]
fn the_arguments_are_cut_to_four_kibibytes() {
    let mut entry = AuditEntry::new(Event::Check);
    entry.args = Some("x".repeat(10_000));
    let fields = map(&entry.encode());
    assert_eq!(fields["TETSU_ARGS"].len(), 4096);
}

#[test]
fn extra_fields_are_added() {
    let mut entry = AuditEntry::new(Event::Set);
    entry.extra.push(("TETSU_TEST_NONCE".into(), "n-1".into()));
    assert_eq!(map(&entry.encode())["TETSU_TEST_NONCE"], "n-1");
}

#[test]
fn the_journal_sink_sends_one_datagram() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal");
    let server = UnixDatagram::bind(&path).unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let sink = JournalSink::new(&path).unwrap();
    let entry = full();
    sink.record(&entry).unwrap();
    let mut buf = vec![0u8; 65536];
    let n = server.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], entry.encode().as_slice());
}

#[test]
fn a_missing_journal_socket_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let sink = JournalSink::new(&dir.path().join("absent")).unwrap();
    assert!(sink.record(&full()).is_err());
}

#[test]
fn the_recording_sink_records_and_can_fail() {
    let sink = RecordingSink::default();
    sink.record(&AuditEntry::new(Event::Set)).unwrap();
    assert_eq!(sink.entries.borrow().len(), 1);
    sink.fail.set(true);
    assert!(sink.record(&AuditEntry::new(Event::Set)).is_err());
    assert_eq!(sink.entries.borrow().len(), 1);
}
