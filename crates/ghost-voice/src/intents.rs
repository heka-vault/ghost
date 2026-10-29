//! Fixed intents that need no model: hang up, hold and cancel (the Jev experiment showed
//! a short exact-match list is nearly as good as a classifier for these, with nothing on
//! the network path). Ghost's own `TurnInterceptor`.
//! "Undo that" is not one: it goes to the model, which proposes the reversing change.
//!
//! Hold (ADR 0021): "hang on, Ghost" keeps the session but Ghost stops listening until it is
//! addressed by name again; the room can talk. There is no "go to sleep" any more: a
//! goodbye hangs up, a wait holds.

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use ghost_core::confirmation::{has_negation, normalize};
use ghost_core::routes::{Quiet, parse_quiet};
use ghost_core::source::{Action, Handled, TurnInterceptor, UserTurn};
use rand::seq::IndexedRandom;

const FILLERS: &[&str] = &["ghost", "okay", "ok", "hey", "oh", "um", "so", "please"];

pub const HANG_UP: &[&str] = &[
    "that's all",
    "that's all for now",
    "that's all thanks",
    "thanks that's all",
    "thanks that's everything",
    "thanks ghost",
    "thank you ghost",
    "goodnight ghost",
    "good night ghost",
    "bye ghost",
    "we're done",
    "done for now",
    "that's it thanks",
];
/// The whole turn (fillers aside) asks Ghost to wait. "Wait" alone is left to the model and
/// the confirmation path: with a question pending it is a no, not a hold.
pub const HOLD: &[&str] = &[
    "hang on",
    "hang on a sec",
    "hang on a second",
    "hang on a minute",
    "hang on a moment",
    "hold on",
    "hold on a sec",
    "hold on a second",
    "hold on a minute",
    "hold on a moment",
    "hold that thought",
    "one sec",
    "one second",
    "one moment",
    "one minute",
    "just a sec",
    "just a second",
    "just a minute",
    "just a moment",
    "give me a sec",
    "give me a second",
    "give me a minute",
    "give me a moment",
    "wait a sec",
    "wait a second",
    "wait a minute",
    "wait a moment",
    "be right back",
    "stand by",
];
/// Two words at least: Kokoro reads a lone "Sure." flat. Short, because the operator has
/// already turned to someone else.
pub const HOLD_ACKS: &[&str] = &[
    "Sure, take your time.",
    "Take your time.",
    "I'll be here.",
    "Sure thing.",
];
/// A goodbye at the end of a turn ends the session too, when everything before it is small
/// talk: "I'll talk to you later, hang up". Anything else before it ("close it and hang up")
/// goes to the model, which does the work first.
pub const GOODBYE_TAILS: &[&str] = &[
    "hang up",
    "you can hang up",
    "goodbye",
    "good bye",
    "bye",
    "bye bye",
    "talk to you later",
    "talk later",
    "see you later",
    "see ya",
    "i'm done",
    "we're done",
    "gotta go",
    "got to go",
    "i have to go",
    "i gotta go",
    "have to run",
    "catch you later",
    "later",
    "that's it",
    "all set",
];
const SMALL_TALK: &[&str] = &[
    "alright",
    "all",
    "right",
    "cool",
    "great",
    "thanks",
    "thank",
    "you",
    "i'll",
    "i",
    "will",
    "talk",
    "to",
    "later",
    "that's",
    "it",
    "for",
    "now",
    "uh",
    "yeah",
    "see",
    "ya",
    "good",
    "bye",
    "night",
    "done",
    "i'm",
    "we're",
    "and",
    // "That is all I need, I'll talk to you later"
    "is",
    "that",
    "need",
    "think",
    "today",
    "everything",
    "was",
    "perfect",
    // "Never mind, I'll talk to you later": a dropped thought before the goodbye
    "never",
    "mind",
    "nevermind",
    "anyway",
    "anyways",
    "nothing",
    "else",
    "then",
];
/// What Ghost says when the shortcut hangs up. Not a bare "Okay.": Kokoro reads a lone
/// word flat and falling, and it came across as sad to be sent away. The goodbye stays
/// (it is what makes the end feel human, 2026-10-20); the speed comes from everything
/// around it: the instant phrase match, no model round trip, and the phone closing the
/// call the moment the goodbye has played.
pub const GOODBYES: &[&str] = &[
    "Talk soon!",
    "Catch you later!",
    "Sounds good, talk soon!",
    "Bye for now!",
];
/// Only when a change is waiting for a yes.
pub const CANCEL: &[&str] = &[
    "never mind",
    "nevermind",
    "forget it",
    "forget that",
    "scratch that",
    "cancel",
    "cancel that",
    "drop it",
    "drop that",
    "leave it",
    "skip it",
];

