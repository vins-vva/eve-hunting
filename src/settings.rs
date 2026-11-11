//! User settings kept between runs: the window's place, the page shown, the filters, the
//! sorts, the splitter and the nav. Stored as JSON in `%APPDATA%\eve-scouting\settings.json`
//! (`$XDG_CONFIG_HOME` or `~/.config` elsewhere). A missing or unreadable file gives defaults.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub window: Option<WindowPlace>,
    /// Index of the page shown (0 Thera, 1 Turnur).
    pub page: usize,
    pub query: String,
    pub high: bool,
    pub low: bool,
    pub null: bool,
    pub wormhole: bool,
    pub min_size: usize,
    pub only_new: bool,
    /// The hunting spots' security: 0 Null, 1 Null + Low, 2 All (`SpotSecurity::index`).
    pub spot_security: usize,
    /// Height of the hunting section under the splitter.
    pub bottom_h: f32,
    pub nav_width: f32,
    pub nav_open: bool,
    /// Per page, the connections' and the spots' sort: (column, descending).
    pub sorts: Vec<PageSorts>,
    pub theme: ThemeChoice,
}

/// The app's appearance: the system's, or always light or dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    /// In the order the settings list them.
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }

    /// Whether to draw dark, given what the system uses.
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            ThemeChoice::System => system_dark,
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&x| x == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> ThemeChoice {
        Self::ALL.get(i).copied().unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageSorts {
    pub connections: (String, bool),
    pub spots: (String, bool),
}

/// The window's place in physical desktop pixels, as `WindowPlacement` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPlace {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl WindowPlace {
    /// A place worth reopening at: a real size, and not Windows' minimized sentinel position
    /// (-32000, -32000).
    pub fn is_usable(&self) -> bool {
        self.width >= 200 && self.height >= 200 && self.x > -20000 && self.y > -20000
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            window: None,
            page: 0,
            query: String::new(),
            high: true,
            low: true,
            null: true,
            wormhole: true,
            min_size: 0,
            only_new: false,
            spot_security: 1,
            bottom_h: 360.0,
            nav_width: 180.0,
            nav_open: true,
            sorts: Vec::new(),
            theme: ThemeChoice::System,
        }
    }
}

fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("eve-scouting")
}

pub fn path() -> PathBuf {
    dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Writes the settings, through a temporary file so a crash mid-write never leaves a broken one.
pub fn save(s: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join("settings.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(s).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let s = Settings {
            window: Some(WindowPlace { x: 100, y: 50, width: 1360, height: 880, maximized: false }),
            page: 1,
            query: "omist".into(),
            null: false,
            min_size: 3,
            sorts: vec![PageSorts { connections: ("best".into(), true), spots: ("jumps".into(), false) }],
            ..Settings::default()
        };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{"page": 1}"#).unwrap();
        assert_eq!(s.page, 1);
        assert!(s.high && s.nav_open);
        assert_eq!(s.bottom_h, Settings::default().bottom_h);
    }

    #[test]
    fn the_theme_is_stored_by_name() {
        let s = Settings { theme: ThemeChoice::Dark, ..Settings::default() };
        assert!(serde_json::to_string(&s).unwrap().contains(r#""theme":"dark""#));
        assert!(ThemeChoice::System.is_dark(true) && !ThemeChoice::System.is_dark(false));
        assert!(ThemeChoice::Dark.is_dark(false) && !ThemeChoice::Light.is_dark(true));
    }

    #[test]
    fn minimized_or_tiny_places_are_ignored() {
        let place = |x, y, width, height| WindowPlace { x, y, width, height, maximized: false };
        assert!(place(10, 10, 1360, 880).is_usable());
        assert!(!place(-32000, -32000, 1360, 880).is_usable());
        assert!(!place(10, 10, 0, 0).is_usable());
    }
}
