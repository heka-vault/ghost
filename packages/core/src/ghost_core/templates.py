"""Department templates (ADR 0023): a folder that creates a department in one step.

Ghost ships a few under ``infra/templates/<name>/``; the operator's own live under
``~/.config/ghost/templates/<name>/``. A template is never live: applying it copies its
skills into Ghost's skills, adds the toolkits it defines to the policy file (the desktop's
import does this; by voice the toolkits must already be there, since the model never
writes policy), and creates a department line with the template's persona, voice, aliases
and toolkits. Exporting a department writes the same folder back.

Folder::

    accounting/
    ├── department.toml     # title, aliases, voice, toolkits, [toolkit.*] definitions
    ├── persona.md          # who the department is, in the operator's words
    └── skills/<name>/SKILL.md
"""

from __future__ import annotations

import json
import re
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from loguru import logger

from ghost_core.toolkits import ToolkitSpec, parse_toolkits, render_toolkits

DEPARTMENT_FILE = "department.toml"
PERSONA_FILE = "persona.md"
SKILLS_DIR = "skills"
NAME_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
MAX_PERSONA = 4000


@dataclass(frozen=True)
class Template:
    name: str  # the folder
    title: str
    description: str
    aliases: tuple[str, ...]
    voice: str | None
    persona: str
    toolkits: tuple[str, ...]  # the names the department is given
    toolkit_specs: Mapping[str, ToolkitSpec]  # inline definitions, merged into the policy
    skills: tuple[Path, ...]  # skill folders inside the template
    path: Path
    source: str  # "shipped" or "ghost"
    servers: tuple[str, ...] = field(default=())  # servers the toolkits grant, by name

    def needs(self, known_toolkits: Sequence[str], known_servers: Sequence[str]) -> list[str]:
        """What is missing for this template to work as declared."""
        out = []
        for t in self.toolkits:
            if t not in known_toolkits and t not in self.toolkit_specs:
                out.append(f"toolkit {t} is neither in the policy file nor in the template")
        for s in self.servers:
            if s not in known_servers:
                out.append(f"server {s} is not declared in the policy file")
        return out


def load_template(folder: Path, source: str) -> Template | None:
    file = folder / DEPARTMENT_FILE
    if not file.is_file() or folder.is_symlink():
        return None
    try:
        data = tomllib.loads(file.read_text())
    except (OSError, tomllib.TOMLDecodeError) as e:
        logger.warning(f"templates: {folder}: {e}")
        return None
    name = folder.name.lower()
    if not NAME_RE.match(name):
        logger.warning(f"templates: {folder}: name is not a slug; skipped")
        return None
    specs = (
        parse_toolkits(data.get("toolkit", {}), str(file))
        if isinstance(data.get("toolkit"), Mapping)
        else {}
    )
    toolkits = tuple(str(t) for t in data.get("toolkits", []) if isinstance(t, str))
    persona_path = folder / PERSONA_FILE
    persona = persona_path.read_text()[:MAX_PERSONA].strip() if persona_path.is_file() else ""
    skills_dir = folder / SKILLS_DIR
    skills = (
        tuple(sorted(p for p in skills_dir.iterdir() if p.is_dir() and (p / "SKILL.md").is_file()))
        if skills_dir.is_dir()
        else ()
    )
    servers = tuple(sorted({s for t in specs.values() for s in t.grants}))
    return Template(
        name=name,
        title=str(data.get("title") or name.replace("-", " ").title()),
        description=" ".join(str(data.get("description") or "").split()),
        aliases=tuple(str(a) for a in data.get("aliases", []) if isinstance(a, str)),
        voice=str(data["voice"]) if data.get("voice") else None,
        persona=persona,
        toolkits=toolkits or tuple(specs),
        toolkit_specs=specs,
        skills=skills,
        path=folder,
        source=source,
        servers=servers,
    )


def discover(shipped: Path | None, own: Path | None) -> dict[str, Template]:
    """The operator's own templates win over shipped ones with the same name."""
    found: dict[str, Template] = {}
    for where, source in ((own, "ghost"), (shipped, "shipped")):
        if where is None or not where.is_dir():
            continue
        for folder in sorted(p for p in where.iterdir() if p.is_dir()):
            t = load_template(folder, source)
            if t is not None and t.name not in found:
                found[t.name] = t
    return found


def render_department_toml(
    *,
    title: str,
    description: str,
    aliases: Sequence[str],
    voice: str | None,
    toolkits: Sequence[str],
    toolkit_specs: Mapping[str, ToolkitSpec],
) -> str:
    q = json.dumps  # JSON's string escaping is valid TOML
    lines = [f"title = {q(title)}"]
    if description:
        lines.append(f"description = {q(description)}")
    if aliases:
        lines.append("aliases = [" + ", ".join(q(a) for a in aliases) + "]")
    if voice:
        lines.append(f"voice = {q(voice)}")
    lines.append("toolkits = [" + ", ".join(q(t) for t in toolkits) + "]")
    lines.append("")
    lines += render_toolkits(toolkit_specs)
    return "\n".join(lines).rstrip() + "\n"


def export_department(
    into: Path,
    *,
    name: str,
    title: str,
    description: str,
    aliases: Sequence[str],
    voice: str | None,
    persona: str,
    toolkits: Sequence[str],
    toolkit_specs: Mapping[str, ToolkitSpec],
    skill_folders: Sequence[Path],
) -> Path:
    """Write a template folder for a department. Existing content is replaced."""
    import shutil

    if not NAME_RE.match(name):
        raise ValueError(f"{name!r} is not a template name")
    folder = into / name
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    (folder / DEPARTMENT_FILE).write_text(
        render_department_toml(
            title=title,
            description=description,
            aliases=aliases,
            voice=voice,
            toolkits=toolkits,
            toolkit_specs=toolkit_specs,
        )
    )
    (folder / PERSONA_FILE).write_text(persona.strip() + "\n")
    for src in skill_folders:
        if src.is_dir() and (src / "SKILL.md").is_file():
            shutil.copytree(
                src,
                folder / SKILLS_DIR / src.name,
                symlinks=False,
                ignore=lambda d, names: {n for n in names if (Path(d) / n).is_symlink()},
            )
    return folder


class Templates:
    def __init__(self, shipped: Path | None, own: Path | None) -> None:
        self.shipped = shipped
        self.own = own
        self._templates: dict[str, Template] = {}
        self.refresh()

    def refresh(self) -> None:
        self._templates = discover(self.shipped, self.own)

    def list(self) -> list[Template]:
        return list(self._templates.values())

    def get(self, name: str) -> Template | None:
        return self._templates.get(name.strip().lower())

    def describe(self) -> dict[str, Any]:
        return {t.name: t.title for t in self._templates.values()}
