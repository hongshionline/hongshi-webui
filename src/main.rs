//! `hongshi` — the WebUI shell executable.
//!
//! Everything of substance lives in the library so it can be tested without
//! spawning a process; this binary is the process.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    hongshi_shell::run(args)
}
