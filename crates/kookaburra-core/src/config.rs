//! Runtime configuration.
//!
//! Phase 5: real TOML loading from `$XDG_CONFIG_HOME/kookaburra/config.toml`
//! via `directories`. Missing file → defaults + log notice. Malformed file →
//! defaults + error log (we never panic on user config).
//!
//! The shape:
//!
//! ```toml
//! [font]
//! family = "Menlo"
//! size_px = 15.0
//!
//! # Either the name of a builtin (Kookaburra / Tokyo Night / Catppuccin
//! # Mocha / Solarized Dark) OR a filename (without extension) under
//! # `$XDG_CONFIG_HOME/kookaburra/themes/`.
//! [theme]
//! name = "Kookaburra"
//!
//! [keybindings]
//! zen_mode = "Cmd+Enter"
//! new_tile = "Cmd+T"
//! # ...
//! ```

use std::fs;
use std::path::PathBuf;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

/// 8-bit RGBA color used by both terminal cells and UI chrome.
///
/// Serialization uses a `#RRGGBB` or `#RRGGBBAA` hex string. This matches
/// how palettes are shared in the wider terminal / theme ecosystem and
/// means hand-edited config files are readable.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize)]
#[serde(into = "String")]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    #[must_use]
    pub const fn array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Convert to a `[f32; 4]` in 0.0–1.0 range. Useful for wgpu clear ops.
    #[must_use]
    pub fn linear(self) -> [f32; 4] {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            f32::from(self.a) / 255.0,
        ]
    }

    /// Parse a `#RRGGBB` or `#RRGGBBAA` hex string (case insensitive,
    /// leading `#` required). Returns `None` on malformed input.
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        let hex = s.strip_prefix('#')?;
        let (r, g, b, a) = match hex.len() {
            6 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                255u8,
            ),
            8 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                u8::from_str_radix(&hex[6..8], 16).ok()?,
            ),
            _ => return None,
        };
        Some(Self { r, g, b, a })
    }

    /// Render as `#RRGGBB` (alpha=255) or `#RRGGBBAA`.
    #[must_use]
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }
}

impl From<Rgba> for String {
    fn from(c: Rgba) -> String {
        c.to_hex()
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HexVisitor;
        impl Visitor<'_> for HexVisitor {
            type Value = Rgba;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a hex color string like \"#RRGGBB\" or \"#RRGGBBAA\"")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Rgba, E> {
                Rgba::from_hex(v)
                    .ok_or_else(|| de::Error::invalid_value(de::Unexpected::Str(v), &self))
            }
        }
        deserializer.deserialize_str(HexVisitor)
    }
}

/// Color palette resolved by the renderer when a terminal cell asks for a
/// named or indexed color. ANSI 0–15 are the standard 16-color palette.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    pub selection_bg: Rgba,
    pub ansi: [Rgba; 16],
}

impl Theme {
    /// Loosely Tokyo Night.
    #[must_use]
    pub fn tokyo_night() -> Self {
        Self {
            name: "Tokyo Night".to_string(),
            foreground: Rgba::rgb(0xa9, 0xb1, 0xd6),
            background: Rgba::rgb(0x1a, 0x1b, 0x26),
            cursor: Rgba::rgb(0xc0, 0xca, 0xf5),
            selection_bg: Rgba::rgb(0x33, 0x46, 0x7c),
            ansi: [
                Rgba::rgb(0x15, 0x16, 0x1e), // black
                Rgba::rgb(0xf7, 0x76, 0x8e), // red
                Rgba::rgb(0x9e, 0xce, 0x6a), // green
                Rgba::rgb(0xe0, 0xaf, 0x68), // yellow
                Rgba::rgb(0x7a, 0xa2, 0xf7), // blue
                Rgba::rgb(0xbb, 0x9a, 0xf7), // magenta
                Rgba::rgb(0x7d, 0xcf, 0xff), // cyan
                Rgba::rgb(0xa9, 0xb1, 0xd6), // white
                Rgba::rgb(0x41, 0x48, 0x68), // bright black
                Rgba::rgb(0xff, 0x9e, 0x64), // bright red
                Rgba::rgb(0xb9, 0xf2, 0x7c), // bright green
                Rgba::rgb(0xff, 0xc7, 0x77), // bright yellow
                Rgba::rgb(0x9d, 0xb1, 0xff), // bright blue
                Rgba::rgb(0xc0, 0xa6, 0xff), // bright magenta
                Rgba::rgb(0x9c, 0xe5, 0xff), // bright cyan
                Rgba::rgb(0xc0, 0xca, 0xf5), // bright white
            ],
        }
    }

