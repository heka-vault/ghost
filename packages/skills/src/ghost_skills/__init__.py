"""Skills: pluggable capabilities behind one interface (ADR 0006)."""

from ghost_skills.base import PrepareRequest, ReadTool, Skill, ToolResult
from ghost_skills.registry import load_skills

__all__ = ["PrepareRequest", "ReadTool", "Skill", "ToolResult", "load_skills"]
