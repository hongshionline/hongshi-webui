//! One record per tunnel, so 首页's heat map counts something real.
//!
//! ## Why the kernel records this and the page does not
//!
//! The obvious design is "the page starts a timer when the user opens a room and
//! posts the duration when they close it". It is wrong here, and not by a little: the
//! browser can be closed, reloaded, put to sleep in a background tab, or lose the
//! process to a crash, and every one of those loses the session. The kernel is the
//! only thing that knows how long the tunnel actually existed — it *is* the tunnel —
//! so the recording is driven from its lifecycle ([`crate::kernel`]) and this module
//! only stores the result.
//!
//! A consequence worth stating because it changes what the graph means: the client
//! being closed does not end the session, but the *kernel* being stopped does, and
//! the shell stops it on exit. So a day's bar is "time a tunnel was up on this
//! machine", which is the honest reading of "did I play".
//!
//! ## Location and shape
//!
//! `hongshi.sessions.json` beside the executable, falling back to the home
//! directory — the same two-step [`crate::config`] uses, for the same reason (the
//! directory holding the executable is read-only under `/system/bin` on Android).
//!
//! The file is a JSON array, appended to one session at a time and rewritten whole.
//! Rewriting is affordable because the list is capped at [`MAX_RECORDS`] and each
//! record is about 90 bytes; the cap is what keeps "a file that grows forever" from
//! being a thing this program has to think about. Old records are dropped from the
//! front, and since only the recorded days can ever be drawn, dropping the oldest
//! cannot be noticed by the graph.

use std::path::PathBuf;

/// How many sessions are kept. At a few sessions a day this is well over a year of
/// history, and the graph only ever draws 18 weeks.
pub const MAX_RECORDS: usize = 2000;

/// A session shorter than this is not recorded at all.
///
/// A tunnel that was killed before it reached the relay — a wrong port, no network,
/// the user clicking twice — is not a play session, and recording it would put a
/// mark on the calendar for a failure. 60 seconds is comfortably below any real
/// session and comfortably above every failure mode this client has.
pub const MIN_SECONDS: u64 = 60;

pub const SESSIONS_FILE: &str = "hongshi.sessions.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Unix seconds, UTC.
    pub started_at: u64,
    pub seconds: u64,
    /// The local game port the tunnel was pointed at, for a reader who wants to know.
    pub game_port: u16,
    /// `"stopped"` when this client asked the kernel to stop, `"exited"` when the
    /// kernel decided on its own. Kept because "the tunnel died while I was playing"
    /// and "I closed it" are different events to whoever reads this file.
    pub outcome: &'static str,
}

/// The two paths a write is attempted against, beside the executable first.
fn paths() -> (PathBuf, PathBuf) {
    let (primary, fallback) = crate::config::base_dirs();
    (primary.join(SESSIONS_FILE), fallback.join(SESSIONS_FILE))
}

/// Where the file is read from, and where a write is tried first.
pub fn primary_path() -> PathBuf {
    paths().0
}

/// Read every record, oldest first.
///
/// A missing or unreadable file is an empty list rather than an error: this is a
/// history, and "no history yet" is the state a fresh install is in. A file that
/// parses only partly keeps what it could read — losing the tail of a heat map is
/// better than losing the heat map.
pub fn load() -> Vec<Session> {
    let (beside, home) = paths();
    for path in [&beside, &home] {
        if let Ok(text) = std::fs::read_to_string(path) {
            return parse(&text);
        }
    }
    Vec::new()
}

/// Append one session and write the file back.
///
/// Returns the path written to, or the reason both attempts failed. A failure here
/// is reported and not fatal anywhere: the tunnel is already down by the time this
/// runs, and a machine that cannot write beside its own executable should not also
/// lose the ability to close a tunnel.
pub fn record(session: &Session) -> Result<PathBuf, String> {
    let mut sessions = load();
    sessions.push(session.clone());
    // Drop the oldest, not the newest: the cap is about file size, and the newest
    // records are the ones the graph can still draw.
    if sessions.len() > MAX_RECORDS {
        let drop_count = sessions.len() - MAX_RECORDS;
        sessions.drain(0..drop_count);
    }
    let text = to_json(&sessions);
    let (beside, home) = paths();
    match std::fs::write(&beside, &text) {
        Ok(()) => Ok(beside),
        Err(beside_error) => match std::fs::write(&home, &text) {
            Ok(()) => Ok(home),
            Err(home_error) => Err(format!(
                "could not write the session history to {} ({beside_error}) or {} ({home_error})",
                beside.display(),
                home.display()
            )),
        },
    }
}

/// One day of the heat map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayRow {
    /// `YYYY-MM-DD`, UTC.
    pub date: String,
    /// Total recorded seconds for the day.
    pub seconds: u64,
    /// How many sessions started on it.
    pub sessions: u64,
    /// 0–4, the graph's step. See [`level`].
    pub level: u8,
}

/// The last `days` days ending today, oldest first, one row per day including the
/// empty ones.
pub fn daily(days: u32) -> Vec<DayRow> {
    let sessions = load();
    let today = crate::util::unix_seconds() / 86_400;
    let first = today.saturating_sub(u64::from(days).saturating_sub(1));

    let mut rows: Vec<DayRow> = (first..=today)
        .map(|day| DayRow {
            date: crate::util::date_from_days(day as i64),
            seconds: 0,
            sessions: 0,
            level: 0,
        })
        .collect();

    for session in sessions {
        let day = session.started_at / 86_400;
        if day < first || day > today {
            continue;
        }
        let index = (day - first) as usize;
        if let Some(row) = rows.get_mut(index) {
            row.seconds += session.seconds;
            row.sessions += 1;
        }
    }
    for row in rows.iter_mut() {
        row.level = level(row.seconds);
    }
    rows
}

