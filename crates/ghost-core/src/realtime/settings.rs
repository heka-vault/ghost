//! The realtime voice's settings, all in settings.env and none of them a password:
//!
//! ```text
//! GHOST_VOICE_MODE               local (the default) or realtime
//! GHOST_REALTIME_PROVIDER        openai (the one shipped)
//! GHOST_REALTIME_MODEL           the provider's model id (a default per provider)
//! GHOST_REALTIME_VOICE           one of the provider's voices (a default per provider)
//! GHOST_REALTIME_PASSWORD        the password's name in Ghost's vault (the provider's own by default)
//! GHOST_REALTIME_DAILY_CAP_USD   the day's spend after which Ghost uses the local voice; empty: none
//! GHOST_REALTIME_SILENCE_MS      how long a pause ends the operator's turn for the provider
//! GHOST_REALTIME_URL             the provider's address (tests and proxies; folded away in the desktop)
//! ```
//!
//! Only the operator sets these (`PUT /settings`): no tool, app, model or run has a route
//! to them, and Ghost never changes the mode itself (ADR 0063).

use zeroize::Zeroizing;

pub const KEYS: [&str; 8] = [
    "GHOST_VOICE_MODE",
    "GHOST_REALTIME_PROVIDER",
    "GHOST_REALTIME_MODEL",
    "GHOST_REALTIME_VOICE",
    "GHOST_REALTIME_PASSWORD",
    "GHOST_REALTIME_DAILY_CAP_USD",
    "GHOST_REALTIME_SILENCE_MS",
    "GHOST_REALTIME_URL",
];

pub const LOCAL: &str = "local";
pub const REALTIME: &str = "realtime";

/// A model a provider offers, with its prices per million tokens (USD), as the provider's
/// own price list gave them on the date in [`PRICES_AS_OF`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub prices: crate::realtime::Prices,
}

/// When the prices below were read from the providers' pages.
pub const PRICES_AS_OF: &str = "2026-11-12";

#[derive(Clone, Copy, Debug)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub url: &'static str,
    pub password: &'static str,
    pub models: &'static [ModelSpec],
    pub default_model: &'static str,
    pub voices: &'static [&'static str],
    pub default_voice: &'static str,
    /// What one minute of talk costs, roughly, with the default model: said to the
    /// operator before the mode is turned on.
    pub cost_line: &'static str,
}

pub const OPENAI: ProviderSpec = ProviderSpec {
    id: "openai",
    title: "OpenAI Realtime",
    url: "wss://api.openai.com/v1/realtime",
    password: "OPENAI_API_KEY",
    models: &[
        ModelSpec {
            id: "gpt-realtime-2.1-mini",
            title: "GPT Realtime 2.1 mini",
            prices: crate::realtime::Prices {
                text_in: 0.60,
                text_cached: 0.06,
                text_out: 2.40,
                audio_in: 10.0,
                audio_cached: 0.30,
                audio_out: 20.0,
            },
        },
        ModelSpec {
            id: "gpt-realtime-2.1",
            title: "GPT Realtime 2.1",
            prices: crate::realtime::Prices {
                text_in: 4.0,
                text_cached: 0.40,
                text_out: 24.0,
                audio_in: 32.0,
                audio_cached: 0.40,
                audio_out: 64.0,
            },
        },
    ],
    default_model: "gpt-realtime-2.1-mini",
    voices: &[
        "marin", "cedar", "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse",
    ],
    default_voice: "marin",
    cost_line: "about 1 to 5 US cents a minute of conversation with the mini model and 5 to 15 with the full one, growing as a session gets long, on top of what the model connection already costs",
};

pub const PROVIDERS: [ProviderSpec; 1] = [OPENAI];

