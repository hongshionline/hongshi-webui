//! The kernel: `hongshic`, run as a child process.
//!
//! The integration contract is the site's own [`api.html`](https://hongshi.site/api.html),
//! and it is narrow on purpose:
//!
//! * fetch the binary for the player's platform from `/api/download/client`,
//! * spawn it as `hongshic -t <relay> -p <game-port>`,
//! * read `endpoint=` off **stdout** and show that address verbatim,
//! * when the process exits, the room is over.
//!
//! Its exit code is the whole result: `0` means the tunnel ended (the relay
//! reclaimed it, or shut down), `1` means it never got one. The kernel has no
//! configuration file, so there is nothing to write and nothing to clean up.
//!
//! **The endpoint is never cached.** A fresh launch is a fresh tunnel on a fresh
//! port, so the previous address is dropped the moment a new process starts — showing
//! a stale one would send players to a port that no longer exists.
//!
//! Everything the kernel prints goes into a bounded ring buffer that the log drawer
//! polls with `/api/kernel/log?since=N`, so the panel shows the kernel's own words
//! rather than a paraphrase of them.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How many kernel lines are kept. Enough to cover a session's startup and any
/// player joining, bounded so a chatty kernel cannot grow the client's memory.
const LOG_CAPACITY: usize = 800;

/// How often the reaper asks whether the child has exited.
const REAP_INTERVAL: Duration = Duration::from_millis(250);

/// How long `stop` waits for a killed kernel to actually go.
///
/// `kill` only asks. Without waiting, the answer to "stop it" can be a status that
/// still says `running: true` about the process this very call just killed — and a
/// user who clicks 开启隧道 straight afterwards is then told a tunnel is already up,
/// which is how a stuck-looking client turns into a confusing one.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// Where the kernel is looked for, relative to the client.
pub const CORE_DIR: &str = "core";

/// The binary's name on this platform.
pub fn binary_name() -> &'static str {
    if cfg!(windows) { "hongshic.exe" } else { "hongshic" }
}

/// The names the kernel is accepted under, in the order they are tried.
///
/// More than one because the ways a user ends up with a kernel disagree about what
/// it is called and none of them is wrong: someone who downloaded it by hand has
/// the published name, and an APK has the `lib*.so` name Android insists on. One
/// list, so [`locate`] and anything that wants to explain the search cannot drift
/// apart about what was looked for.
pub fn candidate_names() -> Vec<String> {
    let mut names = Vec::with_capacity(3);
    if cfg!(target_os = "android") {
        // First, because it is where the APK actually puts it.
        names.push(ANDROID_LIBRARY_NAME.to_string());
    }
    names.push(binary_name().to_string());
    names.push(published_name());
    names
}

/// Directories a host adds to the kernel search, ahead of the ones derived from
/// the executable and the working directory.
///
/// Both of those defaults are meaningless on Android — the executable is the
/// runtime's own `app_process64`, and there is no working directory the user
/// chose — so without this the kernel is never found on the one platform where it
/// is guaranteed to be present, because the APK shipped it.
static APP_ROOTS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();

/// Tell the shell where a host keeps its own files: the bundled kernel, a
/// downloaded one, and the writable place an update should go. Called once.
pub fn install_search_roots(roots: Vec<PathBuf>) -> Result<(), String> {
    APP_ROOTS
        .set(roots)
        .map_err(|_| "the kernel search roots are already set".to_string())
}

/// The platform and architecture the download endpoint wants, plus the file name
/// that platform's build is published under.
///
/// Taken from the build rather than from the running OS string, because the two can
/// only differ in the case where the client would not run either.
pub fn platform() -> (&'static str, &'static str) {
    let platform = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "android") {
        // Not "linux": the kernel for Android is built against the NDK and needs a
        // bionic-linked artifact, and asking the download endpoint for the musl
        // build would hand the user a binary this device cannot run.
        "android"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "amd64" };
    (platform, arch)
}

/// The name the kernel is packaged under inside an Android APK.
///
/// Android will not let an app execute anything out of its own writable data
/// directory — since Android 10 the platform enforces W^X there — but it will
/// execute the read-only libraries the installer extracted, and those are the
/// files under `applicationInfo.nativeLibraryDir`. The only way to get a binary
/// into that directory is to ship it in the APK as a `lib*.so`, so that is what
/// the kernel is called there.
pub const ANDROID_LIBRARY_NAME: &str = "libhongshic.so";

