use std::process::ExitCode;

fn main() -> ExitCode {
    tetsu_installer::app::main(std::env::args().skip(1))
}
