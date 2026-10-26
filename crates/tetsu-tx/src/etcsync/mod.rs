//! The first-boot `/etc` sync (decision #21): a snapshot built by a
//! transaction boots with the changes the user made to the running `/etc`
//! after the transaction started.
//!
//! Three trees meet: `base` (the `/etc` the transaction started from),
//! `old` (the running system's `/etc`, the user's side) and `new` (the
//! transaction's result). Per path:
//!
//! * only the user changed it: the user's version wins, a deletion included;
//! * only the transaction changed it: the transaction's version stays;
//! * both changed it the same way: nothing to do;
//! * both changed it differently: the transaction's version wins, the
//!   user's copy is kept aside and a conflict is reported;
//! * `passwd`, `group`, `shadow` and `gshadow` changed on both sides are
//!   merged by [`crate::tx::accounts::merge`], the running version winning
//!   on a shared key. A user the running system deleted therefore comes
//!   back when the new side still has it, as on a rollback.
//!
//! Sockets, FIFOs and devices are skipped and noted. Extended attributes,
//! ACLs and times are not compared.

pub mod node;
pub mod plan;
