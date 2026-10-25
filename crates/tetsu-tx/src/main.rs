//! tetsu-tx binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tetsu_tx::app::main(&args)
}
