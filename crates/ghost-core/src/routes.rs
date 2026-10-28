//! How Ghost reaches the operator (ADR 0031): the route policy and the route engine.
//!
//! The operator's `[routes]` in `policy.toml` says which routes are on, how loud each
//! watch may get, when to escalate, quiet hours, batching and a cap on calls. The model
//! sets none of it; do-not-disturb is a fixed intent the operator says.
//!
//! Routes, quietest first: `digest` (held until the next wake), `desk` (Ghost greets at
//! the desk and waits for a reply, ADR 0032), `desktop` (a notification), `phone` (the
//! phone app's waiting list and its notification while it runs), `call` (the desk rings
//! the phone), then the carriers (`sms`, `whatsapp`). The digest and the phone are on
//! unless the file says otherwise; every other route is off until enabled.
//!
//! **Delivered is not acknowledged.** A route reports what it did; only the operator
//! acknowledging an item (a reply, opening it, pressing acknowledge) stops the alerting.
//! An item delivered and not acknowledged moves up the ladder after `escalate_after`
//! minutes, or at once when the route knows nobody heard (desk voice with no reply),
//! never past its watch's urgency.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Local, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::redact::redact_text;
use crate::time::{iso, parse_iso, utcnow};
use crate::waiting::{Item, Items, OnChange, Response, Result, State};

/// One way to reach the operator. Declared quietest first, so the derived order is the
/// ladder's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteName {
    Digest,
    Desk,
    Desktop,
    Phone,
    Call,
    Sms,
    Whatsapp,
}

/// The routes, quietest first.
pub const LADDER: [RouteName; 7] = [
    RouteName::Digest,
    RouteName::Desk,
    RouteName::Desktop,
    RouteName::Phone,
    RouteName::Call,
    RouteName::Sms,
    RouteName::Whatsapp,
];
/// The routes that reach the operator only at the desk.
pub const AT_DESK: [RouteName; 2] = [RouteName::Desk, RouteName::Desktop];
pub const DEFAULT_ENABLED: [RouteName; 2] = [RouteName::Digest, RouteName::Phone];

impl RouteName {
    pub fn as_str(self) -> &'static str {
        match self {
            RouteName::Digest => "digest",
            RouteName::Desk => "desk",
            RouteName::Desktop => "desktop",
            RouteName::Phone => "phone",
            RouteName::Call => "call",
            RouteName::Sms => "sms",
            RouteName::Whatsapp => "whatsapp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        LADDER.iter().copied().find(|r| r.as_str() == s)
    }

    /// Its place on the ladder, 0 the digest.
    pub fn index(self) -> usize {
        LADDER.iter().position(|r| *r == self).unwrap_or(0)
    }
}

impl std::fmt::Display for RouteName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How loud a watch's items may get: the loudest route each allows (`ceiling`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Digest,
    Notify,
    Call,
}

impl Urgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Urgency::Digest => "digest",
            Urgency::Notify => "notify",
            Urgency::Call => "call",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "digest" => Some(Urgency::Digest),
            "notify" => Some(Urgency::Notify),
            "call" => Some(Urgency::Call),
            _ => None,
        }
    }

    /// The loudest route per urgency.
    pub fn ceiling(self) -> RouteName {
        match self {
            Urgency::Digest => RouteName::Digest,
            Urgency::Notify => RouteName::Phone,
            Urgency::Call => RouteName::Whatsapp,
        }
    }
}

/// What an outside route may say: the source and a count, or the titles too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Carry {
    Count,
    Title,
}

impl Carry {
    pub fn as_str(self) -> &'static str {
        match self {
            Carry::Count => "count",
            Carry::Title => "title",
        }
    }
}

/// The `[routes]` section doesn't read. The message names the key, as the operator
/// wrote it.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct RouteError(pub String);

const KEYS: [&str; 9] = [
    "enabled",
    "escalate_after",
    "quiet_hours",
    "batch_secs",
    "calls_per_hour",
    "calls_per_day",
    "carry",
    "greeting",
    "urgency",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePolicy {
    pub enabled: Vec<RouteName>,
    /// Minutes a delivery waits for an acknowledgment.
    pub escalate_after: i64,
    /// Local times; nothing but the digest.
    pub quiet_hours: Option<(NaiveTime, NaiveTime)>,
    /// A new item waits this long so others can go with it.
    pub batch_secs: i64,
    pub calls_per_hour: i64,
    pub calls_per_day: i64,
    /// What an outside route may say.
    pub carry: Carry,
    /// What desk voice says before anything else.
    pub greeting: String,
    /// A watch or hook, by name, to how loud it may get; in the file's order.
    pub urgency: Vec<(String, Urgency)>,
}

impl Default for RoutePolicy {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED.to_vec(),
            escalate_after: 15,
            quiet_hours: None,
            batch_secs: 60,
            calls_per_hour: 2,
            calls_per_day: 6,
            carry: Carry::Count,
            greeting: "Got a minute?".into(),
            urgency: Vec::new(),
        }
    }
}

impl RoutePolicy {
    /// From the `[routes]` table the policy file held (`AutonomyPolicy.routes`); None is
    /// the default policy.
    pub fn parse(raw: Option<&toml::Table>, where_: &str) -> std::result::Result<Self, RouteError> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        let mut unknown: Vec<&str> = raw
            .keys()
            .map(String::as_str)
            .filter(|k| !KEYS.contains(k))
            .collect();
        unknown.sort_unstable();
        if !unknown.is_empty() {
            return Err(RouteError(format!(
                "{where_}: unknown key(s) {}",
                unknown.join(", ")
            )));
        }
        let enabled = match raw.get("enabled") {
            None => DEFAULT_ENABLED.to_vec(),
            Some(toml::Value::Array(listed)) => {
                let mut names = Vec::new();
                for v in listed {
                    let Some(r) = v.as_str().and_then(RouteName::parse) else {
                        return Err(enabled_error(where_));
                    };
                    names.push(r);
                }
                // quietest first, whatever order
                LADDER
                    .iter()
                    .copied()
                    .filter(|r| names.contains(r))
                    .collect()
            }
            Some(_) => return Err(enabled_error(where_)),
        };
        Ok(Self {
            enabled,
            escalate_after: whole(
                raw.get("escalate_after"),
                15,
                1,
                24 * 60,
                &format!("{where_}.escalate_after"),
            )?,
            quiet_hours: hours(raw.get("quiet_hours"), &format!("{where_}.quiet_hours"))?,
            batch_secs: whole(
                raw.get("batch_secs"),
                60,
                0,
                3600,
                &format!("{where_}.batch_secs"),
            )?,
            calls_per_hour: whole(
                raw.get("calls_per_hour"),
                2,
                0,
                60,
                &format!("{where_}.calls_per_hour"),
            )?,
            calls_per_day: whole(
                raw.get("calls_per_day"),
                6,
                0,
                200,
                &format!("{where_}.calls_per_day"),
            )?,
            carry: carry(raw.get("carry"), &format!("{where_}.carry"))?,
            greeting: greeting(raw.get("greeting"), &format!("{where_}.greeting"))?,
            urgency: urgency(raw.get("urgency"), &format!("{where_}.urgency"))?,
        })
    }

    /// `[routes]` as the file says it, for a policy file the API rewrites.
    pub fn to_toml(&self) -> Vec<String> {
        let mut lines = vec!["[routes]".to_string()];
        let enabled: Vec<String> = self.enabled.iter().map(|r| format!("\"{r}\"")).collect();
        lines.push(format!("enabled = [{}]", enabled.join(", ")));
        lines.push(format!("escalate_after = {}", self.escalate_after));
        if let Some((a, b)) = self.quiet_hours {
            lines.push(format!(
                "quiet_hours = \"{}-{}\"",
                a.format("%H:%M"),
                b.format("%H:%M")
            ));
        }
        lines.push(format!("batch_secs = {}", self.batch_secs));
        lines.push(format!("calls_per_hour = {}", self.calls_per_hour));
        lines.push(format!("calls_per_day = {}", self.calls_per_day));
        lines.push(format!("carry = \"{}\"", self.carry.as_str()));
        lines.push(format!("greeting = {}", quote(&self.greeting)));
        if !self.urgency.is_empty() {
            lines.push(String::new());
            lines.push("[routes.urgency]".to_string());
            for (k, v) in &self.urgency {
                lines.push(format!("\"{k}\" = \"{}\"", v.as_str()));
            }
        }
        lines
    }

    pub fn is_quiet(&self, local: NaiveDateTime) -> bool {
        let Some((a, b)) = self.quiet_hours else {
            return false;
        };
        let t = local.time();
        if a <= b {
            a <= t && t < b
        } else {
            t >= a || t < b
        }
    }

    /// The urgency the file gives a watch or hook, by name.
    pub fn urgency_for(&self, name: &str) -> Option<Urgency> {
        self.urgency
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, u)| *u)
    }
}

