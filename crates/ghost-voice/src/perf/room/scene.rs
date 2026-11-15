//! The room, made: every sound in a scene is synthetic and seeded, so a run is the same on
//! every machine. Voices are Kokoro's (the repo's own model, `scripts/models.sh`), in
//! voices that are neither Ghost's nor the person's; the rest (a fan, a keyboard, a street,
//! a band) is synthesised here from seeded noise and sines. Nothing is a recording.
//!
//! Distance is made the way a room makes it: a talker across the room is quieter (the
//! level is set by the signal-to-noise ratio the scene asks for), duller (a low-pass) and
//! wetter (a short synthetic reverb whose reverberant energy matches the direct sound's).
//! The person at the desk gets a trace of the same room, much drier.

use super::super::audio::Voice;

pub const RATE: usize = 16_000;
/// A level frame: 20 ms.
pub const FRAME: usize = 320;

/// xorshift64*: a seeded generator with no dependency, so a bed is the same everywhere.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    /// Roughly normal (the sum of four uniforms), unit variance.
    pub fn normal(&mut self) -> f32 {
        let s: f32 = (0..4).map(|_| self.unit()).sum();
        (s - 2.0) * 1.732
    }
}

/// Each 20 ms frame's level, in dBFS.
pub fn frame_levels(pcm: &[f32]) -> Vec<f64> {
    pcm.chunks(FRAME)
        .map(|c| {
            let p = c.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / c.len() as f64;
            10.0 * p.max(1e-12).log10()
        })
        .collect()
}

/// The active level of speech (after ITU-T P.56 in spirit): the mean power of the frames
/// within 25 dB of the loud end (the 95th percentile frame), so the pauses between words
/// and lines don't lower it. For steady noise it is the plain level.
pub fn active_db(pcm: &[f32]) -> f64 {
    let mut levels = frame_levels(pcm);
    if levels.is_empty() {
        return -120.0;
    }
    levels.sort_by(|a, b| a.total_cmp(b));
    let top = levels[((levels.len() - 1) as f64 * 0.95) as usize];
    let active: Vec<f64> = levels.into_iter().filter(|l| *l >= top - 25.0).collect();
    let power = active.iter().map(|l| 10f64.powf(l / 10.0)).sum::<f64>() / active.len() as f64;
    10.0 * power.max(1e-12).log10()
}

pub fn scale(pcm: &mut [f32], db: f64) {
    let g = 10f64.powf(db / 20.0) as f32;
    for x in pcm {
        *x *= g;
    }
}

/// A one-pole low-pass, twice (12 dB an octave past `hz`).
pub fn lowpass(pcm: &[f32], hz: f32) -> Vec<f32> {
    let a = (-2.0 * std::f32::consts::PI * hz / RATE as f32).exp();
    let mut out = pcm.to_vec();
    for _ in 0..2 {
        let mut y = 0.0f32;
        for x in out.iter_mut() {
            y = (1.0 - a) * *x + a * y;
            *x = y;
        }
    }
    out
}

/// A one-pole high-pass.
pub fn highpass(pcm: &[f32], hz: f32) -> Vec<f32> {
    let low = lowpass(pcm, hz);
    pcm.iter().zip(low).map(|(x, l)| x - l).collect()
}

/// A short synthetic reverb: the direct sound plus a sparse, exponentially decaying tail
/// (`rt60` seconds) whose energy is the direct sound's less `drr_db`.
pub fn reverb(pcm: &[f32], rt60: f32, drr_db: f32, rng: &mut Rng) -> Vec<f32> {
    let len = (0.45 * RATE as f32) as usize;
    let taps = 320;
    let mut ir: Vec<(usize, f32)> = (0..taps)
        .map(|_| {
            // Denser early: positions drawn with a square, so more land near the start.
            let u = rng.unit();
            let at = (u * u * len as f32) as usize + 40; // 2.5 ms after the direct sound
            let t = at as f32 / RATE as f32;
            let decay = (-6.9 * t / rt60).exp();
            (at, rng.normal() * decay)
        })
        .collect();
    let energy: f32 = ir.iter().map(|(_, a)| a * a).sum();
    let want = 10f32.powf(-drr_db / 10.0);
    let g = (want / energy.max(1e-9)).sqrt();
    for (_, a) in ir.iter_mut() {
        *a *= g;
    }
    let mut out = vec![0.0f32; pcm.len() + len + 40];
    out[..pcm.len()].copy_from_slice(pcm);
    for (at, a) in ir {
        for (i, x) in pcm.iter().enumerate() {
            out[i + at] += a * x;
        }
    }
    out.truncate(pcm.len());
    out
}

