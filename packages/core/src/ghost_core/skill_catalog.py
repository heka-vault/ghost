"""The skills catalog (#35): find a skill on skills.sh, fetch it from GitHub, show it to the
operator, and only then install it through the gate (``skills.install``).

Three sources, each read by deterministic, tested code and nothing else:

* **skills.sh search.** The directory behind the ``npx skills`` CLI. Its documented API
  needs a Vercel token Ghost can't have, so Ghost uses the public search the site itself
  uses (``/api/search?q=``): undocumented, so its answer is checked field by field and a
  shape Ghost doesn't know is a problem the Skills page shows, never a guess. Results are
  cached for an hour per query, and the page asks only when the operator types.
* **GitHub.** A skill is a folder with a ``SKILL.md`` in a repository. Ghost resolves the
  branch to a commit, reads the commit's tree, picks the folder, and downloads each file
  at that commit from ``raw.githubusercontent.com``, checking each against the tree's
  blob hash. The folder's git tree hash is what ``.skill-lock.json`` calls
  ``skillFolderHash``: when it changes upstream, the skill has.
* **The staged copy.** Downloads land in a folder of their own under Ghost's cache, files
  only (a link or a submodule refuses the skill), 0644 (nothing is made runnable), within
  size limits, with every path checked to stay inside. The operator reads the SKILL.md
  and the list of scripts there; install copies that very folder.

Nothing here runs anything and nothing here is reachable from a session: no turn queries
the catalog or GitHub, and the search, the look and the update check are shown to the
operator, never to the model. Everything a source sends is untrusted text. Once installed,
a skill is like any other: the model reads its SKILL.md on demand (``read_skill``), as
untrusted text that shapes what it does, never what it may do.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import re
import secrets
import shutil
import time
from collections.abc import Awaitable, Callable, Iterable, Mapping
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import quote, urlparse

from loguru import logger

from ghost_core.scripts_server import INTERPRETERS
from ghost_core.skills import (
    MAX_SKILL_BYTES,
    NAME_RE,
    SKILL_FILE,
    SkillOrigin,
    _name_front_matter,
    parse_front_matter,
)

SEARCH_URL = "https://skills.sh/api/search"
GITHUB_API = "https://api.github.com"
GITHUB_RAW = "https://raw.githubusercontent.com"
_HOSTS = {"skills.sh", "api.github.com", "raw.githubusercontent.com"}  # where a fetch may land
SEARCH_TTL_SECS = 3600.0  # a query's results are kept this long
SEARCH_KEEP = 200  # queries kept
MIN_QUERY = 2  # skills.sh refuses shorter
MAX_RESULTS = 50
PREVIEW_TTL_SECS = 1800.0  # a staged skill waits this long for the operator
PREVIEW_KEEP = 8
CHECK_TTL_SECS = 600.0  # an update check's answer per repository
MAX_FILES = 200
MAX_TREE_BYTES = 12_000_000  # GitHub truncates a tree past ~7 MB anyway
# The operator reads the whole SKILL.md before installing it, so one longer than the preview
# shows is refused, never cut: what is installed is what was read.
MAX_SKILL_MD_BYTES = 60_000
MAX_NAME_LOOKUPS = 30  # SKILL.md files read to match a skill by its front-matter name
MAX_DEPTH = 6  # a SKILL.md deeper than this in a repository is not looked for
FETCH_TIMEOUT_SECS = 15.0
PARALLEL = 8
SKIP_DIRS = frozenset({".git", "__pycache__", "node_modules"})

_OWNER = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$")
_REPO = re.compile(r"^[A-Za-z0-9._-]{1,100}$")
_REF = re.compile(r"^[A-Za-z0-9._/-]{1,100}$")
_SLUG = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_SHA = re.compile(r"^[0-9a-f]{40}$")
_CONTROL = re.compile(r"[\x00-\x1f\x7f]")


class SkillCatalogError(ValueError):
    """Something the operator can read: what went wrong and, when it helps, what to do."""


@dataclass(frozen=True)
class Reply:
    status: int
    body: bytes
    headers: Mapping[str, str] = field(default_factory=dict)  # lower-case names


Get = Callable[[str, Mapping[str, str], int], Awaitable[Reply]]  # url, headers, byte limit


class TooBig(Exception):
    pass


def _too_long(size: int) -> str:
    return (
        f"its SKILL.md is {size // 1000:,} KB; Ghost shows you the whole file before "
        f"installing, up to {MAX_SKILL_MD_BYTES // 1000:,} KB"
    )


# --- skills.sh ------------------------------------------------------------------------


@dataclass(frozen=True)
class Listing:
    """One search result: what skills.sh says, checked. ``source`` is a GitHub owner/repo."""

    id: str  # "anthropics/skills/pdf"
    source: str  # "anthropics/skills"
    skill: str  # "pdf", the directory's id for it
    name: str
    installs: int


@dataclass(frozen=True)
class SearchResult:
    query: str
    skills: list[Listing]
    problem: str | None = None  # why the results are old or missing
    fetched_at: float | None = None


def _text(v: object, limit: int = 100) -> str:
    return _CONTROL.sub("", v).strip()[:limit] if isinstance(v, str) else ""


def is_repo(source: str) -> bool:
    owner, _, repo = source.partition("/")
    return bool(_OWNER.match(owner) and _REPO.match(repo) and repo not in (".", ".."))


def parse_search(raw: object) -> list[Listing]:
    """The search answer as listings. Raises ``SkillCatalogError`` when it isn't the shape
    Ghost knows; drops a result that isn't a skill in a GitHub repository (skills.sh also
    lists skills a website serves, which Ghost does not install)."""
    if not isinstance(raw, Mapping) or not isinstance(raw.get("skills"), list):
        raise SkillCatalogError(
            "skills.sh answered in a shape Ghost doesn't know; its search may have changed"
        )
    out: list[Listing] = []
    seen: set[str] = set()
    for r in raw["skills"]:
        if not isinstance(r, Mapping):
            continue
        source, skill = _text(r.get("source")), _text(r.get("skillId"), 128)
        if not is_repo(source) or not _SLUG.match(skill):
            continue
        key = f"{source}/{skill}"
        if key in seen:
            continue
        seen.add(key)
        installs = r.get("installs")
        out.append(
            Listing(
                id=key,
                source=source,
                skill=skill,
                name=_text(r.get("name")) or skill,
                installs=installs if isinstance(installs, int) and installs > 0 else 0,
            )
        )
        if len(out) >= MAX_RESULTS:
            break
    return out


# --- naming a skill on GitHub ----------------------------------------------------------


@dataclass(frozen=True)
class SourceRef:
    """What the operator named: a repository, and maybe the branch, the folder or the
    skill's id in it."""

    owner: str
    repo: str
    ref: str | None = None
    path: str | None = None  # the folder, "" for the repository's root
    skill: str | None = None

    @property
    def source(self) -> str:
        return f"{self.owner}/{self.repo}"


