//! Chord parsing for config-driven keybindings.
//!
//! Chord strings are plus-separated modifier/key tokens, case-insensitive:
//! `"Cmd+Enter"`, `"Cmd+Shift+F"`, `"Ctrl+Alt+T"`. Prefix chords with no
//! terminal key — `"Cmd+Opt"`, `"Cmd"` — are valid and used by the focus /
//! switch-workspace shortcuts where a trailing digit is appended at use
//! site.
//!
//! Modifier aliases: `Cmd`/`Super`/`Meta`, `Alt`/`Opt`/`Option`,
//! `Shift`, `Ctrl`/`Control`.

use crate::config::Keybindings;

/// Parsed chord: a set of modifier bits plus an optional terminal key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Chord {
    pub cmd: bool,
    pub alt: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub key: Option<ChordKey>,
}

/// The non-modifier key at the end of a chord. Chars are always lowercased
/// so `"Cmd+T"` and `"Cmd+t"` match the same physical key.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChordKey {
    Char(char),
    Named(NamedChordKey),
}

/// The handful of named keys our shortcuts actually use today. Extend as
/// new bindings land.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NamedChordKey {
    Enter,
    Tab,
    Space,
    Escape,
}

impl Chord {
    /// Parse a chord string like `"Cmd+Shift+F"`. Returns `None` on any
    /// malformed input (unknown token, empty string, two terminal keys).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let mut chord = Chord::default();
        let mut saw_key = false;
        for raw in s.split('+') {
            let tok = raw.trim();
            if tok.is_empty() {
                return None;
            }
            let lower = tok.to_ascii_lowercase();
            match lower.as_str() {
                "cmd" | "super" | "meta" => chord.cmd = true,
                "alt" | "opt" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                "ctrl" | "control" => chord.ctrl = true,
                "enter" | "return" => {
                    if saw_key {
                        return None;
                    }
                    chord.key = Some(ChordKey::Named(NamedChordKey::Enter));
                    saw_key = true;
                }
                "tab" => {
                    if saw_key {
                        return None;
                    }
                    chord.key = Some(ChordKey::Named(NamedChordKey::Tab));
                    saw_key = true;
                }
                "space" => {
                    if saw_key {
                        return None;
                    }
                    chord.key = Some(ChordKey::Named(NamedChordKey::Space));
                    saw_key = true;
                }
                "esc" | "escape" => {
                    if saw_key {
                        return None;
                    }
                    chord.key = Some(ChordKey::Named(NamedChordKey::Escape));
                    saw_key = true;
                }
                other => {
                    // Single-char terminal key.
                    let mut chars = other.chars();
                    let first = chars.next()?;
                    if chars.next().is_some() {
                        return None;
                    }
                    if saw_key {
                        return None;
                    }
                    chord.key = Some(ChordKey::Char(first));
                    saw_key = true;
                }
            }
        }
        // A chord must carry *something* — either a modifier or a key.
        if !chord.cmd && !chord.alt && !chord.shift && !chord.ctrl && chord.key.is_none() {
            return None;
        }
        Some(chord)
    }

    /// True if both chords have the same modifier set (ignoring the key
    /// field). Used for prefix chords like `focus_tile_prefix` where the
    /// digit is appended at use site.
    #[must_use]
    pub fn modifiers_match(&self, other: &Self) -> bool {
        self.cmd == other.cmd
            && self.alt == other.alt
            && self.shift == other.shift
            && self.ctrl == other.ctrl
    }
}

/// All shortcuts pre-parsed from `Keybindings`. Build once at config load
/// / reload time so the hot shortcut path doesn't re-parse strings.
///
/// A malformed config string falls back to the default binding for that
/// slot and logs a warning; we never panic on user config.
#[derive(Clone, Debug)]
pub struct ResolvedKeybindings {
    pub zen_mode: Chord,
    pub new_tile: Chord,
    pub close_tile: Chord,
    pub paste: Chord,
    pub copy: Chord,
    pub new_workspace: Chord,
    pub rename_workspace: Chord,
    pub cycle_layout: Chord,
}

