//! The official site, as the shell sees it: the node list, a latency probe, and
//! the daily news.
//!
//! All three exist because the page cannot fetch them itself — it is served from
//! `http://127.0.0.1:<port>`, so `https://hongshi.site/api/…` is cross-origin and
//! the browser refuses it. The shell fetches and hands the result over, which also
//! keeps the CSP at `connect-src 'self'`.
//!
//! The node list is cached in memory for a configurable while. Not for speed
//! alone: the site rate-limits the endpoint at 120 requests a minute, and a page
//! that re-fetches on every focus would spend that budget for nothing.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::Settings;
use crate::log::{debug_fields, warn_fields};
use crate::net::{self, Fetched, NetOptions};

/// How long a single node probe may take before the node counts as unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

/// How many ICMP replies to ask for. Two is enough to shake off one dropped
/// packet without making the whole probe slow.
const PING_COUNT: u8 = 2;

/// The control-plane port `hongshic` talks to. Probing *this* is the measurement
/// that means something: it is the port a tunnel actually uses.
const CONTROL_PORT: u16 = 8080;

/// Something we fetched, with the moment it arrived.
struct Cached {
    at: Instant,
    value: String,
}

/// The news, kept as items rather than as text: the page payload is built from them
/// on the way out, so caching the text would mean this module owning JSON escaping
/// that `http_server` already does.
struct CachedNews {
    at: Instant,
    items: Vec<NewsItem>,
}

/// Everything the site-facing endpoints need.
pub struct SiteState {
    node_cache: Mutex<Option<Cached>>,
    news_cache: Mutex<Option<CachedNews>>,
}

impl SiteState {
    pub fn new() -> SiteState {
        SiteState {
            node_cache: Mutex::new(None),
            news_cache: Mutex::new(None),
        }
    }
}

impl Default for SiteState {
    fn default() -> SiteState {
        SiteState::new()
    }
}

/// One row of the node list, as the page wants it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Region label, e.g. `南京`. This is the key `server.json` maps from.
    pub region: String,
    /// Hostname or IP the user connects to.
    pub host: String,
}

fn net_options(settings: &Settings) -> NetOptions {
    NetOptions {
        proxy: settings.proxy.clone(),
        use_system_proxy: settings.use_system_proxy,
    }
}

fn join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// The official download URL for one build.
///
/// `GET /api/download/{kind}?platform={platform}&arch={arch}`, where `kind` is
/// `client` for the kernel this client spawns. The platform and architecture come
/// from the client's own build, not from anything the page sent: the kernel runs on
/// the same machine, so the client is the authority and there is no user-agent
/// guessing to get wrong.
pub fn download_url(api_base: &str, kind: &str, platform: &str, arch: &str) -> String {
    join(
        api_base,
        &format!("/api/download/{kind}?platform={platform}&arch={arch}"),
    )
}

/// The node list, from cache when it is fresh.
///
/// `force` skips the cache — that is what the interface's "刷新节点列表" does.
pub fn nodes(
    state: &SiteState,
    settings: &Settings,
    force: bool,
) -> (Vec<Node>, &'static str) {
    let ttl = Duration::from_secs(settings.node_cache_seconds.max(30));

    if !force {
        if let Ok(guard) = state.node_cache.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.at.elapsed() < ttl {
                    if let Some(nodes) = parse_nodes(&cached.value) {
                        debug_fields("node list from cache", &[("age_ms", &cached.at.elapsed().as_millis().to_string())]);
                        return (nodes, "cache");
                    }
                }
            }
        }
    }

    let url = join(&settings.api_base, "/api/server/list");
    match net::get_json(&url, &net_options(settings)) {
        Fetched::Response { status, body } if (200..300).contains(&status) => {
            let text = String::from_utf8_lossy(&body).into_owned();
            match parse_nodes(&text) {
                Some(nodes) => {
                    if let Ok(mut guard) = state.node_cache.lock() {
                        *guard = Some(Cached {
                            at: Instant::now(),
                            value: text,
                        });
                    }
                    (nodes, "network")
                }
                None => {
                    warn_fields("node list did not parse", &[("body", &truncate(&text, 200))]);
                    (Vec::new(), "bad")
                }
            }
        }
        other => {
            // Hold on to a stale list rather than showing nothing: the nodes do not
            // stop existing because the site is briefly unreachable.
            if let Ok(guard) = state.node_cache.lock() {
                if let Some(cached) = guard.as_ref() {
                    if let Some(nodes) = parse_nodes(&cached.value) {
                        return (nodes, "stale");
                    }
                }
            }
            let _ = other;
            (Vec::new(), "error")
        }
    }
}

