"""The skills catalog (#35): skills.sh search, a skill fetched from GitHub and staged for
the operator to read, installed through the gate with where it came from, and the update
check. The network is a fake that answers as skills.sh and GitHub do, with real git hashes."""

from __future__ import annotations

import hashlib
import json
import shutil
import stat
import subprocess
from collections.abc import Mapping
from pathlib import Path

import pytest
from ghost_core import AutonomyPolicy, Store, WriteGate
from ghost_core.skill_catalog import (
    MAX_FILES,
    Entry,
    Reply,
    SkillCatalog,
    SkillCatalogError,
    blob_hash,
    folder_hash,
    is_script,
    parse_search,
    parse_source,
    skill_folders,
)
from ghost_core.skills import (
    LOCK_FILE,
    MAX_SKILL_BYTES,
    SOURCE,
    SkillOrigin,
    Skills,
    read_origins,
)

PDF_MD = (
    "---\nname: pdf\ndescription: Read and fill PDF forms.\n---\n\n# PDF\n\nRun scripts/fill.py.\n"
)


class FakeWeb:
    """skills.sh's search and GitHub's commits, trees and raw files, from dicts of files per
    repository. ``push`` moves a repository on to a new commit."""

    def __init__(self) -> None:
        self.repos: dict[str, list[tuple[str, dict[str, bytes | tuple[str, bytes]]]]] = {}
        self.search: object = {"skills": []}
        self.search_status = 200
        self.calls: list[str] = []
        self.raw_override: dict[str, bytes] = {}
        self.extra_entries: dict[str, list[dict[str, object]]] = {}
        self.rate_limited = False

    def push(self, repo: str, files: Mapping[str, bytes | str | tuple[str, bytes]]) -> str:
        norm: dict[str, bytes | tuple[str, bytes]] = {}
        for k, v in files.items():
            norm[k] = v.encode() if isinstance(v, str) else v
        history = self.repos.setdefault(repo, [])
        commit = hashlib.sha1(f"{repo}:{len(history)}:{sorted(norm)}".encode()).hexdigest()
        history.append((commit, norm))
        return commit

    def _entries(self, repo: str, files: dict[str, bytes | tuple[str, bytes]]) -> list[Entry]:
        blobs: list[Entry] = []
        for path, v in sorted(files.items()):
            mode, data = v if isinstance(v, tuple) else ("100644", v)
            blobs.append(Entry(path, "blob", mode, blob_hash(data), len(data)))
        dirs = sorted(
            {
                "/".join(e.path.split("/")[:i])
                for e in blobs
                for i in range(1, e.path.count("/") + 1)
            },
            key=lambda d: -d.count("/"),
        )
        trees: list[Entry] = []
        for d in dirs:  # deepest first, so a folder's subfolders have their hashes
            inner = [
                Entry(e.path[len(d) + 1 :], e.type, e.mode, e.sha, e.size)
                for e in blobs + trees
                if e.path.startswith(d + "/")
            ]
            trees.append(Entry(d, "tree", "040000", folder_hash(inner, ""), 0))
        return blobs + trees

    async def get(self, url: str, headers: Mapping[str, str], limit: int) -> Reply:
        self.calls.append(url)
        assert headers.get("User-Agent")
        if url.startswith("https://skills.sh/api/search"):
            return Reply(self.search_status, json.dumps(self.search).encode())
        if self.rate_limited and "api.github.com" in url:
            return Reply(403, b"{}", {"x-ratelimit-remaining": "0", "x-ratelimit-reset": "0"})
        if url.startswith("https://api.github.com/repos/"):
            rest = url.removeprefix("https://api.github.com/repos/")
            owner, repo, kind, ref = rest.split("/", 3)
            history = self.repos.get(f"{owner}/{repo}")
            if not history:
                return Reply(404, b"{}")
            if kind == "commits":
                return Reply(200, history[-1][0].encode())
            sha = ref.removeprefix("trees/").split("?")[0]
            files = next(f for c, f in history if c == sha)
            entries = [
                {"path": e.path, "type": e.type, "mode": e.mode, "sha": e.sha, "size": e.size}
                for e in self._entries(f"{owner}/{repo}", files)
            ]
            entries += self.extra_entries.get(f"{owner}/{repo}", [])
            body = json.dumps({"sha": sha, "tree": entries, "truncated": False}).encode()
            return Reply(200, body)
        if url.startswith("https://raw.githubusercontent.com/"):
            owner, repo, sha, path = url.removeprefix("https://raw.githubusercontent.com/").split(
                "/", 3
            )
            if path in self.raw_override:
                data = self.raw_override[path]
            else:
                files = next(f for c, f in self.repos[f"{owner}/{repo}"] if c == sha)
                v = files[path]
                data = v[1] if isinstance(v, tuple) else v
            assert len(data) <= limit or limit == 0 and not data, "read past the tree's size"
            return Reply(200, data)
        raise AssertionError(f"unexpected fetch {url}")


