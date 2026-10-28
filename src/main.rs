use anyhow::{Context, Result, bail};
use macos_mcp_manager::{clients, manager::Manager};
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod native;

fn main() {
    if let Err(error) = run() {
        eprintln!("MCP Manager: {error:#}");
        #[cfg(target_os = "macos")]
        if !std::env::args().any(|argument| argument == "--list") {
            native::show_message(&format!("Could not start MCP Manager: {error:#}"));
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut inspect = false;
    let mut home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("Cannot find the home folder")?;
    let mut isolated_home = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--list" => inspect = true,
            "--home" => {
                home = PathBuf::from(args.next().context("--home requires a folder")?);
                isolated_home = true;
            }
            "--help" | "-h" => {
                println!(
                    "MCP Manager\n\nRun without arguments for the macOS menu bar.\n  --list          Print configured server names/states, never credentials\n  --home <path>   Use an isolated home folder (also isolates app state)\n  --help          Show this help\n\nCheckmarks represent configuration, not live connection state."
                );
                return Ok(());
            }
            _ => bail!("Unknown argument; use --help"),
        }
    }
    if !home.is_absolute() {
        bail!("Home folder must be an absolute path");
    }
    let codex_home = if isolated_home {
        None
    } else {
        std::env::var_os("CODEX_HOME").map(PathBuf::from)
    };
    let directory = home.join("Library/Application Support/MCP Manager");
    let manager = Manager::open(clients::sources(&home, codex_home), directory)?;
    if inspect {
        println!("{}", serde_json::to_string_pretty(&manager.snapshot()?)?);
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    return native::run(manager);
    #[cfg(not(target_os = "macos"))]
    bail!("The menu bar app requires macOS. Use --list to inspect a fixture home.");
}