fn enabled_error(where_: &str) -> RouteError {
    let all: Vec<&str> = LADDER.iter().map(|r| r.as_str()).collect();
    RouteError(format!(
        "{where_}.enabled lists routes from: {}",
        all.join(", ")
    ))
}

fn whole(
    v: Option<&toml::Value>,
    default: i64,
    lo: i64,
    hi: i64,
    where_: &str,
) -> std::result::Result<i64, RouteError> {
    match v {
        None => Ok(default),
        Some(toml::Value::Integer(n)) if (lo..=hi).contains(n) => Ok(*n),
        Some(_) => Err(RouteError(format!(
            "{where_} is a whole number from {lo} to {hi}"
        ))),
    }
}

fn carry(v: Option<&toml::Value>, where_: &str) -> std::result::Result<Carry, RouteError> {
    match v.map(|v| v.as_str()) {
        None => Ok(Carry::Count),
        Some(Some("count")) => Ok(Carry::Count),
        Some(Some("title")) => Ok(Carry::Title),
        Some(_) => Err(RouteError(format!("{where_} is one of count, title"))),
    }
}

fn greeting(v: Option<&toml::Value>, where_: &str) -> std::result::Result<String, RouteError> {
    match v {
        None => Ok("Got a minute?".into()),
        Some(toml::Value::String(s)) if (1..=80).contains(&s.trim().chars().count()) => {
            Ok(s.trim().to_string())
        }
        Some(_) => Err(RouteError(format!("{where_} is a short phrase"))),
    }
}

static HOURS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{1,2}):(\d{2})-(\d{1,2}):(\d{2})$").unwrap());

fn hours(
    v: Option<&toml::Value>,
    where_: &str,
) -> std::result::Result<Option<(NaiveTime, NaiveTime)>, RouteError> {
    let text = match v {
        None => return Ok(None),
        Some(toml::Value::String(s)) if s.is_empty() => return Ok(None),
        Some(toml::Value::String(s)) => s.replace(' ', ""),
        Some(other) => other.to_string(),
    };
    let Some(m) = HOURS.captures(&text) else {
        return Err(RouteError(format!("{where_} is \"HH:MM-HH:MM\"")));
    };
    let n = |i: usize| m[i].parse::<u32>().unwrap_or(99);
    match (
        NaiveTime::from_hms_opt(n(1), n(2), 0),
        NaiveTime::from_hms_opt(n(3), n(4), 0),
    ) {
        (Some(a), Some(b)) => Ok(Some((a, b))),
        _ => Err(RouteError(format!("{where_} has a time that isn't one"))),
    }
}

fn urgency(
    v: Option<&toml::Value>,
    where_: &str,
) -> std::result::Result<Vec<(String, Urgency)>, RouteError> {
    let err = || {
        RouteError(format!(
            "{where_} maps a watch or hook, by name, to digest, notify or call"
        ))
    };
    match v {
        None => Ok(Vec::new()),
        Some(toml::Value::Table(t)) => t
            .iter()
            .map(|(k, u)| {
                u.as_str()
                    .and_then(Urgency::parse)
                    .map(|u| (k.clone(), u))
                    .ok_or_else(err)
            })
            .collect(),
        Some(_) => Err(err()),
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// --- routes --------------------------------------------------------------------------

/// What a route knows after passing the items on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Sent; nobody has said they heard.
    Delivered,
    /// Nobody answered: go on at once.
    NotHeard,
    /// The operator answered, which is the acknowledgment.
    Heard,
    /// It can't now: the items go on to the next route this time.
    Unavailable,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Delivery::Delivered => "delivered",
            Delivery::NotHeard => "not_heard",
            Delivery::Heard => "heard",
            Delivery::Unavailable => "unavailable",
        }
    }
}

/// One way to reach the operator. `deliver` passes the items on and says what it knows
/// ([`Delivery`]). An error is `unavailable` with a warning: one route failing is not
/// the end.
///
/// A route that gives the operator only the first few items (desk voice's note) says how
/// many in `names`; `heard` acknowledges those, and the rest are only delivered.
#[async_trait]
pub trait Route: Send + Sync {
    fn name(&self) -> RouteName;

    /// How many of the items it gives the operator by title; None is all of them.
    fn names(&self) -> Option<usize> {
        None
    }

    async fn deliver(&self, items: &[Item], policy: &RoutePolicy) -> anyhow::Result<Delivery>;
}

// --- the engine ------------------------------------------------------------------------

/// What goes out by one route this tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub route: RouteName,
    pub items: Vec<Item>,
}

/// One line of the engine's log: when, the item, the route, what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub at: DateTime<Utc>,
    pub key: String,
    pub route: RouteName,
    pub what: Delivery,
}

/// An item's declared urgency (its watch's or hook's, else `notify`).
pub type UrgencyOf = Arc<dyn Fn(&Item) -> Urgency + Send + Sync>;
/// Whether the operator is at the desk now.
pub type AtDesk = Arc<dyn Fn() -> bool + Send + Sync>;
/// A time as the operator's local time, for quiet hours.
pub type LocalTime = Arc<dyn Fn(DateTime<Utc>) -> NaiveDateTime + Send + Sync>;

