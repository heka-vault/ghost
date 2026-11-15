//! The device flow (RFC 8628), for Ghost's own client on a server that wants it (ADR 0076):
//! GitHub's web flow needs a client secret at the token endpoint and Ghost ships none, while
//! its device flow needs the client ID alone.
//!
//! The person is shown a short code and the service's own page; Ghost asks the token
//! endpoint, as often as the server allows, until the code is typed, refused, expired or the
//! sign-in replaced. What the server sends is untrusted: every answer is bounded, the code
//! must look like a code, and the page must be on the issuer's own host, so a server cannot
//! send the person anywhere else.

use std::time::{Duration, Instant};

use rmcp::transport::auth::OAuthTokenResponse;
use serde_json::Value;
use tracing::warn;

use super::SignIns;
use crate::apps::AppError;

/// An answer larger than this is not a device flow's.
const MAX_BODY: usize = 64 * 1024;
/// RFC 8628's default when the server names none.
const DEFAULT_INTERVAL: u64 = 5;
/// The longest Ghost waits for a code, whatever the server says.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// One device sign-in, from asking for a code to the tokens.
#[allow(clippy::too_many_arguments)]
pub(super) async fn sign_in(
    signins: &SignIns,
    app: &str,
    http: &reqwest::Client,
    device_endpoint: &str,
    issuer: Option<&str>,
    client_id: &str,
    scopes: &[String],
    token_endpoint: &str,
) -> Result<OAuthTokenResponse, AppError> {
    let mut form = vec![("client_id", client_id.to_string())];
    if !scopes.is_empty() {
        form.push(("scope", scopes.join(" ")));
    }
    let started = post(http, device_endpoint, &form).await?;
    let start = Started::read(&started, issuer)?;
    let id = signins.start_device_flow(app, &start.page, &start.user_code);
    let deadline = Instant::now() + Duration::from_secs(start.expires_in).min(MAX_WAIT);
    let mut interval = start.interval;
    let out = loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if !signins.flow_open(id) {
            break Err(AppError::NeedsSignIn("the sign-in was replaced".into()));
        }
        if Instant::now() >= deadline {
            break Err(AppError::NeedsSignIn(
                "the code expired before it was entered".into(),
            ));
        }
        let answer = post(
            http,
            token_endpoint,
            &[
                ("client_id", client_id.to_string()),
                ("device_code", start.device_code.clone()),
                ("grant_type", GRANT.to_string()),
            ],
        )
        .await?;
        match answer.get("error").and_then(Value::as_str) {
            None => match serde_json::from_value::<OAuthTokenResponse>(answer) {
                Ok(token) => break Ok(token),
                Err(_) => break Err(AppError::Connect("TokenExchangeFailed".into())),
            },
            Some("authorization_pending") => {}
            Some("slow_down") => interval = (interval + 5).min(60),
            Some("expired_token") => {
                break Err(AppError::NeedsSignIn(
                    "the code expired before it was entered".into(),
                ));
            }
            Some("access_denied") => {
                break Err(AppError::NeedsSignIn(
                    "the sign-in was refused (access_denied)".into(),
                ));
            }
            Some(other) => {
                let short: String = other
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .take(40)
                    .collect();
                warn!("sign-in: device flow: {short}");
                break Err(AppError::NeedsSignIn(format!(
                    "the sign-in was refused ({short})"
                )));
            }
        }
    };
    signins.end_flow(app).await;
    out
}

/// What the server said to show the person.
#[derive(Debug)]
struct Started {
    device_code: String,
    user_code: String,
    page: String,
    interval: u64,
    expires_in: u64,
}

impl Started {
    fn read(v: &Value, issuer: Option<&str>) -> Result<Self, AppError> {
        let bad = || AppError::Connect("DeviceFlowRefused".into());
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let device_code = text("device_code")
            .filter(|c| !c.is_empty() && c.len() <= 512)
            .ok_or_else(bad)?;
        let user_code = text("user_code")
            .filter(|c| looks_like_a_code(c))
            .ok_or_else(bad)?;
        let page = text("verification_uri")
            .filter(|p| on_issuers_host(p, issuer))
            .ok_or_else(bad)?;
        let number = |k: &str, default: u64| v.get(k).and_then(Value::as_u64).unwrap_or(default);
        Ok(Started {
            device_code,
            user_code,
            page,
            interval: number("interval", DEFAULT_INTERVAL).clamp(1, 60),
            expires_in: number("expires_in", 900).clamp(1, MAX_WAIT.as_secs()),
        })
    }
}

/// A code a person types: short, letters, digits and dashes.
fn looks_like_a_code(code: &str) -> bool {
    (4..=16).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ' ')
}

/// The page is on the issuer's own host, exactly, and https (plain http only for an issuer
/// on this computer's loopback, which is a test's).
fn on_issuers_host(page: &str, issuer: Option<&str>) -> bool {
    let (Ok(page), Some(Ok(issuer))) = (url::Url::parse(page), issuer.map(url::Url::parse)) else {
        return false;
    };
    let loopback = issuer.host_str() == Some("127.0.0.1") && issuer.scheme() == "http";
    let scheme_ok = page.scheme() == "https" || (loopback && page.scheme() == "http");
    scheme_ok && page.host_str().is_some() && page.host_str() == issuer.host_str()
}

/// A form POST with a JSON answer (GitHub answers form-encoded unless asked), bounded.
/// A 4xx with a JSON body is an answer too: RFC 8628's errors come that way.
async fn post(
    http: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, String)],
) -> Result<Value, AppError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().map(|(k, v)| (*k, v.as_str())))
        .finish();
    let response = http
        .post(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|_| AppError::Connect("HttpError".into()))?;
    let status = response.status();
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::Connect("HttpError".into()))?
    {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(AppError::Connect("DeviceFlowRefused".into()));
        }
        body.extend_from_slice(&chunk);
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(v @ Value::Object(_)) if status.is_success() || v.get("error").is_some() => Ok(v),
        _ => Err(AppError::Connect(format!(
            "DeviceFlow{}",
            if status.is_success() {
                "Refused".to_string()
            } else {
                status.as_u16().to_string()
            }
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_the_server_says_is_checked_before_the_person_sees_it() {
        let issuer = Some("https://github.com/login/oauth");
        let good = json!({"device_code": "d", "user_code": "WDJB-MJHT",
                          "verification_uri": "https://github.com/login/device",
                          "interval": 5, "expires_in": 900});
        let s = Started::read(&good, issuer).unwrap();
        assert_eq!((s.user_code.as_str(), s.interval), ("WDJB-MJHT", 5));
        for (k, v) in [
            (
                "verification_uri",
                json!("https://github.com.evil.example/login/device"),
            ),
            ("verification_uri", json!("http://github.com/login/device")),
            ("verification_uri", json!("https://evil.example/")),
            ("user_code", json!("<script>alert(1)</script>")),
            ("user_code", json!("x")),
            ("device_code", json!("")),
        ] {
            let mut bad = good.clone();
            bad[k] = v;
            assert!(Started::read(&bad, issuer).is_err(), "{k}: {bad}");
        }
        assert!(Started::read(&good, None).is_err());
        // a server can't make Ghost poll every millisecond, or wait for a day
        let mut fast = good.clone();
        fast["interval"] = json!(0);
        fast["expires_in"] = json!(86400);
        let s = Started::read(&fast, issuer).unwrap();
        assert_eq!(s.interval, 1);
        assert_eq!(s.expires_in, MAX_WAIT.as_secs());
    }
}