    /// Warm Kookaburra amber palette — derived from the design system's
    /// OKLCH values in `docs/design/Kookaburra/data.js`.
    #[must_use]
    pub fn kookaburra() -> Self {
        Self {
            name: "Kookaburra".to_string(),
            foreground: Rgba::rgb(0xee, 0xeb, 0xe5),
            background: Rgba::rgb(0x08, 0x06, 0x04),
            cursor: Rgba::rgb(0xff, 0xa5, 0x1c),
            selection_bg: Rgba::rgb(0xff, 0xa5, 0x1c),
            ansi: [
                Rgba::rgb(0x04, 0x03, 0x02),
                Rgba::rgb(0xfa, 0x68, 0x63),
                Rgba::rgb(0x6e, 0xd2, 0x74),
                Rgba::rgb(0xf5, 0xcc, 0x58),
                Rgba::rgb(0x4d, 0xac, 0xf6),
                Rgba::rgb(0xdb, 0x7c, 0xd4),
                Rgba::rgb(0x48, 0xb7, 0xbd),
                Rgba::rgb(0xee, 0xeb, 0xe5),
                Rgba::rgb(0x2d, 0x28, 0x23),
                Rgba::rgb(0xff, 0x82, 0x7b),
                Rgba::rgb(0x89, 0xec, 0x8d),
                Rgba::rgb(0xff, 0xe0, 0x6d),
                Rgba::rgb(0x68, 0xc6, 0xff),
                Rgba::rgb(0xf6, 0x95, 0xee),
                Rgba::rgb(0x64, 0xd1, 0xd7),
                Rgba::rgb(0xfb, 0xf8, 0xf2),
            ],
        }
    }

    /// Catppuccin Mocha — from the upstream palette spec. Published under
    /// MIT. See <https://github.com/catppuccin/catppuccin>.
    #[must_use]
    pub fn catppuccin_mocha() -> Self {
        Self {
            name: "Catppuccin Mocha".to_string(),
            foreground: Rgba::rgb(0xcd, 0xd6, 0xf4),   // Text
            background: Rgba::rgb(0x1e, 0x1e, 0x2e),   // Base
            cursor: Rgba::rgb(0xf5, 0xe0, 0xdc),       // Rosewater
            selection_bg: Rgba::rgb(0x58, 0x5b, 0x70), // Surface2
            ansi: [
                Rgba::rgb(0x45, 0x47, 0x5a), // Surface1 (black)
                Rgba::rgb(0xf3, 0x8b, 0xa8), // red
                Rgba::rgb(0xa6, 0xe3, 0xa1), // green
                Rgba::rgb(0xf9, 0xe2, 0xaf), // yellow
                Rgba::rgb(0x89, 0xb4, 0xfa), // blue
                Rgba::rgb(0xf5, 0xc2, 0xe7), // magenta (pink)
                Rgba::rgb(0x94, 0xe2, 0xd5), // cyan (teal)
                Rgba::rgb(0xba, 0xc2, 0xde), // white (Subtext1)
                Rgba::rgb(0x58, 0x5b, 0x70), // bright black (Surface2)
                Rgba::rgb(0xeb, 0xa0, 0xac), // bright red (maroon)
                Rgba::rgb(0xa6, 0xe3, 0xa1), // bright green
                Rgba::rgb(0xf9, 0xe2, 0xaf), // bright yellow
                Rgba::rgb(0x89, 0xdc, 0xeb), // bright blue (sky)
                Rgba::rgb(0xcb, 0xa6, 0xf7), // bright magenta (mauve)
                Rgba::rgb(0x94, 0xe2, 0xd5), // bright cyan
                Rgba::rgb(0xa6, 0xad, 0xc8), // bright white (Subtext0)
            ],
        }
    }

