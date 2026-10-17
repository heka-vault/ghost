"""Ghost control API (ADR 0013): FastAPI on a Unix socket; the contract the apps use."""

from ghost_api.app import Controller, Deps, create_app, outcome_view, pending_view
from ghost_api.client import GhostClient
from ghost_api.models import Phase, Status
from ghost_api.server import ApiServer, default_socket_path
from ghost_api.state import DaemonState

__all__ = [
    "ApiServer",
    "Controller",
    "DaemonState",
    "Deps",
    "GhostClient",
    "Phase",
    "Status",
    "create_app",
    "default_socket_path",
    "outcome_view",
    "pending_view",
]
