//! One entry through the real `JournalSink`, read back with `journalctl`.
//! Writes a single test entry (action `list`, secret id = a fresh nonce) to
//! the user's journal, and skips where there is no journal socket (CI
//! containers) or no `journalctl`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use tetsu_secrets::audit::{Action, AuditEntry, AuditSink, JOURNAL_SOCKET, JournalSink, Outcome};
use tetsu_secrets::caller::Caller;

#[test]
fn an_entry_reaches_the_journal() {
    if !Path::new(JOURNAL_SOCKET).exists() {
        eprintln!("skipped: no {JOURNAL_SOCKET}");
        return;
    }
    let nonce = format!(
        "journal-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut entry = AuditEntry::new(Action::List, Outcome::Ok, Caller::this_process());
    entry.secret_id = Some(nonce.clone());
    JournalSink::new(JOURNAL_SOCKET).record(&entry).unwrap();

    let query = format!("TETSU_SECRET_ID={nonce}");
    let start = Instant::now();
    loop {
        let out = match Command::new("journalctl")
            .args(["--user", "-o", "json", "--no-pager", &query])
            .output()
        {
            Ok(out) => out,
            Err(e) => {
                eprintln!("skipped: journalctl: {e}");
                return;
            }
        };
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains(&nonce) {
            let line: serde_json::Value =
                serde_json::from_str(text.lines().next().unwrap()).unwrap();
            assert_eq!(line["SYSLOG_IDENTIFIER"], "tetsu-secrets");
            assert_eq!(line["MESSAGE_ID"], Action::List.message_id());
            assert_eq!(line["TETSU_ACTION"], "list");
            return;
        }
        assert!(
            start.elapsed() <= Duration::from_secs(5),
            "the entry did not reach `journalctl --user` in 5 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