def _clean_path(path: str) -> str:
    parts = [p for p in path.strip("/").split("/") if p]
    if parts and parts[-1] == SKILL_FILE:
        parts = parts[:-1]
    for p in parts:
        if p in (".", "..") or "\\" in p or _CONTROL.search(p):
            raise SkillCatalogError(f"{path!r} is not a folder in a repository")
    return "/".join(parts)


def parse_source(text: str, skill: str | None = None) -> SourceRef:
    """``owner/repo``, ``owner/repo/skill`` (a skills.sh id), or a GitHub address: the
    repository, or a folder in it (``…/tree/main/skills/pdf``), or its SKILL.md."""
    raw = text.strip()
    if not raw:
        raise SkillCatalogError("name a repository, like anthropics/skills")
    ref = path = None
    if "://" in raw or raw.startswith("github.com/"):
        url = urlparse(raw if "://" in raw else f"https://{raw}")
        if url.scheme != "https" or url.hostname not in ("github.com", "www.github.com"):
            raise SkillCatalogError("Ghost installs skills from GitHub only: a github.com address")
        parts = [p for p in url.path.split("/") if p]
        if len(parts) < 2:
            raise SkillCatalogError(f"{raw} doesn't name a repository")
        owner, repo = parts[0], parts[1].removesuffix(".git")
        if len(parts) >= 4 and parts[2] in ("tree", "blob"):
            ref, path = parts[3], _clean_path("/".join(parts[4:]))
        elif len(parts) > 2:
            raise SkillCatalogError(f"{raw} isn't a repository or a folder in one")
    else:
        parts = raw.strip("/").split("/")
        if len(parts) == 3 and not skill:
            owner, repo, skill = parts
        elif len(parts) == 2:
            owner, repo = parts
        else:
            raise SkillCatalogError(
                f"{raw!r} isn't owner/repo, owner/repo/skill or a github.com address"
            )
    if not is_repo(f"{owner}/{repo}"):
        raise SkillCatalogError(f"{owner}/{repo} isn't a GitHub repository name")
    if ref is not None and (not _REF.match(ref) or ".." in ref):
        raise SkillCatalogError(f"{ref!r} isn't a branch, tag or commit")
    if skill is not None and not _SLUG.match(skill):
        raise SkillCatalogError(f"{skill!r} isn't a skill's id")
    return SourceRef(owner, repo, ref, path, skill or None)


