//! End-to-end test of the binary without hardware: a FIFO stands in for the AT
//! port (it also echoes ltentp's own `AT+CTZR=3`, like a modem echo) and a bound
//! datagram socket stands in for chronyd's `refclock SOCK`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 2026-11-17T08:51:24Z
const NITZ_UNIX: i64 = 1_794_905_484;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ctzeu_reaches_csv_and_chrony_socket() {
    let dir = TempDir(std::env::temp_dir().join(format!("ltentp-e2e-{}", std::process::id())));
    fs::create_dir_all(&dir.0).unwrap();
    let fifo = dir.0.join("at");
    let sock_path = dir.0.join("chrony.sock");
    let csv = dir.0.join("out.csv");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );

    // Bound before ltentp starts, as chronyd would be.
    let chronyd = UnixDatagram::bind(&sock_path).unwrap();
    chronyd
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_ltentp"))
        .arg("--device")
        .arg(&fifo)
        .arg("--chrony-sock")
        .arg(&sock_path)
        .arg("--csv")
        .arg(&csv)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // Read+write so the open never blocks, even if ltentp exits before opening
    // the FIFO (then the recv below times out instead of hanging the test run).
    // Only ltentp reads from it.
    let mut modem = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)
        .unwrap();
    modem
        .write_all(b"\r\nOK\r\n+CTZEU: +36,0,\"2026/11/17,08:51:24\"\r\n")
        .unwrap();

    let mut buf = [0u8; 64];
    let received = chronyd.recv(&mut buf);
    // Let the CSV row land (it is written before the send, but be safe), then stop.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && !fs::read_to_string(&csv)
            .unwrap_or_default()
            .contains("CTZEU")
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    let n = received.expect("no sample reached the fake chronyd");
    assert_eq!(n, 40);
    let i64_at = |o: usize| i64::from_ne_bytes(buf[o..o + 8].try_into().unwrap());
    let i32_at = |o: usize| i32::from_ne_bytes(buf[o..o + 4].try_into().unwrap());
    let offset = f64::from_ne_bytes(buf[16..24].try_into().unwrap());
    assert_eq!(i32_at(36), 0x534f_434b);
    // offset = reference - system time at tv.
    #[allow(clippy::cast_precision_loss)]
    let expected = NITZ_UNIX as f64 - (i64_at(0) as f64 + i64_at(8) as f64 / 1e6);
    assert!((offset - expected).abs() < 1e-5, "{offset} vs {expected}");

    let text = fs::read_to_string(&csv).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("mono_s,realtime_s,kind,"), "{text}");
    assert!(text.contains(",LINE,,,,,,,AT+CTZR=3"), "{text}");
    assert!(text.contains(",LINE,,,,,,,OK"), "{text}");
    assert!(
        text.contains(&format!(",CTZEU,36,0,{NITZ_UNIX},")),
        "{text}"
    );
    assert!(text.contains(",sent,"), "{text}");
}

#[test]
fn unknown_option_exits_2() {
    let status = Command::new(env!("CARGO_BIN_EXE_ltentp"))
        .args(["--devcie", "x"])
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}
