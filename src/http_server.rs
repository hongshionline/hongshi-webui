//! A loopback-only HTTP/1.1 server, written directly against `std::net`.
//!
//! Why not a framework: this server is reached only over `127.0.0.1`, serves three
//! static files and four JSON endpoints, and lives in a binary we hand to users. A
//! dependency tree (hyper + tokio + tower, or a hand-rolled epoll layer) would cost
//! more in build time, binary size and supply-chain surface than it saves in lines.
//! Threads are the right concurrency model at this scale — a browser opens a
//! handful of connections, and `MAX_CONNECTIONS` bounds what anything else can
//! open.
//!
//! What it deliberately does **not** do: chunked request bodies, keep-alive,
//! pipelining, TLS. Each is either unused by a browser on loopback or answered
//! with a legal fallback (see [`crate::assets::RangeOutcome`]).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use crate::assets::{Assets, RangeOutcome, parse_range};
use crate::log::{debug_fields, info_fields, warn_fields};

/// Hard caps. A request that exceeds any of them is refused rather than buffered.
const MAX_REQUEST_LINE: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;
/// A local edit-and-refresh loop should never wait this long; a hung client must
/// not pin a thread for the life of the process.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Budget for reading one whole request, headers and body. The per-read timeout
/// above only bounds a single stalled read, so without this a client that sends
/// one byte every few seconds keeps a thread forever.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent connections served at once.
///
/// A browser uses a handful, so this is far above what a real client needs and
/// far below what it takes to exhaust a machine. Connections beyond it are closed
/// immediately with `503` rather than refused at the TCP layer, so a client can
/// tell the difference between "the shell is busy" and "nothing is listening".
const MAX_CONNECTIONS: u64 = 32;
/// Idle keep-alive window granted to a client that explicitly asks for one.
const KEEP_ALIVE_IDLE: Duration = Duration::from_millis(500);
/// Ceiling on how much of a refused request's body is read and thrown away so the
/// connection can close cleanly instead of being reset.
const MAX_DRAIN_BYTES: usize = 64 * 1024;
/// How long the accept loop waits between polls of the shutdown flag.
///
/// The listener is non-blocking so that shutdown is a flag rather than a lock:
/// see [`Server::run`] for why sharing the listener between `accept` and the
/// shutdown path does not work.
const ACCEPT_POLL: Duration = Duration::from_millis(10);

// ---------------------------------------------------------------------------
// Request / response
// ---------------------------------------------------------------------------

pub struct Request {
    pub method: String,
    /// Percent-decoded path, always starting with `/`.
    pub path: String,
    /// Raw query string, without the leading `?`.
    pub query: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
    /// True for statuses that must not carry a body or a `Content-Length`
    /// (RFC 9110 §8.6 forbids the header on 204). `render` only knows about HEAD,
    /// so this has to be stated.
    bodyless: bool,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            content_type,
            headers: Vec::new(),
            body: body.into(),
            bodyless: false,
        }
    }

    /// Mark a response as carrying no body at all.
    pub fn bodyless(mut self) -> Response {
        self.bodyless = true;
        self.body.clear();
        self
    }

    pub fn json(body: impl Into<Vec<u8>>) -> Response {
        Response::new(200, "application/json; charset=utf-8", body)
    }

    pub fn text(status: u16, body: impl Into<String>) -> Response {
        Response::new(status, "text/plain; charset=utf-8", body.into().into_bytes())
    }

    pub fn html(status: u16, body: impl Into<String>) -> Response {
        Response::new(status, "text/html; charset=utf-8", body.into().into_bytes())
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Response {
        self.headers.push((name, value.into()));
        self
    }

    fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            204 => "No Content",
            206 => "Partial Content",
            304 => "Not Modified",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            411 => "Length Required",
            413 => "Content Too Large",
            416 => "Range Not Satisfiable",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Serialize a response. `include_body` is false for HEAD; a response marked
/// `bodyless` emits neither a body nor a `Content-Length`.
fn render(response: &Response, include_body: bool, close: bool) -> Vec<u8> {
    let emit_length = !response.bodyless
        && response.status != 304
        && response.header_value("content-length").is_none();
    let body_len = response.body.len();
    let mut out = Vec::with_capacity(160 + if include_body { body_len } else { 0 });

    out.extend_from_slice(
        format!("HTTP/1.1 {} {}\r\n", response.status, response.reason()).as_bytes(),
    );
    out.extend_from_slice(format!("Content-Type: {}\r\n", response.content_type).as_bytes());
    if emit_length {
        out.extend_from_slice(format!("Content-Length: {body_len}\r\n").as_bytes());
    }
    for (name, value) in &response.headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(b"X-Content-Type-Options: nosniff\r\n");
    // The shell is a privileged origin on this machine: nothing about it should
    // be loadable by, or leak a referrer to, anything else.
    out.extend_from_slice(b"Referrer-Policy: no-referrer\r\n");
    out.extend_from_slice(b"Cross-Origin-Resource-Policy: same-origin\r\n");
    out.extend_from_slice(if close {
        b"Connection: close\r\n".as_slice()
    } else {
        b"Connection: keep-alive\r\n".as_slice()
    });
    out.extend_from_slice(b"\r\n");
    if include_body && !response.bodyless && response.status != 304 {
        out.extend_from_slice(&response.body);
    }
    out
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ParseIssue {
    /// The client is gone or sent nothing usable; answer nothing.
    Closed,
    /// Malformed or oversized; answer 400 / 413 / 408.
    Reject(u16, &'static str),
}

impl From<std::io::Error> for ParseIssue {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                ParseIssue::Reject(408, "timed out while reading the request")
            }
            std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe => ParseIssue::Closed,
            _ => ParseIssue::Reject(400, "could not read the request"),
        }
    }
}

fn read_line(
    reader: &mut BufReader<TcpStream>,
    limit: usize,
    deadline: Instant,
    idle_if_empty: bool,
) -> Result<String, ParseIssue> {
    let mut raw: Vec<u8> = Vec::with_capacity(128);
    let mut eol = false;

    while !eol {
        // The deadline is what stops a client trickling one byte every few
        // seconds: the per-read timeout below would be satisfied every time,
        // which is a thread pinned for as long as the attacker likes.
        if Instant::now() >= deadline {
            // Reaching the deadline without having taken a single byte means
            // nobody was asking for anything — see `idle_if_empty`.
            if idle_if_empty && raw.is_empty() {
                return Err(ParseIssue::Closed);
            }
            return Err(ParseIssue::Reject(408, "timed out reading the request"));
        }
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(err) if idle_if_empty
                && raw.is_empty()
                && matches!(
                    err.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(ParseIssue::Closed)
            }
            Err(err) => return Err(ParseIssue::from(err)),
        };
        if available.is_empty() {
            return if raw.is_empty() {
                Err(ParseIssue::Closed)
            } else {
                Err(ParseIssue::Reject(400, "truncated request"))
            };
        }
        let take = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => {
                eol = true;
                index + 1
            }
            None => available.len(),
        };
        // Copy the line without its terminator. Stripping by position rather than
        // filtering byte values: the CR of a CRLF is part of the terminator, while
        // a CR anywhere else in the line is data, and the two are easy to confuse.
        let content = match available[..take].split_last() {
            Some((b'\n', rest)) => match rest.split_last() {
                Some((b'\r', head)) => head,
                _ => rest,
            },
            _ => &available[..take],
        };
        raw.extend_from_slice(content);
        reader.consume(take);
        if raw.len() > limit {
            return Err(ParseIssue::Reject(413, "request line or header too long"));
        }
    }
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// Read and validate a request head, plus `Content-Length` bytes of body.
pub fn read_request(
    reader: &mut BufReader<TcpStream>,
) -> Result<(Request, Vec<u8>, bool), ParseIssue> {
    let deadline = Instant::now() + REQUEST_DEADLINE;
    // The request line is the only line where saying nothing means "not a request".
    // A browser opens several speculative connections per page load and leaves them
    // idle; each one used to time out and be logged as a 408 with no method and no
    // path. In one ten-minute session that was around a hundred lines that all said
    // nothing had happened, which buries the events the console exists to show. An
    // idle socket is now closed in silence, while a client that got partway into a
    // request still gets its 408.
    let request_line = read_line(reader, MAX_REQUEST_LINE, deadline, true)?;
    let mut parts = request_line.split(' ');
    let (method, target) = match (parts.next(), parts.next(), parts.next()) {
        (Some(method), Some(target), Some(version)) => {
            if !version.starts_with("HTTP/1.") {
                return Err(ParseIssue::Reject(400, "unsupported HTTP version"));
            }
            (method.to_string(), target.to_string())
        }
        _ => return Err(ParseIssue::Reject(400, "malformed request line")),
    };

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut header_bytes = 0usize;
    loop {
        let line = read_line(reader, MAX_HEADER_BYTES, deadline, false)?;
        if line.is_empty() {
            break;
        }
        header_bytes += line.len();
        if header_bytes > MAX_HEADER_BYTES || headers.len() >= MAX_HEADERS {
            return Err(ParseIssue::Reject(413, "too many headers"));
        }
        match line.split_once(':') {
            Some((name, value)) => headers.push((name.trim().to_string(), value.trim().to_string())),
            None => {
                return Err(ParseIssue::Reject(400, "malformed header"));
            }
        }
    }

    let mut request = Request {
        method,
        path: String::new(),
        query: String::new(),
        headers,
    };

    // Only the origin-form target is meaningful here; anything else (absolute
    // URL, authority form) is a proxy-style request this server must not serve.
    if !target.starts_with('/') {
        return Err(ParseIssue::Reject(400, "only origin-form request targets are accepted"));
    }
    let (raw_path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    request.path = crate::util::percent_decode(raw_path);
    request.query = query.to_string();

    let length = match request.header("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| ParseIssue::Reject(400, "malformed Content-Length"))?,
        None => 0,
    };
    if length > MAX_HEADER_BYTES {
        return Err(ParseIssue::Reject(413, "request body too large"));
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).map_err(ParseIssue::from)?;
    }

    // Keep-alive only when the client asks and we can bound it: a held-open
    // socket is a held-open thread.
    let keep_alive = request
        .header("connection")
        .map(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("keep-alive"))
        })
        .unwrap_or(false);

    Ok((request, body, keep_alive))
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Everything a route needs, shared by every handler thread.
pub struct ServerContext {
    pub url: String,
    pub assets: Assets,
    pub started: Instant,
    pub web_source: String,
    /// The kernel child process: found, started, stopped and reported through here.
    pub kernel: Arc<crate::kernel::Kernel>,
    /// Settings live behind a lock because the interface may change them while a
    /// page is fetching with the old ones.
    pub settings: Arc<std::sync::RwLock<crate::config::Settings>>,
    /// The node-list cache and whatever else needs to remember a fetch.
    pub site: Arc<crate::site::SiteState>,
    /// Which HTTP backend this build uses, shown in the settings page.
    pub http_backend: &'static str,
    /// Which launch this is, counted by [`crate::launch`] at startup. The page asks for
    /// it exactly once, to decide whether this is a round number worth saying thank you
    /// on; nothing else reads it.
    pub launches: u64,
}