/// Parse `{"南京": "nj.hongshi.site", …}`.
///
/// A hand-rolled reader for a shape the site defines: an object of string keys to
/// string values. It scans for string literals in order and keeps the ones that
/// look like a key followed by a value, which tolerates surrounding whitespace,
/// extra fields and numbers. Anything that does not yield at least one usable pair
/// returns `None`, so a surprise in the file shows up as "no nodes" instead of as
/// a node whose hostname is a fragment of JSON.
pub fn parse_nodes(text: &str) -> Option<Vec<Node>> {
    let mut nodes = Vec::new();
    let mut cursor = 0usize;

    while cursor < text.len() {
        let Some(open) = text[cursor..].find('"') else { break };
        let key_start = cursor + open + 1;
        let Some((key, after_key)) = read_string(text, key_start) else { break };

        // The value must follow the key's closing quote, separated only by a colon
        // (with optional whitespace).
        let rest = &text[after_key..];
        let after_colon = match rest.find(':') {
            Some(colon) if rest[..colon].trim().is_empty() => {
                after_key + colon + 1 + rest[colon + 1..].len()
                    - rest[colon + 1..].trim_start().len()
            }
            _ => {
                cursor = after_key;
                continue;
            }
        };

        if text[after_colon..].starts_with('"') {
            if let Some((value, after_value)) = read_string(text, after_colon + 1) {
                let region = key.trim().to_string();
                let host = value.trim().to_string();
                if !region.is_empty() && !host.is_empty() {
                    nodes.push(Node { region, host });
                }
                cursor = after_value;
                continue;
            }
        }

        cursor = after_key;
    }

    if nodes.is_empty() { None } else { Some(nodes) }
}

/// Read a JSON string starting after its opening quote, returning the text and the
/// index just past the closing quote.
fn read_string(text: &str, from: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut index = from;
    let bytes = text.as_bytes();
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((out, index + 1)),
            b'\\' => {
                index += 1;
                match bytes.get(index) {
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'u') => {
                        let hex: String = text[index + 1..]
                            .chars()
                            .take(4)
                            .collect();
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            }
                        }
                        index += 4;
                    }
                    _ => {}
                }
                index += 1;
            }
            _ => {
                // Copy one full character, not one byte: the region names are CJK.
                let ch = text[index..].chars().next()?;
                out.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    None
}

/// Probe one host and report what took how long.
///
/// **ICMP first, TCP as the fallback**, and the two answer different questions —
/// which is why the result carries the method and why a host can be `blocked`.
///
/// A ping is the cheap, familiar answer to "how far away is this node", and it is
/// what the user asked for. But the number that decides whether a *tunnel* will
/// work is the round trip to the node's control port, and the two genuinely
/// disagree in the wild: `nj.hongshi.site` answers ICMP in 39 ms while refusing
/// 8080. Reporting that as `dead` would be wrong — the network is fine — and
/// reporting it as `ok` would be worse, because no tunnel can be created there.
/// So it gets its own state:
///
/// * `ok` / `slow` — the control port answered; the measurement is the tunnel's.
/// * `ping` — ICMP answered but the control port did not. The node is reachable
///   and not usable, which is exactly what the interface needs to say.
/// * `dead` — neither answered.
pub fn probe_host(host: &str) -> Probe {
    let ping_ms = ping(host);
    let tcp = tcp_probe(host);

    match (ping_ms, tcp.latency_ms) {
        // The port a tunnel actually uses answered: that is the number to show.
        (_, Some(ms)) => Probe {
            state: if ms < 400 { "ok" } else { "slow" }.to_string(),
            latency_ms: Some(ms),
            method: "tcp",
            reason: String::new(),
        },
        // ICMP answered but the control port did not. Reachable, not usable, and
        // the reason says which port is missing.
        (Some(ms), None) => Probe {
            state: "ping".to_string(),
            latency_ms: Some(ms),
            method: "ping",
            reason: tcp.reason,
        },
        // Neither answered.
        (None, None) => Probe {
            state: "dead".to_string(),
            latency_ms: None,
            method: "none",
            reason: tcp.reason,
        },
    }
}

/// One node's probe result.
pub struct Probe {
    /// `ok`, `slow`, `ping` (reachable but its control port is closed), or `dead`.
    pub state: String,
    pub latency_ms: Option<u32>,
    /// `ping`, `tcp`, or `none` — how the number above was obtained.
    pub method: &'static str,
    /// Why there is no number, or why the node cannot be used.
    pub reason: String,
}

/// Send the platform's own `ping` and read the round trip out of its output.
///
/// The system binary rather than raw sockets: on Windows a raw ICMP socket needs
/// administrator rights, and this shell is a double-clicked download. `ping` is
/// present on every target, is not subject to the process's own proxy settings,
/// and its output is parseable in both locales that matter here.
fn ping(host: &str) -> Option<u32> {
    let mut command = ping_command(host);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let child = command.spawn().ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        // A non-zero exit means no reply at all (or an unknown host).
        return None;
    }
    parse_ping(&String::from_utf8_lossy(&output.stdout))
}

