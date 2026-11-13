//! What the floating room does while the screen is shared (ADR 0059, amended 2026-11-13).
//!
//! By default it hides, as it always has: nothing of Ghost reaches a meeting or a recording by
//! accident. The operator may choose otherwise (Settings › General › The room, "While my screen
//! is shared"): show Ghost in the share, or be asked each time a share starts. Whatever they
//! answer, or toggle from the tray, holds for that share only and is forgotten when it ends.
//!
//! A pure state machine: the shell feeds it the capture it reads from mutter (`capture.rs`) and
//! the clock, and sends the surface the one word it needs (`Sharing`).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::capture::Capture;

/// The operator's choice for every share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhileShared {
    /// Ghost draws nothing at all while the screen is shared (the default, and what it did
    /// before there was a choice).
    #[default]
    Hide,
    /// Ghost shows in the share, with a mark that says the screen is being shared.
    Show,
    /// A small card asks, on the surface's monitor, when a share starts.
    Ask,
}

/// What the surface does now, as one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Sharing {
    /// Nothing is capturing the screen (or this desktop can't tell): as ever.
    #[default]
    None,
    /// A share is on and Ghost stays out of it: nothing drawn, not even the line.
    Hidden,
    /// A share is on and Ghost is in it, marked as shared.
    Shown,
    /// A share has just started and the card asks Show or Hide; nothing else is drawn.
    Asking,
}

impl Sharing {
    /// The surface may show Ghost (the posture rule's `floating` input, with the switch on).
    pub fn lets_ghost_show(self) -> bool {
        matches!(self, Sharing::None | Sharing::Shown)
    }
}

/// How long the card asks before the share is taken as Hide. It is itself in the share, so it
/// must not stay there all meeting; the tray still turns Ghost on for the share after that.
pub const ASK_FOR: Duration = Duration::from_secs(20);

/// One share's memory: since when it has been seen, and the operator's word for it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Share {
    since: Option<Instant>,
    answer: Option<bool>,
}

impl Share {
    /// The capture as read now. A share that ended forgets its answer.
    pub fn seen(&mut self, capture: Capture, now: Instant) {
        if capture == Capture::Yes {
            self.since.get_or_insert(now);
        } else {
            *self = Share::default();
        }
    }

    /// The operator answered the card (or chose from the tray): for this share only. Nothing
    /// to remember when no share is on.
    pub fn answer(&mut self, show: bool) {
        if self.since.is_some() {
            self.answer = Some(show);
        }
    }

    /// The tray's toggle: the opposite of what is in effect now.
    pub fn toggle(&mut self, choice: WhileShared, now: Instant) {
        let shown = self.sharing(choice, now) == Sharing::Shown;
        self.answer(!shown);
    }