/// What changes when the operator turns the realtime voice on, said plainly before it is
/// on (ADR 0063). The desktop shows these lines as written; docs/realtime-voice.md says the
/// same.
pub fn disclosure(spec: &ProviderSpec) -> Vec<String> {
    vec![
        format!(
            "After the wake, for the whole session, your voice itself goes to {}: it hears you and answers in its own voice.",
            spec.title
        ),
        "Before the wake nothing changes: the wake word and everything Ghost hears while it waits stay on this machine. The phone's calls, typed turns and scheduled runs keep the voice on this machine.".into(),
        "Ghost's agent still does every lookup and every change, under your permissions. The realtime voice only talks: it hands your requests to the agent and says its answers word for word, and Ghost checks what it says.".into(),
        format!(
            "It costs {}, billed by {} to the key you give it. You can set a daily cap, after which Ghost uses the voice on this machine.",
            spec.cost_line, spec.title
        ),
        "You can switch back to the voice on this machine at any time. Ghost never switches it on by itself, and nothing an app, a tool or a model says can.".into(),
    ]
}

pub fn provider_spec(id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|p| p.id == id)
}

#[derive(Clone, Debug, PartialEq)]
pub struct RealtimeSettings {
    /// The operator chose realtime. Off unless `GHOST_VOICE_MODE=realtime`.
    pub on: bool,
    pub provider: String,
    pub model: String,
    pub voice: String,
    /// The password's name; the value is read from the environment the vault fills.
    pub password: String,
    pub daily_cap_usd: Option<f64>,
    pub silence_ms: u32,
    /// The provider's address, when not its own.
    pub url: String,
}

impl Default for RealtimeSettings {
    fn default() -> Self {
        Self::from_vars(|_| None).expect("the defaults read")
    }
}

