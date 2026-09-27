//! Console logging.
//!
//! The shell's stdout is read by humans staring at a terminal and, later, by
//! whoever launches it from a script. It stays plain text on one line per event:
//! `LEVEL message  key=value key=value`, the same shape `tracing`'s default
//! formatter produces in the kernel, so a support transcript from either half of
//! the product looks alike.
//!
//! **Nothing here may block the caller.** Logging runs on the request path — every
//! served request produces a line — so a stdout that stops draining stops the whole
//! server. That is not hypothetical: with stdout on a pipe that nobody reads, the
//! pipe's buffer fills after a few dozen lines and the next write parks the calling
//! thread forever. One stalled reader and the client stops answering HTTP entirely,
//! which is exactly what happened the first time the acceptance script made a few
//! more requests than the pipe had room for.
//!
//! So the writing happens on its own thread behind a bounded queue. A caller hands
//! over a line and returns; when the reader is too slow to keep up, lines are
//! dropped and counted rather than queued without limit — a client that keeps
//! serving with a lossy console beats a client that stops serving to keep its log.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::OnceLock;

/// Whether debug-level lines are printed. Set once at startup from `--verbose`.
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// How many lines may be waiting to be written.
///
/// Sized for a burst — a page load logs a dozen lines, a probe of every relay logs a
/// few more — not for a transcript: if a reader is slower than the server for longer
/// than this, the answer is to drop lines, and dropping is what the count is for.
const QUEUE: usize = 1024;

static SENDER: OnceLock<SyncSender<(Stream, String)>> = OnceLock::new();
/// Lines handed to the writer, and lines it has finished with. Equal means drained.
static ENQUEUED: AtomicU64 = AtomicU64::new(0);
static HANDLED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

fn sender() -> &'static SyncSender<(Stream, String)> {
    SENDER.get_or_init(|| {
        let (tx, rx) = sync_channel::<(Stream, String)>(QUEUE);
        std::thread::Builder::new()
            .name("log".to_string())
            .spawn(move || {
                let mut out = std::io::stdout();
                let mut err = std::io::stderr();
                while let Ok((stream, line)) = rx.recv() {
                    // Errors are dropped on purpose: a closed or broken pipe must not
                    // stop the shell, and there is nowhere left to report it.
                    let sink: &mut dyn std::io::Write = match stream {
                        Stream::Out => &mut out,
                        Stream::Err => &mut err,
                    };
                    let _ = sink.write_all(line.as_bytes());
                    let _ = sink.write_all(b"\n");
                    let _ = sink.flush();
                    HANDLED.fetch_add(1, Ordering::Relaxed);
                }
            })
            .expect("spawn the log writer");
        tx
    })
}

