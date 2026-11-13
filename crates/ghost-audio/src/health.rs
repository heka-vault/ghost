//! Whether Ghost's sound output plays (issue #117): one fact for the machine, set when a
//! playback stream stalls (open, but nothing takes its sound) and cleared when one plays out
//! again. The daemon reads it into a note on the waiting list (Settings › Sound), so a turn
//! whose voice could not be played still answers on screen and the operator is told once.

use std::sync::{Arc, Mutex};

type Listener = Arc<dyn Fn() + Send + Sync>;

static OUTPUT: Mutex<Option<String>> = Mutex::new(None);
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

/// The output's problem, in words, while it has one.
pub fn output_problem() -> Option<String> {
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Set or clear it; the listeners hear a change, never a repeat.
pub fn set_output_problem(problem: Option<String>) {
    {
        let mut now = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
        if *now == problem {
            return;
        }
        *now = problem;
    }
    let listeners = LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for l in listeners {
        l();
    }
}

/// Called after every change.
pub fn on_output_change(f: Listener) {
    LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).push(f);
}

/// What the waiting list says while the output is stalled.
pub fn stalled_note(sink: &str) -> String {
    format!(
        "Ghost can't play sound: the output ({sink}) took nothing for a few seconds, so \
         nothing connects Ghost to a speaker. Its replies still show on screen. Settings › \
         Sound shows the speaker it uses."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_change_is_heard_once_and_a_repeat_not_at_all() {
        let heard = Arc::new(AtomicUsize::new(0));
        let h = heard.clone();
        on_output_change(Arc::new(move || {
            h.fetch_add(1, Ordering::SeqCst);
        }));
        set_output_problem(Some(stalled_note("default sink")));
        set_output_problem(Some(stalled_note("default sink")));
        assert!(
            output_problem()
                .unwrap()
                .contains("Its replies still show on screen")
        );
        set_output_problem(None);
        assert_eq!(output_problem(), None);
        assert_eq!(heard.load(Ordering::SeqCst), 2);
    }
}
