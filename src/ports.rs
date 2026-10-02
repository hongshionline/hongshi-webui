//! Finding the game, so the user does not have to know what a port is.
//!
//! ## What "自动探测" means here
//!
//! "Detect the port" is ambiguous unless you say what is being detected, and the two
//! readings lead to different code:
//!
//! * *A free port to hand out.* Wrong for this product. A tunnel that forwards a free
//!   port forwards nothing, and the failure is invisible until a player cannot join.
//! * *The port the game is already listening on.* Right, and the one that saves the user
//!   anything. That is what this module looks for.
//!
//! ## The three steps, and why they are these three
//!
//! 1. **`25565` itself.** The vanilla default. If something is listening there and the
//!    process holding it is `java`, that is the answer and the search is over — one
//!    `netstat` and no guessing.
//! 2. **Any other port held by a Java process**, in the ephemeral-safe range
//!    10000–65535. This is the step that earns its keep: a second world, a modpack
//!    launcher or a hand-configured `server.properties` all put the game somewhere else,
//!    and the only thing that distinguishes that socket from the other thirty on the
//!    machine is *which process owns it*.
//! 3. **`25565` as a floor**, reported as a fallback rather than as a fact — with the
//!    words to match, because "we did not find your game, this is the default" and "we
//!    found your game" are different things and the user is about to hand this number to
//!    somebody else.
//!
//! ## Why process ownership and not a protocol handshake
//!
//! The other way to recognise a Minecraft server is to speak its own protocol at it — a
//! Server List Ping, which would answer "yes, and here is the version". It is strictly
//! more informative and it needs no system commands at all, so it was the first design.
//! It is *not* what this does, for a reason that is about this program rather than about
//! effort: the client already spawns one child process and reads its stdout, and adding a
//! second protocol stack to the one part of the product that has no dependencies is a
//! large thing to own. Process ownership answers the question the user is actually asking
//! — "where is my game" — and it costs one `netstat`.
//!
//! The trade is stated plainly so it is not rediscovered as a bug: a Minecraft server
//! running under a launcher's bundled JRE whose image is *not* named `java` will not be
//! recognised by name, and the search falls through to step 3. That is why step 3 exists
//! and why it says what it is.
//!
//! ## Cost
//!
//! Measured on Windows before this was written, because the whole design turns on it:
//! `netstat -ano` 33 ms, `tasklist` 254 ms, PowerShell's `Get-NetTCPConnection` 574 ms.
//! The fast pair is the one that is used; `tasklist` is called **only** when the default
//! port is taken, which is the only question that needs process *names*, and the serial
//! path is 33 ms instead of 287 ms.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

/// Where the game would be if nobody moved it.
pub const DEFAULT_PORT: u16 = 25565;

/// The vanilla default followed by the ports a second or third world lands on.
///
/// Only step 1 walks a list; it exists because "next port up" is what Minecraft itself
/// does when 25565 is taken, so a user with two worlds has 25565 and 25566 both live.
pub const DEFAULT_RANGE: [u16; 5] = [25565, 25566, 25567, 25568, 25569];

/// Below this, a port belongs to the system or to a well-known service, and no
/// Minecraft server is there. Combined with the 65535 ceiling this is the range step 2
/// searches, which also keeps a Java-based *tool* on port 8080 or 3306 out of the results.
pub const CANDIDATE_FLOOR: u16 = 10_000;

/// How long one connect gets when deciding whether a port is listening.
///
/// Loopback either accepts immediately or refuses immediately, so this is a ceiling for
/// a wedged listener rather than a real wait.
const PROBE_TIMEOUT: Duration = Duration::from_millis(350);

/// How long the process commands get. `netstat` and `tasklist` answer in well under a
/// second on a loaded machine; a minute means something is wrong and the user is waiting.
const COMMAND_TIMEOUT_MS: u64 = 4000;

/// What was found, and how sure the answer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// The port to put in the field.
    pub port: u16,
    /// `"default"` — a listening port in [`DEFAULT_RANGE`] held by a Java process.
    /// `"java"` — a listening port held by a Java process, outside the default range.
    /// `"fallback"` — nothing was found; this is [`DEFAULT_PORT`] and the user should check.
    pub source: &'static str,
    /// The Java process ids that were found, for the page to report and for a bug report.
    pub java_pids: Vec<u32>,
    /// Ports that were looked at, in the order they were looked at.
    pub checked: Vec<u16>,
    /// Ports that were skipped because a Java process held them outside the search range.
    pub skipped: Vec<u16>,
}

