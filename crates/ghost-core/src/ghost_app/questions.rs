//! Questions on Ghost's own app (ADR 0051): `ask`, a change, and `question` and
//! `questions_list`, reads, for whoever asks; `question_cancel` for taking one back; and
//! `answer`, a change only Ghost's own agents have, only in a live session with the
//! operator, and only with words the operator said in their last turn.
//!
//! Who asks is never an argument the model writes: the session's dispatcher sets `_agent`
//! and `_session`, and the listener for outside tools names the client. A question asked
//! from a run with nobody there (`_session` is `run-<id>`) does not hold the run: `wait_s`
//! may wait up to two minutes, and otherwise the answer starts one more run of the same
//! agent (`crate::questions`).
//!
//! `answer` takes the operator's words as they said them. The dispatcher sets `_heard` to
//! its session id only after it found the words in the operator's own last turn (the same
//! check `rules.add` makes with `_request`); a run, an outside tool and the model can never
//! set it, and the server refuses an answer without it.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::Duration;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::clients::{CLIENT_SOURCE, shown};
use super::{AGENT_SOURCE, Caller, GhostServer, reply};
use crate::conversations::GHOST;
use crate::questions::{Ask, Question, QuestionError, Questions, WAIT_MAX_SECS, via};
use crate::routes::Urgency;
use crate::schedules::is_run_session;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskArgs {
    /// The question, in one line.
    pub question: String,
    /// A sentence or two of context, if needed.
    #[serde(default)]
    pub detail: Option<String>,
    /// Up to four short answers to choose from; the operator may still answer in their own words.
    #[serde(default)]
    pub choices: Option<Vec<String>>,
    /// Only one of the choices will do.
    #[serde(default)]
    pub choices_only: Option<bool>,
    /// digest, notify (the default) or call.
    #[serde(default)]
    pub urgency: Option<String>,
    /// How many hours it stays open (24 unless given, a week at most).
    #[serde(default)]
    pub expires_hours: Option<f64>,
    /// Seconds to wait for the answer before returning (0 unless given, 120 at most). Not in a voice session.
    #[serde(default)]
    pub wait_s: Option<u64>,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
    #[serde(default, rename = "_session")]
    #[schemars(skip)]
    pub session: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuestionKey {
    /// The key ghost_ask returned.
    pub key: String,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuestionsList {
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnswerArgs {
    /// The question's key.
    pub key: String,
    /// The operator's answer, in their exact words from what they just said.
    pub text: String,
    #[serde(default, rename = "_agent")]
    #[schemars(skip)]
    pub caller: Option<String>,
    #[serde(default, rename = "_session")]
    #[schemars(skip)]
    pub session: Option<String>,
    #[serde(default, rename = "_heard")]
    #[schemars(skip)]
    pub heard: Option<String>,
}

fn said(e: QuestionError) -> String {
    e.to_string()
}

impl GhostServer {
    /// The same server with the questions, and the tools that go with them.
    pub fn with_questions(mut self, questions: Arc<Questions>) -> Self {
        self.questions = Some(questions);
        self.tool_router.merge(Self::questions_router());
        self
    }

    fn questions(&self) -> Result<&Arc<Questions>, String> {
        self.questions
            .as_ref()
            .ok_or_else(|| "questions are not available here".to_string())
    }

    /// `agent:<slug>` or `client:<name>`, and the name the operator knows it by.
    fn asker(&self, said: Option<String>) -> Result<(String, String), String> {
        let source = self.agent(said)?;
        let who = match source.strip_prefix(CLIENT_SOURCE) {
            Some(name) => shown(name),
            None => {
                let slug = source.strip_prefix(AGENT_SOURCE).unwrap_or(&source);
                self.parts
                    .store
                    .get_conversation(slug, false)
                    .ok()
                    .flatten()
                    .map(|p| p.name)
                    .unwrap_or_else(|| shown(slug))
            }
        };
        Ok((source, who))
    }

    /// Ghost (agent zero) reads every question; any other asker its own.
    fn reads_all(source: &str) -> bool {
        source == format!("{AGENT_SOURCE}{GHOST}")
    }

    async fn do_ask(&self, p: AskArgs) -> Result<Value, String> {
        let questions = self.questions()?.clone();
        let (source, who) = self.asker(p.caller.clone())?;
        let client = source.starts_with(CLIENT_SOURCE);
        // In a session the dispatcher names it; from outside there is none.
        let session = match &self.caller {
            Caller::Named(_) => None,
            Caller::Session => p.session.clone().filter(|s| !s.is_empty()),
        };
        let in_run = session.as_deref().is_some_and(is_run_session);
        let wait = if client || in_run {
            p.wait_s.unwrap_or(0).min(WAIT_MAX_SECS)
        } else {
            0 // a voice session asks aloud; this is for an answer that comes later
        };
        let urgency = match p
            .urgency
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            None => Urgency::Notify,
            Some(u) => Urgency::parse(&u.to_lowercase())
                .ok_or_else(|| format!("urgency is digest, notify or call, not {u}"))?,
        };
        let expires_in = match p.expires_hours {
            None => None,
            Some(h) if h.is_finite() && h > 0.0 => Some(Duration::seconds((h * 3600.0) as i64)),
            Some(_) => return Err("expires_hours is a number of hours".into()),
        };
        let mut ask = Ask::new(source.clone(), who, p.question);
        ask.agent = source.strip_prefix(AGENT_SOURCE).map(String::from);
        ask.asked_in = session.clone();
        // From a run that does not wait, the answer starts the next run at once.
        ask.follow_up = in_run && wait == 0;
        ask.detail = p.detail;
        ask.choices = p.choices.unwrap_or_default();
        ask.choices_only = p.choices_only.unwrap_or(false);
        ask.urgency = urgency;
        ask.expires_in = expires_in;
        let q = questions.ask(ask, None).map_err(said)?;
        if wait > 0 {
            let got = questions
                .wait(&q.key, StdDuration::from_secs(wait))
                .await
                .map_err(said)?;
            if !got.is_open() {
                return Ok(Value::Object(got.fields()));
            }
            if in_run && !questions.arm_follow_up(&q.key).map_err(said)? {
                // Answered between the wait and now: it is the answer.
                if let Some(got) = questions.get(&q.key).map_err(said)? {
                    return Ok(Value::Object(got.fields()));
                }
            }
        }
        let note = if in_run {
            "Asked; it is on the operator's waiting list. Do not wait for it: finish this run \
             now and answer exactly [SILENT] unless there is something else to tell. When \
             they answer, you are run again with the question and their answer."
        } else if client {
            "Asked; it is on the operator's waiting list. events_wait says when it is \
             answered, and question reads the answer."
        } else {
            "Asked; it is on the operator's waiting list and reaches them the way their \
             routes say. If they are here, ask them aloud as well."
        };
        let mut out = q.fields();
        out.insert("note".into(), json!(note));
        Ok(Value::Object(out))
    }

    fn visible(&self, source: &str, q: &Question) -> bool {
        Self::reads_all(source) || q.asker == source
    }

    fn do_question(&self, p: QuestionKey) -> Result<Value, String> {
        let questions = self.questions()?;
        let (source, _) = self.asker(p.caller)?;
        let key = p.key.trim();
        questions.expire(None).map_err(said)?;
        match questions.get(key).map_err(said)? {
            Some(q) if self.visible(&source, &q) => Ok(Value::Object(q.fields())),
            // Another asker's is not there for this one: its words are that asker's.
            _ => Err(format!("there is no question {key} that you asked")),
        }
    }

    fn do_questions_list(&self, p: QuestionsList) -> Result<Value, String> {
        let questions = self.questions()?;
        let (source, _) = self.asker(p.caller)?;
        let all = Self::reads_all(&source);
        let rows = questions
            .recent((!all).then_some(source.as_str()), None, 50)
            .map_err(said)?;
        let list: Vec<Value> = rows.iter().map(|q| Value::Object(q.fields())).collect();
        Ok(json!({"questions": list}))
    }

    fn do_question_cancel(&self, p: QuestionKey) -> Result<Value, String> {
        let questions = self.questions()?;
        let (source, _) = self.asker(p.caller)?;
        let q = questions
            .cancel(p.key.trim(), &source, None)
            .map_err(|e| match e {
                QuestionError::NotFound(k) => format!("there is no question {k} that you asked"),
                other => said(other),
            })?;
        Ok(json!({"key": q.key, "state": q.state.as_str()}))
    }

    fn do_answer(&self, p: AnswerArgs) -> Result<Value, String> {
        let questions = self.questions()?;
        // Only in process, only in a live session, only with the operator's own words.
        if !matches!(self.caller, Caller::Session) {
            return Err("answer is for Ghost's own agents, in a session with the operator".into());
        }
        let source = self.agent(p.caller)?;
        if source.starts_with(CLIENT_SOURCE) {
            return Err("an outside tool cannot answer for the operator".into());
        }
        let session = p.session.unwrap_or_default();
        if session.is_empty() || is_run_session(&session) {
            return Err(
                "nobody is here to answer: a question is answered by the operator, in a \
                 session or on their waiting list"
                    .into(),
            );
        }
        if p.heard.as_deref() != Some(session.as_str()) {
            return Err(
                "that answer is not in what the operator just said; use their exact words, \
                 or ask them"
                    .into(),
            );
        }
        let key = p.key.trim();
        if key.starts_with(crate::waiting::change_key("").as_str()) {
            return Err(
                "that is a change waiting for a yes: take it up with bring_up_change".into(),
            );
        }
        let q = questions
            .get(key)
            .map_err(said)?
            .ok_or_else(|| format!("there is no question {key}"))?;
        if q.asked_in.as_deref() == Some(session.as_str()) && q.asker == source {
            return Err(
                "you asked this in this session: their reply is already in the conversation".into(),
            );
        }
        let q = questions
            .answer(key, &p.text, via::VOICE, None)
            .map_err(said)?;
        Ok(json!({
            "key": q.key,
            "state": q.state.as_str(),
            "answer": q.answer,
            "choice": q.choice,
            "status": "recorded as the operator said it",
        }))
    }
}

#[tool_router(router = questions_router, vis = "pub(super)")]
impl GhostServer {
    #[tool(
        description = "Ask the operator a question whose answer may come later: it goes on their waiting list and reaches them by their routes. Returns its key. choices: up to four short answers. From a run with nobody there, do not wait: your answer to the run is [SILENT], and you are run again with their answer."
    )]
    async fn ask(&self, Parameters(p): Parameters<AskArgs>) -> CallToolResult {
        reply(self.do_ask(p).await)
    }

    #[tool(
        description = "A question you asked: whether it is open, answered, declined or expired, and the operator's answer.",
        annotations(read_only_hint = true)
    )]
    fn question(&self, Parameters(p): Parameters<QuestionKey>) -> CallToolResult {
        reply(self.do_question(p))
    }

    #[tool(
        description = "The questions you asked that are open or were settled in the last day, with the answers.",
        annotations(read_only_hint = true)
    )]
    fn questions_list(&self, Parameters(p): Parameters<QuestionsList>) -> CallToolResult {
        reply(self.do_questions_list(p))
    }

    #[tool(description = "Take back a question you asked that is still open.")]
    fn question_cancel(&self, Parameters(p): Parameters<QuestionKey>) -> CallToolResult {
        reply(self.do_question_cancel(p))
    }

    #[tool(
        description = "Record the operator's answer to a question on their waiting list, in their exact words from what they just said. Only their words count."
    )]
    fn answer(&self, Parameters(p): Parameters<AnswerArgs>) -> CallToolResult {
        reply(self.do_answer(p))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::Map;

    use super::*;
    use crate::apps::{Apps, CALLER_ARG, HEARD_ARG, SESSION_ARG};
    use crate::conversations::models::Profile;
    use crate::gate::{Request, WriteGate};
    use crate::ghost_app::{Attached, NAME, Parts, attach};
    use crate::models::OutcomeStatus;
    use crate::questions::QState;
    use crate::store::Store;
    use crate::waiting::{Items, WaitingList};

    struct World {
        apps: Arc<Apps>,
        questions: Arc<Questions>,
        gate: WriteGate,
        _attached: Attached,
    }

    async fn world() -> World {
        let store = Arc::new(Store::in_memory().unwrap());
        for (slug, name) in [("ghost", "Ghost"), ("ledger", "Ledger"), ("brand", "Brand")] {
            store
                .upsert_conversation(&Profile::new(slug, name))
                .unwrap();
        }
        let apps = Arc::new(Apps::new(
            Vec::new(),
            Arc::new(HashMap::<String, String>::new()),
            None,
        ));
        let waiting = Arc::new(WaitingList::new(Items::new(store.clone())));
        let questions = Arc::new(Questions::new(store.clone(), waiting.clone()));
        let server = GhostServer::new(Parts {
            store: store.clone(),
            waiting,
            apps: Arc::downgrade(&apps),
            status: Arc::new(Map::new),
        })
        .with_questions(questions.clone());
        let attached = attach(&apps, server).await.unwrap();
        let gate = WriteGate::new(store);
        gate.register(NAME, attached.app.operations());
        World {
            apps,
            questions,
            gate,
            _attached: attached,
        }
    }

    /// A change as the session's dispatcher submits it: the agent, the session, and
    /// `_heard` only when given (the dispatcher's own check passed).
    async fn change(
        w: &World,
        agent: &str,
        session: &str,
        heard: Option<&str>,
        tool: &str,
        args: Value,
    ) -> Result<Value, String> {
        let op = w.gate.operation(NAME, tool).unwrap();
        let mut args = args.as_object().cloned().unwrap();
        args.insert(CALLER_ARG.into(), json!(agent));
        args.insert(SESSION_ARG.into(), json!(session));
        if let Some(h) = heard {
            args.insert(HEARD_ARG.into(), json!(h));
        }
        let prepared = (op.prepare)(args).await.map_err(|e| e.to_string())?;
        let mut req = Request::new(session, NAME, tool, prepared.target, prepared.changes, "t");
        req.conversation = Some(agent.to_string());
        let sub = w.gate.submit(req).await.unwrap();
        let outcome = sub.outcome.unwrap();
        match outcome.status {
            OutcomeStatus::Success => match outcome.readback {
                Some(crate::models::Snapshot::App(a)) => Ok(Value::Object(a.result.unwrap())),
                _ => Ok(Value::Null),
            },
            _ => Err(outcome.error.unwrap_or_default()),
        }
    }

    async fn read(w: &World, agent: &str, tool: &str, args: Value) -> Map<String, Value> {
        let src = w
            .apps
            .sources()
            .into_iter()
            .find(|s| s.name == NAME)
            .unwrap();
        let t = src.read_tools.iter().find(|t| t.name == tool).unwrap();
        let mut args = args.as_object().cloned().unwrap();
        args.insert(CALLER_ARG.into(), json!(agent));
        (t.handler)(args).await.unwrap()
    }

    fn from_ledger(text: &str, session: &str) -> Ask {
        let mut a = Ask::new("agent:ledger", "Ledger", text);
        a.agent = Some("ledger".into());
        a.asked_in = Some(session.into());
        a.choices = vec!["Yes".into(), "No".into()];
        a
    }

    #[tokio::test]
    async fn an_agent_asks_and_reads_only_its_own_and_ghost_reads_all() {
        let w = world().await;
        let asked = change(
            &w,
            "ledger",
            "s1",
            None,
            "ask",
            json!({"question": "Should the October invoice go out today", "choices": ["Yes", "No"]}),
        )
        .await
        .unwrap();
        let key = asked["key"].as_str().unwrap().to_string();
        assert_eq!(asked["state"], "open");
        assert_eq!(asked["asked_by"], "Ledger");
        let mine = read(&w, "ledger", "ghost_question", json!({"key": key})).await;
        assert_eq!(mine["question"], "Should the October invoice go out today");
        // Another agent: not there for it.
        let other = read(&w, "brand", "ghost_question", json!({"key": key})).await;
        assert!(other["error"].as_str().unwrap().contains("that you asked"));
        let list = read(&w, "brand", "ghost_questions_list", json!({})).await;
        assert_eq!(list["questions"], json!([]));
        // Ghost, agent zero, reads every question.
        let all = read(&w, "ghost", "ghost_questions_list", json!({})).await;
        assert_eq!(all["questions"].as_array().unwrap().len(), 1);
        // Only who asked takes it back.
        let refused = change(
            &w,
            "brand",
            "s1",
            None,
            "question_cancel",
            json!({"key": key}),
        )
        .await;
        assert!(refused.is_err());
        let gone = change(
            &w,
            "ledger",
            "s1",
            None,
            "question_cancel",
            json!({"key": key}),
        )
        .await
        .unwrap();
        assert_eq!(gone["state"], "cancelled");
    }

    #[tokio::test]
    async fn answer_takes_only_the_operators_words_in_a_live_session() {
        let w = world().await;
        let asked = w
            .questions
            .ask(from_ledger("Send it today?", "run-r1"), None)
            .unwrap();
        let args = json!({"key": asked.key, "text": "yes, send it"});
        // In a run there is nobody to answer.
        let e = change(
            &w,
            "ghost",
            "run-r2",
            Some("run-r2"),
            "answer",
            args.clone(),
        )
        .await
        .unwrap_err();
        assert!(e.contains("nobody is here"), "{e}");
        // The dispatcher did not find the words in the operator's last turn: no _heard.
        let e = change(&w, "ghost", "s9", None, "answer", args.clone())
            .await
            .unwrap_err();
        assert!(e.contains("not in what the operator just said"), "{e}");
        // A _heard for another session is not this one's.
        let e = change(&w, "ghost", "s9", Some("s8"), "answer", args.clone())
            .await
            .unwrap_err();
        assert!(e.contains("not in what the operator just said"), "{e}");
        assert!(w.questions.get(&asked.key).unwrap().unwrap().is_open());
        // The agent that asked it in this very session does not answer it.
        let mine = w
            .questions
            .ask(from_ledger("Send it now?", "s9"), None)
            .unwrap();
        let e = change(
            &w,
            "ledger",
            "s9",
            Some("s9"),
            "answer",
            json!({"key": mine.key, "text": "yes"}),
        )
        .await
        .unwrap_err();
        assert!(e.contains("you asked this in this session"), "{e}");
        // Heard in this session: recorded as said, the choice noted.
        let done = change(&w, "ghost", "s9", Some("s9"), "answer", args)
            .await
            .unwrap();
        assert_eq!(done["answer"], "yes, send it");
        assert_eq!(done["choice"], "Yes");
        let q = w.questions.get(&asked.key).unwrap().unwrap();
        assert_eq!(q.state, QState::Answered);
        assert_eq!(q.answered_via.as_deref(), Some("voice"));
    }

    #[tokio::test]
    async fn a_question_asked_from_a_run_does_not_hold_it_and_is_followed_up() {
        let w = world().await;
        let out = change(
            &w,
            "ledger",
            "run-r1",
            None,
            "ask",
            json!({"question": "Which account?"}),
        )
        .await
        .unwrap();
        assert!(out["note"].as_str().unwrap().contains("[SILENT]"));
        let q = w
            .questions
            .get(out["key"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert!(q.follow_up);
        assert_eq!(q.asked_in.as_deref(), Some("run-r1"));
        // Asked in a session, it is not followed up, and does not wait.
        let out = change(
            &w,
            "ledger",
            "s1",
            None,
            "ask",
            json!({"question": "Later?", "wait_s": 30}),
        )
        .await
        .unwrap();
        let q = w
            .questions
            .get(out["key"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert!(!q.follow_up && q.is_open());
    }

    #[tokio::test]
    async fn a_run_that_waits_gets_the_answer_back_and_no_follow_up() {
        let w = Arc::new(world().await);
        let asking = tokio::spawn({
            let w = w.clone();
            async move {
                change(
                    &w,
                    "ledger",
                    "run-r1",
                    None,
                    "ask",
                    json!({"question": "Which account?", "wait_s": 10}),
                )
                .await
                .unwrap()
            }
        });
        let key = loop {
            if let Some(q) = w.questions.open(None).unwrap().pop() {
                break q.key;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        w.questions
            .answer(&key, "the business one", via::DESKTOP, None)
            .unwrap();
        let got = asking.await.unwrap();
        assert_eq!(got["state"], "answered");
        assert_eq!(got["answer"], "the business one");
        assert!(!w.questions.get(&key).unwrap().unwrap().follow_up);
    }
}
