//! Engine and store values to wire values and contract errors.

use tetsu_bus::server::ErrorReply;
use tetsu_bus::tx as wire;

use crate::boot::entry::BootState;
use crate::store::record::{Record, TxKind};
use crate::tx::TxError;
use crate::tx::engine::{Report, Request};

/// `YYYY-MM-DDTHH:MM:SS.000Z` for Unix seconds.
pub fn rfc3339_ms(secs: u64) -> String {
    let _ = secs;
    todo!()
}

/// The wire kind of a record kind.
pub fn tx_kind(kind: TxKind) -> wire::TxKind {
    let _ = kind;
    todo!()
}

/// A committed record as a wire transaction; `None` for a record that
/// is not a transaction (the installer's, or one rebuilt from the ESP).
pub fn from_record(record: &Record) -> Option<wire::Transaction> {
    let _ = record;
    todo!()
}

/// The transaction `request` is, as it starts at `started` (Unix seconds).
pub fn running(id: &str, request: &Request, started: u64) -> wire::Transaction {
    let _ = (id, request, started);
    todo!()
}

/// The transaction a report closes.
pub fn finished(id: &str, report: &Report, started: u64, finished: u64) -> wire::Transaction {
    let _ = (id, report, started, finished);
    todo!()
}

/// `running` as it ends in failure, with `failure` carried verbatim.
pub fn failed(running: &wire::Transaction, finished: u64, failure: &str) -> wire::Transaction {
    let _ = (running, finished, failure);
    todo!()
}

/// What went wrong, as text for `Transaction.failure`: the reason with the
/// proposal appended.
pub fn failure_text(error: &TxError) -> String {
    let _ = error;
    todo!()
}

/// The contract error for `error`. `asked` is the snapshot id the caller
/// named, if any.
pub fn error_reply(id: &str, error: &TxError, asked: Option<&str>) -> ErrorReply {
    let _ = (id, error, asked);
    todo!()
}

/// The counter a snapshot id names; anything but the canonical decimal
/// spelling is a `SnapshotNotFound`.
pub fn parse_snapshot_id(id: &str) -> Result<u64, ErrorReply> {
    let _ = id;
    todo!()
}

