use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use ltentp::chrony::{ChronySocket, SampleSink};
use ltentp::cli::{self, Command, Options};
use ltentp::clock::SystemClock;
use ltentp::recorder::{Recorder, run};

/// Enables `+CTZEU` (zone + UTC) reports. Sent on every start: the setting is
/// not assumed to survive a modem reset or FCC unlock.
const INIT_COMMAND: &[u8] = b"AT+CTZR=3\r";
const CCLK_COMMAND: &[u8] = b"AT+CCLK?\r";

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Run(o)) => o,
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("ltentp: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match start(&options) {
        Ok(()) => {
            eprintln!("ltentp: {} closed (EOF)", options.device.display());
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("ltentp: {e}");
            ExitCode::FAILURE
        }
    }
}

fn start(o: &Options) -> io::Result<()> {
    // /dev/wwan*at* is a WWAN port character device, not a TTY: no termios setup.
    let mut port = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&o.device)
        .map_err(|e| with_path(&e, &o.device))?;

    let sink: Option<Box<dyn SampleSink>> = match &o.chrony_sock {
        Some(path) => Some(Box::new(ChronySocket::new(path.clone())?)),
        None => None,
    };

    let out: Box<dyn Write> = match &o.csv {
        Some(path) => Box::new(open_csv(path)?),
        None => Box::new(io::stdout()),
    };
    let new_file = match &o.csv {
        Some(path) => std::fs::metadata(path)?.len() == 0,
        None => true,
    };
    let mut recorder = Recorder::new(DebugWrite(out), sink);
    if new_file {
        recorder.write_header()?;
    }

    port.write_all(INIT_COMMAND)?;

    if let Some(interval) = o.cclk_interval {
        let writer = port.try_clone()?;
        thread::spawn(move || poll_cclk(writer, interval));
    }

    run(&mut port, &mut SystemClock::new(), &mut recorder)
}

fn poll_cclk(mut writer: File, interval: Duration) {
    loop {
        thread::sleep(interval);
        if let Err(e) = writer.write_all(CCLK_COMMAND) {
            eprintln!("ltentp: AT+CCLK? polling stopped: {e}");
            return;
        }
    }
}

fn open_csv(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| with_path(&e, path))
}

fn with_path(e: &io::Error, path: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// `Box<dyn Write>` with a `Debug` impl, so `Recorder` stays `Debug`.
struct DebugWrite(Box<dyn Write>);

impl std::fmt::Debug for DebugWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DebugWrite")
    }
}

impl Write for DebugWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