# --- the tree --------------------------------------------------------------------------


@dataclass(frozen=True)
class Entry:
    path: str
    type: str  # blob, tree, commit (a submodule)
    mode: str
    sha: str
    size: int = 0


def parse_tree(raw: object) -> list[Entry]:
    if not isinstance(raw, Mapping) or not isinstance(raw.get("tree"), list):
        raise SkillCatalogError("GitHub answered in a shape Ghost doesn't know")
    if raw.get("truncated"):
        raise SkillCatalogError("that repository is too big for Ghost to read its list of files")
    out = []
    for e in raw["tree"]:
        if not isinstance(e, Mapping):
            continue
        path, kind, mode, sha = (_text(e.get(k), 4096) for k in ("path", "type", "mode", "sha"))
        if not path or kind not in ("blob", "tree", "commit") or not _SHA.match(sha):
            continue
        size = e.get("size")
        out.append(Entry(path, kind, mode, sha, size if isinstance(size, int) else 0))
    return out


def skill_folders(entries: Iterable[Entry]) -> list[str]:
    """The folders in a tree that hold a SKILL.md, shallowest first ("" is the root)."""
    found = []
    for e in entries:
        if e.type != "blob" or PurePosixPath(e.path).name != SKILL_FILE:
            continue
        parts = e.path.split("/")[:-1]
        if len(parts) > MAX_DEPTH or any(p in SKIP_DIRS for p in parts):
            continue
        found.append("/".join(parts))
    return sorted(found, key=lambda f: (f.count("/") + bool(f), f))


def _folders_said(folders: list[str], most: int = 12) -> str:
    """Each folder by its path in the repository, never by its last part alone: two skills
    can share that (``skills/here-now`` and ``.claude/skills/here-now``)."""
    said = [f or "the top folder" for f in folders[:most]]
    more = len(folders) - most
    return ", ".join(said) + (f" and {more} more" if more > 0 else "")


def _folder_link(src: SourceRef, folder: str) -> str:
    """The github.com address of a folder at the branch the operator named (the default
    one otherwise), which names exactly one skill."""
    return f"https://github.com/{src.source}/tree/{src.ref or 'HEAD'}/{folder}".rstrip("/")


def _git_mode(entry: Entry) -> str:
    return entry.mode.lstrip("0") or {"tree": "40000", "commit": "160000"}.get(entry.type, "100644")


