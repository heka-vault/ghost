"""Ghost ability: GitHub issue triage. Exported for the ``ghost.abilities`` entry point."""

from ghost_github_issues.ability import GitHubIssues, Settings, ability, setup
from ghost_github_issues.models import (
    AddComment,
    IssueSnapshot,
    IssueTarget,
    RepoPolicy,
    SetLabels,
    SetState,
)

__all__ = [
    "AddComment",
    "GitHubIssues",
    "IssueSnapshot",
    "IssueTarget",
    "RepoPolicy",
    "Settings",
    "SetLabels",
    "SetState",
    "ability",
    "setup",
]