/// `GetStatus.default_boot` of an entry's boot-counting state.
pub fn default_boot(state: BootState) -> wire::DefaultBoot {
    let _ = state;
    todo!()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Value, json};
    use tetsu_bus_contract::{Contract, check_error, check_reply, load_dir};

    use super::*;
    use crate::store::record::BootVerdict;
    use crate::tx::PackageChange;

    fn contract() -> Contract {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
        load_dir(&dir).expect("the bus contract loads")
    }

    /// The transaction as a `ListTransactions` reply passes the contract.
    fn assert_valid(tx: &wire::Transaction) {
        let reply = json!({ "transactions": [serde_json::to_value(tx).unwrap()] });
        let found = check_reply(&contract(), "org.tetsu.tx.v1.ListTransactions", &reply);
        assert_eq!(found, vec![], "{reply}");
    }

    fn assert_valid_error(e: &ErrorReply) {
        let found = check_error(&contract(), &e.error, &e.parameters);
        assert_eq!(found, vec![], "{e:?}");
    }

    fn record(counter: u64) -> Record {
        Record {
            counter,
            kind: Some(TxKind::Install),
            source: Some(counter - 1),
            boot: Some(BootVerdict::Good),
            packages: vec!["htop".into()],
            changes: vec![PackageChange {
                name: "htop".into(),
                from: None,
                to: Some("3.4.1-1".into()),
            }],
            started_at: Some(1_768_469_400),
            finished_at: Some(1_768_469_472),
            ..Record::default()
        }
    }

    fn report() -> Report {
        Report {
            counter: 9,
            kind: TxKind::Remove,
            packages: vec!["vim".into()],
            changes: vec![PackageChange {
                name: "vim".into(),
                from: Some("9.1-1".into()),
                to: None,
            }],
            log: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn times_are_rfc3339_with_milliseconds() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_ms(1_709_164_800), "2024-02-29T00:00:00.000Z");
        assert_eq!(rfc3339_ms(1_768_469_400), "2026-01-15T09:30:00.000Z");
        assert_eq!(rfc3339_ms(2_147_483_648), "2038-01-19T03:14:08.000Z");
        assert_eq!(rfc3339_ms(951_782_399), "2000-02-28T23:59:59.000Z");
    }

    #[test]
    fn a_record_becomes_a_succeeded_transaction() {
        let tx = from_record(&record(8)).unwrap();
        assert_eq!(tx.id, "tx-8");
        assert_eq!(tx.kind, wire::TxKind::Install);
        assert_eq!(tx.packages, ["htop"]);
        assert_eq!(tx.state, wire::TxState::Succeeded);
        assert_eq!(tx.started_at, "2026-01-15T09:30:00.000Z");
        assert_eq!(tx.finished_at.as_deref(), Some("2026-01-15T09:31:12.000Z"));
        assert_eq!(tx.snapshot.as_deref(), Some("8"));
        assert_eq!(tx.changes.len(), 1);
        assert_eq!(tx.changes[0].to.as_deref(), Some("3.4.1-1"));
        assert_eq!(tx.failure, None);
        assert_valid(&tx);
    }

    #[test]
    fn a_record_keeps_the_id_it_was_given() {
        let tx = from_record(&Record {
            id: Some("tx-0123456789abcdef".into()),
            ..record(8)
        })
        .unwrap();
        assert_eq!(tx.id, "tx-0123456789abcdef");
    }

    #[test]
    fn a_record_without_a_start_time_starts_at_the_epoch() {
        let tx = from_record(&Record {
            started_at: None,
            finished_at: None,
            ..record(3)
        })
        .unwrap();
        assert_eq!(tx.started_at, "1970-01-01T00:00:00.000Z");
        assert_eq!(tx.finished_at, None);
        assert_valid(&tx);
    }

    #[test]
    fn a_record_without_a_kind_is_not_a_transaction() {
        let rebuilt = Record {
            kind: None,
            reconstructed: true,
            ..record(4)
        };
        assert_eq!(from_record(&rebuilt), None);
    }

    #[test]
    fn every_record_kind_has_a_wire_kind() {
        assert_eq!(tx_kind(TxKind::Update), wire::TxKind::Update);
        assert_eq!(tx_kind(TxKind::Install), wire::TxKind::Install);
        assert_eq!(tx_kind(TxKind::InstallAur), wire::TxKind::InstallAur);
        assert_eq!(tx_kind(TxKind::Remove), wire::TxKind::Remove);
        assert_eq!(tx_kind(TxKind::Rollback), wire::TxKind::Rollback);
    }

    #[test]
    fn a_request_starts_a_running_transaction() {
        let tx = running(
            "tx-ab",
            &Request::Install(vec!["htop".into()]),
            1_768_469_400,
        );
        assert_eq!(tx.id, "tx-ab");
        assert_eq!(tx.kind, wire::TxKind::Install);
        assert_eq!(tx.packages, ["htop"]);
        assert_eq!(tx.state, wire::TxState::Running);
        assert_eq!(tx.finished_at, None);
        assert_eq!(tx.snapshot, None);
        assert_valid(&tx);
        let update = running("tx-ac", &Request::Rollback(Some(3)), 0);
        assert_eq!(update.kind, wire::TxKind::Rollback);
        assert!(update.packages.is_empty());
    }

    #[test]
    fn a_report_closes_the_transaction() {
        let tx = finished("tx-ab", &report(), 1_768_469_400, 1_768_469_401);
        assert_eq!(tx.id, "tx-ab");
        assert_eq!(tx.kind, wire::TxKind::Remove);
        assert_eq!(tx.state, wire::TxState::Succeeded);
        assert_eq!(tx.finished_at.as_deref(), Some("2026-01-15T09:30:01.000Z"));
        assert_eq!(tx.snapshot.as_deref(), Some("9"));
        assert_eq!(tx.changes[0].from.as_deref(), Some("9.1-1"));
        assert_valid(&tx);
    }

    #[test]
    fn a_failure_is_carried_verbatim() {
        let text = "error: <b>boom</b>\n\u{1b}[31mred\u{1b}[0m & more";
        let start = running("tx-ab", &Request::Update, 1_768_469_400);
        let tx = failed(&start, 1_768_469_405, text);
        assert_eq!(tx.state, wire::TxState::Failed);
        assert_eq!(tx.failure.as_deref(), Some(text));
        assert_eq!(tx.finished_at.as_deref(), Some("2026-01-15T09:30:05.000Z"));
        assert_eq!(tx.snapshot, None);
        assert_eq!(tx.id, "tx-ab");
        assert_valid(&tx);
    }

    #[test]
    fn the_proposal_is_appended_to_the_reason() {
        let plain = TxError::failed("pacman said <no>\nline two");
        assert_eq!(failure_text(&plain), "pacman said <no>\nline two");
        let advised = TxError::Failed {
            reason: "blocked by vim".into(),
            proposal: Some("tetsu-tx remove vim".into()),
            log: vec![],
        };
        let text = failure_text(&advised);
        assert!(text.starts_with("blocked by vim"), "{text}");
        assert!(text.contains("tetsu-tx remove vim"), "{text}");
        assert!(failure_text(&TxError::PackageNotFound("nope".into())).contains("nope"));
    }

    #[test]
    fn engine_errors_become_contract_errors() {
        let cases: Vec<(TxError, Option<&str>, &str, Value)> = vec![
            (
                TxError::InProgress,
                None,
                wire::TRANSACTION_IN_PROGRESS,
                json!({ "id": "tx-1" }),
            ),
            (
                TxError::PackageNotFound("nope".into()),
                None,
                wire::PACKAGE_NOT_FOUND,
                json!({ "name": "nope" }),
            ),
            (
                TxError::NotUserPackage("bash".into()),
                None,
                wire::NOT_USER_PACKAGE,
                json!({ "name": "bash" }),
            ),
            (
                TxError::SnapshotNotFound(7),
                Some("7"),
                wire::SNAPSHOT_NOT_FOUND,
                json!({ "id": "7" }),
            ),
            (
                TxError::SnapshotNotFound(0),
                None,
                wire::SNAPSHOT_NOT_FOUND,
                json!({ "id": "" }),
            ),
            (
                TxError::failed("<b>no</b>\n"),
                None,
                wire::TRANSACTION_FAILED,
                json!({ "id": "tx-1", "reason": "<b>no</b>\n" }),
            ),
        ];
        for (error, asked, name, parameters) in cases {
            let reply = error_reply("tx-1", &error, asked);
            assert_eq!(reply.error, name, "{error:?}");
            assert_eq!(reply.parameters, parameters, "{error:?}");
            assert_valid_error(&reply);
        }
    }

    #[test]
    fn a_failure_with_a_proposal_keeps_both_in_the_reason() {
        let error = TxError::Failed {
            reason: "blocked by vim".into(),
            proposal: Some("tetsu-tx remove vim".into()),
            log: vec![],
        };
        let reply = error_reply("tx-1", &error, None);
        assert_eq!(reply.error, wire::TRANSACTION_FAILED);
        let reason = reply.parameters["reason"].as_str().unwrap();
        assert!(reason.contains("blocked by vim") && reason.contains("remove vim"));
        assert_valid_error(&reply);
    }

    #[test]
    fn a_snapshot_id_is_a_canonical_decimal() {
        assert_eq!(parse_snapshot_id("12").unwrap(), 12);
        assert_eq!(parse_snapshot_id("0").unwrap(), 0);
        for bad in ["012", "x", "", "-1", "1 ", "+1"] {
            let e = parse_snapshot_id(bad).unwrap_err();
            assert_eq!(e.error, wire::SNAPSHOT_NOT_FOUND, "{bad:?}");
            assert_eq!(e.parameters, json!({ "id": bad }), "{bad:?}");
            assert_valid_error(&e);
        }
    }

    #[test]
    fn the_boot_state_becomes_default_boot() {
        assert_eq!(default_boot(BootState::Good), wire::DefaultBoot::Good);
        assert_eq!(default_boot(BootState::Bad), wire::DefaultBoot::Bad);
        assert_eq!(default_boot(BootState::Untried), wire::DefaultBoot::Untried);
        assert_eq!(default_boot(BootState::Trying), wire::DefaultBoot::Untried);
    }
}
