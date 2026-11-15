//! The person or the room (ADR 0071): whether a stretch of speech the VAD found in a desk
//! session is the person talking to Ghost, judged by how loud it is against the person's
//! own level on this microphone, and by Ghost's name.
//!
//! The person's level comes first from the segment that woke Ghost, when the wake heard the
//! same input as the session (the transcript wake, `ghostd`), and then from the session's
//! own segments with words, read while Ghost was quiet and taken on their level (never one
//! taken only for the name, which may be from across the room). It moves up at once and
//! down a little each time, never more than `late_margin_db` under the loudest the person
//! has been, so someone who lowers their voice is followed, and so is a second person who
//! sits down at the desk and talks as loud, while the room, let in on a wide margin, can't
//! drag it down to its own level.
//!
//! A segment far below that level, and no louder than the room itself is, is the room:
//! the line is drawn at the room's own loud level (`ABOVE_ROOM_DB` over the 90th
//! percentile of the last ten seconds outside the person's speech) when that is under the
//! person's line, so in a quiet room nothing is held at all and the person may speak as
//! softly as they like. Right after Ghost stops speaking (the
//! follow-up window, `window_secs`) the margin is wide (`margin_db`); later it is narrower
//! (`late_margin_db`), since the person going on long after the answer is rarer than the
//! room. Before the person's level is known, a segment in the window is the person's, as
//! every segment was before; one later is taken only when it begins with Ghost's name.
//! Whenever a segment falls short, the name still takes it: "Ghost, ..." always reaches
//! Ghost, as it resumes a hold (ADR 0021).
//!
//! Everything here is arithmetic on levels the speech-in stage already measures: no model,
//! no audio kept. A judgement costs a comparison; a frame costs one level and a push.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The settings (`GHOST_ROOM_*`, `crate::config::VoiceSettings::room`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomSettings {
    /// Below the person's level by more than this, right after Ghost spoke, is the room.
    pub margin_db: f64,
    /// ... and once the follow-up window has passed.
    pub late_margin_db: f64,
    /// How long after Ghost stops speaking a follow-up is taken on the wide margin.
    pub window_secs: f64,
}

impl Default for RoomSettings {
    fn default() -> Self {
        Self {
            margin_db: 8.0,
            late_margin_db: 6.0,
            window_secs: 8.0,
        }
    }
}

/// What a segment is, as far as its level says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    /// Loud enough to be the person: it opens a turn as before.
    Person,
    /// Too quiet, or too late with no level to go on: held, and taken only if it begins
    /// with Ghost's name.
    Doubt,
}

/// A segment's level from its frames' levels (dBFS each, 20 ms): the 75th percentile, so
/// the gaps between words don't pull it down and the room's loudest syllable doesn't push
/// it up (the 90th let a talker 12 dB under the person through on its first 0.4 s,
/// 2026-11-14). The frames are speech's only: the VAD's pre-roll is left out where a
/// segment carries one (`stt::gate_level`).
pub fn segment_level(frames: &[f64]) -> Option<f64> {
    if frames.is_empty() {
        return None;
    }
    let mut v = frames.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[((v.len() - 1) as f64 * 0.75).round() as usize])
}

/// How far a segment's level may move the person's down at once.
const DOWN_STEP: f64 = 0.15;
/// Only a segment this close under the person's level moves it down: one the wide margin
/// let in from further off may be the room's, and must not teach the person's level
/// (2026-11-14: the room, let in at 7 dB under, took it down 5 dB in a minute).
const FOLLOW_DB: f64 = 3.0;
/// Quiet frames this long under the room's line end the person's segment.
pub const LEVEL_STOP_SECS: f64 = 0.3;
/// Frames counted for the room's level: the last ten seconds.
const FLOOR_FRAMES: usize = 500;
/// The line sits this far over the room's own loud level, when that is under the person's.
pub const ABOVE_ROOM_DB: f64 = 3.0;

