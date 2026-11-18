//! Timing of the modem clock's second boundaries by fast `AT+CCLK?` polling.
//!
//! `+CCLK` has one-second resolution, but the instant its seconds field
//! changes can be located to a few milliseconds: when one reply reads `S` and
//! the next reads `S + 1`, the modem's clock ticked after the earlier query
//! was sent and before the later reply arrived. A burst of queries a few
//! milliseconds apart therefore brackets the tick, and the bracket's midpoint
//! compared with `S + 1` gives the offset of the modem clock from the system
//! clock.

use std::time::Duration;

use crate::chrony::Sample;
use crate::clock::Stamp;

/// A burst without an edge gives up after this long (a tick is due within 1 s).
pub const BURST_MAX: Duration = Duration::from_millis(1_200);
/// A query without a `+CCLK` reply after this long is abandoned, and no new
/// query is sent for this long again: a late reply to the abandoned query must
/// not be paired with the send time of a newer one.
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(500);
/// A burst starts this long before the predicted tick.
pub const LEAD: Duration = Duration::from_millis(100);

/// One answered `AT+CCLK?`: when it was sent, when the reply was read, and the
/// clock it reported (Unix seconds, UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Poll {
    pub sent: Stamp,
    pub recv: Stamp,
    pub utc: i64,
}

/// A located tick of the modem clock: it reached `utc` between `lo` and `hi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub utc: i64,
    pub lo: Stamp,
    pub hi: Stamp,
}

impl Edge {
    /// Midpoint of the bracket: the best estimate of the tick on the system clock.
    #[must_use]
    pub fn mid(&self) -> Stamp {
        Stamp {
            realtime_ns: self.lo.realtime_ns + (self.hi.realtime_ns - self.lo.realtime_ns) / 2,
            mono: self.lo.mono + (self.hi.mono.saturating_sub(self.lo.mono)) / 2,
        }
    }

    /// Modem clock − system clock at the tick, in seconds (chrony's sign: positive
    /// when the system clock is behind).
    #[must_use]
    pub fn offset_s(&self) -> f64 {
        Sample::from_reference(self.utc, self.mid()).offset_s
    }

    /// Half the bracket width in seconds: the bound on the midpoint's error.
    #[must_use]
    pub fn half_width_s(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)] // widths are far below 2^53 ns
        let w = (self.hi.realtime_ns - self.lo.realtime_ns) as f64;
        w / 2e9
    }
}

/// Finds ticks in a sequence of polls.
#[derive(Debug, Default)]
pub struct EdgeDetector {
    prev: Option<Poll>,
}

impl EdgeDetector {
    /// Forgets the previous poll, e.g. when a reply was lost.
    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// Returns the tick between the previous poll and this one, if the clock
    /// advanced by exactly one second. Any other change (a set clock, a lost
    /// reply) yields nothing and starts over from this poll.
    pub fn observe(&mut self, poll: Poll) -> Option<Edge> {
        let edge = match self.prev {
            Some(p) if poll.utc == p.utc + 1 => Some(Edge {
                utc: poll.utc,
                lo: p.sent,
                hi: poll.recv,
            }),
            _ => None,
        };
        self.prev = Some(poll);
        edge
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle { next_burst: Duration },
    Burst { end: Duration, next_query: Duration },
}

/// Decides when to send `AT+CCLK?`: every `interval`, a burst of queries
/// `step` apart until one tick is located. One query is in flight at a time.
/// Times are monotonic (`Stamp::mono`).
#[derive(Debug)]
pub struct CclkPoller {
    interval: Duration,
    step: Duration,
    state: State,
    in_flight: Option<Stamp>,
    /// No query is sent before this (quiet time after an abandoned query).
    quiet_until: Duration,
    detector: EdgeDetector,
}

impl CclkPoller {
    /// `interval` is rounded down to whole seconds (at least 1) so that each
    /// burst can be aimed at a tick predicted from the previous one.
    #[must_use]
    pub fn new(interval: Duration, step: Duration) -> Self {
        Self {
            interval: Duration::from_secs(interval.as_secs().max(1)),
            step,
            state: State::Idle {
                next_burst: Duration::ZERO,
            },
            in_flight: None,
            quiet_until: Duration::ZERO,
            detector: EdgeDetector::default(),
        }
    }

    /// Gives up on the query in flight, if any. Replies that arrive during the
    /// following quiet time are ignored because nothing is in flight.
    fn abandon(&mut self, t: Duration) {
        if self.in_flight.take().is_some() {
            self.detector.reset();
            self.quiet_until = t + REPLY_TIMEOUT;
        }
    }

