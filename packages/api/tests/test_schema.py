"""The committed OpenAPI document must match the code. Regenerate with `make api-schema`."""

import json
from pathlib import Path

from ghost_api.export import openapi


def test_committed_openapi_is_current():
    path = Path(__file__).parents[1] / "openapi.json"
    assert path.exists(), "run `make api-schema`"
    assert json.loads(path.read_text()) == openapi(), "openapi.json is stale: run `make api-schema`"


def test_the_contract_says_app_not_the_retired_words():
    """Departments no longer have an mcp.json (ADR 0034), and a proposal's origin is its
    ``source``, not an ability (ADR 0022)."""
    text = json.dumps(openapi())
    assert "mcp.json" not in text
    client = Path(__file__).parents[2] / "client-ts" / "src" / "index.ts"
    assert "mcp.json" not in client.read_text()