def pdf_repo(web: FakeWeb, **extra: bytes | str | tuple[str, bytes]) -> str:
    return web.push(
        "anthropics/skills",
        {
            "README.md": "# Skills\n",
            "skills/pdf/SKILL.md": PDF_MD,
            "skills/pdf/forms.md": "Fill forms like this.\n",
            "skills/pdf/scripts/fill.py": "print('fill')\n",
            "skills/pdf/scripts/run": ("100755", b"#!/bin/sh\necho run\n"),
            "skills/docx/SKILL.md": "---\nname: docx\ndescription: Word files.\n---\nDo it.\n",
            **extra,
        },
    )


@pytest.fixture
def web() -> FakeWeb:
    return FakeWeb()


@pytest.fixture
def catalog(web: FakeWeb, tmp_path: Path) -> SkillCatalog:
    return SkillCatalog(tmp_path / "cache" / "previews", get=web.get)


def gated(skills: Skills) -> WriteGate:
    gate = WriteGate(Store(), autonomy=AutonomyPolicy())
    gate.register(SOURCE, skills.gate_operations())
    return gate


async def install(gate: WriteGate, skills: Skills, preview, **kw):
    prepared = skills.prepare_install(preview.folder, preview.origin, **kw)
    return await gate.apply_internal(
        session_id="ui",
        ability=SOURCE,
        operation="install",
        target=prepared.target,
        changes=prepared.changes,
        rationale="test",
    )


# --- parsing ------------------------------------------------------------------------------


def test_search_answer_is_checked_field_by_field():
    listings = parse_search(
        {
            "query": "pdf",
            "skills": [
                {
                    "id": "x",
                    "source": "anthropics/skills",
                    "skillId": "pdf",
                    "name": "pdf",
                    "installs": 9,
                },
                {"source": "anthropics/skills", "skillId": "pdf", "name": "again"},  # duplicate
                {"source": "smithery.ai", "skillId": "pdf", "name": "not GitHub"},
                {"source": "a/b", "skillId": "../etc", "name": "bad id"},
                {"source": "a/b", "skillId": "ok", "name": "x\x00y", "installs": "lots"},
                "junk",
            ],
        }
    )
    assert [(s.id, s.name, s.installs) for s in listings] == [
        ("anthropics/skills/pdf", "pdf", 9),
        ("a/b/ok", "xy", 0),
    ]
    for bad in ({"results": []}, [], None, {"skills": "no"}):
        with pytest.raises(SkillCatalogError, match="shape"):
            parse_search(bad)


def test_a_source_is_a_repository_a_skill_in_one_or_a_folder_on_github():
    assert parse_source("anthropics/skills").source == "anthropics/skills"
    s = parse_source("anthropics/skills/pdf")
    assert (s.source, s.skill, s.path) == ("anthropics/skills", "pdf", None)
    s = parse_source("https://github.com/anthropics/skills/tree/main/skills/pdf")
    assert (s.ref, s.path, s.skill) == ("main", "skills/pdf", None)
    s = parse_source("github.com/a/b/blob/v1/x/SKILL.md")
    assert (s.ref, s.path) == ("v1", "x")
    assert parse_source("https://github.com/a/b.git").repo == "b"
    assert parse_source("a/b", "pdf").skill == "pdf"
    for bad in (
        "",
        "a",
        "a/b/c/d",
        "https://gitlab.com/a/b",
        "http://github.com/a/b",
        "https://github.com/a/b/tree/main/../../etc",
        "https://github.com/a/b/issues",
        "-a/b",
        "a/..",
        "a/b/../x",
    ):
        with pytest.raises(SkillCatalogError):
            parse_source(bad)


