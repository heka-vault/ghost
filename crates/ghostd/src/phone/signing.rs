//! Request and response signatures for the phone listener (docs/phone-protocol.md).
//!
//! Requests are HMAC-SHA256 under the device secret over:
//!
//! ```text
//! ghost-v1 / METHOD / path?query / time / nonce / sha256(body)
//! ```
//!
//! and every response to a signed request is signed back over:
//!
//! ```text
//! ghost-v1-response / status / request nonce / time / sha256(body)
//! ```
//!
//! each joined by newlines. Signatures are base64url without padding. The secret is held
//! zeroized and never printed; a signature or a nonce is never logged either.

use std::collections::HashMap;
use std::fmt;
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use hmac::{Hmac, Mac};
use rand::RngCore;
use regex::Regex;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{Clock, mono, wall};

pub const REQUEST_PREFIX: &str = "ghost-v1";
pub const RESPONSE_PREFIX: &str = "ghost-v1-response";
pub const CLOCK_WINDOW_SECS: f64 = 60.0;
pub const NONCE_TTL_SECS: f64 = 120.0;
/// The nonce cache never grows past this: only a verified request adds to it, and the one
/// paired phone sends a few a minute, so the bound is a backstop, not a budget.
pub const NONCE_CACHE_MAX: usize = 10_000;

pub const H_DEVICE: &str = "x-ghost-device";
pub const H_TIME: &str = "x-ghost-time";
pub const H_NONCE: &str = "x-ghost-nonce";
pub const H_SIGNATURE: &str = "x-ghost-signature";

static NONCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{32}$").unwrap());

/// base64url, no padding on the way out; padding or none, and loose trailing bits, on
/// the way in, as Python's `urlsafe_b64decode` read it.
const ENGINE: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

pub fn b64url(data: &[u8]) -> String {
    ENGINE.encode(data)
}

pub fn b64url_decode(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    ENGINE.decode(text).ok().map(Zeroizing::new)
}

/// A device secret as the phone and `phone.json` hold it: 32 random bytes, base64url
/// without padding. Zeroized when dropped; `Debug` never shows it.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// 32 random bytes, base64url without padding.
    pub fn new() -> Self {
        let mut raw = Zeroizing::new([0u8; 32]);
        rand::rng().fill_bytes(&mut *raw);
        Self(Zeroizing::new(b64url(&*raw)))
    }

    /// A secret as text; None when it isn't base64url.
    pub fn parse(text: &str) -> Option<Self> {
        b64url_decode(text)?;
        Some(Self(Zeroizing::new(text.to_string())))
    }

    /// The text form, at its point of use only: the pairing reply and the file.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn key(&self) -> Zeroizing<Vec<u8>> {
        b64url_decode(&self.0).unwrap_or_else(|| Zeroizing::new(Vec::new()))
    }
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(**********)")
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        same(&self.0, &other.0)
    }
}

/// 32 random bytes, base64url without padding.
pub fn new_secret() -> Secret {
    Secret::new()
}

/// 16 random bytes, hex.
pub fn new_nonce() -> String {
    let mut raw = [0u8; 16];
    rand::rng().fill_bytes(&mut raw);
    hex::encode(raw)
}