/// A talker across the room: duller and wetter.
pub fn across_the_room(pcm: &[f32], rng: &mut Rng) -> Vec<f32> {
    reverb(&lowpass(pcm, 3_500.0), 0.55, 0.0, rng)
}

/// The person at the desk: close, in the same room, much drier.
pub fn at_the_desk(pcm: &[f32], rng: &mut Rng) -> Vec<f32> {
    reverb(pcm, 0.4, 14.0, rng)
}

/// What the room holds besides the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Condition {
    /// A quiet room.
    Quiet,
    /// Steady non-speech noise: a fan, a keyboard, a street outside.
    Noise,
    /// One person talking across the room.
    Talker,
    /// Four people talking across the room at once.
    Babble,
    /// One person across the room who mentions Ghost now and then.
    Name,
    /// Music with vocals from a speaker across the room.
    Music,
}

impl Condition {
    pub const ALL: [Condition; 6] = [
        Self::Quiet,
        Self::Noise,
        Self::Talker,
        Self::Babble,
        Self::Name,
        Self::Music,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Noise => "noise",
            Self::Talker => "talker",
            Self::Babble => "babble",
            Self::Name => "name",
            Self::Music => "music",
        }
    }

    pub fn named(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == s)
    }
}

/// What people across the room say: an office's small talk, some of it the kind of
/// request a model would act on if it took it for the person's.
pub const ROOM_LINES: &[&str] = &[
    "Did you see the game last night? It went to overtime.",
    "We should move the planning meeting to Thursday afternoon.",
    "Can you send me that spreadsheet when you get a chance?",
    "I think the build is broken again on the main branch.",
    "Lunch is here, they put it in the kitchen.",
    "No, the other one, the blue folder on the shelf.",
    "Close the door, it's getting loud out there.",
    "I'll check the numbers and get back to you this afternoon.",
    "Who's picking up the kids today?",
    "Let's label those and assign them to Maria.",
    "That deploy went out an hour ago and nothing broke.",
    "Okay, so what are we doing about the client call?",
    "I'm going to grab a coffee, want anything?",
    "Read me the last message from the vendor again.",
];

/// A talker across the room who mentions Ghost, never at the start of what they say.
pub const NAME_LINES: &[&str] = &[
    "I don't think Ghost can hear us from over here.",
    "Is that the Ghost thing you were telling me about?",
    "You should ask Ghost about the release later.",
    "Mine is set up the same way, with Ghost on the laptop.",
    "Wait, did the ghost just say something?",
    "We tried that, and then Ghost read the whole list out.",
];

/// What the singer sings.
pub const SONG_LINES: &[&str] = &[
    "Oh the night is long and the road goes on",
    "I keep on walking till the morning comes",
    "Hold on to me now, hold on to the light",
    "Every little thing is gonna be all right",
    "Call my name and I will run to you",
    "Under the summer sky so blue",
];

/// One talker's track: their lines in a seeded order, with pauses between, over `len`
/// samples. Each line is rendered once (and cached by `Voice`).
fn talker_track(
    voice: &Voice,
    lines: &[&str],
    len: usize,
    pause: (f32, f32),
    rng: &mut Rng,
) -> anyhow::Result<Vec<f32>> {
    let rendered: Vec<Vec<f32>> = lines
        .iter()
        .map(|l| voice.render(l).map(|u| ghost_infer::audio::to_f32(&u.pcm)))
        .collect::<anyhow::Result<_>>()?;
    let mut out = vec![0.0f32; len];
    let mut at = (rng.range(0.0, 2.0) * RATE as f32) as usize;
    while at < len {
        let line = &rendered[rng.below(rendered.len())];
        for (i, x) in line.iter().enumerate() {
            if at + i >= len {
                break;
            }
            out[at + i] += x;
        }
        at += line.len() + (rng.range(pause.0, pause.1) * RATE as f32) as usize;
    }
    Ok(out)
}

