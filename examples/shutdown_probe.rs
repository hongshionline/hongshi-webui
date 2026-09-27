//! Manual probe: start the shell, POST `/api/shutdown`, dump everything the
//! process said and the code it exited with.
//!
//! It exists because a panic during shutdown is invisible from the outside — the
//! process merely exits with 101 — and a test harness that only checks the exit
//! code cannot say why.
//!
//! Run with `cargo run --example shutdown_probe`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    let exe = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/debug/hongshi.exe".to_string());

    let mut child = Command::new(&exe)
        .args(["--no-browser", "--port", "45996", "--verbose"])
        .stdout(Stdio::piped())
        // Inherited, so a panic message lands in this terminal the moment it is
        // written rather than being read back after the pipe closes.
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the shell");

    // Read stderr inherited by this process, so there is nothing to drain here.
    let stderr_thread = std::thread::spawn(|| String::new());

    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut url = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let mut line = String::new();
        match stdout.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                println!("[shell] {}", line.trim_end());
                if let Some(rest) = line.trim().strip_prefix("URL") {
                    url = Some(rest.trim().to_string());
                    break;
                }
            }
        }
    }
    let Some(url) = url else {
        eprintln!("no URL printed");
        let _ = child.kill();
        return;
    };

    // `url` is the session URL, so lift the token out of it and aim at the
    // shutdown endpoint rather than at the page.
    let token = url
        .split_once("?token=")
        .map(|(_, token)| token.to_string())
        .expect("token in URL");
    let without_scheme = url.trim_start_matches("http://");
    let (authority, _) = without_scheme.split_once('/').expect("authority");
    println!("[probe] POST /api/shutdown to {authority}");

    // Before anything else: the very first request a browser makes. This is the
    // one a PowerShell client was seen to lose, so it gets its own check here.
    {
        let mut stream = TcpStream::connect(authority).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            stream,
            "GET /?token={token} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
        )
        .expect("write");
        let mut response = Vec::new();
        match stream.read_to_end(&mut response) {
            Ok(_) => println!("[probe] first request: {} bytes", response.len()),
            Err(err) => println!("[probe] first request FAILED: {err}"),
        }
        let text = String::from_utf8_lossy(&response);
        println!(
            "[probe]   status line: {:?}",
            text.lines().next().unwrap_or("<none>")
        );
    }

    let target = format!("/api/shutdown?token={token}");
    println!("[probe] POST {target} to {authority}");

    let mut stream = TcpStream::connect(authority).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        stream,
        "POST {target} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut response = Vec::new();
    match stream.read_to_end(&mut response) {
        Ok(_) => println!("[probe] response:\n{}", String::from_utf8_lossy(&response)),
        Err(err) => println!("[probe] read failed: {err}"),
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break status.code(),
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            None => {
                println!("[probe] still running after 10s — killing");
                let _ = child.kill();
                break child.wait().expect("wait").code();
            }
        }
    };

    println!("[probe] exit code: {code:?}");
    for line in stdout.lines().map_while(Result::ok) {
        println!("[shell] {line}");
    }
    println!("[probe] stderr:\n{}", stderr_thread.join().unwrap_or_default());
}
