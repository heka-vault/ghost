//! How targets, snapshots and changes are stored: `{"$type": "module:Class", "data": {...}}`,
//! the form the Python daemon wrote, so every row in the operator's database reads back.
//! A tag nobody knows becomes a `Retired` value, loaded so the audit log still reads; a
//! tag of the wrong kind (a target where a snapshot belongs) is an error.
//!
//! The database is local and operator-owned (ADR 0009); this is a schema, not a trust
//! boundary.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::models::{
    AppSnapshot, AppTarget, Change, ConversationSnapshot, ConversationTarget, FileSnapshot,
    FileTarget, OutputSnapshot, OutputTarget, Proposal, Retired, RetiredSnapshot, RulesSnapshot,
    RulesTarget, SkillSnapshot, SkillTarget, Snapshot, Target,
};

const APPS: &str = "ghost_core.apps";
const CONVERSATIONS: &str = "ghost_core.conversations.models";
const OUTPUTS: &str = "ghost_core.outputs";
const RULES: &str = "ghost_core.rules";
const SKILLS: &str = "ghost_core.skills";
const FILES: &str = "ghost_core.targets.files";
#[cfg(any(test, feature = "fakes"))]
const FAKES: &str = "fakes";

#[derive(Serialize, Deserialize)]
struct Tagged {
    #[serde(rename = "$type")]
    type_name: String,
    data: Value,
}

fn tag(module: &str, class: &str) -> String {
    format!("{module}:{class}")
}

fn tagged<S: Serializer, T: Serialize>(
    module: &str,
    class: &str,
    data: &T,
    s: S,
) -> Result<S::Ok, S::Error> {
    let data = serde_json::to_value(data).map_err(serde::ser::Error::custom)?;
    Tagged {
        type_name: tag(module, class),
        data,
    }
    .serialize(s)
}

fn parse<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(
    data: Value,
) -> Result<T, D::Error> {
    serde_json::from_value(data).map_err(D::Error::custom)
}

