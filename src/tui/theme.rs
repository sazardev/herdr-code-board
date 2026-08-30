//! Colours, taken from herdr's own theme.
//!
//! The board is a pane inside herdr, so it should not look like a guest. Herdr
//! names its palette with a fixed set of tokens — `accent`, `surface0`,
//! `overlay1`, `text`, `subtext0`, and the colour names — and lets you pick a
//! built-in theme and override any token. This reads `[theme]` out of herdr's
//! config and uses the same tokens, so changing herdr's theme changes the board.
//!
//! What it cannot do is read herdr's compiled palette: there is no API for it.
//! So these are the upstream palettes each theme is built from, which matches
//! the family and the mood rather than claiming to match every pixel.

use std::str::FromStr;

use ratatui::style::Color;

use crate::model::Column;

/// Herdr's palette tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub accent: Color,
    pub text: Color,
    pub subtext: Color,
    pub overlay: Color,
    pub surface: Color,
    pub active_row_bg: Color,
    pub panel_bg: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub teal: Color,
    pub peach: Color,
    pub mauve: Color,
}

fn hex(s: &str) -> Color {
    Color::from_str(s).unwrap_or(Color::Reset)
}

impl Theme {
    /// Herdr's default theme.
    pub fn catppuccin() -> Self {
        Self {
            accent: hex("#cba6f7"),
            text: hex("#cdd6f4"),
            subtext: hex("#a6adc8"),
            overlay: hex("#6c7086"),
            surface: hex("#313244"),
            active_row_bg: hex("#313244"),
            panel_bg: hex("#1e1e2e"),
            green: hex("#a6e3a1"),
            yellow: hex("#f9e2af"),
            red: hex("#f38ba8"),
            blue: hex("#89b4fa"),
            teal: hex("#94e2d5"),
            peach: hex("#fab387"),
            mauve: hex("#cba6f7"),
        }
    }

    pub fn gruvbox() -> Self {
        Self {
            accent: hex("#83a598"),
            text: hex("#ebdbb2"),
            subtext: hex("#a89984"),
            overlay: hex("#928374"),
            surface: hex("#3c3836"),
            active_row_bg: hex("#3c3836"),
            panel_bg: hex("#282828"),
            green: hex("#b8bb26"),
            yellow: hex("#fabd2f"),
            red: hex("#fb4934"),
            blue: hex("#83a598"),
            teal: hex("#8ec07c"),
            peach: hex("#fe8019"),
            mauve: hex("#d3869b"),
        }
    }

    pub fn tokyonight() -> Self {
        Self {
            accent: hex("#7aa2f7"),
            text: hex("#c0caf5"),
            subtext: hex("#a9b1d6"),
            overlay: hex("#565f89"),
            surface: hex("#292e42"),
            active_row_bg: hex("#292e42"),
            panel_bg: hex("#1a1b26"),
            green: hex("#9ece6a"),
            yellow: hex("#e0af68"),
            red: hex("#f7768e"),
            blue: hex("#7aa2f7"),
            teal: hex("#73daca"),
            peach: hex("#ff9e64"),
            mauve: hex("#bb9af7"),
        }
    }

    pub fn nord() -> Self {
        Self {
            accent: hex("#88c0d0"),
            text: hex("#eceff4"),
            subtext: hex("#d8dee9"),
            overlay: hex("#4c566a"),
            surface: hex("#3b4252"),
            active_row_bg: hex("#3b4252"),
            panel_bg: hex("#2e3440"),
            green: hex("#a3be8c"),
            yellow: hex("#ebcb8b"),
            red: hex("#bf616a"),
            blue: hex("#81a1c1"),
            teal: hex("#8fbcbb"),
            peach: hex("#d08770"),
            mauve: hex("#b48ead"),
        }
    }

    /// Pick a palette by herdr's theme name. Unknown names fall back to herdr's
    /// own default rather than to something invented.
    pub fn named(name: &str) -> Self {
        match name.trim().to_lowercase().as_str() {
            "gruvbox" => Self::gruvbox(),
            "tokyonight" | "tokyo-night" | "tokyo_night" => Self::tokyonight(),
            "nord" => Self::nord(),
            _ => Self::catppuccin(),
        }
    }

