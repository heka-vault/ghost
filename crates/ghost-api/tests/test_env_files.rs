//! The KEY=value file helpers the API reads settings with live in `ghost_core::envfile`
//! (the Python `ghost_api.files` re-exported them): one round trip here, as there.

use std::collections::BTreeMap;

use ghost_core::envfile::{read_env, update_env, write_env};

#[test]
fn env_round_trip_preserves_comments_and_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("env");
    std::fs::write(&p, "# keep me\nA=1\nexport B='two words'\nC=\"q\\\"x\"\n").unwrap();
    let read = read_env(&p).unwrap();
    assert_eq!(read["A"], "1");
    assert_eq!(read["B"], "two words");
    assert_eq!(read["C"], "q\\\"x");
    let mut changes = BTreeMap::new();
    changes.insert("A".to_string(), None);
    changes.insert("D".to_string(), Some("has space #hash".to_string()));
    update_env(&p, &changes, 0o600).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.starts_with("# keep me\n") && !text.contains("A="));
    assert_eq!(read_env(&p).unwrap()["D"], "has space #hash");
    let mut only = BTreeMap::new();
    only.insert("X".to_string(), String::new());
    write_env(&p, &only, 0o600).unwrap();
    assert_eq!(read_env(&p).unwrap(), only);
}