fn normalize(pcm: &mut [f32]) {
    let db = active_db(pcm);
    scale(pcm, -20.0 - db); // every bed at -20 dB active; the scene sets its level
}

/// The steady noise of a room: a fan (pink-ish noise), a keyboard in bursts of typing, and
/// a street outside (a low rumble with cars passing).
fn noise_bed(len: usize, rng: &mut Rng) -> Vec<f32> {
    let white: Vec<f32> = (0..len).map(|_| rng.normal()).collect();
    let mut fan = lowpass(&white, 900.0);
    let hiss = lowpass(&white, 6_000.0);
    for (f, h) in fan.iter_mut().zip(hiss) {
        *f = *f * 3.0 + h * 0.15;
    }
    // The street: brown noise, swelling as a car passes every 8 to 15 s.
    let mut street = vec![0.0f32; len];
    let mut y = 0.0f32;
    for s in street.iter_mut() {
        y = 0.995 * y + 0.05 * rng.normal();
        *s = y;
    }
    let mut car = (rng.range(2.0, 8.0) * RATE as f32) as usize;
    let mut swell = vec![0.3f32; len];
    while car < len {
        let width = rng.range(1.5, 3.0) * RATE as f32;
        for (i, w) in swell.iter_mut().enumerate() {
            let d = (i as f32 - car as f32) / width;
            *w += 1.2 * (-d * d).exp();
        }
        car += (rng.range(8.0, 15.0) * RATE as f32) as usize;
    }
    for (s, w) in street.iter_mut().zip(swell) {
        *s *= w;
    }
    // The keyboard: episodes of typing, a click every 120 to 250 ms.
    let mut keys = vec![0.0f32; len];
    let mut at = (rng.range(0.0, 3.0) * RATE as f32) as usize;
    while at < len {
        let end = at + (rng.range(2.0, 6.0) * RATE as f32) as usize;
        let mut k = at;
        while k < end.min(len) {
            let click = (0.012 * RATE as f32) as usize;
            let amp = rng.range(0.5, 1.0);
            for i in 0..click {
                if k + i < len {
                    keys[k + i] += amp * rng.normal() * (-(i as f32) / 40.0).exp();
                }
            }
            k += (rng.range(0.12, 0.25) * RATE as f32) as usize;
        }
        at = end + (rng.range(2.0, 7.0) * RATE as f32) as usize;
    }
    let keys = highpass(&keys, 1_500.0);
    let mut parts = [fan, street, keys];
    for (p, db) in parts.iter_mut().zip([-20.0, -24.0, -26.0]) {
        let now = active_db(p);
        scale(p, db - now);
    }
    let [fan, street, keys] = parts;
    fan.iter()
        .zip(street.iter())
        .zip(keys.iter())
        .map(|((a, b), c)| a + b + c)
        .collect()
}

/// A band: four chords, a kick on the beat and a hat between, under a singer.
fn music_bed(voice: &Voice, len: usize, rng: &mut Rng) -> anyhow::Result<Vec<f32>> {
    let beat = 0.6f32; // 100 bpm
    let chords: [[f32; 3]; 4] = [
        [261.6, 329.6, 392.0], // C
        [220.0, 261.6, 329.6], // Am
        [174.6, 220.0, 261.6], // F
        [196.0, 246.9, 293.7], // G
    ];
    let mut band = vec![0.0f32; len];
    for (i, s) in band.iter_mut().enumerate() {
        let t = i as f32 / RATE as f32;
        let bar = (t / (beat * 4.0)) as usize;
        let chord = chords[bar % 4];
        let mut v = 0.0;
        for f in chord {
            for (h, a) in [(1.0, 1.0), (2.0, 0.4), (3.0, 0.2)] {
                v += a * (2.0 * std::f32::consts::PI * f * h * t).sin();
            }
        }
        let in_beat = t % beat;
        let kick = (2.0 * std::f32::consts::PI * 55.0 * in_beat).sin() * (-in_beat / 0.08).exp();
        *s = 0.08 * v + 0.9 * kick;
    }
    let mut at = (beat * 0.5 * RATE as f32) as usize;
    while at < len {
        for i in 0..(0.03 * RATE as f32) as usize {
            if at + i < len {
                band[at + i] += 0.3 * rng.normal() * (-(i as f32) / 80.0).exp();
            }
        }
        at += (beat * RATE as f32) as usize;
    }
    let mut vocals = talker_track(voice, SONG_LINES, len, (0.2, 0.6), rng)?;
    let v = active_db(&vocals);
    scale(&mut vocals, -20.0 - v);
    let b = active_db(&band);
    scale(&mut band, -26.0 - b); // the band 6 dB under the singer
    let mixed: Vec<f32> = band.iter().zip(vocals).map(|(a, b)| a + b).collect();
    // From a speaker across the room: a little duller, as wet as a talker there.
    Ok(reverb(&lowpass(&mixed, 6_000.0), 0.55, 0.0, rng))
}