@pytest.mark.skipif(shutil.which("git") is None, reason="needs git")
def test_folder_hash_is_gits_tree_hash(tmp_path: Path):
    """What the lock file calls skillFolderHash is git's own tree hash; Ghost works it out
    from a tree listing for a skill at a repository's root."""
    repo = tmp_path / "repo"
    (repo / "sub" / "deep").mkdir(parents=True)
    (repo / "SKILL.md").write_text("---\nname: x\n---\n")
    (repo / "a-b.md").write_text("dash\n")
    (repo / "a.md").write_text("plain\n")
    (repo / "sub" / "run.sh").write_text("echo hi\n")
    (repo / "sub" / "run.sh").chmod(0o755)
    (repo / "sub" / "deep" / "x.txt").write_text("deep\n")
    (repo / "sub.txt").write_text("sorts after the folder in git's order\n")

    def git(*args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(repo), *args], check=True, capture_output=True, text=True
        ).stdout

    git("init", "-q")
    git("add", ".")
    git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x")
    entries = []
    for line in git("ls-tree", "-r", "-t", "-l", "HEAD").splitlines():
        meta, path = line.split("\t", 1)
        mode, kind, sha, size = meta.split()
        entries.append(Entry(path, kind, mode, sha, int(size) if size != "-" else 0))
    assert folder_hash(entries, "") == git("rev-parse", "HEAD^{tree}").strip()
    assert folder_hash(entries, "sub") == git("rev-parse", "HEAD:sub").strip()
    inner = [Entry(e.path[4:], e.type, e.mode, e.sha) for e in entries if e.path.startswith("sub/")]
    assert folder_hash(inner, "") == git("rev-parse", "HEAD:sub").strip()
    assert blob_hash(b"dash\n") == git("rev-parse", "HEAD:a-b.md").strip()


def test_skill_folders_and_scripts():
    e = [
        Entry("SKILL.md", "blob", "100644", "0" * 40),
        Entry("skills/a/SKILL.md", "blob", "100644", "0" * 40),
        Entry("node_modules/x/SKILL.md", "blob", "100644", "0" * 40),
        Entry("skills/b/skill.md", "blob", "100644", "0" * 40),
    ]
    assert skill_folders(e) == ["", "skills/a"]
    assert is_script("scripts/fill.py") and is_script("run", "100755")
    assert not is_script("forms.md")


# --- search -------------------------------------------------------------------------------


async def test_search_is_cached_and_says_when_skills_sh_fails(web: FakeWeb, catalog: SkillCatalog):
    web.search = {"skills": [{"source": "anthropics/skills", "skillId": "pdf", "name": "pdf"}]}
    assert (await catalog.search("p")).skills == []  # too short to ask
    assert web.calls == []
    first = await catalog.search("  PDF ")
    assert [s.id for s in first.skills] == ["anthropics/skills/pdf"] and first.problem is None
    again = await catalog.search("pdf")
    assert again.skills == first.skills and len(web.calls) == 1  # from the cache
    catalog.search_ttl = 0
    web.search_status = 500
    stale = await catalog.search("pdf")
    assert stale.skills == first.skills and stale.problem == "skills.sh answered 500"
    web.search_status = 200
    web.search = {"results": "a new shape"}
    fresh = await catalog.search("docx")
    assert fresh.skills == [] and "shape" in (fresh.problem or "")


# --- preview, install, update -------------------------------------------------------------


