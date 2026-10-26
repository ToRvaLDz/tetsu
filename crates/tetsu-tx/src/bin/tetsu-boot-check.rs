//! `tetsu-boot-check`: the health check that gates `boot-complete.target`.
//! Exit 0 when the system is healthy, 1 when it is not.

use std::process::ExitCode;

use tetsu_tx::boot::tool::SystemRunner;
use tetsu_tx::bootcheck::{Paths, lines, run};

fn main() -> ExitCode {
    let report = run(&Paths::system(), &SystemRunner::new());
    for line in lines(&report) {
        println!("{line}");
    }
    if report.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
