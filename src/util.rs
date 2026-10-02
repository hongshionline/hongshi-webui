//! Small helpers: query parsing, percent decoding and ISO-8601 timestamps.
//!
//! There used to be a fourth thing here — a 16-byte session token, generated from
//! the OS CSPRNG and compared in constant time. It was removed when the page was
//! made openable at any time (see `http_server::route`): the token's only job was
//! to keep *other programs on the same machine* out, and it cost the user a page
//! that died on every reload, because a refresh re-requests `/` without the query
//! string the token lived in. What guards the port now is narrower and has no
//! footprint in the URL: loopback-only binding, plus an `Origin`/`Host` check that
//! refuses a request made by any page but our own.

use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Percent decoding
// ---------------------------------------------------------------------------

/// Decode `%XX` escapes. Invalid escapes are passed through verbatim rather
/// than failing the request: a malformed query string is the caller's problem,
/// and it can never make the path escape the embedded asset table.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            match (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Whole seconds since the Unix epoch, or 0 if the clock is before 1970.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// ISO-8601 UTC timestamp to the second, e.g. `2026-09-25T16:10:52Z`.
///
/// Hand-rolled because the shell has no date library and needs exactly one
/// format: the log lines and the `/api/health` payload both carry it, and both
/// are read by a human during a bug report.
pub fn iso8601_utc() -> String {
    iso8601_from(unix_seconds())
}

/// Same as [`iso8601_utc`] but for an explicit instant, so it can be tested
/// against known dates instead of against the wall clock.
pub fn iso8601_from(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let time_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    )
}

/// `YYYY-MM-DD` for a day number (days since the epoch), UTC.
///
/// The calendar in [`crate::sessions`] buckets by whole days and needs a label for
/// each, which is the day part of [`iso8601_from`] without the time — written as its
/// own function because carrying a `T00:00:00Z` around to slice it off again is how
/// two date formats end up in one payload.
pub fn date_from_days(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 -> (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding_handles_escapes_and_junk() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn iso8601_matches_known_instants() {
        // 1970-01-01, a leap day, and a date after it.
        assert_eq!(iso8601_from(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_from(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601_from(1_767_225_600), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_now_looks_like_a_timestamp() {
        let now = iso8601_utc();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z'), "{now}");
        assert!(now.starts_with("20"), "{now}");
    }
}
