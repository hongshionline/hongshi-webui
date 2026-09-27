//! # hongshi-shell
//!
//! The official WebUI shell for the [`hongshi`](https://github.com/HongshiOnline/hongshi2)
//! kernel — the download that does not start with a terminal.
//!
//! It is deliberately **not** a webview wrapper. It starts a small HTTP server on
//! loopback, hands the address to the browser the user already trusts, and serves
//! the UI from there. That keeps the binary tiny (no Chromium, no WebKit, and — at
//! present — no dependencies at all), keeps page development down to editing HTML
//! and pressing F5, and means the interface is inspectable with the browser's own
//! devtools instead of a bespoke host.
//!
//! Design consequences worth knowing before changing anything:
//!
//! * **Loopback only.** The listener binds `127.0.0.1`, never `0.0.0.0`.
//! * **The port is not a secret, and neither is anything else.** Any process on the
//!   machine, and any web page the user has open, can reach a loopback port. There
//!   is no session token: what refuses a request is where it came from, not what it
//!   carries — see `http_server::request_belongs_to_this_server`.
//! * **Host and Origin are checked.** A hostile page can point its own DNS name at
//!   `127.0.0.1` (DNS rebinding); then `Origin` and `Host` are both the attacker's
//!   name, so the same-origin check passes and it is the loopback-`Host` rule that
//!   refuses.
//! * **The UI is embedded in the binary** with `include_str!`, and overridden by a
//!   `web/` directory next to the executable when one exists. One file to ship,
//!   no rebuild to iterate.
//!
//! Both phases have landed: the shell itself (local server, browser launch, health
//! endpoint) and the kernel supervision in `src/kernel.rs` — spawning `hongshic`,
//! streaming its stdout and lifting `endpoint=…` into the page.

/// The release channel, shown beside the version everywhere a user can see one.
///
/// One constant rather than a string in each of `--version`, the About panel and the
/// site's `/api/webui/version`: those are three places a user can compare, and two of
/// them disagreeing is worse than either being wrong.
pub const CHANNEL: &str = "测试版";

/// The version this build reports.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod assets;
pub mod browser;
pub mod config;
pub mod http_server;
pub mod kernel;
pub mod log;
pub mod net;
pub mod options;
pub mod site;
pub mod util;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::assets::Assets;
use crate::http_server::{Server, ServerContext};
use crate::log::{eprint_raw, info, info_fields, print_raw, warn_fields};

/// Program name used in usage text and log lines.
pub const PROGRAM: &str = "hongshi";

/// What a host asks a shell to do.
///
/// The command line is one caller of this and an embedded host is another, so the
/// shape here is deliberately the intersection of the two: a port, an optional
/// place to serve the UI from, and a log level. Opening a browser is **not** part
/// of it — that is a desktop convenience the command line adds on top, and a phone
/// has a WebView instead.
#[derive(Debug, Clone, Default)]
pub struct ShellConfig {
    /// Loopback port; 0 means "let the OS pick", which is the default.
    pub port: u16,
    /// Serve the UI from here instead of the embedded copy.
    pub web_dir: Option<std::path::PathBuf>,
    /// Print DEBUG lines.
    pub verbose: bool,
}

/// A running shell: the loopback server plus the kernel supervisor behind it.
///
/// This is the embeddable half of the program, and [`run`] is written in terms of
/// it. That is on purpose: there is exactly one place that decides what "starting
/// the shell" means, so a host that embeds it and a host that double-clicks the
/// binary cannot drift apart about the bind, the kernel search or the assets.
pub struct Shell {
    url: String,
    port: u16,
    web_source: String,
    kernel: Arc<crate::kernel::Kernel>,
    serving: crate::http_server::ServeHandle,
}