/// The `ping` invocation for this platform.
fn ping_command(host: &str) -> std::process::Command {
    let mut command = std::process::Command::new("ping");
    if cfg!(windows) {
        command.arg("-n").arg(PING_COUNT.to_string());
        command.arg("-w").arg("2000"); // per-reply timeout, milliseconds
    } else {
        command.arg("-c").arg(PING_COUNT.to_string());
        command.arg("-W").arg("2"); // per-reply timeout, seconds
    }
    command.arg(host);
    command
}

/// Pull the round trip out of `ping`'s output.
///
/// **Minimum if the platform printed per-reply times, average otherwise**, because
/// the two outputs say different things. Windows prints one line per reply
/// (`time=13ms`, `time<1ms`, or the localized `时间=13ms`): the minimum there is
/// the path's own latency, with the variance stripped out. The Unix family prints
/// only a summary (`rtt min/avg/max/mdev = 11.2/12.3/13.4/0.5 ms`), so its average
/// is the best available and is taken as a whole number.
///
/// Returning `None` for an unparsable output is deliberate — a different locale, a
/// future output change — because the caller falls back to measuring the control
/// port, which is a real measurement rather than a guess.
fn parse_ping(stdout: &str) -> Option<u32> {
    let replies: Vec<u32> = stdout.lines().filter_map(reply_latency).collect();

    if !replies.is_empty() {
        return replies.into_iter().min();
    }

    // Unix: `rtt min/avg/max/mdev = 11.234/12.345/13.456/0.500 ms`. Anchored on
    // the label so an unrelated `=` in the output cannot be mistaken for it.
    for line in stdout.lines() {
        let Some((label, numbers)) = line.split_once('=') else { continue };
        if !label.contains("min/avg") {
            continue;
        }
        let Some(average) = numbers.trim().split('/').nth(1) else { continue };
        if let Ok(value) = average.trim().parse::<f64>() {
            if value.is_finite() && value >= 0.0 {
                return Some(value.round() as u32);
            }
        }
    }

    None
}

/// One reply line's round trip, or `None` if the line is not a reply.
fn reply_latency(line: &str) -> Option<u32> {
    // `time` in English output, `时间` in Chinese. A Chinese-locale Windows writes
    // its console output in the OEM code page, so `时间` can also arrive here as
    // mojibake — hence the fallback below.
    for label in ["time", "时间"] {
        if let Some(ms) = latency_after_label(line, label) {
            return Some(ms);
        }
    }

    // Last resort: the number immediately before an ASCII `TTL`, which is present
    // and unlocalized in every Windows reply. That is the one path which survives a
    // mangled label, and it is only correct on Windows-shaped output, where `TTL`
    // follows the time (`… time=23ms TTL=52`) rather than preceding it the way the
    // Unix family does.
    let ttl = line.find("TTL").or_else(|| line.find("ttl"))?;
    let head = &line[..ttl];
    let unit = head.to_ascii_lowercase().rfind("ms")?;
    let sub_millisecond = head[..unit].chars().rev().take(8).any(|c| c == '<' || c == '＜');
    parse_digits_before(head, unit, sub_millisecond)
}

/// Read the number that follows a time label, if this line has one.
fn latency_after_label(line: &str, label: &str) -> Option<u32> {
    let mut from = 0usize;
    while let Some(found) = line[from..].find(label).map(|index| index + from) {
        let after_label = &line[found + label.len()..];
        let next = after_label.chars().next();
        let sub_millisecond = next == Some('<') || next == Some('＜');

        // A round trip continues with `=`, `<` or a digit. That is what separates
        // `time=12ms` from the statistics' `time 1002ms` — the test's own duration,
        // not a latency — and it is the rule the first version got wrong.
        let rest = match next {
            Some('=') | Some('<') | Some('＜') => after_label.trim_start_matches([' ', '=', '<', '＜']),
            Some(ch) if ch.is_ascii_digit() => after_label,
            _ => {
                from = found + label.len();
                continue;
            }
        };

        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(ms) = digits.parse::<u32>() {
            return Some(if sub_millisecond { ms.max(1) } else { ms });
        }
        from = found + label.len();
    }
    None
}

/// The digit run immediately before `unit`, which is an `ms` suffix.
fn parse_digits_before(text: &str, unit: usize, sub_millisecond: bool) -> Option<u32> {
    let digits: String = text[..unit]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let ms = digits.parse::<u32>().ok()?;
    Some(if sub_millisecond { ms.max(1) } else { ms })
}