async def test_preview_stages_a_pinned_folder_and_install_goes_through_the_gate(
    web: FakeWeb, catalog: SkillCatalog, tmp_path: Path
):
    commit = pdf_repo(web)
    p = await catalog.preview("anthropics/skills/pdf")
    assert p.name == "pdf" and p.description == "Read and fill PDF forms."
    assert p.skill_md == PDF_MD
    assert [f.path for f in p.files] == ["forms.md", "scripts/fill.py", "scripts/run"]
    assert p.scripts == ["scripts/fill.py", "scripts/run"]
    assert p.origin.commit == commit and p.origin.skill_path == "skills/pdf/SKILL.md"
    tree = json.loads(
        (
            await web.get(
                f"https://api.github.com/repos/anthropics/skills/git/trees/{commit}",
                {"User-Agent": "t"},
                10**6,
            )
        ).body
    )
    assert p.origin.folder_hash == next(e["sha"] for e in tree["tree"] if e["path"] == "skills/pdf")
    # Nothing is made runnable: the file executable upstream is staged 0644.
    run = p.folder / "scripts" / "run"
    assert stat.S_IMODE(run.stat().st_mode) == 0o644
    assert not any(c.endswith("README.md") for c in web.calls)  # only the skill's files

    skills = Skills(tmp_path / "skills", tmp_path / "home")
    gate = gated(skills)
    out = await install(gate, skills, p)
    assert out.status == "success", out.error
    assert (tmp_path / "skills" / "pdf" / "scripts" / "fill.py").read_text() == "print('fill')\n"
    skills.refresh()
    assert skills.get("pdf") is not None
    origin = skills.origins()["pdf"]
    assert (origin.source, origin.commit, origin.installed_at != "") == (
        "anthropics/skills",
        commit,
        True,
    )
    lock = json.loads((tmp_path / "skills" / LOCK_FILE).read_text())
    assert lock["version"] == 3 and lock["skills"]["pdf"]["skillPath"] == "skills/pdf/SKILL.md"
    assert lock["skills"]["pdf"]["sourceUrl"] == "https://github.com/anthropics/skills.git"

    # The source hasn't moved: no update, and no tree read for it.
    assert not (await catalog.latest(origin)).moved_on(origin)
    # A change elsewhere in the repository moves the commit, not the skill.
    pdf_repo(web, **{"README.md": "# Skills, edited\n"})
    catalog._checked.clear()
    latest = await catalog.latest(origin)
    assert latest.commit != commit and not latest.moved_on(origin)
    # A change in the skill's folder is an update.
    new = pdf_repo(web, **{"skills/pdf/forms.md": "Fill forms better.\n"})
    catalog._checked.clear()
    found = await catalog.check(skills.origins())
    assert found["pdf"].moved_on(origin) and found["pdf"].commit == new
    upd = await catalog.preview_update(origin)
    assert (
        upd.origin.commit == new and (upd.folder / "forms.md").read_text() == "Fill forms better.\n"
    )
    again = await install(gate, skills, upd)
    assert again.status == "failed" and "already installed" in (again.error or "")
    assert (await install(gate, skills, upd, replace=True)).status == "success"
    assert skills.origins()["pdf"].commit == new
    assert (tmp_path / "skills" / "pdf" / "forms.md").read_text() == "Fill forms better.\n"

    # Removing it forgets where it came from.
    remove = next(o for o in skills.operations() if o.name == "remove")
    prepared = await remove.prepare({"name": "pdf"})
    await gate.apply_internal(
        session_id="ui",
        ability=SOURCE,
        operation="remove",
        target=prepared.target,
        changes=prepared.changes,
        rationale="t",
    )
    assert read_origins(tmp_path / "skills") == {}


async def test_the_model_is_never_offered_install(tmp_path: Path):
    skills = Skills(tmp_path / "skills")
    assert {o.name for o in skills.source().operations} == {"add", "remove"}
    install_op = next(o for o in skills.gate_operations() if o.name == "install")
    with pytest.raises(ValueError, match="desktop"):
        await install_op.prepare({"path": "/tmp/x"})


async def test_choosing_the_skill_in_a_repository(web: FakeWeb, catalog: SkillCatalog):
    pdf_repo(web, **{"skills/other-folder/SKILL.md": "---\nname: Fancy Name\n---\nHi.\n"})
    with pytest.raises(SkillCatalogError, match="has 3 skills; name one") as e:
        await catalog.preview("anthropics/skills")
    assert "docx" in str(e.value)
    # By the folder's name, else by the name its SKILL.md gives itself.
    fancy = await catalog.preview("anthropics/skills", "fancy-name")
    assert fancy.name == "fancy-name" and fancy.origin.skill_path == "skills/other-folder/SKILL.md"
    # The staged copy names its folder, as an installed one does; the preview shows the original.
    assert "name: fancy-name" in (fancy.folder / "SKILL.md").read_text()
    assert "Fancy Name" in fancy.skill_md
    with pytest.raises(SkillCatalogError, match="no skill called nope"):
        await catalog.preview("anthropics/skills", "nope")
    with pytest.raises(SkillCatalogError, match="no SKILL.md at skills"):
        await catalog.preview("https://github.com/anthropics/skills/tree/main/skills")
    with pytest.raises(SkillCatalogError, match="no repository"):
        await catalog.preview("someone/missing")
    one = web.push("solo/skill", {"SKILL.md": "---\nname: solo\n---\nAt the root.\n"})
    root = await catalog.preview("solo/skill")
    assert root.origin.skill_path == "SKILL.md" and root.origin.commit == one