fn object(data: Value) -> Map<String, Value> {
    match data {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The tags each kind knows: a tag of another kind is refused, not retired.
const TARGET_TAGS: &[(&str, &str)] = &[
    (APPS, "AppTarget"),
    (CONVERSATIONS, "ConversationTarget"),
    (OUTPUTS, "OutputTarget"),
    (RULES, "RulesTarget"),
    (SKILLS, "SkillTarget"),
    (FILES, "FileTarget"),
    #[cfg(any(test, feature = "fakes"))]
    (FAKES, "Issue"),
];
const SNAPSHOT_TAGS: &[(&str, &str)] = &[
    (APPS, "AppSnapshot"),
    (CONVERSATIONS, "ConversationSnapshot"),
    (OUTPUTS, "OutputSnapshot"),
    (RULES, "RulesSnapshot"),
    (SKILLS, "SkillSnapshot"),
    (FILES, "FileSnapshot"),
    #[cfg(any(test, feature = "fakes"))]
    (FAKES, "IssueSnap"),
];
const CHANGE_TAGS: &[(&str, &str)] = &[
    (APPS, "AppChange"),
    (CONVERSATIONS, "Create"),
    (CONVERSATIONS, "Retire"),
    (CONVERSATIONS, "Rehire"),
    (CONVERSATIONS, "Rename"),
    (CONVERSATIONS, "SetVoice"),
    (CONVERSATIONS, "SetPersona"),
    (CONVERSATIONS, "AppendNote"),
    (CONVERSATIONS, "SetNotes"),
    (CONVERSATIONS, "SetMemory"),
    (CONVERSATIONS, "Reset"),
    (CONVERSATIONS, "Delete"),
    (OUTPUTS, "WriteFile"),
    (RULES, "AddRule"),
    (SKILLS, "AddSkill"),
    (SKILLS, "InstallSkill"),
    (SKILLS, "RemoveSkill"),
    (FILES, "AppendText"),
    #[cfg(any(test, feature = "fakes"))]
    (FAKES, "SetLabels"),
    #[cfg(any(test, feature = "fakes"))]
    (FAKES, "AddComment"),
    #[cfg(any(test, feature = "fakes"))]
    (FAKES, "SetState"),
];

fn known(tags: &[(&str, &str)], type_name: &str) -> bool {
    tags.iter().any(|(m, c)| tag(m, c) == type_name)
}

fn wrong_kind<E: serde::de::Error>(type_name: &str, kind: &str) -> Option<E> {
    let all = TARGET_TAGS.iter().chain(SNAPSHOT_TAGS).chain(CHANGE_TAGS);
    if all.into_iter().any(|(m, c)| tag(m, c) == type_name) {
        Some(E::custom(format!("{type_name} is not a {kind}")))
    } else {
        None
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Target::App(t) => tagged(APPS, "AppTarget", t, s),
            Target::Conversation(t) => tagged(CONVERSATIONS, "ConversationTarget", t, s),
            Target::Output(t) => tagged(OUTPUTS, "OutputTarget", t, s),
            Target::Rules(t) => tagged(RULES, "RulesTarget", t, s),
            Target::Skill(t) => tagged(SKILLS, "SkillTarget", t, s),
            Target::File(t) => tagged(FILES, "FileTarget", t, s),
            Target::Retired(r) => Tagged {
                type_name: r.type_name.clone(),
                data: Value::Object(r.data.clone()),
            }
            .serialize(s),
            #[cfg(any(test, feature = "fakes"))]
            Target::FakeIssue(t) => tagged(FAKES, "Issue", t, s),
        }
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Tagged { type_name, data } = Tagged::deserialize(d)?;
        let (module, class) = type_name
            .split_once(':')
            .unwrap_or(("", type_name.as_str()));
        Ok(match (module, class) {
            (APPS, "AppTarget") => Target::App(parse::<D, AppTarget>(data)?),
            (CONVERSATIONS, "ConversationTarget") => {
                Target::Conversation(parse::<D, ConversationTarget>(data)?)
            }
            (OUTPUTS, "OutputTarget") => Target::Output(parse::<D, OutputTarget>(data)?),
            (RULES, "RulesTarget") => Target::Rules(parse::<D, RulesTarget>(data)?),
            (SKILLS, "SkillTarget") => Target::Skill(parse::<D, SkillTarget>(data)?),
            (FILES, "FileTarget") => Target::File(parse::<D, FileTarget>(data)?),
            #[cfg(any(test, feature = "fakes"))]
            (FAKES, "Issue") => Target::FakeIssue(parse::<D, crate::models::FakeIssue>(data)?),
            _ => {
                if let Some(e) = wrong_kind::<D::Error>(&type_name, "target") {
                    return Err(e);
                }
                Target::Retired(Retired {
                    type_name,
                    data: object(data),
                })
            }
        })
    }
}

impl Serialize for Snapshot {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Snapshot::App(v) => tagged(APPS, "AppSnapshot", v, s),
            Snapshot::Conversation(v) => tagged(CONVERSATIONS, "ConversationSnapshot", v, s),
            Snapshot::Output(v) => tagged(OUTPUTS, "OutputSnapshot", v, s),
            Snapshot::Rules(v) => tagged(RULES, "RulesSnapshot", v, s),
            Snapshot::Skill(v) => tagged(SKILLS, "SkillSnapshot", v, s),
            Snapshot::File(v) => tagged(FILES, "FileSnapshot", v, s),
            Snapshot::Retired(r) => {
                let mut data = r.data.clone();
                data.entry("version")
                    .or_insert_with(|| Value::String(r.version.clone()));
                Tagged {
                    type_name: r.type_name.clone(),
                    data: Value::Object(data),
                }
                .serialize(s)
            }
            #[cfg(any(test, feature = "fakes"))]
            Snapshot::FakeIssue(v) => tagged(FAKES, "IssueSnap", v, s),
        }
    }
}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Tagged { type_name, data } = Tagged::deserialize(d)?;
        let (module, class) = type_name
            .split_once(':')
            .unwrap_or(("", type_name.as_str()));
        Ok(match (module, class) {
            (APPS, "AppSnapshot") => Snapshot::App(parse::<D, AppSnapshot>(data)?),
            (CONVERSATIONS, "ConversationSnapshot") => {
                Snapshot::Conversation(parse::<D, ConversationSnapshot>(data)?)
            }
            (OUTPUTS, "OutputSnapshot") => Snapshot::Output(parse::<D, OutputSnapshot>(data)?),
            (RULES, "RulesSnapshot") => Snapshot::Rules(parse::<D, RulesSnapshot>(data)?),
            (SKILLS, "SkillSnapshot") => Snapshot::Skill(parse::<D, SkillSnapshot>(data)?),
            (FILES, "FileSnapshot") => Snapshot::File(parse::<D, FileSnapshot>(data)?),
            #[cfg(any(test, feature = "fakes"))]
            (FAKES, "IssueSnap") => {
                Snapshot::FakeIssue(parse::<D, crate::models::FakeIssueSnap>(data)?)
            }
            _ => {
                if let Some(e) = wrong_kind::<D::Error>(&type_name, "snapshot") {
                    return Err(e);
                }
                let data = object(data);
                let version = data
                    .get("version")
                    .map(crate::models::py_str)
                    .unwrap_or_default();
                Snapshot::Retired(RetiredSnapshot {
                    version,
                    type_name,
                    data,
                })
            }
        })
    }
}

