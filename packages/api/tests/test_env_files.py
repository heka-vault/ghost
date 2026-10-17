from ghost_api.files import read_env, update_env, write_env


def test_env_round_trip_preserves_comments_and_quotes(tmp_path):
    p = tmp_path / "env"
    p.write_text('# keep me\nA=1\nexport B=\'two words\'\nC="q\\"x"\n')
    assert read_env(p) == {"A": "1", "B": "two words", "C": 'q\\"x'}
    update_env(p, {"A": None, "D": "has space #hash"})
    text = p.read_text()
    assert text.startswith("# keep me\n") and "A=" not in text
    assert read_env(p)["D"] == "has space #hash"
    write_env(p, {"X": ""})
    assert read_env(p) == {"X": ""}