pub const DND_KEY: &str = "dnd_until";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Decides, each tick, which unacknowledged items go out by which route, and records
/// every delivery. `urgency_of` gives an item's declared urgency (its watch's or hook's,
/// else `notify`); `at_desk` says whether the operator is at the desk now; `local`
/// turns a time into the operator's local time for quiet hours.
pub struct RouteEngine {
    pub items: Items,
    policy: Mutex<RoutePolicy>,
    routes: Mutex<HashMap<RouteName, Arc<dyn Route>>>,
    urgency_of: UrgencyOf,
    at_desk: AtDesk,
    local: LocalTime,
    on_change: OnChange,
    /// Do-not-disturb; kept over restarts.
    dnd_until: Mutex<Option<DateTime<Utc>>>,
    /// When calls went out, for the cap.
    calls: Mutex<Vec<DateTime<Utc>>>,
    log: Mutex<Vec<LogEntry>>,
    /// Delivered, and the route knows nobody heard.
    not_heard: Mutex<HashSet<String>>,
}

impl RouteEngine {
    /// The default policy, no routes, nobody at the desk, the machine's local time.
    /// Reads do-not-disturb back from the store.
    pub fn new(items: Items) -> Result<Self> {
        let dnd_until = when(items.setting(DND_KEY)?);
        Ok(Self {
            items,
            policy: Mutex::new(RoutePolicy::default()),
            routes: Mutex::new(HashMap::new()),
            urgency_of: Arc::new(|_| Urgency::Notify),
            at_desk: Arc::new(|| false),
            local: Arc::new(|t| t.with_timezone(&Local).naive_local()),
            on_change: Arc::new(|| {}),
            dnd_until: Mutex::new(dnd_until),
            calls: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            not_heard: Mutex::new(HashSet::new()),
        })
    }

    pub fn with_policy(self, policy: RoutePolicy) -> Self {
        *lock(&self.policy) = policy;
        self
    }

    pub fn with_routes(self, routes: Vec<Arc<dyn Route>>) -> Self {
        for r in routes {
            self.add_route(r);
        }
        self
    }

    pub fn with_urgency_of(mut self, urgency_of: UrgencyOf) -> Self {
        self.urgency_of = urgency_of;
        self
    }

    pub fn with_at_desk(mut self, at_desk: AtDesk) -> Self {
        self.at_desk = at_desk;
        self
    }

    pub fn with_local(mut self, local: LocalTime) -> Self {
        self.local = local;
        self
    }

    pub fn with_on_change(mut self, on_change: OnChange) -> Self {
        self.on_change = on_change;
        self
    }

    pub fn add_route(&self, route: Arc<dyn Route>) {
        lock(&self.routes).insert(route.name(), route);
    }

    pub fn set_policy(&self, policy: RoutePolicy) {
        *lock(&self.policy) = policy;
    }

    pub fn policy(&self) -> RoutePolicy {
        lock(&self.policy).clone()
    }

    pub fn dnd_until(&self) -> Option<DateTime<Utc>> {
        *lock(&self.dnd_until)
    }

    /// When calls went out, for the cap.
    pub fn calls(&self) -> Vec<DateTime<Utc>> {
        lock(&self.calls).clone()
    }

    pub fn set_calls(&self, calls: Vec<DateTime<Utc>>) {
        *lock(&self.calls) = calls;
    }

    /// Every delivery and digest mark, so "why did it call me?" has an answer.
    pub fn log(&self) -> Vec<LogEntry> {
        lock(&self.log).clone()
    }

    /// Do-not-disturb, from the operator's own words (a fixed intent). Kept in the
    /// store, so "nothing until two" still holds after a restart.
    pub fn quiet_until(&self, until: Option<DateTime<Utc>>) -> Result<()> {
        *lock(&self.dnd_until) = until;
        self.items
            .set_setting(DND_KEY, &until.as_ref().map(iso).unwrap_or_default())
    }

    // --- what is due --------------------------------------------------------------------

    /// The enabled routes this item may use now, quietest first, without the digest.
    pub fn ladder(&self, item: &Item, now: DateTime<Utc>) -> Vec<RouteName> {
        let policy = self.policy();
        let urgency = policy
            .urgency_for(short(&item.source))
            .unwrap_or_else(|| (self.urgency_of)(item));
        let top = urgency.ceiling().index();
        let mut out = Vec::new();
        {
            let routes = lock(&self.routes);
            for r in &LADDER[1..=top] {
                if !policy.enabled.contains(r)
                    || (*r != RouteName::Digest && !routes.contains_key(r))
                {
                    continue;
                }
                if *r == RouteName::Call && !self.calls_left(now, &policy) {
                    continue;
                }
                out.push(*r);
            }
        }
        if !(self.at_desk)() {
            out.retain(|r| !AT_DESK.contains(r));
        }
        out
    }

    fn calls_left(&self, now: DateTime<Utc>, policy: &RoutePolicy) -> bool {
        let mut calls = lock(&self.calls);
        calls.retain(|t| now - *t < Duration::days(1));
        let hour = calls
            .iter()
            .filter(|t| now - **t < Duration::hours(1))
            .count() as i64;
        hour < policy.calls_per_hour && (calls.len() as i64) < policy.calls_per_day
    }

    pub fn quiet(&self, now: DateTime<Utc>) -> bool {
        if let Some(until) = *lock(&self.dnd_until)
            && now < until
        {
            return true;
        }
        self.policy().is_quiet((self.local)(now))
    }

    /// Which items go out by which route now. Held for the digest when it is quiet, when
    /// no louder route is on, or while a new item waits out the batch window. `skip`
    /// names, per item, the routes that were unavailable for it this tick.
    pub fn due(
        &self,
        now: Option<DateTime<Utc>>,
        skip: &HashMap<String, HashSet<RouteName>>,
    ) -> Result<Vec<Plan>> {
        let now = now.unwrap_or_else(utcnow);
        if self.quiet(now) {
            return Ok(Vec::new());
        }
        let policy = self.policy();
        let none = HashSet::new();
        let mut plans: HashMap<RouteName, Vec<Item>> = HashMap::new();
        for item in self.items.open(Some(Response::Tell))? {
            if !item.needs_alerting(Some(now)) {
                continue;
            }
            if (now - item.changed_at).num_seconds() < policy.batch_secs {
                continue;
            }
            let ladder = self.ladder(&item, now);
            if ladder.is_empty() {
                continue;
            }
            let skipped = skip.get(&item.key).unwrap_or(&none);
            if let Some(next) = self.next(&item, &ladder, now, skipped, &policy) {
                plans.entry(next).or_default().push(item);
            }
        }
        Ok(LADDER
            .iter()
            .filter_map(|r| plans.remove(r).map(|items| Plan { route: *r, items }))
            .collect())
    }

