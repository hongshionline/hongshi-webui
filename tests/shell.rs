//! End-to-end tests: start the real binary, talk to it over a real socket, stop
//! it the way a page would.
//!
//! Nothing here reaches inside the library. The point is to prove the shipped
//! artifact behaves — binds loopback only, answers the page and its assets, refuses
//! a request that came from somewhere else, honours a Range request, and exits when
//! asked to. Unit tests for the parsing decisions live next to the code they test.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one read may take before the harness calls it a timeout. Generous on
/// purpose: these tests run in parallel and each one owns a child process.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// A shell process under test, killed when the test ends.
struct Shell {
    child: Child,
    port: u16,
    /// Everything the process wrote to stderr, collected as it is written. A
    /// panic makes the process exit 101 and says nothing on stdout, so without
    /// this a failing test can only report the number.
    stderr: Arc<Mutex<String>>,
}

impl Shell {
    /// Start the binary with `--no-browser` and wait for it to print its URL.
    fn start() -> Shell {
        Shell::start_with(&["--no-browser"])
    }

    fn start_with(extra: &[&str]) -> Shell {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hongshi"));
        command
            .args(extra)
            // A panic while the harness is closing pipes is otherwise just "101".
            .env("RUST_BACKTRACE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("the shell binary should start");

        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(handle) = child.stderr.take() {
            let sink = Arc::clone(&stderr);
            std::thread::spawn(move || {
                for line in BufReader::new(handle).lines().map_while(Result::ok) {
                    if let Ok(mut text) = sink.lock() {
                        text.push_str(&line);
                        text.push('\n');
                    }
                }
            });
        }

        let port = read_url(&rx).unwrap_or_else(|message| {
            let _ = child.kill();
            panic!(
                "{message}\nstderr was:\n{}",
                stderr.lock().map(|text| text.clone()).unwrap_or_default()
            );
        });

        Shell {
            child,
            port,
            stderr,
        }
    }

    fn stderr(&self) -> String {
        self.stderr
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }

    fn authority(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Send one raw request and read the whole response.
    fn request(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(self.authority()).expect("connect");
        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .expect("read timeout");
        stream.write_all(request.as_bytes()).expect("write");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read");
        String::from_utf8_lossy(&response).into_owned()
    }

    fn get(&self, path_and_query: &str) -> String {
        self.request(&format!(
            "GET {path_and_query} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.authority()
        ))
    }

    fn post(&self, path: &str, body: &str) -> String {
        self.request(&format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.authority(),
            body.len(),
            body
        ))
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    // Give the stderr collector a moment to catch up: the process
                    // can exit before its last stderr line has been read, and a
                    // failure message that omits the panic is useless.
                    let settle = Instant::now() + Duration::from_millis(500);
                    let mut last = self.stderr().len();
                    while Instant::now() < settle {
                        std::thread::sleep(Duration::from_millis(25));
                        let now = self.stderr().len();
                        if now == last {
                            break;
                        }
                        last = now;
                    }
                    return Some(status.code().unwrap_or(-1));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read the console until the `URL` line appears, and lift the port out of it.
///
/// The URL is a plain `http://127.0.0.1:<port>/` now. It used to end in
/// `?token=…`, which is exactly what made a reload fail: the browser re-requests
/// `/` with no query string, so the shell answered its own page with 403.
fn read_url(lines: &Receiver<String>) -> Result<u16, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match lines.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                if let Some(rest) = line.trim().strip_prefix("URL") {
                    let url = rest.trim();
                    return parse_url(url).ok_or_else(|| format!("unparsable URL line: {line}"));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the shell exited before printing its URL".to_string());
            }
        }
    }
    Err("timed out waiting for the shell to print its URL".to_string())
}

fn parse_url(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    let (port, path) = rest.split_once('/')?;
    if !path.is_empty() {
        return None;
    }
    port.parse().ok()
}

fn status_line(response: &str) -> String {
    response.lines().next().unwrap_or_default().to_string()
}

fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or("")
}

fn header_value<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    response
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.eq_ignore_ascii_case(name) {
                Some(value.trim())
            } else {
                None
            }
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn the_shell_starts_binds_loopback_and_announces_a_plain_url() {
    let shell = Shell::start();
    assert!(shell.port > 0, "a port should have been chosen");
    // The announced URL has to be a URL a user can open and reload. It carried a
    // `?token=…` once, and a reload of it — which is a fresh request for `/` with
    // no query string — came back 403.
    let page = shell.get("/");
    assert!(status_line(&page).contains("200"), "{page}");
}

