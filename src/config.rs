use anyhow::{Context, Result};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DockPosition {
    #[default]
    Right,
    Bottom,
}

impl DockPosition {
    pub fn toggle(self) -> Self {
        match self {
            Self::Right => Self::Bottom,
            Self::Bottom => Self::Right,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeMode,
    pub dock_position: DockPosition,
    pub dock_percent: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            dock_position: DockPosition::Right,
            dock_percent: 42,
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("cells").join("config.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut cfg: Self = toml::from_str(&raw).unwrap_or_default();
        cfg.dock_percent = cfg.dock_percent.clamp(20, 80);
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let encoded = toml::to_string_pretty(self).context("failed to serialize config")?;
        fs::write(&path, encoded).with_context(|| format!("failed to write {}", path.display()))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub grid_bg: Color,
    pub grid_line: Color,
    pub frozen_line: Color,
    pub header_bg: Color,
    pub header_fg: Color,
    pub header_active_bg: Color,
    pub header_active_fg: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub selection_bg: Color,
    pub cursor_bg: Color,
    pub cursor_fg: Color,
    pub fill_preview: Color,
    pub error_fg: Color,
    pub formula_fg: Color,
    pub menu_bg: Color,
    pub menu_fg: Color,
    pub menu_hi_bg: Color,
    pub menu_hi_fg: Color,
    pub border: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub match_bg: Color,
    pub chart_bg: [u8; 3],
    pub chart_plot_bg: [u8; 3],
    pub chart_fg: [u8; 3],
    pub chart_grid: [u8; 3],
    pub series: [[u8; 3]; 8],
}

impl Palette {
    pub fn for_theme(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self {
                bg: Color::Rgb(17, 19, 26),
                surface: Color::Rgb(24, 27, 36),
                grid_bg: Color::Rgb(20, 23, 31),
                grid_line: Color::Rgb(42, 47, 62),
                frozen_line: Color::Rgb(110, 155, 255),
                header_bg: Color::Rgb(28, 32, 44),
                header_fg: Color::Rgb(156, 166, 190),
                header_active_bg: Color::Rgb(54, 79, 138),
                header_active_fg: Color::Rgb(240, 245, 255),
                text: Color::Rgb(226, 232, 244),
                muted: Color::Rgb(125, 136, 161),
                accent: Color::Rgb(96, 165, 250),
                selection_bg: Color::Rgb(36, 54, 92),
                cursor_bg: Color::Rgb(79, 134, 247),
                cursor_fg: Color::Rgb(12, 15, 22),
                fill_preview: Color::Rgb(52, 211, 153),
                error_fg: Color::Rgb(248, 113, 113),
                formula_fg: Color::Rgb(192, 132, 252),
                menu_bg: Color::Rgb(28, 32, 45),
                menu_fg: Color::Rgb(226, 232, 244),
                menu_hi_bg: Color::Rgb(69, 115, 212),
                menu_hi_fg: Color::Rgb(255, 255, 255),
                border: Color::Rgb(64, 73, 96),
                status_bg: Color::Rgb(25, 29, 40),
                status_fg: Color::Rgb(195, 205, 226),
                match_bg: Color::Rgb(113, 63, 18),
                chart_bg: [20, 23, 31],
                chart_plot_bg: [26, 30, 41],
                chart_fg: [220, 227, 241],
                chart_grid: [50, 57, 76],
                series: [
                    [96, 165, 250],  // blue
                    [52, 211, 153],  // emerald
                    [251, 191, 36],  // amber
                    [244, 114, 182], // pink
                    [167, 139, 250], // violet
                    [45, 212, 191],  // teal
                    [251, 146, 60],  // orange
                    [248, 113, 113], // coral
                ],
            },
            ThemeMode::Light => Self {
                bg: Color::Rgb(246, 248, 252),
                surface: Color::Rgb(255, 255, 255),
                grid_bg: Color::Rgb(255, 255, 255),
                grid_line: Color::Rgb(218, 224, 235),
                frozen_line: Color::Rgb(37, 99, 235),
                header_bg: Color::Rgb(236, 240, 247),
                header_fg: Color::Rgb(71, 85, 105),
                header_active_bg: Color::Rgb(206, 223, 252),
                header_active_fg: Color::Rgb(15, 23, 42),
                text: Color::Rgb(15, 23, 42),
                muted: Color::Rgb(100, 116, 139),
                accent: Color::Rgb(37, 99, 235),
                selection_bg: Color::Rgb(222, 235, 255),
                cursor_bg: Color::Rgb(37, 99, 235),
                cursor_fg: Color::Rgb(255, 255, 255),
                fill_preview: Color::Rgb(5, 150, 105),
                error_fg: Color::Rgb(220, 38, 38),
                formula_fg: Color::Rgb(124, 58, 237),
                menu_bg: Color::Rgb(241, 245, 249),
                menu_fg: Color::Rgb(15, 23, 42),
                menu_hi_bg: Color::Rgb(37, 99, 235),
                menu_hi_fg: Color::Rgb(255, 255, 255),
                border: Color::Rgb(180, 192, 210),
                status_bg: Color::Rgb(232, 238, 247),
                status_fg: Color::Rgb(30, 41, 59),
                match_bg: Color::Rgb(254, 240, 138),
                chart_bg: [255, 255, 255],
                chart_plot_bg: [248, 250, 252],
                chart_fg: [23, 32, 48],
                chart_grid: [216, 224, 236],
                series: [
                    [37, 99, 235],  // blue
                    [5, 150, 105],  // emerald
                    [217, 119, 6],  // amber
                    [219, 39, 119], // pink
                    [124, 58, 237], // violet
                    [13, 148, 136], // teal
                    [234, 88, 12],  // orange
                    [220, 38, 38],  // crimson
                ],
            },
        }
    }

    pub fn series_color(&self, idx: usize) -> Color {
        let [r, g, b] = self.series[idx % self.series.len()];
        Color::Rgb(r, g, b)
    }
}

pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let s = hex.trim().trim_start_matches('#');
    let s = match s.len() {
        8 => &s[..6],
        6 => s,
        _ => return None,
    };
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

pub const SWATCH_COLORS: &[(&str, &str)] = &[
    ("None / Clear", ""),
    ("Blue", "#3B82F6"),
    ("Green", "#10B981"),
    ("Amber", "#F59E0B"),
    ("Red", "#EF4444"),
    ("Violet", "#8B5CF6"),
    ("Slate", "#334155"),
    ("Soft Yellow", "#FEF3C7"),
    ("Soft Green", "#D1FAE5"),
    ("Soft Blue", "#DBEAFE"),
    ("Soft Rose", "#FFE4E6"),
    ("White", "#FFFFFF"),
];