def folder_hash(entries: Iterable[Entry], folder: str) -> str:
    """The folder's git tree hash at the commit: the tree's own entry for a folder, worked
    out from its children for the repository's root (git's tree object, sorted as git
    sorts it, a folder's name with a ``/`` after it)."""
    entries = list(entries)
    for e in entries:
        if folder and e.type == "tree" and e.path == folder:
            return e.sha
    if folder:
        raise SkillCatalogError(f"there is no folder {folder} at that commit")
    children = [e for e in entries if "/" not in e.path]
    children.sort(key=lambda e: e.path.encode() + (b"/" if e.type == "tree" else b""))
    body = b"".join(
        f"{_git_mode(e)} {e.path}".encode() + b"\0" + bytes.fromhex(e.sha) for e in children
    )
    return hashlib.sha1(b"tree %d\0" % len(body) + body).hexdigest()  # noqa: S324 - git's hash


def blob_hash(data: bytes) -> str:
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()  # noqa: S324 - git's hash


def slug(name: str) -> str:
    s = re.sub(r"[\s_]+", "-", name.strip().lower())
    s = re.sub(r"[^a-z0-9.-]", "", s)
    return re.sub(r"-{2,}", "-", s).lstrip(".-")[:64].rstrip(".-")


def is_script(path: str, mode: str = "100644") -> bool:
    """What ``ghost-scripts`` would offer to run, or what was executable upstream (installed
    without its executable bit, so only an interpreter could run it)."""
    return PurePosixPath(path).suffix in INTERPRETERS or mode == "100755"


def _safe_parts(rel: str) -> list[str]:
    parts = rel.split("/")
    if (
        not rel
        or rel.startswith("/")
        or len(parts) > 8
        or any(
            p in ("", ".", "..") or "\\" in p or len(p) > 255 or _CONTROL.search(p) for p in parts
        )
    ):
        raise SkillCatalogError(f"the skill has a file Ghost won't write: {rel[:80]!r}")
    return parts


# --- staged previews -------------------------------------------------------------------


@dataclass(frozen=True)
class StagedFile:
    path: str  # relative to the skill's folder
    bytes: int
    script: bool


@dataclass(frozen=True)
class Preview:
    """A skill fetched and staged, waiting for the operator to read it and install it."""

    token: str
    name: str
    description: str
    folder: Path  # the staged copy
    origin: SkillOrigin  # installed_at is set at install
    skill_md: str  # the whole file, front matter included: untrusted text
    files: tuple[StagedFile, ...]
    created: float

    @property
    def scripts(self) -> list[str]:
        return [f.path for f in self.files if f.script]


@dataclass(frozen=True)
class Latest:
    """An installed skill's source now."""

    commit: str | None = None
    folder_hash: str | None = None  # None: the folder is gone from the source
    problem: str | None = None

    def moved_on(self, origin: SkillOrigin) -> bool:
        return self.folder_hash is not None and self.folder_hash != origin.folder_hash


# --- the catalog -----------------------------------------------------------------------


