//! Which monitor the floating room comes forward on, as a pure function of what is known
//! (ADR 0059). The operator's words: "I don't want it to jump onto the monitor that has the
//! mouse pointer because that's the one I'm most likely interacting with; I would like it to
//! appear on the second monitor."
//!
//! "Where I am" is the focused window's monitor when that is known, else the pointer's. Under
//! XWayland neither is known from the X11 layer while a native Wayland window has the focus
//! (the pointer query goes stale and `_NET_ACTIVE_WINDOW` is mutter's no-focus window), so
//! they come from the optional GNOME Shell helper there, and are `None` without it.

use serde::{Deserialize, Serialize};

/// The operator's choice (Settings › General, The floating room: "Ghost appears on").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MonitorPref {
    /// Not the monitor the operator is working on (the default).
    #[default]
    Other,
    /// Always this monitor, by its output name (`HDMI-3`).
    Named(String),
    /// Where the operator is working.
    Here,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
    fn centre(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
    /// How much of `other` lies on this rectangle.
    fn overlap(&self, other: &Rect) -> i64 {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if w <= 0 || h <= 0 {
            0
        } else {
            i64::from(w) * i64::from(h)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    /// The output's name (`HDMI-1`), the same in X11 (RandR) and in GNOME's settings.
    pub name: String,
    pub geometry: Rect,
    pub primary: bool,
}

/// What the desktop told us at the moment of the wake. Only what is reliable goes in: a
/// stale pointer is `None`, not a guess.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub monitors: Vec<Monitor>,
    pub pointer: Option<(i32, i32)>,
    /// The focused window's frame, when it is a real client this layer can see.
    pub focus: Option<Rect>,
    /// The focused window fills its monitor (a game, a video, a presentation).
    pub focus_fullscreen: bool,
}

/// Where Ghost's stage sits on the chosen monitor: centred, or on the half away from the
/// operator's work when there is only the one monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Centre,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// The monitor the operator named.
    Named,
    /// The named monitor isn't connected: the rule for "the other monitor" chose.
    NamedMissing,
    /// Away from where the operator is working.
    Other,
    /// Where the operator is working.
    Here,
    /// Where the operator is working isn't known: the monitor that isn't the primary one.
    NotPrimary,
    /// Only one monitor.
    Only,
    /// The operator's choice had a full-screen app in focus: another monitor instead.
    AwayFromFullScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Index into `Known::monitors`.
    pub monitor: usize,
    pub side: Side,
    pub why: Why,
    /// A focused full-screen app covers the chosen monitor and there is no other: Ghost stays
    /// out of the way (the voice goes on, nothing comes forward).
    pub covered: bool,
}

/// The monitor holding the most of `r`.
fn monitor_of_rect(monitors: &[Monitor], r: &Rect) -> Option<usize> {
    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| (i, m.geometry.overlap(r)))
        .filter(|(_, a)| *a > 0)
        .max_by_key(|(_, a)| *a)
        .map(|(i, _)| i)
}

fn monitor_of_point(monitors: &[Monitor], (x, y): (i32, i32)) -> Option<usize> {
    monitors.iter().position(|m| m.geometry.contains(x, y))
}

/// The monitor the operator is working on, when it is known: the focused window's, else the
/// pointer's.
pub fn here(k: &Known) -> Option<usize> {
    k.focus
        .as_ref()
        .and_then(|f| monitor_of_rect(&k.monitors, f))
        .or_else(|| k.pointer.and_then(|p| monitor_of_point(&k.monitors, p)))
}