    fn next(
        &self,
        item: &Item,
        ladder: &[RouteName],
        now: DateTime<Utc>,
        skip: &HashSet<RouteName>,
        policy: &RoutePolicy,
    ) -> Option<RouteName> {
        let mut last = if item.state(Some(now)) == State::Delivered {
            item.delivered_via.as_deref().and_then(RouteName::parse)
        } else {
            None
        };
        if let (Some(snoozed), Some(delivered)) = (item.snoozed_until, item.delivered_at)
            && delivered < snoozed
        {
            last = None; // a snooze ran out: it alerts again from the start of its ladder
        }
        let options: Vec<RouteName> = match last {
            None | Some(RouteName::Digest) => ladder.to_vec(),
            Some(last) => {
                let waited = item
                    .delivered_at
                    .is_some_and(|d| now - d >= Duration::minutes(policy.escalate_after));
                if !waited && !lock(&self.not_heard).contains(&item.key) {
                    return None; // still waiting for the operator to acknowledge it
                }
                ladder
                    .iter()
                    .copied()
                    .filter(|r| r.index() > last.index())
                    .collect()
            }
        };
        // A route that couldn't take it this tick hands it to the next one up; the top
        // of the ladder is the end.
        options.into_iter().find(|r| !skip.contains(r))
    }

    /// Send what is due. A route that says nobody heard, or that can't take the items
    /// now, hands them on at once.
    pub async fn tick(&self, now: Option<DateTime<Utc>>) -> Result<Vec<(RouteName, Delivery)>> {
        let now = now.unwrap_or_else(utcnow);
        let mut done = Vec::new();
        let mut skip: HashMap<String, HashSet<RouteName>> = HashMap::new(); // per item, the routes unavailable this tick
        for _ in 0..LADDER.len() {
            // at most one pass per route: "not heard" moves up
            let plans = self.due(Some(now), &skip)?;
            if plans.is_empty() {
                break;
            }
            let mut moved = false;
            for plan in plans {
                let route = lock(&self.routes).get(&plan.route).cloned();
                let Some(route) = route else {
                    continue;
                };
                let policy = self.policy();
                let what = match route.deliver(&plan.items, &policy).await {
                    Ok(what) => what,
                    Err(e) => {
                        warn!(
                            "routes: {} failed: {}",
                            plan.route,
                            redact_text(&e.to_string())
                        );
                        Delivery::Unavailable
                    }
                };
                done.push((plan.route, what));
                if plan.route == RouteName::Call && what != Delivery::Unavailable {
                    lock(&self.calls).push(now);
                }
                let names = route.names();
                for (n, item) in plan.items.iter().enumerate() {
                    lock(&self.log).push(LogEntry {
                        at: now,
                        key: item.key.clone(),
                        route: plan.route,
                        what,
                    });
                    if what == Delivery::Unavailable {
                        skip.entry(item.key.clone()).or_default().insert(plan.route);
                        moved = true;
                        continue;
                    }
                    self.items
                        .delivered(&item.key, plan.route.as_str(), Some(now))?;
                    lock(&self.not_heard).remove(&item.key);
                    if what == Delivery::Heard && names.is_none_or(|most| n < most) {
                        self.items.acknowledge(&item.key, Some(now), None)?;
                    } else if what == Delivery::NotHeard {
                        lock(&self.not_heard).insert(item.key.clone()); // nobody heard: move on now
                        moved = true;
                    }
                }
            }
            (self.on_change)();
            if !moved {
                break;
            }
        }
        Ok(done)
    }

    // --- the digest ---------------------------------------------------------------------

    /// What the next wake should say before anything else: every open item that still
    /// needs the operator to hear it, oldest first.
    pub fn digest(&self, now: Option<DateTime<Utc>>) -> Result<Vec<Item>> {
        let now = now.unwrap_or_else(utcnow);
        Ok(self
            .items
            .open(Some(Response::Tell))?
            .into_iter()
            .filter(|i| i.needs_alerting(Some(now)))
            .collect())
    }

    /// The wake's greeting went out with them: delivered by the digest, not heard.
    pub fn said_in_digest(&self, items: &[Item], now: Option<DateTime<Utc>>) -> Result<()> {
        self.mark(items, Delivery::Delivered, now)
    }

    /// The operator answered the greeting that said these titles: heard (ADR 0029).
    pub fn heard_in_digest(&self, items: &[Item], now: Option<DateTime<Utc>>) -> Result<()> {
        self.mark(items, Delivery::Heard, now)
    }

    fn mark(&self, items: &[Item], what: Delivery, now: Option<DateTime<Utc>>) -> Result<()> {
        let now = now.unwrap_or_else(utcnow);
        for i in items {
            if what == Delivery::Heard {
                self.items.acknowledge(&i.key, Some(now), None)?;
            } else {
                self.items
                    .delivered(&i.key, RouteName::Digest.as_str(), Some(now))?;
            }
            lock(&self.log).push(LogEntry {
                at: now,
                key: i.key.clone(),
                route: RouteName::Digest,
                what,
            });
        }
        if !items.is_empty() {
            (self.on_change)();
        }
        Ok(())
    }
}

/// Titles the digest says; the rest are a count.
pub const DIGEST_TITLES: usize = 3;

/// One bare wake's digest, from the greeting to the operator's answer (ADR 0031). The
/// greeting reaching the voice delivers every item it counts; the operator's first turn
/// after it acknowledges only the items whose titles it said, since "and two more" is
/// not hearing them. `on_turn` takes the session's turns as they happen; a greeting that
/// never went out (the voice failed) marks nothing.
pub struct SpokenDigest {
    engine: Arc<RouteEngine>,
    pub items: Vec<Item>,
    pub named: Vec<Item>,
    pub text: String,
    said: bool,
    heard: bool,
}

impl SpokenDigest {
    pub fn new(engine: Arc<RouteEngine>, items: &[Item]) -> Self {
        Self::with_most(engine, items, DIGEST_TITLES)
    }

    pub fn with_most(engine: Arc<RouteEngine>, items: &[Item], most: usize) -> Self {
        Self {
            engine,
            items: items.to_vec(),
            named: items.iter().take(most).cloned().collect(),
            text: spoken_digest(items, most),
            said: false,
            heard: false,
        }
    }

    pub fn on_turn(&mut self, role: &str, text: &str, now: Option<DateTime<Utc>>) -> Result<()> {
        if self.items.is_empty() || self.heard {
            return Ok(());
        }
        if role == "assistant" && !self.said && text.trim() == self.text {
            self.said = true;
            self.engine.said_in_digest(&self.items, now)?;
        } else if role == "user" && self.said {
            self.heard = true;
            self.engine.heard_in_digest(&self.named, now)?;
        }
        Ok(())
    }
}

/// `watch:ci` and `hook:ci` are `ci` in `[routes.urgency]` (ADR 0035).
fn short(source: &str) -> &str {
    source.split_once(':').map_or(source, |(_, rest)| rest)
}

fn when(v: Option<String>) -> Option<DateTime<Utc>> {
    v.filter(|s| !s.is_empty()).and_then(|s| parse_iso(&s))
}