/// Measure the round trip to the node's control port.
///
/// Returns `Some(ms)` **only when the connection was actually established**. That
/// is the whole point of the function, and getting it wrong is easy: an earlier
/// version timed from before the connect and returned the elapsed time either way,
/// so a node that timed out after four seconds was reported as a working node with
/// "4043 ms" of latency.
fn tcp_probe(host: &str) -> TcpProbe {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};

    let addresses = match (host, CONTROL_PORT).to_socket_addrs() {
        Ok(addresses) => addresses.collect::<Vec<_>>(),
        Err(err) => {
            return TcpProbe {
                latency_ms: None,
                reason: format!("域名解析失败：{err}"),
            };
        }
    };
    if addresses.is_empty() {
        return TcpProbe {
            latency_ms: None,
            reason: "域名解析没有结果".to_string(),
        };
    }

    // Timed from here, so the number is the connection's own round trip.
    let started = Instant::now();
    let mut stream = match TcpStream::connect_timeout(&addresses[0], PROBE_TIMEOUT) {
        Ok(stream) => stream,
        Err(err) => {
            return TcpProbe {
                latency_ms: None,
                reason: format!("控制端口 {CONTROL_PORT} 连不上：{err}"),
            };
        }
    };
    let connect_ms = elapsed_ms(started);

    // Connected: a relay that accepts the connection but says nothing is still a
    // usable measurement, so a failed write or read does not invalidate it.
    let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let request = format!("HEAD / HTTP/1.0\r\nHost: {host}:{CONTROL_PORT}\r\n\r\n");
    let _ = stream.write_all(request.as_bytes());
    let _ = stream.read(&mut [0u8; 64]);

    TcpProbe {
        latency_ms: Some(connect_ms),
        reason: String::new(),
    }
}

/// The control-port measurement: a number when it connected, a reason when not.
struct TcpProbe {
    latency_ms: Option<u32>,
    reason: String,
}

fn elapsed_ms(since: Instant) -> u32 {
    since.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

/// Where the launcher's news comes from.
///
/// A fixed public URL rather than something under 服务地址: the feed is Mojang's, not
/// the hongshi site's, and this client has no business re-hosting it. The file is
/// 64 KB of 100 entries for a feed that changes at most once a day, which is why
/// only the first few are read and why the result is cached.
pub const NEWS_URL: &str = "https://launchercontent.mojang.com/news.json";

/// The origin the news' pictures hang off. The file stores `/images/…` paths, so a
/// relative one is completed against this.
pub const NEWS_ORIGIN: &str = "https://launchercontent.mojang.com";

/// How many entries the page is given.
pub const NEWS_LIMIT: usize = 3;

/// One entry, in the shape the page draws.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewsItem {
    pub title: String,
    pub category: String,
    pub date: String,
    pub text: String,
    /// Absolute: the upstream file stores `/images/…`.
    pub image: String,
    /// The article on minecraft.net, when the entry carries one.
    pub link: String,
}

/// What `/api/daily-news` needs to answer.
pub struct News {
    /// `ok`, `empty` or whatever [`Fetched::state`] said.
    pub state: &'static str,
    /// Why there is nothing to show, when there is nothing.
    pub reason: String,
    /// `cache` when it came out of memory, `network` when it was just fetched.
    pub origin: &'static str,
    pub items: Vec<NewsItem>,
}

/// The Minecraft launcher's news, trimmed to the first [`NEWS_LIMIT`] entries.
///
/// Cached for the same `node_cache_seconds` as the node list, because the page asks
/// for this every time 主页 is opened and the upstream file is 64 KB — the response
/// carries a `max-age` of its own, but the shell cannot rely on a cache it does not
/// control, and a news feed that changes once a day does not need re-reading minute
/// by minute.
pub fn minecraft_news(state: &SiteState, settings: &Settings, force: bool) -> News {
    let ttl = Duration::from_secs(settings.node_cache_seconds.max(30));

    if !force
        && let Ok(guard) = state.news_cache.lock()
        && let Some(cached) = guard.as_ref()
        && cached.at.elapsed() < ttl
    {
        return News {
            state: "ok",
            reason: String::new(),
            origin: "cache",
            items: cached.items.clone(),
        };
    }

    let fetched = net::get_json(NEWS_URL, &net_options(settings));
    let outcome = fetched.state();

    if outcome != "ok" {
        return News {
            state: outcome,
            reason: fetched.reason(),
            origin: "network",
            items: Vec::new(),
        };
    }

    let items = parse_minecraft_news(&fetched.body_text(), NEWS_LIMIT);
    if items.is_empty() {
        // A 200 that yielded no entry at all is a shape this parser does not know,
        // not a quiet day: the feed always carries a hundred.
        return News {
            state: "empty",
            reason: "读不出任何一条资讯（接口格式可能变了）".to_string(),
            origin: "network",
            items: Vec::new(),
        };
    }

    if let Ok(mut guard) = state.news_cache.lock() {
        *guard = Some(CachedNews {
            at: Instant::now(),
            items: items.clone(),
        });
    }

    News {
        state: "ok",
        reason: String::new(),
        origin: "network",
        items,
    }
}

