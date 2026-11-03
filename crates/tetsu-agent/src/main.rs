//! tetsu-agent: lists the bus tools and runs one through the capability
//! gate. The model loop (`tetsu ask`, the daemon) is #79/#80; with no
//! backend configured only the rules act.

use std::process::ExitCode;

use tetsu_agent::cli::{self, Command, USAGE, USAGE_EXIT};
use tetsu_agent::contract;
use tetsu_agent::discovery::{self, Catalog, Dirs, Pinned};
use tetsu_agent::runner::{BusRunner, ToolRunner, stderr_sink};
use tetsu_agent::tools::{self, Tool};
use tetsu_bus::access::{ExpectedServer, capd_path};
use tetsu_bus::endpoint::Endpoint;
use tokio::task::LocalSet;

/// The unit that must serve capd.
const CAPD_UNIT: &str = "tetsu-capd.service";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let command = match cli::parse(&args) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("tetsu-agent: {e}\n{USAGE}");
            return ExitCode::from(USAGE_EXIT);
        }
    };
    match run(command) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("tetsu-agent: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<u8, String> {
    if command == Command::Idle {
        println!("no backend is configured; only the rules act");
        return Ok(0);
    }
    let all =
        tools::build(&contract::load().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    LocalSet::new().block_on(&runtime, execute(command, all))
}

async fn execute(command: Command, all: Vec<Tool>) -> Result<u8, String> {
    let euid = rustix::process::geteuid().as_raw();
    let catalog = discovery::discover(&all, &Dirs::production(), &Pinned::production(euid)).await;
    match command {
        Command::Idle => Ok(0),
        Command::Tools { json } => {
            println!("{}", cli::render_tools(&catalog, json));
            Ok(0)
        }
        Command::Call { tool, input } => call(catalog, euid, &tool, input).await,
    }
}

async fn call(
    catalog: Catalog,
    euid: u32,
    tool_id: &str,
    input: serde_json::Value,
) -> Result<u8, String> {
    let tool = catalog
        .tools
        .iter()
        .find(|t| t.id == tool_id)
        .ok_or_else(|| format!("{tool_id} is not served here (try `tetsu-agent tools`)"))?
        .clone();
    let capd = Endpoint::new(
        capd_path(euid),
        ExpectedServer::UserUnit {
            uid: euid,
            unit: CAPD_UNIT.to_owned(),
        },
    );
    let runner = BusRunner::new(catalog.routes, capd, stderr_sink());
    let outcome = runner.run(&tool, input).await;
    println!("{}", cli::render_outcome(&outcome));
    Ok(cli::exit_code(&outcome))
}
