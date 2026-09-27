//! Opening the user's own browser.
//!
//! The shell ships no webview on purpose: the browsers people already have are
//! better than anything that could be embedded, they update themselves, and not
//! embedding one is most of the reason this binary stays small.
//!
//! On Windows the URL is handed to `rundll32 url.dll,FileProtocolHandler` rather
//! than `cmd /C start`. Two reasons, both learned the hard way elsewhere:
//! `cmd.exe` re-parses its arguments, so a URL containing `&`, `^` or a space
//! needs quoting rules that differ again when the browser path has spaces in it;
//! and `rundll32` is the documented shell association path, so a machine where the
//! default browser was changed still opens the right one. `cmd /C start` stays as
//! the fallback.

use std::process::{Command, Stdio};

use crate::log::{info_fields, warn_fields};

/// Open `url` in the default browser, detached from this process.
pub fn open(url: &str) -> bool {
    match launcher(url) {
        Some((program, args)) => match spawn_detached(&program, &args) {
            Ok(()) => {
                info_fields("opened the browser", &[("url", url)]);
                true
            }
            Err(err) => {
                warn_fields(
                    "could not start a browser",
                    &[("err", &err.to_string()), ("url", url)],
                );
                false
            }
        },
        None => {
            warn_fields("no known way to open a browser on this platform", &[("url", url)]);
            false
        }
    }
}

/// The platform's "open this URL" command. Split out so the choice is testable
/// without launching anything.
fn launcher(url: &str) -> Option<(String, Vec<String>)> {
    if cfg!(windows) {
        Some((
            "rundll32.exe".to_string(),
            vec!["url.dll,FileProtocolHandler".to_string(), url.to_string()],
        ))
    } else if cfg!(target_os = "macos") {
        Some(("open".to_string(), vec![url.to_string()]))
    } else {
        Some(("xdg-open".to_string(), vec![url.to_string()]))
    }
}

/// The Windows fallback, kept separate because it is the one that has to survive
/// `cmd.exe` argument re-parsing.
fn fallback_launcher(url: &str) -> (String, Vec<String>) {
    (
        "cmd.exe".to_string(),
        vec![
            "/C".to_string(),
            "start".to_string(),
            // The empty string is the window title; without it `start` treats a
            // quoted URL as the title and opens nothing.
            "".to_string(),
            url.to_string(),
        ],
    )
}

fn spawn_detached(program: &str, args: &[String]) -> std::io::Result<()> {
    let result = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_child| ());

    match result {
        Ok(()) => Ok(()),
        Err(first_error) if cfg!(windows) => {
            // `rundll32` missing is unusual but cheap to survive.
            let (program, args) = fallback_launcher(&args.last().cloned().unwrap_or_default());
            Command::new(&program)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|_child| ())
                .map_err(|_| first_error)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_gets_the_whole_url_as_one_argument() {
        let url = "http://127.0.0.1:54321/";
        let (_, args) = launcher(url).expect("this platform has a launcher");
        assert_eq!(args.last().map(String::as_str), Some(url), "the URL must not be split");
    }

    #[test]
    fn the_windows_fallback_keeps_a_title_slot_before_the_url() {
        let url = "http://127.0.0.1:1/?a=1&b=2";
        let (program, args) = fallback_launcher(url);
        assert_eq!(program, "cmd.exe");
        assert_eq!(args[1], "start");
        // Title before URL, otherwise `start` would use the URL as the title and
        // open nothing at all.
        assert_eq!(args[2], "");
        assert_eq!(args.last().map(String::as_str), Some(url));
    }
}