fn is_small_talk(w: &str) -> bool {
    FILLERS.contains(&w) || SMALL_TALK.contains(&w)
}

pub fn pick(lines: &[&str]) -> String {
    lines
        .choose(&mut rand::rng())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct QuickIntents;

#[async_trait]
impl TurnInterceptor for QuickIntents {
    fn name(&self) -> &str {
        "quick_intents"
    }

    async fn on_user_turn(&self, turn: &UserTurn) -> Option<Handled> {
        let norm = normalize(&turn.text);
        let words: Vec<&str> = norm.split_whitespace().collect();
        if words.is_empty() || words.len() > 12 {
            return None;
        }
        if words.len() > 6 {
            return goodbye(&words).then(|| Handled {
                say: Some(pick(GOODBYES)),
                action: Some(Action::HangUp),
                until: None,
            });
        }
        // Try the words as heard and with fillers ("ghost", "okay") stripped.
        let stripped: String = words
            .iter()
            .filter(|w| !FILLERS.contains(w))
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let phrases = [norm.as_str(), stripped.as_str()];
        let any_in = |set: &[&str]| phrases.iter().any(|p| set.contains(p));
        if any_in(HANG_UP) || goodbye(&words) {
            return Some(Handled {
                say: Some(pick(GOODBYES)),
                action: Some(Action::HangUp),
                until: None,
            });
        }
        if any_in(HOLD) && !has_negation(&norm) {
            return Some(Handled {
                say: Some(pick(HOLD_ACKS)),
                action: Some(Action::Hold),
                until: None,
            });
        }
        if turn.pending && any_in(CANCEL) {
            return Some(Handled {
                say: Some("Dropped it.".into()),
                action: Some(Action::Cancel),
                until: None,
            });
        }
        match parse_quiet(&turn.text, Local::now()) {
            // quiet, asked for with a time that isn't one: nothing is set
            Some(Quiet::Unclear) => Some(Handled {
                say: Some("Sorry, I didn't catch the time. Say it like: nothing until two.".into()),
                action: None,
                until: None,
            }),
            // ADR 0031: the operator's words set it; no model
            Some(Quiet::Until(until)) => Some(Handled {
                say: Some(format!("Okay, nothing until {}.", spoken_time(until))),
                action: Some(Action::Quiet),
                until: Some(until),
            }),
            None => None,
        }
    }
}

fn spoken_time(t: DateTime<chrono::Utc>) -> String {
    let t = Local.from_utc_datetime(&t.naive_utc());
    let hour = match t.hour() % 12 {
        0 => 12,
        h => h,
    };
    let half = if t.hour() < 12 {
        "in the morning"
    } else if t.hour() < 18 {
        "in the afternoon"
    } else {
        "tonight"
    };
    if t.hour() == 12 && t.minute() == 0 {
        return "noon".into();
    }
    let when = if t.minute() == 0 {
        format!("{hour}")
    } else {
        format!("{hour}:{:02}", t.minute())
    };
    let now = Local::now();
    let tomorrow = (t.year(), t.ordinal()) != (now.year(), now.ordinal());
    format!("{when} {half}{}", if tomorrow { " tomorrow" } else { "" })
}

/// Ends with a goodbye, and only small talk comes before it; never with a negation.
fn goodbye(words: &[&str]) -> bool {
    if has_negation(&words.join(" ")) || words.contains(&"don't") || words.contains(&"not") {
        return false;
    }
    let mut words = words;
    while let Some(last) = words.last()
        && FILLERS.contains(last)
    {
        words = &words[..words.len() - 1];
    }
    let mut tails: Vec<&str> = GOODBYE_TAILS.to_vec();
    tails.sort_by_key(|t| std::cmp::Reverse(t.len()));
    for tail in tails {
        let t: Vec<&str> = tail.split_whitespace().collect();
        if words.len() >= t.len() && words[words.len() - t.len()..] == t[..] {
            return words[..words.len() - t.len()]
                .iter()
                .all(|w| is_small_talk(w));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quick_intents() {
        for (text, pending, expect) in [
            ("thanks ghost", false, Some(Action::HangUp)),
            ("That's all for now.", true, Some(Action::HangUp)),
            ("Ghost, go to sleep.", false, None), // no such phrase any more (ADR 0021)
            // hold: the whole turn asks Ghost to wait
            ("hang on, Ghost", false, Some(Action::Hold)),
            ("Hold on a sec.", true, Some(Action::Hold)),
            ("okay ghost one second", false, Some(Action::Hold)),
            ("don't hold on", false, None),
            ("hold on, close it first", false, None),
            ("never mind", true, Some(Action::Cancel)),
            ("never mind", false, None), // nothing pending: let the model handle it
            // no undo intent: taking a change back is a turn for the model
            ("undo that", false, None),
            ("Ghost, undo that.", false, None),
            ("take that back", false, None),
            ("go to sleep after you close it", false, None),
            ("what needs my sign off", false, None),
            // a goodbye at the end, after small talk
            (
                "I'll talk to you later. Hang up.",
                false,
                Some(Action::HangUp),
            ),
            ("Okay, thanks, bye.", false, Some(Action::HangUp)),
            ("Alright, I'm done.", false, Some(Action::HangUp)),
            (
                "That is all I need. I will talk to you later.",
                false,
                Some(Action::HangUp),
            ),
            (
                "I think that is everything for today. Bye.",
                false,
                Some(Action::HangUp),
            ),
            ("goodbye", true, Some(Action::HangUp)),
            (
                "Uh never mind. I will talk to you later.",
                false,
                Some(Action::HangUp),
            ),
            ("Nothing else, gotta go.", true, Some(Action::HangUp)),
            // anything else first goes to the model, which does the work
            ("close it and then hang up", false, None),
            ("label it bug, bye", false, None),
            ("don't hang up", false, None),
            ("not yet, don't hang up", false, None),
            ("how do I hang up", false, None),
        ] {
            let h = QuickIntents
                .on_user_turn(&UserTurn {
                    text: text.into(),
                    pending,
                    last_assistant: String::new(),
                })
                .await;
            assert_eq!(h.and_then(|h| h.action), expect, "{text:?}");
        }
    }

    #[tokio::test]
    async fn test_the_shortcut_says_a_warm_goodbye_not_a_bare_okay() {
        let h = QuickIntents
            .on_user_turn(&UserTurn {
                text: "I'll talk to you later. Hang up.".into(),
                pending: false,
                last_assistant: String::new(),
            })
            .await
            .unwrap();
        let say = h.say.unwrap();
        assert!(GOODBYES.contains(&say.as_str()) && say != "Okay.");
        assert_eq!(h.action, Some(Action::HangUp));
    }

    #[tokio::test]
    async fn test_do_not_disturb_is_a_fixed_intent_with_its_time() {
        let h = QuickIntents
            .on_user_turn(&UserTurn {
                text: "Ghost, nothing for an hour".into(),
                pending: false,
                last_assistant: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(h.action, Some(Action::Quiet));
        assert!(h.until.is_some());
        assert!(h.say.unwrap().starts_with("Okay, nothing until"));
        assert!(
            QuickIntents
                .on_user_turn(&UserTurn {
                    text: "nothing until later".into(),
                    pending: false,
                    last_assistant: String::new(),
                })
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_a_quiet_time_that_isnt_one_is_said_back_not_set() {
        for said in ["quiet until 3:75", "nothing until 14 pm"] {
            let h = QuickIntents
                .on_user_turn(&UserTurn {
                    text: said.into(),
                    pending: false,
                    last_assistant: String::new(),
                })
                .await
                .unwrap();
            assert!(h.action.is_none() && h.until.is_none(), "{said}");
            assert!(h.say.unwrap().contains("didn't catch the time"));
        }
    }
}
