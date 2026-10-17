"""The committed OpenAPI document must match the code. Regenerate with `make api-schema`."""

import json
from pathlib import Path

from ghost_api.export import openapi


def test_committed_openapi_is_current():
    path = Path(__file__).parents[1] / "openapi.json"
    assert path.exists(), "run `make api-schema`"
    assert json.loads(path.read_text()) == openapi(), "openapi.json is stale: run `make api-schema`"