#[derive(Debug, Clone)]
pub struct RoomGate {
    settings: RoomSettings,
    /// The person's level, dBFS, once known, and the loudest it has been.
    person: Option<f64>,
    loudest: f64,
    /// When Ghost last stopped speaking; None while it speaks.
    quiet_since: Option<Instant>,
    /// The levels of the frames heard outside the person's segments while Ghost was quiet.
    floor: VecDeque<f64>,
    /// Segments held and let go since the session began.
    pub dropped: usize,
}

impl RoomGate {
    pub fn new(settings: RoomSettings, now: Instant) -> Self {
        Self {
            settings,
            person: None,
            loudest: f64::MIN,
            quiet_since: Some(now),
            floor: VecDeque::new(),
            dropped: 0,
        }
    }

    pub fn person(&self) -> Option<f64> {
        self.person
    }

    /// Ghost's voice started or stopped.
    pub fn ghost(&mut self, speaking: bool, now: Instant) {
        match (speaking, self.quiet_since) {
            (true, _) => self.quiet_since = None,
            (false, None) => self.quiet_since = Some(now),
            (false, Some(_)) => {}
        }
    }

    fn in_window(&self, now: Instant) -> bool {
        match self.quiet_since {
            None => true, // over Ghost's voice the barge-in rule decides, on words
            Some(t) => now.duration_since(t) <= Duration::from_secs_f64(self.settings.window_secs),
        }
    }

    /// The level under which sound is the room's now, when the person's is known: the
    /// person's less the margin, or just over the room's own loud level if that is lower.
    pub fn line(&self, now: Instant) -> Option<f64> {
        let margin = if self.in_window(now) {
            self.settings.margin_db
        } else {
            self.settings.late_margin_db
        };
        let person = self.person? - margin;
        Some(match self.room_level() {
            Some(room) => person.min(room + ABOVE_ROOM_DB),
            None => person,
        })
    }

    /// A segment at `level` (dBFS) so far.
    pub fn judge(&self, level: f64, now: Instant) -> Judgement {
        match self.line(now) {
            Some(line) if level >= line => Judgement::Person,
            Some(_) => Judgement::Doubt,
            None if self.in_window(now) => Judgement::Person,
            None => Judgement::Doubt,
        }
    }

    /// A segment taken on its level was read and had words: the person's level follows it,
    /// up at once, down a quarter of the way.
    pub fn heard_person(&mut self, level: f64) {
        self.loudest = self.loudest.max(level);
        let floor = self.loudest - self.settings.late_margin_db;
        self.person = Some(match self.person {
            None => level,
            Some(p) if level > p => level,
            Some(p) if level >= p - FOLLOW_DB => (p + DOWN_STEP * (level - p)).max(floor),
            Some(p) => p,
        });
    }

    /// A frame outside the person's segments while Ghost was quiet: the room's level.
    pub fn room_frame(&mut self, level: f64) {
        if self.quiet_since.is_none() {
            return;
        }
        self.floor.push_back(level);
        while self.floor.len() > FLOOR_FRAMES {
            self.floor.pop_front();
        }
    }

    /// The room's loud level: the 90th percentile of the last ten seconds outside the
    /// person's speech (its talkers' syllables, its music's beats), once five seconds have
    /// been heard.
    pub fn room_level(&self) -> Option<f64> {
        if self.floor.len() < FLOOR_FRAMES / 2 {
            return None;
        }
        let mut v: Vec<f64> = self.floor.iter().copied().collect();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[((v.len() - 1) as f64 * 0.9).round() as usize])
    }

    /// The room is as loud as the person, or nearly: what is said across it can't be told
    /// from the person by level. Some once the person's level and the room's are known.
    pub fn too_loud(&self) -> Option<bool> {
        let (p, r) = (self.person?, self.room_level()?);
        Some(r >= p - self.settings.late_margin_db)
    }
}

/// The person's segment ends on its level: quiet frames under the room's line for
/// [`LEVEL_STOP_SECS`], where the VAD, hearing the room go on, would not end it.
#[derive(Debug, Default, Clone)]
pub struct LevelStop {
    quiet_secs: f64,
}