@pytest.mark.parametrize(
    ("extra", "message"),
    [
        ({"path": "skills/pdf/link", "type": "blob", "mode": "120000", "sha": "1" * 40}, "link"),
        (
            {"path": "skills/pdf/vendor", "type": "commit", "mode": "160000", "sha": "1" * 40},
            "another repository",
        ),
        (
            {
                "path": "skills/pdf/../../../etc/x",
                "type": "blob",
                "mode": "100644",
                "sha": "1" * 40,
                "size": 1,
            },
            "won't write",
        ),
        (
            {
                "path": "skills/pdf/big.bin",
                "type": "blob",
                "mode": "100644",
                "sha": "1" * 40,
                "size": MAX_SKILL_BYTES,
            },
            "KB",
        ),
    ],
)
async def test_a_skill_that_isnt_plain_small_files_is_refused_before_anything_is_fetched(
    web: FakeWeb, catalog: SkillCatalog, extra, message
):
    pdf_repo(web)
    web.extra_entries["anthropics/skills"] = [extra]
    with pytest.raises(SkillCatalogError, match=message):
        await catalog.preview("anthropics/skills/pdf")
    assert not any("raw.githubusercontent" in c for c in web.calls)
    assert not any(catalog.staging_dir.glob("*/*"))


async def test_too_many_files_are_refused(web: FakeWeb, catalog: SkillCatalog):
    pdf_repo(web, **{f"skills/pdf/many/{i}.md": "x" for i in range(MAX_FILES + 1)})
    with pytest.raises(SkillCatalogError, match="files; Ghost takes"):
        await catalog.preview("anthropics/skills/pdf")


async def test_a_file_that_doesnt_match_the_commit_is_refused_and_nothing_is_kept(
    web: FakeWeb, catalog: SkillCatalog
):
    pdf_repo(web)
    web.raw_override["skills/pdf/forms.md"] = b"Fill forms LIKE THIS\n"  # same size, other bytes
    with pytest.raises(SkillCatalogError, match="didn't match the commit"):
        await catalog.preview("anthropics/skills/pdf")
    assert not any(catalog.staging_dir.glob("*/*"))


async def test_github_limits_and_failures_are_said_plainly(web: FakeWeb, catalog: SkillCatalog):
    pdf_repo(web)
    web.rate_limited = True
    with pytest.raises(SkillCatalogError, match="limit for requests without signing in"):
        await catalog.preview("anthropics/skills/pdf")
    origin = SkillOrigin("anthropics/skills", "skills/pdf/SKILL.md", "a" * 40, "b" * 40)
    latest = await catalog.latest(origin)
    assert latest.problem and not latest.moved_on(origin)

    async def down(url: str, headers: Mapping[str, str], limit: int) -> Reply:
        raise OSError("no network")

    offline = SkillCatalog(catalog.staging_dir, get=down)
    with pytest.raises(SkillCatalogError, match=r"couldn't reach GitHub \(OSError\)"):
        await offline.preview("anthropics/skills/pdf")
    assert (await offline.search("pdf")).problem == "couldn't reach skills.sh (OSError)"


async def test_previews_expire_and_are_discarded(web: FakeWeb, catalog: SkillCatalog):
    pdf_repo(web)
    p = await catalog.preview("anthropics/skills/pdf")
    assert catalog.staged(p.token) is p
    catalog.preview_ttl = 0
    with pytest.raises(LookupError):
        catalog.staged(p.token)
    assert not p.folder.exists()


async def test_install_wont_hide_the_operators_own_skill(
    web: FakeWeb, catalog: SkillCatalog, tmp_path: Path
):
    pdf_repo(web)
    home = tmp_path / "home" / "pdf"
    home.mkdir(parents=True)
    (home / "SKILL.md").write_text("---\nname: pdf\ndescription: Mine.\n---\nMine.\n")
    skills = Skills(tmp_path / "skills", tmp_path / "home")
    p = await catalog.preview("anthropics/skills/pdf")
    with pytest.raises(ValueError, match="already one of yours"):
        skills.prepare_install(p.folder, p.origin, replace=True)
    out = await install(gated(skills), skills, p, name="pdf-forms")
    assert out.status == "success"
    assert (home / "SKILL.md").read_text().endswith("Mine.\n")  # never written
    assert set(skills.origins()) == {"pdf-forms"}


def test_a_broken_lock_file_is_no_origins(tmp_path: Path):
    (tmp_path / LOCK_FILE).write_text("{not json")
    assert read_origins(tmp_path) == {}
    (tmp_path / LOCK_FILE).write_text(
        json.dumps(
            {
                "skills": {
                    "x": {
                        "source": "a/b",
                        "skillPath": "SKILL.md",
                        "commit": "nope",
                        "skillFolderHash": "1" * 40,
                    }
                }
            }
        )
    )
    assert read_origins(tmp_path) == {}