impl Shell {
    /// Bind loopback, find the kernel, and start accepting.
    ///
    /// Returns once the accept loop is running, so a caller that hands the URL to
    /// a browser — or to a WebView — is not racing the server.
    ///
    /// `Err` is a startup failure and reads as a whole sentence for a human, since
    /// the only way to reach it from the command line is to print it.
    pub fn start(config: ShellConfig) -> Result<Shell, String> {
        // Set before anything logs, so `--verbose` covers the startup lines too.
        log::set_verbose(config.verbose);

        // Establish which copy of the UI is being served before announcing anything,
        // so the console always answers "why is my edit not showing up".
        let assets = Assets::discover(config.web_dir.as_deref());
        let web_source = match assets.runtime_root() {
            Some(root) => root.display().to_string(),
            None => "embedded in the binary".to_string(),
        };

        // Settings are read once at startup and kept in memory; the settings page
        // writes through the same lock, so a change takes effect without a restart.
        let settings = config::load();
        info_fields(
            "settings",
            &[
                ("api_base", &settings.api_base),
                ("proxy", if settings.proxy.is_empty() { "自动" } else { &settings.proxy }),
            ],
        );
        let started = Instant::now();

        // Bind before building anything else: the URL cannot be known until the port
        // is, and the accept loop must not start before the context it serves is
        // complete.
        let listener = Server::listen(config.port).map_err(|err| {
            format!(
                "could not bind 127.0.0.1:{}: {err}\n       \
                 (is another hongshi shell already running? pass --port 0 to pick a free port)",
                config.port
            )
        })?;
        let port = listener.local_addr().map(|addr| addr.port()).unwrap_or(0);
        let url = format!("http://127.0.0.1:{port}/");

        // Find the kernel before anything is announced, so the caller can say
        // whether this client can actually start a tunnel. It is not fatal when it
        // cannot: everything else works, and the 联机 page offers the download.
        let kernel = Arc::new(crate::kernel::Kernel::discover());

        let context = ServerContext {
            url: url.clone(),
            assets,
            started,
            web_source: web_source.clone(),
            kernel: Arc::clone(&kernel),
            settings: Arc::new(std::sync::RwLock::new(settings)),
            site: Arc::new(crate::site::SiteState::new()),
            http_backend: crate::http_server::http_backend_name(),
        };
        let server = Server::new(listener, context);

        // Start accepting before returning. The URL is a promise that the address
        // works, and a caller that reads it and connects immediately must not be
        // able to land in a window where nothing is accepting yet.
        let serving = server.serve();

        Ok(Shell { url, port, web_source, kernel, serving })
    }

    /// The address the UI is served from. Needs no token; reloading it works.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The port actually bound, which is not the one asked for when 0 was asked for.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Which copy of the UI is being served: a directory, or the embedded one.
    pub fn web_source(&self) -> &str {
        &self.web_source
    }

    /// The kernel supervisor, for a host that wants to start or stop a tunnel
    /// without going through the page.
    pub fn kernel(&self) -> &Arc<crate::kernel::Kernel> {
        &self.kernel
    }

    /// Ask the accept loop to stop. Idempotent, and it does not wait.
    pub fn shutdown(&self) {
        self.serving.shutdown_handle().request();
    }

    /// Wait for the accept loop to finish, after [`Shell::shutdown`] or a request
    /// to `/api/shutdown`.
    pub fn wait(self) -> std::io::Result<()> {
        self.serving.join()
    }
}

