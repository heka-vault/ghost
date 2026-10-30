//! Pause the music for a session, resume it after (ADR 0018's "performance is a feature"
//! met the echo canceller on 2026-10-30: with Spotify playing, even ducked, the canceller's
//! far end is never silent, and it suppresses the operator's voice against the music; with
//! the music off, every barge-in landed). Ducking stays (`ghost_audio::Ducker`): it covers
//! what isn't a player.
//!
//! Players are found over MPRIS on the session bus (`org.mpris.MediaPlayer2.*`); the ones
//! that are `Playing` get `Pause`, and only those get `Play` back when the session ends.
//! A player that went away in between is skipped. Nothing here reads what is playing.

use std::sync::Mutex;

use futures::future::BoxFuture;
use tracing::{info, warn};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

/// Pause and resume the players; the daemon's port, a fake in tests.
pub trait PlayersPort: Send + Sync {
    /// Pause every player that is playing; what was paused is remembered for `resume`.
    fn pause_playing(&self) -> BoxFuture<'_, ()>;
    /// Play again what `pause_playing` paused.
    fn resume(&self) -> BoxFuture<'_, ()>;
}

/// The MPRIS players on the session bus.
#[derive(Default)]
pub struct MprisPlayers {
    paused: Mutex<Vec<String>>,
}

/// A player's bus name is a player's: `org.mpris.MediaPlayer2.spotify`, not the bus's own.
pub fn is_player(bus_name: &str) -> bool {
    bus_name.starts_with(MPRIS_PREFIX) && bus_name.len() > MPRIS_PREFIX.len()
}

impl MprisPlayers {
    async fn playing_players() -> Result<Vec<(zbus::Connection, String)>, zbus::Error> {
        let conn = zbus::Connection::session().await?;
        let bus = zbus::fdo::DBusProxy::new(&conn).await?;
        let mut playing = Vec::new();
        for name in bus.list_names().await? {
            let name = name.to_string();
            if !is_player(&name) {
                continue;
            }
            let player = zbus::Proxy::new(&conn, name.clone(), PLAYER_PATH, PLAYER_IFACE).await?;
            let status: String = match player.get_property("PlaybackStatus").await {
                Ok(s) => s,
                Err(e) => {
                    warn!("players: {name}: {e}");
                    continue;
                }
            };
            if status == "Playing" {
                playing.push((conn.clone(), name));
            }
        }
        Ok(playing)
    }

    async fn call(conn: &zbus::Connection, name: &str, method: &str) -> Result<(), zbus::Error> {
        let player = zbus::Proxy::new(conn, name.to_string(), PLAYER_PATH, PLAYER_IFACE).await?;
        player.call_method(method, &()).await?;
        Ok(())
    }
}

impl PlayersPort for MprisPlayers {
    fn pause_playing(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let playing = match Self::playing_players().await {
                Ok(p) => p,
                Err(e) => {
                    warn!("players: {e}");
                    return;
                }
            };
            let mut paused = Vec::new();
            for (conn, name) in playing {
                match Self::call(&conn, &name, "Pause").await {
                    Ok(()) => {
                        info!("paused {}", &name[MPRIS_PREFIX.len()..]);
                        paused.push(name);
                    }
                    Err(e) => warn!("players: pause {name}: {e}"),
                }
            }
            self.paused.lock().unwrap().extend(paused);
        })
    }

    fn resume(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let names: Vec<String> = std::mem::take(&mut *self.paused.lock().unwrap());
            if names.is_empty() {
                return;
            }
            let conn = match zbus::Connection::session().await {
                Ok(c) => c,
                Err(e) => {
                    warn!("players: {e}");
                    return;
                }
            };
            for name in names {
                match Self::call(&conn, &name, "Play").await {
                    Ok(()) => info!("resumed {}", &name[MPRIS_PREFIX.len()..]),
                    Err(e) => warn!("players: resume {name}: {e}"), // gone meanwhile
                }
            }
        })
    }
}

/// No players: tests, and `GHOST_PAUSE_PLAYERS=0`.
pub struct NoPlayers;

impl PlayersPort for NoPlayers {
    fn pause_playing(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn resume(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_players_name_is_under_the_mpris_prefix() {
        assert!(is_player("org.mpris.MediaPlayer2.spotify"));
        assert!(is_player("org.mpris.MediaPlayer2.firefox.instance_12"));
        assert!(!is_player("org.mpris.MediaPlayer2."));
        assert!(!is_player("org.freedesktop.DBus"));
        assert!(!is_player(":1.4"));
    }
}
