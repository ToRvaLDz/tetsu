//! tetsu-bar: the status bar.

use std::process::ExitCode;

const USAGE: &str = "usage: tetsu-bar

Shows workspaces, clock, network and battery on a layer-shell surface.
Environment:
  TETSU_COMP_SOCKET  the tetsu-comp socket (workspaces are hidden when unset)
  TETSU_BAR_TRACE=1  print one `draw <reason>` line per committed frame";

fn main() -> ExitCode {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("tetsu-bar: unknown argument {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    ExitCode::SUCCESS
}