pub fn body_hash(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

fn mac(secret: &Secret, text: &str) -> String {
    let key = secret.key();
    let mut m = Hmac::<Sha256>::new_from_slice(&key).expect("HMAC takes any key length");
    m.update(text.as_bytes());
    b64url(&m.finalize().into_bytes())
}

/// Constant-time compare that tolerates any input (header values).
fn same(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub fn request_string(method: &str, path: &str, ts: &str, nonce: &str, body: &[u8]) -> String {
    [
        REQUEST_PREFIX,
        &method.to_uppercase(),
        path,
        ts,
        nonce,
        &body_hash(body),
    ]
    .join("\n")
}

pub fn response_string(status: u16, nonce: &str, ts: &str, body: &[u8]) -> String {
    [
        RESPONSE_PREFIX,
        &status.to_string(),
        nonce,
        ts,
        &body_hash(body),
    ]
    .join("\n")
}

pub fn sign_request(
    secret: &Secret,
    method: &str,
    path: &str,
    ts: &str,
    nonce: &str,
    body: &[u8],
) -> String {
    mac(secret, &request_string(method, path, ts, nonce, body))
}

pub fn sign_response(secret: &Secret, status: u16, nonce: &str, ts: &str, body: &[u8]) -> String {
    mac(secret, &response_string(status, nonce, ts, body))
}

/// The four request headers, as the phone sends them. Used by tests and tools.
#[derive(Debug, Clone)]
pub struct SignedHeaders {
    pub device: String,
    pub time: String,
    pub nonce: String,
    pub signature: String,
}

impl SignedHeaders {
    pub fn pairs(&self) -> [(&'static str, &str); 4] {
        [
            ("X-Ghost-Device", &self.device),
            ("X-Ghost-Time", &self.time),
            ("X-Ghost-Nonce", &self.nonce),
            ("X-Ghost-Signature", &self.signature),
        ]
    }
}

/// The four request headers for one request; `now` and `nonce` default to the clock and
/// a fresh nonce.
pub fn signed_headers(
    secret: &Secret,
    device_id: &str,
    method: &str,
    path: &str,
    body: &[u8],
    now: Option<f64>,
    nonce: Option<&str>,
) -> SignedHeaders {
    let ts = (now.unwrap_or_else(super::wall_clock).floor() as i64).to_string();
    let nonce = nonce.map_or_else(new_nonce, String::from);
    SignedHeaders {
        device: device_id.to_string(),
        time: ts.clone(),
        signature: sign_request(secret, method, path, &ts, &nonce, body),
        nonce,
    }
}

pub fn verify_response(
    secret: &Secret,
    status: u16,
    nonce: &str,
    ts: &str,
    body: &[u8],
    signature: &str,
) -> bool {
    same(&sign_response(secret, status, nonce, ts, body), signature)
}

/// Nonces seen in the last `ttl` seconds. A nonce is recorded only once its request
/// verified, so junk can't fill it with entries that block real requests; past
/// `NONCE_CACHE_MAX` the ones expiring soonest make room.
pub struct NonceCache {
    ttl: f64,
    clock: Clock,
    max: usize,
    seen: HashMap<String, f64>,
}

impl Default for NonceCache {
    fn default() -> Self {
        Self::new(NONCE_TTL_SECS, mono())
    }
}

impl NonceCache {
    pub fn new(ttl: f64, clock: Clock) -> Self {
        Self {
            ttl,
            clock,
            max: NONCE_CACHE_MAX,
            seen: HashMap::new(),
        }
    }

    pub fn with_max(mut self, max: usize) -> Self {
        self.max = max.max(1);
        self
    }

    pub fn seen(&mut self, nonce: &str) -> bool {
        self.prune();
        self.seen.contains_key(nonce)
    }

    pub fn add(&mut self, nonce: &str) {
        self.prune();
        while self.seen.len() >= self.max {
            let soonest = self
                .seen
                .iter()
                .min_by(|a, b| a.1.total_cmp(b.1))
                .map(|(n, _)| n.clone());
            match soonest {
                Some(n) => {
                    self.seen.remove(&n);
                }
                None => break,
            }
        }
        self.seen
            .insert(nonce.to_string(), (self.clock)() + self.ttl);
    }

    fn prune(&mut self) {
        let now = (self.clock)();
        self.seen.retain(|_, exp| *exp > now);
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// What a request carried, before it is believed.
#[derive(Debug, Clone, Copy)]
pub struct Unverified<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub body: &'a [u8],
    pub device_id: Option<&'a str>,
    pub ts: Option<&'a str>,
    pub nonce: Option<&'a str>,
    pub signature: Option<&'a str>,
}

/// The secret for a device id, or None when it isn't the paired phone.
pub type Lookup = Box<dyn Fn(&str) -> Option<Secret> + Send + Sync>;

/// Checks one signed request against the paired device.
pub struct Verifier {
    lookup: Lookup,
    window: f64,
    nonces: Mutex<NonceCache>,
    clock: Clock,
}

impl Verifier {
    pub fn new(lookup: impl Fn(&str) -> Option<Secret> + Send + Sync + 'static) -> Self {
        Self {
            lookup: Box::new(lookup),
            window: CLOCK_WINDOW_SECS,
            nonces: Mutex::new(NonceCache::default()),
            clock: wall(),
        }
    }

    pub fn with_window(mut self, window: f64) -> Self {
        self.window = window;
        self
    }

    pub fn with_nonces(mut self, nonces: NonceCache) -> Self {
        self.nonces = Mutex::new(nonces);
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn secret_for(&self, device_id: Option<&str>) -> Option<Secret> {
        device_id.and_then(|id| (self.lookup)(id))
    }

    pub fn verify(&self, req: &Unverified<'_>) -> bool {
        let Some(secret) = self.secret_for(req.device_id) else {
            return false;
        };
        let (Some(ts), Some(nonce), Some(signature)) = (req.ts, req.nonce, req.signature) else {
            return false;
        };
        if ts.is_empty() || nonce.is_empty() || signature.is_empty() {
            return false;
        }
        let Ok(t) = ts.parse::<i64>() else {
            return false;
        };
        if ((self.clock)() - t as f64).abs() > self.window {
            return false;
        }
        let mut nonces = self.nonces.lock().unwrap_or_else(|e| e.into_inner());
        if !NONCE_RE.is_match(nonce) || nonces.seen(nonce) {
            return false;
        }
        let expected = sign_request(&secret, req.method, req.path, ts, nonce, req.body);
        if !same(&expected, signature) {
            return false;
        }
        nonces.add(nonce);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn nonce_cache_forgets_after_ttl() {
        let t = Arc::new(Mutex::new(1000.0));
        let clock: Clock = {
            let t = t.clone();
            Arc::new(move || *t.lock().unwrap())
        };
        let mut cache = NonceCache::new(120.0, clock);
        cache.add(&"a".repeat(32));
        assert!(cache.seen(&"a".repeat(32)));
        *t.lock().unwrap() += 121.0;
        assert!(!cache.seen(&"a".repeat(32)));
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn the_nonce_cache_is_bounded() {
        let mut cache = NonceCache::new(120.0, mono()).with_max(3);
        for i in 0..5 {
            cache.add(&format!("{i:032x}"));
        }
        assert_eq!(cache.len(), 3);
        assert!(cache.seen(&format!("{:032x}", 4)));
        assert!(!cache.seen(&format!("{:032x}", 0)));
    }

    #[test]
    fn a_secret_is_32_bytes_and_never_printed() {
        let s = Secret::new();
        assert_eq!(s.as_str().len(), 43);
        assert_eq!(b64url_decode(s.as_str()).unwrap().len(), 32);
        assert_eq!(format!("{s:?}"), "Secret(**********)");
        assert_eq!(Secret::parse(s.as_str()).unwrap(), s);
        assert!(Secret::parse("not base64 at all!").is_none());
        assert_eq!(new_nonce().len(), 32);
    }

    #[test]
    fn the_strings_to_sign_are_the_protocols() {
        let s = request_string("get", "/hello?x=1", "1700000000", "ab", b"");
        assert_eq!(
            s,
            format!(
                "ghost-v1\nGET\n/hello?x=1\n1700000000\nab\n{}",
                body_hash(b"")
            )
        );
        let r = response_string(200, "ab", "1700000000", b"{}");
        assert_eq!(
            r,
            format!(
                "ghost-v1-response\n200\nab\n1700000000\n{}",
                body_hash(b"{}")
            )
        );
        let secret = Secret::new();
        let sig = sign_response(&secret, 200, "ab", "1700000000", b"{}");
        assert!(verify_response(
            &secret,
            200,
            "ab",
            "1700000000",
            b"{}",
            &sig
        ));
        assert!(!verify_response(
            &secret,
            201,
            "ab",
            "1700000000",
            b"{}",
            &sig
        ));
    }
}