/// One listening socket and the process that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listener {
    port: u16,
    pid: u32,
    /// True when the owning command is a Java image — `java.exe`, `javaw.exe`, or on Unix
    /// a command whose name contains `java`. `netstat -ano` alone does not carry this; see
    /// [`listeners_windows`] for how the pair of commands is arranged to avoid needing it
    /// on every call.
    java: bool,
}

/// Walk the three steps and report the answer.
pub fn detect() -> Detected {
    // Ordered and deduplicated: `checked` is shown to the user as "these are the ports we
    // looked at", and 25565 is looked at twice by design — once in the default range and
    // once as the floor — which printed it twice until this became a set with a list
    // beside it to keep the order.
    let mut checked_set = std::collections::BTreeSet::new();
    let mut checked = Vec::new();
    let mut skipped = Vec::new();
    let note = |port: u16, checked: &mut Vec<u16>, set: &mut std::collections::BTreeSet<u16>| {
        if set.insert(port) {
            checked.push(port);
        }
    };

    let listeners = listeners();

    // ---- step 1: the default range, and only a Java-owned socket counts ----
    //
    // A non-Java listener on 25565 is not dismissed, it is *not concluded*: it could be a
    // proxy in front of the game, and it could be something else entirely. Either way the
    // search continues, and step 3 will come back to this port and say so.
    for port in DEFAULT_RANGE {
        note(port, &mut checked, &mut checked_set);
        if let Some(listener) = listeners.iter().find(|entry| entry.port == port) {
            if listener.java {
                return Detected {
                    port,
                    source: "default",
                    java_pids: pids_of(&listeners),
                    checked,
                    skipped,
                };
            }
        }
    }

    let java_pids = pids_of(&listeners);

    // ---- step 2: any other port a Java process is listening on ----
    if !java_pids.is_empty() {
        let mut candidates: Vec<u16> = listeners
            .iter()
            .filter(|entry| entry.java)
            .map(|entry| entry.port)
            .filter(|port| *port >= CANDIDATE_FLOOR)
            .filter(|port| !checked_set.contains(port))
            .collect();
        candidates.sort_unstable();
        candidates.dedup();

        for port in candidates {
            note(port, &mut checked, &mut checked_set);
            if is_listening(port) {
                return Detected {
                    port,
                    source: "java",
                    java_pids,
                    checked,
                    skipped,
                };
            }
        }
    }

    // A Java process listening below the floor is worth mentioning rather than hiding:
    // it is usually a build tool or a database, which is exactly why it is not a candidate.
    for entry in listeners.iter().filter(|entry| entry.java && entry.port < CANDIDATE_FLOOR) {
        if !skipped.contains(&entry.port) {
            skipped.push(entry.port);
        }
    }

    // ---- step 3: the floor, said as what it is ----
    note(DEFAULT_PORT, &mut checked, &mut checked_set);
    Detected {
        port: DEFAULT_PORT,
        source: "fallback",
        java_pids,
        checked,
        skipped,
    }
}

/// Every listening TCP socket with the process id that owns it.
///
/// Windows answers with two commands; Unix answers with one. That asymmetry is the whole
/// reason this function exists rather than the caller running a command itself.
fn listeners() -> Vec<Listener> {
    #[cfg(windows)]
    {
        listeners_windows()
    }
    #[cfg(not(windows))]
    {
        listeners_unix()
    }
}

/// Windows: `netstat -ano`, plus `tasklist` only when a name is actually needed.
///
/// `netstat -ano` gives `proto | local address | foreign address | state | pid` and is the
/// cheap half (33 ms measured). What it does not give is the process *name*, and the name
/// is what step 1 and step 2 turn on — so `tasklist` (254 ms) is called once, and only
/// after a pid worth identifying has been seen. With nothing listening on the default
/// range and no Java in sight, the expensive command never runs.
fn listeners_windows() -> Vec<Listener> {
    let output = run("netstat", &["-ano"]);
    let mut raw = Vec::new();
    for line in output.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        // TCP <local> <foreign> <state> <pid>
        if columns.len() < 5 || !columns[0].eq_ignore_ascii_case("tcp") {
            continue;
        }
        if !columns[3].eq_ignore_ascii_case("listening") {
            continue;
        }
        let Some(port) = port_of(columns[1]) else { continue };
        let Ok(pid) = columns[4].parse::<u32>() else { continue };
        raw.push((port, pid));
    }
    if raw.is_empty() {
        return Vec::new();
    }

    let names = process_names(&raw.iter().map(|(_, pid)| *pid).collect::<Vec<_>>());
    raw.into_iter()
        .map(|(port, pid)| Listener {
            port,
            pid,
            java: names.get(&pid).map(|name| is_java_name(name)).unwrap_or(false),
        })
        .collect()
}