    /// Read `[theme]` from herdr's config: the built-in name plus any
    /// `theme.custom.*` overrides, which herdr applies on top of the base.
    pub fn from_herdr_config(body: &str) -> Self {
        // Parse as a document table: `toml::Value`'s FromStr does not accept a
        // whole config, and getting that wrong silently returned the default
        // theme no matter what herdr was set to.
        let parsed: toml::Table = match body.parse() {
            Ok(v) => v,
            Err(_) => return Self::catppuccin(),
        };
        let theme = parsed.get("theme");
        let name = theme
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("catppuccin");
        let mut out = Self::named(name);

        let Some(custom) = theme.and_then(|t| t.get("custom")) else {
            return out;
        };
        let set = |key: &str, field: &mut Color| {
            if let Some(v) = custom.get(key).and_then(|v| v.as_str()) {
                if let Ok(c) = Color::from_str(v) {
                    *field = c;
                }
            }
        };
        set("accent", &mut out.accent);
        set("text", &mut out.text);
        set("subtext0", &mut out.subtext);
        set("overlay1", &mut out.overlay);
        set("surface0", &mut out.surface);
        set("active_row_bg", &mut out.active_row_bg);
        set("panel_bg", &mut out.panel_bg);
        set("green", &mut out.green);
        set("yellow", &mut out.yellow);
        set("red", &mut out.red);
        set("blue", &mut out.blue);
        set("teal", &mut out.teal);
        set("peach", &mut out.peach);
        set("mauve", &mut out.mauve);
        out
    }

    /// Load herdr's theme, falling back to its default if the config cannot be
    /// read. Never fails: a board with odd colours beats a board that will not open.
    pub fn from_herdr() -> Self {
        crate::integrate::config_path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|body| Self::from_herdr_config(&body))
            .unwrap_or_else(Self::catppuccin)
    }

    /// The colour that carries a lane's meaning.
    pub fn lane(&self, column: Column) -> Color {
        match column {
            Column::Backlog => self.overlay,
            Column::Ready => self.blue,
            Column::Running => self.green,
            Column::Waiting => self.yellow,
            Column::Blocked => self.red,
            Column::Review => self.mauve,
            Column::Done => self.teal,
            Column::Failed => self.red,
            Column::Cancelled => self.overlay,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_follows_the_theme_herdr_is_set_to() {
        let gruvbox = Theme::from_herdr_config("[theme]\nname = \"gruvbox\"\n");
        assert_eq!(gruvbox, Theme::gruvbox());
        assert_ne!(gruvbox, Theme::catppuccin());
    }

    #[test]
    fn a_theme_we_do_not_ship_falls_back_to_herdrs_default() {
        assert_eq!(Theme::named("something-new"), Theme::catppuccin());
        assert_eq!(Theme::from_herdr_config(""), Theme::catppuccin());
        assert_eq!(
            Theme::from_herdr_config("not = = toml"),
            Theme::catppuccin()
        );
    }

    #[test]
    fn custom_overrides_are_applied_on_top_of_the_base() {
        // Note the `##`: a hex colour contains `"#`, which would close an `r#"`.
        let body = r##"
[theme]
name = "gruvbox"

[theme.custom]
accent = "#ff0000"
text = "#00ff00"
"##;
        let t = Theme::from_herdr_config(body);
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        assert_eq!(t.text, Color::Rgb(0, 255, 0));
        // Everything the user did not name keeps the base theme's value.
        assert_eq!(t.green, Theme::gruvbox().green);
    }

    #[test]
    fn a_broken_colour_leaves_the_base_value_alone() {
        let body = "[theme]\nname = \"nord\"\n\n[theme.custom]\naccent = \"not a colour\"\n";
        assert_eq!(Theme::from_herdr_config(body).accent, Theme::nord().accent);
    }

    #[test]
    fn every_lane_has_a_colour_in_every_theme() {
        for theme in [
            Theme::catppuccin(),
            Theme::gruvbox(),
            Theme::tokyonight(),
            Theme::nord(),
        ] {
            for c in Column::ALL {
                assert_ne!(theme.lane(c), Color::Reset);
            }
        }
    }
}