impl LevelStop {
    /// A frame of `secs` at `level`; true when the segment should end here.
    pub fn frame(&mut self, level: f64, line: Option<f64>, secs: f64) -> bool {
        match line {
            Some(line) if level < line => {
                self.quiet_secs += secs;
                self.quiet_secs >= LEVEL_STOP_SECS
            }
            _ => {
                self.quiet_secs = 0.0;
                false
            }
        }
    }

    pub fn reset(&mut self) {
        self.quiet_secs = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, secs: f64) -> Instant {
        t0 + Duration::from_secs_f64(secs)
    }

    #[test]
    fn before_the_persons_level_is_known_the_window_takes_everything_and_later_the_name() {
        let t0 = Instant::now();
        let g = RoomGate::new(RoomSettings::default(), t0);
        assert_eq!(g.judge(-60.0, at(t0, 2.0)), Judgement::Person);
        assert_eq!(g.judge(-20.0, at(t0, 20.0)), Judgement::Doubt);
    }

    #[test]
    fn far_under_the_person_is_the_room_and_the_margin_narrows_after_the_window() {
        let t0 = Instant::now();
        let mut g = RoomGate::new(RoomSettings::default(), t0);
        g.heard_person(-25.0);
        assert_eq!(g.judge(-24.0, at(t0, 1.0)), Judgement::Person);
        assert_eq!(
            g.judge(-32.0, at(t0, 1.0)),
            Judgement::Person,
            "7 dB under, early"
        );
        assert_eq!(g.judge(-35.0, at(t0, 1.0)), Judgement::Doubt, "10 dB under");
        assert_eq!(
            g.judge(-32.0, at(t0, 12.0)),
            Judgement::Doubt,
            "7 dB under, late"
        );
        assert_eq!(
            g.judge(-29.0, at(t0, 12.0)),
            Judgement::Person,
            "4 dB under, late"
        );
    }

    #[test]
    fn the_window_starts_again_when_ghost_stops_and_holds_while_it_speaks() {
        let t0 = Instant::now();
        let mut g = RoomGate::new(RoomSettings::default(), t0);
        g.heard_person(-25.0);
        g.ghost(true, at(t0, 20.0));
        assert_eq!(
            g.judge(-32.0, at(t0, 25.0)),
            Judgement::Person,
            "over Ghost"
        );
        g.ghost(false, at(t0, 30.0));
        assert_eq!(g.judge(-32.0, at(t0, 31.0)), Judgement::Person);
        assert_eq!(g.judge(-32.0, at(t0, 39.0)), Judgement::Doubt);
    }

    #[test]
    fn the_persons_level_follows_them_up_at_once_and_down_slowly() {
        let mut g = RoomGate::new(RoomSettings::default(), Instant::now());
        g.heard_person(-30.0);
        g.heard_person(-22.0);
        assert_eq!(g.person(), Some(-22.0));
        g.heard_person(-24.0);
        assert!((g.person().unwrap() + 22.3).abs() < 1e-9);
        // Speech let in from further under it doesn't move it at all.
        g.heard_person(-29.0);
        assert!((g.person().unwrap() + 22.3).abs() < 1e-9);
        // A person who goes on quieter is followed, never past the loudest less 6 dB.
        for _ in 0..80 {
            let p = g.person().unwrap();
            g.heard_person(p - 2.5);
        }
        assert_eq!(g.person(), Some(-28.0));
    }

    #[test]
    fn a_room_as_loud_as_the_person_is_too_loud_to_tell_apart() {
        let mut g = RoomGate::new(RoomSettings::default(), Instant::now());
        for _ in 0..FLOOR_FRAMES {
            g.room_frame(-30.0);
        }
        assert_eq!(g.too_loud(), None, "the person's level is not known yet");
        g.heard_person(-22.0);
        assert_eq!(g.too_loud(), Some(false));
        for _ in 0..FLOOR_FRAMES {
            g.room_frame(-26.0);
        }
        assert_eq!(g.too_loud(), Some(true));
    }

