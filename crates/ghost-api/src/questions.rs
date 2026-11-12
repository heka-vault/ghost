//! Questions for the operator (ADR 0051), as the desktop answers them.
//!
//! `GET /questions` lists the open questions and the ones settled in the last day, with
//! the changes waiting for a yes shown as the questions they are (Yes or No, ADR 0030).
//! `POST /questions/answer` takes the operator's answer, in their words or as one of the
//! choices, recorded with the route `desktop`; `POST /questions/decline` declines one. A
//! change answered Yes is confirmed by id as the Confirm button does (`classifier: ui`), and
//! No cancels it: nothing about what counts as a yes changes. The model has no route here.

use axum::Json;
use chrono::Utc;
use ghost_core::questions::{QState, Question, QuestionError, change_question, match_choice, via};
use ghost_core::routes::Urgency as CoreUrgency;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::outcome_view;
use crate::deps::{Deps, SharedDeps};
use crate::error::{ApiError, ApiResult, Body, HTTPValidationError, check_len};
use crate::models::{
    ItemKey, QuestionAnswer, QuestionKind, QuestionResult, QuestionState, QuestionView,
    QuestionsView, Urgency,
};
use axum::extract::State;

/// A question as the desktop, the event stream and the phone see it.
pub fn question_view(q: &Question) -> QuestionView {
    QuestionView {
        key: q.key.clone(),
        kind: if q.is_change() {
            QuestionKind::Change
        } else {
            QuestionKind::Question
        },
        asked_by: q.who.clone(),
        source: q.asker.clone(),
        question: q.text.clone(),
        detail: q.detail.clone(),
        choices: q.choices.clone(),
        choices_only: q.choices_only,
        urgency: match q.urgency {
            CoreUrgency::Digest => Urgency::Digest,
            CoreUrgency::Notify => Urgency::Notify,
            CoreUrgency::Call => Urgency::Call,
        },
        asked_at: q.created_at,
        expires_at: q.expires_at,
        state: match q.state {
            QState::Open => QuestionState::Open,
            QState::Answered => QuestionState::Answered,
            QState::Declined => QuestionState::Declined,
            QState::Expired => QuestionState::Expired,
            QState::Cancelled => QuestionState::Cancelled,
        },
        answer: q.answer.clone(),
        choice: q.choice.clone(),
        answered_via: q.answered_via.clone(),
        answered_at: q.answered_at,
        change_id: q.change.clone(),
    }
}

/// The question an item on the waiting list is, if it is one: a question, or a change
/// waiting for a yes.
pub fn question_for(deps: &Deps, key: &str) -> Option<Question> {
    if let Some(id) = key.strip_prefix("change:") {
        return deps
            .gate
            .waiting_one(id, None)
            .ok()
            .flatten()
            .map(|p| change_question(&p));
    }
    deps.questions.as_ref()?.get(key).ok().flatten()
}

/// Open first (oldest first), then settled in the last day (newest first); the changes
/// waiting for a yes among the open.
pub fn questions_view(deps: &Deps) -> ApiResult<QuestionsView> {
    let mut open: Vec<Question> = deps
        .gate
        .waiting(None)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .iter()
        .map(change_question)
        .collect();
    let mut settled = Vec::new();
    if let Some(qs) = &deps.questions {
        for q in qs
            .recent(None, None, 100)
            .map_err(|e| ApiError::internal(e.to_string()))?
        {
            if q.is_open() {
                open.push(q);
            } else {
                settled.push(q);
            }
        }
    }
    open.sort_by_key(|q| q.created_at);
    Ok(QuestionsView {
        questions: open
            .iter()
            .chain(settled.iter())
            .map(question_view)
            .collect(),
    })
}

fn refused(e: QuestionError) -> ApiError {
    match e {
        QuestionError::NotFound(_) => ApiError::not_found("there is no such question"),
        QuestionError::NotOpen(state) => {
            ApiError::conflict(format!("that question was {state} already"))
        }
        QuestionError::Invalid(why) => ApiError::unprocessable(why),
        other => ApiError::internal(other.to_string()),
    }
}

