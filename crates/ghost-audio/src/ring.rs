//! The lock-free ring between PipeWire's thread and tokio, on `rtrb` (a realtime-safe
//! single-producer single-consumer ring). These helpers are the only code the process
//! callbacks run against the ring: they move samples in chunks and allocate nothing. What
//! does not fit is dropped and counted, never blocked on.

use rtrb::{Consumer, Producer, RingBuffer};

/// A ring holding `seconds` of mono audio at `rate`.
pub fn ring(rate: u32, seconds: f32) -> (Producer<i16>, Consumer<i16>) {
    let capacity = ((rate as f32 * seconds) as usize).max(1);
    RingBuffer::new(capacity)
}

/// Push little-endian s16 bytes into the ring. Returns `(written, dropped)` in samples;
/// an odd trailing byte is ignored. Allocation-free.
pub fn push_le_bytes(producer: &mut Producer<i16>, bytes: &[u8]) -> (usize, usize) {
    let n = bytes.len() / 2;
    let take = n.min(producer.slots());
    if take == 0 {
        return (0, n);
    }
    let Ok(chunk) = producer.write_chunk_uninit(take) else {
        return (0, n);
    };
    let (pairs, _) = bytes.as_chunks::<2>();
    let written = chunk.fill_from_iter(pairs.iter().take(take).map(|c| i16::from_le_bytes(*c)));
    (written, n - written)
}

/// Push samples into the ring. Returns `(written, dropped)`. Allocation-free.
pub fn push_samples(producer: &mut Producer<i16>, samples: &[i16]) -> (usize, usize) {
    let take = samples.len().min(producer.slots());
    if take == 0 {
        return (0, samples.len());
    }
    let Ok(chunk) = producer.write_chunk_uninit(take) else {
        return (0, samples.len());
    };
    let written = chunk.fill_from_iter(samples.iter().copied());
    (written, samples.len() - written)
}

/// Pop up to `out.len() / 2` samples into `out` as little-endian s16 bytes. Returns how
/// many samples were written; the bytes after them are left as they were. Allocation-free.
pub fn pop_into_le_bytes(consumer: &mut Consumer<i16>, out: &mut [u8]) -> usize {
    let want = out.len() / 2;
    let take = want.min(consumer.slots());
    if take == 0 {
        return 0;
    }
    let Ok(chunk) = consumer.read_chunk(take) else {
        return 0;
    };
    let (first, second) = chunk.as_slices();
    let (pairs, _) = out.as_chunks_mut::<2>();
    for (dst, &s) in pairs.iter_mut().zip(first.iter().chain(second.iter())) {
        *dst = s.to_le_bytes();
    }
    chunk.commit_all();
    take
}

/// Pop exactly `n` samples as a new `Vec`, or `None` when the ring holds fewer. This is
/// the consumer side (tokio), where an allocation is fine.
pub fn pop_frame(consumer: &mut Consumer<i16>, n: usize) -> Option<Vec<i16>> {
    if consumer.slots() < n {
        return None;
    }
    let chunk = consumer.read_chunk(n).ok()?;
    let (first, second) = chunk.as_slices();
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(first);
    out.extend_from_slice(second);
    chunk.commit_all();
    Some(out)
}

/// Drop everything queued. Returns how many samples went. Allocation-free.
pub fn drop_all<T>(consumer: &mut Consumer<T>) -> usize {
    let n = consumer.slots();
    if n == 0 {
        return 0;
    }
    match consumer.read_chunk(n) {
        Ok(chunk) => {
            chunk.commit_all();
            n
        }
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_capacity_is_seconds_at_rate() {
        let (p, _c) = ring(16_000, 1.0);
        assert_eq!(p.buffer().capacity(), 16_000);
        let (p, _c) = ring(24_000, 0.5);
        assert_eq!(p.buffer().capacity(), 12_000);
        let (p, _c) = ring(16_000, 0.0);
        assert_eq!(p.buffer().capacity(), 1); // never zero
    }

    #[test]
    fn bytes_round_trip_little_endian() {
        let (mut p, mut c) = RingBuffer::<i16>::new(8);
        let samples: [i16; 3] = [1, -2, i16::MIN];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(push_le_bytes(&mut p, &bytes), (3, 0));
        let mut out = [0xAAu8; 8];
        assert_eq!(pop_into_le_bytes(&mut c, &mut out), 3);
        assert_eq!(&out[..6], &bytes[..]);
        assert_eq!(
            &out[6..],
            &[0xAA, 0xAA],
            "bytes past the data are untouched"
        );
    }

    #[test]
    fn an_odd_trailing_byte_is_ignored() {
        let (mut p, mut c) = RingBuffer::<i16>::new(8);
        assert_eq!(push_le_bytes(&mut p, &[1, 0, 2]), (1, 0));
        assert_eq!(pop_frame(&mut c, 1), Some(vec![1]));
    }

    #[test]
    fn overrun_drops_and_counts() {
        let (mut p, mut c) = RingBuffer::<i16>::new(4);
        assert_eq!(push_samples(&mut p, &[1, 2, 3]), (3, 0));
        assert_eq!(push_samples(&mut p, &[4, 5, 6]), (1, 2));
        assert_eq!(push_samples(&mut p, &[7]), (0, 1));
        assert_eq!(pop_frame(&mut c, 4), Some(vec![1, 2, 3, 4]));
        assert_eq!(push_samples(&mut p, &[]), (0, 0));
    }

    #[test]
    fn pop_frame_waits_for_a_whole_frame() {
        let (mut p, mut c) = RingBuffer::<i16>::new(1000);
        push_samples(&mut p, &[0; 319]);
        assert_eq!(pop_frame(&mut c, 320), None);
        push_samples(&mut p, &[7]);
        let f = pop_frame(&mut c, 320).unwrap();
        assert_eq!(f.len(), 320);
        assert_eq!(f[319], 7);
        assert_eq!(pop_frame(&mut c, 320), None);
    }

    #[test]
    fn pop_frame_crosses_the_wrap() {
        let (mut p, mut c) = RingBuffer::<i16>::new(6);
        push_samples(&mut p, &[1, 2, 3, 4]);
        pop_frame(&mut c, 4);
        push_samples(&mut p, &[5, 6, 7, 8]); // wraps around the end of the buffer
        assert_eq!(pop_frame(&mut c, 4), Some(vec![5, 6, 7, 8]));
        push_samples(&mut p, &[9, 10, 11, 12]);
        let mut out = [0u8; 8];
        assert_eq!(pop_into_le_bytes(&mut c, &mut out), 4);
        assert_eq!(out, [9, 0, 10, 0, 11, 0, 12, 0]);
    }

    #[test]
    fn drop_all_empties_the_ring() {
        let (mut p, mut c) = RingBuffer::<i16>::new(8);
        assert_eq!(drop_all(&mut c), 0);
        push_samples(&mut p, &[1, 2, 3]);
        assert_eq!(drop_all(&mut c), 3);
        assert!(c.is_empty());
        assert_eq!(push_samples(&mut p, &[0; 8]), (8, 0));
    }

    #[test]
    fn pop_into_short_output_takes_what_fits() {
        let (mut p, mut c) = RingBuffer::<i16>::new(8);
        push_samples(&mut p, &[1, 2, 3]);
        let mut out = [0u8; 3]; // room for one sample and a stray byte
        assert_eq!(pop_into_le_bytes(&mut c, &mut out), 1);
        assert_eq!(c.slots(), 2);
        assert_eq!(pop_into_le_bytes(&mut c, &mut []), 0);
    }
}
