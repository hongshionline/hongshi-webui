//! Does the kernel colour its output, and does `NO_COLOR` stop it?
//!
//! This exists because the first attempt to answer the question got it wrong. The
//! shell's own environment here happens to have `NO_COLOR=1`, the child inherited it,
//! and the probe therefore measured the harness rather than the product: it reported
//! "a piped kernel does not colour its output" about a kernel that colours on every
//! ordinary machine. A diagnostic that cannot tell you which of those two it is
//! looking at is worse than no diagnostic.
//!
//! What it does: spawns the kernel twice with the same `Stdio` configuration
//! `kernel.rs` uses, once inheriting this process's environment and once with
//! `NO_COLOR=1` set explicitly, and reports how many lines carried escape bytes.
//!
//!     cargo run --example kernel_colour_probe
//!     cargo run --example kernel_colour_probe -- <path-to-hongshic> <relay>

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long to let the kernel print before giving up on it. It logs the tunnel line
/// and then goes quiet, so reading "the first few lines" would block forever.
const WINDOW: Duration = Duration::from_secs(6);

fn spawn_and_collect(path: &str, relay: &str, force_no_color: bool) -> Vec<String> {
    let mut command = Command::new(path);
    command
        .arg("-t")
        .arg(relay)
        .arg("-p")
        .arg("25565")
        .arg("--game-host")
        .arg("127.0.0.1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if force_no_color {
        command.env("NO_COLOR", "1");
    }

    let mut child = command.spawn().expect("spawn the kernel");
    let stdout = child.stdout.take().expect("stdout");

    let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            sink.lock().unwrap().push(line);
        }
    });

    std::thread::sleep(WINDOW);
    let _ = child.kill();
    let _ = child.wait();
    let lines = collected.lock().unwrap().clone();
    lines
}

fn report(label: &str, lines: &[String]) {
    let escapes = lines.iter().filter(|line| line.contains('\u{1b}')).count();
    println!("\n{label}");
    for line in lines.iter().take(2) {
        println!("    ESC={}  {}", line.contains('\u{1b}'), line.replace('\u{1b}', "<ESC>"));
    }
    if lines.is_empty() {
        println!("    (the kernel printed nothing, so this proves nothing)");
        return;
    }
    println!("    {escapes} of {} line(s) carried escape bytes", lines.len());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "core/hongshic.exe".to_string());
    let relay = args.next().unwrap_or_else(|| "cd.hongshi.site".to_string());

    match std::env::var("NO_COLOR") {
        Ok(value) => println!("this process has NO_COLOR={value:?}, and a spawned child inherits it"),
        Err(_) => println!("this process has NO_COLOR unset, which is what a user's machine looks like"),
    }
    println!("kernel: {path}");

    report("inheriting this environment:", &spawn_and_collect(&path, &relay, false));
    report("with NO_COLOR=1 set on the child:", &spawn_and_collect(&path, &relay, true));

    println!(
        "\nThe client must produce a clean log either way: it sets NO_COLOR=1 on the child \
         *and* strips escapes from whatever arrives, because the kernel is a separate program \
         whose build it does not control."
    );
}