impl Serialize for Change {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let empty = Map::new();
        match self {
            Change::App(v) => tagged(APPS, "AppChange", v, s),
            Change::Create(v) => tagged(CONVERSATIONS, "Create", v, s),
            Change::Retire => tagged(CONVERSATIONS, "Retire", &empty, s),
            Change::Rehire => tagged(CONVERSATIONS, "Rehire", &empty, s),
            Change::Rename(v) => tagged(CONVERSATIONS, "Rename", v, s),
            Change::SetVoice(v) => tagged(CONVERSATIONS, "SetVoice", v, s),
            Change::SetPersona(v) => tagged(CONVERSATIONS, "SetPersona", v, s),
            Change::AppendNote(v) => tagged(CONVERSATIONS, "AppendNote", v, s),
            Change::SetNotes(v) => tagged(CONVERSATIONS, "SetNotes", v, s),
            Change::SetMemory(v) => tagged(CONVERSATIONS, "SetMemory", v, s),
            Change::Reset => tagged(CONVERSATIONS, "Reset", &empty, s),
            Change::Delete => tagged(CONVERSATIONS, "Delete", &empty, s),
            Change::WriteFile(v) => tagged(OUTPUTS, "WriteFile", v, s),
            Change::AddRule(v) => tagged(RULES, "AddRule", v, s),
            Change::AddSkill(v) => tagged(SKILLS, "AddSkill", v, s),
            Change::InstallSkill(v) => tagged(SKILLS, "InstallSkill", v, s),
            Change::RemoveSkill => tagged(SKILLS, "RemoveSkill", &empty, s),
            Change::AppendText(v) => tagged(FILES, "AppendText", v, s),
            Change::Retired(r) => Tagged {
                type_name: r.type_name.clone(),
                data: Value::Object(r.data.clone()),
            }
            .serialize(s),
            #[cfg(any(test, feature = "fakes"))]
            Change::SetLabels(v) => tagged(FAKES, "SetLabels", v, s),
            #[cfg(any(test, feature = "fakes"))]
            Change::AddComment(v) => tagged(FAKES, "AddComment", v, s),
            #[cfg(any(test, feature = "fakes"))]
            Change::SetState(v) => tagged(FAKES, "SetState", v, s),
        }
    }
}

impl<'de> Deserialize<'de> for Change {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use crate::models::*;
        let Tagged { type_name, data } = Tagged::deserialize(d)?;
        let (module, class) = type_name
            .split_once(':')
            .unwrap_or(("", type_name.as_str()));
        Ok(match (module, class) {
            (APPS, "AppChange") => Change::App(parse::<D, AppChange>(data)?),
            (CONVERSATIONS, "Create") => Change::Create(parse::<D, Create>(data)?),
            (CONVERSATIONS, "Retire") => Change::Retire,
            (CONVERSATIONS, "Rehire") => Change::Rehire,
            (CONVERSATIONS, "Rename") => Change::Rename(parse::<D, Rename>(data)?),
            (CONVERSATIONS, "SetVoice") => Change::SetVoice(parse::<D, SetVoice>(data)?),
            (CONVERSATIONS, "SetPersona") => Change::SetPersona(parse::<D, SetPersona>(data)?),
            (CONVERSATIONS, "AppendNote") => Change::AppendNote(parse::<D, AppendNote>(data)?),
            (CONVERSATIONS, "SetNotes") => Change::SetNotes(parse::<D, SetNotes>(data)?),
            (CONVERSATIONS, "SetMemory") => Change::SetMemory(parse::<D, SetMemory>(data)?),
            (CONVERSATIONS, "Reset") => Change::Reset,
            (CONVERSATIONS, "Delete") => Change::Delete,
            (OUTPUTS, "WriteFile") => Change::WriteFile(parse::<D, WriteFile>(data)?),
            (RULES, "AddRule") => Change::AddRule(parse::<D, AddRule>(data)?),
            (SKILLS, "AddSkill") => Change::AddSkill(parse::<D, AddSkill>(data)?),
            (SKILLS, "InstallSkill") => Change::InstallSkill(parse::<D, InstallSkill>(data)?),
            (SKILLS, "RemoveSkill") => Change::RemoveSkill,
            (FILES, "AppendText") => Change::AppendText(parse::<D, AppendText>(data)?),
            #[cfg(any(test, feature = "fakes"))]
            (FAKES, "SetLabels") => Change::SetLabels(parse::<D, SetLabels>(data)?),
            #[cfg(any(test, feature = "fakes"))]
            (FAKES, "AddComment") => Change::AddComment(parse::<D, AddComment>(data)?),
            #[cfg(any(test, feature = "fakes"))]
            (FAKES, "SetState") => Change::SetState(parse::<D, SetState>(data)?),
            _ => {
                if let Some(e) = wrong_kind::<D::Error>(&type_name, "change") {
                    return Err(e);
                }
                Change::Retired(Retired {
                    type_name,
                    data: object(data),
                })
            }
        })
    }
}