/// Entry point shared by `main.rs` and the integration tests.
///
/// Returns the process exit code: `0` after an orderly shutdown, `2` on a usage
/// or configuration error, `1` on a runtime failure.
pub fn run(args: Vec<String>) -> ExitCode {
    // Logging is asynchronous so that a console nobody is reading cannot block a
    // request thread; this guard drains what is left on every return path, including
    // the early ones. Its own `Drop` is what makes `--version` and a usage error
    // print at all.
    let _flush_log = crate::log::FlushOnExit;

    let options = match options::parse(&args) {
        Ok(options::Parsed::Run(options)) => options,
        Ok(options::Parsed::Print(text)) => {
            print_raw(&text);
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            // Through `log`, not `eprintln!`: see `log::write_line` for why a
            // closed pipe must never be allowed to panic the process.
            eprint_raw(&format!("error: {err}\n\n{}", options::usage()));
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let shell = match Shell::start(ShellConfig {
        port: options.port,
        web_dir: options.web_dir.clone(),
        verbose: options.verbose,
    }) {
        Ok(shell) => shell,
        Err(err) => {
            eprint_raw(&format!("error: {err}"));
            return ExitCode::from(1);
        }
    };

    print_banner(shell.url(), shell.web_source(), options.port, &shell.kernel().status());

    if options.open_browser {
        // Not fatal: the URL is on the console, and printing is the fallback.
        if !browser::open(shell.url()) {
            warn_fields("open this URL yourself", &[("url", shell.url())]);
        }
    } else {
        info("--no-browser given, open the URL above yourself");
    }

    // Nothing more to coordinate: `/api/shutdown` sets the flag the accept loop
    // polls, and this call returns once the loop has stopped.
    let result = shell.wait();

    // Reaching here means the server stopped: either `/api/shutdown` was hit, or
    // the listener died. Either way the console should say so and the process
    // should exit — a shell with no server is a zombie window.
    //
    // The pause is not politeness. The accept loop stops the moment the flag is
    // set, while the connection that set it is still finishing: its response is
    // written by a connection thread, and if this thread reaches the end of `main`
    // first, Windows closes the process's sockets with an RST and the browser (or
    // the page's `fetch`) sees a reset instead of `{"state":"shutting down"}`. One
    // grace period here is cheaper than losing the body of the last response.
    std::thread::sleep(SHUTDOWN_GRACE);

    // Say so if the console lost lines. Logging is asynchronous now precisely so it
    // cannot block a request, and the price is that a reader slower than the server
    // loses lines — which a person reading a support transcript should know rather
    // than silently see a gap in.
    let dropped = crate::log::dropped_lines();
    if dropped > 0 {
        warn_fields(
            "some console lines were dropped; nothing was reading stdout fast enough",
            &[("dropped", &dropped.to_string())],
        );
    }

    match result {
        Ok(()) => {
            info_fields(
                "shell stopped",
                &[("uptime", &format!("{:.1}s", started.elapsed().as_secs_f32()))],
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprint_raw(&format!("error: the shell stopped: {err}"));
            ExitCode::from(1)
        }
    }
}

/// The banner is printed through `log`, not `println!`: `hongshi | head -1` closes
/// stdout while the banner is still going out, and a panic there would kill the
/// shell before the browser ever opens.
///
/// The kernel line is here rather than only in the page because "can this client
/// start a tunnel at all" is the first thing to check when a user reports that the
/// button does nothing, and the console is what they have open.
fn print_banner(url: &str, web_source: &str, requested_port: u16, kernel: &crate::kernel::Status) {
    print_raw("");
    info("hongshi shell is running");
    print_raw(&format!("  URL      {url}"));
    print_raw("  bound    127.0.0.1 (loopback only — nothing on your network can reach this)");
    if requested_port == 0 {
        print_raw("  port     a free port was picked automatically");
    }
    print_raw(&format!("  UI       {web_source}"));
    match (&kernel.path, kernel.found) {
        (Some(path), true) => print_raw(&format!("  内核     {path}")),
        _ => print_raw(&format!(
            "  内核     没有找到 {}（放到 {}/ 即可，或在页面里点「下载内核」）",
            crate::kernel::binary_name(),
            crate::kernel::CORE_DIR
        )),
    }
    print_raw("");
    print_raw("  Keep this window open while you play.");
    print_raw("  Press Ctrl+C here, or use Quit in the page, to stop the shell.");
    print_raw("");
}

/// How long the shell waits between the accept loop stopping and the process
/// exiting, so the response that asked for the shutdown is actually delivered.
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);