impl RealtimeSettings {
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let val = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mode = val("GHOST_VOICE_MODE").unwrap_or_else(|| LOCAL.into());
        let on = match mode.to_ascii_lowercase().as_str() {
            LOCAL => false,
            REALTIME => true,
            other => {
                return Err(format!(
                    "GHOST_VOICE_MODE is {other:?}; want local or realtime"
                ));
            }
        };
        let provider = val("GHOST_REALTIME_PROVIDER").unwrap_or_else(|| OPENAI.id.into());
        let spec = provider_spec(&provider)
            .ok_or_else(|| format!("GHOST_REALTIME_PROVIDER is {provider:?}; want openai"))?;
        let model = val("GHOST_REALTIME_MODEL").unwrap_or_else(|| spec.default_model.into());
        let voice = val("GHOST_REALTIME_VOICE").unwrap_or_else(|| spec.default_voice.into());
        if !spec.voices.contains(&voice.as_str()) {
            return Err(format!(
                "GHOST_REALTIME_VOICE is {voice:?}; {} has {}",
                spec.title,
                spec.voices.join(", ")
            ));
        }
        let password = val("GHOST_REALTIME_PASSWORD").unwrap_or_else(|| spec.password.into());
        if !crate::connection::is_password_name(&password) {
            return Err(format!(
                "GHOST_REALTIME_PASSWORD is {password:?}; want a password's name like OPENAI_API_KEY"
            ));
        }
        let daily_cap_usd = match val("GHOST_REALTIME_DAILY_CAP_USD") {
            None => None,
            Some(v) => match v.parse::<f64>() {
                Ok(c) if c.is_finite() && c >= 0.0 => Some(c),
                _ => {
                    return Err(format!(
                        "GHOST_REALTIME_DAILY_CAP_USD is {v:?}; want an amount in US dollars"
                    ));
                }
            },
        };
        let silence_ms = match val("GHOST_REALTIME_SILENCE_MS") {
            None => 500,
            Some(v) => v
                .parse::<u32>()
                .ok()
                .filter(|ms| (200..=2000).contains(ms))
                .ok_or_else(|| format!("GHOST_REALTIME_SILENCE_MS is {v:?}; want 200 to 2000"))?,
        };
        let url = val("GHOST_REALTIME_URL").unwrap_or_default();
        Ok(Self {
            on,
            provider,
            model,
            voice,
            password,
            daily_cap_usd,
            silence_ms,
            url,
        })
    }

    pub fn spec(&self) -> &'static ProviderSpec {
        provider_spec(&self.provider).unwrap_or(&OPENAI)
    }

    /// The model's prices, when Ghost knows them (an id it does not know has none, and the
    /// spend line says so rather than guess).
    pub fn prices(&self) -> Option<crate::realtime::Prices> {
        self.spec()
            .models
            .iter()
            .find(|m| m.id == self.model)
            .map(|m| m.prices)
    }

    pub fn url(&self) -> String {
        if self.url.is_empty() {
            self.spec().url.to_string()
        } else {
            self.url.clone()
        }
    }

    /// The key from the environment the vault fills; `None` when it is not there.
    pub fn key(&self, get: impl Fn(&str) -> Option<String>) -> Option<Zeroizing<String>> {
        get(&self.password)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(Zeroizing::new)
    }

    /// One line for `ghostd check` and the start-up log, naming no value.
    pub fn describe(&self) -> String {
        if !self.on {
            return "on this machine (Parakeet, the model connection, Kokoro)".into();
        }
        let cap = self
            .daily_cap_usd
            .map(|c| format!(", ${c:.2} a day at most"))
            .unwrap_or_default();
        format!(
            "realtime: {} {} voice {}, key {}{cap}",
            self.spec().title,
            self.model,
            self.voice,
            self.password
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(pairs: &[(&str, &str)]) -> Result<RealtimeSettings, String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        RealtimeSettings::from_vars(move |k| {
            pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn what_turning_it_on_changes_is_said_as_the_operators_page_says_it() {
        let page = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/realtime-voice.md"),
        )
        .unwrap();
        let page = page.split_whitespace().collect::<Vec<_>>().join(" ");
        for line in disclosure(&OPENAI) {
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(page.contains(&line), "docs/realtime-voice.md lacks: {line}");
        }
    }

    #[test]
    fn the_default_is_the_voice_on_this_machine() {
        let s = read(&[]).unwrap();
        assert!(!s.on);
        assert_eq!(s.provider, "openai");
        assert_eq!(s.model, "gpt-realtime-2.1-mini");
        assert_eq!(s.voice, "marin");
        assert_eq!(s.password, "OPENAI_API_KEY");
        assert_eq!(s.daily_cap_usd, None);
        assert!(s.describe().starts_with("on this machine"));
    }

    #[test]
    fn realtime_is_chosen_only_by_its_own_setting_and_reads_back() {
        let s = read(&[
            ("GHOST_VOICE_MODE", "realtime"),
            ("GHOST_REALTIME_VOICE", "cedar"),
            ("GHOST_REALTIME_DAILY_CAP_USD", "1.50"),
            ("GHOST_REALTIME_PASSWORD", "MY_REALTIME_KEY"),
        ])
        .unwrap();
        assert!(s.on);
        assert_eq!(s.daily_cap_usd, Some(1.5));
        assert!(s.describe().contains("MY_REALTIME_KEY"));
        assert!(s.prices().is_some());
    }

    #[test]
    fn a_setting_that_does_not_read_is_an_error_not_a_guess() {
        assert!(read(&[("GHOST_VOICE_MODE", "cloud")]).is_err());
        assert!(read(&[("GHOST_REALTIME_PROVIDER", "elsewhere")]).is_err());
        assert!(read(&[("GHOST_REALTIME_VOICE", "nobody")]).is_err());
        assert!(read(&[("GHOST_REALTIME_DAILY_CAP_USD", "lots")]).is_err());
        assert!(read(&[("GHOST_REALTIME_PASSWORD", "sk-not-a-name")]).is_err());
        assert!(read(&[("GHOST_REALTIME_SILENCE_MS", "50")]).is_err());
    }

    #[test]
    fn the_key_is_read_by_name_and_never_described() {
        let s = read(&[("GHOST_VOICE_MODE", "realtime")]).unwrap();
        assert!(s.key(|_| None).is_none());
        let key = s
            .key(|k| (k == "OPENAI_API_KEY").then(|| "sk-test-value-123".to_string()))
            .unwrap();
        assert_eq!(key.as_str(), "sk-test-value-123");
        assert!(!s.describe().contains("sk-test"));
    }
}