/// Hand one line to the writer, or drop it if the reader is too far behind.
///
/// Returns immediately in every case. That is the whole contract: a request thread
/// must never wait on a console.
fn enqueue(stream: Stream, line: String) {
    // Counted before the attempt, so a drop can settle the same counter the writer
    // would have: `flush` then means "nothing is left in the queue", whether the
    // lines were written or discarded.
    ENQUEUED.fetch_add(1, Ordering::Relaxed);
    match sender().try_send((stream, line)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            HANDLED.fetch_add(1, Ordering::Relaxed);
        }
        // The writer thread is gone, which can only happen while the process is
        // tearing down. Dropping the line is the only thing left to do.
        Err(TrySendError::Disconnected(_)) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            HANDLED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many lines were dropped because nothing was reading fast enough.
pub fn dropped_lines() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// Wait, briefly, for the queue to empty; returns how many lines were dropped.
///
/// Called on the way out so the last lines are not lost to the exit — but bounded,
/// because the one thing a reader that has stopped reading must not be able to do is
/// hold the process open.
pub fn flush(timeout: std::time::Duration) -> u64 {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if HANDLED.load(Ordering::Relaxed) >= ENQUEUED.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    DROPPED.load(Ordering::Relaxed)
}

/// Flushes the log when it goes out of scope, on every return path.
///
/// A `Drop` guard rather than a call before each `return`: `run()` has several exits
/// — usage errors, `--version`, the normal shutdown — and the ones that print least
/// are the ones whose output a user is most likely to be waiting for.
pub struct FlushOnExit;

impl Drop for FlushOnExit {
    fn drop(&mut self) {
        flush(std::time::Duration::from_millis(500));
    }
}

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

fn enabled(level: &str) -> bool {
    level != "DEBUG" || VERBOSE.load(Ordering::Relaxed)
}

/// Longest a single logged value may be, so one hostile request cannot wrap the
/// console into a screenful of its own text.
const MAX_VALUE_LEN: usize = 120;
const TRUNCATED: &str = "…";

fn emit(level: &str, message: &str, fields: &[(&str, &str)]) {
    if !enabled(level) {
        return;
    }
    let mut line = String::with_capacity(64 + message.len());
    line.push_str(level);
    line.push(' ');
    line.push_str(&sanitize(message, MAX_VALUE_LEN));
    for (key, value) in fields {
        line.push_str("  ");
        line.push_str(key);
        line.push('=');
        line.push_str(&sanitize(value, MAX_VALUE_LEN));
    }
    write_line(&line);
}

/// Make a value safe to place on one console line.
///
/// Everything logged here is attacker-influenced: the request path is
/// percent-decoded before it is logged, so `GET /x%0aINFO%20forged` would
/// otherwise fabricate a whole extra log line — and since refusals are logged
/// too, a caller that is turned away still reaches this code path first. Escape
/// sequences are the same problem one level down: a
/// raw `ESC[31m` in a path repaints the terminal the user is being told to trust.
///
/// So: replace control characters with `?` and cap the length. Replacing rather
/// than dropping keeps a hostile path visible — `/x?INFO?forged` reads as the
/// attack it is — while guaranteeing one log line per request. Real paths
/// (`/api/health`, `/main.css`) pass through untouched.
fn sanitize(value: &str, limit: usize) -> String {
    let mut out = String::with_capacity(value.len().min(limit + TRUNCATED.len()));
    let mut taken = 0usize;
    for ch in value.chars() {
        if taken >= limit {
            out.push_str(TRUNCATED);
            break;
        }
        out.push(if ch.is_control() { '?' } else { ch });
        taken += 1;
    }
    out
}

/// Write one line to stdout, and never fail the process doing it.
///
/// `println!` panics when the write fails, which means a closed pipe kills the
/// shell: `hongshi | head`, a console window the user closed, or a launcher that
/// stops reading. The shell itself is fine in all three cases — it has a server
/// to run and a browser to serve — so logging is best effort and its errors are
/// dropped. A panic here would report "an orderly shutdown" as exit code 101.
///
/// (On Unix this is not about SIGPIPE: `std` ignores SIGPIPE at startup, so the
/// write returns `EPIPE` and it is `println!`'s own unwrap that panics. The fix is
/// the same either way.)
pub fn write_line(line: &str) {
    enqueue(Stream::Out, line.to_string());
}

/// [`write_line`] for output that is not a log record: the banner, the usage
/// text, the version line. Same rule — a write error must never be fatal.
pub fn print_raw(text: &str) {
    write_line(text.trim_end_matches('\n'));
}

/// Print usage text to stderr without the possibility of a panic.
pub fn eprint_raw(text: &str) {
    for line in text.lines() {
        enqueue(Stream::Err, line.to_string());
    }
}

pub fn info(message: &str) {
    emit("INFO", message, &[]);
}

pub fn info_fields(message: &str, fields: &[(&str, &str)]) {
    emit("INFO", message, fields);
}

pub fn warn(message: &str) {
    emit("WARN", message, &[]);
}

pub fn warn_fields(message: &str, fields: &[(&str, &str)]) {
    emit("WARN", message, fields);
}

pub fn debug(message: &str) {
    emit("DEBUG", message, &[]);
}

pub fn debug_fields(message: &str, fields: &[(&str, &str)]) {
    emit("DEBUG", message, fields);
}

/// A fatal error, printed the way the kernel prints one: the caller decides the
/// exit code, this only guarantees the shape of the line.
pub fn error(message: &str) {
    emit("ERROR", message, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newline_in_a_value_cannot_forge_a_log_line() {
        // The request path is percent-decoded before it is logged, so a hostile
        // page can put a newline in it. Without this, `GET /x%0aINFO%20forged`
        // produces a second console line that looks exactly like a real one.
        let forged = "/x\nINFO forged URL http://evil/ status=200";
        let sanitized = sanitize(forged, MAX_VALUE_LEN);
        assert!(!sanitized.contains('\n'), "{sanitized:?}");
        assert!(!sanitized.contains('\r'), "{sanitized:?}");
        assert_eq!(sanitized, "/x?INFO forged URL http://evil/ status=200");
    }

    #[test]
    fn an_escape_sequence_cannot_repaint_the_terminal() {
        let sanitized = sanitize("/y\u{1b}[31mred", MAX_VALUE_LEN);
        assert!(!sanitized.chars().any(char::is_control), "{sanitized:?}");
        assert_eq!(sanitized, "/y?[31mred");
    }

    #[test]
    fn real_paths_are_left_alone() {
        for path in ["/", "/index.html", "/main.css", "/api/health", "/asset/icons/linux.svg"] {
            assert_eq!(sanitize(path, MAX_VALUE_LEN), path);
        }
    }

    #[test]
    fn a_long_value_is_truncated_to_one_console_line() {
        let long = "a".repeat(5000);
        let sanitized = sanitize(&long, MAX_VALUE_LEN);
        assert_eq!(sanitized.chars().count(), MAX_VALUE_LEN + 1);
        assert!(sanitized.ends_with(TRUNCATED));
    }
}