    #[test]
    fn the_rooms_level_is_not_counted_over_ghosts_voice() {
        let t0 = Instant::now();
        let mut g = RoomGate::new(RoomSettings::default(), t0);
        g.ghost(true, t0);
        for _ in 0..FLOOR_FRAMES {
            g.room_frame(-10.0);
        }
        assert_eq!(g.room_level(), None);
    }

    #[test]
    fn in_a_quiet_room_nothing_is_held_however_softly_the_person_speaks() {
        // 2026-11-14: a follow-up said 5 dB softer, nine seconds after the answer, measured
        // 9 dB under the person and was held in a room with nobody else in it.
        let t0 = Instant::now();
        let mut g = RoomGate::new(RoomSettings::default(), t0);
        g.heard_person(-21.0);
        for _ in 0..FLOOR_FRAMES {
            g.room_frame(-70.0);
        }
        assert_eq!(g.judge(-30.0, at(t0, 12.0)), Judgement::Person);
        assert_eq!(g.judge(-60.0, at(t0, 12.0)), Judgement::Person);
        assert_eq!(g.judge(-68.0, at(t0, 12.0)), Judgement::Doubt);
    }

    #[test]
    fn with_a_room_the_line_is_the_persons_or_just_over_the_rooms_whichever_is_lower() {
        let t0 = Instant::now();
        let mut g = RoomGate::new(RoomSettings::default(), t0);
        g.heard_person(-21.0);
        for i in 0..FLOOR_FRAMES {
            // A talker across the room: syllables at -33, gaps at -50.
            g.room_frame(if i % 2 == 0 { -33.0 } else { -50.0 });
        }
        assert_eq!(g.line(at(t0, 1.0)), Some(-30.0), "the room's -33 and 3 dB");
        assert_eq!(g.judge(-32.0, at(t0, 1.0)), Judgement::Doubt);
        assert_eq!(
            g.line(at(t0, 12.0)),
            Some(-30.0),
            "late: the person's line is -27, the room's lower"
        );
    }

    #[test]
    fn a_segment_level_ignores_the_gaps_between_words() {
        let mut frames = vec![-25.0; 30];
        frames.extend(vec![-70.0; 10]);
        assert_eq!(segment_level(&frames), Some(-25.0));
        assert_eq!(segment_level(&[]), None);
    }

    /// What the gate costs a frame (ADR 0018): `cargo test --release -p ghost-voice
    /// what_the_room_gate_costs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn what_the_room_gate_costs() {
        let t0 = Instant::now();
        let mut g = RoomGate::new(RoomSettings::default(), t0);
        g.heard_person(-21.0);
        let frame: Vec<i16> = (0..320)
            .map(|i| ((i * 7919) % 2001) as i16 - 1000)
            .collect();
        let mut stop = LevelStop::default();
        let mut frames: Vec<f64> = Vec::new();
        let n = 50 * 600; // ten minutes of 20 ms frames
        let began = Instant::now();
        for i in 0..n {
            let level = crate::stt::level_dbfs(&frame);
            g.room_frame(level);
            frames.push(level);
            if frames.len() > 25 {
                frames.remove(0);
            }
            let _ = g.judge(segment_level(&frames).unwrap_or(-120.0), t0);
            let _ = stop.frame(level, g.line(t0), 0.02);
            if i % 50 == 0 {
                let _ = g.too_loud();
            }
        }
        let per = began.elapsed().as_secs_f64() / n as f64;
        println!("room gate: {:.2} us a 20 ms frame", per * 1e6);
        assert!(per < 200e-6, "{per}");
    }

    #[test]
    fn the_persons_segment_ends_on_level_after_the_quiet_has_lasted() {
        let mut s = LevelStop::default();
        let line = Some(-35.0);
        for _ in 0..14 {
            assert!(!s.frame(-40.0, line, 0.02));
        }
        assert!(!s.frame(-20.0, line, 0.02), "a word again: start over");
        let ended = (0..20).any(|_| s.frame(-40.0, line, 0.02));
        assert!(ended);
        assert!(
            !LevelStop::default().frame(-80.0, None, 1.0),
            "no line, no stop"
        );
    }
}