pub fn choose(pref: &MonitorPref, k: &Known) -> Option<Choice> {
    let n = k.monitors.len();
    if n == 0 {
        return None;
    }
    let here = here(k);
    // A focused full-screen window covers the monitor it is on (mutter stacks it above
    // windows that keep themselves above).
    let covered_at = if k.focus_fullscreen { here } else { None };
    if n == 1 {
        // One monitor: away from the pointer or the focused window, on the other half.
        let m = &k.monitors[0].geometry;
        let at = k.focus.map(|f| f.centre()).or(k.pointer);
        let side = match at {
            Some((x, _)) if x < m.x + m.width / 2 => Side::Right,
            Some(_) => Side::Left,
            None => Side::Centre,
        };
        return Some(Choice {
            monitor: 0,
            side,
            why: Why::Only,
            covered: covered_at == Some(0),
        });
    }
    let not_here = |here: Option<usize>| -> (usize, Why) {
        match here {
            Some(h) => ((0..n).find(|&i| i != h).unwrap_or(0), Why::Other),
            None => (
                k.monitors.iter().position(|m| !m.primary).unwrap_or(n - 1),
                Why::NotPrimary,
            ),
        }
    };
    let (monitor, why) = match pref {
        MonitorPref::Named(name) => match k.monitors.iter().position(|m| &m.name == name) {
            Some(i) => (i, Why::Named),
            None => {
                let (i, _) = not_here(here);
                (i, Why::NamedMissing)
            }
        },
        MonitorPref::Here => match here {
            Some(h) => (h, Why::Here),
            None => not_here(None),
        },
        MonitorPref::Other => not_here(here),
    };
    if covered_at == Some(monitor) {
        let other = (0..n).find(|&i| Some(i) != covered_at).unwrap_or(monitor);
        return Some(Choice {
            monitor: other,
            side: Side::Centre,
            why: Why::AwayFromFullScreen,
            covered: other == monitor,
        });
    }
    Some(Choice {
        monitor,
        side: Side::Centre,
        why,
        covered: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Vec<Monitor> {
        vec![
            Monitor {
                name: "HDMI-1".into(),
                geometry: Rect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                primary: true,
            },
            Monitor {
                name: "HDMI-3".into(),
                geometry: Rect {
                    x: 1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                primary: false,
            },
        ]
    }

    fn at(monitors: Vec<Monitor>) -> Known {
        Known {
            monitors,
            ..Known::default()
        }
    }

    #[test]
    fn the_other_monitor_is_away_from_the_focused_window_before_the_pointer() {
        let mut k = at(two());
        k.pointer = Some((100, 100)); // on HDMI-1
        k.focus = Some(Rect {
            x: 2000,
            y: 50,
            width: 800,
            height: 600,
        }); // on HDMI-3
        let c = choose(&MonitorPref::Other, &k).unwrap();
        assert_eq!((c.monitor, c.why), (0, Why::Other));
    }

    #[test]
    fn the_other_monitor_is_away_from_the_pointer_when_the_focus_is_unknown() {
        let mut k = at(two());
        k.pointer = Some((2500, 400));
        assert_eq!(choose(&MonitorPref::Other, &k).unwrap().monitor, 0);
        k.pointer = Some((10, 10));
        assert_eq!(choose(&MonitorPref::Other, &k).unwrap().monitor, 1);
    }

    #[test]
    fn with_nothing_known_it_is_the_monitor_that_is_not_primary() {
        let c = choose(&MonitorPref::Other, &at(two())).unwrap();
        assert_eq!((c.monitor, c.why), (1, Why::NotPrimary));
        let c = choose(&MonitorPref::Here, &at(two())).unwrap();
        assert_eq!((c.monitor, c.why), (1, Why::NotPrimary));
    }

    #[test]
    fn a_named_monitor_wins_and_a_missing_one_falls_back() {
        let mut k = at(two());
        k.pointer = Some((2500, 400));
        let c = choose(&MonitorPref::Named("HDMI-3".into()), &k).unwrap();
        assert_eq!((c.monitor, c.why), (1, Why::Named));
        let c = choose(&MonitorPref::Named("DP-2".into()), &k).unwrap();
        assert_eq!((c.monitor, c.why), (0, Why::NamedMissing));
    }

    #[test]
    fn where_i_am_follows_the_work() {
        let mut k = at(two());
        k.pointer = Some((2500, 400));
        let c = choose(&MonitorPref::Here, &k).unwrap();
        assert_eq!((c.monitor, c.why), (1, Why::Here));
    }

    #[test]
    fn a_focused_full_screen_app_is_left_alone() {
        let mut k = at(two());
        k.focus = Some(two()[1].geometry);
        k.focus_fullscreen = true;
        // "Where I am" would be the game's monitor: the other one instead.
        let c = choose(&MonitorPref::Here, &k).unwrap();
        assert_eq!(
            (c.monitor, c.why, c.covered),
            (0, Why::AwayFromFullScreen, false)
        );
        // Named onto the game's monitor: the same.
        let c = choose(&MonitorPref::Named("HDMI-3".into()), &k).unwrap();
        assert_eq!((c.monitor, c.covered), (0, false));
        // "The other monitor" was never the game's.
        let c = choose(&MonitorPref::Other, &k).unwrap();
        assert_eq!((c.monitor, c.why, c.covered), (0, Why::Other, false));
    }

    #[test]
    fn one_monitor_comes_forward_on_the_half_away_from_the_work() {
        let one = vec![two().remove(0)];
        let mut k = at(one);
        assert_eq!(choose(&MonitorPref::Other, &k).unwrap().side, Side::Centre);
        k.pointer = Some((300, 500));
        let c = choose(&MonitorPref::Other, &k).unwrap();
        assert_eq!((c.monitor, c.side, c.why), (0, Side::Right, Why::Only));
        k.focus = Some(Rect {
            x: 1200,
            y: 0,
            width: 700,
            height: 900,
        });
        assert_eq!(choose(&MonitorPref::Here, &k).unwrap().side, Side::Left);
    }

    #[test]
    fn one_monitor_under_a_full_screen_app_is_covered() {
        let one = vec![two().remove(0)];
        let mut k = at(one.clone());
        k.focus = Some(one[0].geometry);
        k.focus_fullscreen = true;
        assert!(choose(&MonitorPref::Other, &k).unwrap().covered);
    }

    #[test]
    fn no_monitors_no_choice() {
        assert_eq!(choose(&MonitorPref::Other, &Known::default()), None);
    }

    #[test]
    fn the_preference_is_stored_as_plain_words() {
        assert_eq!(
            serde_json::to_string(&MonitorPref::Other).unwrap(),
            "\"other\""
        );
        assert_eq!(
            serde_json::to_string(&MonitorPref::Named("HDMI-3".into())).unwrap(),
            "{\"named\":\"HDMI-3\"}"
        );
    }
}
