//! RTP bookkeeping the stacks share: packets, sequence gaps and RFC 3550 interarrival
//! jitter on the inbound stream. Numbers only.

use std::time::Instant;

/// Counts one inbound RTP stream.
#[derive(Debug)]
pub struct RtpCounters {
    clock_rate: u32,
    started: bool,
    /// The highest extended sequence number seen.
    max_ext: u64,
    first: u64,
    cycles: u64,
    last_seq: u16,
    received: u64,
    bytes: u64,
    last_transit: Option<f64>,
    jitter: f64,
}

/// What one packet meant for the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    /// Packets missing before this one, by its sequence number (0 in order; a late or
    /// duplicate packet is `late` instead).
    pub gap: u16,
    pub late: bool,
}

impl RtpCounters {
    pub fn new(clock_rate: u32) -> Self {
        RtpCounters {
            clock_rate,
            started: false,
            max_ext: 0,
            first: 0,
            cycles: 0,
            last_seq: 0,
            received: 0,
            bytes: 0,
            last_transit: None,
            jitter: 0.0,
        }
    }

    /// Note a packet by its sequence number, RTP timestamp, arrival and size.
    pub fn packet(
        &mut self,
        seq: u16,
        timestamp: u32,
        arrival: Instant,
        bytes: usize,
        epoch: Instant,
    ) -> Arrival {
        self.received += 1;
        self.bytes += bytes as u64;
        // RFC 3550 A.8: transit = arrival (in RTP units) - RTP timestamp.
        let arrival_units = arrival.duration_since(epoch).as_secs_f64() * self.clock_rate as f64;
        let transit = arrival_units - timestamp as f64;
        if let Some(last) = self.last_transit {
            let d = (transit - last).abs();
            self.jitter += (d - self.jitter) / 16.0;
        }
        self.last_transit = Some(transit);

        if !self.started {
            self.started = true;
            self.last_seq = seq;
            self.max_ext = seq as u64;
            self.first = seq as u64;
            return Arrival {
                gap: 0,
                late: false,
            };
        }
        let delta = seq.wrapping_sub(self.last_seq);
        if delta == 0 || delta >= 0x8000 {
            // A duplicate or a packet that came in late: already counted as lost when its
            // successor arrived, so the gap is not charged twice.
            return Arrival { gap: 0, late: true };
        }
        if seq < self.last_seq {
            self.cycles += 1 << 16;
        }
        self.last_seq = seq;
        self.max_ext = self.cycles + seq as u64;
        Arrival {
            gap: delta - 1,
            late: false,
        }
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Packets expected by the sequence numbers that never came.
    pub fn lost(&self) -> u64 {
        if !self.started {
            return 0;
        }
        let expected = self.max_ext - (self.first_ext()) + 1;
        expected.saturating_sub(self.received)
    }

    fn first_ext(&self) -> u64 {
        // The counters start at the first packet's sequence number with no cycles.
        self.first
    }

    /// Interarrival jitter in milliseconds.
    pub fn jitter_ms(&self) -> f64 {
        self.jitter * 1000.0 / self.clock_rate as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn counts_gaps_wraps_and_duplicates() {
        let epoch = Instant::now();
        let mut c = RtpCounters::new(48_000);
        let t = |ms: u64| epoch + Duration::from_millis(ms);
        assert_eq!(
            c.packet(65_534, 0, t(0), 50, epoch),
            Arrival {
                gap: 0,
                late: false
            }
        );
        assert_eq!(
            c.packet(65_535, 960, t(20), 50, epoch),
            Arrival {
                gap: 0,
                late: false
            }
        );
        // 0 is lost, 1 arrives: one gap, across the wrap.
        assert_eq!(
            c.packet(1, 960 * 3, t(60), 50, epoch),
            Arrival {
                gap: 1,
                late: false
            }
        );
        assert_eq!(c.lost(), 1);
        // The late one shows up: not charged twice, and the loss count drops.
        assert_eq!(
            c.packet(0, 960 * 2, t(61), 50, epoch),
            Arrival { gap: 0, late: true }
        );
        assert_eq!(c.lost(), 0);
        assert_eq!(
            c.packet(1, 960 * 3, t(62), 50, epoch),
            Arrival { gap: 0, late: true }
        );
        assert_eq!(c.received(), 5);
        assert_eq!(c.bytes(), 250);
    }

    #[test]
    fn jitter_is_zero_for_a_steady_stream_and_grows_with_wobble() {
        let epoch = Instant::now();
        let mut c = RtpCounters::new(48_000);
        for i in 0..50u64 {
            c.packet(
                i as u16,
                (i * 960) as u32,
                epoch + Duration::from_millis(i * 20),
                50,
                epoch,
            );
        }
        assert!(c.jitter_ms() < 0.01, "{}", c.jitter_ms());
        let mut w = RtpCounters::new(48_000);
        for i in 0..200u64 {
            let wobble = if i % 2 == 0 { 0 } else { 10 };
            w.packet(
                i as u16,
                (i * 960) as u32,
                epoch + Duration::from_millis(i * 20 + wobble),
                50,
                epoch,
            );
        }
        // Each step is 10 ms off the last; the estimator settles near it.
        assert!(
            w.jitter_ms() > 8.0 && w.jitter_ms() < 10.5,
            "{}",
            w.jitter_ms()
        );
    }
}
