//! `/skill-catalog` (#35): search skills.sh, preview a skill from GitHub, install it
//! through the gate, check for and preview an update. The network is a small fake.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use common::World;
use ghost_core::skill_catalog::{Entry, SkillCatalog, blob_hash, folder_hash};
use ghost_core::skills::{SOURCE, Skills, SkillsFrontMatter};
use serde_json::json;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SKILL_MD: &[u8] = b"---\nname: pdf\ndescription: Read PDFs.\n---\n\nUse scripts/fill.py.\n";

/// One repository whose skill's forms.md changes when `version` is bumped.
struct Web {
    version: AtomicU32,
    search_fails: AtomicBool,
}

impl Web {
    fn files(&self) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        files.insert("skills/pdf/SKILL.md".to_string(), SKILL_MD.to_vec());
        files.insert(
            "skills/pdf/forms.md".to_string(),
            format!("Forms, version {}.\n", self.version.load(Ordering::SeqCst)).into_bytes(),
        );
        files.insert(
            "skills/pdf/scripts/fill.py".to_string(),
            b"print('fill')\n".to_vec(),
        );
        files
    }

    fn commit(&self) -> String {
        format!("{:040x}", self.version.load(Ordering::SeqCst) + 1)
    }

    /// The tree GitHub would answer: every blob, and a tree per folder with its hash.
    fn entries(&self) -> Vec<serde_json::Value> {
        let files = self.files();
        let mut blobs: Vec<Entry> = files
            .iter()
            .map(|(p, d)| Entry::new(p, "blob", "100644", &blob_hash(d), d.len()))
            .collect();
        let mut dirs: Vec<String> = blobs
            .iter()
            .flat_map(|e| {
                let parts: Vec<&str> = e.path.split('/').collect();
                (1..parts.len())
                    .map(move |i| parts[..i].join("/"))
                    .collect::<Vec<_>>()
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        dirs.sort_by_key(|d| (std::cmp::Reverse(d.matches('/').count()), d.clone()));
        let mut trees: Vec<Entry> = Vec::new();
        for d in dirs {
            let inner: Vec<Entry> = blobs
                .iter()
                .chain(trees.iter())
                .filter(|e| e.path.starts_with(&format!("{d}/")))
                .map(|e| Entry::new(&e.path[d.len() + 1..], &e.kind, &e.mode, &e.sha, e.size))
                .collect();
            trees.push(Entry::new(
                &d,
                "tree",
                "040000",
                &folder_hash(&inner, "").unwrap(),
                0,
            ));
        }
        blobs.extend(trees);
        blobs
            .iter()
            .map(|e| json!({"path": e.path, "type": e.kind, "mode": e.mode, "sha": e.sha, "size": e.size}))
            .collect()
    }
}

struct Serve(Arc<Web>);

impl Respond for Serve {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let web = &self.0;
        let path = request.url.path().to_string();
        if path == "/api/search" {
            if web.search_fails.load(Ordering::SeqCst) {
                return ResponseTemplate::new(502).set_body_bytes(b"bad gateway".to_vec());
            }
            let hits = json!({"skills": [{"source": "anthropics/skills", "skillId": "pdf", "name": "pdf", "installs": 7}]});
            return ResponseTemplate::new(200).set_body_bytes(serde_json::to_vec(&hits).unwrap());
        }
        if path.contains("/commits/") {
            return ResponseTemplate::new(200).set_body_bytes(web.commit().into_bytes());
        }
        if path.contains("/git/trees/") {
            let body = json!({"tree": web.entries(), "truncated": false});
            return ResponseTemplate::new(200).set_body_bytes(serde_json::to_vec(&body).unwrap());
        }
        let marker = format!("/{}/", web.commit());
        if let Some((_, file)) = path.split_once(&marker)
            && let Some(data) = web.files().get(file)
        {
            return ResponseTemplate::new(200).set_body_bytes(data.clone());
        }
        ResponseTemplate::new(404).set_body_string(format!("unexpected fetch {path}"))
    }
}

fn catalog(server: &MockServer, dir: &std::path::Path) -> Arc<SkillCatalog> {
    Arc::new(
        SkillCatalog::new(dir, Arc::new(SkillsFrontMatter)).with_endpoints(
            &format!("{}/api/search", server.uri()),
            &server.uri(),
            &format!("{}/raw", server.uri()),
        ),
    )
}

