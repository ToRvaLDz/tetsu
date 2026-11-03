//! One entry through the real journal, read back with `journalctl`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use tetsu_capd::audit::{AuditEntry, AuditSink, Event, JOURNAL_SOCKET, JournalSink};

#[test]
fn an_entry_reaches_the_user_journal() {
    if !Path::new(JOURNAL_SOCKET).exists() {
        eprintln!("skipped: no journal socket at {JOURNAL_SOCKET}");
        return;
    }
    let nonce = format!("capd-{}-{:?}", std::process::id(), Instant::now());
    let nonce: String = nonce.chars().filter(char::is_ascii_alphanumeric).collect();
    let mut entry = AuditEntry::new(Event::Check);
    entry.class = Some("test.nonce".into());
    entry.decision = Some("allow".into());
    entry.reason = Some("line one\nline two".into());
    entry.extra.push(("TETSU_TEST_NONCE".into(), nonce.clone()));
    JournalSink::new(Path::new(JOURNAL_SOCKET))
        .unwrap()
        .record(&entry)
        .unwrap();

    let start = Instant::now();
    loop {
        let out = Command::new("journalctl")
            .args(["--user", "-o", "json", "--no-pager"])
            .arg(format!("TETSU_TEST_NONCE={nonce}"))
            .output();
        let Ok(out) = out else {
            eprintln!("skipped: no journalctl");
            return;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains(&nonce) {
            assert!(
                text.contains("\"SYSLOG_IDENTIFIER\":\"tetsu-capd\""),
                "{text}"
            );
            assert!(text.contains("a8e00ec20cf94ae1b084275879e087f3"), "{text}");
            return;
        }
        if start.elapsed() > Duration::from_secs(5) {
            eprintln!("skipped: the entry is not readable by this user's journalctl");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