class SkillCatalog:
    """Search, preview, and the update check. Holds no state that matters across a
    restart: a search cache, staged previews (cleared at start), recent update checks."""

    def __init__(
        self,
        staging_dir: Path,
        *,
        get: Get | None = None,
        search_url: str = SEARCH_URL,
        github_api: str = GITHUB_API,
        github_raw: str = GITHUB_RAW,
        search_ttl: float = SEARCH_TTL_SECS,
        preview_ttl: float = PREVIEW_TTL_SECS,
    ) -> None:
        self.staging_dir = Path(staging_dir).expanduser()
        self._get = get or self._http_get
        self.search_url = search_url
        self.github_api = github_api.rstrip("/")
        self.github_raw = github_raw.rstrip("/")
        self.search_ttl = search_ttl
        self.preview_ttl = preview_ttl
        self._searches: dict[str, tuple[float, list[Listing]]] = {}
        self._previews: dict[str, Preview] = {}
        self._trees: dict[tuple[str, str], list[Entry]] = {}
        self._checked: dict[tuple[str, str | None], tuple[float, str]] = {}
        self._client: Any = None
        shutil.rmtree(self.staging_dir, ignore_errors=True)  # what an earlier run left

    # --- fetching ----------------------------------------------------------------------

    async def _http_get(self, url: str, headers: Mapping[str, str], limit: int) -> Reply:
        import httpx

        if self._client is None:
            self._client = httpx.AsyncClient(timeout=FETCH_TIMEOUT_SECS, follow_redirects=True)
        async with self._client.stream("GET", url, headers=dict(headers)) as r:
            if r.url.host not in _HOSTS:
                raise SkillCatalogError(f"{urlparse(url).hostname} sent Ghost somewhere else")
            body = bytearray()
            async for chunk in r.aiter_bytes():
                body += chunk
                if len(body) > limit:
                    raise TooBig
            return Reply(r.status_code, bytes(body), {k.lower(): v for k, v in r.headers.items()})

    async def _fetch(self, url: str, *, who: str, limit: int, accept: str) -> Reply:
        headers = {"Accept": accept, "User-Agent": "ghost-skills"}
        try:
            return await self._get(url, headers, limit)
        except SkillCatalogError:
            raise
        except TooBig:
            raise SkillCatalogError(f"{who} sent more than Ghost reads for a skill") from None
        except Exception as e:  # noqa: BLE001 - the network's failures, said plainly
            raise SkillCatalogError(f"couldn't reach {who} ({type(e).__name__})") from None

    def _github_problem(self, r: Reply, what: str) -> SkillCatalogError:
        if r.status in (403, 429) and r.headers.get("x-ratelimit-remaining") == "0":
            until = r.headers.get("x-ratelimit-reset", "")
            when = (
                datetime.fromtimestamp(int(until)).strftime("%H:%M")
                if until.isdigit()
                else "in an hour"
            )
            return SkillCatalogError(
                f"GitHub's limit for requests without signing in is used up until {when}"
            )
        if r.status == 404:
            return SkillCatalogError(f"GitHub has no {what} (or it is private)")
        return SkillCatalogError(f"GitHub answered {r.status} for {what}")

    async def _commit(self, src: SourceRef) -> str:
        ref = src.ref or "HEAD"
        url = f"{self.github_api}/repos/{src.source}/commits/{quote(ref, safe='')}"
        r = await self._fetch(url, who="GitHub", limit=4096, accept="application/vnd.github.sha")
        if r.status != 200:
            what = f"branch or tag {ref} in {src.source}" if src.ref else f"repository {src.source}"
            raise self._github_problem(r, what)
        sha = r.body.decode(errors="replace").strip()
        if not _SHA.match(sha):
            raise SkillCatalogError("GitHub answered in a shape Ghost doesn't know")
        return sha

    async def _tree(self, source: str, commit: str) -> list[Entry]:
        hit = self._trees.get((source, commit))
        if hit is not None:
            return hit
        url = f"{self.github_api}/repos/{source}/git/trees/{commit}?recursive=1"
        r = await self._fetch(
            url, who="GitHub", limit=MAX_TREE_BYTES, accept="application/vnd.github+json"
        )
        if r.status != 200:
            raise self._github_problem(r, f"commit {commit[:7]} in {source}")
        try:
            entries = parse_tree(json.loads(r.body))
        except ValueError as e:
            raise SkillCatalogError(str(e)) from None
        if len(self._trees) >= 16:
            self._trees.pop(next(iter(self._trees)))
        self._trees[(source, commit)] = entries
        return entries

    async def _raw(self, source: str, commit: str, path: str, size: int) -> bytes:
        url = f"{self.github_raw}/{source}/{commit}/{quote(path, safe='/')}"
        r = await self._fetch(url, who="GitHub", limit=max(size, 0), accept="*/*")
        if r.status != 200:
            raise self._github_problem(r, f"file {path}")
        return r.body

    # --- search --------------------------------------------------------------------------

    async def search(self, query: str) -> SearchResult:
        """skills.sh's results for ``query``, from the cache when they are recent. When the
        site fails, the last results for the query (if any) with the problem."""
        q = " ".join(query.split())[:100]
        if len(q) < MIN_QUERY:
            return SearchResult(query=q, skills=[])
        key = q.lower()
        hit = self._searches.get(key)
        if hit is not None and time.time() - hit[0] < self.search_ttl:
            return SearchResult(query=q, skills=hit[1], fetched_at=hit[0])
        url = f"{self.search_url}?q={quote(q)}&limit={MAX_RESULTS}"
        try:
            r = await self._fetch(url, who="skills.sh", limit=2_000_000, accept="application/json")
            if r.status != 200:
                raise SkillCatalogError(f"skills.sh answered {r.status}")
            try:
                data = json.loads(r.body)
            except ValueError:
                raise SkillCatalogError(
                    "skills.sh answered in a shape Ghost doesn't know; its search may have changed"
                ) from None
            listings = parse_search(data)
        except SkillCatalogError as e:
            logger.warning(f"skill catalog: search failed: {e}")
            if hit is not None:
                return SearchResult(query=q, skills=hit[1], problem=str(e), fetched_at=hit[0])
            return SearchResult(query=q, skills=[], problem=str(e))
        now = time.time()
        self._searches.pop(key, None)
        self._searches[key] = (now, listings)
        while len(self._searches) > SEARCH_KEEP:
            self._searches.pop(next(iter(self._searches)))
        return SearchResult(query=q, skills=listings, fetched_at=now)

    # --- preview ---------------------------------------------------------------------------

    async def preview(self, source: str, skill: str | None = None) -> Preview:
        """Fetch a skill from GitHub, pinned to the commit its branch is on now, into a
        staged folder the operator reads before installing."""
        return await self._preview(parse_source(source, skill))

    async def preview_update(self, origin: SkillOrigin) -> Preview:
        """The source's newer version of an installed skill, staged the same way."""
        owner, _, repo = origin.source.partition("/")
        return await self._preview(SourceRef(owner, repo, origin.ref, origin.folder))

    async def _preview(self, src: SourceRef) -> Preview:
        self._expire()
        commit = await self._commit(src)
        entries = await self._tree(src.source, commit)
        folder = await self._choose(src, commit, entries)
        prefix = f"{folder}/" if folder else ""
        wanted: list[tuple[str, Entry]] = []
        total = 0
        for e in entries:
            if not e.path.startswith(prefix) or e.type == "tree":
                continue
            rel = e.path[len(prefix) :]
            if e.type == "commit":
                raise SkillCatalogError(f"the skill holds another repository ({rel}); Ghost won't")
            if e.mode == "120000":
                raise SkillCatalogError(f"the skill holds a link ({rel}); a skill is plain files")
            parts = _safe_parts(rel)
            if any(p in SKIP_DIRS for p in parts[:-1]):
                continue
            total += e.size
            wanted.append((rel, e))
        if len(wanted) > MAX_FILES:
            raise SkillCatalogError(f"the skill has {len(wanted)} files; Ghost takes {MAX_FILES}")
        md = next((e for rel, e in wanted if rel == SKILL_FILE), None)
        if md is not None and md.size > MAX_SKILL_MD_BYTES:
            raise SkillCatalogError(_too_long(md.size))
        if total > MAX_SKILL_BYTES:
            raise SkillCatalogError(
                f"the skill is {total // 1000:,} KB; Ghost takes {MAX_SKILL_BYTES // 1000:,} KB"
            )
        token = secrets.token_urlsafe(12)
        base = self.staging_dir / token
        try:
            return await self._stage(src, commit, folder, entries, wanted, token, base)
        except BaseException:
            shutil.rmtree(base, ignore_errors=True)
            raise

    async def _stage(
        self,
        src: SourceRef,
        commit: str,
        folder: str,
        entries: list[Entry],
        wanted: list[tuple[str, Entry]],
        token: str,
        base: Path,
    ) -> Preview:
        limit = asyncio.Semaphore(PARALLEL)

        async def one(e: Entry) -> bytes:
            async with limit:
                data = await self._raw(src.source, commit, e.path, e.size)
            if blob_hash(data) != e.sha:
                raise SkillCatalogError(f"{e.path} didn't match the commit; nothing was kept")
            return data

        blobs = await asyncio.gather(*(one(e) for _, e in wanted))
        skill_md = dict(zip((rel for rel, _ in wanted), blobs, strict=True)).get(SKILL_FILE)
        if skill_md is None:
            raise SkillCatalogError(f"there is no {SKILL_FILE} in {folder or 'the repository'}")
        if len(skill_md) > MAX_SKILL_MD_BYTES:  # the tree said less; the file decides
            raise SkillCatalogError(_too_long(len(skill_md)))
        text = skill_md.decode(errors="replace")
        try:
            meta, _ = parse_front_matter(text)
        except ValueError as e:
            raise SkillCatalogError(f"its SKILL.md {e}") from None
        tail = folder.rsplit("/", 1)[-1] if folder else src.repo
        names = (slug(str(meta.get("name") or "")), slug(tail), slug(src.skill or ""))
        name = next((n for n in names if NAME_RE.match(n)), None)
        if name is None:
            raise SkillCatalogError("the skill has no name Ghost can use")
        stage = base / name
        stage.mkdir(parents=True)
        root = stage.resolve()
        files: list[StagedFile] = []
        for (rel, e), data in zip(wanted, blobs, strict=True):
            dest = stage.joinpath(*rel.split("/"))
            dest.parent.mkdir(parents=True, exist_ok=True)
            if root not in dest.resolve().parents:
                raise SkillCatalogError(f"the skill has a file Ghost won't write: {rel[:80]!r}")
            fd = os.open(dest, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644)
            with os.fdopen(fd, "wb") as f:
                f.write(data)
            if rel != SKILL_FILE:
                files.append(StagedFile(rel, len(data), is_script(rel, e.mode)))
        # The staged copy's front matter names its folder ("PDF tools" is "pdf-tools"), as
        # an installed copy's does; the operator reads the file as the source wrote it.
        _name_front_matter(stage / SKILL_FILE, name)
        preview = Preview(
            token=token,
            name=name,
            description=" ".join(str(meta.get("description") or "").split())[:1000],
            folder=stage,
            origin=SkillOrigin(
                source=src.source,
                skill_path=f"{folder}/{SKILL_FILE}" if folder else SKILL_FILE,
                commit=commit,
                folder_hash=folder_hash(entries, folder),
                ref=src.ref,
            ),
            skill_md=text,  # whole: a longer one was refused above
            files=tuple(sorted(files, key=lambda f: f.path)),
            created=time.time(),
        )
        self._previews[token] = preview
        while len(self._previews) > PREVIEW_KEEP:
            self.discard(next(iter(self._previews)))
        return preview

    async def _choose(self, src: SourceRef, commit: str, entries: list[Entry]) -> str:
        """The folder the operator means: the one named, else the skill whose folder (or,
        failing that, whose front-matter name) is the id, else the repository's only one."""
        folders = skill_folders(entries)
        if src.path is not None:
            if src.path not in folders:
                where = src.path or "the top"
                raise SkillCatalogError(f"{src.source} has no {SKILL_FILE} at {where}")
            return src.path
        if not folders:
            raise SkillCatalogError(f"{src.source} has no skills (no {SKILL_FILE} in it)")
        if src.skill:
            want = slug(src.skill)
            by_folder = [f for f in folders if f and slug(f.rsplit("/", 1)[-1]) == want]
            if not by_folder:
                by_folder = [f async for f in self._named(src, commit, entries, folders, want)]
            if len(by_folder) == 1:
                return by_folder[0]
            if by_folder:
                # Never pick one of two skills with the same name: ask for its folder.
                raise SkillCatalogError(
                    f"{src.source} has {len(by_folder)} skills called {src.skill}, in "
                    f"{_folders_said(by_folder)}; give the address of the one you mean, like "
                    f"{_folder_link(src, by_folder[0])}"
                )
            raise SkillCatalogError(f"{src.source} has no skill called {src.skill}")
        if "" in folders or len(folders) == 1:
            return folders[0]
        names = [f.rsplit("/", 1)[-1] for f in folders]
        first = folders[0]
        like = (
            f"{src.source}/{names[0]}"
            if names.count(names[0]) == 1
            else _folder_link(src, first)  # its name is two skills' here: only its folder says
        )
        raise SkillCatalogError(
            f"{src.source} has {len(folders)} skills; name one, like {like}. "
            f"Its skills' folders: {_folders_said(folders)}"
        )

    async def _named(
        self, src: SourceRef, commit: str, entries: list[Entry], folders: list[str], want: str
    ):
        """The folders whose SKILL.md gives itself the name ``want``, from the first
        ``MAX_NAME_LOOKUPS``."""
        for f in folders[:MAX_NAME_LOOKUPS]:
            e = next(x for x in entries if x.path == (f"{f}/{SKILL_FILE}" if f else SKILL_FILE))
            if e.size > 200_000:
                continue
            try:
                meta, _ = parse_front_matter(
                    (await self._raw(src.source, commit, e.path, e.size)).decode(errors="replace")
                )
            except (ValueError, SkillCatalogError):
                continue
            if slug(str(meta.get("name") or "")) == want:
                yield f

    def staged(self, token: str) -> Preview:
        self._expire()
        p = self._previews.get(token)
        if p is None or not p.folder.is_dir():
            raise LookupError("that preview is gone; look at the skill again")
        return p

    def discard(self, token: str) -> None:
        p = self._previews.pop(token, None)
        if p is not None:
            shutil.rmtree(p.folder.parent, ignore_errors=True)

    def _expire(self) -> None:
        now = time.time()
        for token in [t for t, p in self._previews.items() if now - p.created > self.preview_ttl]:
            self.discard(token)

    # --- updates -------------------------------------------------------------------------

    async def latest(self, origin: SkillOrigin) -> Latest:
        """Where the skill's source is now. Asked only when the Skills page asks."""
        try:
            key = (origin.source, origin.ref)
            hit = self._checked.get(key)
            if hit is not None and time.time() - hit[0] < CHECK_TTL_SECS:
                commit = hit[1]
            else:
                owner, _, repo = origin.source.partition("/")
                commit = await self._commit(SourceRef(owner, repo, origin.ref))
                self._checked[key] = (time.time(), commit)
            if commit == origin.commit:
                return Latest(commit, origin.folder_hash)
            entries = await self._tree(origin.source, commit)
            if origin.folder not in skill_folders(entries):
                return Latest(commit, None, f"{origin.source} no longer has this skill")
            return Latest(commit, folder_hash(entries, origin.folder))
        except SkillCatalogError as e:
            return Latest(problem=str(e))

    async def check(self, origins: Mapping[str, SkillOrigin]) -> dict[str, Latest]:
        """``latest`` for each installed skill, one repository at a time (they share a
        commit lookup, and GitHub's limit without signing in is 60 an hour)."""
        out: dict[str, Latest] = {}
        for name, origin in sorted(origins.items(), key=lambda kv: (kv[1].source, kv[0])):
            out[name] = await self.latest(origin)
        return out
