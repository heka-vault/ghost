//! What a route answers when it refuses: the same `{"detail": ...}` body FastAPI sent, so
//! the desktop reads either daemon the same way. A 422 from a request body says where and
//! why, never what was sent: it may be a password (ADR 0036).

use axum::Json;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use utoipa::ToSchema;

/// One thing wrong with a request, as FastAPI says it: the kind, where, and why.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct ValidationError {
    pub loc: Vec<Value>,
    pub msg: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// pydantic's extra, never filled: Ghost's 422 says where and why only (ADR 0036).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<serde_json::Map<String, Value>>,
}

/// The 422 body: a list of what was wrong.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct HTTPValidationError {
    #[serde(default)]
    pub detail: Vec<ValidationError>,
}

/// A refusal: the status and a detail (text, or the validation errors).
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: StatusCode,
    pub detail: Value,
}

impl ApiError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail: Value::String(detail.into()),
        }
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(404, detail)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(409, detail)
    }

    pub fn locked(detail: impl Into<String>) -> Self {
        Self::new(423, detail)
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::new(503, detail)
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(500, detail)
    }

    /// A 422 with one plain message, as `HTTPException(422, msg)` answered.
    pub fn unprocessable(detail: impl Into<String>) -> Self {
        Self::new(422, detail)
    }

    /// A 422 for one field of the body, where and why only.
    pub fn invalid(loc: &[&str], msg: impl Into<String>, kind: &str) -> Self {
        Self::validation(vec![ValidationError {
            loc: loc
                .iter()
                .map(|s| Value::String((*s).to_string()))
                .collect(),
            msg: msg.into(),
            kind: kind.to_string(),
            input: None,
            ctx: None,
        }])
    }

    pub fn validation(errors: Vec<ValidationError>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: serde_json::to_value(errors).unwrap_or(Value::Null),
        }
    }

    /// The detail as text, when it is.
    pub fn text(&self) -> String {
        match &self.detail {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status.as_u16(), self.text())
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"detail": self.detail}))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// serde's messages quote what they found (`invalid type: string "sk-…", expected a map`);
/// the quoted part is dropped, so a password never comes back in a 422.
pub fn scrub(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut chars = msg.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for d in chars.by_ref() {
                    if d == '"' {
                        break;
                    }
                }
                // "invalid type: string \"x\", expected" -> "invalid type: string, expected"
                while out.ends_with(' ') {
                    out.pop();
                }
            }
            '`' => {
                for d in chars.by_ref() {
                    if d == '`' {
                        break;
                    }
                }
                while out.ends_with(' ') {
                    out.pop();
                }
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// A JSON body, read with the path to what was wrong (`["body", "password_value"]`) and a
/// message that never echoes the input. The one way a route reads a body.
pub struct Body<T>(pub T);

/// A JSON body that may be absent (`WakeRequest | None`): no body, or `null`, is `None`.
pub struct OptionalBody<T>(pub Option<T>);

fn body_error(path: &str, message: &str) -> ApiError {
    let mut loc = vec!["body".to_string()];
    if !path.is_empty() && path != "." {
        loc.extend(path.split('.').map(String::from));
    }
    let loc: Vec<&str> = loc.iter().map(String::as_str).collect();
    ApiError::invalid(&loc, scrub(message), "value_error")
}

/// What one body read gives: the parsed value, or the 422 for it.
fn parse_body<T: DeserializeOwned>(bytes: &[u8]) -> ApiResult<T> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    serde_path_to_error::deserialize(&mut de)
        .map_err(|e| body_error(&e.path().to_string(), &e.inner().to_string()))
}

async fn read_bytes(req: Request) -> ApiResult<axum::body::Bytes> {
    axum::body::Bytes::from_request(req, &())
        .await
        .map_err(|_| ApiError::invalid(&["body"], "the body could not be read", "value_error"))
}

impl<S, T> FromRequest<S> for Body<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bytes = read_bytes(req).await?;
        if bytes.is_empty() {
            return Err(ApiError::invalid(&["body"], "Field required", "missing"));
        }
        parse_body(&bytes).map(Body)
    }
}

impl<S, T> FromRequest<S> for OptionalBody<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bytes = read_bytes(req).await?;
        if bytes.is_empty() || bytes.as_ref() == b"null" {
            return Ok(OptionalBody(None));
        }
        parse_body(&bytes).map(|v| OptionalBody(Some(v)))
    }
}

/// `min_length`/`max_length` as pydantic checked a string field: a 422 that names the
/// field only.
pub fn check_len(loc: &[&str], value: &str, min: usize, max: usize) -> ApiResult<()> {
    let n = value.chars().count();
    if n < min {
        return Err(ApiError::invalid(
            loc,
            format!(
                "String should have at least {min} character{}",
                if min == 1 { "" } else { "s" }
            ),
            "string_too_short",
        ));
    }
    if n > max {
        return Err(ApiError::invalid(
            loc,
            format!(
                "String should have at most {max} character{}",
                if max == 1 { "" } else { "s" }
            ),
            "string_too_long",
        ));
    }
    Ok(())
}

/// `ge`/`le` on a number, as pydantic checked it.
pub fn check_range<N: PartialOrd + std::fmt::Display + Copy>(
    loc: &[&str],
    value: N,
    min: N,
    max: N,
) -> ApiResult<()> {
    if value < min {
        return Err(ApiError::invalid(
            loc,
            format!("Input should be greater than or equal to {min}"),
            "greater_than_equal",
        ));
    }
    if value > max {
        return Err(ApiError::invalid(
            loc,
            format!("Input should be less than or equal to {max}"),
            "less_than_equal",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_value_is_dropped_from_a_message() {
        assert_eq!(
            scrub("invalid type: string \"sk-typed-key\", expected a map"),
            "invalid type: string, expected a map"
        );
        assert_eq!(
            scrub("unknown variant `yolo`, expected one of `open`, `ask`"),
            "unknown variant, expected one of,"
        );
        assert_eq!(scrub("missing field password"), "missing field password");
    }

    #[test]
    fn a_body_error_says_where_and_why() {
        let e = parse_body::<crate::models::PasswordSet>(br#"{"value": {"nested": "ghp_x"}}"#)
            .unwrap_err();
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = &e.detail[0];
        assert_eq!(detail["loc"], json!(["body", "value"]));
        assert!(!detail["msg"].as_str().unwrap().contains("ghp_x"));
        assert_eq!(detail["type"], "value_error");
    }
}