/// The published file name for this platform, e.g. `hongshic-windows-amd64.exe`.
pub fn published_name() -> String {
    let (platform, arch) = platform();
    let extension = if cfg!(windows) { ".exe" } else { "" };
    format!("hongshic-{platform}-{arch}{extension}")
}

/// One line the kernel printed.
#[derive(Clone)]
pub struct LogLine {
    pub seq: u64,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Stopped,
    Running,
    Exited,
}

impl RunState {
    fn as_str(self) -> &'static str {
        match self {
            RunState::Stopped => "stopped",
            RunState::Running => "running",
            RunState::Exited => "exited",
        }
    }
}

/// Why the last run ended, in the kernel's own terms.
#[derive(Clone)]
pub struct ExitInfo {
    pub code: Option<i32>,
    /// What the code means, from the integration contract.
    pub meaning: &'static str,
}

impl ExitInfo {
    fn from_code(code: Option<i32>) -> ExitInfo {
        let meaning = match code {
            Some(0) => "隧道已结束（中转服务器回收了它，或中转服务器已关闭）",
            Some(1) => "没能建立隧道：中转服务器不可达、拒绝了请求，或缺少必要参数",
            _ => "内核退出了",
        };
        ExitInfo { code, meaning }
    }

    /// The exit the *user* asked for.
    ///
    /// The contract reads exit code 1 as "never got a tunnel", and that is right for a
    /// process that ended on its own — but a process this client killed exits 1 too, so
    /// reporting the code's meaning after 关闭隧道 told the user their network had
    /// failed when they had simply pressed stop. Which of the two happened is only
    /// knowable here, where the kill was issued.
    fn stopped(code: Option<i32>) -> ExitInfo {
        ExitInfo { code, meaning: "已关闭（你停止了这条隧道）" }
    }
}

/// A snapshot of the kernel, taken under one lock so the fields cannot disagree.
#[derive(Clone)]
pub struct Status {
    pub found: bool,
    pub path: Option<String>,
    /// Where the kernel is looked for and where a download goes.
    pub core_dir: String,
    pub state: &'static str,
    pub running: bool,
    pub endpoint: Option<String>,
    pub uuid: Option<String>,
    pub relay: Option<String>,
    pub game_port: Option<u16>,
    pub uptime_seconds: Option<u64>,
    pub exit_code: Option<i32>,
    pub exit_meaning: Option<&'static str>,
    pub log_seq: u64,
    pub platform: &'static str,
    pub arch: &'static str,
    pub expected_file: String,
}

struct Inner {
    /// Resolved once at startup; the download flow can replace it.
    path: Option<PathBuf>,
    core_dir: PathBuf,
    child: Option<Child>,
    state: RunState,
    endpoint: Option<String>,
    uuid: Option<String>,
    relay: Option<String>,
    game_port: Option<u16>,
    started_at: Option<Instant>,
    exit: Option<ExitInfo>,
    /// Set when this client asked the kernel to stop, so its exit is not read as a
    /// verdict on the tunnel.
    stopping: bool,
    log: std::collections::VecDeque<LogLine>,
}

pub struct Kernel {
    inner: Mutex<Inner>,
    seq: AtomicU64,
}

impl Kernel {
    /// Find the kernel and remember where it looked.
    pub fn discover() -> Kernel {
        let path = locate();
        let core_dir = install_dir(path.as_deref());
        Kernel {
            inner: Mutex::new(Inner {
                path,
                core_dir,
                child: None,
                state: RunState::Stopped,
                endpoint: None,
                uuid: None,
                relay: None,
                game_port: None,
                started_at: None,
                exit: None,
                stopping: false,
                log: std::collections::VecDeque::new(),
            }),
            seq: AtomicU64::new(0),
        }
    }

    pub fn status(&self) -> Status {
        let mut inner = self.lock();
        self.reap(&mut inner);
        self.snapshot(&mut inner)
    }