    /// Solarized Dark — Ethan Schoonover's original palette. MIT licensed.
    /// <https://ethanschoonover.com/solarized/>
    #[must_use]
    pub fn solarized_dark() -> Self {
        Self {
            name: "Solarized Dark".to_string(),
            foreground: Rgba::rgb(0x83, 0x94, 0x96), // base0
            background: Rgba::rgb(0x00, 0x2b, 0x36), // base03
            cursor: Rgba::rgb(0x93, 0xa1, 0xa1),     // base1
            selection_bg: Rgba::rgb(0x07, 0x36, 0x42), // base02
            ansi: [
                Rgba::rgb(0x07, 0x36, 0x42), // base02 (black)
                Rgba::rgb(0xdc, 0x32, 0x2f), // red
                Rgba::rgb(0x85, 0x99, 0x00), // green
                Rgba::rgb(0xb5, 0x89, 0x00), // yellow
                Rgba::rgb(0x26, 0x8b, 0xd2), // blue
                Rgba::rgb(0xd3, 0x36, 0x82), // magenta
                Rgba::rgb(0x2a, 0xa1, 0x98), // cyan
                Rgba::rgb(0xee, 0xe8, 0xd5), // white (base2)
                Rgba::rgb(0x00, 0x2b, 0x36), // bright black (base03)
                Rgba::rgb(0xcb, 0x4b, 0x16), // bright red (orange)
                Rgba::rgb(0x58, 0x6e, 0x75), // bright green (base01)
                Rgba::rgb(0x65, 0x7b, 0x83), // bright yellow (base00)
                Rgba::rgb(0x83, 0x94, 0x96), // bright blue (base0)
                Rgba::rgb(0x6c, 0x71, 0xc4), // bright magenta (violet)
                Rgba::rgb(0x93, 0xa1, 0xa1), // bright cyan (base1)
                Rgba::rgb(0xfd, 0xf6, 0xe3), // bright white (base3)
            ],
        }
    }

    /// Look up a builtin theme by case-insensitive name. Accepts the
    /// spaced display name ("Tokyo Night") or the kebab form
    /// ("tokyo-night"). Returns `None` if not a builtin.
    #[must_use]
    pub fn builtin(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase().replace(['-', '_'], " ");
        match key.as_str() {
            "kookaburra" => Some(Self::kookaburra()),
            "tokyo night" => Some(Self::tokyo_night()),
            "catppuccin mocha" => Some(Self::catppuccin_mocha()),
            "solarized dark" => Some(Self::solarized_dark()),
            _ => None,
        }
    }

    /// All builtin theme names, in display order.
    #[must_use]
    pub fn builtin_names() -> &'static [&'static str] {
        &[
            "Kookaburra",
            "Tokyo Night",
            "Catppuccin Mocha",
            "Solarized Dark",
        ]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::kookaburra()
    }
}

/// Either a reference to a named theme, or a fully inline palette.
///
/// Variants are tried in order by `serde(untagged)`, so Inline must come
/// before ByName — a `[theme]` table with every field set should be
/// parsed as the full palette, not as a name-only reference to a builtin.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum ThemeRef {
    /// `theme = "Tokyo Night"`.
    Named(String),
    /// Full inline palette in `[theme]`.
    Inline(Theme),
    /// `[theme]` table with just `name = "..."` — refers to a builtin
    /// or an external themes/<name>.toml file.
    ByName { name: String },
}

/// Font configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FontConfig {
    #[serde(default = "default_font_family")]
    pub family: String,
    #[serde(default = "default_font_size")]
    pub size_px: f32,
}

fn default_font_family() -> String {
    "Menlo".to_string()
}

fn default_font_size() -> f32 {
    19.0
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: default_font_family(),
            size_px: default_font_size(),
        }
    }
}