    /// Advances the schedule to `now`. Returns true when a query must be sent
    /// now; `now` is then taken as its send time, so call this right before writing.
    pub fn on_timer(&mut self, now: Stamp) -> bool {
        let t = now.mono;
        if let Some(sent) = self.in_flight {
            if t >= sent.mono + REPLY_TIMEOUT {
                self.abandon(t);
            }
        }
        match self.state {
            State::Idle { next_burst } if t >= next_burst => {
                self.detector.reset();
                self.state = State::Burst {
                    end: t + BURST_MAX,
                    next_query: t,
                };
            }
            State::Burst { end, .. } if t >= end => {
                self.abandon(t);
                self.state = State::Idle {
                    next_burst: t + self.interval.saturating_sub(BURST_MAX),
                };
                return false;
            }
            _ => {}
        }
        if let State::Burst { end, next_query } = self.state {
            if self.in_flight.is_none() && t >= next_query.max(self.quiet_until) {
                self.in_flight = Some(now);
                self.state = State::Burst {
                    end,
                    next_query: t + self.step,
                };
                return true;
            }
        }
        false
    }

    /// The monotonic time at which [`CclkPoller::on_timer`] next has work to do.
    #[must_use]
    pub fn deadline(&self) -> Duration {
        match self.state {
            State::Idle { next_burst } => next_burst,
            State::Burst { end, next_query } => {
                let next = match self.in_flight {
                    Some(sent) => sent.mono + REPLY_TIMEOUT,
                    None => next_query.max(self.quiet_until),
                };
                next.min(end)
            }
        }
    }

    /// Takes a `+CCLK` reply read at `recv`. Returns the located tick, after
    /// which the burst ends and the next one is aimed at a later tick.
    /// A reply that no query of ours is waiting for is ignored.
    pub fn on_cclk(&mut self, utc: i64, recv: Stamp) -> Option<Edge> {
        let sent = self.in_flight.take()?;
        let State::Burst { .. } = self.state else {
            return None;
        };
        let edge = self.detector.observe(Poll { sent, recv, utc })?;
        // The clock ticks once a second, so a whole number of seconds later
        // the next tick is at the same phase.
        let next_tick = edge.mid().mono + self.interval;
        self.state = State::Idle {
            next_burst: next_tick.saturating_sub(LEAD).max(recv.mono),
        };
        Some(edge)
    }