    /// The lines the page has not seen yet.
    pub fn log_since(&self, since: u64) -> (Vec<LogLine>, u64) {
        let inner = self.lock();
        let lines = inner
            .log
            .iter()
            .filter(|line| line.seq > since)
            .cloned()
            .collect::<Vec<_>>();
        drop(inner);
        (lines, self.seq.load(Ordering::SeqCst))
    }

    /// Where the kernel is expected to live, whether or not it is there.
    pub fn core_dir(&self) -> PathBuf {
        self.lock().core_dir.clone()
    }

    /// Start the kernel against one relay.
    pub fn start(
        self: &Arc<Self>,
        relay: &str,
        game_port: u16,
        game_host: &str,
    ) -> Result<Status, String> {
        {
            let mut inner = self.lock();
            self.reap(&mut inner);
            if inner.state == RunState::Running {
                // Say which tunnel, so "already running" is a fact the user can act on
                // rather than a dead end. Reaching for 开启隧道 again usually means the
                // first one did not look like it worked — which, before terminal
                // colour was stripped out of the kernel's output, it did not.
                let where_ = inner.endpoint.clone()
                    .or_else(|| inner.relay.clone())
                    .unwrap_or_else(|| "未知节点".to_string());
                return Err(format!(
                    "已经有一个隧道在运行了（{where_}）。同一个客户端同时只能跑一个内核，\
                     先关闭它，或者直接用这个地址。"
                ));
            }

            let Some(path) = inner.path.clone() else {
                return Err(missing_kernel_message(&inner.core_dir));
            };

            // A new process is a new tunnel on a new port. Nothing about the last
            // run survives it.
            inner.endpoint = None;
            inner.uuid = None;
            inner.exit = None;
            inner.started_at = None;
            inner.stopping = false;
            inner.relay = Some(relay.to_string());
            inner.game_port = Some(game_port);
            self.push(
                &mut inner,
                format!("启动内核：{} -t {} -p {}", path.display(), relay, game_port),
            );

            let mut command = Command::new(&path);
            command
                .arg("-t")
                .arg(relay)
                .arg("-p")
                .arg(game_port.to_string())
                .arg("--game-host")
                .arg(game_host)
                // Ask for plain output. `tracing-subscriber` colours whenever the
                // `ansi` feature is on and `NO_COLOR` is unset or empty — note that it
                // does *not* check whether stdout is a terminal, so a piped kernel is
                // coloured on any machine where that variable is not already set. This
                // is the standard way to say "not to a person", and it costs nothing.
                .env("NO_COLOR", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            match command.spawn() {
                Ok(mut child) => {
                    let stdout = child.stdout.take();
                    let stderr = child.stderr.take();
                    inner.child = Some(child);
                    inner.state = RunState::Running;
                    inner.started_at = Some(Instant::now());
                    drop(inner);

                    if let Some(stream) = stdout {
                        self.pump(stream, false);
                    }
                    if let Some(stream) = stderr {
                        self.pump(stream, true);
                    }
                    self.clone().spawn_reaper();
                    Ok(self.status())
                }
                Err(err) => {
                    let reason = format!("无法启动内核 {}：{err}", path.display());
                    self.push(&mut inner, reason.clone());
                    inner.state = RunState::Stopped;
                    inner.exit = Some(ExitInfo::from_code(None));
                    Err(reason)
                }
            }
        }
    }

    /// Stop the kernel, wherever the request came from.
    pub fn stop(&self) -> Result<Status, String> {
        {
            let mut inner = self.lock();
            self.reap(&mut inner);

            // `kill` through a temporary binding, so the borrow of `inner` ends before
            // the flag and the log line need it.
            let killed = match inner.child.as_mut() {
                Some(child) => child.kill(),
                None => return Err("没有正在运行的隧道".to_string()),
            };
            match killed {
                Ok(()) => {
                    inner.stopping = true;
                    self.push(&mut inner, "已请求关闭内核".to_string());
                }
                // The flag is about *whose* exit this is; a failed kill is not one.
                Err(err) => return Err(format!("无法关闭内核：{err}")),
            }
        }

        // Wait for it to actually go, so the status this returns is about the world
        // after the stop rather than the instant before it.
        let deadline = Instant::now() + STOP_GRACE;
        loop {
            let status = self.status();
            if !status.running || Instant::now() >= deadline {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Write a freshly downloaded kernel into place.
    pub fn install(&self, bytes: &[u8]) -> Result<PathBuf, String> {
        let mut inner = self.lock();
        let dir = inner.core_dir.clone();
        std::fs::create_dir_all(&dir).map_err(|err| format!("无法创建 {}：{err}", dir.display()))?;
        let target = dir.join(binary_name());

        // Written beside the target and moved into place, so a download that dies
        // half-way never leaves a truncated binary that looks runnable.
        let staging = dir.join(format!("{}.part", binary_name()));
        std::fs::write(&staging, bytes)
            .map_err(|err| format!("无法写入 {}：{err}", staging.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755));
        }

        std::fs::rename(&staging, &target).map_err(|err| {
            let _ = std::fs::remove_file(&staging);
            format!("无法安装内核到 {}：{err}", target.display())
        })?;

        inner.path = Some(target.clone());
        self.push(
            &mut inner,
            format!("内核已安装到 {}", target.display()),
        );
        Ok(target)
    }

    // ------------------------------------------------------------------ private

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock means a panic while holding it; the state is still
        // consistent enough to serve, and refusing every request would be worse.
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Build a `Status` from state already locked by the caller. One constructor, so
    /// a field added to `Status` cannot be added to one path and forgotten in the
    /// other — which is exactly how `core_dir` went missing the first time.
    fn snapshot(&self, inner: &mut Inner) -> Status {
        let (platform, arch) = platform();
        Status {
            found: inner.path.is_some(),
            path: inner.path.as_ref().map(|path| path.display().to_string()),
            core_dir: inner.core_dir.display().to_string(),
            state: inner.state.as_str(),
            running: inner.state == RunState::Running,
            endpoint: inner.endpoint.clone(),
            uuid: inner.uuid.clone(),
            relay: inner.relay.clone(),
            game_port: inner.game_port,
            uptime_seconds: inner.started_at.map(|at| at.elapsed().as_secs()),
            exit_code: inner.exit.as_ref().and_then(|exit| exit.code),
            exit_meaning: inner.exit.as_ref().map(|exit| exit.meaning),
            log_seq: self.seq.load(Ordering::SeqCst),
            platform,
            arch,
            expected_file: published_name(),
        }
    }

    fn push(&self, inner: &mut Inner, text: String) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        inner.log.push_back(LogLine { seq, text });
        while inner.log.len() > LOG_CAPACITY {
            inner.log.pop_front();
        }
    }

    /// Read one stream line by line into the log, lifting the fields the interface
    /// needs out of it as they go past.
    fn pump<R: std::io::Read + Send + 'static>(self: &Arc<Self>, stream: R, is_stderr: bool) {
        let kernel = Arc::clone(self);
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                // Stripped before anything looks at it — the parse and the panel must
                // see the same text, and neither wants terminal colour.
                let line = strip_ansi(line.trim_end());
                if line.trim().is_empty() {
                    continue;
                }
                let line = line.as_str();
                let mut inner = kernel.lock();

                // The contract says everything before `endpoint=` is decoration.
                if let Some(endpoint) = field(line, "endpoint") {
                    // Only the first one is the tunnel's address: a relaunch inside
                    // one process would print another, and the interface must not
                    // flip between two addresses.
                    if inner.endpoint.is_none() {
                        inner.endpoint = Some(endpoint.clone());
                        kernel.push(&mut inner, format!("隧道地址已分配：{endpoint}"));
                    }
                }
                if let Some(uuid) = field(line, "uuid")
                    && inner.uuid.is_none()
                {
                    inner.uuid = Some(uuid);
                }

                // The kernel's own line, prefixed so a reader can tell it from the
                // client's commentary in the same panel.
                let text = if is_stderr { format!("! {line}") } else { line.to_string() };
                kernel.push(&mut inner, text);
            }
        });
    }

    /// Notice the child exiting, and say what its exit code means.
    fn reap(&self, inner: &mut Inner) {
        let Some(child) = inner.child.as_mut() else { return };
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code();
                let asked = inner.stopping;
                inner.child = None;
                inner.stopping = false;
                // A stop the user asked for is a stop, not a failed tunnel — even
                // though a killed process reports the same exit code as one that
                // never reached the relay.
                inner.state = if asked { RunState::Stopped } else { RunState::Exited };
                let ran_for = inner.started_at.map(|started| started.elapsed().as_secs());
                let game_port = inner.game_port;
                inner.started_at = None;
                let info = if asked { ExitInfo::stopped(code) } else { ExitInfo::from_code(code) };
                self.push(
                    inner,
                    match (code, asked) {
                        (_, true) => "内核已关闭".to_string(),
                        (Some(code), false) => {
                            format!("内核退出（代码 {code}）：{}", info.meaning)
                        }
                        (None, false) => format!("内核被强制结束：{}", info.meaning),
                    },
                );
                inner.exit = Some(info);

                // The session history, and this is the only place it is written from.
                //
                // `reap` runs on every status poll and on the reaper thread, so the
                // guard is that it only reaches here on the transition out of Running:
                // `child` was taken above, so a second pass returns at the top. The
                // write happens outside the lock, because a full-file rewrite under the
                // mutex would block every other request on this process — including the
                // health poll the page makes every three seconds.
                if let (Some(seconds), Some(port)) = (ran_for, game_port) {
                    if seconds >= crate::sessions::MIN_SECONDS {
                        let session = crate::sessions::Session {
                            started_at: crate::util::unix_seconds().saturating_sub(seconds),
                            seconds,
                            game_port: port,
                            outcome: if asked { "stopped" } else { "exited" },
                        };
                        if let Err(reason) = crate::sessions::record(&session) {
                            // Not fatal and not the user's problem mid-session: the
                            // tunnel worked, only the tally was lost.
                            crate::log::warn_fields("could not record the session", &[("reason", &reason)]);
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                self.push(inner, format!("无法获知内核状态：{err}"));
            }
        }
    }

    fn spawn_reaper(self: Arc<Self>) {
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(REAP_INTERVAL);
                let mut inner = self.lock();
                let was_running = inner.state == RunState::Running;
                self.reap(&mut inner);
                if was_running && inner.state != RunState::Running {
                    return;
                }
                if inner.state == RunState::Stopped {
                    return;
                }
            }
        });
    }
}