/// Keybinding configuration. Stored as human-readable chord strings like
/// `"Cmd+T"` or `"Cmd+Shift+F"`; parsing into actual key events is the
/// app layer's job. Missing fields fall back to the defaults below.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybindings {
    pub zen_mode: String,
    pub new_tile: String,
    pub close_tile: String,
    pub paste: String,
    pub copy: String,
    pub new_workspace: String,
    pub rename_workspace: String,
    pub cycle_layout: String,
    /// Base chord for `Cmd+Opt+N` (focus Nth tile). The `N` digit is
    /// appended by the app. e.g. `"Cmd+Opt"` → `Cmd+Opt+1`..`Cmd+Opt+6`.
    pub focus_tile_prefix: String,
    /// Base chord for `Cmd+N` (switch to Nth workspace). See above.
    pub switch_workspace_prefix: String,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            zen_mode: "Cmd+Enter".into(),
            new_tile: "Cmd+T".into(),
            close_tile: "Cmd+W".into(),
            paste: "Cmd+V".into(),
            copy: "Cmd+C".into(),
            new_workspace: "Cmd+N".into(),
            rename_workspace: "Cmd+L".into(),
            cycle_layout: "Cmd+G".into(),
            focus_tile_prefix: "Cmd+Opt".into(),
            switch_workspace_prefix: "Cmd".into(),
        }
    }
}

/// Top-level runtime config.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub theme: Theme,
    pub font: FontConfig,
    pub keybindings: Keybindings,
}

/// On-disk representation. Deserialization goes through this so we can
/// apply our "theme name → builtin lookup with optional inline override"
/// semantics before handing off to the rest of the app.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    font: FontConfig,
    theme: Option<ThemeRef>,
    keybindings: Keybindings,
}

impl Config {
    /// Load config from the standard XDG path. Returns the default config
    /// when the file is absent; logs and returns the default when the
    /// file is malformed. Never panics.
    ///
    /// External themes: if `theme = "my-theme"` refers to neither a builtin
    /// nor an inline palette, we look for
    /// `<config_dir>/themes/my-theme.toml` (case-insensitive match on the
    /// kebab form) and parse that as a standalone `Theme`.
    #[must_use]
    pub fn load_or_default() -> Self {
        match Self::try_load() {
            Ok(Some(cfg)) => cfg,
            Ok(None) => {
                log::info!("no config file found; using defaults");
                Self::default()
            }
            Err(e) => {
                log::error!("config load failed ({e}); using defaults");
                Self::default()
            }
        }
    }

    /// Returns `Ok(None)` when no config file exists, `Ok(Some(_))` on a
    /// successful load, and `Err(_)` for IO / parse errors.
    pub fn try_load() -> Result<Option<Self>, ConfigError> {
        let Some(paths) = ConfigPaths::discover() else {
            return Ok(None);
        };
        if !paths.config_file.exists() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&paths.config_file).map_err(|e| ConfigError::Io(e.to_string()))?;
        let raw: RawConfig =
            toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let theme = resolve_theme(raw.theme, &paths)?;
        Ok(Some(Self {
            theme,
            font: raw.font,
            keybindings: raw.keybindings,
        }))
    }
}

/// Resolved locations for config and theme assets.
#[derive(Clone, Debug)]
pub struct ConfigPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub themes_dir: PathBuf,
}

impl ConfigPaths {
    /// `$XDG_CONFIG_HOME/kookaburra/` on Linux,
    /// `~/Library/Application Support/kookaburra/` on macOS,
    /// `%APPDATA%\kookaburra\config\` on Windows.
    /// Returns `None` on exotic platforms where no config dir exists
    /// (e.g. certain sandboxed environments).
    #[must_use]
    pub fn discover() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("", "", "kookaburra")?;
        let config_dir = dirs.config_dir().to_path_buf();
        Some(Self {
            config_file: config_dir.join("config.toml"),
            themes_dir: config_dir.join("themes"),
            config_dir,
        })
    }
}

fn resolve_theme(r: Option<ThemeRef>, paths: &ConfigPaths) -> Result<Theme, ConfigError> {
    let Some(r) = r else {
        return Ok(Theme::default());
    };
    match r {
        ThemeRef::Named(name) | ThemeRef::ByName { name } => {
            if let Some(t) = Theme::builtin(&name) {
                return Ok(t);
            }
            // Look in themes/<kebab>.toml.
            let kebab = name.trim().to_ascii_lowercase().replace(' ', "-");
            let path = paths.themes_dir.join(format!("{kebab}.toml"));
            if !path.exists() {
                return Err(ConfigError::UnknownTheme(name));
            }
            let text = fs::read_to_string(&path).map_err(|e| ConfigError::Io(e.to_string()))?;
            let theme: Theme =
                toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string()))?;
            Ok(theme)
        }
        ThemeRef::Inline(theme) => Ok(theme),
    }
}

