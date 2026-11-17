//! Splits the raw byte stream from the AT port into lines.
//!
//! The port is read in arbitrary chunks: a line can span reads, and one read
//! can hold several lines. CR and LF both end a line; empty lines are dropped.

/// Lines longer than this are cut so a stream without line ends cannot grow memory.
pub const MAX_LINE: usize = 4096;

/// Accumulates bytes and yields complete lines.
#[derive(Debug, Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one read's bytes and returns the lines completed by it, in order.
    /// Invalid UTF-8 is replaced, not rejected, so the raw line can still be logged.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in bytes {
            if b == b'\r' || b == b'\n' {
                self.flush_into(&mut lines);
            } else {
                self.pending.push(b);
                if self.pending.len() >= MAX_LINE {
                    self.flush_into(&mut lines);
                }
            }
        }
        lines
    }

    fn flush_into(&mut self, lines: &mut Vec<String>) {
        if !self.pending.is_empty() {
            lines.push(String::from_utf8_lossy(&self.pending).into_owned());
            self.pending.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_split_across_reads() {
        let mut s = LineSplitter::new();
        assert_eq!(s.push(b"\r\n+CTZEU: +36,0,\"2026/11"), Vec::<String>::new());
        assert_eq!(
            s.push(b"/17,08:51:24\"\r\n"),
            vec!["+CTZEU: +36,0,\"2026/11/17,08:51:24\""]
        );
    }

    #[test]
    fn two_lines_in_one_read_with_ok_interleaved() {
        let mut s = LineSplitter::new();
        assert_eq!(
            s.push(b"AT+CCLK?\r\r\n+CCLK: \"26/11/17,08:51:24+36\"\r\n\r\nOK\r\n+CTZV: +36\r\n"),
            vec![
                "AT+CCLK?",
                "+CCLK: \"26/11/17,08:51:24+36\"",
                "OK",
                "+CTZV: +36"
            ]
        );
    }

    #[test]
    fn lone_cr_ends_line() {
        let mut s = LineSplitter::new();
        assert_eq!(s.push(b"OK\r+CTZV: +36\r"), vec!["OK", "+CTZV: +36"]);
        assert_eq!(s.push(b"\r\r\n\n"), Vec::<String>::new());
    }

    #[test]
    fn garbage_is_kept_lossily() {
        let mut s = LineSplitter::new();
        assert_eq!(s.push(b"\xff\xfeAB\r\n"), vec!["\u{fffd}\u{fffd}AB"]);
    }

    #[test]
    fn overlong_line_is_cut() {
        let mut s = LineSplitter::new();
        let lines = s.push(&vec![b'x'; MAX_LINE + 10]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_LINE);
        assert_eq!(s.push(b"\n"), vec!["x".repeat(10)]);
    }
}