#[test]
fn the_page_can_load_its_own_stylesheet_and_script() {
    // This is the test whose absence let a completely dead UI pass 54 tests:
    // every other test fetched a URL no browser ever requests. A browser resolves
    // the page's `href`/`src` against the page URL, so it asks for `/main.css` as
    // written in the markup. If the server refuses that, the page arrives
    // unstyled, `app.js` never runs, the session card stays on "checking…", and
    // Quit does nothing.
    let shell = Shell::start();

    let page = shell.get("/");
    assert!(status_line(&page).contains("200"), "{page}");

    // Exactly what the browser will request, derived from the page's own markup.
    let references = extract_asset_references(body(&page));
    let assets: Vec<&String> = references
        .iter()
        .filter(|reference| {
            let name = reference
                .split(['?', '#'])
                .next()
                .unwrap_or("")
                .rsplit('/')
                .next()
                .unwrap_or("");
            name.contains('.')
        })
        .collect();

    // The shell's own files are all here.
    for expected in ["main.css", "app.css", "icons.svg", "app.js", "pages.js"] {
        let found = assets
            .iter()
            .find(|reference| reference.contains(expected))
            .unwrap_or_else(|| panic!("the page no longer references {expected}: {assets:?}"));
        assert!(
            !found.contains("token="),
            "the page asks for {found:?} with a session token; there is no token any more"
        );
    }

    // The sprite reference keeps its fragment.
    assert!(
        assets.iter().any(|reference| reference.contains("icons.svg") && reference.contains('#')),
        "the sprite reference lost its fragment: {assets:?}"
    );

    for reference in &assets {
        // A fragment is resolved by the browser, not sent to the server.
        let without_fragment = reference.split('#').next().unwrap_or(reference);
        let target = if without_fragment.starts_with('/') {
            without_fragment.to_string()
        } else {
            format!("/{without_fragment}")
        };
        let response = shell.get(&target);
        assert!(
            status_line(&response).contains("200"),
            "the page's own asset {target:?} returned {:?}",
            status_line(&response)
        );
        assert!(
            body(&response).len() > 100,
            "{target:?} came back suspiciously small ({} bytes)",
            body(&response).len()
        );
    }

    // Navigation targets are documents, not assets, and must stay clean: `/` with
    // a query string on it would be a link to something other than the page.
    for reference in references.iter().filter(|reference| {
        !reference
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .contains('.')
    }) {
        assert!(
            !reference.contains('?'),
            "a route reference was rewritten: {reference:?}"
        );
    }
}

/// Pull `href="…"` / `src="…"` values out of `html`, the way a browser would.
fn extract_asset_references(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    for attribute in ["href=\"", "src=\""] {
        let mut rest = html;
        while let Some((_, after)) = rest.split_once(attribute) {
            let Some((value, remaining)) = after.split_once('"') else {
                break;
            };
            found.push(value.to_string());
            rest = remaining;
        }
    }
    found
}

#[test]
fn the_page_and_its_assets_are_served_without_any_ceremony() {
    // The regression this guards is a page that could not be reloaded: the shell
    // used to gate every route behind a `?token=…`, so pressing F5 — a fresh
    // request for `/` with no query string — was answered 403 and the whole
    // interface died, leaving only "This page needs the session token."
    let shell = Shell::start();
    for path in ["/main.css", "/app.js", "/", "/favicon.ico", "/connect", "/settings"] {
        let response = shell.get(path);
        let status = status_line(&response);
        assert!(
            status.contains("200") || status.contains("204"),
            "{path} returned {status:?}"
        );
    }

    // A reload is the same request, and has to keep working.
    let again = shell.get("/");
    assert!(status_line(&again).contains("200"), "{again}");
}

