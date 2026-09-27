//! Minimal experiment: does a client's half-open request stay open?
//!
//! Opens one connection, sends a partial request line, and holds the socket for a
//! while. If the shell closes it immediately, the connection lifetime printed by
//! `--verbose` will be microseconds rather than the hold time.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let exe = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/debug/hongshi.exe".to_string());

    let mut child = Command::new(&exe)
        .args(["--no-browser", "--verbose"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the shell");

    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut url = None;
    let mut line = String::new();
    while stdout.read_line(&mut line).unwrap_or(0) > 0 {
        if let Some(rest) = line.trim().strip_prefix("URL") {
            url = Some(rest.trim().to_string());
            break;
        }
        line.clear();
    }
    let Some(url) = url else {
        let _ = child.kill();
        panic!("no URL");
    };
    let without_scheme = url.trim_start_matches("http://");
    let (authority, query) = without_scheme.split_once('/').expect("path");
    let token = query.trim_start_matches("?token=").to_string();
    let authority = authority.to_string();

    println!("--- holding one half-open request for 3 seconds ---");
    let mut held = TcpStream::connect(&authority).expect("connect");
    held.set_nodelay(true).ok();
    let written = write!(held, "GET /slow HTTP/1.1\r\n").and_then(|()| held.flush());
    println!("write result: {written:?}");

    // What did the server actually send back, if anything?
    held.set_read_timeout(Some(Duration::from_millis(400))).ok();
    let mut response = Vec::new();
    match held.read_to_end(&mut response) {
        Ok(_) => println!("server closed with: {:?}", String::from_utf8_lossy(&response).replace("\r\n", " | ")),
        Err(err) => println!("read_to_end: {err} after {} bytes: {:?}", response.len(), String::from_utf8_lossy(&response).replace("\r\n", " | ")),
    }

    std::thread::sleep(Duration::from_secs(3));
    let mut probe = [0u8; 64];
    match held.peek(&mut probe) {
        Ok(0) => println!("immediately after writing: peer has closed (EOF)"),
        Ok(n) => println!("immediately after writing: {n} bytes waiting"),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
            println!("immediately after writing: still open, nothing waiting");
        }
        Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {
            println!("immediately after writing: still open (peek timed out)");
        }
        Err(err) => println!("immediately after writing: peek error {err}"),
    }

    std::thread::sleep(Duration::from_secs(3));
    match held.peek(&mut probe) {
        Ok(0) => println!("after 3s: peer has closed (EOF)"),
        Ok(n) => println!("after 3s: {n} bytes waiting"),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => println!("after 3s: still open"),
        Err(err) if err.kind() == std::io::ErrorKind::TimedOut => println!("after 3s: still open"),
        Err(err) => println!("after 3s: peek error {err}"),
    }

    drop(held);
    std::thread::sleep(Duration::from_millis(400));

    // Stop it.
    let mut quit = TcpStream::connect(&authority).expect("connect to quit");
    let _ = write!(
        quit,
        "POST /api/shutdown?token={token} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let mut _ignored = String::new();
    let _ = quit.read_to_string(&mut _ignored);
    let _ = child.wait();
    let _ = child.kill();
}
