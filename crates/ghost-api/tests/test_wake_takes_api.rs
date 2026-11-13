//! Kept wake takes (ADR 0065): the list carries no audio, the choice and labels go through,
//! deleting one and deleting all really delete. A fake port over a real keeper in a temp
//! folder; no audio device.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::World;
use ghost_api::wake_takes::{
    WakeTakeLabel, WakeTakeOutcome, WakeTakeView, WakeTakesChoice, WakeTakesError, WakeTakesPort,
    WakeTakesResult, WakeTakesView,
};
use ghost_core::wake_takes::{Choice, Label, NewTake, Outcome, Takes};
use serde_json::json;

struct Port {
    takes: Takes,
    played: Mutex<Vec<(String, bool)>>,
}

fn view(m: ghost_core::wake_takes::TakeMeta) -> WakeTakeView {
    WakeTakeView {
        id: m.id,
        at: ghost_core::time::iso(&m.at),
        outcome: match m.outcome {
            Outcome::Accepted => WakeTakeOutcome::Accepted,
            Outcome::Refused => WakeTakeOutcome::Refused,
        },
        model: m.model,
        score: m.score,
        threshold: m.threshold,
        input: m.input,
        array: m.array,
        secs: m.secs,
        hit_at: m.hit_at,
        scores: m.scores,
        scores_end: m.scores_end,
        reading: m.reading,
        label: m.label.map(|l| match l {
            Label::Me => WakeTakeLabel::Me,
            Label::Not => WakeTakeLabel::Not,
            Label::MeMissedBefore => WakeTakeLabel::MeMissedBefore,
        }),
        bytes: m.bytes,
    }
}

fn nf(e: std::io::Error) -> WakeTakesError {
    WakeTakesError::NotFound(e.to_string())
}

#[async_trait]
impl WakeTakesPort for Port {
    fn view(&self) -> WakeTakesView {
        let c = self.takes.choice();
        let list = self.takes.list();
        WakeTakesView {
            choice: WakeTakesChoice {
                keep: c.keep,
                unlabelled_accepted_are_me: c.unlabelled_accepted_are_me,
            },
            count: list.len(),
            bytes: list.iter().map(|t| t.bytes).sum(),
            max_count: 2000,
            max_bytes: 1 << 30,
            playing: None,
            takes: list.into_iter().map(view).collect(),
        }
    }
    fn set_choice(&self, c: WakeTakesChoice) -> WakeTakesResult<WakeTakesView> {
        self.takes
            .set_choice(Choice {
                keep: c.keep,
                unlabelled_accepted_are_me: c.unlabelled_accepted_are_me,
            })
            .unwrap();
        Ok(self.view())
    }
    fn label(&self, id: &str, l: Option<WakeTakeLabel>) -> WakeTakesResult<WakeTakeView> {
        let l = l.map(|l| match l {
            WakeTakeLabel::Me => Label::Me,
            WakeTakeLabel::Not => Label::Not,
            WakeTakeLabel::MeMissedBefore => Label::MeMissedBefore,
        });
        self.takes.label(id, l).map(view).map_err(nf)
    }
    fn delete(&self, id: &str) -> WakeTakesResult<()> {
        match self.takes.delete(id) {
            Ok(true) => Ok(()),
            _ => Err(WakeTakesError::NotFound("no such take".into())),
        }
    }
    fn delete_all(&self) -> WakeTakesResult<usize> {
        Ok(self.takes.delete_all().unwrap())
    }
    async fn play(&self, id: &str, array: bool) -> WakeTakesResult<()> {
        self.takes.audio(id, array).map_err(nf)?;
        self.played.lock().unwrap().push((id.to_string(), array));
        Ok(())
    }
    async fn stop_playing(&self) {}
}

fn take(accepted: bool) -> NewTake {
    NewTake {
        audio: vec![3; 16_000],
        hit: 8_000,
        accepted,
        model: "hey_ghost".into(),
        score: 0.42,
        threshold: 0.6,
        input: "ghost.mic".into(),
        reading: Some("A ghost.".into()),
        scores: vec![0.1, 0.42],
        scores_end: 8_000,
        array: None,
    }
}

#[tokio::test]
async fn the_takes_are_listed_labelled_played_and_deleted_without_audio() {
    let mut w = World::new();
    let port = Arc::new(Port {
        takes: Takes::new(&w.path().join("data")),
        played: Mutex::new(Vec::new()),
    });
    let a = port.takes.keep(take(true)).unwrap().unwrap();
    let r = port.takes.keep(take(false)).unwrap().unwrap();
    w.wake_takes = Some(port.clone());
    let c = w.client();

    let got = c.get("/wake/takes").await;
    assert_eq!(got.status, 200, "{}", got.text);
    assert_eq!(got["choice"]["keep"], true);
    assert_eq!(got["count"], 2);
    let first = &got["takes"][0];
    assert_eq!(first["reading"], "A ghost.");
    // metadata only: no field carries samples
    assert!(!got.text.contains("audio"));
    assert!(got.text.len() < 4_000);

    let l = c
        .put(
            &format!("/wake/takes/{}/label", r.id),
            json!({"label": "me"}),
        )
        .await;
    assert_eq!(l.status, 200, "{}", l.text);
    assert_eq!(l["label"], "me");
    assert_eq!(port.takes.get(&r.id).unwrap().label, Some(Label::Me));
    let l = c
        .put(
            &format!("/wake/takes/{}/label", r.id),
            json!({"label": null}),
        )
        .await;
    assert_eq!(l["label"], serde_json::Value::Null);
    assert_eq!(
        c.put("/wake/takes/nope/label", json!({"label": "not"}))
            .await
            .status,
        404
    );
    assert_eq!(
        c.put(
            &format!("/wake/takes/{}/label", r.id),
            json!({"label": "maybe"})
        )
        .await
        .status,
        422
    );

    let p = c.post(&format!("/wake/takes/{}/play", a.id), None).await;
    assert_eq!(p.status, 200, "{}", p.text);
    assert_eq!(port.played.lock().unwrap()[0], (a.id.clone(), false));

    assert_eq!(c.delete(&format!("/wake/takes/{}", a.id)).await.status, 200);
    assert_eq!(c.delete(&format!("/wake/takes/{}", a.id)).await.status, 404);

    let off = c
        .put(
            "/wake/takes/choice",
            json!({"keep": false, "unlabelled_accepted_are_me": false}),
        )
        .await;
    assert_eq!(off.status, 200, "{}", off.text);
    assert!(!port.takes.wants());

    let all = c.delete("/wake/takes").await;
    assert_eq!(all["deleted"], 1);
    assert!(!w.path().join("data/wake/takes").exists());
    assert_eq!(c.get("/wake/takes").await["count"], 0);
}

#[tokio::test]
async fn without_a_keeper_the_routes_say_so() {
    let w = World::new();
    assert_eq!(w.client().get("/wake/takes").await.status, 503);
}
