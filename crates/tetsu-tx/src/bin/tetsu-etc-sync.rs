//! `tetsu-etc-sync`: the initrd service that carries the user's `/etc`
//! changes into the first boot of a transaction's snapshot. It never blocks
//! the boot: it always exits 0.

use std::process::ExitCode;

use tetsu_tx::boot::tool::SystemRunner;
use tetsu_tx::etcsync::initrd::{Paths, lines, run};

fn main() -> ExitCode {
    for line in lines(&run(&Paths::system(), &SystemRunner::new())) {
        println!("{line}");
    }
    ExitCode::SUCCESS
}