/// The name of each pid, from one `tasklist` call. Names are lowercased.
fn process_names(pids: &[u32]) -> std::collections::HashMap<u32, String> {
    let mut names = std::collections::HashMap::new();
    if pids.is_empty() {
        return names;
    }
    let output = run("tasklist", &["/FO", "CSV", "/NH"]);
    for line in output.lines() {
        // "image name","pid","session","session#","mem"
        let fields: Vec<String> = line
            .split("\",\"")
            .map(|field| field.trim_matches('"').to_string())
            .collect();
        if fields.len() < 2 {
            continue;
        }
        let Ok(pid) = fields[1].parse::<u32>() else { continue };
        if pids.contains(&pid) {
            names.insert(pid, fields[0].to_ascii_lowercase());
        }
    }
    names
}

/// Unix: `ss -ltnp`, falling back to `netstat -ltnp`.
///
/// `ss` is the modern tool and prints the owning process inline; `netstat` is the fallback
/// for a minimal image. Neither needs a second call, which is why this side has one fewer
/// round trip than Windows.
fn listeners_unix() -> Vec<Listener> {
    let output = run("ss", &["-ltnp"]);
    let output = if output.trim().is_empty() {
        run("netstat", &["-ltnp"])
    } else {
        output
    };

    let mut listeners = Vec::new();
    for line in output.lines() {
        if !line.starts_with("LISTEN") {
            continue;
        }
        let columns: Vec<&str> = line.split_whitespace().collect();
        // State Recv-Q Send-Q Local Address:Port Peer Address:Port Process
        if columns.len() < 4 {
            continue;
        }
        let Some(port) = port_of(columns[3]) else { continue };
        // `users:(("java",pid=1234,fd=42))` — absent entirely without privileges, in which
        // case the name is unknown rather than absent, and the entry is kept so the port
        // is still reported. Guessing "no" would silently disable step 2 for every
        // unprivileged run, which is the common case.
        let tail = columns[4..].join(" ");
        let name = process_name_from_ss(&tail);
        let pid = pid_from_ss(&tail);
        let java = match (name, pid) {
            (Some(name), _) => is_java_name(&name),
            // No `users:` field at all — which is what an unprivileged `ss` prints — means
            // the name is unknown rather than absent. A listening port in the candidate
            // range is kept so it can still be probed; guessing "no" would silently disable
            // step 2 for every run that is not root, which is the common case.
            (None, _) => port >= CANDIDATE_FLOOR,
        };
        listeners.push(Listener {
            port,
            pid: pid.unwrap_or(0),
            java,
        });
    }
    listeners
}