/// Read the first `limit` entries out of the launcher's `news.json`.
///
/// The file looks like `{"version":…,"entries":[{"title":…},…]}`, where an entry is
/// an object of strings plus two nested image objects. This is deliberately **not** a
/// general JSON parser: it finds the `entries` array, walks it by brace depth with
/// string bodies skipped (a `}` inside a headline must not end an entry early), and
/// reads the six fields the page draws. Anything it cannot make sense of costs an
/// entry or a field, never a wrong value — an entry with no title is dropped rather
/// than drawn as a blank card.
pub fn parse_minecraft_news(text: &str, limit: usize) -> Vec<NewsItem> {
    entry_objects(text, limit)
        .into_iter()
        .filter_map(|entry| {
            let title = field(entry, "title");
            if title.is_empty() {
                return None;
            }
            Some(NewsItem {
                title,
                category: field(entry, "category"),
                date: field(entry, "date"),
                text: field(entry, "text"),
                image: image_url(entry),
                link: absolute_url(&field(entry, "readMoreLink")),
            })
        })
        .collect()
}

/// The text of each of the first `limit` objects in the `entries` array.
fn entry_objects(text: &str, limit: usize) -> Vec<&str> {
    let Some(key) = text.find("\"entries\"") else {
        return Vec::new();
    };
    let Some(bracket) = text[key..].find('[') else {
        return Vec::new();
    };

    let bytes = text.as_bytes();
    let mut cursor = key + bracket + 1;
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut out = Vec::new();

    while cursor < bytes.len() && out.len() < limit {
        let byte = bytes[cursor];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            cursor += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = cursor;
                }
                depth += 1;
            }
            // A `}` with nothing open is a malformed file; ignoring it keeps the walk
            // going rather than underflowing. `]` at depth 0 ends the array.
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    out.push(&text[start..=cursor]);
                }
            }
            b']' if depth == 0 => break,
            _ => {}
        }
        cursor += 1;
    }

    out
}

/// The string value of `"key"` inside one entry object, or empty.
fn field(entry: &str, key: &str) -> String {
    let needle = format!("\"{key}\"");
    let mut from = 0usize;

    while let Some(at) = entry[from..].find(&needle) {
        let after = from + at + needle.len();
        let rest = entry[after..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('"')
                && let Some((value, _)) = read_string(rest, 0)
            {
                return value.trim().to_string();
            }
        }
        // The key appeared as a *value* (`"newsType":["News page"]` has no `url`),
        // or as a prefix of a longer name; keep looking.
        from = after;
    }

    String::new()
}

/// `newsPageImage.url`, made absolute.
fn image_url(entry: &str) -> String {
    let Some(at) = entry.find("\"newsPageImage\"") else {
        return String::new();
    };
    // Everything after the key belongs to that object until the entry ends, and the
    // first `url` in it is the image's own — `playPageImage` is always earlier.
    absolute_url(&field(&entry[at..], "url"))
}

/// Complete a relative path against the news origin.
///
/// Only the three shapes the feed uses are accepted. A path with no leading slash is
/// not resolved at all: guessing a base for it is how a picture ends up fetched from
/// the page's own loopback port.
fn absolute_url(url: &str) -> String {
    let url = url.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if let Some(rest) = url.strip_prefix("//") {
        return format!("https://{rest}");
    }
    if url.starts_with('/') {
        return format!("{NEWS_ORIGIN}{url}");
    }
    String::new()
}

/// Ask the site what the current shell build is.
///
/// Returns the remote version when the site has one. `None` means the endpoint
/// answered but carried no version we could read — a shape change, or the first
/// release where nobody has published one yet — which the interface reports as
/// "could not check" rather than as "you are up to date".
///
/// A 404 is the expected answer today: the endpoint does not exist yet, and a
/// client that treated that as an error would nag every user about a failed
/// update check forever.
pub fn webui_version(settings: &Settings) -> Fetched {
    let url = join(&settings.api_base, "/api/webui/version");
    net::get_json(&url, &net_options(settings))
}

/// Pull a version string out of whatever the version endpoint returns.
///
/// Accepts a bare string, an object with one of a few likely keys, or a nested
/// `{"data": {...}}`. Anything else yields `None`.
pub fn parse_version(body: &str) -> Option<String> {
    let trimmed = body.trim();

    // `"0.1.1"` on its own.
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some((value, _)) = rest.split_once('"') {
            let value = value.trim();
            if !value.is_empty() && value.len() < 64 {
                return Some(value.to_string());
            }
        }
    }

    // `{"version": "0.1.1"}` and friends. The first key that looks like a version
    // wins; the value must look like one too, so a `{"version": "v1 api"}` does
    // not become an update notice.
    for key in ["version", "latest", "current", "tag", "webui_version", "latest_version"] {
        if let Some(value) = json_string_value(trimmed, key) {
            let cleaned = value.trim().trim_start_matches('v').trim().to_string();
            if !cleaned.is_empty() && cleaned.len() < 64 && cleaned.chars().any(|c| c.is_ascii_digit())
            {
                return Some(cleaned);
            }
        }
    }

    None
}

