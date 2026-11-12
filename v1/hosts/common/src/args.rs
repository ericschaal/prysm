//! Command-line options. Process options live here, never in the settings file.
use prysm_web_portal::Access;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

const DEFAULT_LISTEN: &str = "0.0.0.0:8787";

const OPTIONS: &str = "Options:
  --listen ADDR             Portal address; port 0 picks one
                            [default: 0.0.0.0:8787, every interface]
  --allow-origin ORIGIN     Also let browser pages from ORIGIN (such as
                            http://192.168.1.30:3000) use the API, repeatable;
                            'null' allows pages without an origin, such as the
                            Prysm TV app (and sandboxed frames on any site);
                            '*' allows every origin
                            [default: only the portal's own pages]
  --no-portal               Serve no web portal
  --start                   Request processed lighting at launch (else idle)
  --log-file PATH           Append log lines to PATH instead of stderr
                            [default: stderr; none while a full-screen
                            terminal surface uses the terminal]
  -h, --help                Print this help

The portal has no password: anyone who can reach ADDR can control the
lights. Browser pages from other sites are refused unless allowed above.
SIGINT or SIGTERM stops lighting, idles the output and exits. Status goes to
stderr. RUST_LOG filters log lines by target [default: warn,prysm=info].";

pub(crate) fn usage(name: &str, about: &str) -> String {
    format!("usage: {name} [options] <settings.json>\n{about}\n\n{OPTIONS}")
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Run(Options),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub settings: PathBuf,
    /// `None` under `--no-portal`.
    pub portal: Option<PortalOptions>,
    pub start: bool,
    pub log_file: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PortalOptions {
    pub listen: SocketAddr,
    /// Validated by `WebPortal`.
    pub access: Access,
}

/// Parse arguments after the program name.
///
/// # Errors
/// A message naming the offending argument.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
    let mut args = args.into_iter();
    let mut settings = None;
    let mut listen = None;
    let mut origins = Vec::new();
    let mut any_origin = false;
    let mut no_portal = false;
    let mut start = false;
    let mut log_file = None;
    while let Some(arg) = args.next() {
        let Some(flag) = arg.to_str().filter(|arg| arg.starts_with('-')) else {
            if arg.as_encoded_bytes().starts_with(b"-") {
                return Err(format!("unknown option {}", arg.to_string_lossy()));
            }
            if settings.replace(PathBuf::from(arg)).is_some() {
                return Err("expected exactly one settings file".into());
            }
            continue;
        };
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--listen" => {
                let address = value()?;
                let address = address
                    .to_str()
                    .and_then(|address| address.parse().ok())
                    .ok_or_else(|| {
                        format!("--listen expects IP:PORT, got {}", address.display())
                    })?;
                once(&mut listen, address, flag)?;
            }
            "--allow-origin" => {
                let origin = value()?
                    .into_string()
                    .map_err(|_| "--allow-origin must be UTF-8".to_owned())?;
                if origin == "*" {
                    switch(&mut any_origin, "--allow-origin '*'")?;
                } else {
                    origins.push(origin);
                }
            }
            "--no-portal" => switch(&mut no_portal, flag)?,
            "--start" => switch(&mut start, flag)?,
            "--log-file" => once(&mut log_file, PathBuf::from(value()?), flag)?,
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    let settings = settings.ok_or("expected a settings file path")?;
    let portal = if no_portal {
        if listen.is_some() || !origins.is_empty() || any_origin {
            return Err("--no-portal cannot be combined with portal options".into());
        }
        None
    } else {
        let access = match (any_origin, origins.is_empty()) {
            (false, true) => Access::SameOrigin,
            (false, false) => Access::Origins(origins),
            (true, true) => Access::AnyOrigin,
            (true, false) => {
                return Err("--allow-origin '*' cannot be combined with other origins".into());
            }
        };
        Some(PortalOptions {
            listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.parse().expect("valid default")),
            access,
        })
    };
    Ok(Parsed::Run(Options {
        settings,
        portal,
        start,
        log_file,
    }))
}

fn once<T>(slot: &mut Option<T>, value: T, flag: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{flag} given more than once"));
    }
    Ok(())
}

fn switch(on: &mut bool, flag: &str) -> Result<(), String> {
    if std::mem::replace(on, true) {
        return Err(format!("{flag} given more than once"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Parsed, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn defaults_to_an_idle_host_with_a_same_origin_portal_on_every_interface() {
        let Parsed::Run(options) = run(&["settings.json"]).unwrap() else {
            panic!("expected options");
        };
        assert_eq!(options.settings, PathBuf::from("settings.json"));
        assert!(!options.start);
        assert_eq!(options.log_file, None);
        assert_eq!(
            options.portal,
            Some(PortalOptions {
                listen: "0.0.0.0:8787".parse().unwrap(),
                access: Access::SameOrigin,
            })
        );
    }

    #[test]
    fn reads_every_option_in_any_order() {
        let parsed = run(&[
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
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Parsed::Run(Options {
                settings: "s.json".into(),
                start: true,
                log_file: Some("prysm.log".into()),
                portal: Some(PortalOptions {
                    listen: "[::1]:0".parse().unwrap(),
                    access: Access::Origins(vec![
                        "http://192.168.1.30:3000".into(),
                        "app://tv".into(),
                        "null".into()
                    ]),
                }),
            })
        );
        let Parsed::Run(options) = run(&["s.json", "--allow-origin", "*"]).unwrap() else {
            panic!("expected options");
        };
        assert_eq!(options.portal.unwrap().access, Access::AnyOrigin);
        let Parsed::Run(options) = run(&["s.json", "--no-portal"]).unwrap() else {
            panic!("expected options");
        };
        assert_eq!(options.portal, None);
    }

    #[test]
    fn help_wins_over_everything_else() {
        assert_eq!(run(&["--help"]).unwrap(), Parsed::Help);
        assert_eq!(run(&["s.json", "-h", "--bogus"]).unwrap(), Parsed::Help);
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
        ] {
            assert!(run(args).is_err(), "{args:?} should be rejected");
        }
    }
}
