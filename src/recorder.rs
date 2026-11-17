//! Turns lines from the AT port into CSV rows and chrony samples.

use std::io::{self, Read, Write};

use crate::at::{TimeLine, parse_time_line};
use crate::chrony::{Sample, SampleSink};
use crate::clock::{Clock, Stamp};
use crate::framing::LineSplitter;

pub const CSV_HEADER: &str =
    "mono_s,realtime_s,kind,tz_quarters,dst,remote_unix_s,offset_s,chrony,note,raw";

/// Writes one CSV row per line and forwards `+CTZEU` samples to chrony.
#[derive(Debug)]
pub struct Recorder<W: Write> {
    out: W,
    sink: Option<Box<dyn SampleSink>>,
}

impl<W: Write> Recorder<W> {
    pub fn new(out: W, sink: Option<Box<dyn SampleSink>>) -> Self {
        Self { out, sink }
    }

    /// # Errors
    /// Fails if the CSV output cannot be written.
    pub fn write_header(&mut self) -> io::Result<()> {
        writeln!(self.out, "{CSV_HEADER}")
    }

    /// Records one line received at `stamp`. Every line is logged, including
    /// echoes, `OK` and lines that fail to parse, so the raw stream can be re-analysed.
    ///
    /// # Errors
    /// Fails if the CSV output cannot be written. A chrony send failure is only logged.
    pub fn on_line(&mut self, line: &str, stamp: Stamp) -> io::Result<()> {
        let mut row = Row::new(stamp, line);
        match parse_time_line(line) {
            None => row.kind = "LINE",
            Some(Err(e)) => {
                row.kind = "PARSE_ERROR";
                row.note = e.to_string();
            }
            Some(Ok(t)) => {
                row.kind = t.kind();
                self.fill(&mut row, &t, stamp);
            }
        }
        row.write(&mut self.out)?;
        self.out.flush()
    }

    fn fill(&mut self, row: &mut Row, t: &TimeLine, stamp: Stamp) {
        match *t {
            TimeLine::Ctzeu {
                tz_quarters,
                dst,
                utc,
            } => {
                row.tz = Some(tz_quarters);
                row.dst = Some(dst);
                if let Some(utc) = utc {
                    let sample = Sample::from_reference(utc.unix_seconds(), stamp);
                    row.remote = Some(utc.unix_seconds());
                    row.offset = Some(sample.offset_s);
                    row.chrony = self.send(&sample);
                }
            }
            TimeLine::Ctze {
                tz_quarters, dst, ..
            } => {
                // Local time: left unconverted, the raw column keeps it.
                row.tz = Some(tz_quarters);
                row.dst = Some(dst);
            }
            TimeLine::Ctzv { tz_quarters } => row.tz = Some(tz_quarters),
            TimeLine::Cclk { time, tz_quarters } => {
                // The time field is read as UTC without applying the zone
                // (observed behaviour of the L860-GL; see docs/at-commands.md).
                // Not sent to chrony: its meaning is not confirmed.
                row.tz = tz_quarters;
                row.remote = Some(time.unix_seconds());
                row.offset = Some(Sample::from_reference(time.unix_seconds(), stamp).offset_s);
            }
        }
    }

    fn send(&mut self, sample: &Sample) -> String {
        let Some(sink) = self.sink.as_mut() else {
            return String::new();
        };
        match sink.send(sample) {
            Ok(()) => "sent".to_owned(),
            Err(e) => {
                eprintln!("ltentp: chrony send failed: {e}");
                format!("error: {e}")
            }
        }
    }
}

/// Reads the port until EOF, stamping each read as soon as it returns.
///
/// # Errors
/// Returns read errors (other than `Interrupted`) and CSV write errors.
pub fn run<R: Read, C: Clock, W: Write>(
    reader: &mut R,
    clock: &mut C,
    recorder: &mut Recorder<W>,
) -> io::Result<()> {
    let mut splitter = LineSplitter::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        // Stamp before any parsing; every line completed by this read shares it.
        let stamp = clock.now();
        for line in splitter.push(&buf[..n]) {
            recorder.on_line(&line, stamp)?;
        }
    }
}

struct Row {
    stamp: Stamp,
    kind: &'static str,
    tz: Option<i32>,
    dst: Option<u8>,
    remote: Option<i64>,
    offset: Option<f64>,
    chrony: String,
    note: String,
    raw: String,
}

impl Row {
    fn new(stamp: Stamp, raw: &str) -> Self {
        Self {
            stamp,
            kind: "",
            tz: None,
            dst: None,
            remote: None,
            offset: None,
            chrony: String::new(),
            note: String::new(),
            raw: raw.to_owned(),
        }
    }

    fn write<W: Write>(&self, out: &mut W) -> io::Result<()> {
        let opt = |v: Option<String>| v.unwrap_or_default();
        writeln!(
            out,
            "{:.6},{},{},{},{},{},{},{},{},{}",
            self.stamp.mono.as_secs_f64(),
            format_realtime(self.stamp.realtime_ns),
            self.kind,
            opt(self.tz.map(|v| v.to_string())),
            opt(self.dst.map(|v| v.to_string())),
            opt(self.remote.map(|v| v.to_string())),
            opt(self.offset.map(|v| format!("{v:.9}"))),
            csv_field(&self.chrony),
            csv_field(&self.note),
            csv_field(&self.raw),
        )
    }
}