#[test]
fn the_page_is_a_page_every_time_it_is_asked_for() {
    let shell = Shell::start();

    let ok = shell.get("/");
    assert!(status_line(&ok).contains("200"), "{ok}");
    // A marker that survives copy changes: the sidebar's own heading, in Chinese.
    assert!(
        body(&ok).contains("红石联机"),
        "the shell page did not render its sidebar: {}",
        body(&ok)
    );
    assert_eq!(
        header_value(&ok, "content-security-policy").map(|csp| csp.contains("frame-ancestors 'none'")),
        Some(true),
        "the page must forbid framing"
    );
    assert_eq!(header_value(&ok, "cache-control"), Some("no-store"));

    // Served verbatim, not rewritten on the way out.
    assert!(
        body(&ok).contains(r#"href="main.css""#),
        "the page's asset references were rewritten: {}",
        body(&ok)
    );
}

#[test]
fn assets_are_served_and_a_range_request_gets_206() {
    let shell = Shell::start();

    let css = shell.get("/main.css");
    assert!(status_line(&css).contains("200"), "{css}");
    assert!(body(&css).contains("--ground: #313131"), "{}", body(&css));

    let js = shell.get("/app.js");
    assert!(status_line(&js).contains("200"), "{js}");
    assert!(body(&js).contains("api/health"), "{}", body(&js));

    let ranged = shell.request(&format!(
        "GET /app.js HTTP/1.1\r\nHost: {}\r\nRange: bytes=0-9\r\nConnection: close\r\n\r\n",
        shell.authority()
    ));
    assert!(status_line(&ranged).contains("206"), "{ranged}");
    assert_eq!(header_value(&ranged, "content-length"), Some("10"));
    assert_eq!(body(&ranged).len(), 10, "{ranged}");

    let unsatisfiable = shell.request(&format!(
        "GET /app.js HTTP/1.1\r\nHost: {}\r\nRange: bytes=999999-\r\nConnection: close\r\n\r\n",
        shell.authority()
    ));
    assert!(status_line(&unsatisfiable).contains("416"), "{unsatisfiable}");
}

#[test]
fn health_reports_a_shell_that_is_up_and_its_kernel_state() {
    let shell = Shell::start();

    let health = shell.get("/api/health");
    let health_body = body(&health);
    assert!(status_line(&health).contains("200"), "{health}");
    assert!(health_body.contains("\"shell\":\"hongshi-shell\""), "{health_body}");

    // The kernel block is a report, not a promise: this runs on a machine that may or
    // may not have `hongshic` installed, and what matters here is that every field the
    // page reads is present and that nothing claims a tunnel is up.
    assert!(health_body.contains("\"kernel\":{"), "{health_body}");
    assert!(health_body.contains("\"running\":false"), "{health_body}");
    assert!(health_body.contains("\"found\":true") || health_body.contains("\"found\":false"), "{health_body}");
    assert!(health_body.contains("\"expected_file\":\"hongshic-"), "{health_body}");
    assert!(health_body.contains("\"platform\":\""), "{health_body}");
    assert!(!health_body.contains("\"running\":true"), "{health_body}");

    assert!(health_body.contains("\"uptime_seconds\":"), "{health_body}");
    assert!(health_body.contains(&format!("\"pid\":{}", shell.child.id())), "{health_body}");

    // The health block no longer carries a session token, and nothing else should
    // have grown one: the gate is where a request came from, not what it carries.
    assert!(!health_body.contains("token"), "{health_body}");
}

#[test]
fn the_kernel_endpoint_names_the_build_this_client_would_download() {
    let shell = Shell::start();
    let response = shell.get("/api/kernel");
    let text = body(&response);
    assert!(status_line(&response).contains("200"), "{response}");

    // The client is the authority on the platform: it runs on the same machine the
    // kernel will, so the name it asks for has to be its own.
    let expected = if cfg!(windows) {
        "hongshic-windows-amd64.exe"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") { "hongshic-macos-arm64" } else { "hongshic-macos-amd64" }
    } else if cfg!(target_arch = "aarch64") {
        "hongshic-linux-arm64"
    } else {
        "hongshic-linux-amd64"
    };
    assert!(text.contains(&format!("\"expected_file\":\"{expected}\"")), "{text}");
    assert!(text.contains("\"core_dir\":\""), "{text}");
}

#[test]
fn asking_to_start_a_tunnel_never_claims_a_tunnel_the_kernel_did_not_make() {
    let shell = Shell::start();
    let response = shell.post(
        "/api/tunnel/start",
        r#"{"relay":"relay.invalid","game_port":25565}"#,
    );
    let text = body(&response);
    let status = status_line(&response).to_string();

    if status.contains("200") {
        // A kernel is installed on this machine: the child starts, and `running`
        // describes the process. What must never appear is an *endpoint*, because
        // `relay.invalid` cannot hand one out — that is the field the user copies and
        // sends to their friends.
        assert!(text.contains("\"endpoint\":null"), "{text}");
        assert!(!text.contains("\"endpoint\":\""), "{text}");
    } else {
        // No kernel: a real status with a reason, never a 200 with a fake tunnel.
        assert!(status.contains("424"), "{status}: {text}");
        assert!(text.contains("no_kernel"), "{text}");
        assert!(!text.contains("\"running\":true"), "{text}");
    }

    // The log is pollable either way, which is how the panel gets the kernel's own
    // words rather than a paraphrase of them.
    let log = shell.get(&format!("/api/kernel/log?since=0"));
    assert!(status_line(&log).contains("200"), "{log}");
    let log_body = body(&log);
    assert!(log_body.contains("\"seq\":"), "{log_body}");
    assert!(log_body.contains("\"kernel\":{"), "{log_body}");

    // Leave nothing running: a stray kernel would hold the game port for the next
    // test, and this one was pointed at a relay that does not exist.
    let stopped = shell.post("/api/tunnel/stop", "{}");

    // A stop has to *be* a stop by the time it answers. `kill` only asks, and a status
    // that still says `running: true` about the process this very call just killed is
    // how a user who presses 开启隧道 straight afterwards is told a tunnel is already
    // up — which is exactly what happened, and it looked like a client that would not
    // start a second tunnel.
    let stopped_text = body(&stopped);
    assert!(
        stopped_text.contains("\"running\":false") || !stopped_text.contains("\"running\":true"),
        "a completed stop still reports a running kernel: {stopped_text}"
    );

    let after = shell.get("/api/tunnel/status");
    assert!(body(&after).contains("\"running\":false"), "{}", body(&after));
}

#[test]
fn a_tunnel_can_be_started_again_right_after_it_is_stopped() {
    let shell = Shell::start();
    let kernel_response = shell.get("/api/kernel");
    let kernel = body(&kernel_response);
    if !kernel.contains("\"found\":true") {
        eprintln!("skipping: no kernel installed on this machine");
        return;
    }

    let start = |tag: &str| {
        let response = shell.post(
            "/api/tunnel/start",
            r#"{"relay":"relay.invalid","game_port":25565}"#,
        );
        let text = body(&response);
        assert!(
            status_line(&response).contains("200"),
            "{tag}: the kernel is installed, so a start should be accepted: {text}"
        );
    };

    start("first");
    // Stop and immediately start again, with no pause: the window in which the child
    // is dead but not yet reaped is the whole bug.
    let _ = shell.post("/api/tunnel/stop", "{}");
    start("second, immediately after a stop");

    let _ = shell.post("/api/tunnel/stop", "{}");
}

#[test]
fn a_posted_body_is_read_and_decoded() {
    let shell = Shell::start();
    let response = shell.post("/api/echo", "a=1&b=hello%20world");
    assert!(status_line(&response).contains("200"), "{response}");
    assert_eq!(body(&response), r#"{"a":"1","b":"hello world"}"#);
}

#[test]
fn a_foreign_origin_cannot_reach_the_api() {
    let shell = Shell::start();
    let response = shell.request(&format!(
        "GET /api/health HTTP/1.1\r\nHost: {}\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n",
        shell.authority()
    ));
    assert!(status_line(&response).contains("403"), "{response}");

    // A page on our own origin is not "foreign", whichever loopback name it used.
    let own = shell.request(&format!(
        "GET /api/health HTTP/1.1\r\nHost: {}\r\nOrigin: http://{}\r\nConnection: close\r\n\r\n",
        shell.authority(),
        shell.authority()
    ));
    assert!(status_line(&own).contains("200"), "{own}");
}

#[test]
fn a_rebound_host_header_is_refused() {
    let shell = Shell::start();
    // The browser is on the attacker's page, and the name resolves to 127.0.0.1,
    // so `Origin` and `Host` agree with each other — the loopback rule is what
    // refuses it, and it is the only thing that does.
    let response = shell.request(
        "GET /api/health HTTP/1.1\r\nHost: evil.example\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n",
    );
    assert!(status_line(&response).contains("403"), "{response}");

    let plain = shell.request("GET /api/health HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n");
    assert!(status_line(&plain).contains("403"), "{plain}");
}

#[test]
fn malformed_requests_are_refused_rather_than_crashing_the_shell() {
    let shell = Shell::start();
    for bad in [
        "GET\r\n\r\n",
        "GET / HTTP/9.9\r\n\r\n",
        "GET http://example.com/ HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        "RUBBISH\r\n\r\n",
        "GET / HTTP/1.1\r\nBadHeaderWithoutColon\r\n\r\n",
    ] {
        // No retry here, deliberately. This used to be flaky and a lenient retry
        // was added; the real cause was that accepted sockets inherited the
        // listener's non-blocking flag, so a refusal could come back as an RST
        // before the client read it. With that fixed the refusal is deterministic,
        // and a retry would hide the next regression of the same kind.
        let response = shell.request(bad);
        assert!(
            status_line(&response).contains("400"),
            "sending {bad:?} produced {:?}",
            status_line(&response)
        );
    }
    // Still alive and serving after all of that.
    let health = shell.get("/api/health");
    assert!(status_line(&health).contains("200"), "{health}");
}

#[test]
fn a_head_request_gets_headers_without_a_body() {
    let shell = Shell::start();
    let response = shell.request(&format!(
        "HEAD / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        shell.authority()
    ));
    assert!(status_line(&response).contains("200"), "{response}");
    assert!(body(&response).is_empty(), "{response}");
    assert!(header_value(&response, "content-length").is_some(), "{response}");
}

#[test]
fn asking_the_shell_to_quit_stops_it() {
    let mut shell = Shell::start();
    let response = shell.post("/api/shutdown", "");
    assert!(status_line(&response).contains("200"), "{response}");
    assert!(body(&response).contains("shutting down"), "{response}");

    let code = shell
        .wait_for_exit(Duration::from_secs(10))
        .expect("the shell should exit after /api/shutdown");
    assert_eq!(
        code,
        0,
        "an orderly shutdown exits 0. stderr was:\n{}",
        shell.stderr()
    );

    // And the port is free again: nothing is listening where it used to be.
    assert!(
        TcpStream::connect(shell.authority()).is_err(),
        "the listener should be gone after shutdown"
    );
}

#[test]
fn an_unknown_option_is_a_usage_error_and_prints_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_hongshi"))
        .args(["--nope"])
        .output()
        .expect("spawn");
    assert_eq!(output.status.code(), Some(2), "usage errors exit 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown option"), "{stderr}");
    assert!(stderr.contains("Usage: hongshi"), "{stderr}");
}

