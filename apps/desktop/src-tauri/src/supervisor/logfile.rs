//! Where the daemon's output goes when there is no journal: `~/.local/state/ghost/ghostd.log`,
//! turned over at 10 MB (the last file kept as `ghostd.log.1`).

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

pub const ROTATE_AT: u64 = 10 * 1024 * 1024;

/// `$XDG_STATE_HOME/ghost/ghostd.log`, `~/.local/state/ghost/ghostd.log` without it.
pub fn default_path() -> PathBuf {
    let base = match std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
            home.join(".local").join("state")
        }
    };
    base.join("ghost").join("ghostd.log")
}

/// An append-only log that turns over when it passes `rotate_at` bytes.
pub struct LogFile {
    path: PathBuf,
    file: File,
    written: u64,
    rotate_at: u64,
}

impl LogFile {
    pub fn open(path: &Path, rotate_at: u64) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            written,
            rotate_at,
        })
    }

    /// Write one line (its newline added), turning the file over first when it is full.
    pub fn line(&mut self, line: &[u8]) -> io::Result<()> {
        if self.written >= self.rotate_at {
            self.rotate()?;
        }
        self.file.write_all(line)?;
        self.file.write_all(b"\n")?;
        self.written += line.len() as u64 + 1;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let old = self.path.with_extension("log.1");
        std::fs::rename(&self.path, &old)?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }

    /// Copy every line of `reader` into the log until it ends (the daemon exited).
    pub fn drain(&mut self, reader: impl Read) {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    if self.line(&buf).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_over_at_the_limit_and_keeps_one_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ghostd.log");
        let mut log = LogFile::open(&p, 40).unwrap();
        for i in 0..10 {
            log.line(format!("line {i} of some length").as_bytes())
                .unwrap();
        }
        let old = p.with_extension("log.1");
        assert!(old.exists());
        let kept = std::fs::read_to_string(&p).unwrap();
        assert!(kept.contains("line 9"), "{kept}");
        assert!(std::fs::metadata(&p).unwrap().len() <= 80);
        // Open again: the size carries over, so the next turn-over is on time.
        let log = LogFile::open(&p, 40).unwrap();
        assert_eq!(log.written, std::fs::metadata(&p).unwrap().len());
    }

    #[test]
    fn drains_a_reader_line_by_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ghostd.log");
        let mut log = LogFile::open(&p, ROTATE_AT).unwrap();
        log.drain(&b"one\ntwo\nthree"[..]);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\ntwo\nthree\n");
    }

    #[test]
    fn the_default_path_is_under_the_state_dir() {
        assert!(default_path().ends_with("ghost/ghostd.log"));
    }
}
