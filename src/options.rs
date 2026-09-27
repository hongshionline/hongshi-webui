//! Command line parsing.
//!
//! The shell is launched by double-click far more often than from a terminal, so
//! **every option has a default and none of them is required**. The flags exist
//! for the two people who need them: whoever is developing a page, and whoever is
//! debugging why the browser did not open.
//!
//! Parsing is a pure function of the argument list — no `std::env` inside — so the
//! contract can be tested without spawning a process.

use std::path::PathBuf;

/// Everything the shell was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Loopback port; 0 means "let the OS pick", which is the default.
    pub port: u16,
    /// Serve the UI from here instead of the embedded copy.
    pub web_dir: Option<PathBuf>,
    /// Whether to hand the URL to the default browser.
    pub open_browser: bool,
    /// Print DEBUG lines.
    pub verbose: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            port: 0,
            web_dir: None,
            open_browser: true,
            verbose: false,
        }
    }
}

/// The two shapes a successful parse can have: run, or print and exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    Print(String),
}

/// Parse the arguments *after* the program name.
pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut options = Options::default();
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;

        // `--flag=value` and `--flag value` are both accepted; the first form is
        // what people paste in from shell history.
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (arg, None),
        };

        let mut take_value = |what: &str| -> Result<String, String> {
            match inline.clone() {
                Some(value) => Ok(value),
                None => {
                    if index < args.len() {
                        let value = args[index].clone();
                        index += 1;
                        Ok(value)
                    } else {
                        Err(format!("{what} needs a value (for example `{what} 3080`)"))
                    }
                }
            }
        };

        match name {
            "-h" | "--help" => return Ok(Parsed::Print(usage())),
            "-v" | "--version" => {
                return Ok(Parsed::Print(format!(
                    "hongshi shell {} v{} (hongshi-shell crate {})",
                    crate::CHANNEL,
                    crate::VERSION,
                    env!("CARGO_PKG_VERSION")
                )));
            }
            "--port" => {
                let value = take_value("--port")?;
                options.port = value
                    .parse::<u16>()
                    .map_err(|_| format!("--port expects a number from 0 to 65535, got `{value}`"))?;
            }
            "--web-dir" => {
                options.web_dir = Some(PathBuf::from(take_value("--web-dir")?));
            }
            "--no-browser" => options.open_browser = false,
            "--verbose" => options.verbose = true,
            other => {
                return Err(format!("unknown option `{other}`"));
            }
        }
    }

    Ok(Parsed::Run(options))
}

pub fn usage() -> String {
    format!(
        "Usage: {} [OPTIONS]\n\n\
         Starts a local server for the hongshi WebUI shell and opens it in your browser.\n\
         No option is required: double-clicking the binary is the intended way to run it.\n\n\
         Options:\n  \
         --port <N>      Loopback port to listen on [default: 0, the OS picks a free one]\n  \
         --web-dir <DIR> Serve the UI from DIR instead of the copy inside the binary\n  \
         --no-browser    Print the URL instead of opening a browser\n  \
         --verbose       Print DEBUG lines\n  \
         -v, --version   Print version information\n  \
         -h, --help      Print this help\n\n\
         The URL is the whole address: it needs no token, and reloading it works.\n",
        crate::PROGRAM
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn run(list: &[&str]) -> Options {
        match parse(&args(list)).expect("parse should succeed") {
            Parsed::Run(options) => options,
            Parsed::Print(text) => panic!("expected Run, got Print: {text}"),
        }
    }

    #[test]
    fn no_arguments_means_run_with_defaults() {
        assert_eq!(
            run(&[]),
            Options {
                port: 0,
                web_dir: None,
                open_browser: true,
                verbose: false,
            }
        );
    }

    #[test]
    fn long_options_accept_both_spellings() {
        assert_eq!(run(&["--port", "3080"]).port, 3080);
        assert_eq!(run(&["--port=3080"]).port, 3080);
        assert_eq!(run(&["--port=0"]).port, 0);
        assert_eq!(
            run(&["--web-dir", "web"]).web_dir,
            Some(PathBuf::from("web"))
        );
        assert_eq!(
            run(&["--web-dir=shell/web"]).web_dir,
            Some(PathBuf::from("shell/web"))
        );
    }

    #[test]
    fn flags_combine_in_any_order() {
        let options = run(&["--verbose", "--no-browser", "--port", "1234"]);
        assert!(options.verbose);
        assert!(!options.open_browser);
        assert_eq!(options.port, 1234);
    }

    #[test]
    fn help_and_version_print_instead_of_running() {
        assert!(matches!(parse(&args(&["--help"])), Ok(Parsed::Print(_))));
        assert!(matches!(parse(&args(&["-h"])), Ok(Parsed::Print(_))));
        assert!(matches!(parse(&args(&["-v"])), Ok(Parsed::Print(_))));
        assert!(matches!(parse(&args(&["--version"])), Ok(Parsed::Print(_))));
    }

    #[test]
    fn a_missing_or_bad_value_is_a_usage_error() {
        for bad in [
            vec!["--port"],
            vec!["--web-dir"],
            vec!["--port", "eighty"],
            vec!["--port", "65536"],
            vec!["--port", "-1"],
            vec!["--nope"],
        ] {
            assert!(parse(&args(&bad)).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn usage_text_documents_every_option() {
        let usage = usage();
        for option in ["--port", "--web-dir", "--no-browser", "--verbose", "--help", "--version"] {
            assert!(usage.contains(option), "{option} is undocumented");
        }
        assert!(usage.starts_with("Usage: hongshi "), "{usage}");
    }
}