fn format_realtime(ns: i128) -> String {
    let sec = ns.div_euclid(1_000_000_000);
    let frac = ns.rem_euclid(1_000_000_000);
    format!("{sec}.{frac:09}")
}

/// Quotes a field when it contains a comma, quote or line break (RFC 4180).
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    #[derive(Debug, Default)]
    struct Capture(Rc<RefCell<Vec<Sample>>>);

    impl SampleSink for Capture {
        fn send(&mut self, sample: &Sample) -> io::Result<()> {
            self.0.borrow_mut().push(*sample);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct Failing;

    impl SampleSink for Failing {
        fn send(&mut self, _: &Sample) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::ConnectionRefused))
        }
    }

    /// Returns 1794905484.250 s, then +1 s per call.
    struct FakeClock(i128);

    impl Clock for FakeClock {
        fn now(&mut self) -> Stamp {
            let s = Stamp {
                realtime_ns: 1_794_905_484_250_000_000 + self.0 * 1_000_000_000,
                mono: Duration::from_secs(u64::try_from(self.0).unwrap()),
            };
            self.0 += 1;
            s
        }
    }

    /// Hands out the given chunks one read at a time.
    struct Chunks(Vec<&'static [u8]>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Ok(0);
            }
            let c = self.0.remove(0);
            buf[..c.len()].copy_from_slice(c);
            Ok(c.len())
        }
    }

    fn run_chunks(chunks: Vec<&'static [u8]>, sink: Option<Box<dyn SampleSink>>) -> String {
        let mut out = Vec::new();
        let mut rec = Recorder::new(&mut out, sink);
        rec.write_header().unwrap();
        run(&mut Chunks(chunks), &mut FakeClock(0), &mut rec).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn ctzeu_goes_to_csv_and_chrony() {
        let samples = Rc::new(RefCell::new(Vec::new()));
        let csv = run_chunks(
            vec![
                b"AT+CTZR=3\r\r\nOK\r\n",
                b"+CTZEU: +36,0,\"2026/11",
                b"/17,08:51:24\"\r\n",
            ],
            Some(Box::new(Capture(samples.clone()))),
        );
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], CSV_HEADER);
        assert_eq!(
            lines[1],
            "0.000000,1794905484.250000000,LINE,,,,,,,AT+CTZR=3"
        );
        assert_eq!(lines[2], "0.000000,1794905484.250000000,LINE,,,,,,,OK");
        // Stamped at the read that completed the line (third read, +2 s).
        assert_eq!(
            lines[3],
            "2.000000,1794905486.250000000,CTZEU,36,0,1794905484,-2.250000000,sent,,\
             \"+CTZEU: +36,0,\"\"2026/11/17,08:51:24\"\"\""
        );
        assert_eq!(lines.len(), 4);
        let samples = samples.borrow();
        assert_eq!(samples.len(), 1);
        assert!((samples[0].offset_s + 2.25).abs() < 1e-9);
    }

    #[test]
    fn cclk_is_logged_but_not_sent() {
        let samples = Rc::new(RefCell::new(Vec::new()));
        let csv = run_chunks(
            vec![b"+CCLK: \"26/11/17,08:51:24+36\"\r\n\r\nOK\r\n"],
            Some(Box::new(Capture(samples.clone()))),
        );
        assert!(csv.contains(
            "0.000000,1794905484.250000000,CCLK,36,,1794905484,-0.250000000,,,\
             \"+CCLK: \"\"26/11/17,08:51:24+36\"\"\""
        ));
        assert_eq!(samples.borrow().len(), 0);
    }

    #[test]
    fn parse_error_and_garbage_are_logged() {
        let csv = run_chunks(vec![b"+CTZEU: +99,0\r\n\xff,x\r\n"], None);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[1].contains(",PARSE_ERROR,"), "{}", lines[1]);
        assert!(
            lines[1].contains("time zone out of range 99"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].ends_with(",LINE,,,,,,,\"\u{fffd},x\""),
            "{}",
            lines[2]
        );
    }

    #[test]
    fn chrony_failure_is_recorded_and_run_continues() {
        let csv = run_chunks(
            vec![b"+CTZEU: +36,0,\"2026/11/17,08:51:24\"\r\n", b"OK\r\n"],
            Some(Box::new(Failing)),
        );
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[1].contains(",error: "), "{}", lines[1]);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn ctzeu_without_time_sends_nothing() {
        let samples = Rc::new(RefCell::new(Vec::new()));
        let csv = run_chunks(
            vec![b"+CTZEU: +36,0\r\n"],
            Some(Box::new(Capture(samples.clone()))),
        );
        assert!(csv.contains(",CTZEU,36,0,,,,,"), "{csv}");
        assert_eq!(samples.borrow().len(), 0);
    }

    #[test]
    fn realtime_formatting() {
        assert_eq!(format_realtime(1_500_000_000), "1.500000000");
        assert_eq!(format_realtime(-1), "-1.999999999");
    }
}
