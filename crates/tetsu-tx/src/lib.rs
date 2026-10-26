//! tetsu-tx library: the ESP side of a transaction (`boot`), the Btrfs
//! snapshot store (`store`), the transaction pieces (`tx`) and the
//! first-boot `/etc` sync (`etcsync`) and the `org.tetsu.tx.v1` service
//! (`serve`).

pub mod app;
pub mod boot;
pub mod bootcheck;
pub mod cli;
pub mod etcsync;
pub mod serve;
pub mod store;
pub mod tx;
