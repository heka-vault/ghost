"""``/skill-catalog`` (#35): search skills.sh, preview a skill from GitHub, install it
through the gate, check for and preview an update. The network is a small fake."""

from __future__ import annotations

import json
from collections.abc import Mapping
from pathlib import Path

from ghost_core.skill_catalog import Entry, Reply, SkillCatalog, blob_hash, folder_hash
from ghost_core.skills import SOURCE, Skills

SKILL_MD = b"---\nname: pdf\ndescription: Read PDFs.\n---\n\nUse scripts/fill.py.\n"


class Web:
    """One repository whose skill's forms.md changes when ``bump`` is called."""

    def __init__(self) -> None:
        self.version = 0
        self.search_fails = False

    def files(self) -> dict[str, bytes]:
        return {
            "skills/pdf/SKILL.md": SKILL_MD,
            "skills/pdf/forms.md": f"Forms, version {self.version}.\n".encode(),
            "skills/pdf/scripts/fill.py": b"print('fill')\n",
        }

    @property
    def commit(self) -> str:
        return f"{self.version + 1:040x}"

    def entries(self) -> list[dict[str, object]]:
        blobs = [Entry(p, "blob", "100644", blob_hash(d), len(d)) for p, d in self.files().items()]
        out = [
            {"path": e.path, "type": "blob", "mode": e.mode, "sha": e.sha, "size": e.size}
            for e in blobs
        ]
        scripts = [Entry("fill.py", "blob", "100644", blobs[2].sha)]
        scripts_sha = folder_hash(scripts, "")
        pdf = [
            Entry("SKILL.md", "blob", "100644", blobs[0].sha),
            Entry("forms.md", "blob", "100644", blobs[1].sha),
            Entry("scripts", "tree", "040000", scripts_sha),
        ]
        out.append(
            {"path": "skills/pdf/scripts", "type": "tree", "mode": "040000", "sha": scripts_sha}
        )
        out.append(
            {"path": "skills/pdf", "type": "tree", "mode": "040000", "sha": folder_hash(pdf, "")}
        )
        return out

    async def get(self, url: str, headers: Mapping[str, str], limit: int) -> Reply:
        if url.startswith("https://skills.sh/"):
            if self.search_fails:
                return Reply(502, b"bad gateway")
            hits = [{"source": "anthropics/skills", "skillId": "pdf", "name": "pdf", "installs": 7}]
            return Reply(200, json.dumps({"skills": hits}).encode())
        if "/commits/" in url:
            return Reply(200, self.commit.encode())
        if "/git/trees/" in url:
            return Reply(200, json.dumps({"tree": self.entries(), "truncated": False}).encode())
        path = url.split(f"/{self.commit}/", 1)[1]
        return Reply(200, self.files()[path])


def test_find_preview_install_and_update(world, tmp_path: Path):
    client, state, control, gate, writer, loaded, deps = world
    assert client.get("/skill-catalog", params={"search": "pdf"}).status_code == 503
    web = Web()
    skills = Skills(tmp_path / "ghost-skills", tmp_path / "home-skills")
    gate.register(SOURCE, skills.gate_operations())
    deps.skills = skills
    deps.skill_catalog = SkillCatalog(tmp_path / "previews", get=web.get)

    page = client.get("/skill-catalog", params={"search": "pdf"}).json()
    assert page["problem"] is None and page["fetched_at"]
    assert page["skills"] == [
        {
            "id": "anthropics/skills/pdf",
            "source": "anthropics/skills",
            "skill": "pdf",
            "name": "pdf",
            "installs": 7,
            "installed": None,
        }
    ]

    r = client.post("/skill-catalog/preview", json={"source": "anthropics/skills/pdf"})
    assert r.status_code == 200, r.text
    preview = r.json()
    assert preview["skill_md"] == SKILL_MD.decode() and preview["scripts"] == ["scripts/fill.py"]
    assert preview["commit"] == web.commit and preview["installed"] is None
    assert [f["path"] for f in preview["files"]] == ["forms.md", "scripts/fill.py"]
    bad = client.post("/skill-catalog/preview", json={"source": "https://gitlab.com/a/b"})
    assert bad.status_code == 409 and "GitHub only" in bad.json()["detail"]

    r = client.post("/skill-catalog/install", json={"token": preview["token"]})
    assert r.status_code == 200, r.text
    info = r.json()
    assert info["name"] == "pdf" and info["source"] == "ghost"
    assert info["origin"]["source"] == "anthropics/skills"
    assert info["origin"]["commit"] == web.commit
    # The install is audited like any write from the desktop.
    audit = client.get("/audit").json()
    entries = audit if isinstance(audit, list) else audit.get("items", [])
    assert any(e["source"] == "skills" and e["operation"] == "install" for e in entries)
    # The token is spent.
    assert (
        client.post("/skill-catalog/install", json={"token": preview["token"]}).status_code == 404
    )
    listed = client.get("/skills").json()
    assert listed[0]["origin"]["skill_path"] == "skills/pdf/SKILL.md"
    marked = client.get("/skill-catalog", params={"search": "pdf"}).json()["skills"][0]
    assert marked["installed"] == "pdf"

    updates = client.get("/skill-catalog/updates").json()
    assert [(u["name"], u["update"]) for u in updates] == [("pdf", False)]
    web.version = 1
    deps.skill_catalog._checked.clear()
    updates = client.get("/skill-catalog/updates").json()
    assert updates[0]["update"] is True and updates[0]["latest_commit"] == web.commit
    newer = client.post("/skill-catalog/updates/pdf").json()
    assert newer["update_of"] == "pdf" and newer["installed"] == "ghost"
    plain = client.post("/skill-catalog/install", json={"token": newer["token"]})
    assert plain.status_code == 409 and "already installed" in plain.json()["detail"]
    newer = client.post("/skill-catalog/updates/pdf").json()
    r = client.post(
        "/skill-catalog/install", json={"token": newer["token"], "name": "pdf", "replace": True}
    )
    assert r.status_code == 200 and r.json()["origin"]["commit"] == web.commit
    assert (tmp_path / "ghost-skills" / "pdf" / "forms.md").read_text() == "Forms, version 1.\n"
    assert client.post("/skill-catalog/updates/nope").status_code == 404

    # skills.sh down: the page says so, with the last results it had for the search.
    web.search_fails = True
    deps.skill_catalog.search_ttl = 0
    page = client.get("/skill-catalog", params={"search": "pdf"}).json()
    assert page["problem"] == "skills.sh answered 502" and len(page["skills"]) == 1