#[tokio::test]
async fn find_preview_install_and_update() {
    let mut w = World::new();
    let c = w.client();
    assert_eq!(c.get("/skill-catalog?search=pdf").await.status, 503);
    let web = Arc::new(Web {
        version: AtomicU32::new(0),
        search_fails: AtomicBool::new(false),
    });
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(Serve(web.clone()))
        .mount(&server)
        .await;
    let skills = Arc::new(Skills::new(
        &w.path().join("ghost-skills"),
        Some(&w.path().join("home-skills")),
    ));
    w.gate.register(SOURCE, skills.gate_operations());
    w.skills = Some(skills);
    let first = catalog(&server, &w.path().join("previews"));
    w.skill_catalog = Some(first.clone());
    let c = w.client();

    let page = c.get("/skill-catalog?search=pdf").await;
    assert_eq!(page["problem"], serde_json::Value::Null, "{}", page.text);
    assert!(page["fetched_at"].is_string());
    assert_eq!(
        page["skills"],
        json!([{
            "id": "anthropics/skills/pdf",
            "source": "anthropics/skills",
            "skill": "pdf",
            "name": "pdf",
            "installs": 7,
            "installed": null,
        }])
    );

    let r = c
        .post(
            "/skill-catalog/preview",
            Some(json!({"source": "anthropics/skills/pdf"})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let preview = r.json.clone();
    assert_eq!(
        preview["skill_md"],
        String::from_utf8_lossy(SKILL_MD).as_ref()
    );
    assert_eq!(preview["scripts"], json!(["scripts/fill.py"]));
    assert_eq!(preview["commit"], web.commit());
    assert_eq!(preview["installed"], serde_json::Value::Null);
    assert_eq!(
        common::strings(&preview["files"], "path"),
        vec!["forms.md".to_string(), "scripts/fill.py".to_string()]
    );
    let bad = c
        .post(
            "/skill-catalog/preview",
            Some(json!({"source": "https://gitlab.com/a/b"})),
        )
        .await;
    assert_eq!(bad.status, 409);
    assert!(bad.detail().contains("GitHub only"));

    let r = c
        .post(
            "/skill-catalog/install",
            Some(json!({"token": preview["token"]})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let info = &r.json;
    assert_eq!(info["name"], "pdf");
    assert_eq!(info["source"], "ghost");
    assert_eq!(info["origin"]["source"], "anthropics/skills");
    assert_eq!(info["origin"]["commit"], web.commit());
    // The install is audited like any write from the desktop.
    let audit = c.get("/audit").await;
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"] == "skills" && e["operation"] == "install")
    );
    // The token is spent.
    assert_eq!(
        c.post(
            "/skill-catalog/install",
            Some(json!({"token": preview["token"]}))
        )
        .await
        .status,
        404
    );
    let listed = c.get("/skills").await;
    assert_eq!(listed[0]["origin"]["skill_path"], "skills/pdf/SKILL.md");
    let marked = c.get("/skill-catalog?search=pdf").await["skills"][0].clone();
    assert_eq!(marked["installed"], "pdf");

    let updates = c.get("/skill-catalog/updates").await;
    let shape: Vec<(String, bool)> = updates
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["name"].as_str().unwrap().to_string(),
                u["update"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(shape, vec![("pdf".to_string(), false)]);
    // The repository moves on; a catalog that hasn't checked lately asks again.
    web.version.store(1, Ordering::SeqCst);
    let searched = c;
    w.skill_catalog = Some(catalog(&server, &w.path().join("previews2")));
    let c = w.client();
    let updates = c.get("/skill-catalog/updates").await;
    assert_eq!(updates[0]["update"], true, "{}", updates.text);
    assert_eq!(updates[0]["latest_commit"], web.commit());
    let newer = c.post("/skill-catalog/updates/pdf", None).await;
    assert_eq!(newer["update_of"], "pdf", "{}", newer.text);
    assert_eq!(newer["installed"], "ghost");
    let plain = c
        .post(
            "/skill-catalog/install",
            Some(json!({"token": newer["token"]})),
        )
        .await;
    assert_eq!(plain.status, 409);
    assert!(plain.detail().contains("already installed"));
    let newer = c.post("/skill-catalog/updates/pdf", None).await;
    let r = c
        .post(
            "/skill-catalog/install",
            Some(json!({"token": newer["token"], "name": "pdf", "replace": true})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r["origin"]["commit"], web.commit());
    assert_eq!(
        std::fs::read_to_string(w.path().join("ghost-skills").join("pdf").join("forms.md"))
            .unwrap(),
        "Forms, version 1.\n"
    );
    assert_eq!(
        c.post("/skill-catalog/updates/nope", None).await.status,
        404
    );

    // skills.sh down: the page says so, with the last results it had for the search.
    web.search_fails.store(true, Ordering::SeqCst);
    first.set_search_ttl(0.0);
    let page = searched.get("/skill-catalog?search=pdf").await;
    assert_eq!(page["problem"], "skills.sh answered 502");
    assert_eq!(page["skills"].as_array().unwrap().len(), 1);
}