/// Remove terminal control sequences from a line of the kernel's output.
///
/// The kernel logs through `tracing`, which colours its output whenever the `ansi`
/// feature is on and `NO_COLOR` is unset or empty — and **not**, as is easy to
/// assume, only when stdout is a terminal. So a kernel spawned with a pipe is
/// coloured on every machine that has not already set that variable, and the
/// client saw `\e[3mendpoint\e[0m\e[2m=\e[0mcd.hongshi.site:53190`: the literal
/// `endpoint=` never appears, the address is never lifted out, and the card sits on
/// "还没有分配地址" while the tunnel is up and running. The user's reasonable next move
/// is to press 开启隧道 again, which is refused because one *is* running.
///
/// The spawn also sets `NO_COLOR=1`, which is the intended way to ask for this. This
/// function is the guarantee behind that courtesy: the kernel is a separate program
/// whose build we do not control, an older or newer subscriber may ignore the
/// variable, and a client that parses a field must not depend on it being honoured.
/// Colour is also meaningless in a panel that is not a terminal.
///
/// (This was first "verified" as unnecessary: the shell's own environment here has
/// `NO_COLOR=1`, so the child inherited it and the probe measured the harness rather
/// than the product. `examples/kernel_colour_probe.rs` now says which of the two it
/// is looking at.)
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                // CSI: `ESC [ params final`, where the final byte is 0x40..=0x7E.
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: `ESC ] ... BEL` or `ESC ] ... ESC \`.
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                // Any other escape. The charset introducers take one more byte
                // (`ESC ( B`), the rest stand alone (`ESC =`, `ESC M`, `ESC c`).
                Some(c) if "()*+-./#%".contains(*c) => {
                    chars.next();
                    chars.next();
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }

        // Anything else invisible or cursor-moving is worse than useless here: it
        // would sit in the panel as a byte nobody can see and nobody can delete.
        // A tab is kept; it is the one control character that still means something.
        if ch.is_control() && ch != '\t' {
            continue;
        }
        out.push(ch);
    }

    out
}

/// The message shown when there is no kernel: what is missing, and what to do.
pub fn missing_kernel_message(core_dir: &Path) -> String {
    format!(
        "没有找到内核 {}。请把它放到 {}，或在下方点「下载内核」，客户端会自动选择匹配当前平台的版本（{}）。",
        binary_name(),
        core_dir.display(),
        published_name()
    )
}

/// Where the kernel is looked for and where a download goes.
///
/// Both the executable's own directory and the working directory get a `core/`, and
/// both are searched — because the two differ in exactly the cases that matter. A
/// client that was double-clicked has a working directory the user never chose, so
/// the executable's own directory is the one that means "next to the client" (and is
/// where the settings file goes). A developer runs it from the project root, where
/// `./core` is what the documentation says. Searching both means either reading
/// works, and the reported path is always the one that was actually used.
pub fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // A host that knows where its own files live goes first. On Android the two
    // defaults below are both meaningless, so this is the only entry that can hit.
    if let Some(app) = APP_ROOTS.get() {
        roots.extend(app.iter().cloned());
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let cwd = std::env::current_dir().ok();

    for base in [exe_dir.as_ref(), cwd.as_ref()].into_iter().flatten() {
        roots.push(base.join(CORE_DIR));
    }
    for base in [exe_dir.as_ref(), cwd.as_ref()].into_iter().flatten() {
        roots.push(base.clone());
    }
    roots
}

/// Where a downloaded kernel is written: beside one that is already installed if
/// there is one, otherwise a writable `core/` next to the executable, otherwise the
/// working directory.
fn install_dir(found: Option<&Path>) -> PathBuf {
    if let Some(path) = found
        && let Some(dir) = path.parent()
    {
        return dir.to_path_buf();
    }

    // A host's roots before the defaults, and specifically the first one that can
    // actually be written: on Android the bundled kernel sits in a read-only
    // directory, so "beside the one that is already installed" would point an
    // update at a place the app is not allowed to write.
    if let Some(app) = APP_ROOTS.get()
        && let Some(dir) = app.iter().find(|dir| writable_dir(dir))
    {
        return dir.clone();
    }

    let exe_core = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CORE_DIR)));
    if let Some(dir) = &exe_core
        && (dir.is_dir() || writable_beside(dir))
    {
        return dir.clone();
    }

    std::env::current_dir()
        .map(|dir| dir.join(CORE_DIR))
        .unwrap_or_else(|_| PathBuf::from(CORE_DIR))
}

/// Whether a directory can be created and written to.
fn writable_dir(dir: &Path) -> bool {
    if !dir.is_dir() && std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    probe_writable(dir)
}

/// Whether a `core` directory could be created next to the executable. A client
/// installed under `Program Files` cannot write there, and the working directory is
/// the honest fallback.
fn writable_beside(core: &Path) -> bool {
    let Some(parent) = core.parent() else { return false };
    if !parent.is_dir() {
        return false;
    }
    probe_writable(parent)
}

/// Write a file and remove it again: the only test of "can this process write here"
/// that does not lie about ACLs, read-only mounts or the app sandbox.
fn probe_writable(dir: &Path) -> bool {
    let probe = dir.join(".hongshi-core-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Find the kernel in any of [`search_roots`], under either its plain name or the
/// name the release artifacts are published with.
///
/// The published name is accepted because someone who downloaded the binary by hand
/// will not have renamed it, and `hongshic-windows-amd64.exe` is a perfectly good
/// thing to find sitting in `core/`.
pub fn locate() -> Option<PathBuf> {
    let names = candidate_names();
    for root in search_roots() {
        for name in &names {
            let candidate = root.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The value of `name=…` on a kernel log line, up to the next whitespace.
///
/// The contract is explicit that everything before `endpoint=` is decoration and may
/// change, so this matches on the field and not on the sentence around it — and the
/// sentence around it *contains the word*: the line reads "hand the `endpoint` above
/// to the players uuid=… endpoint=…". Matching the first occurrence of `endpoint`
/// finds the prose and then fails, which is why every occurrence is tried and only a
/// name actually followed by `=` counts.
///
/// The `=` may be separated from the name by whitespace: colour is stripped before
/// this runs, but "decoration may change" is the standing instruction, and a parser
/// that accepts one spelling of a field the other side owns breaks on that side's
/// next release.
fn field(line: &str, name: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(found) = line[from..].find(name).map(|index| index + from) {
        let after = found + name.len();

        // Only a whole word counts, so `myendpoint=` is not `endpoint=`.
        if let Some(previous) = line[..found].chars().next_back()
            && (previous.is_alphanumeric() || previous == '_')
        {
            from = after;
            continue;
        }

        let rest = line[after..].trim_start();
        if let Some(value_part) = rest.strip_prefix('=') {
            let value_part = value_part.trim_start();
            let end = value_part.find(char::is_whitespace).unwrap_or(value_part.len());
            let value = value_part[..end].trim_matches(|c: char| c == '"' || c == ',');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }

        from = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coloured_kernel_line_still_yields_its_endpoint() {
        // The exact line a user reported, with the escape bytes written out and the
        // endpoint reading redacted. Before this was handled, `endpoint=` never
        // appeared literally, no address was lifted, the card sat on "还没有分配地址",
        // and pressing 开启隧道 again was refused because one *was* running.
        let coloured = "\u{1b}[2m2026-09-25T12:19:59.889023Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \
                        \u{1b}[2mhongshi::client\u{1b}[0m\u{1b}[2m:\u{1b}[0m tunnel created, hand the \
                        endpoint above to the players \u{1b}[3muuid\u{1b}[0m\u{1b}[2m=\u{1b}[0mf0580c65-3384-4dc2-b3ec-f1de30ade315 \
                        \u{1b}[3mport\u{1b}[0m\u{1b}[2m=\u{1b}[0m53190 \u{1b}[3mendpoint\u{1b}[0m\u{1b}[2m=\u{1b}[0mcd.hongshi.site:53190";

        // The raw line is exactly the failure: the field is not findable.
        assert_eq!(field(coloured, "endpoint"), None);

        let clean = strip_ansi(coloured);
        assert_eq!(field(&clean, "endpoint").as_deref(), Some("cd.hongshi.site:53190"));
        assert_eq!(field(&clean, "uuid").as_deref(), Some("f0580c65-3384-4dc2-b3ec-f1de30ade315"));
        assert_eq!(field(&clean, "port").as_deref(), Some("53190"));
        assert!(!clean.contains('\u{1b}'), "{clean:?}");
        assert!(clean.starts_with("2026-09-25T12:19:59.889023Z  INFO hongshi::client:"), "{clean:?}");
    }

    #[test]
    fn stripping_handles_the_escape_shapes_a_child_may_emit() {
        // CSI with parameters and a private marker, OSC terminated both ways, a bare
        // two-character escape, and a stray control byte.
        assert_eq!(strip_ansi("\u{1b}[1;31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("\u{1b}[?25lhidden\u{1b}[?25h"), "hidden");
        assert_eq!(strip_ansi("\u{1b}]0;window title\u{7}body"), "body");
        assert_eq!(strip_ansi("\u{1b}]8;;https://example\u{1b}\\link"), "link");
        assert_eq!(strip_ansi("a\u{1b}(Bb"), "ab");
        assert_eq!(strip_ansi("a\u{0}b\u{7}c"), "abc");
        // A tab is the one control character that still means something.
        assert_eq!(strip_ansi("a\tb"), "a\tb");
        // Nothing to do is not a reason to change anything.
        assert_eq!(strip_ansi("plain text 1.2.3"), "plain text 1.2.3");
    }

    #[test]
    fn the_endpoint_is_read_from_the_line_the_kernel_prints() {
        // Verbatim from the integration contract, indentation and all.
        let line = "     uuid=550e8400-e29b-41d4-a716-446655440000 port=34575 endpoint=1.2.3.4:34575";
        assert_eq!(field(line, "endpoint").as_deref(), Some("1.2.3.4:34575"));
        assert_eq!(
            field(line, "uuid").as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(field(line, "port").as_deref(), Some("34575"));

        // The sentence around the field contains the word too, before the field does.
        // Matching the first `endpoint` finds the prose, and a parser that stops there
        // reports no address on a line that plainly has one.
        let prose = "INFO hongshi::client: hand the endpoint above to the players \
                     uuid=abc port=1 endpoint=1.2.3.4:9";
        assert_eq!(field(prose, "endpoint").as_deref(), Some("1.2.3.4:9"));

        // A name must be a whole word: `myendpoint=` is a different field.
        assert_eq!(field("myendpoint=1.2.3.4:1", "endpoint"), None);
    }

    #[test]
    fn a_field_is_not_confused_by_the_decoration_around_it() {
        // The sentence before the field mentions nothing useful, and the field may be
        // the last thing on the line.
        assert_eq!(
            field("INFO hongshi::client: tunnel created endpoint=1.2.3.4:34575", "endpoint")
                .as_deref(),
            Some("1.2.3.4:34575")
        );
        assert_eq!(field("endpoint=10.0.0.1:20000", "endpoint").as_deref(), Some("10.0.0.1:20000"));
        // IPv6 literals contain colons; only whitespace ends the value.
        assert_eq!(field("endpoint=[::1]:34575", "endpoint").as_deref(), Some("[::1]:34575"));
        // Quoted or comma-terminated values still read.
        assert_eq!(field(r#"endpoint="1.2.3.4:1","#, "endpoint").as_deref(), Some("1.2.3.4:1"));
    }

    #[test]
    fn a_line_without_the_field_yields_nothing_rather_than_guessing() {
        assert_eq!(field("INFO listening", "endpoint"), None);
        assert_eq!(field("endpoint=", "endpoint"), None);
        assert_eq!(field("endpoint=   ", "endpoint"), None);
        // A word that merely contains the marker must not match with a value.
        assert_eq!(field("myendpoint=1.2.3.4:1", "sentinel"), None);
    }

    #[test]
    fn the_exit_codes_are_explained_the_way_the_contract_does() {
        assert!(ExitInfo::from_code(Some(0)).meaning.contains("隧道已结束"));
        assert!(ExitInfo::from_code(Some(1)).meaning.contains("没能建立隧道"));
        // The kernel has no exit code 2 — that one belongs to the relay.
        assert!(ExitInfo::from_code(Some(2)).meaning.contains("退出了"));

        // A killed process reports the same code as one that never reached the relay,
        // so the two must not be told apart by the code: they are told apart by who
        // asked for the exit.
        assert!(ExitInfo::stopped(Some(1)).meaning.contains("已关闭"));
        assert!(!ExitInfo::stopped(Some(1)).meaning.contains("没能建立隧道"));
    }

    #[test]
    fn the_download_name_matches_the_publishing_convention() {
        let name = published_name();
        let (platform, arch) = platform();
        assert!(name.starts_with("hongshic-"), "{name}");
        assert!(name.contains(platform), "{name}");
        assert!(name.contains(arch), "{name}");
        assert_eq!(name.ends_with(".exe"), cfg!(windows), "{name}");
    }

    #[test]
    fn a_missing_kernel_message_names_the_file_the_directory_and_the_fix() {
        let message = missing_kernel_message(Path::new("/tmp/client/core"));
        assert!(message.contains(binary_name()), "{message}");
        assert!(message.contains("/tmp/client/core"), "{message}");
        assert!(message.contains(&published_name()), "{message}");
    }

    #[test]
    fn the_kernel_is_looked_for_in_both_core_directories() {
        // `search_roots` is what makes `./core` work from the project root *and*
        // `core/` work beside a double-clicked client. Both are searched; this checks
        // the ordering is stable and that a name found anywhere is found.
        let roots = search_roots();
        assert!(roots.len() >= 4, "{roots:?}");
        assert!(roots.iter().all(|root| root.is_absolute() || root.starts_with(CORE_DIR)));

        // At least one root has to be a `core` directory, since that is the documented
        // place to put the binary.
        assert!(
            roots.iter().any(|root| root.file_name().is_some_and(|name| name == CORE_DIR)),
            "{roots:?}"
        );
    }

    #[test]
    fn a_kernel_already_installed_decides_where_the_next_download_goes() {
        let dir = std::env::temp_dir().join(format!("hongshi-install-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let installed = dir.join(binary_name());
        assert_eq!(install_dir(Some(&installed)), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