/// The digest as Ghost says it at a bare wake: a count, then up to `most` titles. The
/// titles come from outside systems; they are read as they are, never acted on.
pub fn spoken_digest(items: &[Item], most: usize) -> String {
    if items.is_empty() {
        return String::new();
    }
    let titles: Vec<String> = items
        .iter()
        .take(most)
        .map(|i| clip_said(&i.title, 90))
        .collect();
    let head = if items.len() == 1 {
        "One thing is waiting".to_string()
    } else {
        format!("{} things are waiting", count(items.len()))
    };
    let rest = if items.len() <= most {
        String::new()
    } else {
        format!(", and {} more", count(items.len() - most).to_lowercase())
    };
    format!("Hi. {head}: {}{rest}.", titles.join("; "))
}

fn clip_said(t: &str, n: usize) -> String {
    let t = t
        .strip_prefix("Waiting for your yes: ")
        .unwrap_or(t)
        .trim_end_matches('.');
    if t.chars().count() <= n {
        t.to_string()
    } else {
        let mut cut: String = t.chars().take(n - 1).collect();
        cut.push('…');
        cut
    }
}

fn count(n: usize) -> String {
    const WORDS: [&str; 11] = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| (*w).to_string())
}

// --- do not disturb, from the operator's words ------------------------------------------

const NUM: [(&str, u32); 15] = [
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("an", 1),
    ("a", 1),
    ("half", 0),
];

fn num(word: &str) -> Option<u32> {
    NUM.iter().find(|(w, _)| *w == word).map(|(_, n)| *n)
}

static QUIET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:ghost )?(?:nothing|no interruptions|no notifications|don'?t disturb me|do not disturb(?: me)?|quiet|hold everything|no calls)\s+(until|till|til|for)\s+(.+)$",
    )
    .unwrap()
});
static FOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+|\w+) (hour|hours|minute|minutes)$").unwrap());
static CLOCK_DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{1,2})(?::(\d{2}))?\s*(am|pm|a m|p m)?$").unwrap());
static CLOCK_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\w+)(?: (thirty|fifteen|forty five|forty-five))?\s*(am|pm|a m|p m)?$").unwrap()
});

/// What the operator's words ask for: quiet until a time, or quiet with a time that
/// isn't one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    Until(DateTime<Utc>),
    /// The words ask for quiet; the time isn't one.
    Unclear,
}

pub const UNCLEAR: Quiet = Quiet::Unclear;
const MOST_QUIET_SECS: u64 = 24 * 3600;

/// "Nothing until two", "do not disturb for an hour", "quiet till 3:30 pm": when the
/// quiet ends, read in `now`'s timezone; `UNCLEAR` when they ask for quiet with a time
/// that isn't one ("until 3:75", "for a million hours"); None when the words aren't that.
pub fn parse_quiet<Tz: TimeZone>(text: &str, now: DateTime<Tz>) -> Option<Quiet> {
    let lowered = text.to_lowercase();
    let cleaned: String = lowered
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | ' ' | '\'') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let t = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.strip_prefix("okay ").unwrap_or(&t);
    let t = t.strip_prefix("ok ").unwrap_or(t);
    let m = QUIET.captures(t)?;
    let (how, rest) = (&m[1], &m[2]);
    if how == "for" {
        if matches!(rest, "an hour" | "a hour" | "one hour") {
            return Some(Quiet::Until((now + Duration::hours(1)).with_timezone(&Utc)));
        }
        if matches!(rest, "half an hour" | "a half hour") {
            return Some(Quiet::Until(
                (now + Duration::minutes(30)).with_timezone(&Utc),
            ));
        }
        let d = FOR.captures(rest)?;
        let n: u64 = if d[1].chars().all(|c| c.is_ascii_digit()) {
            d[1].parse().unwrap_or(u64::MAX)
        } else {
            u64::from(num(&d[1])?)
        };
        if n == 0 {
            return None;
        }
        let secs = if d[2].starts_with("hour") {
            n.checked_mul(3600)
        } else {
            n.checked_mul(60)
        };
        return Some(match secs {
            Some(s) if s <= MOST_QUIET_SECS => {
                Quiet::Until((now + Duration::seconds(s as i64)).with_timezone(&Utc))
            }
            _ => Quiet::Unclear,
        });
    }
    next_clock(rest, now)
}

