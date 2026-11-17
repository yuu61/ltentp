//! ltentp: proof of concept for evaluating LTE network time (NITZ) as a time source.
//!
//! Reads time-related AT lines from a WWAN AT port, stamps each read with the
//! system clock, logs every line to CSV and forwards `+CTZEU` samples to
//! chrony's `refclock SOCK`. See README.md and docs/.

#[cfg(not(target_os = "linux"))]
compile_error!("ltentp targets Linux (Debian); build and test it in WSL or on the target");

pub mod at;
pub mod chrony;
pub mod civil;
pub mod cli;
pub mod clock;
pub mod framing;
pub mod recorder;