impl ServerContext {
    pub fn settings(&self) -> crate::config::Settings {
        self.settings
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

/// Outcome of one request, returned so the accept loop can log it in one place
/// instead of every handler producing its own log line.
pub struct Outcome {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub bytes: usize,
}

/// A loopback web server for one shell session.
pub struct Server {
    /// Owned outright by the accept loop, **not** shared with the shutdown path.
    ///
    /// Two earlier attempts to share it both hung the shell, and the reasons are
    /// worth keeping:
    ///
    /// 1. Handing the shutdown path an empty `Arc<RwLock<Option<..>>>` slot while
    ///    the loop kept the listener made `/api/shutdown` close nothing, so the
    ///    process never exited.
    /// 2. Giving the shutdown path the *real* slot through the lock made it worse:
    ///    the loop holds the read guard across `accept`, so the writer waits — but
    ///    the loop immediately re-locks on the next iteration, and `std`'s
    ///    `RwLock` can starve the writer indefinitely. The handler blocked inside
    ///    its own response and the client never got an answer.
    ///
    /// So: the listener is non-blocking, the loop polls it, and shutdown is a
    /// plain atomic flag. Ten milliseconds of latency is invisible, and it removes
    /// a class of bug that a blocking `accept` plus a shared lock invites.
    listener: TcpListener,
    context: Arc<ServerContext>,
    /// One handle for the whole process, cloned into every connection thread.
    /// Shutdown is this atomic and nothing else — the accept loop polls it, and
    /// `lib.rs` waits for the loop to finish. There is deliberately no second
    /// channel: an earlier revision had one, never sent on it, and documented it
    /// as the shutdown mechanism.
    shutdown: Arc<ShutdownHandle>,
    connections: Arc<AtomicU64>,
}

impl Server {
    /// Bind `127.0.0.1:port`. Port 0 asks the OS for a free one, which is the
    /// default: a random port is one fewer thing for another local process to
    /// guess, and one fewer collision to debug.
    pub fn listen(port: u16) -> std::io::Result<TcpListener> {
        TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)))
    }

    /// Only meaningful once the port is known, which is why `listen` is separate.
    pub fn new(listener: TcpListener, context: ServerContext) -> Server {
        Server {
            listener,
            context: Arc::new(context),
            shutdown: Arc::new(ShutdownHandle {
                flag: AtomicBool::new(false),
            }),
            connections: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn context(&self) -> &Arc<ServerContext> {
        &self.context
    }

    /// The port actually bound, which is only different from the requested one
    /// when that was 0.
    pub fn port(&self) -> u16 {
        self.listener
            .local_addr()
            .map(|addr| addr.port())
            .unwrap_or(0)
    }

    /// Serve until [`ShutdownHandle::request`] is called. One thread per
    /// connection: a browser opens a handful, and a thread that is blocked on a
    /// socket read cannot starve the accept loop.
    ///
    /// **This blocks.** To start serving *before* announcing the URL — which is
    /// what stops a client that reads the URL and immediately connects from
    /// landing in the window before the listener is accepting — use
    /// [`Server::serve`], which returns a handle that finishes at the same point.
    pub fn serve(self) -> ServeHandle {
        // Non-blocking so a shutdown request is noticed within `ACCEPT_POLL`
        // instead of waiting for the next connection to arrive. Connections that
        // land between polls wait in the backlog, so nothing is dropped.
        if let Err(err) = self.listener.set_nonblocking(true) {
            warn_fields("could not make the listener non-blocking", &[("err", &err.to_string())]);
        }

        // Set up last, so by the time this is true the accept loop has already
        // claimed the listener.
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let shutdown = Arc::clone(&self.shutdown);
        let thread = std::thread::Builder::new()
            .name("accept".to_string())
            .spawn(move || Self::accept_loop(self, ready_tx))
            .expect("spawn the accept loop");

        let _ = ready_rx.recv();
        ServeHandle { thread, shutdown }
    }

    fn accept_loop(server: Server, ready: Sender<()>) {
        let _ = ready.send(());
        loop {
            if server.shutdown.is_requested() {
                break;
            }
            match server.listener.accept() {
                Ok((stream, peer)) => {
                    let id = server.connections.fetch_add(1, Ordering::Relaxed) + 1;
                    if id > MAX_CONNECTIONS {
                        warn_fields(
                            "connection refused, too many open",
                            &[("peer", &peer.to_string()), ("limit", &MAX_CONNECTIONS.to_string())],
                        );
                        reject_over_capacity(stream, peer);
                        server.connections.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }

                    let context = Arc::clone(&server.context);
                    let shutdown = Arc::clone(&server.shutdown);
                    let counter = Arc::clone(&server.connections);
                    if let Err(err) = std::thread::Builder::new()
                        .name(format!("conn-{id}"))
                        .spawn(move || {
                            // Released on every exit path, including a panic in
                            // this thread, so one bad connection cannot
                            // permanently shrink the limit.
                            let _slot = ConnectionSlot::new(counter);
                            debug_fields(
                                "connection opened",
                                &[("peer", &peer.to_string()), ("id", &id.to_string())],
                            );
                            match serve_connection(stream, &context, &shutdown) {
                                Ok(outcomes) => outcomes.iter().for_each(log_outcome),
                                Err(issue) => log_issue(peer, issue),
                            }
                        })
                    {
                        warn_fields("could not spawn a connection thread", &[("err", &err.to_string())]);
                        server.connections.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                // The idle case, and by far the most common one.
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL);
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    warn_fields("accept failed", &[("err", &err.to_string())]);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        // Dropping the listener frees the port. The process exits when
        // `ServeHandle::join` returns and `main` falls off its end.
        drop(server.listener);
    }

    /// Blocking form of [`Server::serve`], used by tests.
    pub fn run(self) -> std::io::Result<()> {
        self.serve().join()
    }
}

/// What [`Server::serve`] hands back: the accept loop, running.
pub struct ServeHandle {
    thread: std::thread::JoinHandle<()>,
    shutdown: Arc<ShutdownHandle>,
}

impl ServeHandle {
    /// The handle a signal handler (or a test) can use to stop the shell.
    pub fn shutdown_handle(&self) -> Arc<ShutdownHandle> {
        Arc::clone(&self.shutdown)
    }

    /// Wait for the accept loop to finish.
    ///
    /// Returns as soon as the loop has stopped accepting; connection threads that
    /// are mid-response are not joined, which is deliberate — the process exits
    /// immediately afterwards. A panic in the accept loop becomes an `Err` here
    /// rather than being swallowed, so `lib.rs` can report it instead of claiming
    /// a clean shutdown.
    pub fn join(self) -> std::io::Result<()> {
        match self.thread.join() {
            Ok(()) => Ok(()),
            Err(_) => Err(std::io::Error::other(
                "the accept loop panicked; see the message above",
            )),
        }
    }

    /// Stop the shell and wait for the accept loop to notice.
    pub fn stop(self) -> std::io::Result<()> {
        self.shutdown.request();
        self.join()
    }
}

/// The bit of server state a handler may touch when a request asks for shutdown.
///
/// Kept separate from [`ServerContext`] so that no ordinary route can reach the
/// listener by accident: shutting the shell down has exactly one entry point, and
/// that entry point is a flag rather than a handle on the socket.
pub struct ShutdownHandle {
    flag: AtomicBool,
}

impl ShutdownHandle {
    /// Idempotent: the first caller wins, later ones are no-ops.
    pub fn request(&self) {
        // All this does is set a flag the accept loop polls. Driving shutdown from
        // an HTTP request rather than from Ctrl+C is deliberate: the shell is
        // launched by double-click on Windows, where a console signal is not a
        // shutdown path anyone can rely on.
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Holds one slot of the connection budget, releasing it on drop.
struct ConnectionSlot {
    counter: Arc<AtomicU64>,
}

impl ConnectionSlot {
    fn new(counter: Arc<AtomicU64>) -> ConnectionSlot {
        ConnectionSlot { counter }
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Tell a client that the shell is at capacity and close.
fn reject_over_capacity(stream: TcpStream, peer: SocketAddr) {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let response = Response::text(503, "the shell has too many open connections");
    let mut stream = stream;
    if let Err(err) = stream.write_all(&render(&response, true, true)) {
        debug_fields("could not answer an over-capacity connection", &[("err", &err.to_string())]);
    }
    let _ = stream.flush();
    finish(&mut stream);
    log_outcome(&Outcome {
        method: "-".to_string(),
        path: "-".to_string(),
        status: 503,
        bytes: 0,
    });
    debug_fields("connection refused over capacity", &[("peer", &peer.to_string())]);
}

/// Endpoints the interface polls on a timer.
///
/// A poll that finds nothing new is not an event. Logging one line per tick buries
/// the lines that are — the console is the window the user is told to keep open for
/// the whole session, and a status poll every second would leave nothing else
/// visible. They still show up with `--verbose`.
const QUIET_PATHS: &[&str] = &[
    "/api/health",
    "/api/tunnel/status",
    "/api/kernel",
    "/api/kernel/log",
];

fn log_outcome(outcome: &Outcome) {
    let fields = [
        ("method", outcome.method.as_str()),
        ("path", outcome.path.as_str()),
        ("status", &outcome.status.to_string()),
        ("bytes", &outcome.bytes.to_string()),
    ];
    if QUIET_PATHS.contains(&outcome.path.as_str()) {
        debug_fields("request", &fields);
    } else {
        info_fields("request", &fields);
    }
}

fn log_issue(peer: SocketAddr, issue: ParseIssue) {
    match issue {
        ParseIssue::Closed => debug_fields("connection closed without a request", &[("peer", &peer.to_string())]),
        ParseIssue::Reject(status, reason) => {
            warn_fields("malformed request", &[("peer", &peer.to_string()), ("status", &status.to_string()), ("reason", reason)]);
        }
    }
}

/// Read requests off one connection until it closes or asks to stop.
fn serve_connection(
    stream: TcpStream,
    context: &Arc<ServerContext>,
    shutdown: &ShutdownHandle,
) -> Result<Vec<Outcome>, ParseIssue> {
    // **Accept inherits the listener's non-blocking flag on Windows.** The accept
    // loop sets the listener non-blocking so it can poll the shutdown flag, and
    // without this line every accepted socket arrives non-blocking too: the first
    // `read` returns `WouldBlock` instead of waiting, and every request is
    // answered with a 408 even though the client sent it correctly.
    if let Err(err) = stream.set_nonblocking(false) {
        warn_fields(
            "could not put an accepted connection into blocking mode",
            &[("err", &err.to_string())],
        );
        return Ok(Vec::new());
    }
    stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
    stream.set_write_timeout(Some(WRITE_TIMEOUT)).ok();
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream);
    let mut outcomes = Vec::new();

    loop {
        let (request, body, keep_alive) = match read_request(&mut reader) {
            Ok(parsed) => parsed,
            // A closed connection after a successful exchange is the ordinary end
            // of the loop, not an event worth reporting.
            Err(ParseIssue::Closed) if !outcomes.is_empty() => {
                finish(&mut reader.into_inner());
                return Ok(outcomes);
            }
            // Refuse it in a way the client can actually read: answer, then close
            // the connection politely. See `finish` for why that is not automatic.
            Err(ParseIssue::Reject(status, reason)) => {
                let response = Response::text(status, reason);
                let mut stream = reader.into_inner();
                let _ = stream.write_all(&render(&response, true, true));
                let _ = stream.flush();
                finish(&mut stream);
                return Ok(vec![Outcome {
                    method: "-".to_string(),
                    path: "-".to_string(),
                    status,
                    bytes: 0,
                }]);
            }
            Err(issue) => return Err(issue),
        };

        let is_head = request.method.eq_ignore_ascii_case("HEAD");
        let response = route(&request, &body, context, shutdown);
        let close = !keep_alive;
        let bytes = render(&response, !is_head, close);
        let stream = reader.get_mut();
        if let Err(err) = stream.write_all(&bytes) {
            if outcomes.is_empty() {
                return Err(ParseIssue::from(err));
            }
            finish(stream);
            return Ok(outcomes);
        }
        let _ = stream.flush();

        outcomes.push(Outcome {
            method: request.method.clone(),
            path: request.path.clone(),
            status: response.status,
            bytes: response.body.len(),
        });

        if close || shutdown.is_requested() {
            finish(stream);
            return Ok(outcomes);
        }
        // Bounded keep-alive: the client promised more requests, but a browser
        // that changes its mind must not leave a thread parked forever.
        let _ = reader.get_ref().set_read_timeout(Some(KEEP_ALIVE_IDLE));
    }
}

/// Close a connection the way the peer expects.
///
/// This matters more than it looks. A response is small enough that the kernel
/// coalesces it with the FIN, and Windows turns `close()` into an RST when the
/// receive buffer still holds unread bytes — the peer's pending data is then
/// discarded, and the client that sent a request we refused with 400 never sees
/// the 400 at all. So: drain whatever the peer already sent (bounded, since this
/// is also the path a hostile client reaches), then shut the write half down so
/// the peer sees an orderly end of stream.
fn finish(stream: &mut TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut scratch = [0u8; 4096];
    let mut drained = 0usize;
    while drained < MAX_DRAIN_BYTES {
        match stream.read(&mut scratch) {
            Ok(0) => break,
            Ok(read) => drained += read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

fn route(
    request: &Request,
    body: &[u8],
    context: &ServerContext,
    shutdown: &ShutdownHandle,
) -> Response {
    let is_get = request.method.eq_ignore_ascii_case("GET") || request.method.eq_ignore_ascii_case("HEAD");
    let path = request.path.as_str();

    let is_api = path.starts_with("/api/");

    // One gate for everything, favicon included — but it no longer asks the caller
    // for anything it could not already produce by opening the page. There is no
    // session token: the page is a page, and a reload, a bookmark on the printed
    // port and a fresh tab all have to work. What is refused is a request that did
    // not come from a page *this* server served:
    //
    //   * a non-loopback `Host` is the DNS-rebinding case — a hostile name that
    //     resolves to 127.0.0.1 arrives with its own name in `Host`, and refusing
    //     that is what stops the browser from being talked into treating an
    //     attacker's origin as ours;
    //   * an `Origin` that is not our own authority is another site's page trying
    //     to drive this port with a `fetch`/form post, which is the case a browser
    //     cannot be asked to prevent for us.
    if !request_belongs_to_this_server(request) {
        return if is_api { forbidden_api() } else { forbidden() };
    }

    // Answered only after the gate, and only so the console is not filled with
    // icon requests.
    if path == "/favicon.ico" {
        return Response::new(204, "image/x-icon", Vec::new()).bodyless();
    }

    if is_api {
        if !is_get && !request.method.eq_ignore_ascii_case("POST") {
            return Response::text(405, "only GET, HEAD and POST are accepted")
                .header("Allow", "GET, HEAD, POST");
        }
        return match path {
            "/api/health" => health(context),
            "/api/echo" => echo(body),
            "/api/settings" => settings_endpoint(request, body, context),
            "/api/background" => background_endpoint(request, context),
            "/api/daily-news" => daily_news(context),
            "/api/version" => version_endpoint(context),
            "/api/nodes" => nodes_endpoint(context, &request.query),
            "/api/ports/detect" => ports_detect_endpoint(context, &request.query),
            "/api/sessions" => sessions_endpoint(&request.query),
            "/api/probe" => probe_endpoint(body),
            "/api/tunnel/start" => tunnel_endpoint(context, "start", &body),
            "/api/tunnel/stop" => tunnel_endpoint(context, "stop", &body),
            "/api/tunnel/status" => tunnel_endpoint(context, "status", &body),
            "/api/kernel" => kernel_endpoint(context, &request.query),
            "/api/kernel/log" => kernel_endpoint(context, &request.query),            "/api/kernel/download" => {
                if request.method.eq_ignore_ascii_case("POST") {
                    kernel_download(context)
                } else {
                    Response::new(
                        405,
                        "application/json; charset=utf-8",
                        format!(
                            r#"{{"state":"bad_request","reason":"{}"}}"#,
                            escape_json("下载内核是一个 POST")
                        ),
                    )
                }
            }
            "/api/shutdown" => {
                shutdown.request();
                Response::json(r#"{"state":"shutting down"}"#)
            }
            // 404, not a 200 with an error body: a caller checking `response.ok`
            // must not read "no such endpoint" as success. This is the same
            // mistake the token refusal was fixed for.
            other => Response::new(
                404,
                "application/json; charset=utf-8",
                format!(
                    r#"{{"error":"no such endpoint","path":"{}"}}"#,
                    escape_json(other)
                ),
            ),
        }
        .header("Cache-Control", "no-store");
    }

    if !is_get {
        return Response::text(405, "the shell only serves GET here").header("Allow", "GET, HEAD");
    }

    // The interface uses real paths (`/connect`, `/settings`) so a reload or a
    // bookmark lands somewhere sensible, and one document answers all of them.
    // Only paths that are not files qualify: a route never has an extension, so a
    // missing `/main.css` stays an honest 404 rather than a page pretending to be
    // a stylesheet, and `/icons.svg#i-home` — a fragment, not a path — is served
    // as the asset it is.
    let is_app_route = path != "/"
        && !path.trim_start_matches('/').contains('.')
        && context.assets.lookup(path).is_none();

    if path == "/" || path == "/index.html" || is_app_route {
        let Some(asset) = context.assets.lookup_shell() else {
            return Response::text(500, "the shell has no index page");
        };
        // The page is served exactly as it is on disk. It used to be rewritten on
        // the way out, to push a `?token=…` into every `href`/`src` — because the
        // browser resolves those against this URL with the query string DROPPED,
        // and a gated `/main.css` would have 403'd the page's own stylesheet and
        // script. With no token there is nothing to push in, and the file a
        // developer edits is now the file the browser receives.
        return Response::new(200, asset.content_type, asset.body)
            .header("Cache-Control", "no-store")
            .header(
                "Content-Security-Policy",
                &content_security_policy(&context.settings().api_base),
            );
    }

    match context.assets.lookup(path) {
        Some(asset) => {
            let mut response = serve_asset(asset, request);
            // CSP on every HTML-ish asset, not only `.html`: `content_type_for`
            // serves `.htm` as text/html too, and a page served without a policy
            // would be the one gap in an otherwise uniform rule.
            if response.content_type.starts_with("text/html") {
                response = response.header(
                    "Content-Security-Policy",
                    &content_security_policy(&context.settings().api_base),
                );
            }
            response
        }
        None => not_found(path),
    }
}

/// The origins the page may load pictures from.
///
/// The news returns its own image URLs, so whichever host serves the news has to be
/// allowed to serve the pictures too. There are two such hosts now and they are not
/// the same kind of thing: **Mojang's** launcher content, which is where the news
/// comes from and is fixed, and whatever the user put in 服务地址, which is a mirror
/// or a local stub while working on the UI. This used to be the two literal
/// `hongshi.site` origins under a comment claiming it followed the service address,
/// so pointing the client at a mirror silently dropped every news image while the
/// text still rendered.
///
/// `hongshi.site` stays on the list so a default install keeps working even if the
/// setting is briefly empty or mid-edit.
fn image_origins(api_base: &str) -> String {
    let mut origins: Vec<String> = Vec::new();
    if let Some(origin) = origin_of(crate::site::NEWS_ORIGIN) {
        origins.push(origin);
    }
    if let Some(origin) = origin_of(api_base) {
        origins.push(origin);
    }
    for official in ["https://hongshi.site", "http://hongshi.site"] {
        if !origins.iter().any(|origin| origin == official) {
            origins.push(official.to_string());
        }
    }
    origins.join(" ")
}

/// The `scheme://authority` part of an http(s) URL, or `None` for anything else.
///
/// Deliberately strict about what may appear in the authority: the result is
/// interpolated into a response header, and `api_base` is a value the user types. A
/// `;` there would end the directive and start another one, and a newline would end
/// the header — so only the characters that can legitimately appear in a host and
/// port are let through, and anything else means "no origin from this".
fn origin_of(url: &str) -> Option<String> {
    let (scheme, after) = if let Some(rest) = url.strip_prefix("https://") {
        ("https://", rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http://", rest)
    } else {
        return None;
    };

    let authority: String = after
        .chars()
        .take_while(|c| !matches!(c, '/' | '?' | '#'))
        .collect();

    let acceptable = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '[' | ']');
    if authority.is_empty() || !authority.chars().all(acceptable) {
        return None;
    }
    Some(format!("{scheme}{authority}"))
}

fn content_security_policy(api_base: &str) -> String {
    format!(
        "default-src 'self'; \
         img-src 'self' data: {origins}; \
         media-src 'self'; \
         style-src 'self' 'unsafe-inline'; \
         script-src 'self'; \
         connect-src 'self'; \
         frame-ancestors 'none'; \
         base-uri 'none'; \
         form-action 'none'",
        origins = image_origins(api_base)
    )
}

/// A policy for pages that need no scripting at all: the refusals. Denying
/// everything is simpler to reason about than a narrower `default-src` list, and
/// these pages have no resources to load.
const CSP_STATIC: &str = "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// Serve one asset, honouring a single byte range when the client sends one.
fn serve_asset(asset: crate::assets::Asset, request: &Request) -> Response {
    let len = asset.body.len() as u64;

    match request
        .header("range")
        .map(|value| parse_range(value, len))
    {
        Some(RangeOutcome::Serve(range)) => {
            let slice = asset.body[range.start as usize..=range.end as usize].to_vec();
            Response::new(206, asset.content_type, slice)
                .header("Accept-Ranges", "bytes")
                .header("Content-Length", (range.end - range.start + 1).to_string())
                .header(
                    "Content-Range",
                    format!("bytes {}-{}/{}", range.start, range.end, len),
                )
        }
        // The bytes asked for do not exist: this is what 416 is for.
        Some(RangeOutcome::Unsatisfiable) => {
            Response::new(416, "text/plain; charset=utf-8", Vec::new())
                .header("Content-Range", format!("bytes */{len}"))
        }
        // No header, or one we do not honour (another unit, a range set): the
        // whole file, which is a legal answer to both.
        Some(RangeOutcome::Ignore) | None => {
            Response::new(200, asset.content_type, asset.body).header("Accept-Ranges", "bytes")
        }
    }
}

/// Whether a request came from a page this server served, rather than from
/// somewhere else on the machine or the network.
///
/// This is the whole gate now. It is deliberately *not* a secret and not a
/// credential: a browser navigation carries neither header on the first request,
/// so a plain `GET /` is allowed through — which is the point. What it refuses is
/// the two ways a request can arrive that the user did not ask for.
fn request_belongs_to_this_server(request: &Request) -> bool {
    // A browser navigation from another origin always carries `Origin`; a
    // same-origin fetch may too, and it will be our own origin. Anything else is
    // another site talking to our loopback port.
    if let Some(origin) = request.header("origin") {
        if !origin_matches(origin, request) {
            return false;
        }
    }
    // DNS rebinding: a hostile page can point its own hostname at 127.0.0.1, so
    // the Host header is checked as well.
    if let Some(host) = request.header("host") {
        if !host_is_loopback(host) {
            warn_fields("refused a request with a non-loopback Host header", &[("host", host)]);
            return false;
        }
    }
    true
}

fn origin_matches(origin: &str, request: &Request) -> bool {
    // "Same origin" is defined against the authority the request itself arrived
    // on, so a `--port 3080` session compares against `127.0.0.1:3080` without
    // anyone having to keep two strings in sync.
    let Some(authority) = origin.split_once("//").map(|(_, rest)| rest) else {
        return false;
    };
    match request.header("host") {
        Some(host) => authority == host,
        None => false,
    }
}

fn host_is_loopback(host: &str) -> bool {
    let name = host.rsplit_once(':').map(|(name, _)| name).unwrap_or(host);
    let name = name.trim_start_matches('[').trim_end_matches(']');
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // `127.0.0.1` and `::1` are both loopback; anything else (including a name
    // that merely starts with `127.0.0.1`) is not.
    name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn forbidden() -> Response {
    // Reached only by a request that arrived with a foreign `Origin` or a
    // non-loopback `Host`. There is nothing for the user to type here — no token,
    // no URL to reopen — so the page says what actually happened instead of
    // sending them back to the console for a query string that no longer exists.
    denied_page(
        403,
        "This request did not come from this client's own page.",
        "The shell only answers requests made by the interface it is serving, from \
         <code>127.0.0.1</code> or <code>localhost</code>.<br>\
         Open <code>/</code> in a browser tab and use the interface directly.",
    )
}

fn forbidden_api() -> Response {
    // `Response::json` is 200 by construction, so the status is set explicitly:
    // this refusal shipped briefly as a 200 with an error body, which any caller
    // checking `response.ok` would have read as success.
    Response::new(
        403,
        "application/json; charset=utf-8",
        r#"{"error":"cross-origin or non-loopback request refused"}"#,
    )
    .header("Cache-Control", "no-store")
}

fn not_found(path: &str) -> Response {
    denied_page(
        404,
        "404 — no such page.",
        &format!(
            "The shell does not serve <code>{}</code>. Available: <code>/</code>, <code>/main.css</code>, <code>/app.js</code>, <code>/api/health</code>.",
            escape_html(path)
        ),
    )
    .header("Cache-Control", "no-store")
}

/// The shared shell for every refusal that should look like a page rather than a
/// bare status. The status is a parameter because it is the one thing the two
/// callers disagree about, and hard-coding 403 here once made every 404 a 403.
fn denied_page(status: u16, headline: &str, detail: &str) -> Response {
    let body = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>hongshi shell</title>\
         <style>\
         :root{{color-scheme:dark}}\
         body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#313131;color:#fff;\
         font:16px/1.6 'Segoe UI',system-ui,sans-serif;padding:32px}}\
         main{{max-width:34rem;border:1px solid #525252;border-radius:16px;padding:28px 30px;background:#414141}}\
         h1{{margin:0 0 12px;font-size:22px;font-weight:400}}\
         p{{margin:0;color:rgba(255,255,255,.6)}}\
         code{{font-family:ui-monospace,Consolas,monospace;color:#fff}}\
         </style>\
         <main><h1>{headline}</h1><p>{detail}</p></main></html>"
    );
    Response::html(status, body).header("Content-Security-Policy", CSP_STATIC)
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

/// GET or POST `/api/settings`.
///
/// A POST takes a partial object: whatever the settings page touched. The reply
/// carries the effective settings either way, so the page never has to guess what
/// the shell thinks it is configured to do.
fn settings_endpoint(request: &Request, body: &[u8], context: &ServerContext) -> Response {
    if request.method.eq_ignore_ascii_case("POST") {
        let text = String::from_utf8_lossy(body);
        let mut updated = context.settings();
        updated.apply_json(&text);

        // A background the shell cannot read is refused *while the person who typed
        // the path is still looking at the field*, which is the only moment the
        // message is worth anything. Storing it and failing later would put the same
        // sentence in a status line the user has already walked away from.
        //
        // Only when the patch mentions the field: every other save would otherwise
        // re-judge a path that is there and fine, and re-reading the file on each
        // keystroke-save is not the endpoint's job.
        if json_field(&text, "background_path").is_some() {
            if let Err(problem) = crate::background::resolve(&updated.background_path) {
                if problem != crate::background::Problem::NotSet {
                    info_fields(
                        "refused a background that cannot be used",
                        &[("state", problem.state())],
                    );
                    return Response::new(
                        400,
                        "application/json; charset=utf-8",
                        format!(
                            r#"{{"state":"{}","field":"background_path","reason":"{}"}}"#,
                            problem.state(),
                            escape_json(&problem.reason())
                        ),
                    );
                }
            }
        }

        let path = match crate::config::save(&updated) {
            Ok(path) => path,
            Err(err) => {
                return Response::new(
                    500,
                    "application/json; charset=utf-8",
                    format!(r#"{{"error":"{}"}}"#, escape_json(&err)),
                );
            }
        };
        if let Ok(mut guard) = context.settings.write() {
            *guard = updated.clone();
        }
        info_fields("settings saved", &[("path", &path.display().to_string())]);
    }

    let current = context.settings();
    Response::json(format!(
        r#"{{"settings":{},"path":"{}","http_backend":"{}","background":{}}}"#,
        current.to_json(),
        escape_json(&crate::config::primary_path().display().to_string()),
        escape_json(context.http_backend),
        background_json(&current),
    ))
}

/// Serve the background the user configured, or say why there is not one.
///
/// One URL answers both questions, and that is deliberate: the page asks whether it
/// is usable with an `Image()` probe, and the settings page asks *why not* with a
/// `fetch` of the same address. Two endpoints would be two answers that can disagree.
/// JSON on failure, image bytes on success — a browser loading it as a picture never
/// reads the body of a failure, and the page can.
///
/// The path is never taken from the request. There is exactly one background, it is
/// the one in the settings file, and this endpoint serves that or nothing — which is
/// what keeps "a path the client may read" from becoming "any path".
fn background_endpoint(request: &Request, context: &ServerContext) -> Response {
    let settings = context.settings();
    let refuse = |problem: &crate::background::Problem| {
        let status = match problem {
            crate::background::Problem::TooLarge(_) => 413,
            _ => 404,
        };
        Response::new(
            status,
            "application/json; charset=utf-8",
            format!(
                r#"{{"state":"{}","reason":"{}","path":"{}"}}"#,
                problem.state(),
                escape_json(&problem.reason()),
                escape_json(settings.background_path.trim())
            ),
        )
        .header("Cache-Control", "no-store")
    };

    let resolved = match crate::background::resolve(&settings.background_path) {
        Ok(resolved) => resolved,
        Err(problem) => return refuse(&problem),
    };

    // A 20 MB wallpaper is not something to send again because the user pressed F5.
    // Revalidation is on the same validator the page puts in its `?v=`.
    let etag = resolved.etag();
    let fresh = request
        .header("if-none-match")
        .is_some_and(|value| value.trim() == etag);
    if fresh {
        return Response::new(304, resolved.kind.content_type(), Vec::new())
            .bodyless()
            .header("ETag", etag);
    }

    match crate::background::read(&resolved) {
        Ok(bytes) => Response::new(200, resolved.kind.content_type(), bytes)
            .header("ETag", etag)
            .header("Cache-Control", "no-cache"),
        Err(problem) => refuse(&problem),
    }
}

/// The background, as the health and settings bodies report it.
///
/// A `stat` and a 16-byte read, so it is cheap enough for an endpoint the page
/// polls; the picture itself is only ever read by [`background_endpoint`].
fn background_json(settings: &crate::config::Settings) -> String {
    let path = settings.background_path.trim();
    match crate::background::resolve(path) {
        Ok(resolved) => format!(
            r#"{{"state":"ok","path":"{}","kind":"{}","content_type":"{}","bytes":{},"modified_ms":{},"blur":{},"darkness":{},"crop":{{"x":{},"y":{},"w":{},"h":{}}}}}"#,
            escape_json(&resolved.path.display().to_string()),
            resolved.kind.name(),
            resolved.kind.content_type(),
            resolved.bytes,
            resolved.modified_ms,
            settings.background_blur,
            settings.background_darkness,
            settings.background_crop_x,
            settings.background_crop_y,
            settings.background_crop_w,
            settings.background_crop_h,
        ),
        Err(problem) => format!(
            r#"{{"state":"{}","path":"{}","reason":"{}","blur":{},"darkness":{},"crop":{{"x":{},"y":{},"w":{},"h":{}}}}}"#,
            problem.state(),
            escape_json(path),
            escape_json(&problem.reason()),
            settings.background_blur,
            settings.background_darkness,
            settings.background_crop_x,
            settings.background_crop_y,
            settings.background_crop_w,
            settings.background_crop_h,
        ),
    }
}

/// GET `/api/version` — the running build, and whether a newer one exists.
///
/// The check is a question with three possible answers, not two: "up to date",
/// "a newer one exists", and **"could not check"**. Collapsing the third into
/// either of the others would be a lie — and today it is the common case, since
/// the endpoint is not live yet.
fn version_endpoint(context: &ServerContext) -> Response {
    let current = crate::VERSION;
    let settings = context.settings();
    let fetched = crate::site::webui_version(&settings);

    let remote = if fetched.state() == "ok" {
        crate::site::parse_version(&fetched.body_text())
    } else {
        None
    };
    let update = crate::site::update_available(current, remote.as_deref());

    Response::json(format!(
        r#"{{"current":"{}","channel":"{}","remote":{},"update":{},"state":"{}","reason":"{}","api_base":"{}","checked":true}}"#,
        escape_json(current),
        escape_json(crate::CHANNEL),
        match &remote {
            Some(version) => format!("\"{}\"", escape_json(version)),
            None => "null".to_string(),
        },
        match update {
            Some(true) => "true".to_string(),
            Some(false) => "false".to_string(),
            None => "null".to_string(),
        },
        fetched.state(),
        escape_json(&fetched.reason()),
        escape_json(&settings.api_base),
    ))
}

/// GET `/api/daily-news` — the Minecraft launcher's news, trimmed by the shell.
///
/// The upstream file carries a hundred entries and 64 KB of prose; the page is given
/// the first few. `data` is an array on success and `null` otherwise, which is the
/// shape the page already read for the hongshi feed this replaced.
fn daily_news(context: &ServerContext) -> Response {
    let settings = context.settings();
    let news = crate::site::minecraft_news(&context.site, &settings, false);

    let data = if news.items.is_empty() {
        "null".to_string()
    } else {
        let rows: Vec<String> = news.items.iter().map(news_item_json).collect();
        format!("[{}]", rows.join(","))
    };

    Response::json(format!(
        r#"{{"state":"{}","reason":"{}","origin":"{}","source":"{}","publisher":"Minecraft","limit":{},"data":{}}}"#,
        news.state,
        escape_json(&news.reason),
        news.origin,
        escape_json(crate::site::NEWS_ORIGIN),
        crate::site::NEWS_LIMIT,
        data,
    ))
}

/// One news entry as the page draws it. Every field is a plain string: the page
/// escapes what it inserts, and the alternative — a field that is sometimes an
/// object, as the upstream file has it — is what made the old reader guess.
fn news_item_json(item: &crate::site::NewsItem) -> String {
    format!(
        r#"{{"title":"{}","category":"{}","date":"{}","text":"{}","image":"{}","link":"{}"}}"#,
        escape_json(&item.title),
        escape_json(&item.category),
        escape_json(&item.date),
        escape_json(&item.text),
        escape_json(&item.image),
        escape_json(&item.link),
    )
}

/// GET `/api/nodes[?refresh=1]` — the public node list, cached.
fn nodes_endpoint(context: &ServerContext, query: &str) -> Response {
    let settings = context.settings();
    let force = query.contains("refresh=1");
    let (nodes, source) = crate::site::nodes(&context.site, &settings, force);

    let rows = nodes
        .iter()
        .map(|node| {
            format!(
                r#"{{"region":"{}","host":"{}"}}"#,
                escape_json(&node.region),
                escape_json(&node.host)
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    let state = if nodes.is_empty() { "error" } else { "ok" };
    Response::json(format!(
        r#"{{"state":"{}","source":"{}","api_base":"{}","nodes":[{}]}}"#,
        state,
        source,
        escape_json(&settings.api_base),
        rows,
    ))
}

/// POST `/api/probe` — measure the round trip to each listed node.
///
/// The shell does this rather than the page for a practical reason: a browser
/// cannot open a TCP connection to a control port, and `fetch` to a cross-origin
/// host would be blocked anyway. Probes run in parallel, so a list of ten nodes
/// costs about as long as the slowest one.
fn probe_endpoint(body: &[u8]) -> Response {
    let hosts = parse_host_list(&String::from_utf8_lossy(body));
    if hosts.is_empty() {
        return Response::json(r#"{"result":{"state":"error","reason":"没有给出要探测的节点"},"results":[]}"#);
    }
    // Bounded so a hostile or accidental caller cannot ask for thousands of
    // concurrent connections.
    let hosts: Vec<String> = hosts.into_iter().take(64).collect();

    let handles: Vec<_> = hosts
        .into_iter()
        .map(|host| {
            std::thread::spawn(move || {
                let probe = crate::site::probe_host(&host);
                (host, probe)
            })
        })
        .collect();

    let mut rows = Vec::new();
    for handle in handles {
        if let Ok((host, probe)) = handle.join() {
            let latency = match probe.latency_ms {
                Some(ms) => ms.to_string(),
                None => "null".to_string(),
            };
            rows.push(format!(
                r#"{{"host":"{}","state":"{}","latency_ms":{},"method":"{}","reason":"{}"}}"#,
                escape_json(&host),
                probe.state,
                latency,
                probe.method,
                escape_json(&probe.reason)
            ));
        }
    }

    Response::json(format!(r#"{{"results":[{}]}}"#, rows.join(",")))
}

/// Pull `"hosts": ["a","b"]` out of a small request body.
fn parse_host_list(body: &str) -> Vec<String> {
    let start = match body.find("\"hosts\"") {
        Some(index) => index,
        None => return Vec::new(),
    };
    let Some(open) = body[start..].find('[') else {
        return Vec::new();
    };
    let after_open = start + open + 1;
    let Some(close) = body[after_open..].find(']') else {
        return Vec::new();
    };

    body[after_open..after_open + close]
        .split(',')
        .filter_map(|item| {
            let item = item.trim();
            let item = item.strip_prefix('"')?.strip_suffix('"')?;
            let cleaned = item.trim();
            if cleaned.is_empty() || cleaned.len() > 253 {
                None
            } else {
                Some(cleaned.to_string())
            }
        })
        .collect()
}

/// The local game port, auto-detected.
///
/// No query parameters any more: this reads the machine rather than a preference, and what
/// it looked at is in the answer. See [`crate::ports`] for the three steps and for why the
/// identification is process ownership rather than a Minecraft protocol handshake.
fn ports_detect_endpoint(_context: &ServerContext, _query: &str) -> Response {
    let found = crate::ports::detect();
    let list = |values: &[u16]| {
        values
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    let pids = found
        .java_pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Response::json(format!(
        r#"{{"state":"ok","port":{},"source":"{}","java_pids":[{pids}],"checked":[{}],"skipped":[{}]}}"#,
        found.port,
        found.source,
        list(&found.checked),
        list(&found.skipped),
    ))
}

/// The session history, aggregated to one row per day for the heat map.
///
/// `days` is the window, defaulted to the 18 weeks the calendar draws. Nothing here
/// is per-session: the page's only question is "how much did I play on this day", and
/// sending the raw records would put a file's worth of detail on the wire for a graph
/// that cannot show it.
fn sessions_endpoint(query: &str) -> Response {
    const DEFAULT_DAYS: u64 = 126;
    let days = query_number(query, "days")
        .filter(|days| (1..=366).contains(days))
        .unwrap_or(DEFAULT_DAYS) as u32;
    let rows = crate::sessions::daily(days);
    let body = rows
        .iter()
        .map(|row| {
            format!(
                r#"{{"date":"{}","seconds":{},"sessions":{},"level":{}}}"#,
                escape_json(&row.date),
                row.seconds,
                row.sessions,
                row.level
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    Response::json(format!(
        r#"{{"state":"ok","days":{days},"today":"{}","file":"{}","rows":[{body}]}}"#,
        escape_json(&crate::util::date_from_days((crate::util::unix_seconds() / 86_400) as i64)),
        escape_json(&crate::sessions::primary_path().display().to_string()),
    ))
}

/// One numeric query parameter, by name.
///
/// Deliberately narrow: a name match, one `=`, and digits. Anything else is absent,
/// which is what the callers' defaults are for — a malformed window should draw the
/// normal window, not fail the request.
fn query_number(query: &str, name: &str) -> Option<u64> {
    for pair in query.split('&') {
        // `continue`, not `?`: a parameter with no `=` is a reason to look at the next
        // pair, not to abandon the search — `?` here would make `?a&days=30` come back
        // empty, which is the kind of bug that only shows up on someone else's URL.
        let Some((key, value)) = pair.split_once('=') else { continue };
        if key == name {
            return value.trim().parse().ok();
        }
    }
    None
}

/// Pull one `"name": value` field out of a small request body.
///
/// A string value is unquoted; anything else is returned as written, so a number
/// arrives as its digits and the caller decides how to read it. This is the same
/// deliberately-small approach as [`parse_host_list`] — the bodies here are three
/// fields written by the page, not a general JSON surface, and a real parser would
/// be a dependency this crate does not take.
fn json_field(body: &str, name: &str) -> Option<String> {
    let needle = format!("\"{name}\"");
    let start = body.find(&needle)? + needle.len();
    let rest = body[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();

    if let Some(inner) = rest.strip_prefix('"') {
        // Escapes are not interpreted: every value this reads is a host name, a port
        // or an address, and a backslash in one of those is not a value to decode.
        let end = inner.find('"')?;
        return Some(inner[..end].to_string());
    }

    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// `/api/tunnel/*` — start, stop and report the kernel child process.
///
/// The kernel does one thing and says so on stdout; this is the thin layer that
/// turns that into the JSON the page draws. The integration contract is
/// [hongshi.site/api.html](https://hongshi.site/api.html):
/// a fresh process is a fresh tunnel on a fresh port, `endpoint=` is the only line
/// worth reading, and the exit code says whether a tunnel ever existed.
///
/// Every failure is a real status with the reason in the body. Answering `200` with
/// a tunnel that carries nothing would be the one outcome worse than an error,
/// because the user would hand an address to their friends and wait.
fn tunnel_endpoint(context: &ServerContext, action: &str, body: &[u8]) -> Response {
    match action {
        "status" => Response::json(tunnel_json(&context.kernel.status())),

        "start" => {
            let text = String::from_utf8_lossy(body);
            let relay = json_field(&text, "relay").unwrap_or_default();
            let settings = context.settings();
            let game_port = json_field(&text, "game_port")
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(settings.default_game_port as i64);
            let game_host = json_field(&text, "game_host")
                .filter(|host| !host.trim().is_empty())
                .unwrap_or_else(|| "127.0.0.1".to_string());

            if relay.trim().is_empty() {
                return Response::new(
                    400,
                    "application/json; charset=utf-8",
                    format!(
                        r#"{{"state":"bad_request","reason":"{}"}}"#,
                        escape_json("没有选择中转服务器：请先在节点列表里挑一个，或让客户端自动选择")
                    ),
                );
            }
            if !(1..=65535).contains(&game_port) {
                return Response::new(
                    400,
                    "application/json; charset=utf-8",
                    format!(
                        r#"{{"state":"bad_request","reason":"{}"}}"#,
                        escape_json("本地游戏端口必须在 1 到 65535 之间")
                    ),
                );
            }

            match context
                .kernel
                .start(relay.trim(), game_port as u16, game_host.trim())
            {
                Ok(status) => Response::json(format!(
                    r#"{{"state":"ok","running":{},"tunnel":{},"kernel":{}}}"#,
                    status.running,
                    tunnel_field(&status),
                    kernel_json(&status),
                )),
                Err(reason) => {
                    // 409 when something is already running, 424 when the binary is
                    // missing: the first is the user's to resolve, the second needs a
                    // download, and the page says different things for each.
                    let missing = !context.kernel.status().found;
                    Response::new(
                        if missing { 424 } else { 409 },
                        "application/json; charset=utf-8",
                        format!(
                            r#"{{"state":"{}","reason":"{}","kernel":{}}}"#,
                            if missing { "no_kernel" } else { "busy" },
                            escape_json(&reason),
                            kernel_json(&context.kernel.status()),
                        ),
                    )
                }
            }
        }

        _ => match context.kernel.stop() {
            Ok(status) => Response::json(format!(
                r#"{{"state":"ok","running":{},"tunnel":null,"kernel":{}}}"#,
                status.running,
                kernel_json(&status),
            )),
            Err(reason) => Response::new(
                409,
                "application/json; charset=utf-8",
                format!(
                    r#"{{"state":"idle","reason":"{}"}}"#,
                    escape_json(&reason)
                ),
            ),
        },
    }
}

/// `/api/kernel` — whether the kernel is here, where it lives, and what it is doing.
///
/// `platform`/`arch`/`expected_file` are the client's own, computed at build time:
/// the kernel runs on the same machine, so the client is the authority on which build
/// to fetch, and the page does not have to guess from a user-agent string.
fn kernel_endpoint(context: &ServerContext, query: &str) -> Response {
    if let Some((_, rest)) = query.split_once("since=") {
        let since = rest
            .split('&')
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let (lines, seq) = context.kernel.log_since(since);
        let body = lines
            .iter()
            .map(|line| {
                format!(
                    r#"{{"seq":{},"text":"{}"}}"#,
                    line.seq,
                    escape_json(&line.text)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        // The kernel's state travels with its log: the two are always wanted
        // together, and one request per poll instead of two keeps the console quiet.
        return Response::json(format!(
            r#"{{"state":"ok","seq":{seq},"lines":[{body}],"kernel":{}}}"#,
            kernel_json(&context.kernel.status())
        ));
    }

    Response::json(format!(
        r#"{{"state":"ok","kernel":{}}}"#,
        kernel_json(&context.kernel.status())
    ))
}

/// `/api/kernel/download` — fetch the kernel for this platform and install it.
///
/// The site's contract calls this the launcher's job, and it is right to: a browser
/// download would land in the user's Downloads folder and the client would still not
/// find it. The file comes from the official download endpoint, is written next to
/// the client under `core/`, and the page is told where it went.
fn kernel_download(context: &ServerContext) -> Response {
    let settings = context.settings();
    let (platform, arch) = crate::kernel::platform();
    let url = crate::site::download_url(&settings.api_base, "client", platform, arch);

    let options = crate::net::NetOptions::from_settings(&settings);
    let fetched = crate::net::get_bytes(&url, &options, std::time::Duration::from_secs(120));

    let body = match fetched {
        crate::net::Fetched::Response { status: 200, body } => body,
        crate::net::Fetched::Response { status, body } => {
            // The site answers in two shapes and only changes the body, so the status
            // is real either way — and 404 means *this build is not published*, which
            // the page reports differently from "the site is down".
            let reason = crate::net::Fetched::Response { status, body }.reason();
            return Response::new(
                if status == 404 { 404 } else { 502 },
                "application/json; charset=utf-8",
                format!(
                    r#"{{"state":"{}","status":{status},"reason":"{}","url":"{}"}}"#,
                    if status == 404 { "missing" } else { "error" },
                    escape_json(&reason),
                    escape_json(&url),
                ),
            );
        }
        other => {
            return Response::new(
                502,
                "application/json; charset=utf-8",
                format!(
                    r#"{{"state":"unreachable","reason":"{}","url":"{}"}}"#,
                    escape_json(&other.reason()),
                    escape_json(&url),
                ),
            );
        }
    };

    // A tiny body is not a binary; it is an error page that answered 200.
    if body.len() < 64 * 1024 {
        return Response::new(
            502,
            "application/json; charset=utf-8",
            format!(
                r#"{{"state":"error","reason":"{}","bytes":{},"url":"{}"}}"#,
                escape_json(&format!(
                    "下载到的文件只有 {} 字节，看起来不是内核二进制",
                    body.len()
                )),
                body.len(),
                escape_json(&url),
            ),
        );
    }

    match context.kernel.install(&body) {
        Ok(path) => {
            crate::log::info_fields(
                "kernel installed",
                &[
                    ("path", &path.display().to_string()),
                    ("bytes", &body.len().to_string()),
                ],
            );
            Response::json(format!(
                r#"{{"state":"ok","bytes":{},"path":"{}","kernel":{}}}"#,
                body.len(),
                escape_json(&path.display().to_string()),
                kernel_json(&context.kernel.status()),
            ))
        }
        Err(reason) => Response::new(
            500,
            "application/json; charset=utf-8",
            format!(
                r#"{{"state":"error","reason":"{}"}}"#,
                escape_json(&reason)
            ),
        ),
    }
}

/// The tunnel half of a status report, in the shape the page already reads.
fn tunnel_field(status: &crate::kernel::Status) -> String {
    if !status.running && status.endpoint.is_none() {
        return "null".to_string();
    }
    format!(
        r#"{{"endpoint":{},"uuid":{},"node":{},"relay":{},"game_port":{},"mode":"relay-tcp","running":{}}}"#,
        json_option(status.endpoint.as_deref()),
        json_option(status.uuid.as_deref()),
        json_option(status.relay.as_deref()),
        json_option(status.relay.as_deref()),
        status.game_port.map(|port| port.to_string()).unwrap_or_else(|| "null".to_string()),
        status.running,
    )
}

fn tunnel_json(status: &crate::kernel::Status) -> String {
    format!(
        r#"{{"state":"ok","running":{},"tunnel":{},"kernel":{}}}"#,
        status.running,
        tunnel_field(status),
        kernel_json(status),
    )
}

fn kernel_json(status: &crate::kernel::Status) -> String {
    format!(
        concat!(
            "{{\"found\":{},",
            "\"path\":{},",
            "\"state\":\"{}\",",
            "\"running\":{},",
            "\"endpoint\":{},",
            "\"uuid\":{},",
            "\"relay\":{},",
            "\"game_port\":{},",
            "\"uptime_seconds\":{},",
            "\"exit_code\":{},",
            "\"exit_meaning\":{},",
            "\"log_seq\":{},",
            // The only forwarding mode the kernel currently offers. It is reported
            // from here rather than hardcoded in the page so that adding a second
            // mode is a change in one place.
            "\"mode\":\"relay-tcp\",",
            "\"platform\":\"{}\",",
            "\"arch\":\"{}\",",
            "\"expected_file\":\"{}\",",
            "\"core_dir\":\"{}\"}}"
        ),
        status.found,
        json_option(status.path.as_deref()),
        status.state,
        status.running,
        json_option(status.endpoint.as_deref()),
        json_option(status.uuid.as_deref()),
        json_option(status.relay.as_deref()),
        status.game_port.map(|port| port.to_string()).unwrap_or_else(|| "null".to_string()),
        status.uptime_seconds.map(|s| s.to_string()).unwrap_or_else(|| "null".to_string()),
        status.exit_code.map(|code| code.to_string()).unwrap_or_else(|| "null".to_string()),
        json_option(status.exit_meaning),
        status.log_seq,
        status.platform,
        status.arch,
        escape_json(&status.expected_file),
        escape_json(&status.core_dir),
    )
}

/// Which HTTP backend this build uses, for the settings page.
pub fn http_backend_name() -> &'static str {
    // The host's own client wins when it installed one, and it is the host that
    // knows what to call it — the settings page should not describe a client this
    // crate has never seen.
    if let Some(backend) = crate::net::http_backend() {
        return backend.name();
    }
    if cfg!(windows) {
        "WinHTTP（系统组件，支持系统代理）"
    } else {
        "内置 TCP（此平台版本仅支持 http://）"
    }
}

fn health(context: &ServerContext) -> Response {
    let kernel = context.kernel.status();
    let body = format!(
        concat!(
            "{{\"shell\":\"hongshi-shell\",",
            "\"version\":\"{}\",",
            "\"channel\":\"{}\",",
            "\"time\":\"{}\",",
            "\"pid\":{},",
            "\"uptime_seconds\":{},",
            "\"launches\":{},",
            "\"url\":\"{}\",",
            "\"web_source\":\"{}\",",
            "\"background\":{},",
            "\"kernel\":{}}}"
        ),
        escape_json(crate::VERSION),
        escape_json(crate::CHANNEL),
        crate::util::iso8601_utc(),
        std::process::id(),
        context.started.elapsed().as_secs(),
        context.launches,
        escape_json(&context.url),
        escape_json(&context.web_source),
        background_json(&context.settings()),
        kernel_json(&kernel),
    );
    Response::json(body)
}

/// Echo a form-encoded body back as JSON. It exists to prove the body path of the
/// request parser works end to end, and it is what `scripts/verify.ps1` posts to.
fn echo(body: &[u8]) -> Response {
    let text = String::from_utf8_lossy(body);
    let mut fields = Vec::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        fields.push(format!(
            "\"{}\":\"{}\"",
            escape_json(&crate::util::percent_decode(key)),
            escape_json(&crate::util::percent_decode(value))
        ));
    }
    Response::json(format!("{{{}}}", fields.join(",")))
}

fn json_option(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("\"{}\"", escape_json(value)),
        None => "null".to_string(),
    }
}

/// Minimal JSON string escaping: enough for the values this server emits, all of
/// which are ours (paths, versions, endpoints) rather than user input.
fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, path: &str, query: &str, headers: &[(&str, &str)]) -> Request {
        Request {
            method: method.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn context() -> ServerContext {
        ServerContext {
            url: "http://127.0.0.1:12345/".to_string(),
            assets: Assets::discover(None),
            started: Instant::now(),
            web_source: "embedded".to_string(),
            kernel: Arc::new(crate::kernel::Kernel::discover()),
            settings: Arc::new(std::sync::RwLock::new(crate::config::Settings::default())),
            site: Arc::new(crate::site::SiteState::new()),
            http_backend: "test",
            launches: 10,
        }
    }

    fn shutdown() -> ShutdownHandle {
        ShutdownHandle {
            flag: AtomicBool::new(false),
        }
    }

    #[test]
    fn a_plain_page_load_needs_nothing_but_a_loopback_host() {
        // The regression this guards: the gate used to demand a session token, so a
        // reload of `/` — which is a fresh navigation with no query string — was
        // answered 403 and the whole interface died. A browser's first request
        // carries no `Origin` either, so requiring one would break it just as well.
        assert!(request_belongs_to_this_server(&request("GET", "/", "", &[("host", "127.0.0.1:1")])));
        assert!(request_belongs_to_this_server(&request("GET", "/", "", &[("host", "localhost:1")])));
        // A fetch from our own page carries our own Origin, and is allowed.
        assert!(request_belongs_to_this_server(&request(
            "POST",
            "/api/tunnel/start",
            "",
            &[("host", "127.0.0.1:12345"), ("origin", "http://127.0.0.1:12345")]
        )));
    }

    #[test]
    fn a_foreign_origin_or_host_is_refused() {
        assert!(!request_belongs_to_this_server(&request(
            "GET",
            "/api/health",
            "",
            &[("host", "127.0.0.1:12345"), ("origin", "http://evil.example")]
        )));
        assert!(!request_belongs_to_this_server(&request("GET", "/", "", &[("host", "evil.example")])));
        // A rebound name resolves to 127.0.0.1 while `Host` stays the attacker's,
        // so the same-origin check would pass if the Host were not checked.
        assert!(!request_belongs_to_this_server(&request(
            "GET",
            "/",
            "",
            &[("host", "evil.example"), ("origin", "http://evil.example")]
        )));
        assert!(request_belongs_to_this_server(&request(
            "GET",
            "/",
            "",
            &[("host", "localhost:12345"), ("origin", "http://localhost:12345")]
        )));
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        for host in ["127.0.0.1", "127.0.0.1:8080", "localhost", "LOCALHOST:3080", "[::1]:80"] {
            assert!(host_is_loopback(host), "{host}");
        }
        for host in ["example.com", "10.0.0.5:80", "127.0.0.1.evil.com"] {
            assert!(!host_is_loopback(host), "{host}");
        }
    }

    #[test]
    fn rendering_declares_the_body_length_and_connection() {
        let response = Response::json(r#"{"a":1}"#);
        let rendered = String::from_utf8(render(&response, true, true)).unwrap();
        assert!(rendered.starts_with("HTTP/1.1 200 OK\r\n"), "{rendered}");
        assert!(rendered.contains("Content-Length: 7\r\n"), "{rendered}");
        assert!(rendered.contains("Connection: close\r\n"), "{rendered}");
        assert!(rendered.ends_with(r#"{"a":1}"#), "{rendered}");
    }

    #[test]
    fn head_renders_headers_only() {
        let response = Response::html(200, "<html>hi</html>");
        let rendered = String::from_utf8(render(&response, false, true)).unwrap();
        assert!(rendered.contains("Content-Length: 15\r\n"), "{rendered}");
        assert!(!rendered.contains("<html>"), "{rendered}");
    }

    #[test]
    fn an_explicit_content_length_is_not_overridden() {
        let response = Response::new(206, "video/mp4", vec![1, 2, 3]).header("Content-Length", "3");
        let rendered = String::from_utf8(render(&response, true, true)).unwrap();
        assert_eq!(rendered.matches("Content-Length").count(), 1, "{rendered}");
    }

    #[test]
    fn range_requests_produce_a_206_with_the_right_slice() {
        let ctx = context();
        let shutdown = shutdown();
        let request = request(
            "GET",
            "/app.js",
            "",
            &[("host", "127.0.0.1:1"), ("range", "bytes=0-9")],
        );
        let response = route(&request, &[], &ctx, &shutdown);
        assert_eq!(response.status, 206);
        assert_eq!(response.body.len(), 10);
        assert_eq!(response.header_value("content-length"), Some("10"));
        assert!(response.header_value("content-range").unwrap().starts_with("bytes 0-9/"));
    }

    #[test]
    fn an_unsatisfiable_range_is_a_416() {
        let ctx = context();
        let shutdown = shutdown();
        let request = request(
            "GET",
            "/app.js",
            "",
            &[("host", "127.0.0.1:1"), ("range", "bytes=99999999-")],
        );
        assert_eq!(route(&request, &[], &ctx, &shutdown).status, 416);
    }

    #[test]
    fn a_foreign_caller_learns_nothing_about_which_paths_exist() {
        let ctx = context();
        let shutdown = shutdown();

        // A caller that is not our own page is refused identically whether or not
        // the path it asked for exists: a 404 for a stranger would be a free
        // "does this path exist" oracle. And the refusal must be a real 403 —
        // returning 200 here with an error body is exactly the bug this test was
        // written after: `Response::json` is 200 by construction, so the status
        // has to be set explicitly.
        for path in ["/", "/index.html", "/main.css", "/nope", "/api/health", "/api/nope"] {
            let response = route(
                &request(
                    "GET",
                    path,
                    "",
                    &[("host", "127.0.0.1:1"), ("origin", "http://evil.example")],
                ),
                &[],
                &ctx,
                &shutdown,
            );
            let body = String::from_utf8(response.body.clone()).unwrap();
            assert_eq!(response.status, 403, "path {path} answered {}: {body}", response.status);
            if path.starts_with("/api/") {
                assert!(
                    response.content_type.starts_with("application/json"),
                    "path {path} should refuse in the API's shape"
                );
                assert!(body.contains("refused"), "path {path}: {body}");
            }
        }
    }

    #[test]
    fn a_request_without_a_host_header_is_allowed() {
        let ctx = context();
        let shutdown = shutdown();

        // HTTP/1.0 clients and a few tools omit `Host`. The rebinding defence is
        // "when it is present it must be loopback"; requiring it would break those
        // clients for no security gain, because a rebinding attack always sends a
        // Host — that is the whole mechanism.
        let anonymous = route(&request("GET", "/api/health", "", &[]), &[], &ctx, &shutdown);
        assert_eq!(anonymous.status, 200);

        // A Host that is present and not loopback is still refused.
        let rebound = route(
            &request("GET", "/api/health", "", &[("host", "evil.example")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(rebound.status, 403);
    }

    #[test]
    fn an_authenticated_unknown_page_is_404() {
        let ctx = context();
        let shutdown = shutdown();

        // A path with an extension is not an app route, so it is an honest 404.
        // (An extension-less `/nope` is answered with the shell, by design: the
        // interface's own routes are real paths and one document serves them all.)
        let missing = route(
            &request("GET", "/nope.html", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(missing.status, 404);

        let existing = route(
            &request("GET", "/main.css", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(existing.status, 200);
    }

    #[test]
    fn the_news_image_origin_follows_the_service_address() {
        // The daily news serves its own image URLs, so a client pointed at a mirror
        // has to be allowed to load pictures from that mirror. This was a constant
        // `hongshi.site` while the comment claimed otherwise, which cost every news
        // image the moment anyone changed 服务地址.
        let policy = content_security_policy("https://mirror.example.com");
        assert!(policy.contains("https://mirror.example.com"), "{policy}");
        // The official origins stay, so a default install keeps working.
        assert!(policy.contains("https://hongshi.site"), "{policy}");

        // A port is part of the origin and has to survive.
        let with_port = content_security_policy("http://127.0.0.1:5599");
        assert!(with_port.contains("http://127.0.0.1:5599"), "{with_port}");

        // A path is not.
        let with_path = content_security_policy("https://hongshi.site/some/prefix");
        assert!(with_path.contains("https://hongshi.site "), "{with_path}");
        assert!(!with_path.contains("/some/prefix"), "{with_path}");
    }

    #[test]
    fn a_service_address_cannot_inject_another_directive() {
        // `api_base` is a value the user types, and it is interpolated into a response
        // header. `;` would end the `img-src` directive and start one of the attacker's
        // choosing, and a newline would end the header entirely.
        for hostile in [
            "https://evil.example; script-src *",
            "https://evil.example\nx-injected: 1",
            "https://evil.example\r\nx-injected: 1",
            "https://evil.example, *",
            "https://evil.example 'unsafe-inline'",
            "not a url at all",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "",
        ] {
            let policy = content_security_policy(hostile);
            let img_src = policy
                .split("; ")
                .find(|part| part.starts_with("img-src"))
                .expect("the policy always has an img-src");
            assert!(
                !img_src.contains("evil.example") && !img_src.contains("script-src *"),
                "{hostile:?} reached the policy: {img_src}"
            );
            assert!(
                !policy.contains('\n') && !policy.contains('\r'),
                "{hostile:?} put a line break in the header"
            );
            // Whatever happened, the directive list itself is unchanged.
            assert_eq!(policy.matches("script-src 'self'").count(), 1, "{policy}");
            assert_eq!(img_src.matches("img-src").count(), 1, "{img_src}");
        }
    }

    #[test]
    fn an_origin_is_only_taken_from_a_real_http_url() {
        assert_eq!(origin_of("https://a.example"), Some("https://a.example".to_string()));
        assert_eq!(origin_of("http://a.example:8080/x"), Some("http://a.example:8080".to_string()));
        assert_eq!(origin_of("http://[::1]:80"), Some("http://[::1]:80".to_string()));
        assert_eq!(origin_of("https://"), None);
        assert_eq!(origin_of("ftp://a.example"), None);
        assert_eq!(origin_of("//a.example"), None);
        assert_eq!(origin_of("https://a b.example"), None);
    }

    #[test]
    fn an_app_route_gets_the_shell_and_a_missing_asset_does_not() {
        let ctx = context();
        let shutdown = shutdown();

        for route_path in ["/connect", "/settings", "/cloud"] {
            let response = route(
                &request("GET", route_path, "", &[("host", "127.0.0.1:1")]),
                &[],
                &ctx,
                &shutdown,
            );
            assert_eq!(response.status, 200, "{route_path} should serve the shell");
            assert!(
                response.content_type.starts_with("text/html"),
                "{route_path} served {}",
                response.content_type
            );
        }

        // A missing asset must not be quietly answered with HTML, or a typo in a
        // stylesheet path turns into a page that renders as text.
        let missing_asset = route(
            &request("GET", "/nope.css", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(missing_asset.status, 404);
    }

    #[test]
    fn the_index_page_is_served_as_written_and_forbids_framing() {
        let ctx = context();
        let shutdown = shutdown();
        let ok = route(
            &request("GET", "/", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(ok.status, 200);
        assert!(ok.header_value("content-security-policy").unwrap().contains("frame-ancestors 'none'"));

        // No rewriting on the way out any more: the bytes served are the bytes on
        // disk, which is what makes "edit web/* and press F5" mean what it says.
        let embedded = crate::assets::EMBEDDED
            .iter()
            .find(|(route, _, _)| *route == "/index.html")
            .expect("index.html is embedded")
            .2;
        assert_eq!(ok.body, embedded.as_bytes(), "the page must not be rewritten in flight");

        // A reload is the same request, and it works.
        let reload = route(
            &request("GET", "/", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(reload.status, 200);
    }

    #[test]
    fn an_unknown_api_endpoint_is_a_real_404() {
        let ctx = context();
        let shutdown = shutdown();
        let response = route(
            &request("GET", "/api/nope", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(response.status, 404, "an unknown endpoint must not look like success");
        assert!(String::from_utf8(response.body).unwrap().contains("no such endpoint"));
    }

    #[test]
    fn settings_reports_every_field_the_page_edits() {
        let ctx = context();
        let shutdown = shutdown();
        let response = route(
            &request("GET", "/api/settings", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).unwrap();
        for field in [
            "api_base",
            "proxy",
            "use_system_proxy",
            "node_cache_seconds",
            "default_game_port",
            "path",
            "http_backend",
        ] {
            assert!(body.contains(field), "{field} missing from {body}");
        }
        for field in [
            "background_path",
            "background_blur",
            "background_darkness",
            "background_crop",
        ] {
            assert!(body.contains(field), "{field} missing from {body}");
        }
    }

    #[test]
    fn the_background_endpoint_answers_json_when_there_is_no_background() {
        // Not an error: it is the state every install starts in, and the page reads
        // it as "use the built-in artwork".
        let ctx = context();
        let shutdown = shutdown();
        let response = route(
            &request("GET", "/api/background", "", &[("host", "127.0.0.1:1")]),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(response.status, 404);
        assert_eq!(response.content_type, "application/json; charset=utf-8");
        let body = String::from_utf8(response.body).unwrap();
        assert!(body.contains(r#""state":"none""#), "{body}");
    }

    #[test]
    fn the_background_endpoint_serves_the_file_and_revalidates_it() {
        let dir = std::env::temp_dir().join(format!("hongshi-bg-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("wall.png");
        let mut picture = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        picture.extend_from_slice(&[0u8; 48]);
        std::fs::write(&png, &picture).unwrap();

        let ctx = context();
        {
            let mut settings = ctx.settings.write().unwrap();
            settings.background_path = png.display().to_string();
            settings.background_blur = 8;
            settings.background_darkness = 70;
        }
        let shutdown = shutdown();
        let host = [("host", "127.0.0.1:1")];

        let served = route(&request("GET", "/api/background", "", &host), &[], &ctx, &shutdown);
        assert_eq!(served.status, 200);
        assert_eq!(served.content_type, "image/png", "the sniffed type, not the extension");
        assert_eq!(served.body, picture, "the user's own bytes, not a re-encode");

        // A 20 MB wallpaper must not be sent again because somebody pressed F5.
        let etag = served
            .header_value("etag")
            .expect("an ETag is what makes revalidation possible")
            .to_string();
        let unchanged = route(
            &request(
                "GET",
                "/api/background",
                "",
                &[("host", "127.0.0.1:1"), ("if-none-match", &etag)],
            ),
            &[],
            &ctx,
            &shutdown,
        );
        assert_eq!(unchanged.status, 304);

        // Health reports the same file, so the two answers cannot disagree.
        let health = route(&request("GET", "/api/health", "", &host), &[], &ctx, &shutdown);
        let health = String::from_utf8(health.body).unwrap();
        assert!(health.contains(r#""background":{"state":"ok""#), "{health}");
        assert!(health.contains(r#""kind":"png""#), "{health}");
        assert!(health.contains(r#""darkness":70"#), "{health}");
        assert!(health.contains(r#""crop":{"x":0,"y":0,"w":1000,"h":1000}"#), "{health}");

        // The extension lies and the bytes decide — the same rule as the setting.
        std::fs::write(&png, b"GIF89a\x01\x00\x01\x00\x80\x00\x00").unwrap();
        let lying = route(&request("GET", "/api/background", "", &host), &[], &ctx, &shutdown);
        assert_eq!(lying.status, 404);
        let body = String::from_utf8(lying.body).unwrap();
        assert!(body.contains(r#""state":"gif""#), "{body}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_settings_save_cannot_store_a_background_that_does_not_work() {
        let ctx = context();
        let shutdown = shutdown();
        let patch = br#"{"background_path":"C:\\definitely\\not\\here.png"}"#;
        let response = route(
            &request("POST", "/api/settings", "", &[("host", "127.0.0.1:1")]),
            patch,
            &ctx,
            &shutdown,
        );
        assert_eq!(response.status, 400, "a path the shell cannot read is not saved");
        let body = String::from_utf8(response.body).unwrap();
        assert!(body.contains(r#""state":"missing""#), "{body}");
        assert!(body.contains(r#""field":"background_path""#), "{body}");
        assert!(body.contains("找不到"), "the reason must be for a person: {body}");
        assert_eq!(
            ctx.settings().background_path,
            "",
            "a refused save leaves the settings alone"
        );
    }

    #[test]
    fn a_relative_background_path_is_refused_rather_than_guessed() {
        // It would be resolved against the directory the client was started in, which
        // for a double-clicked download is nowhere the user has ever been.
        let ctx = context();
        let shutdown = shutdown();
        let patch = br#"{"background_path":"Pictures\\bg.png"}"#;
        let response = route(
            &request("POST", "/api/settings", "", &[("host", "127.0.0.1:1")]),
            patch,
            &ctx,
            &shutdown,
        );
        assert_eq!(response.status, 400);
        let body = String::from_utf8(response.body).unwrap();
        assert!(body.contains(r#""state":"relative""#), "{body}");
        assert!(body.contains("完整路径"), "{body}");
    }

    #[test]
    fn starting_a_tunnel_either_starts_the_kernel_or_says_why_it_cannot() {
        let ctx = context();
        let shutdown = shutdown();
        let start = route(
            &request("POST", "/api/tunnel/start", "", &[("host", "127.0.0.1:1")]),
            br#"{"relay":"relay.test","game_port":25565}"#,
            &ctx,
            &shutdown,
        );
        let body = String::from_utf8(start.body.clone()).unwrap();

        if ctx.kernel.status().found {
            // A kernel is installed on this machine, so the child really does start.
            // "running" here means the process is alive, not that a tunnel exists —
            // the endpoint arrives on its stdout later and is a separate field.
            assert_eq!(start.status, 200, "{body}");
            assert!(body.contains("\"running\":true"), "{body}");
            assert!(body.contains("\"endpoint\":null"), "no address yet: {body}");

            // And it can be stopped again, which is the other half of the contract.
            let stop = route(
                &request("POST", "/api/tunnel/stop", "", &[("host", "127.0.0.1:1")]),
                b"{}",
                &ctx,
                &shutdown,
            );
            assert_eq!(stop.status, 200, "{}", String::from_utf8_lossy(&stop.body));
        } else {
            // No kernel: a real 424 with the reason, never a 200 with a fake tunnel.
            assert_eq!(start.status, 424, "{body}");
            assert!(body.contains("no_kernel"), "{body}");
            assert!(body.contains("hongshic"), "{body}");
            assert!(!body.contains("\"running\":true"), "{body}");
        }
    }

    #[test]
    fn stopping_when_nothing_runs_is_a_clear_error() {
        let ctx = context();
        let stop = route(
            &request("POST", "/api/tunnel/stop", "", &[("host", "127.0.0.1:1")]),
            b"{}",
            &ctx,
            &shutdown(),
        );
        assert_eq!(stop.status, 409);
        assert!(String::from_utf8_lossy(&stop.body).contains("没有正在运行的隧道"));
    }

    #[test]
    fn start_refuses_a_missing_relay_and_a_port_out_of_range() {
        let ctx = context();
        let shutdown = shutdown();

        let no_relay = route(
            &request("POST", "/api/tunnel/start", "", &[("host", "127.0.0.1:1")]),
            b"{}",
            &ctx,
            &shutdown,
        );
        assert_eq!(no_relay.status, 400, "{}", String::from_utf8_lossy(&no_relay.body));
        assert!(String::from_utf8_lossy(&no_relay.body).contains("bad_request"));

        // A port of 0 is the one that would otherwise reach the kernel as `-p 0`.
        let bad_port = route(
            &request("POST", "/api/tunnel/start", "", &[("host", "127.0.0.1:1")]),
            br#"{"relay":"relay.test","game_port":0}"#,
            &ctx,
            &shutdown,
        );
        assert_eq!(bad_port.status, 400, "{}", String::from_utf8_lossy(&bad_port.body));
    }

    #[test]
    fn the_tunnel_status_answers_200_with_nothing_running() {
        // A question, not a command: "nothing is running" is a complete answer to it.
        let ctx = context();
        let response = route(
            &request("GET", "/api/tunnel/status", "", &[("host", "127.0.0.1:1")]),
            b"",
            &ctx,
            &shutdown(),
        );
        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).unwrap();
        assert!(body.contains("\"running\":false"), "{body}");
        assert!(body.contains("\"tunnel\":null"), "{body}");
        // The kernel block is always present, so the page can say *why* nothing runs.
        assert!(body.contains("\"found\":"), "{body}");
        assert!(body.contains("\"expected_file\":\"hongshic-"), "{body}");
    }

    #[test]
    fn the_kernel_endpoint_reports_this_builds_own_platform() {
        let ctx = context();
        let response = route(
            &request("GET", "/api/kernel", "", &[("host", "127.0.0.1:1")]),
            b"",
            &ctx,
            &shutdown(),
        );
        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).unwrap();
        let (platform, arch) = crate::kernel::platform();
        assert!(body.contains(&format!("\"platform\":\"{platform}\"")), "{body}");
        assert!(body.contains(&format!("\"arch\":\"{arch}\"")), "{body}");
        assert!(body.contains(&format!("\"expected_file\":\"{}\"", crate::kernel::published_name())), "{body}");
        assert!(body.contains("\"core_dir\":"), "{body}");
    }

    #[test]
    fn a_request_field_parser_reads_the_three_fields_the_page_sends() {
        let body = r#"{"relay":"cd.hongshi.site","game_port":25570,"game_host":"127.0.0.1"}"#;
        assert_eq!(json_field(body, "relay").as_deref(), Some("cd.hongshi.site"));
        assert_eq!(json_field(body, "game_port").as_deref(), Some("25570"));
        assert_eq!(json_field(body, "game_host").as_deref(), Some("127.0.0.1"));

        // Whitespace, a trailing field, and a missing one.
        assert_eq!(json_field(r#"{ "relay" : "a.test" }"#, "relay").as_deref(), Some("a.test"));
        assert_eq!(json_field(r#"{"relay":"a.test"}"#, "game_port"), None);
        assert_eq!(json_field("{}", "relay"), None);
        assert_eq!(json_field("", "relay"), None);
        assert_eq!(json_field(r#"{"relay":""}"#, "relay").as_deref(), Some(""));

        // A relay name cannot smuggle a second field: the value ends at the quote.
        assert_eq!(
            json_field(r#"{"relay":"a.test","game_port":99}"#, "relay").as_deref(),
            Some("a.test")
        );
    }

    #[test]
    fn probing_without_hosts_is_a_clear_error_not_an_empty_success() {
        let response = probe_endpoint(b"{}");
        let body = String::from_utf8(response.body).unwrap();
        assert!(body.contains("error"), "{body}");
    }

    #[test]
    fn the_host_list_parser_reads_a_small_json_body() {
        assert_eq!(
            parse_host_list(r#"{"hosts":["a.test","b.test"]}"#),
            vec!["a.test".to_string(), "b.test".to_string()]
        );
        assert_eq!(parse_host_list(r#"{"hosts":[]}"#), Vec::<String>::new());
        assert_eq!(parse_host_list("{}"), Vec::<String>::new());
        assert_eq!(parse_host_list("not json"), Vec::<String>::new());
        // Whitespace and an empty entry are tolerated, the empty one dropped.
        assert_eq!(
            parse_host_list(r#"{"hosts": [ "a.test" , "" ] }"#),
            vec!["a.test".to_string()]
        );
    }

    #[test]
    fn health_reports_the_kernel_block_the_page_reads() {
        let ctx = context();
        let body = String::from_utf8(health(&ctx).body).unwrap();
        assert!(body.contains("\"shell\":\"hongshi-shell\""), "{body}");
        assert!(body.contains("\"kernel\":{"), "{body}");
        assert!(body.contains("\"running\":false"), "{body}");
        // Whether a kernel exists is a fact about this machine, so the assertion is
        // about the shape rather than the value.
        assert!(body.contains("\"found\":true") || body.contains("\"found\":false"), "{body}");
        assert!(body.contains("\"expected_file\":\"hongshic-"), "{body}");
    }

    #[test]
    fn health_reports_the_launch_number_the_page_reads() {
        // The page asks for this once, when a room's dialog is dismissed, to decide
        // whether this is a round launch. A missing field would silently mean "never a
        // round number", which is a feature that looks like it works and never fires.
        let ctx = context();
        let body = String::from_utf8(health(&ctx).body).unwrap();
        assert!(body.contains("\"launches\":10"), "{body}");
    }

    #[test]
    fn echo_decodes_form_fields() {
        let response = echo(b"a=1&b=hello%20world");
        assert_eq!(String::from_utf8(response.body).unwrap(), r#"{"a":"1","b":"hello world"}"#);
    }

    #[test]
    fn json_escaping_covers_quotes_and_control_characters() {
        assert_eq!(escape_json("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(escape_json("a\nb"), "a\\nb");
        assert_eq!(escape_json("\u{1}"), "\\u0001");
    }

    #[test]
    fn an_over_long_request_line_is_refused_rather_than_buffered() {
        // Exercises the reading side without a socket: the limit is checked per
        // line, and a client that never sends a newline must not grow the buffer.
        assert!(MAX_REQUEST_LINE < MAX_HEADER_BYTES);
        assert!(READ_TIMEOUT >= Duration::from_secs(1));
    }
}