#[test]
fn help_and_version_exit_zero_without_starting_a_server() {
    for flag in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hongshi"))
            .arg(flag)
            .output()
            .expect("spawn");
        assert_eq!(output.status.code(), Some(0), "{flag}");
        assert!(!output.stdout.is_empty(), "{flag} printed nothing");
    }
}

#[test]
fn two_shells_can_run_at_once_because_the_port_is_random() {
    let first = Shell::start();
    let second = Shell::start();
    assert_ne!(first.port, second.port);
    for shell in [&first, &second] {
        let health = shell.get("/api/health");
        assert!(status_line(&health).contains("200"), "{health}");
    }
}

#[test]
fn the_ui_can_be_served_from_a_directory_instead_of_the_binary() {
    let dir = std::env::temp_dir().join(format!("hongshi-shell-web-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create web dir");
    std::fs::write(dir.join("index.html"), "<!doctype html><title>from disk</title>").expect("write");

    let shell = Shell::start_with(&[
        "--no-browser",
        "--web-dir",
        &dir.display().to_string(),
    ]);

    let page = shell.get("/");
    assert!(body(&page).contains("from disk"), "{}", body(&page));

    let health = shell.get("/api/health");
    assert!(
        body(&health).contains(&dir.display().to_string().replace('\\', "\\\\")),
        "health should name the directory it is serving: {}",
        body(&health)
    );

    // An asset that is not on disk still falls back to the embedded copy.
    let css = shell.get("/main.css");
    assert!(status_line(&css).contains("200"), "{css}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_directory_that_does_not_exist_falls_back_to_the_embedded_copy() {
    let missing = PathBuf::from("definitely-not-a-directory-here");
    let shell = Shell::start_with(&["--no-browser", "--web-dir", &missing.display().to_string()]);
    let page = shell.get("/");
    assert!(status_line(&page).contains("200"), "{page}");
    assert!(body(&page).contains("红石联机"), "{}", body(&page));
}
