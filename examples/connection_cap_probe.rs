//! Verify the connection cap: hold `MAX_CONNECTIONS` half-open requests, then
//! check that the next connection is told `503` rather than served or left to hang.
//!
//! Run with `cargo run --example connection_cap_probe`. It exists because a
//! PowerShell client could not be made to hold sockets open reliably, and a cap
//! that is never exercised is a cap nobody knows works.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Must match `MAX_CONNECTIONS` in `src/http_server.rs`.
const CAP: usize = 32;

fn main() {
    let exe = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/debug/hongshi.exe".to_string());

    let mut child = Command::new(&exe)
        .args(["--no-browser"])
        .stdout(Stdio::piped())
        // Inherited so the server's own DEBUG lines land in this terminal.
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the shell");

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
        panic!("no URL printed");
    };
    let without_scheme = url.trim_start_matches("http://");
    let (authority, query) = without_scheme.split_once('/').expect("path");
    let token = query.trim_start_matches("?token=").to_string();
    let authority = authority.to_string();

    // Hold CAP connections, each with a partial request line: inside a request,
    // holding its slot, and not going to be answered until the deadline.
    let mut held = Vec::new();
    for index in 0..CAP {
        match TcpStream::connect(&authority) {
            Ok(mut stream) => {
                let _ = write!(stream, "GET /slow{index} HTTP/1.1\r\n");
                let _ = stream.flush();
                held.push(stream);
            }
            Err(err) => {
                println!("connect {index} failed: {err}");
                break;
            }
        }
    }
    println!("holding {} connections (cap is {CAP})", held.len());

    // The next one must be told the shell is busy.
    let mut over = TcpStream::connect(&authority).expect("connect the over-cap socket");
    over.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        over,
        "GET /?token={token} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut response = Vec::new();
    let read = over.read_to_end(&mut response);
    let status = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or("<nothing>")
        .to_string();
    println!("past the cap: {status}  ({:?}, {} bytes)", read.map(|_| ()), response.len());

    // Done with the held slots, then confirm the shell still serves.
    drop(held);
    std::thread::sleep(Duration::from_millis(600));
    let mut after = TcpStream::connect(&authority).expect("reconnect");
    after.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        after,
        "GET /api/health?token={token} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut after_body = String::new();
    let _ = after.read_to_string(&mut after_body);
    println!(
        "after the held connections close: {}",
        after_body.lines().next().unwrap_or("<nothing>")
    );

    // Stop it.
    let mut quit = TcpStream::connect(&authority).expect("connect to quit");
    write!(
        quit,
        "POST /api/shutdown?token={token} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut _ignored = String::new();
    let _ = quit.read_to_string(&mut _ignored);
    let _ = child.wait();

    let capped = status.contains("503");
    println!("\n{}", if capped { "PASS: the cap answers 503" } else { "FAIL: the cap did not answer 503" });
    let _ = child.kill();
    if !capped {
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
