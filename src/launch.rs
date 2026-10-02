//! How many times this client has been started.
//!
//! ## Why the count lives here and not in the page
//!
//! "第 N 次启动" is a fact about the *client*, not about one browser profile. Counted in
//! `localStorage` it would reset whenever somebody cleared site data or opened the page
//! in another browser, and it would be counted twice if both were used — so a thank-you
//! that is meant to arrive on the tenth launch would arrive on the tenth launch *of that
//! browser*, or not at all. It is bumped once per process start instead.
//!
//! ## Shape and location
//!
//! `hongshi.launches.json` — one object, one field — beside the executable, falling back
//! to the home directory, which is the two-step [`crate::config`] and
//! [`crate::sessions`] both use (the directory holding the executable is read-only under
//! `/system/bin` on Android).
//!
//! Deliberately **not** a field in `settings.json`: that file is the user's preferences
//! and is rewritten whole every time the settings form is saved, so a counter living
//! there would reset whenever somebody changed their service address. A launch counter
//! that forgets is worse than no launch counter, because the page's only use for it is
//! "is this a round number".
//!
//! ## Failing quietly, in the right direction
//!
//! A missing, unreadable or corrupt file starts the count again at 1. The number is a
//! nudge, and no part of starting a client may depend on it — so every failure here is
//! ignored on purpose, including a write that cannot happen. The failure that *would*
//! matter is counting one launch twice, which is why [`bump`] is called exactly once, at
//! startup, and the value is then carried in memory for the life of the process.

use std::path::PathBuf;

pub const LAUNCHES_FILE: &str = "hongshi.launches.json";

/// The two paths a write is attempted against, beside the executable first.
fn paths() -> (PathBuf, PathBuf) {
    let (primary, fallback) = crate::config::base_dirs();
    (primary.join(LAUNCHES_FILE), fallback.join(LAUNCHES_FILE))
}

/// Where the file is read from, and where a write is tried first.
pub fn primary_path() -> PathBuf {
    paths().0
}

/// The count as it stands on disk, or 0 when there is nothing readable.
///
/// Zero rather than 1 for "never started": the caller that cares is about to bump it,
/// and the difference between "no file" and "one previous launch" has to survive to
/// [`bump`] for the very first launch to come out as 1.
pub fn read() -> u64 {
    let (beside, home) = paths();
    for path in [&beside, &home] {
        if let Ok(text) = std::fs::read_to_string(path) {
            return parse(&text);
        }
    }
    0
}

/// Count this launch and return its number. Called once, from `Shell::start`.
pub fn bump() -> u64 {
    let this_launch = read().saturating_add(1);
    let text = to_json(this_launch);
    let (beside, home) = paths();
    if std::fs::write(&beside, &text).is_err() {
        // A machine that cannot write beside its own executable still gets a count for
        // this run — it just cannot remember it. Nothing is reported: a log line about a
        // launch counter is noise in a console that exists to show tunnels.
        let _ = std::fs::write(&home, text);
    }
    this_launch
}

/// Parse the one field this module writes. Tolerant of anything else in the file.
pub fn parse(text: &str) -> u64 {
    let Some(after) = text.split_once("\"count\"").map(|(_, rest)| rest) else {
        return 0;
    };
    let Some(after) = after.split_once(':').map(|(_, rest)| rest.trim_start()) else {
        return 0;
    };
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
}

/// Serialize the whole file. Hand-rolled for the same reason as `config.rs`: no
/// dependencies, and the shape is one integer.
pub fn to_json(count: u64) -> String {
    format!("{{\"count\":{count}}}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_count_reads_back_unchanged() {
        for count in [0u64, 1, 9, 10, 4_294_967_296] {
            assert_eq!(parse(&to_json(count)), count);
        }
    }

    #[test]
    fn an_empty_or_corrupt_file_is_no_count_at_all() {
        // Not 1: `bump` adds the one, and a default of 1 here would make the first
        // launch report itself as the second — which is the whole feature.
        assert_eq!(parse(""), 0);
        assert_eq!(parse("{}"), 0);
        assert_eq!(parse("not json at all"), 0);
        assert_eq!(parse("{\"count\":}"), 0);
        assert_eq!(parse("{\"count\":\"three\"}"), 0);
    }

    #[test]
    fn a_count_survives_whatever_else_is_in_the_file() {
        // The file is on disk where somebody can edit it, and a parser that trusts its
        // input is the bug that shows up later.
        assert_eq!(parse("{\"note\":\"hand edited\",\"count\":42,\"extra\":true}"), 42);
        assert_eq!(parse("{\"count\": 7 }"), 7);
    }
}
