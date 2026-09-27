//! Stress the first request after startup.
//!
//! The first connection a client makes can lose its request: the shell logs 408
//! (accepted, read nothing) and the client sees an RST with zero bytes. It is
//! rare — about one run in twenty — so this exists to reproduce it on demand and
//! to prove a fix. Run with:
//!
//! ```text
//! cargo run --release --example first_request_probe [runs]
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(40);
    let exe = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "target/debug/hongshi.exe".to_string());

    let mut ok = 0usize;
    let mut failures = Vec::new();

    for run in 1..=runs {
        let mut child = Command::new(&exe)
            .args(["--no-browser"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the shell");

        // Read the banner exactly as the verification script does, then connect
        // the moment the URL line arrives.
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut url = None;
        while let Ok(Some(line)) = read_line(&mut stdout) {
            if let Some(rest) = line.trim().strip_prefix("URL") {
                url = Some(rest.trim().to_string());
                break;
            }
        }
        let Some(url) = url else {
            let _ = child.kill();
            failures.push(format!("run {run}: no URL printed"));
            continue;
        };
        let without_scheme = url.trim_start_matches("http://");
        let (authority, query) = without_scheme.split_once('/').expect("path");
        let token = query.trim_start_matches("?token=").to_string();

        let started = Instant::now();
        let result = one_request(authority, &token);
        match result {
            Ok(bytes) if bytes > 1000 => ok += 1,
            Ok(bytes) => failures.push(format!("run {run}: only {bytes} bytes")),
            Err(err) => failures.push(format!("run {run}: {err} in {:?}", started.elapsed())),
        }

        let _ = child.kill();
        let _ = child.wait();
    }

    println!("first request: OK={ok} BAD={} of {runs}", failures.len());
    for failure in &failures {
        println!("  {failure}");
    }
    if !failures.is_empty() {
        std::process::exit(1);
    }
}

fn read_line(reader: &mut BufReader<std::process::ChildStdout>) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(line)),
        Err(err) => Err(err),
    }
}

fn one_request(authority: &str, token: &str) -> Result<usize, String> {
    let mut stream = TcpStream::connect(authority).map_err(|err| format!("connect: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|err| err.to_string())?;
    write!(
        stream,
        "GET /?token={token} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|err| format!("write: {err}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|err| format!("read: {err}"))?;
    Ok(response.len())
}
