//! Colours, resolved once from the user's config.

use std::str::FromStr;

use ratatui::style::Color;

use crate::config::Theme as ThemeConfig;
use crate::model::Column;

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub running: Color,
    pub waiting: Color,
    pub blocked: Color,
    pub done: Color,
    pub failed: Color,
    pub muted: Color,
}

fn color(spec: &str, fallback: Color) -> Color {
    Color::from_str(spec).unwrap_or(fallback)
}

impl Theme {
    pub fn from_config(cfg: &ThemeConfig) -> Self {
        Self {
            accent: color(&cfg.accent, Color::Cyan),
            running: color(&cfg.running, Color::Green),
            waiting: color(&cfg.waiting, Color::Yellow),
            blocked: color(&cfg.blocked, Color::Red),
            done: color(&cfg.done, Color::Green),
            failed: color(&cfg.failed, Color::Red),
            muted: color(&cfg.muted, Color::DarkGray),
        }
    }

    pub fn lane(&self, column: Column) -> Color {
        match column {
            Column::Backlog => self.muted,
            Column::Ready => self.accent,
            Column::Running => self.running,
            Column::Waiting => self.waiting,
            Column::Blocked => self.blocked,
            Column::Review => self.accent,
            Column::Done => self.done,
            Column::Failed => self.failed,
            Column::Cancelled => self.muted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours_from_the_config_are_honoured() {
        let theme = Theme::from_config(&ThemeConfig::default());
        assert_eq!(theme.accent, Color::Rgb(0x83, 0xa5, 0x98));
    }

    #[test]
    fn a_broken_colour_falls_back_instead_of_crashing_the_tui() {
        let cfg = ThemeConfig {
            accent: "not a colour".into(),
            ..ThemeConfig::default()
        };
        assert_eq!(Theme::from_config(&cfg).accent, Color::Cyan);
    }

    #[test]
    fn every_lane_has_a_colour() {
        let theme = Theme::from_config(&ThemeConfig::default());
        for c in Column::ALL {
            let _ = theme.lane(c);
        }
    }
}