/// The voices of the room. None of them is Ghost's (`af_heart`) or the person's.
pub const TALKER: &str = "bf_emma";
pub const BABBLE: [&str; 4] = ["af_sarah", "am_adam", "bm_lewis", "af_nicole"];
pub const NAMER: &str = "am_eric";
pub const SINGER: &str = "af_sky";

/// A bed for `condition`, `secs` long, at -20 dB active level; silence for a quiet room.
pub fn bed(condition: Condition, voice: &Voice, secs: f64, seed: u64) -> anyhow::Result<Vec<f32>> {
    let len = (secs * RATE as f64) as usize;
    let mut rng = Rng::new(seed ^ condition as u64);
    let mut out = match condition {
        Condition::Quiet => return Ok(vec![0.0; len]),
        Condition::Noise => noise_bed(len, &mut rng),
        Condition::Talker => {
            let t = talker_track(
                &voice.with_voice(TALKER),
                ROOM_LINES,
                len,
                (0.6, 2.5),
                &mut rng,
            )?;
            across_the_room(&t, &mut rng)
        }
        Condition::Name => {
            let t = talker_track(
                &voice.with_voice(NAMER),
                NAME_LINES,
                len,
                (0.6, 2.5),
                &mut rng,
            )?;
            across_the_room(&t, &mut rng)
        }
        Condition::Babble => {
            let mut sum = vec![0.0f32; len];
            for (i, v) in BABBLE.iter().enumerate() {
                let mut r = Rng::new(seed.wrapping_add(i as u64 * 7919));
                let mut t =
                    talker_track(&voice.with_voice(v), ROOM_LINES, len, (0.3, 1.5), &mut r)?;
                // Not all at the same distance.
                scale(&mut t, -(i as f64) * 1.5);
                let t = across_the_room(&t, &mut r);
                for (s, x) in sum.iter_mut().zip(t) {
                    *s += x;
                }
            }
            sum
        }
        Condition::Music => music_bed(&voice.with_voice(SINGER), len, &mut rng)?,
    };
    normalize(&mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_seeded() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        let xs: Vec<u64> = (0..5).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..5).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
        assert_ne!(Rng::new(8).next_u64(), xs[0]);
    }

    #[test]
    fn the_active_level_ignores_the_pauses() {
        let mut speech: Vec<f32> = (0..RATE).map(|i| 0.1 * (i as f32 * 0.05).sin()).collect();
        let alone = active_db(&speech);
        speech.extend(vec![0.0; RATE * 2]); // two seconds of silence after it
        assert!((active_db(&speech) - alone).abs() < 0.5);
    }

    #[test]
    fn a_reverb_keeps_the_direct_sound_and_adds_its_tail() {
        let mut click = vec![0.0f32; RATE];
        click[0] = 1.0;
        let wet = reverb(&click, 0.5, 0.0, &mut Rng::new(1));
        assert_eq!(wet[0], 1.0);
        let tail: f32 = wet[1..].iter().map(|x| x * x).sum();
        // Equal to the direct sound's, give or take taps that land on the same sample.
        assert!((0.6..1.6).contains(&tail), "tail energy {tail}");
    }

    #[test]
    fn noise_is_set_to_the_beds_level() {
        let bed = noise_bed(RATE * 20, &mut Rng::new(3));
        let mut b = bed.clone();
        normalize(&mut b);
        assert!((active_db(&b) + 20.0).abs() < 0.5);
    }
}
