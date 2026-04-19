//! Runtime configuration.
//!
//! Phase 5 wires real TOML loading via `directories` and hot reload via
//! `notify`. For the rough draft we ship hard-coded defaults so the rest
//! of the app has something to read.

/// 8-bit RGBA color used by both terminal cells and UI chrome.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
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
}

/// Color palette resolved by the renderer when a terminal cell asks for a
/// named or indexed color. ANSI 0–15 are the standard 16-color palette.
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    pub selection_bg: Rgba,
    pub ansi: [Rgba; 16],
}

impl Theme {
    /// Builtin default. Loosely Tokyo Night.
    #[must_use]
    pub fn tokyo_night() -> Self {
        Self {
            name: "Tokyo Night",
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
                // bright variants
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
    /// OKLCH values in `docs/design/Kookaburra/data.js`. Background is
    /// near-black with a warm brown tint; foreground is a warm off-white.
    /// ANSI 0..15 map to the design's `red/green/yellow/blue/magenta/teal`
    /// plus brighter variants (L bumped by ~+0.08).
    #[must_use]
    pub fn kookaburra() -> Self {
        Self {
            name: "Kookaburra",
            foreground: Rgba::rgb(0xee, 0xeb, 0xe5),   // fg
            background: Rgba::rgb(0x08, 0x06, 0x04),   // bg — near-pure-black, warm tint
            cursor: Rgba::rgb(0xff, 0xa5, 0x1c),       // accent amber
            selection_bg: Rgba::rgb(0xff, 0xa5, 0x1c), // accent (alpha applied at paint time)
            ansi: [
                Rgba::rgb(0x04, 0x03, 0x02), // 0 black     → bgDeep
                Rgba::rgb(0xfa, 0x68, 0x63), // 1 red
                Rgba::rgb(0x6e, 0xd2, 0x74), // 2 green
                Rgba::rgb(0xf5, 0xcc, 0x58), // 3 yellow
                Rgba::rgb(0x4d, 0xac, 0xf6), // 4 blue
                Rgba::rgb(0xdb, 0x7c, 0xd4), // 5 magenta
                Rgba::rgb(0x48, 0xb7, 0xbd), // 6 cyan (teal)
                Rgba::rgb(0xee, 0xeb, 0xe5), // 7 white (fg)
                // bright variants: same hues, L + ~0.08
                Rgba::rgb(0x2d, 0x28, 0x23), // 8 bright black → gridLine
                Rgba::rgb(0xff, 0x82, 0x7b), // 9 bright red
                Rgba::rgb(0x89, 0xec, 0x8d), // 10 bright green
                Rgba::rgb(0xff, 0xe0, 0x6d), // 11 bright yellow
                Rgba::rgb(0x68, 0xc6, 0xff), // 12 bright blue
                Rgba::rgb(0xf6, 0x95, 0xee), // 13 bright magenta
                Rgba::rgb(0x64, 0xd1, 0xd7), // 14 bright cyan
                Rgba::rgb(0xfb, 0xf8, 0xf2), // 15 bright white
            ],
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::kookaburra()
    }
}

/// Font configuration. Phase 5 will surface this in TOML.
#[derive(Clone, Debug)]
pub struct FontConfig {
    pub family: String,
    pub size_px: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Menlo".to_string(),
            size_px: 15.0,
        }
    }
}

/// Top-level runtime config.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub theme: Theme,
    pub font: FontConfig,
}

impl Config {
    /// Returns the hard-coded default config. Phase 5 swaps this for a
    /// real loader.
    #[must_use]
    pub fn load_or_default() -> Self {
        Self::default()
    }
}

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
        // 255 → 1.0, 0 → 0.0, 128 → ~0.502
        assert!((lin[0] - 1.0).abs() < 1e-6);
        assert!(lin[1].abs() < 1e-6);
        assert!((lin[2] - 128.0 / 255.0).abs() < 1e-6);
        assert!((lin[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tokyo_night_theme_has_sixteen_ansi_colors() {
        let t = Theme::tokyo_night();
        assert_eq!(t.ansi.len(), 16);
        // Background must render darker than foreground on a tokyo-night
        // theme, otherwise we've swapped the fields.
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
}