/// A proposal's stored body.
pub fn dump_proposal(p: &Proposal) -> String {
    serde_json::to_string(p).expect("a proposal serializes")
}

pub fn load_proposal(text: &str) -> Result<Proposal, serde_json::Error> {
    serde_json::from_str(text)
}

#[allow(dead_code)]
fn _tags_are_used() -> bool {
    known(TARGET_TAGS, "") || known(SNAPSHOT_TAGS, "") || known(CHANGE_TAGS, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;
    use serde_json::json;

    fn issue() -> Target {
        Target::FakeIssue(FakeIssue {
            owner: "a".into(),
            repo: "b".into(),
            number: 1,
        })
    }

    fn proposal(changes: ChangeSet) -> Proposal {
        Proposal::new(Draft {
            session_id: "s".into(),
            ability: "x".into(),
            operation: "y".into(),
            target: issue(),
            changes,
            rationale: String::new(),
            snapshot: Snapshot::FakeIssue(FakeIssueSnap {
                version: "v1".into(),
                title: "T".into(),
                state: "open".into(),
                labels: vec![],
            }),
            preview: "p".into(),
        })
    }

    #[test]
    fn round_trip_keeps_every_kind() {
        let t = issue();
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["$type"], "fakes:Issue");
        assert_eq!(serde_json::from_value::<Target>(v).unwrap(), t);
        let p = proposal(
            ChangeSet::new(vec![
                Change::SetLabels(SetLabels {
                    add: vec!["bug".into()],
                    remove: vec![],
                }),
                Change::AddComment(AddComment { body: "hi".into() }),
            ])
            .unwrap(),
        );
        let back = load_proposal(&dump_proposal(&p)).unwrap();
        assert_eq!(back, p);
        assert!(matches!(back.changes.steps()[1], Change::AddComment(_)));
    }

    #[test]
    fn the_stored_body_has_the_shape_the_python_daemon_wrote() {
        let p = proposal(ChangeSet::one(Change::SetState(SetState {
            state: "closed".into(),
        })));
        let body: Value = serde_json::from_str(&dump_proposal(&p)).unwrap();
        assert_eq!(body["target"]["$type"], "fakes:Issue");
        assert_eq!(body["target"]["data"]["number"], 1);
        assert_eq!(body["snapshot"]["$type"], "fakes:IssueSnap");
        assert_eq!(body["changes"][0]["$type"], "fakes:SetState");
        assert_eq!(body["changes"][0]["data"]["state"], "closed");
        assert_eq!(body["status"], "pending");
        assert!(body["created_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(body["ability"], "x");
    }

    #[test]
    fn load_refuses_a_tag_of_the_wrong_kind() {
        let v = serde_json::to_value(issue()).unwrap();
        let err = serde_json::from_value::<Snapshot>(v)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is not a snapshot"), "{err}");
    }

    #[test]
    fn a_retired_change_class_still_loads_for_the_audit() {
        let p = proposal(ChangeSet::one(Change::SetLabels(SetLabels {
            add: vec!["bug".into()],
            remove: vec![],
        })));
        let mut body: Value = serde_json::from_str(&dump_proposal(&p)).unwrap();
        body["operation"] = json!("undo:y");
        body["undo_of"] = json!("abc");
        body["changes"] =
            json!([{"$type": "fakes:DeleteComment", "data": {"marker": "ghost:abc"}}]);
        let back = load_proposal(&body.to_string()).unwrap();
        assert_eq!(back.operation, "undo:y");
        assert!(matches!(back.changes.steps()[0], Change::Retired(_)));
        assert!(back.changes.describe().contains("no longer supported"));
    }

    #[test]
    fn rows_from_a_retired_ability_still_read() {
        let row = json!({
            "id": "abc", "session_id": "s", "ability": "github_issues", "operation": "close",
            "rationale": "", "preview": "On acme/app#7: close the issue.", "autonomy": "confirm",
            "created_at": "2026-10-19T10:00:00Z", "expires_at": "2026-10-19T10:01:00Z", "status": "executed",
            "target": {"$type": "ghost_retired_ability.models:IssueTarget", "data": {"owner": "acme", "repo": "app", "number": 7}},
            "snapshot": {"$type": "ghost_retired_ability.models:IssueSnapshot", "data": {"version": "2026-10-19T09:00:00Z", "state": "open", "labels": ["bug"]}},
            "changes": [{"$type": "ghost_retired_ability.models:SetState", "data": {"state": "closed"}}]
        });
        let p = load_proposal(&row.to_string()).unwrap();
        assert!(matches!(p.target, Target::Retired(_)));
        assert_eq!(p.target.spoken(), "acme/app#7");
        assert_eq!(p.target.scope(), "acme/app");
        assert_eq!(p.snapshot.version(), "2026-10-19T09:00:00Z");
        assert_eq!(p.snapshot.spoken(), "open, labels bug");
        assert!(p.changes.describe().ends_with("(no longer supported)"));
        assert_eq!(p.status, ProposalStatus::Executed);
        // And it writes back as it was read.
        let again: Value = serde_json::from_str(&dump_proposal(&p)).unwrap();
        assert_eq!(
            again["target"]["$type"],
            "ghost_retired_ability.models:IssueTarget"
        );
        assert_eq!(again["snapshot"]["data"]["version"], "2026-10-19T09:00:00Z");
    }

    #[test]
    fn real_sources_round_trip_with_their_python_tags() {
        let cases: Vec<(Target, &str)> = vec![
            (
                Target::App(AppTarget {
                    app: "github".into(),
                    tool: "issue_write".into(),
                    fields: Map::new(),
                    title: String::new(),
                }),
                "ghost_core.apps:AppTarget",
            ),
            (
                Target::Conversation(ConversationTarget {
                    slug: "ledger".into(),
                }),
                "ghost_core.conversations.models:ConversationTarget",
            ),
            (
                Target::Output(OutputTarget {
                    name: "a.md".into(),
                }),
                "ghost_core.outputs:OutputTarget",
            ),
            (
                Target::Rules(RulesTarget {}),
                "ghost_core.rules:RulesTarget",
            ),
            (
                Target::Skill(SkillTarget { name: "pr".into() }),
                "ghost_core.skills:SkillTarget",
            ),
            (
                Target::File(FileTarget {
                    path: "/tmp/x".into(),
                }),
                "ghost_core.targets.files:FileTarget",
            ),
        ];
        for (t, tag) in cases {
            let v = serde_json::to_value(&t).unwrap();
            assert_eq!(v["$type"], tag);
            assert_eq!(serde_json::from_value::<Target>(v).unwrap(), t);
        }
        let c = Change::InstallSkill(InstallSkill {
            staged_path: "/s".into(),
            source: "o/r".into(),
            skill_path: "skills/pr".into(),
            commit: "abcdef0123".into(),
            folder_hash: "h".into(),
            git_ref: Some("main".into()),
            replace: false,
        });
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["$type"], "ghost_core.skills:InstallSkill");
        assert_eq!(v["data"]["ref"], "main");
        assert_eq!(serde_json::from_value::<Change>(v).unwrap(), c);
        let v = serde_json::to_value(Change::Retire).unwrap();
        assert_eq!(
            v,
            json!({"$type": "ghost_core.conversations.models:Retire", "data": {}})
        );
        // A Python row carries the `kind` discriminator; it is ignored on the way in.
        let v = json!({"$type": "ghost_core.conversations.models:Create", "data": {"kind": "create", "name": "Ledger"}});
        assert!(
            matches!(serde_json::from_value::<Change>(v).unwrap(), Change::Create(c) if c.name == "Ledger")
        );
    }
}