/// Errors surfaced by the config loader. The app wraps these with a
/// log line and falls back to defaults, but tests want the full error.
#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
    UnknownTheme(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "io error: {s}"),
            Self::Parse(s) => write!(f, "parse error: {s}"),
            Self::UnknownTheme(n) => write!(f, "unknown theme: {n}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_rgb_sets_alpha_to_opaque() {
        let c = Rgba::rgb(0x10, 0x20, 0x30);
        assert_eq!(c.r, 0x10);
        assert_eq!(c.g, 0x20);
        assert_eq!(c.b, 0x30);
        assert_eq!(c.a, 255);
    }

    #[test]
    fn rgba_linear_clamps_into_unit_range() {
        let c = Rgba::rgb(255, 0, 128);
        let lin = c.linear();
        assert!((lin[0] - 1.0).abs() < 1e-6);
        assert!(lin[1].abs() < 1e-6);
        assert!((lin[2] - 128.0 / 255.0).abs() < 1e-6);
        assert!((lin[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rgba_from_hex_rgb() {
        assert_eq!(Rgba::from_hex("#102030"), Some(Rgba::rgb(0x10, 0x20, 0x30)));
        assert_eq!(Rgba::from_hex("#abcdef"), Some(Rgba::rgb(0xAB, 0xCD, 0xEF)));
        assert_eq!(Rgba::from_hex("#ABCDEF"), Some(Rgba::rgb(0xAB, 0xCD, 0xEF)));
    }

    #[test]
    fn rgba_from_hex_rgba() {
        let c = Rgba::from_hex("#10203040").unwrap();
        assert_eq!(
            c,
            Rgba {
                r: 0x10,
                g: 0x20,
                b: 0x30,
                a: 0x40
            }
        );
    }

    #[test]
    fn rgba_from_hex_rejects_garbage() {
        assert!(Rgba::from_hex("abcdef").is_none(), "missing #");
        assert!(
            Rgba::from_hex("#abc").is_none(),
            "3-digit short form not supported"
        );
        assert!(Rgba::from_hex("#ZZZZZZ").is_none(), "non-hex chars");
        assert!(Rgba::from_hex("").is_none());
    }

    #[test]
    fn rgba_hex_roundtrip() {
        let c = Rgba::rgb(0xDE, 0xAD, 0xBE);
        assert_eq!(c.to_hex(), "#DEADBE");
        assert_eq!(Rgba::from_hex(&c.to_hex()), Some(c));
        let a = Rgba {
            r: 1,
            g: 2,
            b: 3,
            a: 4,
        };
        assert_eq!(a.to_hex(), "#01020304");
        assert_eq!(Rgba::from_hex(&a.to_hex()), Some(a));
    }

    #[test]
    fn rgba_deserialize_from_string() {
        #[derive(Deserialize)]
        struct W {
            c: Rgba,
        }
        let v: W = toml::from_str("c = \"#112233\"").unwrap();
        assert_eq!(v.c, Rgba::rgb(0x11, 0x22, 0x33));
    }

    #[test]
    fn tokyo_night_theme_has_sixteen_ansi_colors() {
        let t = Theme::tokyo_night();
        assert_eq!(t.ansi.len(), 16);
        let bg_sum =
            u32::from(t.background.r) + u32::from(t.background.g) + u32::from(t.background.b);
        let fg_sum =
            u32::from(t.foreground.r) + u32::from(t.foreground.g) + u32::from(t.foreground.b);
        assert!(
            bg_sum < fg_sum,
            "background should be darker than foreground"
        );
    }

    #[test]
    fn all_builtin_themes_have_darker_background() {
        for name in Theme::builtin_names() {
            let t = Theme::builtin(name).unwrap_or_else(|| panic!("builtin {name} missing"));
            let bg =
                u32::from(t.background.r) + u32::from(t.background.g) + u32::from(t.background.b);
            let fg =
                u32::from(t.foreground.r) + u32::from(t.foreground.g) + u32::from(t.foreground.b);
            assert!(bg < fg, "theme {name}: bg should be darker than fg");
            assert_eq!(t.ansi.len(), 16);
        }
    }

    #[test]
    fn theme_builtin_lookup_is_case_and_separator_insensitive() {
        assert!(Theme::builtin("kookaburra").is_some());
        assert!(Theme::builtin("TOKYO NIGHT").is_some());
        assert!(Theme::builtin("catppuccin-mocha").is_some());
        assert!(Theme::builtin("solarized_dark").is_some());
        assert!(Theme::builtin("not a real theme").is_none());
    }

    #[test]
    fn default_config_uses_kookaburra_theme() {
        let c = Config::default();
        assert_eq!(c.theme.name, "Kookaburra");
    }

    #[test]
    fn default_font_size_is_reasonable() {
        let f = FontConfig::default();
        assert!(f.size_px >= 8.0 && f.size_px <= 64.0);
        assert!(!f.family.is_empty());
    }

    #[test]
    fn default_keybindings_use_cmd_prefix() {
        let k = Keybindings::default();
        assert!(k.zen_mode.starts_with("Cmd"));
        assert!(k.new_tile.starts_with("Cmd"));
        assert!(k.cycle_layout.starts_with("Cmd"));
    }

    #[test]
    fn parse_minimal_toml() {
        // NB: top-level `theme = "..."` must come BEFORE any table header
        // — toml semantics put it inside the previous table otherwise.
        let text = r##"
            theme = "Tokyo Night"

            [font]
            family = "Fira Code"
            size_px = 14.0

            [keybindings]
            new_tile = "Cmd+Shift+T"
        "##;
        let raw: RawConfig = toml::from_str(text).unwrap();
        assert_eq!(raw.font.family, "Fira Code");
        assert_eq!(raw.font.size_px, 14.0);
        assert_eq!(raw.keybindings.new_tile, "Cmd+Shift+T");
        // Other keybindings should fall back to defaults.
        assert_eq!(raw.keybindings.zen_mode, "Cmd+Enter");
        // theme_ref stays a bare name; resolution happens in resolve_theme.
        match raw.theme.unwrap() {
            ThemeRef::Named(n) => assert_eq!(n, "Tokyo Night"),
            other => panic!("expected Named, got {other:?}"),
        }
    }

    #[test]
    fn parse_inline_theme() {
        let text = r##"
            [theme]
            name = "Custom"
            foreground = "#ffffff"
            background = "#000000"
            cursor = "#ff0000"
            selection_bg = "#222222"
            ansi = [
                "#000000","#ff0000","#00ff00","#ffff00",
                "#0000ff","#ff00ff","#00ffff","#ffffff",
                "#111111","#ff8888","#88ff88","#ffff88",
                "#8888ff","#ff88ff","#88ffff","#eeeeee",
            ]
        "##;
        let raw: RawConfig = toml::from_str(text).unwrap();
        match raw.theme.unwrap() {
            ThemeRef::Inline(t) => {
                assert_eq!(t.name, "Custom");
                assert_eq!(t.foreground, Rgba::rgb(0xff, 0xff, 0xff));
                assert_eq!(t.ansi[1], Rgba::rgb(0xff, 0, 0));
            }
            other => panic!("expected Inline, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_toml_uses_defaults() {
        let raw: RawConfig = toml::from_str("").unwrap();
        assert_eq!(raw.font.family, "Menlo");
        assert!(raw.theme.is_none());
        assert_eq!(raw.keybindings.zen_mode, "Cmd+Enter");
    }

    #[test]
    fn resolve_unknown_theme_is_an_error() {
        let paths = ConfigPaths {
            config_dir: PathBuf::from("/nonexistent"),
            config_file: PathBuf::from("/nonexistent/config.toml"),
            themes_dir: PathBuf::from("/nonexistent/themes"),
        };
        let err = resolve_theme(Some(ThemeRef::Named("not a theme".into())), &paths).unwrap_err();
        matches!(err, ConfigError::UnknownTheme(_));
    }
}
