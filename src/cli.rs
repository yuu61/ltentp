//! Command-line options (standard library only).

use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "\
usage: ltentp --device <path> [--chrony-sock <path>] [--csv <path>]
              [--cclk-interval <seconds> [--cclk-step <ms>]]

  --device <path>           AT port, e.g. /dev/wwan0at1 (opened as a plain file)
  --chrony-sock <path>      send +CTZEU samples to chrony's `refclock SOCK` at this path
  --csv <path>              append CSV rows to this file (default: stdout)
  --cclk-interval <seconds> every N seconds, time one tick of the modem clock by
                            polling AT+CCLK? (default: 0 = off)
  --cclk-step <ms>          spacing of those queries (default: 10)
  -h, --help                show this help";

/// Default spacing of `AT+CCLK?` queries within a burst.
pub const DEFAULT_CCLK_STEP: Duration = Duration::from_millis(10);

/// Parsed options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub device: PathBuf,
    pub chrony_sock: Option<PathBuf>,
    pub csv: Option<PathBuf>,
    pub cclk_interval: Option<Duration>,
    pub cclk_step: Duration,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Help,
}

/// Parses arguments (without the program name). Unknown options are errors.
///
/// # Errors
/// Returns a message for unknown, repeated or missing options and bad values.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut device = None;
    let mut chrony_sock = None;
    let mut csv = None;
    let mut cclk_interval = None;
    let mut cclk_step = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--device" => set_once(&mut device, PathBuf::from(value("--device")?), &arg)?,
            "--chrony-sock" => {
                set_once(
                    &mut chrony_sock,
                    PathBuf::from(value("--chrony-sock")?),
                    &arg,
                )?;
            }
            "--csv" => set_once(&mut csv, PathBuf::from(value("--csv")?), &arg)?,
            "--cclk-interval" => {
                let v = value("--cclk-interval")?;
                let secs: u64 = v.parse().map_err(|_| {
                    format!("--cclk-interval: not a whole number of seconds: {v:?}")
                })?;
                set_once(&mut cclk_interval, secs, &arg)?;
            }
            "--cclk-step" => {
                let v = value("--cclk-step")?;
                let ms: u64 = v
                    .parse()
                    .ok()
                    .filter(|ms| (1..=1_000).contains(ms))
                    .ok_or_else(|| format!("--cclk-step: not 1..=1000 milliseconds: {v:?}"))?;
                set_once(&mut cclk_step, Duration::from_millis(ms), &arg)?;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(Command::Run(Options {
        device: device.ok_or("--device is required")?,
        chrony_sock,
        csv,
        cclk_interval: cclk_interval.filter(|&s| s > 0).map(Duration::from_secs),
        cclk_step: cclk_step.unwrap_or(DEFAULT_CCLK_STEP),
    }))
}

fn set_once<T>(slot: &mut Option<T>, v: T, name: &str) -> Result<(), String> {
    if slot.replace(v).is_some() {
        return Err(format!("{name} given more than once"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn minimal_and_full() {
        assert_eq!(
            p(&["--device", "/dev/wwan0at1"]),
            Ok(Command::Run(Options {
                device: "/dev/wwan0at1".into(),
                chrony_sock: None,
                csv: None,
                cclk_interval: None,
                cclk_step: DEFAULT_CCLK_STEP,
            }))
        );
        assert_eq!(
            p(&[
                "--device",
                "/dev/wwan0at1",
                "--chrony-sock",
                "/run/chrony/nitz.sock",
                "--csv",
                "nitz.csv",
                "--cclk-interval",
                "60",
                "--cclk-step",
                "5",
            ]),
            Ok(Command::Run(Options {
                device: "/dev/wwan0at1".into(),
                chrony_sock: Some("/run/chrony/nitz.sock".into()),
                csv: Some("nitz.csv".into()),
                cclk_interval: Some(Duration::from_secs(60)),
                cclk_step: Duration::from_millis(5),
            }))
        );
    }

    #[test]
    fn zero_interval_is_off() {
        let Ok(Command::Run(o)) = p(&["--device", "x", "--cclk-interval", "0"]) else {
            panic!()
        };
        assert_eq!(o.cclk_interval, None);
    }

    #[test]
    fn errors() {
        assert!(p(&[]).is_err());
        assert!(p(&["--device"]).is_err());
        assert!(p(&["--device", "x", "--devcie", "y"]).is_err());
        assert!(p(&["--device", "x", "--device", "y"]).is_err());
        assert!(p(&["--device", "x", "--cclk-interval", "-1"]).is_err());
        assert!(p(&["--device", "x", "--cclk-interval", "1.5"]).is_err());
        assert!(p(&["--device", "x", "--cclk-step", "0"]).is_err());
        assert!(p(&["--device", "x", "--cclk-step", "1001"]).is_err());
        assert!(p(&["--device", "x", "--cclk-step", "5", "--cclk-step", "5"]).is_err());
    }

    #[test]
    fn help() {
        assert_eq!(p(&["--device", "x", "--help"]), Ok(Command::Help));
    }
}