impl ResolvedKeybindings {
    #[must_use]
    pub fn from_config(k: &Keybindings) -> Self {
        let defaults = Keybindings::default();
        Self {
            zen_mode: parse_or_default("zen_mode", &k.zen_mode, &defaults.zen_mode),
            new_tile: parse_or_default("new_tile", &k.new_tile, &defaults.new_tile),
            close_tile: parse_or_default("close_tile", &k.close_tile, &defaults.close_tile),
            paste: parse_or_default("paste", &k.paste, &defaults.paste),
            copy: parse_or_default("copy", &k.copy, &defaults.copy),
            new_workspace: parse_or_default(
                "new_workspace",
                &k.new_workspace,
                &defaults.new_workspace,
            ),
            rename_workspace: parse_or_default(
                "rename_workspace",
                &k.rename_workspace,
                &defaults.rename_workspace,
            ),
            cycle_layout: parse_or_default("cycle_layout", &k.cycle_layout, &defaults.cycle_layout),
        }
    }
}

impl Default for ResolvedKeybindings {
    fn default() -> Self {
        Self::from_config(&Keybindings::default())
    }
}

fn parse_or_default(slot: &str, value: &str, default: &str) -> Chord {
    if let Some(c) = Chord::parse(value) {
        return c;
    }
    log::warn!("keybinding '{slot}' = \"{value}\" failed to parse; using default \"{default}\"");
    Chord::parse(default).expect("built-in default keybinding must parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_chord() {
        let c = Chord::parse("Cmd+T").unwrap();
        assert!(c.cmd);
        assert!(!c.alt);
        assert_eq!(c.key, Some(ChordKey::Char('t')));
    }

    #[test]
    fn parse_is_case_insensitive() {
        let a = Chord::parse("cmd+shift+f").unwrap();
        let b = Chord::parse("CMD+SHIFT+F").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parse_named_keys() {
        assert_eq!(
            Chord::parse("Cmd+Enter").unwrap().key,
            Some(ChordKey::Named(NamedChordKey::Enter))
        );
        assert_eq!(
            Chord::parse("Shift+Tab").unwrap().key,
            Some(ChordKey::Named(NamedChordKey::Tab))
        );
        assert_eq!(
            Chord::parse("Escape").unwrap().key,
            Some(ChordKey::Named(NamedChordKey::Escape))
        );
        assert_eq!(
            Chord::parse("Space").unwrap().key,
            Some(ChordKey::Named(NamedChordKey::Space))
        );
    }

    #[test]
    fn modifier_aliases() {
        assert!(Chord::parse("Super+T").unwrap().cmd);
        assert!(Chord::parse("Meta+T").unwrap().cmd);
        assert!(Chord::parse("Opt+T").unwrap().alt);
        assert!(Chord::parse("Option+T").unwrap().alt);
        assert!(Chord::parse("Control+T").unwrap().ctrl);
    }

    #[test]
    fn parse_prefix_chord_has_no_key() {
        let c = Chord::parse("Cmd+Opt").unwrap();
        assert!(c.cmd);
        assert!(c.alt);
        assert!(c.key.is_none());
    }

    #[test]
    fn parse_rejects_two_terminal_keys() {
        assert!(Chord::parse("T+W").is_none());
        assert!(Chord::parse("Cmd+T+W").is_none());
        assert!(Chord::parse("Enter+Tab").is_none());
    }

    #[test]
    fn parse_rejects_empty_and_garbage() {
        assert!(Chord::parse("").is_none());
        assert!(Chord::parse("+").is_none());
        assert!(Chord::parse("Cmd+").is_none());
        assert!(Chord::parse("Cmd+xyz").is_none());
        assert!(Chord::parse("Nope").is_none());
    }

    #[test]
    fn modifiers_match_ignores_key() {
        let a = Chord::parse("Cmd+Opt+1").unwrap();
        let b = Chord::parse("Cmd+Opt").unwrap();
        assert!(a.modifiers_match(&b));
        let c = Chord::parse("Cmd+Shift").unwrap();
        assert!(!a.modifiers_match(&c));
    }

    #[test]
    fn resolved_from_defaults_all_valid() {
        let r = ResolvedKeybindings::default();
        assert_eq!(r.zen_mode.key, Some(ChordKey::Named(NamedChordKey::Enter)));
        assert_eq!(r.new_tile.key, Some(ChordKey::Char('t')));
    }

    #[test]
    fn resolved_falls_back_on_garbage() {
        let k = Keybindings {
            new_tile: "totally bogus".into(),
            ..Keybindings::default()
        };
        let r = ResolvedKeybindings::from_config(&k);
        assert_eq!(
            r.new_tile.key,
            Some(ChordKey::Char('t')),
            "garbage should fall back to the default binding"
        );
    }
}
