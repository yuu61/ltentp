//! Samples for chrony's `refclock SOCK` driver.
//!
//! chronyd owns and binds the socket path; this side only sends datagrams to it.
//! Layout of `struct sock_sample` (chrony `refclock_sock.c`, 64-bit Linux):
//!
//! | offset | field                 | type   |
//! |--------|-----------------------|--------|
//! | 0      | `tv.tv_sec`           | i64    |
//! | 8      | `tv.tv_usec`          | i64    |
//! | 16     | `offset`              | f64    |
//! | 24     | `pulse`               | i32    |
//! | 28     | `leap`                | i32    |
//! | 32     | `_pad`                | i32    |
//! | 36     | `magic` (0x534f434b)  | i32    |
//!
//! All fields are native-endian. `offset` is true time minus system time at `tv`.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("sock_sample layout here assumes a 64-bit time_t/timeval");

use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

use crate::clock::Stamp;

pub const SOCK_MAGIC: i32 = 0x534f_434b;
pub const SAMPLE_LEN: usize = 40;

/// One measurement: at system time `stamp`, true time was `stamp + offset_s`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub stamp: Stamp,
    pub offset_s: f64,
}

impl Sample {
    /// Builds the measurement from a reference instant (whole Unix seconds, UTC)
    /// and the system time at which it was received.
    #[must_use]
    pub fn from_reference(reference_unix_s: i64, stamp: Stamp) -> Self {
        let reference_ns = i128::from(reference_unix_s) * 1_000_000_000;
        let diff_ns = reference_ns - stamp.realtime_ns;
        #[allow(clippy::cast_precision_loss)] // offsets are far below 2^53 ns
        let offset_s = diff_ns as f64 / 1e9;
        Self { stamp, offset_s }
    }

    /// Encodes as chrony's `struct sock_sample` (leap = 0, pulse = 0).
    #[must_use]
    pub fn encode(&self) -> [u8; SAMPLE_LEN] {
        let sec = self.stamp.realtime_ns.div_euclid(1_000_000_000);
        let usec = self.stamp.realtime_ns.rem_euclid(1_000_000_000) / 1_000;
        let mut b = [0u8; SAMPLE_LEN];
        b[0..8].copy_from_slice(&i64::try_from(sec).unwrap_or(i64::MAX).to_ne_bytes());
        b[8..16].copy_from_slice(&i64::try_from(usec).unwrap_or(0).to_ne_bytes());
        b[16..24].copy_from_slice(&self.offset_s.to_ne_bytes());
        b[24..28].copy_from_slice(&0i32.to_ne_bytes()); // pulse
        b[28..32].copy_from_slice(&0i32.to_ne_bytes()); // leap
        b[32..36].copy_from_slice(&0i32.to_ne_bytes()); // _pad
        b[36..40].copy_from_slice(&SOCK_MAGIC.to_ne_bytes());
        b
    }
}

/// Destination for samples. A trait so tests can capture them.
pub trait SampleSink: std::fmt::Debug {
    /// # Errors
    /// Returns the send error; callers log it and keep running.
    fn send(&mut self, sample: &Sample) -> io::Result<()>;
}

/// Sends to chronyd's SOCK refclock socket from an unbound datagram socket.
#[derive(Debug)]
pub struct ChronySocket {
    socket: UnixDatagram,
    path: PathBuf,
}

impl ChronySocket {
    /// # Errors
    /// Fails only if a local socket cannot be created; chronyd need not be running yet.
    pub fn new(path: PathBuf) -> io::Result<Self> {
        Ok(Self {
            socket: UnixDatagram::unbound()?,
            path,
        })
    }
}

impl SampleSink for ChronySocket {
    fn send(&mut self, sample: &Sample) -> io::Result<()> {
        self.socket
            .send_to(&sample.encode(), &self.path)
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn stamp(realtime_ns: i128) -> Stamp {
        Stamp {
            realtime_ns,
            mono: Duration::ZERO,
        }
    }

    #[test]
    fn offset_is_reference_minus_system() {
        // System clock behind: received at 1000.25 s, reference says 1001 s.
        let s = Sample::from_reference(1001, stamp(1_000_250_000_000));
        assert!((s.offset_s - 0.75).abs() < 1e-9, "{}", s.offset_s);
        // System clock ahead: received at 1001.5 s, reference says 1001 s.
        let s = Sample::from_reference(1001, stamp(1_001_500_000_000));
        assert!((s.offset_s + 0.5).abs() < 1e-9, "{}", s.offset_s);
    }

    #[test]
    fn encoded_layout() {
        let s = Sample {
            stamp: stamp(1_794_905_484_123_456_789),
            offset_s: -0.25,
        };
        let b = s.encode();
        assert_eq!(b.len(), 40);
        let i64_at = |o: usize| i64::from_ne_bytes(b[o..o + 8].try_into().unwrap());
        let i32_at = |o: usize| i32::from_ne_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!(i64_at(0), 1_794_905_484);
        assert_eq!(i64_at(8), 123_456);
        assert!((f64::from_ne_bytes(b[16..24].try_into().unwrap()) + 0.25).abs() < f64::EPSILON);
        assert_eq!(i32_at(24), 0);
        assert_eq!(i32_at(28), 0);
        assert_eq!(i32_at(32), 0);
        assert_eq!(i32_at(36), 0x534f_434b);
    }

    #[test]
    fn send_to_missing_socket_is_an_error_not_a_panic() {
        let mut sock = ChronySocket::new(PathBuf::from("/nonexistent/ltentp-test.sock")).unwrap();
        let s = Sample::from_reference(1, stamp(0));
        assert!(sock.send(&s).is_err());
    }
}