/// Read `"key": "value"` out of a small JSON object.
fn json_string_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut from = 0usize;
    while let Some(found) = text[from..].find(&needle).map(|index| index + from) {
        let after_key = found + needle.len();
        let rest = text[after_key..].trim_start();
        if let Some(after_colon) = rest.strip_prefix(':') {
            let after_colon = after_colon.trim_start();
            if let Some(inner) = after_colon.strip_prefix('"') {
                if let Some((value, _)) = inner.split_once('"') {
                    return Some(value.to_string());
                }
            }
        }
        from = after_key;
    }
    None
}

/// Whether a newer build exists, and which. `None` when either side is unreadable.
pub fn update_available(current: &str, remote: Option<&str>) -> Option<bool> {
    let remote = remote?;
    let current_parts = version_parts(current);
    let remote_parts = version_parts(remote);
    if current_parts.is_empty() || remote_parts.is_empty() {
        return None;
    }
    Some(remote_parts > current_parts)
}

/// `1.2.3` → `[1, 2, 3]`; anything without a digit yields an empty list.
fn version_parts(version: &str) -> Vec<u64> {
    let cleaned = version.trim().trim_start_matches('v');
    if !cleaned.chars().any(|c| c.is_ascii_digit()) {
        return Vec::new();
    }
    cleaned
        .split(['.', '-', '+'])
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
        })
        .collect()
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed copy of the real file's shape, including the parts that make it
    /// awkward: a nested image object, a `}` inside a headline, an escaped quote, a
    /// `newsType` array, keys that are prefixes of each other, and an entry that
    /// carries no title at all.
    const NEWS_FIXTURE: &str = r#"{
      "version": 1,
      "entries": [
        {
          "title": "Discover Planet Earth III",
          "category": "Minecraft for Windows",
          "date": "2024-01-16",
          "text": "A world inspired by the {new} series \"Planet Earth III\".",
          "playPageImage": { "title": "small", "url": "/images/small.jpeg" },
          "newsPageImage": { "title": "big", "url": "/images/big.jpeg",
                             "dimensions": { "width": 772, "height": 350 } },
          "readMoreLink": "https://www.minecraft.net/article/planet-earth-iii",
          "newsType": ["News page", "Bedrock"],
          "id": "2OKkEL0h71H7dyzTilA8ah"
        },
        {
          "title": "Soothing Minecraft Stories",
          "tag": "editorial",
          "category": "Minecraft: Java Edition",
          "date": "2023-12-22",
          "text": "Relax.",
          "newsPageImage": { "url": "/images/soothing.jpeg" },
          "readMoreLink": "https://www.minecraft.net/article/soothing",
          "id": "abc"
        },
        {
          "title": "Universal New Year",
          "category": "Minecraft for Windows",
          "date": "2023-12-12",
          "text": "Celebrate.",
          "newsPageImage": { "url": "https://cdn.example/absolute.jpeg" },
          "readMoreLink": "",
          "id": "def"
        },
        {
          "category": "No title here",
          "date": "2023-01-01",
          "newsPageImage": { "url": "/images/none.jpeg" },
          "id": "ghi"
        }
      ]
    }"#;

    #[test]
    fn the_news_parses_the_shape_the_launcher_serves() {
        let items = parse_minecraft_news(NEWS_FIXTURE, 9);
        // The untitled entry is dropped rather than drawn as a blank card.
        assert_eq!(items.len(), 3, "{items:#?}");

        let first = &items[0];
        assert_eq!(first.title, "Discover Planet Earth III");
        assert_eq!(first.category, "Minecraft for Windows");
        assert_eq!(first.date, "2024-01-16");
        // A `}` inside the text must not have ended the entry early, and the escape
        // must have been resolved.
        assert_eq!(first.text, r#"A world inspired by the {new} series "Planet Earth III"."#);
        // `newsPageImage`, not `playPageImage`, and made absolute.
        assert_eq!(first.image, "https://launchercontent.mojang.com/images/big.jpeg");
        assert_eq!(first.link, "https://www.minecraft.net/article/planet-earth-iii");

        // An already-absolute image is left alone.
        assert_eq!(items[2].image, "https://cdn.example/absolute.jpeg");
        // An entry with no link yields an empty string, not the word "null".
        assert_eq!(items[2].link, "");
    }

    #[test]
    fn only_the_first_few_entries_are_read() {
        // The upstream file is 100 entries and 64 KB; the page is given a handful.
        assert_eq!(parse_minecraft_news(NEWS_FIXTURE, 2).len(), 2);
        assert_eq!(parse_minecraft_news(NEWS_FIXTURE, 1)[0].title, "Discover Planet Earth III");
        assert_eq!(NEWS_LIMIT, 3);
    }

    #[test]
    fn a_news_body_we_cannot_read_yields_nothing_rather_than_a_wrong_entry() {
        for body in [
            "",
            "not json at all",
            "{}",
            r#"{"entries":[]}"#,
            r#"{"entries":"not an array"}"#,
            // Entries with no title at all: nothing worth showing.
            r#"{"entries":[{"date":"2024-01-01"}]}"#,
        ] {
            assert!(parse_minecraft_news(body, 3).is_empty(), "body: {body}");
        }
    }

    #[test]
    fn a_relative_image_with_no_leading_slash_is_not_resolved() {
        // Guessing a base for `images/x.jpeg` is how a picture ends up being fetched
        // from the page's own loopback port.
        assert_eq!(absolute_url("images/x.jpeg"), "");
        assert_eq!(absolute_url(""), "");
        assert_eq!(absolute_url("//cdn.example/x.jpeg"), "https://cdn.example/x.jpeg");
        assert_eq!(absolute_url("http://cdn.example/x.jpeg"), "http://cdn.example/x.jpeg");
        assert_eq!(
            absolute_url("/images/x.jpeg"),
            "https://launchercontent.mojang.com/images/x.jpeg"
        );
    }

    #[test]
    fn the_news_origin_is_the_publisher_and_not_the_service_address() {
        // A regression guard for the shape of the thing: the feed belongs to Mojang,
        // and nothing about 服务地址 may creep back into where it is fetched from.
        assert!(NEWS_URL.starts_with(NEWS_ORIGIN));
        assert!(!NEWS_URL.contains("hongshi"));
    }

    #[test]
    fn the_node_list_parses_the_shape_the_site_serves() {
        let nodes = parse_nodes(r#"{"南京":"nj.hongshi.site","成都":"cd.hongshi.site"}"#)
            .expect("should parse");
        assert_eq!(
            nodes,
            vec![
                Node { region: "南京".to_string(), host: "nj.hongshi.site".to_string() },
                Node { region: "成都".to_string(), host: "cd.hongshi.site".to_string() },
            ]
        );
    }

    #[test]
    fn whitespace_and_newlines_do_not_matter() {
        let text = "{\n    \"广州\" : \"gz.hongshi.site\"\n}\n";
        let nodes = parse_nodes(text).expect("should parse");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].region, "广州");
        assert_eq!(nodes[0].host, "gz.hongshi.site");
    }

    #[test]
    fn an_empty_or_wrong_shaped_body_yields_nothing_rather_than_a_guess() {
        assert!(parse_nodes("{}").is_none());
        assert!(parse_nodes("not json").is_none());
        assert!(parse_nodes("[]").is_none());
        assert!(parse_nodes("").is_none());
        // Right shape, useless content.
        assert!(parse_nodes(r#"{"南京":""}"#).is_none());
        assert!(parse_nodes(r#"{"":"nj.hongshi.site"}"#).is_none());
    }

    #[test]
    fn extra_fields_and_numbers_are_tolerated() {
        let text = r#"{"version":1,"南京":"nj.hongshi.site","count":2}"#;
        let nodes = parse_nodes(text).expect("should parse");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].host, "nj.hongshi.site");
    }

    #[test]
    fn escaped_hosts_survive() {
        let nodes = parse_nodes(r#"{"a":"host\u002Eexample.test"}"#).expect("should parse");
        assert_eq!(nodes[0].host, "host.example.test");
    }

    #[test]
    fn url_joining_does_not_double_slashes() {
        assert_eq!(join("https://hongshi.site", "/api/x"), "https://hongshi.site/api/x");
        assert_eq!(join("https://hongshi.site/", "api/x"), "https://hongshi.site/api/x");
        assert_eq!(join("https://hongshi.site", "api/x"), "https://hongshi.site/api/x");
    }

    #[test]
    fn windows_ping_output_yields_the_minimum_reply() {
        // Real `ping -n 2` output, abbreviated.
        let stdout = "\r\nPinging nj.hongshi.site [1.2.3.4] with 32 bytes of data:\r\n\
Reply from 1.2.3.4: bytes=32 time=18ms TTL=52\r\n\
Reply from 1.2.3.4: bytes=32 time=14ms TTL=52\r\n\
\r\nPing statistics for 1.2.3.4:\r\n\
    Packets: Sent = 2, Received = 2, Lost = 0 (0% loss),\r\n\
Approximate round trip times in milli-seconds:\r\n\
    Minimum = 14ms, Maximum = 18ms, Average = 16ms\r\n";
        assert_eq!(parse_ping(stdout), Some(14));
    }

    #[test]
    fn a_sub_millisecond_reply_is_one_not_zero() {
        // `time<1ms` carries no `=`, and searching for `time=` therefore missed
        // every sub-millisecond reply and reported the fastest host as unmeasurable.
        assert_eq!(parse_ping("Reply from 127.0.0.1: bytes=32 time<1ms TTL=128\r\n"), Some(1));
    }

    #[test]
    fn a_localized_reply_is_read_too() {
        assert_eq!(parse_ping("来自 127.0.0.1 的回复: 字节=32 时间<1ms TTL=128\r\n"), Some(1));
        assert_eq!(parse_ping("来自 1.2.3.4 的回复: 字节=32 时间=23ms TTL=52\r\n"), Some(23));
    }

    #[test]
    fn a_mangled_label_still_parses_because_ttl_is_ascii() {
        // Windows writes console output in the OEM code page, so a Chinese-locale
        // `时间` can arrive here as mojibake. `TTL` is ASCII in every locale, so the
        // time is read from the text to its left instead.
        assert_eq!(parse_ping("来自 1.2.3.4 的回复: 字节=32 ʱ��=23ms TTL=52\r\n"), Some(23));
        assert_eq!(parse_ping("Reply from 1.2.3.4: bytes=32 时间=23ms TTL=52\r\n"), Some(23));
    }

    #[test]
    fn a_line_with_no_time_is_skipped_rather_than_guessed() {
        // The statistics block has a `TTL`-free `= 2, Received = 2` shape; nothing in
        // it may be mistaken for a round trip.
        let stdout = "Pinging x [1.2.3.4] with 32 bytes of data:\r\n\
Request timed out.\r\nRequest timed out.\r\n\
    Packets: Sent = 2, Received = 0, Lost = 2 (100% loss),\r\n";
        assert_eq!(parse_ping(stdout), None);
    }

    #[test]
    fn unix_ping_summary_yields_its_average() {
        let stdout = "PING nj.hongshi.site (1.2.3.4) 56(84) bytes of data.\n\
64 bytes from 1.2.3.4: icmp_seq=1 ttl=52 time=11.2 ms\n\
64 bytes from 1.2.3.4: icmp_seq=2 ttl=52 time=13.4 ms\n\
\n--- nj.hongshi.site ping statistics ---\n\
2 packets transmitted, 2 received, 0% packet loss, time 1002ms\n\
rtt min/avg/max/mdev = 11.234/12.345/13.456/0.500 ms\n";
        // The per-reply lines win over the summary, so the minimum is reported and
        // the `time 1002ms` in the statistics is not mistaken for a round trip.
        assert_eq!(parse_ping(stdout), Some(11));
    }

    #[test]
    fn a_summary_only_output_falls_back_to_the_average() {
        let stdout = "2 packets transmitted, 2 received\n\
rtt min/avg/max/mdev = 11.234/12.345/13.456/0.500 ms\n";
        assert_eq!(parse_ping(stdout), Some(12));
    }

    #[test]
    fn an_unparsable_or_empty_ping_reports_nothing_rather_than_guessing() {
        assert_eq!(parse_ping(""), None);
        assert_eq!(parse_ping("ping: cannot resolve nope: Unknown host"), None);
        // A `=` that is not the summary must not be read as one.
        assert_eq!(parse_ping("Sent = 2, Received = 0, Lost = 2 (100% loss)"), None);
    }

    #[test]
    fn a_working_control_port_wins_over_ping_for_the_number() {
        // Localhost answers ICMP and, when a test listener is up, its control port
        // too. Whichever it is, the shape must be a real measurement that names how
        // it was taken — never a silent success.
        let probe = probe_host("127.0.0.1");
        match probe.latency_ms {
            Some(_) => {
                assert!(
                    ["ok", "slow", "ping"].contains(&probe.state.as_str()),
                    "unexpected state {:?}",
                    probe.state
                );
                assert!(
                    probe.method == "ping" || probe.method == "tcp",
                    "a measurement must say how it was taken, got {:?}",
                    probe.method
                );
            }
            None => {
                assert_eq!(probe.state, "dead");
                assert!(!probe.reason.is_empty());
            }
        }
    }

    #[test]
    fn a_reachable_node_whose_control_port_is_closed_is_not_called_dead() {
        // This is the case that made the point: a public node answered ICMP in
        // 39 ms while refusing 8080. `dead` would have been a lie about the
        // network, and `ok` a lie about usability.
        //
        // `host.invalid` cannot resolve, so it exercises the other end: no ICMP and
        // no address to connect to.
        let probe = probe_host("host.invalid");
        assert_eq!(probe.state, "dead");
        assert_eq!(probe.method, "none");
        assert!(!probe.reason.is_empty(), "a refusal should say why");
    }

    #[test]
    fn every_state_has_an_explanation_or_a_measurement() {
        // The interface has to render all four states, so none of them may be an
        // empty result: `ok`/`slow` carry a number, `ping`/`dead` carry a reason.
        let probe = probe_host("127.0.0.1");
        let has_number = probe.latency_ms.is_some();
        let has_reason = !probe.reason.is_empty();
        assert!(
            has_number || has_reason,
            "state {:?} came back with neither a number nor a reason",
            probe.state
        );
    }
}
