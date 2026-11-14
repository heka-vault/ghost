//! Writes Ghost's own cue sounds to WAV files for the site's hero (docs/site.md): the rising
//! chime a wake plays, the falling one a session's end plays, and one turn of the thinking
//! loop, exactly the samples the daemon plays, at the playback rate. Nothing is played.
//!
//! ```text
//! cargo run -q -p ghost-audio --example site-sounds -- <folder>
//! ```
//!
//! `site/scripts/audio.py` runs it and mixes the files the way the daemon does (the thinking
//! loop under nothing, at `GHOST_THINKING_VOLUME`'s default 0.12).

use std::path::PathBuf;

use ghost_audio::{CHIME_RATE, chime, sounds, thinking_loop};

fn main() -> anyhow::Result<()> {
    let dir: PathBuf = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("usage: site-sounds <folder>"))?;
    sounds::write_wav(&dir.join("chime-up.wav"), CHIME_RATE, &chime(true))?;
    sounds::write_wav(&dir.join("chime-down.wav"), CHIME_RATE, &chime(false))?;
    sounds::write_wav(
        &dir.join("thinking.wav"),
        CHIME_RATE,
        &thinking_loop(CHIME_RATE, sounds::THINKING_SECONDS),
    )?;
    println!("{}", dir.display());
    Ok(())
}
