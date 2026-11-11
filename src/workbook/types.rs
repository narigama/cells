use crate::config::DockPosition;
use ironcalc_base::ClipboardData;
use ratatui::layout::Alignment as UiAlign;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const META_SHEET_NAME: &str = "_cells_meta";
pub(super) const META_CHUNK_CHARS: usize = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartKind {
    Line,
    Bar,
    StackedBar,
    DualAxis,
    Scatter,
    Area,
    Histogram,
    Pie,
}

impl ChartKind {
    pub const ALL: [ChartKind; 8] = [
        Self::Line,
        Self::Bar,
        Self::StackedBar,
        Self::DualAxis,
        Self::Scatter,
        Self::Area,
        Self::Histogram,
        Self::Pie,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Line => "Line",
            Self::Bar => "Bar",
            Self::StackedBar => "Stacked Bar",
            Self::DualAxis => "Dual Axis",
            Self::Scatter => "Scatter",
            Self::Area => "Area",
            Self::Histogram => "Histogram",
            Self::Pie => "Pie",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Line => Self::Bar,
            Self::Bar => Self::StackedBar,
            Self::StackedBar => Self::DualAxis,
            Self::DualAxis => Self::Scatter,
            Self::Scatter => Self::Area,
            Self::Area => Self::Histogram,
            Self::Histogram => Self::Pie,
            Self::Pie => Self::Line,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Line => Self::Pie,
            Self::Bar => Self::Line,
            Self::StackedBar => Self::Bar,
            Self::DualAxis => Self::StackedBar,
            Self::Scatter => Self::DualAxis,
            Self::Area => Self::Scatter,
            Self::Histogram => Self::Area,
            Self::Pie => Self::Histogram,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NumericFilterOp {
    #[default]
    None,
    Gt,
    Lt,
    Eq,
    Top10,
}

impl NumericFilterOp {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::Gt => "> Value",
            Self::Lt => "< Value",
            Self::Eq => "= Value",
            Self::Top10 => "Top 10",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Gt,
            Self::Gt => Self::Lt,
            Self::Lt => Self::Eq,
            Self::Eq => Self::Top10,
            Self::Top10 => Self::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ColumnFilter {
    #[serde(default)]
    pub substring: String,
    #[serde(default)]
    pub numeric_op: NumericFilterOp,
    #[serde(default)]
    pub numeric_val: f64,
    #[serde(default)]
    pub excluded_values: BTreeSet<String>,
}

impl ColumnFilter {
    pub fn is_active(&self) -> bool {
        !self.substring.trim().is_empty()
            || self.numeric_op != NumericFilterOp::None
            || !self.excluded_values.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoFilterState {
    pub sheet_name: String,
    pub range: [i32; 4],
    #[serde(default)]
    pub col_filters: BTreeMap<i32, ColumnFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VisualRule {
    DataBar {
        sheet_name: String,
        range: [i32; 4],
        color_hex: String,
    },
    ColorScale {
        sheet_name: String,
        range: [i32; 4],
        low_hex: String,
        high_hex: String,
    },
    Sparkline {
        sheet_name: String,
        target_row: i32,
        target_col: i32,
        source_range: [i32; 4],
    },
}

impl VisualRule {
    pub fn sheet_name_mut(&mut self) -> &mut String {
        match self {
            Self::DataBar { sheet_name, .. }
            | Self::ColorScale { sheet_name, .. }
            | Self::Sparkline { sheet_name, .. } => sheet_name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PivotAgg {
    Sum,
    Avg,
    Count,
    Min,
    Max,
}

impl PivotAgg {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sum => "SUM",
            Self::Avg => "AVG",
            Self::Count => "COUNT",
            Self::Min => "MIN",
            Self::Max => "MAX",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Sum => Self::Avg,
            Self::Avg => Self::Count,
            Self::Count => Self::Min,
            Self::Min => Self::Max,
            Self::Max => Self::Sum,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub col: i32,
    pub ascending: bool,
}

#[derive(Debug, Clone)]
pub struct AuxSnapshot {
    pub charts: Vec<ChartSpec>,
    pub next_chart_id: u64,
    pub visual_rules: Vec<VisualRule>,
    pub autofilters: Vec<AutoFilterState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChartSpec {
    pub id: u64,
    pub title: String,
    pub kind: ChartKind,
    pub sheet_name: String,
    pub range: String,
    #[serde(default = "default_true")]
    pub has_header: bool,
    #[serde(default = "default_true")]
    pub show_legend: bool,
    #[serde(default = "default_true")]
    pub show_grid: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmbeddedMeta {
    pub charts: Vec<ChartSpec>,
    pub dock_visible: bool,
    pub dock_position: DockPosition,
    pub dock_percent: u16,
    #[serde(default)]
    pub visual_rules: Vec<VisualRule>,
    #[serde(default)]
    pub autofilters: Vec<AutoFilterState>,
}

#[derive(Deserialize)]
pub(super) struct ClipboardMirror {
    pub csv: String,
    pub data: ClipboardData,
    pub sheet: u32,
    pub range: (i32, i32, i32, i32),
}

pub(super) struct InternalClip {
    pub tsv: String,
    pub data: ClipboardData,
    pub sheet: u32,
    pub range: (i32, i32, i32, i32),
    pub is_cut: bool,
    pub synced_to_os: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CellRenderStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub align: UiAlign,
    pub is_error: bool,
    pub is_formula: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SelectionStats {
    pub non_empty: usize,
    pub numeric_count: usize,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

impl SelectionStats {
    pub fn avg(&self) -> Option<f64> {
        (self.numeric_count > 0).then(|| self.sum / self.numeric_count as f64)
    }
}