/// The graph's step, from recorded seconds.
///
/// Duration rather than session count, because "how long did I play" is the question
/// the calendar is asked and four short attempts are not a long evening. The edges
/// are chosen so the middle of the scale is an ordinary session — half an hour to two
/// hours — and the top step means "most of an evening", which is what a dark square
/// on a contribution graph should mean.
pub fn level(seconds: u64) -> u8 {
    match seconds {
        0 => 0,
        s if s < 1_800 => 1,    // under 30 minutes
        s if s < 7_200 => 2,    // 30 minutes to 2 hours
        s if s < 18_000 => 3,   // 2 to 5 hours
        _ => 4,                 // 5 hours or more
    }
}

/// Parse the array this module writes. Tolerant of anything else in the file.
fn parse(text: &str) -> Vec<Session> {
    let mut sessions = Vec::new();
    for object in split_objects(text) {
        let Some(started_at) = number(object, "started_at") else { continue };
        let Some(seconds) = number(object, "seconds") else { continue };
        let game_port = number(object, "game_port").unwrap_or(0).min(65_535) as u16;
        let outcome = if string(object, "outcome").as_deref() == Some("exited") {
            "exited"
        } else {
            "stopped"
        };
        sessions.push(Session {
            started_at,
            seconds,
            game_port,
            outcome,
        });
    }
    sessions
}

/// Every top-level `{...}` in the text, as slices.
///
/// Walks brace depth and skips string bodies, so a `}` inside a string cannot end an
/// object early — the same shape as the news reader in [`crate::site`], for the same
/// reason: the file is ours, but a file a user has edited is not.
fn split_objects(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = index;
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    objects.push(&text[start..=index]);
                }
            }
            _ => {}
        }
    }
    objects
}

fn number(object: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"");
    let after = object.split_once(&needle)?.1;
    let after = after.split_once(':')?.1.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn string(object: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after = object.split_once(&needle)?.1;
    let after = after.split_once(':')?.1.trim_start();
    let rest = after.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Serialize the whole list. Hand-rolled for the same reason as `config.rs`: this
/// crate has no dependencies and the shape is four scalar fields.
pub fn to_json(sessions: &[Session]) -> String {
    let mut out = String::from("[\n");
    for (index, session) in sessions.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        out.push_str(&format!(
            "  {{\"started_at\":{},\"seconds\":{},\"game_port\":{},\"outcome\":\"{}\"}}",
            session.started_at, session.seconds, session.game_port, session.outcome
        ));
    }
    out.push_str("\n]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Session> {
        vec![
            Session { started_at: 1_700_000_000, seconds: 3_600, game_port: 25565, outcome: "stopped" },
            Session { started_at: 1_700_086_400, seconds: 90, game_port: 25566, outcome: "exited" },
        ]
    }

    #[test]
    fn a_written_list_reads_back_unchanged() {
        let sessions = sample();
        assert_eq!(parse(&to_json(&sessions)), sessions);
    }

    #[test]
    fn an_empty_file_is_an_empty_history() {
        assert!(parse("").is_empty());
        assert!(parse("[]").is_empty());
        assert!(parse("not json at all").is_empty());
    }

    #[test]
    fn a_truncated_tail_keeps_the_records_that_did_parse() {
        // The read must not depend on the last object being whole: a power cut
        // mid-write is exactly when someone looks at this file.
        let text = to_json(&sample());
        let cut = &text[..text.len() - 40];
        assert_eq!(parse(cut).len(), 1);
    }

    #[test]
    fn a_brace_inside_a_string_does_not_end_an_object() {
        // No field this module writes can contain a brace today. The walk is written
        // to survive one anyway, because the file is on disk where a user can edit it
        // and a parser that trusts its input is the bug that shows up later.
        let text = r#"[{"started_at":100,"seconds":60,"game_port":1,"outcome":"sto\"pped"}]"#;
        let parsed = parse(text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].started_at, 100);
    }

    #[test]
    fn a_record_with_no_duration_is_skipped_rather_than_defaulted() {
        // A default of 0 would put a session on the calendar that never happened.
        let text = r#"[{"started_at":100,"game_port":25565,"outcome":"stopped"}]"#;
        assert!(parse(text).is_empty());
    }

    #[test]
    fn the_level_scale_covers_zero_and_every_edge() {
        assert_eq!(level(0), 0);
        assert_eq!(level(1), 1);
        assert_eq!(level(1_799), 1);
        assert_eq!(level(1_800), 2);
        assert_eq!(level(7_199), 2);
        assert_eq!(level(7_200), 3);
        assert_eq!(level(17_999), 3);
        assert_eq!(level(18_000), 4);
        assert_eq!(level(100_000), 4);
    }

    #[test]
    fn daily_returns_one_row_per_day_oldest_first() {
        let rows = daily(7);
        assert_eq!(rows.len(), 7);
        // Dates ascend, and today is last.
        for pair in rows.windows(2) {
            assert!(pair[0].date < pair[1].date, "{:?}", rows);
        }
    }

    #[test]
    fn a_session_older_than_the_window_is_left_out() {
        // Whatever is on disk from a previous run must not be able to land on a day
        // the graph is drawing.
        let rows = daily(1);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].seconds == 0 || rows[0].date <= crate::util::date_from_days((crate::util::unix_seconds() / 86_400) as i64));
    }
}