    pub fn sharing(&self, choice: WhileShared, now: Instant) -> Sharing {
        let Some(since) = self.since else {
            return Sharing::None;
        };
        match (self.answer, choice) {
            (Some(true), _) => Sharing::Shown,
            (Some(false), _) => Sharing::Hidden,
            (None, WhileShared::Hide) => Sharing::Hidden,
            (None, WhileShared::Show) => Sharing::Shown,
            (None, WhileShared::Ask) if now.duration_since(since) < ASK_FOR => Sharing::Asking,
            (None, WhileShared::Ask) => Sharing::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    #[test]
    fn hide_is_the_default_and_hides_every_share() {
        assert_eq!(WhileShared::default(), WhileShared::Hide);
        let t = Instant::now();
        let mut s = Share::default();
        assert_eq!(s.sharing(WhileShared::Hide, t), Sharing::None);
        s.seen(Capture::Yes, t);
        assert_eq!(s.sharing(WhileShared::Hide, at(t, 1)), Sharing::Hidden);
        assert!(!s.sharing(WhileShared::Hide, t).lets_ghost_show());
        s.seen(Capture::No, at(t, 9));
        assert_eq!(s.sharing(WhileShared::Hide, at(t, 9)), Sharing::None);
        assert!(Sharing::None.lets_ghost_show());
    }

    #[test]
    fn show_shows_ghost_in_the_share() {
        let t = Instant::now();
        let mut s = Share::default();
        s.seen(Capture::Yes, t);
        assert_eq!(s.sharing(WhileShared::Show, t), Sharing::Shown);
        assert!(Sharing::Shown.lets_ghost_show());
    }

    #[test]
    fn a_desktop_that_cannot_tell_is_not_a_share() {
        let t = Instant::now();
        let mut s = Share::default();
        s.seen(Capture::Unknown, t);
        assert_eq!(s.sharing(WhileShared::Hide, t), Sharing::None);
        assert_eq!(s.sharing(WhileShared::Ask, t), Sharing::None);
    }

    #[test]
    fn ask_asks_for_a_while_then_hides_and_the_answer_holds_for_that_share_only() {
        let t = Instant::now();
        let mut s = Share::default();
        s.seen(Capture::Yes, t);
        assert_eq!(s.sharing(WhileShared::Ask, t), Sharing::Asking);
        assert!(!Sharing::Asking.lets_ghost_show());
        // The poll every two seconds doesn't restart the clock.
        s.seen(Capture::Yes, at(t, 10));
        assert_eq!(s.sharing(WhileShared::Ask, at(t, 19)), Sharing::Asking);
        // Unanswered: hidden, never left asking in the share all meeting.
        assert_eq!(s.sharing(WhileShared::Ask, at(t, 20)), Sharing::Hidden);
        // Answered Show: shown until the share ends.
        s.answer(true);
        assert_eq!(s.sharing(WhileShared::Ask, at(t, 30)), Sharing::Shown);
        s.seen(Capture::No, at(t, 40));
        // The next share asks again.
        s.seen(Capture::Yes, at(t, 50));
        assert_eq!(s.sharing(WhileShared::Ask, at(t, 50)), Sharing::Asking);
        s.answer(false);
        assert_eq!(s.sharing(WhileShared::Ask, at(t, 51)), Sharing::Hidden);
    }

    #[test]
    fn the_tray_toggles_what_is_in_effect_for_this_share() {
        let t = Instant::now();
        let mut s = Share::default();
        // No share: nothing to toggle, nothing remembered for the next one.
        s.toggle(WhileShared::Hide, t);
        s.seen(Capture::Yes, t);
        assert_eq!(s.sharing(WhileShared::Hide, t), Sharing::Hidden);
        s.toggle(WhileShared::Hide, t);
        assert_eq!(s.sharing(WhileShared::Hide, t), Sharing::Shown);
        s.toggle(WhileShared::Hide, t);
        assert_eq!(s.sharing(WhileShared::Hide, t), Sharing::Hidden);
        // While asking, the tray answers Show.
        let mut a = Share::default();
        a.seen(Capture::Yes, t);
        a.toggle(WhileShared::Ask, t);
        assert_eq!(a.sharing(WhileShared::Ask, t), Sharing::Shown);
        // A Show choice toggled hides for this share, and the next share shows again.
        let mut b = Share::default();
        b.seen(Capture::Yes, t);
        b.toggle(WhileShared::Show, t);
        assert_eq!(b.sharing(WhileShared::Show, t), Sharing::Hidden);
        b.seen(Capture::No, at(t, 5));
        b.seen(Capture::Yes, at(t, 6));
        assert_eq!(b.sharing(WhileShared::Show, at(t, 6)), Sharing::Shown);
    }

    #[test]
    fn the_word_reads_back_as_the_page_expects() {
        for (s, w) in [
            (Sharing::None, "\"none\""),
            (Sharing::Hidden, "\"hidden\""),
            (Sharing::Shown, "\"shown\""),
            (Sharing::Asking, "\"asking\""),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), w);
        }
        assert_eq!(serde_json::to_string(&WhileShared::Ask).unwrap(), "\"ask\"");
    }
}
