"""Load enabled skills by name from config. The agent has no skill-specific code."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from typing import Any

from ghost_skills.base import Skill

SkillFactory = Callable[[Mapping[str, Any]], Skill]


def _github_issues(config: Mapping[str, Any]) -> Skill:
    from ghost_skills.github_issues.skill import GitHubIssuesSkill

    return GitHubIssuesSkill.from_config(config)


_FACTORIES: dict[str, SkillFactory] = {"github_issues": _github_issues}


def load_skills(enabled: list[str], config: Mapping[str, Mapping[str, Any]]) -> list[Skill]:
    skills: list[Skill] = []
    for name in enabled:
        if name not in _FACTORIES:
            raise KeyError(f"unknown skill {name!r}; known: {sorted(_FACTORIES)}")
        skills.append(_FACTORIES[name](config.get(name, {})))
    return skills
