//! The main loop: lines read from the AT port go to the recorder, and
//! `AT+CCLK?` queries go out on the poller's schedule.
//!
//! A reader thread blocks in `read` and stamps each chunk the moment it
//! returns; the main thread handles chunks and timers, so a query can be sent
//! while no data is arriving.

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::at::TimeLine;
use crate::clock::{Clock, Stamp};
use crate::edge::CclkPoller;
use crate::framing::LineSplitter;
use crate::recorder::Recorder;

pub const CCLK_QUERY: &[u8] = b"AT+CCLK?\r";

/// Recorder, line framing, the optional `AT+CCLK?` poller and the port's write side.
#[derive(Debug)]
pub struct Session<W: Write, P: Write> {
    recorder: Recorder<W>,
    splitter: LineSplitter,
    poller: Option<CclkPoller>,
    port: P,
}

impl<W: Write, P: Write> Session<W, P> {
    pub fn new(recorder: Recorder<W>, poller: Option<CclkPoller>, port: P) -> Self {
        Self {
            recorder,
            splitter: LineSplitter::new(),
            poller,
            port,
        }
    }

    /// Handles one read's bytes, all stamped at `stamp`.
    ///
    /// # Errors
    /// Fails if the CSV output cannot be written.
    pub fn on_chunk(&mut self, bytes: &[u8], stamp: Stamp) -> io::Result<()> {
        for line in self.splitter.push(bytes) {
            let parsed = self.recorder.on_line(&line, stamp)?;
            let Some(poller) = self.poller.as_mut() else {
                continue;
            };
            match parsed {
                Some(t @ TimeLine::Cclk { tz_quarters, .. }) => {
                    let utc = t.cclk_utc().unwrap_or_default();
                    if let Some(edge) = poller.on_cclk(utc, stamp) {
                        self.recorder.on_edge(&edge, tz_quarters)?;
                    }
                }
                // An unparsable +CCLK reply fails the query just like ERROR.
                _ if is_error(&line) || line.trim_start().starts_with("+CCLK:") => {
                    poller.on_error();
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Sends `AT+CCLK?` if the poller says so; `now` is taken as the send time.
    ///
    /// # Errors
    /// Fails if the port cannot be written.
    pub fn on_timer(&mut self, now: Stamp) -> io::Result<()> {
        if let Some(poller) = self.poller.as_mut() {
            if poller.on_timer(now) {
                self.port.write_all(CCLK_QUERY)?;
            }
        }
        Ok(())
    }

    /// Monotonic time of the next timer work, or `None` to wait for data only.
    #[must_use]
    pub fn deadline(&self) -> Option<Duration> {
        self.poller.as_ref().map(CclkPoller::deadline)
    }
}

fn is_error(line: &str) -> bool {
    let l = line.trim();
    l == "ERROR" || l.starts_with("+CME ERROR")
}

enum Event {
    Data(Stamp, Vec<u8>),
    Eof,
    Error(io::Error),
}

/// Runs until the port reaches EOF (`Ok`) or fails. `reader` is moved to a
/// reader thread with a clone of `clock`; both clones must share one origin
/// for monotonic times to compare.
///
/// # Errors
/// Returns read errors (other than `Interrupted`), and port or CSV write errors.
pub fn run<R, C, W, P>(reader: R, mut clock: C, session: &mut Session<W, P>) -> io::Result<()>
where
    R: Read + Send + 'static,
    C: Clock + Clone + Send + 'static,
    W: Write,
    P: Write,
{
    let (tx, rx) = mpsc::channel();
    let reader_clock = clock.clone();
    thread::spawn(move || read_loop(reader, reader_clock, &tx));
    loop {
        session.on_timer(clock.now())?;
        let event = match session.deadline() {
            None => rx.recv().map_err(|_| reader_gone())?,
            Some(at) => match rx.recv_timeout(at.saturating_sub(clock.now().mono)) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(reader_gone()),
            },
        };
        match event {
            Event::Data(stamp, bytes) => session.on_chunk(&bytes, stamp)?,
            Event::Eof => return Ok(()),
            Event::Error(e) => return Err(e),
        }
    }
}

#[allow(clippy::needless_pass_by_value)] // owned by the thread
fn read_loop<R: Read, C: Clock>(mut reader: R, mut clock: C, tx: &Sender<Event>) {
    let mut buf = [0u8; 1024];
    loop {
        let event = match reader.read(&mut buf) {
            Ok(0) => Event::Eof,
            Ok(n) => {
                // Stamp before anything else; every line completed by this read shares it.
                let stamp = clock.now();
                Event::Data(stamp, buf[..n].to_vec())
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => Event::Error(e),
        };
        let more = matches!(event, Event::Data(..));
        if tx.send(event).is_err() || !more {
            return;
        }
    }
}

fn reader_gone() -> io::Error {
    io::Error::other("reader thread ended unexpectedly")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::CSV_HEADER;

    /// A stamp at `ms`; the system clock reads 01:10:19 UTC (10:10:19 JST) + `ms`.
    fn at(ms: u64) -> Stamp {
        Stamp {
            realtime_ns: 1_794_964_219_000_000_000 + i128::from(ms) * 1_000_000,
            mono: Duration::from_millis(ms),
        }
    }

    /// `+CCLK` reply for a modem clock that is 10 ms ahead of the system clock.
    fn reply(sample_ms: u64) -> Vec<u8> {
        let s = (sample_ms + 10) / 1_000;
        format!(
            "\r\n+CCLK: \"26/11/18,10:10:{:02}+36\"\r\n\r\nOK\r\n",
            19 + s
        )
        .into_bytes()
    }

    #[test]
    fn polling_records_queries_replies_and_one_edge() {
        let mut out = Vec::new();
        let mut rec = Recorder::new(&mut out, None);
        rec.write_header().unwrap();
        let poller = CclkPoller::new(Duration::from_secs(10), Duration::from_millis(10));
        let mut port = Vec::new();
        let mut s = Session::new(rec, Some(poller), &mut port);
        let mut t = 0;
        let mut queries = 0;
        while t < 1_500 {
            s.on_timer(at(t)).unwrap();
            let sent = s.port.len() / CCLK_QUERY.len();
            if sent > queries {
                queries = sent;
                // Sampled at t + 2, read at t + 4.
                s.on_chunk(&reply(t + 2), at(t + 4)).unwrap();
            }
            t = u64::try_from(s.deadline().unwrap().as_millis())
                .unwrap()
                .max(t + 1);
        }
        drop(s);
        assert!(port.starts_with(CCLK_QUERY));
        let csv = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], CSV_HEADER);
        let edges: Vec<&&str> = lines.iter().filter(|l| l.contains(",CCLK_EDGE,")).collect();
        assert_eq!(edges.len(), 1, "{csv}");
        // Tick at system 0.990 s, bracketed by the queries around it.
        let f: Vec<&str> = edges[0].split(',').collect();
        assert_eq!(f[2], "CCLK_EDGE");
        assert_eq!(f[3], "36");
        assert_eq!(f[5], "1794964220");
        let offset: f64 = f[6].parse().unwrap();
        assert!((offset - 0.010).abs() <= 0.007, "{offset}");
        assert!(f[8].starts_with("halfwidth_s=0.00"), "{}", f[8]);
        // Every reply line is still logged.
        assert_eq!(
            lines.iter().filter(|l| l.contains(",CCLK,36,")).count(),
            queries
        );
    }

    #[test]
    fn error_reply_releases_the_query() {
        let mut out = Vec::new();
        let rec = Recorder::new(&mut out, None);
        let poller = CclkPoller::new(Duration::from_secs(10), Duration::from_millis(10));
        let mut s = Session::new(rec, Some(poller), Vec::new());
        s.on_timer(at(0)).unwrap();
        assert_eq!(s.deadline(), Some(Duration::from_millis(500)));
        s.on_chunk(b"\r\n+CME ERROR: unknown\r\n", at(3)).unwrap();
        assert_eq!(s.deadline(), Some(Duration::from_millis(10)));
    }

    #[test]
    fn without_poller_nothing_is_sent() {
        let rec = Recorder::new(Vec::new(), None);
        let mut s = Session::new(rec, None, Vec::new());
        s.on_timer(at(0)).unwrap();
        assert_eq!(s.deadline(), None);
        assert_eq!(s.port, Vec::<u8>::new());
    }
}
