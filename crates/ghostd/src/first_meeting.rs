//! The first meeting (ADR 0059 as amended 2026-11-14): the first run's wake moment.
//!
//! The desktop's first run arms it (`PUT /first-meeting`) while its wake beat is open. The
//! next session with Ghost at this desk takes it, once, and opens with Ghost's own first
//! line, said before the model is asked anything, so the first thing a newcomer hears never
//! waits on a slow model and never depends on one: with no model connected, the same line
//! says, in one honest sentence, that Ghost needs one, and every turn after it answers that
//! with Ghost's own words instead of a model's. Nothing here comes from a model, an app or
//! the operator's words; the lines are constants, and the tests hold that.

use ghost_voice::pipeline::Greeting;

/// Said first, in every case.
pub const HELLO: &str = "Hi, I'm Ghost. Good to meet you.";
/// After the hello, when the operator said only the name and a model is connected.
pub const INVITE: &str = "I come forward whenever you say my name. What can I do for you?";
/// After the hello, when no model is connected: what is missing and where to give it.
pub const NEEDS_MODEL: &str = "I can hear you, but I need a model to think with before I can answer. It's the first step on your screen.";
/// Every turn of that session after the hello, while there is still no model: Ghost's own
/// words, never a failed call said in plain words.
pub const NEEDS_MODEL_AGAIN: &str =
    "I need a model to think with first. Give me one on your screen, then call me again.";

/// Whether the session opening now may take the moment: at this desk (never the phone's
/// call), with Ghost itself (not a call to an agent), and not one Ghost raised itself.
pub fn takes(phone: bool, start_in: &str, raised: bool) -> bool {
    !phone && !raised && start_in == ghost_core::conversations::GHOST
}

/// How the first meeting opens: the hello, then the invitation when the operator said only
/// the name, or the sentence about the model when there is none. With a request and a model,
/// the hello alone, and the model's reply to the request follows it.
pub fn greeting(model_ready: bool, has_request: bool) -> Greeting {
    let line = if !model_ready {
        format!("{HELLO} {NEEDS_MODEL}")
    } else if has_request {
        HELLO.to_string()
    } else {
        format!("{HELLO} {INVITE}")
    };
    Greeting::Intro(line)
}

/// The stand-in for a model a first meeting has without one: every turn answered with
/// [`NEEDS_MODEL_AGAIN`] at once. It reaches no network and calls no tool.
pub fn no_model() -> std::sync::Arc<dyn ghost_voice::llm::ModelPort> {
    use ghost_voice::llm::ScriptedModel;
    ScriptedModel::new(
        vec![vec![ScriptedModel::say(NEEDS_MODEL_AGAIN)]],
        std::time::Duration::ZERO,
    ) as std::sync::Arc<dyn ghost_voice::llm::ModelPort>
}

/// With no model, a request said with the name is not sent anywhere: the hello says what is
/// missing instead.
pub fn keeps_request(model_ready: bool) -> bool {
    model_ready
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(g: Greeting) -> String {
        match g {
            Greeting::Intro(t) => t,
            other => panic!("the first meeting opens with its own line, not {other:?}"),
        }
    }

    #[test]
    fn the_first_line_is_ghosts_own_in_every_case() {
        for (ready, request) in [(true, false), (true, true), (false, false), (false, true)] {
            let l = line(greeting(ready, request));
            assert!(l.starts_with(HELLO), "{l}");
        }
        assert_eq!(line(greeting(true, true)), HELLO);
        assert_eq!(line(greeting(true, false)), format!("{HELLO} {INVITE}"));
        // No model: one honest sentence, and where to give it one, request or not.
        assert_eq!(
            line(greeting(false, false)),
            format!("{HELLO} {NEEDS_MODEL}")
        );
        assert_eq!(
            line(greeting(false, true)),
            format!("{HELLO} {NEEDS_MODEL}")
        );
        assert!(!keeps_request(false));
        assert!(keeps_request(true));
    }

    #[test]
    fn only_a_session_with_ghost_at_this_desk_takes_it() {
        assert!(takes(false, "ghost", false));
        assert!(!takes(true, "ghost", false), "the phone's call");
        assert!(!takes(false, "accounting", false), "a call to an agent");
        assert!(
            !takes(false, "ghost", true),
            "a session Ghost raised itself"
        );
    }

    #[test]
    fn the_lines_are_short_enough_to_say_at_once_and_plain() {
        for l in [HELLO, INVITE, NEEDS_MODEL, NEEDS_MODEL_AGAIN] {
            assert!(l.len() < 120, "{l}");
            assert!(!l.contains('!'), "no exclamation marks: {l}");
            assert!(!l.to_lowercase().contains("mcp"), "{l}");
        }
    }
}
