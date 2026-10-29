//! Opus through the system `libopus` (the `opus` crate): the phone's packets decoded at
//! 48 kHz mono, Ghost's speech encoded at 48 kHz mono, 20 ms a packet, VoIP mode. Packet
//! loss is concealed by the decoder (`conceal`), one frame per missing packet, as Opus
//! does on the phone.

use crate::MediaError;
use crate::frame::{WIRE_RATE, frame_samples};

/// The most samples one Opus packet can hold: 120 ms at 48 kHz.
const MAX_PACKET_SAMPLES: usize = 5760;
/// The most bytes one packet can be (RFC 6716 recommends 1275 for 20 ms at 48 kHz).
const MAX_PACKET_BYTES: usize = 1275;

fn opus_err(e: opus::Error) -> MediaError {
    MediaError::Opus(e.to_string())
}

/// Decodes the phone's packets to 48 kHz mono s16.
pub struct OpusDecoder {
    inner: opus::Decoder,
    buf: Vec<i16>,
}

impl OpusDecoder {
    pub fn new() -> Result<Self, MediaError> {
        Ok(OpusDecoder {
            inner: opus::Decoder::new(WIRE_RATE, opus::Channels::Mono).map_err(opus_err)?,
            buf: vec![0; MAX_PACKET_SAMPLES],
        })
    }

    /// One packet to PCM: 960 samples for the 20 ms the phone sends, whatever the packet's
    /// own duration says otherwise.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>, MediaError> {
        let n = self
            .inner
            .decode(packet, &mut self.buf, false)
            .map_err(opus_err)?;
        Ok(self.buf[..n].to_vec())
    }

    /// Packet loss concealment: one 20 ms frame guessed from what came before.
    pub fn conceal(&mut self) -> Result<Vec<i16>, MediaError> {
        let n = self
            .inner
            .decode(&[], &mut self.buf[..frame_samples(WIRE_RATE)], false)
            .map_err(opus_err)?;
        Ok(self.buf[..n].to_vec())
    }
}

/// Encodes Ghost's speech, 48 kHz mono s16 in 20 ms frames.
pub struct OpusEncoder {
    inner: opus::Encoder,
    buf: Vec<u8>,
}

impl OpusEncoder {
    pub fn new() -> Result<Self, MediaError> {
        let mut inner =
            opus::Encoder::new(WIRE_RATE, opus::Channels::Mono, opus::Application::Voip)
                .map_err(opus_err)?;
        // What aiortc sent: the encoder's own bitrate choice for speech, no FEC, no DTX.
        inner.set_inband_fec(false).map_err(opus_err)?;
        inner.set_dtx(false).map_err(opus_err)?;
        Ok(OpusEncoder {
            inner,
            buf: vec![0; MAX_PACKET_BYTES],
        })
    }

    /// One 20 ms frame (960 samples) to one packet.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, MediaError> {
        debug_assert_eq!(pcm.len(), frame_samples(WIRE_RATE));
        let n = self.inner.encode(pcm, &mut self.buf).map_err(opus_err)?;
        Ok(self.buf[..n].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::rms;

    fn tone(hz: f32, amplitude: f32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / WIRE_RATE as f32;
                (amplitude * (2.0 * std::f32::consts::PI * hz * t).sin() * 32767.0) as i16
            })
            .collect()
    }

    #[test]
    fn round_trip_keeps_a_tones_level_and_a_silent_frame_stays_quiet() {
        let mut enc = OpusEncoder::new().unwrap();
        let mut dec = OpusDecoder::new().unwrap();
        let pcm = tone(440.0, 0.5, 960 * 10);
        let mut out = Vec::new();
        let mut sizes = Vec::new();
        for frame in pcm.chunks(960) {
            let packet = enc.encode(frame).unwrap();
            assert!(!packet.is_empty() && packet.len() <= MAX_PACKET_BYTES);
            sizes.push(packet.len());
            let decoded = dec.decode(&packet).unwrap();
            assert_eq!(decoded.len(), 960);
            out.extend(decoded);
        }
        // After the codec's warm-up the level is within 1.5 dB of what went in.
        let input = rms(&pcm[960 * 5..]);
        let output = rms(&out[960 * 5..]);
        let db = 20.0 * (output / input).log10();
        assert!(db.abs() < 1.5, "level changed by {db} dB");
        let mut silent = Vec::new();
        let mut decoded = Vec::new();
        for _ in 0..6 {
            silent = enc.encode(&[0; 960]).unwrap();
            decoded = dec.decode(&silent).unwrap();
        }
        assert!(
            silent.len() < 40,
            "silence should be a small packet, got {}",
            silent.len()
        );
        assert!(rms(&decoded) < 200.0, "{}", rms(&decoded));
    }

    #[test]
    fn concealment_gives_a_full_frame_shaped_like_the_last_one() {
        let mut enc = OpusEncoder::new().unwrap();
        let mut dec = OpusDecoder::new().unwrap();
        let pcm = tone(440.0, 0.5, 960 * 5);
        for frame in pcm.chunks(960) {
            dec.decode(&enc.encode(frame).unwrap()).unwrap();
        }
        let concealed = dec.conceal().unwrap();
        assert_eq!(concealed.len(), 960);
        assert!(rms(&concealed) > 1000.0, "PLC continues the tone");
        let mut fresh = OpusDecoder::new().unwrap();
        let from_nothing = fresh.conceal().unwrap();
        assert_eq!(from_nothing.len(), 960);
        assert_eq!(rms(&from_nothing), 0.0);
    }

    #[test]
    fn a_bad_packet_is_an_error_not_a_panic() {
        let mut dec = OpusDecoder::new().unwrap();
        assert!(matches!(dec.decode(&[0xff; 4]), Err(MediaError::Opus(_))));
    }
}
