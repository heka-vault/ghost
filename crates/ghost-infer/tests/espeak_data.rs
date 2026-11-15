//! #168: espeak-ng with data it cannot use. `espeak_Initialize` ended the whole process
//! (`exit(1)`) when its data was missing or made for another version, so a daemon on a
//! machine without Ubuntu's data folder died and its supervisor started it again forever.
//! Now each is an error that names the library and the folder, the process carries on, and
//! a later call can still succeed. One test in its own binary: the library is global to the
//! process.

use std::path::Path;

use ghost_infer::InferError;
use ghost_infer::espeak::{DataProblem, Espeak, STAMP};

fn kind(e: &InferError) -> Option<DataProblem> {
    match e {
        InferError::EspeakData { kind, .. } => Some(*kind),
        _ => None,
    }
}

fn folder(dir: &Path, files: &[(&str, &[u8])]) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
}

#[test]
fn data_espeak_cannot_use_is_an_error_and_the_process_lives_on() {
    let t = tempfile::tempdir().unwrap();

    // Phoneme data for another version: its first four bytes are the version it was made by.
    let other = t.path().join("other");
    folder(
        &other,
        &[
            ("phontab", b"x"),
            ("phondata", &[0x01, 0x00, 0x00, 0x00, 0, 0, 0, 0]),
            ("phonindex", b"x"),
            ("intonations", b"x"),
        ],
    );
    let e = Espeak::init(Some(&other))
        .err()
        .expect("an error, not exit(1)");
    assert_eq!(kind(&e), Some(DataProblem::VersionMismatch), "{e}");
    let e = e.to_string();
    assert!(e.contains(&other.display().to_string()), "{e}");
    assert!(e.contains("espeak-ng "), "{e}");

    // A folder missing a file espeak reads.
    let partial = t.path().join("partial");
    folder(&partial, &[("phontab", b"x"), ("phondata", b"x")]);
    let e = Espeak::init(Some(&partial)).err().expect("an error");
    assert_eq!(kind(&e), Some(DataProblem::Missing), "{e}");
    let e = e.to_string();
    assert!(e.contains(&partial.display().to_string()), "{e}");
    assert!(e.contains("GHOST_ESPEAK_DATA"), "{e}");

    // A stamp for another release is refused before espeak reads anything.
    let stamped = t.path().join("stamped");
    folder(
        &stamped,
        &[("phontab", b"x"), ("phondata", b"x"), (STAMP, b"0.1\n")],
    );
    let e = Espeak::init(Some(&stamped))
        .err()
        .expect("an error")
        .to_string();
    assert!(e.contains("is for espeak-ng 0.1"), "{e}");

    // And the process is still here, able to start espeak with good data.
    let espeak = Espeak::init(None).expect("the system's data after the failures");
    assert!(
        espeak
            .text_to_phonemes("hello", "en-us")
            .unwrap()
            .contains('h')
    );
}