fn next_clock<Tz: TimeZone>(rest: &str, now: DateTime<Tz>) -> Option<Quiet> {
    let rest = rest.replace("o'clock", "").replace("oclock", "");
    let rest = rest.trim();
    let (h, mins, half): (u32, u32, String) = if rest == "noon" || rest == "midday" {
        (12, 0, "pm".into())
    } else if rest == "midnight" {
        (0, 0, "am".into())
    } else {
        let m = CLOCK_DIGITS
            .captures(rest)
            .or_else(|| CLOCK_WORDS.captures(rest))?;
        let hour_s = &m[1];
        let min_s = m.get(2).map(|g| g.as_str());
        let half_s = m.get(3).map_or("", |g| g.as_str());
        let h = if hour_s.chars().all(|c| c.is_ascii_digit()) {
            hour_s.parse::<u32>().unwrap_or(u32::MAX)
        } else {
            num(hour_s)? // "until later": not a time at all
        };
        let mins = match min_s {
            None => 0,
            Some(s) if s.chars().all(|c| c.is_ascii_digit()) => s.parse().unwrap_or(u32::MAX),
            Some("thirty") => 30,
            Some("fifteen") => 15,
            Some(_) => 45, // "forty five", "forty-five"
        };
        let half = half_s.replace(' ', "");
        if h > 23 || mins > 59 || (!half.is_empty() && !(1..=12).contains(&h)) {
            return Some(Quiet::Unclear); // "3:75", "14 pm", "25": a time, but not one there is
        }
        (h, mins, half)
    };
    let candidates: Vec<u32> = match half.as_str() {
        "pm" => vec![if h == 12 { 12 } else { h + 12 }],
        "am" => vec![if h == 12 { 0 } else { h }],
        _ if h <= 12 => vec![h % 12, h % 12 + 12],
        _ => vec![h],
    };
    let mut best: Option<DateTime<Tz>> = None;
    for hh in candidates {
        let Some(at) = now
            .with_hour(hh)
            .and_then(|t| t.with_minute(mins))
            .and_then(|t| t.with_second(0))
            .and_then(|t| t.with_nanosecond(0))
        else {
            continue; // a local time that doesn't exist today (a clock change)
        };
        let at = if at <= now {
            at + Duration::days(1)
        } else {
            at
        };
        if best.as_ref().is_none_or(|b| at < *b) {
            best = Some(at);
        }
    }
    Some(best.map_or(Quiet::Unclear, |b| Quiet::Until(b.with_timezone(&Utc))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AutonomyPolicy;
    use crate::store::Store;
    use crate::waiting::ItemDraft;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 27, 15, 0, 0).unwrap()
    }

    fn mins(m: i64) -> Option<DateTime<Utc>> {
        Some(t0() + Duration::minutes(m))
    }

    struct Fake {
        name: RouteName,
        answer: Mutex<Delivery>,
        got: Mutex<Vec<Vec<String>>>,
        names: Mutex<Option<usize>>,
        broken: AtomicBool,
    }

    impl Fake {
        fn new(name: RouteName) -> Arc<Self> {
            Arc::new(Self {
                name,
                answer: Mutex::new(Delivery::Delivered),
                got: Mutex::new(Vec::new()),
                names: Mutex::new(None),
                broken: AtomicBool::new(false),
            })
        }

        fn answer(&self, d: Delivery) {
            *lock(&self.answer) = d;
        }

        fn got(&self) -> Vec<Vec<String>> {
            lock(&self.got).clone()
        }
    }

    #[async_trait]
    impl Route for Fake {
        fn name(&self) -> RouteName {
            self.name
        }

        fn names(&self) -> Option<usize> {
            *lock(&self.names)
        }

        async fn deliver(&self, items: &[Item], _policy: &RoutePolicy) -> anyhow::Result<Delivery> {
            if self.broken.load(Ordering::SeqCst) {
                anyhow::bail!("no signal");
            }
            lock(&self.got).push(items.iter().map(|i| i.key.clone()).collect());
            Ok(*lock(&self.answer))
        }
    }

    type Fakes = HashMap<RouteName, Arc<Fake>>;

    const FIVE: [RouteName; 5] = [
        RouteName::Digest,
        RouteName::Desk,
        RouteName::Desktop,
        RouteName::Phone,
        RouteName::Call,
    ];

    fn ci(key: &str, title: &str) -> ItemDraft {
        ItemDraft::new(key, title)
    }

    fn one(keys: &[&str]) -> Vec<Vec<String>> {
        vec![keys.iter().map(|k| (*k).to_string()).collect()]
    }

    fn engine(
        enabled: &[RouteName],
        at_desk: bool,
        tweak: impl FnOnce(&mut RoutePolicy),
    ) -> (Arc<RouteEngine>, Fakes, Items) {
        let items = Items::new(Arc::new(Store::in_memory().unwrap()));
        items
            .sync(
                "watch:eng/ci",
                &[ci("eng:ci:1", "CI failed on main")],
                Some(t0()),
            )
            .unwrap();
        let routes: Fakes = [
            RouteName::Desk,
            RouteName::Desktop,
            RouteName::Phone,
            RouteName::Call,
            RouteName::Sms,
        ]
        .into_iter()
        .map(|r| (r, Fake::new(r)))
        .collect();
        let mut policy = RoutePolicy {
            enabled: enabled.to_vec(),
            batch_secs: 0,
            ..Default::default()
        };
        tweak(&mut policy);
        let e = RouteEngine::new(items.clone())
            .unwrap()
            .with_policy(policy)
            .with_routes(
                routes
                    .values()
                    .map(|f| f.clone() as Arc<dyn Route>)
                    .collect(),
            )
            .with_at_desk(Arc::new(move || at_desk))
            .with_local(Arc::new(|t| t.naive_utc()));
        (Arc::new(e), routes, items)
    }

    fn plain() -> (Arc<RouteEngine>, Fakes, Items) {
        engine(&FIVE, false, |_| {})
    }

    fn at_desk() -> (Arc<RouteEngine>, Fakes, Items) {
        engine(&FIVE, true, |_| {})
    }

    #[tokio::test]
    async fn away_from_the_desk_it_goes_to_the_phone_then_escalates_to_a_call() {
        let (e, routes, _) = engine(&FIVE, false, |p| {
            p.urgency = vec![("eng/ci".into(), Urgency::Call)];
        });
        e.tick(Some(t0())).await.unwrap();
        assert_eq!(routes[&RouteName::Phone].got(), one(&["eng:ci:1"]));
        assert!(routes[&RouteName::Desk].got().is_empty());
        e.tick(mins(5)).await.unwrap(); // still waiting for an acknowledgment
        assert!(routes[&RouteName::Call].got().is_empty());
        e.tick(mins(16)).await.unwrap();
        assert_eq!(routes[&RouteName::Call].got(), one(&["eng:ci:1"]));
    }

    #[tokio::test]
    async fn notify_never_goes_past_the_phone() {
        let (e, routes, _) = plain();
        for m in [0, 16, 40] {
            e.tick(mins(m)).await.unwrap();
        }
        assert_eq!(routes[&RouteName::Phone].got(), one(&["eng:ci:1"]));
        assert!(routes[&RouteName::Call].got().is_empty());
    }

    #[tokio::test]
    async fn at_the_desk_nobody_answering_goes_on_at_once() {
        let (e, routes, items) = at_desk();
        routes[&RouteName::Desk].answer(Delivery::NotHeard);
        e.tick(Some(t0())).await.unwrap();
        assert_eq!(routes[&RouteName::Desk].got(), one(&["eng:ci:1"]));
        assert_eq!(routes[&RouteName::Desktop].got(), one(&["eng:ci:1"]));
        assert_eq!(
            items
                .get("eng:ci:1")
                .unwrap()
                .unwrap()
                .delivered_via
                .as_deref(),
            Some("desktop")
        );
    }

    #[tokio::test]
    async fn heard_is_the_acknowledgment() {
        let (e, routes, items) = at_desk();
        routes[&RouteName::Desk].answer(Delivery::Heard);
        e.tick(Some(t0())).await.unwrap();
        e.tick(mins(30)).await.unwrap();
        assert!(routes[&RouteName::Desktop].got().is_empty());
        assert_eq!(
            items.get("eng:ci:1").unwrap().unwrap().state(Some(t0())),
            State::Acknowledged
        );
    }

    #[tokio::test]
    async fn an_acknowledged_item_stops_until_it_changes() {
        let (e, routes, items) = plain();
        e.tick(Some(t0())).await.unwrap();
        items.acknowledge("eng:ci:1", mins(1), None).unwrap();
        e.tick(mins(20)).await.unwrap();
        assert_eq!(routes[&RouteName::Phone].got(), one(&["eng:ci:1"]));
        items
            .sync(
                "watch:eng/ci",
                &[ci("eng:ci:1", "CI failed again")],
                mins(21),
            )
            .unwrap();
        e.tick(mins(22)).await.unwrap();
        assert_eq!(
            routes[&RouteName::Phone].got(),
            vec![vec!["eng:ci:1".to_string()], vec!["eng:ci:1".to_string()]]
        );
    }

    #[tokio::test]
    async fn only_enabled_routes_with_a_route_behind_them_are_used() {
        let (e, routes, _) = engine(&[RouteName::Digest], false, |_| {});
        e.tick(Some(t0())).await.unwrap();
        assert!(routes.values().all(|r| r.got().is_empty()));
        let keys: Vec<String> = e
            .digest(Some(t0()))
            .unwrap()
            .into_iter()
            .map(|i| i.key)
            .collect();
        assert_eq!(keys, vec!["eng:ci:1"]);
    }

    #[tokio::test]
    async fn quiet_hours_and_do_not_disturb_hold_everything_for_the_digest() {
        let (e, routes, _) = engine(&FIVE, false, |p| {
            p.quiet_hours = Some((
                NaiveTime::from_hms_opt(14, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(16, 0, 0).unwrap(),
            ));
        });
        e.tick(Some(t0())).await.unwrap();
        assert!(routes[&RouteName::Phone].got().is_empty());
        let (e2, routes2, _) = plain();
        e2.quiet_until(Some(t0() + Duration::hours(1))).unwrap();
        e2.tick(Some(t0())).await.unwrap();
        assert!(routes2[&RouteName::Phone].got().is_empty());
        e2.tick(mins(61)).await.unwrap();
        assert_eq!(routes2[&RouteName::Phone].got(), one(&["eng:ci:1"]));
    }

    #[tokio::test]
    async fn a_new_item_waits_out_the_batch_window() {
        let (e, routes, items) = plain();
        e.set_policy(RoutePolicy {
            enabled: e.policy().enabled,
            batch_secs: 120,
            ..Default::default()
        });
        e.tick(Some(t0() + Duration::seconds(30))).await.unwrap();
        assert!(routes[&RouteName::Phone].got().is_empty());
        items
            .sync(
                "watch:eng/ci",
                &[ci("eng:ci:1", "CI failed on main"), ci("eng:ci:2", "again")],
                Some(t0() + Duration::seconds(10)),
            )
            .unwrap();
        e.tick(mins(3)).await.unwrap();
        assert_eq!(
            routes[&RouteName::Phone].got(),
            one(&["eng:ci:1", "eng:ci:2"]),
            "together"
        );
    }

    #[tokio::test]
    async fn the_call_cap_drops_to_the_route_below() {
        let (e, routes, _) = engine(&[RouteName::Digest, RouteName::Call], false, |p| {
            p.urgency = vec![("eng/ci".into(), Urgency::Call)];
            p.calls_per_hour = 1;
        });
        e.set_calls(vec![t0() - Duration::minutes(10)]);
        e.tick(Some(t0())).await.unwrap();
        assert!(routes[&RouteName::Call].got().is_empty());
    }

    #[tokio::test]
    async fn a_route_that_fails_is_skipped_not_fatal() {
        let (e, routes, items) = plain();
        routes[&RouteName::Phone]
            .broken
            .store(true, Ordering::SeqCst);
        assert_eq!(
            e.tick(Some(t0())).await.unwrap(),
            vec![(RouteName::Phone, Delivery::Unavailable)]
        );
        assert_eq!(
            items.get("eng:ci:1").unwrap().unwrap().state(Some(t0())),
            State::New
        );
    }

    #[tokio::test]
    async fn an_unavailable_route_hands_the_item_on() {
        // notify-send missing and the desk busy: the phone still hears of it (QA, 2026-10-27)
        let (e, routes, items) = at_desk();
        routes[&RouteName::Desk].answer(Delivery::Unavailable);
        routes[&RouteName::Desktop].answer(Delivery::Unavailable);
        for m in (0..105).step_by(15) {
            e.tick(mins(m)).await.unwrap();
        }
        assert_eq!(routes[&RouteName::Phone].got(), one(&["eng:ci:1"]));
        assert_eq!(
            items
                .get("eng:ci:1")
                .unwrap()
                .unwrap()
                .delivered_via
                .as_deref(),
            Some("phone")
        );
    }

    #[tokio::test]
    async fn a_snooze_ends_and_the_item_alerts_again_from_the_start() {
        let (e, routes, items) = plain(); // away from the desk: the phone is the whole ladder
        e.tick(Some(t0())).await.unwrap();
        items
            .snooze("eng:ci:1", t0() + Duration::minutes(30))
            .unwrap();
        e.tick(mins(20)).await.unwrap();
        assert_eq!(
            routes[&RouteName::Phone].got(),
            one(&["eng:ci:1"]),
            "snoozed: nothing"
        );
        e.tick(mins(31)).await.unwrap();
        assert_eq!(
            routes[&RouteName::Phone].got(),
            vec![vec!["eng:ci:1".to_string()], vec!["eng:ci:1".to_string()]]
        );
    }

    #[tokio::test]
    async fn after_a_snooze_at_the_desk_it_starts_at_the_desk_again() {
        let (e, routes, items) = at_desk();
        routes[&RouteName::Desk].answer(Delivery::NotHeard);
        e.tick(Some(t0())).await.unwrap(); // the desk, then the desktop at once
        items
            .snooze("eng:ci:1", t0() + Duration::minutes(30))
            .unwrap();
        routes[&RouteName::Desk].answer(Delivery::Delivered);
        e.tick(mins(31)).await.unwrap();
        assert_eq!(
            routes[&RouteName::Desk].got(),
            vec![vec!["eng:ci:1".to_string()], vec!["eng:ci:1".to_string()]]
        );
    }

    fn many(e: &RouteEngine, items: &Items, n: usize) -> Vec<Item> {
        let drafts: Vec<ItemDraft> = (0..n)
            .map(|i| ci(&format!("w{i}"), &format!("Item {i}")))
            .collect();
        items.sync("watch:eng/ci", &drafts, Some(t0())).unwrap();
        e.digest(Some(t0())).unwrap()
    }

    fn states(items: &Items) -> Vec<(State, Option<String>)> {
        items
            .open(None)
            .unwrap()
            .into_iter()
            .map(|i| (i.state(Some(t0())), i.delivered_via))
            .collect()
    }

    fn digest_keys(e: &RouteEngine) -> Vec<String> {
        e.digest(Some(t0()))
            .unwrap()
            .into_iter()
            .map(|i| i.key)
            .collect()
    }

    #[test]
    fn the_digest_acknowledges_only_the_titles_it_said_once_the_operator_replies() {
        let (e, _, items) = plain();
        let mut d = SpokenDigest::new(e.clone(), &many(&e, &items, 5));
        assert_eq!(
            d.text,
            "Hi. Five things are waiting: Item 0; Item 1; Item 2, and two more."
        );
        d.on_turn("user", "hello?", Some(t0())).unwrap(); // before the greeting: not a reply to it
        assert!(states(&items).iter().all(|(s, _)| *s == State::New));
        let text = d.text.clone();
        d.on_turn("assistant", &text, Some(t0())).unwrap(); // the greeting reached the voice: delivered
        assert!(
            states(&items)
                .iter()
                .all(|(s, via)| *s == State::Delivered && via.as_deref() == Some("digest"))
        );
        d.on_turn("user", "okay, thanks", Some(t0())).unwrap(); // the operator answered: the three said are heard
        let left = digest_keys(&e);
        assert_eq!(
            left,
            vec!["w3", "w4"],
            "\"and two more\" was not hearing them"
        );
        d.on_turn("user", "and the rest?", Some(t0())).unwrap();
        assert_eq!(digest_keys(&e), left, "only the first reply counts");
    }

    #[test]
    fn a_digest_that_was_never_spoken_marks_nothing() {
        let (e, _, items) = plain();
        let mut d = SpokenDigest::new(e.clone(), &many(&e, &items, 5));
        d.on_turn("user", "hello?", Some(t0())).unwrap(); // the voice failed: the operator never heard it
        d.on_turn("assistant", "Sure, what do you need?", Some(t0()))
            .unwrap();
        d.on_turn("user", "nothing", Some(t0())).unwrap();
        assert!(states(&items).iter().all(|(s, _)| *s == State::New));
    }

    #[tokio::test]
    async fn desk_voice_acknowledges_only_the_titles_it_could_give() {
        let (e, routes, items) = at_desk();
        routes[&RouteName::Desk].answer(Delivery::Heard);
        *lock(&routes[&RouteName::Desk].names) = Some(5); // what the session's note carries
        many(&e, &items, 7);
        e.tick(Some(t0())).await.unwrap();
        let mut left: Vec<String> = items
            .open(None)
            .unwrap()
            .into_iter()
            .filter(|i| i.needs_alerting(Some(t0())))
            .map(|i| i.key)
            .collect();
        left.sort();
        assert_eq!(left, vec!["w5", "w6"]);
        assert!(
            left.iter().all(|k| {
                items.get(k).unwrap().unwrap().delivered_via.as_deref() == Some("desk")
            })
        );
    }

    #[test]
    fn spoken_digest_counts_and_clips() {
        let many: Vec<ItemDraft> = (0..5)
            .map(|i| ci(&format!("k{i}"), &format!("Item {i}")))
            .collect();
        let items = Items::new(Arc::new(Store::in_memory().unwrap()));
        items.sync("s", &many, Some(t0())).unwrap();
        let out = spoken_digest(&items.open(None).unwrap(), DIGEST_TITLES);
        assert!(
            out.starts_with("Hi. Five things are waiting: Item 0; Item 1; Item 2, and two more")
        );
    }

    // --- the policy file --------------------------------------------------------------------

    fn routes_of(text: &str) -> std::result::Result<RoutePolicy, RouteError> {
        let p = AutonomyPolicy::parse(text, "<test>").unwrap();
        RoutePolicy::parse(p.routes.as_ref(), "[routes]")
    }

    #[test]
    fn routes_live_in_the_policy_file_and_round_trip() {
        let text = "[routes]\nenabled = [\"phone\", \"digest\", \"desk\"]\nquiet_hours = \"22:00-07:00\"\n\
                    [routes.urgency]\n\"eng/ci\" = \"call\"\n[conversations]\ndelete = \"confirm\"\n";
        let p = AutonomyPolicy::parse(text, "<test>").unwrap();
        let routes = RoutePolicy::parse(p.routes.as_ref(), "[routes]").unwrap();
        assert_eq!(
            routes.enabled,
            vec![RouteName::Digest, RouteName::Desk, RouteName::Phone],
            "quietest first"
        );
        let ask: Vec<String> = p.permissions.ask.iter().map(ToString::to_string).collect();
        assert_eq!(ask, vec!["conversations.delete"]);
        let again = AutonomyPolicy::parse(&p.to_toml(), "<test>").unwrap();
        assert_eq!(
            RoutePolicy::parse(again.routes.as_ref(), "[routes]").unwrap(),
            routes
        );
        assert_eq!(again.permissions, p.permissions);
        // The typed rendering reads back as itself too.
        assert_eq!(routes_of(&routes.to_toml().join("\n")).unwrap(), routes);
        assert_eq!(
            routes.to_toml()[1..3],
            [
                "enabled = [\"digest\", \"desk\", \"phone\"]",
                "escalate_after = 15"
            ]
        );
    }

    #[test]
    fn a_bad_routes_section_is_refused() {
        for body in [
            "[routes]\nenabled = [\"pager\"]",
            "[routes]\nescalate_after = 0",
            "[routes]\nquiet_hours = \"late\"",
            "[routes]\ncarry = \"everything\"",
            "[routes.urgency]\n\"eng/ci\" = \"scream\"",
            "[routes]\nsurprise = 1",
        ] {
            assert!(routes_of(body).is_err(), "{body}");
        }
        assert_eq!(
            routes_of("[routes]\nsurprise = 1").unwrap_err().0,
            "[routes]: unknown key(s) surprise"
        );
        assert_eq!(
            routes_of("[routes]\nquiet_hours = \"25:00-07:00\"")
                .unwrap_err()
                .0,
            "[routes].quiet_hours has a time that isn't one"
        );
    }

    #[test]
    fn quiet_hours_across_midnight() {
        let p = RoutePolicy {
            quiet_hours: Some((
                NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
            )),
            ..Default::default()
        };
        let at = |h: u32| t0().with_hour(h).unwrap().naive_utc();
        assert!(p.is_quiet(at(23)) && p.is_quiet(at(6)));
        assert!(!p.is_quiet(at(12)));
    }

    // --- do not disturb, in the operator's words -----------------------------------------------

    #[test]
    fn parse_quiet_reads_the_time() {
        for (said, hours, minutes) in [
            ("Nothing until two", 11, 0), // 15:00 now: the next two o'clock is 02:00
            ("Ghost, do not disturb for an hour", 1, 0),
            ("no interruptions for 45 minutes", 0, 45),
            ("quiet till 5:30 pm", 2, 30),
            ("nothing until noon", 21, 0),
            ("don't disturb me for half an hour", 0, 30),
        ] {
            assert_eq!(
                parse_quiet(said, t0()),
                Some(Quiet::Until(
                    t0() + Duration::hours(hours) + Duration::minutes(minutes)
                )),
                "{said}"
            );
        }
    }

    #[test]
    fn other_words_are_not_quiet() {
        for said in [
            "nothing",
            "what's until two",
            "quiet please",
            "nothing until later",
        ] {
            assert_eq!(parse_quiet(said, t0()), None, "{said}");
        }
    }

    #[test]
    fn a_time_that_isnt_one_is_not_understood() {
        for said in [
            "quiet until 3:75",
            "nothing until 14 pm",
            "nothing until 25",
            "quiet for 9999999 hours",
        ] {
            assert_eq!(parse_quiet(said, t0()), Some(UNCLEAR), "{said}");
        }
    }

    #[test]
    fn do_not_disturb_outlives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path().join("ghost.db")).unwrap());
        RouteEngine::new(Items::new(store.clone()))
            .unwrap()
            .quiet_until(Some(t0() + Duration::hours(1)))
            .unwrap();
        let again = RouteEngine::new(Items::new(store.clone())).unwrap();
        assert_eq!(again.dnd_until(), Some(t0() + Duration::hours(1)));
        assert!(again.quiet(t0()));
        again.quiet_until(None).unwrap();
        assert!(
            RouteEngine::new(Items::new(store))
                .unwrap()
                .dnd_until()
                .is_none()
        );
    }
}