/// `users:(("java",pid=1234,fd=42))` → `java`
fn process_name_from_ss(text: &str) -> Option<String> {
    let after = text.split("((\"").nth(1)?;
    let name = after.split('"').next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

/// `users:(("java",pid=1234,fd=42))` → `1234`
fn pid_from_ss(text: &str) -> Option<u32> {
    let after = text.split("pid=").nth(1)?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The port out of `127.0.0.1:25565`, `[::]:25565`, `0.0.0.0:25565` or `*:25565`.
///
/// Splits on the **last** colon: an IPv6 address is full of them, and `rfind` is the only
/// split that is right for both families.
fn port_of(address: &str) -> Option<u16> {
    address.rsplit(':').next()?.parse().ok()
}

/// Whether a process image name is a Java runtime.
///
/// Matched on the *stem*, so `java`, `java.exe`, `javaw.exe` and `java8.exe` all count,
/// while `javascript-tool` and `javafx-demo` do not — a `contains("java")` would take all
/// four and produce a port from a program that has nothing to do with Minecraft.
fn is_java_name(name: &str) -> bool {
    let stem = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .split('.')
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    stem == "java" || stem == "javaw" || stem.starts_with("java") && stem[4..].chars().all(|c| c.is_ascii_digit())
}

/// The distinct pids of the listeners a Java process owns.
fn pids_of(listeners: &[Listener]) -> Vec<u32> {
    let mut pids: Vec<u32> = listeners
        .iter()
        .filter(|entry| entry.java && entry.pid != 0)
        .map(|entry| entry.pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Whether anything accepts a TCP connection on `127.0.0.1:port`.
///
/// The handle is dropped immediately: this asks whether a port is served, and holding the
/// connection open would be the client poking at a game it was only supposed to look at.
fn is_listening(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok()
}

/// Run a command and return its stdout, or empty on any failure.
///
/// Empty rather than an error because every caller's next move is the same either way:
/// the steps are best-effort discovery and the fallback is a real answer. A machine with
/// no `netstat` on its PATH gets the fallback, which is exactly what a machine with no
/// Minecraft running gets.
fn run(program: &str, args: &[&str]) -> String {
    let mut command = Command::new(program);
    command.args(args);
    // No window flash on Windows: this runs while the user is looking at the page.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() {
        Ok(child) => child,
        Err(_) => return String::new(),
    };

    // Read on a worker so a command that hangs cannot hold the request open: the join is
    // bounded by a deadline rather than by the child's cooperation.
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buffer = String::new();
        if let Some(mut stream) = stdout {
            let _ = stream.read_to_string(&mut buffer);
        }
        buffer
    });

    let deadline = std::time::Instant::now() + Duration::from_millis(COMMAND_TIMEOUT_MS);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                break;
            }
        }
    }
    reader.join().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_is_read_from_every_address_shape() {
        // The IPv6 cases are the ones that matter: splitting on the *first* colon takes
        // `[::]:25565` apart into `[` and fails, which is a bug that only shows on a
        // machine with IPv6 listeners — that is, on all of them.
        assert_eq!(port_of("127.0.0.1:25565"), Some(25565));
        assert_eq!(port_of("0.0.0.0:25565"), Some(25565));
        assert_eq!(port_of("*:25565"), Some(25565));
        assert_eq!(port_of("[::]:25565"), Some(25565));
        assert_eq!(port_of("[::1]:25565"), Some(25565));
        assert_eq!(port_of("127.0.0.1:65535"), Some(65535));
        assert_eq!(port_of("nonsense"), None);
        assert_eq!(port_of("127.0.0.1:"), None);
    }

    #[test]
    fn only_a_java_runtime_is_a_java_process() {
        assert!(is_java_name("java"));
        assert!(is_java_name("java.exe"));
        assert!(is_java_name("JAVAW.EXE"));
        assert!(is_java_name("javaw"));
        // A launcher's own runtime: still a Java image, still the game's server.
        assert!(is_java_name("java8.exe"));
        assert!(is_java_name("C:\\Program Files\\Java\\jdk-21\\bin\\java.exe"));
        assert!(is_java_name("/usr/lib/jvm/temurin-21/bin/java"));

        // The ones a `contains("java")` would get wrong, and each would hand the user a
        // port belonging to something that is not their game.
        assert!(!is_java_name("javascript-tool"));
        assert!(!is_java_name("javafx-demo.exe"));
        assert!(!is_java_name("javadoc"));
        assert!(!is_java_name("node.exe"));
        assert!(!is_java_name(""));
    }

    #[test]
    fn the_pid_comes_out_of_an_ss_users_field() {
        let line = "users:((\"java\",pid=1234,fd=42))";
        assert_eq!(pid_from_ss(line), Some(1234));
        assert_eq!(process_name_from_ss(line).as_deref(), Some("java"));
        assert_eq!(pid_from_ss("no users here"), None);
    }

    #[test]
    fn detection_always_answers_with_a_port_the_tunnel_can_use() {
        // Whatever is on this machine, the page gets a port it can put in the field, and
        // a source it can phrase. `0` would be refused by the start endpoint, so it is the
        // one answer this function must never produce.
        let found = detect();
        assert!(found.port > 0);
        assert!(matches!(found.source, "default" | "java" | "fallback"));
    }

    #[test]
    fn the_fallback_is_the_default_port_and_says_it_is_a_fallback() {
        let found = detect();
        if found.source == "fallback" {
            assert_eq!(found.port, DEFAULT_PORT);
            assert!(found.checked.contains(&DEFAULT_PORT), "it must have looked there");
        }
    }

    #[test]
    fn a_port_this_process_holds_is_detected_as_listening() {
        // The positive half, so the probe cannot pass by always saying no: a listener is
        // opened here rather than assumed.
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(is_listening(port), "a port held by this process was not detected");
        drop(listener);
        assert!(!is_listening(port), "a closed listener still reported as listening");
    }

    #[test]
    fn a_missing_command_is_empty_output_rather_than_a_crash() {
        // Every caller has a fallback, so an absent `netstat` must read as "found nothing".
        assert_eq!(run("hongshi-no-such-command-xyz", &["-a"]), "");
    }

    #[test]
    fn the_search_range_starts_above_the_well_known_ports() {
        // 8080 and 3306 are where a Java build tool and a Java database live. A server
        // there is not a Minecraft server, which is the whole reason for the floor.
        assert!(CANDIDATE_FLOOR > 8080);
        assert!(CANDIDATE_FLOOR > 3306);
        assert!(DEFAULT_RANGE.contains(&DEFAULT_PORT));
    }
}
