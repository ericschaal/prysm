//! Command-line options. Process options live here, never in the settings file.
use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches, Parser};
use prysm_web_portal::Access;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

const AFTER_HELP: &str = "\
The portal has no password: anyone who can reach ADDR can control the lights.
Browser pages from other sites are refused unless allowed with --allow-origin.
SIGINT or SIGTERM stops lighting, idles the output and exits. Status goes to
stderr. RUST_LOG filters log lines by target [default: warn,prysm=info].";

#[derive(Debug, PartialEq, Eq, Parser)]
#[command(version, after_help = AFTER_HELP)]
pub(crate) struct Options {
    /// Settings file
    #[arg(value_name = "settings.json")]
    pub settings: PathBuf,
    /// Portal address; port 0 picks one
    #[arg(long, value_name = "ADDR", default_value = "0.0.0.0:8787")]
    pub listen: SocketAddr,
    /// Also allow browser pages from ORIGIN (repeatable; 'null': no origin, as the TV app; '*': any)
    #[arg(long, value_name = "ORIGIN")]
    pub allow_origin: Vec<String>,
    /// Also serve Prometheus metrics at /metrics on the portal
    #[arg(long)]
    pub metrics: bool,
    /// Serve no web portal
    #[arg(long, conflicts_with_all = ["listen", "allow_origin", "metrics"])]
    pub no_portal: bool,
    /// Request processed lighting at launch (else idle)
    #[arg(long)]
    pub start: bool,
    /// Append log lines to PATH instead of stderr
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<PathBuf>,
    /// Only validate the settings file for this host's adapters, then exit
    #[arg(long, conflicts_with_all = ["listen", "allow_origin", "metrics", "no_portal", "start", "log_file"])]
    pub check_settings: bool,
}

impl Options {
    /// Who may use the API besides the portal's own pages; validated by `WebPortal`.
    #[must_use]
    pub(crate) fn access(&self) -> Access {
        match self.allow_origin.as_slice() {
            [] => Access::SameOrigin,
            [any] if any == "*" => Access::AnyOrigin,
            origins => Access::Origins(origins.to_vec()),
        }
    }
}

/// Parse a full command line (program name first) for the program `name`.
///
/// # Errors
/// Help, version or a usage error, to be shown with [`clap::Error::exit`].
pub(crate) fn parse<I, T>(
    name: &'static str,
    about: &'static str,
    args: I,
) -> Result<Options, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut command = Options::command().name(name).bin_name(name).about(about);
    let matches = command.try_get_matches_from_mut(args)?;
    let options =
        Options::from_arg_matches(&matches).map_err(|error| error.format(&mut command))?;
    if options.allow_origin.len() > 1 && options.allow_origin.iter().any(|origin| origin == "*") {
        return Err(command.error(
            ErrorKind::ArgumentConflict,
            "--allow-origin '*' cannot be combined with other origins",
        ));
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Options, clap::Error> {
        parse("prysm", "about", ["prysm"].iter().chain(args))
    }

    #[test]
    fn defaults_to_an_idle_host_with_a_same_origin_portal_on_every_interface() {
        let options = run(&["settings.json"]).unwrap();
        assert_eq!(options.settings, PathBuf::from("settings.json"));
        assert!(!options.start && !options.no_portal && !options.metrics);
        assert!(!options.check_settings);
        assert_eq!(options.log_file, None);
        assert_eq!(options.listen, "0.0.0.0:8787".parse().unwrap());
        assert_eq!(options.access(), Access::SameOrigin);
    }

    #[test]
    fn reads_every_option_in_any_order() {
        let options = run(&[
            "--start",
            "--listen",
            "[::1]:0",
            "s.json",
            "--allow-origin",
            "http://192.168.1.30:3000",
            "--allow-origin",
            "app://tv",
            "--allow-origin",
            "null",
            "--log-file",
            "prysm.log",
            "--metrics",
        ])
        .unwrap();
        assert_eq!(options.settings, PathBuf::from("s.json"));
        assert!(options.start && options.metrics && !options.no_portal);
        assert_eq!(options.log_file, Some("prysm.log".into()));
        assert_eq!(options.listen, "[::1]:0".parse().unwrap());
        assert_eq!(
            options.access(),
            Access::Origins(vec![
                "http://192.168.1.30:3000".into(),
                "app://tv".into(),
                "null".into()
            ])
        );
        let options = run(&["s.json", "--allow-origin", "*"]).unwrap();
        assert_eq!(options.access(), Access::AnyOrigin);
        assert!(run(&["s.json", "--no-portal"]).unwrap().no_portal);
        let options = run(&["--check-settings", "s.json"]).unwrap();
        assert!(options.check_settings);
        assert_eq!(options.settings, PathBuf::from("s.json"));
    }

    #[test]
    fn help_and_version_win_over_everything_else() {
        assert_eq!(run(&["--help"]).unwrap_err().kind(), ErrorKind::DisplayHelp);
        assert_eq!(
            run(&["s.json", "-h", "--bogus"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        assert_eq!(
            run(&["--version"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn rejects_bad_arguments() {
        for args in [
            &[][..],
            &["--start"],
            &["a.json", "b.json"],
            &["s.json", "--start", "--start"],
            &["s.json", "--no-portal", "--no-portal"],
            &["s.json", "--terminal"],
            &["s.json", "--listen"],
            &["s.json", "--listen", "localhost:80"],
            &[
                "s.json",
                "--listen",
                "127.0.0.1:1",
                "--listen",
                "127.0.0.1:2",
            ],
            &["s.json", "--no-portal", "--listen", "127.0.0.1:1"],
            &["s.json", "--no-portal", "--allow-origin", "*"],
            &["s.json", "--no-portal", "--metrics"],
            &["s.json", "--metrics", "--metrics"],
            &["s.json", "--allow-origin"],
            &["s.json", "--log-file"],
            &["s.json", "--log-file", "a.log", "--log-file", "b.log"],
            &["s.json", "--allow-origin", "*", "--allow-origin", "*"],
            &[
                "s.json",
                "--allow-origin",
                "*",
                "--allow-origin",
                "http://a",
            ],
            &["s.json", "--token-file", "t"],
            &["s.json", "--allow-authority", "localhost:8787"],
            &["--check-settings"],
            &["s.json", "--check-settings", "--check-settings"],
            &["s.json", "--check-settings", "--start"],
            &["s.json", "--check-settings", "--no-portal"],
            &["s.json", "--check-settings", "--listen", "127.0.0.1:1"],
            &["s.json", "--check-settings", "--log-file", "a.log"],
        ] {
            let error = run(args).expect_err(&format!("{args:?} should be rejected"));
            assert_eq!(error.exit_code(), 2, "{args:?}");
        }
    }
}