    /// Takes an `ERROR` reply: the query in flight, if any, failed.
    pub fn on_error(&mut self) {
        if self.in_flight.take().is_some() {
            self.detector.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stamp at `ms` milliseconds; the system clock reads 1794964219 s + `ms`.
    fn at(ms: u64) -> Stamp {
        Stamp {
            realtime_ns: 1_794_964_219_000_000_000 + i128::from(ms) * 1_000_000,
            mono: Duration::from_millis(ms),
        }
    }

    fn poll(sent: u64, recv: u64, utc: i64) -> Poll {
        Poll {
            sent: at(sent),
            recv: at(recv),
            utc,
        }
    }

    #[test]
    fn edge_is_bracketed_by_previous_send_and_current_receive() {
        let mut d = EdgeDetector::default();
        assert_eq!(d.observe(poll(0, 4, 1_794_964_219)), None);
        assert_eq!(d.observe(poll(10, 14, 1_794_964_219)), None);
        let e = d.observe(poll(20, 24, 1_794_964_220)).unwrap();
        assert_eq!((e.lo, e.hi, e.utc), (at(10), at(24), 1_794_964_220));
        assert_eq!(e.mid(), at(17));
        assert!((e.half_width_s() - 0.007).abs() < 1e-12);
    }

    #[test]
    fn offset_sign_both_ways() {
        // Tick at system 219.017 s for modem 220: the system is 0.983 s behind.
        let e = Edge {
            utc: 1_794_964_220,
            lo: at(10),
            hi: at(24),
        };
        assert!((e.offset_s() - 0.983).abs() < 1e-9, "{}", e.offset_s());
        // Tick at system 220.017 s for modem 220: the system is 17 ms ahead.
        let e = Edge {
            utc: 1_794_964_220,
            lo: at(1_010),
            hi: at(1_024),
        };
        assert!((e.offset_s() + 0.017).abs() < 1e-9, "{}", e.offset_s());
    }

    #[test]
    fn jumps_and_resets_give_no_edge() {
        let mut d = EdgeDetector::default();
        d.observe(poll(0, 4, 3_155_760_000));
        // Clock set by the network: a jump, not a tick.
        assert_eq!(d.observe(poll(10, 14, 1_794_964_219)), None);
        d.reset();
        assert_eq!(d.observe(poll(20, 24, 1_794_964_220)), None);
        assert!(d.observe(poll(30, 34, 1_794_964_221)).is_some());
        // Backwards.
        assert_eq!(d.observe(poll(40, 44, 1_794_964_220)), None);
    }

    /// Drives a poller with a modem whose clock ticks at `tick_ms` past each
    /// system second and answers 4 ms after a query. Returns (sent, edge) events.
    fn simulate(p: &mut CclkPoller, until_ms: u64, tick_ms: u64) -> (Vec<u64>, Vec<Edge>) {
        let mut sent = Vec::new();
        let mut edges = Vec::new();
        let mut t = 0;
        while t < until_ms {
            if p.on_timer(at(t)) {
                sent.push(t);
                // The modem samples its clock at t + 2 and replies at t + 4.
                let sample = t + 2;
                let utc =
                    1_794_964_219 + i64::try_from((sample + 1_000 - tick_ms) / 1_000).unwrap();
                if let Some(e) = p.on_cclk(utc, at(t + 4)) {
                    edges.push(e);
                }
            }
            let next = p.deadline().as_millis();
            assert!(next > u128::from(t), "deadline {next} not after {t}");
            t = u64::try_from(next).unwrap().max(t + 1);
        }
        (sent, edges)
    }

    #[test]
    fn bursts_find_one_edge_per_interval_and_then_aim() {
        let mut p = CclkPoller::new(Duration::from_secs(10), Duration::from_millis(10));
        let (sent, edges) = simulate(&mut p, 35_000, 300);
        // Ticks at 0.3 s, 10.3 s, 20.3 s, 30.3 s.
        assert_eq!(edges.len(), 4, "{edges:?}");
        for e in &edges {
            let tick =
                Duration::from_millis(300 + 1_000 * u64::try_from(e.utc - 1_794_964_220).unwrap());
            assert!(e.lo.mono < tick && tick <= e.hi.mono, "{e:?}");
            assert!(e.half_width_s() <= 0.0071, "{e:?}");
        }
        // The first burst searches from t=0; later ones start 100 ms early.
        let later: Vec<u64> = sent.iter().copied().filter(|&t| t > 5_000).collect();
        assert!(later.len() <= 3 * 12, "too many queries: {}", later.len());
    }

    #[test]
    fn lost_reply_times_out_then_waits_out_a_late_reply() {
        let mut p = CclkPoller::new(Duration::from_secs(10), Duration::from_millis(10));
        assert!(p.on_timer(at(0)));
        // No reply: nothing more is sent until the timeout.
        assert!(!p.on_timer(at(100)));
        assert_eq!(p.deadline(), Duration::from_millis(500));
        // Abandoned at 500 ms, then quiet until 1000 ms.
        assert!(!p.on_timer(at(500)));
        assert_eq!(p.deadline(), Duration::from_millis(1_000));
        // The late reply arrives while nothing is in flight: ignored.
        assert_eq!(p.on_cclk(1_794_964_219, at(700)), None);
        assert!(!p.on_timer(at(999)));
        assert!(p.on_timer(at(1_000)));
        // An ERROR ends the query in flight; the next one goes out a step later.
        p.on_error();
        assert!(!p.on_timer(at(1_005)));
        assert!(p.on_timer(at(1_010)));
    }

    #[test]
    fn late_reply_is_not_paired_with_a_newer_query() {
        let mut p = CclkPoller::new(Duration::from_secs(10), Duration::from_millis(10));
        assert!(p.on_timer(at(0)));
        assert_eq!(p.on_cclk(1_794_964_219, at(4)), None);
        assert!(p.on_timer(at(10)));
        // The reply to the 10 ms query (clock still at :19) stalls past the timeout.
        assert!(!p.on_timer(at(510)));
        assert_eq!(p.on_cclk(1_794_964_219, at(900)), None);
        // Polling resumes after the quiet time from a fresh start: the first reply
        // cannot form an edge with anything read before the stall.
        assert!(p.on_timer(at(1_010)));
        assert_eq!(p.on_cclk(1_794_964_220, at(1_014)), None);
        assert!(p.on_timer(at(1_024)));
        assert_eq!(p.on_cclk(1_794_964_220, at(1_028)), None);
    }

    #[test]
    fn burst_without_edge_gives_up() {
        let mut p = CclkPoller::new(Duration::from_secs(10), Duration::from_millis(10));
        let mut t = 0;
        while t < 1_200 {
            if p.on_timer(at(t)) {
                // The clock never advances (e.g. a frozen RTC).
                assert_eq!(p.on_cclk(1_794_964_219, at(t + 4)), None);
            }
            t += 1;
        }
        assert!(!p.on_timer(at(1_200)));
        // Given up at 1.2 s; the next burst is one interval after this one began.
        assert_eq!(p.deadline(), Duration::from_secs(10));
    }

    #[test]
    fn unsolicited_reply_is_ignored() {
        let mut p = CclkPoller::new(Duration::from_secs(10), Duration::from_millis(10));
        assert_eq!(p.on_cclk(1_794_964_219, at(1)), None);
    }
}
