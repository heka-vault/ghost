//! The four levels a change can run at (ADR 0011). The operator's words for them are the
//! permission lists (ADR 0034): `block`, `ask`, `allow` (announce) and `quiet`.

use serde::{Deserialize, Serialize};

/// How a change is handled, least to most autonomous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    /// Blocked: not offered to the model at all.
    Deny,
    /// Ask: propose, ask, wait for a spoken yes, then apply.
    Confirm,
    /// Allow: apply now, say briefly what happened.
    Announce,
    /// Apply now, mention only if asked or if it failed.
    Quiet,
}

impl Autonomy {
    pub const ALL: [Autonomy; 4] = [
        Autonomy::Deny,
        Autonomy::Confirm,
        Autonomy::Announce,
        Autonomy::Quiet,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Autonomy::Deny => "deny",
            Autonomy::Confirm => "confirm",
            Autonomy::Announce => "announce",
            Autonomy::Quiet => "quiet",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

impl std::fmt::Display for Autonomy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A source's own note about an operation; the level is the operator's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    #[default]
    Medium,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
        }
    }
}