#[utoipa::path(get, path = "/questions", operation_id = "questions_questions_get", tag = "waiting",
    summary = "Questions",
    description = "The open questions for the operator, oldest first, with the changes waiting for a yes as questions; then the ones settled in the last day.",
    responses((status = 200, description = "Successful Response", body = QuestionsView)))]
async fn questions(State(deps): State<SharedDeps>) -> ApiResult<Json<QuestionsView>> {
    Ok(Json(questions_view(&deps)?))
}

/// The operator's answer from the desktop: their words, or a choice as it is written.
#[utoipa::path(post, path = "/questions/answer", operation_id = "answer_questions_answer_post",
    tag = "waiting", summary = "Answer",
    description = "The operator's answer, recorded as they gave it with the route desktop. For a change waiting for a yes, Yes confirms it (classifier ui) and No cancels it.",
    request_body = QuestionAnswer,
    responses((status = 200, description = "Successful Response", body = QuestionResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn answer(
    State(deps): State<SharedDeps>,
    Body(body): Body<QuestionAnswer>,
) -> ApiResult<Json<QuestionResult>> {
    check_len(&["body", "key"], &body.key, 1, 300)?;
    check_len(&["body", "text"], &body.text, 1, 2000)?;
    if let Some(id) = body.key.strip_prefix("change:") {
        let yes_no = ["Yes".to_string(), "No".to_string()];
        return match match_choice(&body.text, &yes_no).as_deref() {
            Some("Yes") => {
                let outcome = deps
                    .control
                    .confirm_proposal(id)
                    .await
                    .ok_or_else(|| ApiError::conflict("that change isn't waiting"))?;
                Ok(Json(QuestionResult {
                    ok: true,
                    detail: None,
                    question: None,
                    outcome: Some(outcome_view(&outcome)),
                }))
            }
            Some(_) => {
                let ok = deps.control.cancel_proposal(id).await;
                Ok(Json(QuestionResult {
                    ok,
                    detail: (!ok).then(|| "that change isn't waiting".to_string()),
                    question: None,
                    outcome: None,
                }))
            }
            None => Err(ApiError::unprocessable("a change is answered Yes or No")),
        };
    }
    let qs = deps
        .questions
        .as_ref()
        .ok_or_else(|| ApiError::conflict("questions are not available"))?;
    let q = qs
        .answer(&body.key, &body.text, via::DESKTOP, Some(Utc::now()))
        .map_err(refused)?;
    Ok(Json(QuestionResult {
        ok: true,
        detail: None,
        question: Some(question_view(&q)),
        outcome: None,
    }))
}

#[utoipa::path(post, path = "/questions/decline", operation_id = "decline_questions_decline_post",
    tag = "waiting", summary = "Decline",
    description = "The operator will not answer it. A change waiting for a yes is cancelled.",
    request_body = ItemKey,
    responses((status = 200, description = "Successful Response", body = QuestionResult),
              (status = 422, description = "Validation Error", body = HTTPValidationError)))]
async fn decline(
    State(deps): State<SharedDeps>,
    Body(body): Body<ItemKey>,
) -> ApiResult<Json<QuestionResult>> {
    check_len(&["body", "key"], &body.key, 1, 300)?;
    if let Some(id) = body.key.strip_prefix("change:") {
        let ok = deps.control.cancel_proposal(id).await;
        return Ok(Json(QuestionResult {
            ok,
            detail: (!ok).then(|| "that change isn't waiting".to_string()),
            question: None,
            outcome: None,
        }));
    }
    let qs = deps
        .questions
        .as_ref()
        .ok_or_else(|| ApiError::conflict("questions are not available"))?;
    let q = qs.decline(&body.key, via::DESKTOP, None).map_err(refused)?;
    Ok(Json(QuestionResult {
        ok: true,
        detail: None,
        question: Some(question_view(&q)),
        outcome: None,
    }))
}

pub fn router() -> OpenApiRouter<SharedDeps> {
    OpenApiRouter::new()
        .routes(routes!(questions))
        .routes(routes!(answer))
        .routes(routes!(decline))
}
