use std::process::ExitCode;

fn main() -> ExitCode {
    tetsu_cli::app::main(std::env::args().skip(1))
}
