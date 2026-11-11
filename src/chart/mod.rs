mod render;

pub use render::{
    ensure_fonts_registered, export_chart_png, export_chart_svg, render_braille_fallback,
    render_chart_bitmap,
};

use crate::address::col_to_letters;
use crate::config::Palette;
use crate::workbook::{ChartKind, ChartSpec, WorkbookState};
use ratatui::layout::{Rect, Size};
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Debug, Clone)]
pub struct ChartSeries {
    pub name: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct ResolvedChartData {
    pub sheet: u32,
    pub categories: Vec<String>,
    pub source_rows: Vec<i32>,
    pub series: Vec<ChartSeries>,
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    pub y2_min: f64,
    pub y2_max: f64,
    pub use_category_x: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartHoverHit {
    pub sheet: u32,
    pub source_row: i32,
    pub category: String,
    pub series_name: String,
    pub value: f64,
}

pub fn hover_chart_point(
    data: &ResolvedChartData,
    body: Rect,
    mx: u16,
    my: u16,
) -> Option<ChartHoverHit> {
    if body.width < 6 || body.height < 3 || data.series.is_empty() {
        return None;
    }
    let pad_l = 5u16.min(body.width / 4);
    let pad_r = 2u16;
    let plot_w = body.width.saturating_sub(pad_l + pad_r).max(1) as f64;
    let rel_x = mx.saturating_sub(body.x + pad_l) as f64;
    let frac_x = (rel_x / plot_w).clamp(0.0, 1.0);
    let rel_y = my.saturating_sub(body.y) as f64;
    let frac_y = 1.0 - (rel_y / body.height.max(1) as f64).clamp(0.0, 1.0);

    if data.use_category_x && !data.categories.is_empty() {
        let cat_idx = ((frac_x * (data.categories.len().saturating_sub(1)) as f64).round()
            as usize)
            .min(data.categories.len() - 1);
        let target_y = data.y_min + frac_y * (data.y_max - data.y_min);
        let mut best: Option<(&str, f64, f64)> = None;
        for s in &data.series {
            if let Some(&(_, y)) = s
                .points
                .iter()
                .find(|&&(px, _)| (px.round() as usize) == cat_idx)
            {
                let dist = (y - target_y).abs();
                if best.is_none_or(|(_, _, bd)| dist < bd) {
                    best = Some((s.name.as_str(), y, dist));
                }
            }
        }
        let (s_name, val, _) = best.or_else(|| {
            let s = &data.series[0];
            let &(_, y) = s.points.first()?;
            Some((s.name.as_str(), y, 0.0))
        })?;
        let source_row = data.source_rows.get(cat_idx).copied().unwrap_or(1);
        return Some(ChartHoverHit {
            sheet: data.sheet,
            source_row,
            category: data.categories[cat_idx].clone(),
            series_name: s_name.to_string(),
            value: val,
        });
    }

    let target_x = data.x_min + frac_x * (data.x_max - data.x_min);
    let mut best: Option<(&str, usize, f64, f64, f64)> = None;
    for s in &data.series {
        for (idx, &(px, py)) in s.points.iter().enumerate() {
            let dx = (px - target_x).abs();
            if best.is_none_or(|(_, _, _, _, bd)| dx < bd) {
                best = Some((s.name.as_str(), idx, px, py, dx));
            }
        }
    }
    let (s_name, idx, px, py, _) = best?;
    let source_row = data.source_rows.get(idx).copied().unwrap_or(1);
    Some(ChartHoverHit {
        sheet: data.sheet,
        source_row,
        category: format_compact(px),
        series_name: s_name.to_string(),
        value: py,
    })
}

pub fn resolve_chart_data(wb: &WorkbookState, spec: &ChartSpec) -> Option<ResolvedChartData> {
    let (sheet, [r1, c1, raw_r2, raw_c2]) = wb.parse_spec_bounds(spec)?;
    let (used_r, used_c) = wb.sheet_bounds(sheet);
    let r2 = raw_r2.min(used_r.max(r1));
    let c2 = raw_c2.min(used_c.max(c1));
    let data_start_row = if spec.has_header && r2 > r1 {
        r1 + 1
    } else {
        r1
    };
    if data_start_row > r2 {
        return None;
    }

    if spec.kind == ChartKind::Histogram {
        return resolve_histogram_data(wb, sheet, data_start_row, r2, c1, c2);
    }

    let multi_col = c2 > c1;
    // shortcut: downsample sheets > 1,500 visible rows evenly so live terminal chart rasterization stays sub-10ms.
    const MAX_CHART_POINTS: usize = 1_500;
    let visible_rows: Vec<i32> = if !wb.has_any_hidden_rows(sheet) {
        let n = (r2 - data_start_row + 1) as usize;
        if n <= MAX_CHART_POINTS {
            (data_start_row..=r2).collect()
        } else {
            (0..MAX_CHART_POINTS)
                .map(|i| data_start_row + ((i * (n - 1)) / (MAX_CHART_POINTS - 1)) as i32)
                .collect()
        }
    } else {
        let all_visible: Vec<i32> = (data_start_row..=r2)
            .filter(|&r| !wb.is_row_hidden(sheet, r))
            .collect();
        if all_visible.is_empty() {
            return None;
        }
        if all_visible.len() <= MAX_CHART_POINTS {
            all_visible
        } else {
            let n = all_visible.len();
            (0..MAX_CHART_POINTS)
                .map(|i| all_visible[(i * (n - 1)) / (MAX_CHART_POINTS - 1)])
                .collect()
        }
    };

    let first_col_all_numeric = visible_rows.iter().all(|&r| {
        wb.cell_numeric(sheet, r, c1).is_some() || wb.cell_display(sheet, r, c1).trim().is_empty()
    });
    let scatter_xy = multi_col && spec.kind == ChartKind::Scatter && first_col_all_numeric;
    let use_category_x = !scatter_xy;

    let mut categories = Vec::with_capacity(visible_rows.len());
    for (idx, &r) in visible_rows.iter().enumerate() {
        let label = if multi_col && use_category_x {
            let s = wb.cell_display(sheet, r, c1);
            if s.is_empty() {
                format!("{}", idx + 1)
            } else {
                s
            }
        } else {
            format!("{}", idx + 1)
        };
        categories.push(label);
    }

    let series_cols = if multi_col { (c1 + 1)..=c2 } else { c1..=c1 };
    let mut series = Vec::new();
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;

    for col in series_cols {
        let name = if spec.has_header && r2 > r1 {
            let h = wb.cell_display(sheet, r1, col);
            if h.is_empty() {
                format!("Col {}", col_to_letters(col))
            } else {
                h
            }
        } else {
            format!("Col {}", col_to_letters(col))
        };

        let mut points = Vec::new();
        for (idx, &r) in visible_rows.iter().enumerate() {
            let Some(y) = wb.cell_numeric(sheet, r, col) else {
                continue;
            };
            let x = if scatter_xy {
                let Some(xv) = wb.cell_numeric(sheet, r, c1) else {
                    continue;
                };
                xv
            } else {
                idx as f64
            };
            x_min = x_min.min(x);
            x_max = x_max.max(x);
            y_min = y_min.min(y);
            y_max = y_max.max(y);
            points.push((x, y));
        }
        if !points.is_empty() {
            series.push(ChartSeries { name, points });
        }
    }

    if series.is_empty() {
        return None;
    }

    let mut y2_min = 0.0;
    let mut y2_max = 1.0;

    if spec.kind == ChartKind::StackedBar {
        y_min = 0.0;
        y_max = 0.0;
        for idx in 0..categories.len() {
            let mut pos_sum = 0.0;
            let mut neg_sum = 0.0;
            for s in &series {
                if let Some(&(_, y)) = s.points.iter().find(|&&(px, _)| px.round() as usize == idx)
                {
                    if y >= 0.0 {
                        pos_sum += y;
                    } else {
                        neg_sum += y;
                    }
                }
            }
            y_max = y_max.max(pos_sum);
            y_min = y_min.min(neg_sum);
        }
    } else if spec.kind == ChartKind::DualAxis && series.len() >= 2 {
        y_min = series[0]
            .points
            .iter()
            .map(|&(_, y)| y)
            .fold(0.0_f64, f64::min);
        y_max = series[0]
            .points
            .iter()
            .map(|&(_, y)| y)
            .fold(0.0_f64, f64::max);
        let mut s2_lo = f64::INFINITY;
        let mut s2_hi = f64::NEG_INFINITY;
        for s in &series[1..] {
            for &(_, y) in &s.points {
                s2_lo = s2_lo.min(y);
                s2_hi = s2_hi.max(y);
            }
        }
        if s2_lo.is_finite() && s2_hi.is_finite() {
            let pad2 = ((s2_hi - s2_lo).abs() * 0.12).max(1.0);
            y2_min = if s2_lo >= 0.0 {
                (s2_lo - pad2).max(0.0)
            } else {
                s2_lo - pad2
            };
            y2_max = s2_hi + pad2;
        }
    }

    let zero_anchored = matches!(
        spec.kind,
        ChartKind::Bar | ChartKind::StackedBar | ChartKind::DualAxis | ChartKind::Area
    );
    if zero_anchored {
        y_min = y_min.min(0.0);
        y_max = y_max.max(0.0);
    }
    if (x_max - x_min).abs() < f64::EPSILON {
        x_min -= 1.0;
        x_max += 1.0;
    }
    if (y_max - y_min).abs() < f64::EPSILON {
        let pad = y_max.abs().max(1.0) * 0.1;
        y_min -= pad;
        y_max += pad;
    } else {
        let pad = (y_max - y_min) * 0.08;
        if y_min < 0.0 || !zero_anchored {
            y_min -= pad;
        }
        y_max += pad;
    }

    Some(ResolvedChartData {
        sheet,
        categories,
        source_rows: visible_rows,
        series,
        x_min,
        x_max,
        y_min,
        y_max,
        y2_min,
        y2_max,
        use_category_x,
    })
}

fn resolve_histogram_data(
    wb: &WorkbookState,
    sheet: u32,
    data_start_row: i32,
    r2: i32,
    c1: i32,
    c2: i32,
) -> Option<ResolvedChartData> {
    let mut samples: Vec<(f64, i32)> = Vec::new();
    let probe_end = r2.min(data_start_row + 64);
    let cols = if c2 > c1
        && (data_start_row..=probe_end).any(|r| {
            wb.cell_numeric(sheet, r, c1).is_none() && !wb.cell_display(sheet, r, c1).is_empty()
        }) {
        (c1 + 1)..=c2
    } else {
        c1..=c2
    };
    let row_span = (r2 - data_start_row + 1) as usize;
    let stride = (row_span / 8_000).max(1);
    for r in (data_start_row..=r2).step_by(stride) {
        if wb.is_row_hidden(sheet, r) {
            continue;
        }
        for c in cols.clone() {
            if let Some(v) = wb.cell_numeric(sheet, r, c) {
                samples.push((v, r));
            }
        }
    }
    if samples.is_empty() {
        return None;
    }
    let lo = samples
        .iter()
        .map(|(v, _)| *v)
        .fold(f64::INFINITY, f64::min);
    let hi = samples
        .iter()
        .map(|(v, _)| *v)
        .fold(f64::NEG_INFINITY, f64::max);
    let bins = ((samples.len() as f64).sqrt().round() as usize).clamp(4, 14);
    let width = if (hi - lo).abs() < 1e-12 {
        1.0
    } else {
        (hi - lo) / bins as f64
    };
    let mut counts = vec![0usize; bins];
    let mut rep_rows = vec![data_start_row; bins];
    for (v, src_r) in samples {
        let b = if (hi - lo).abs() < 1e-12 {
            0
        } else {
            (((v - lo) / width).floor() as usize).min(bins - 1)
        };
        if counts[b] == 0 {
            rep_rows[b] = src_r;
        }
        counts[b] += 1;
    }
    let categories: Vec<String> = (0..bins)
        .map(|b| {
            let a = lo + b as f64 * width;
            let z = a + width;
            format!("{}..{}", format_compact(a), format_compact(z))
        })
        .collect();
    let points: Vec<(f64, f64)> = counts
        .iter()
        .enumerate()
        .map(|(i, &cnt)| (i as f64, cnt as f64))
        .collect();
    let max_cnt = counts.into_iter().max().unwrap_or(1) as f64;
    Some(ResolvedChartData {
        sheet,
        categories,
        source_rows: rep_rows,
        series: vec![ChartSeries {
            name: "Frequency".to_string(),
            points,
        }],
        x_min: 0.0,
        x_max: (bins.saturating_sub(1)).max(1) as f64,
        y_min: 0.0,
        y_max: (max_cnt * 1.12).max(1.0),
        y2_min: 0.0,
        y2_max: 1.0,
        use_category_x: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartRenderKey {
    pub chart_id: u64,
    pub data_rev: u64,
    pub cols: u16,
    pub rows: u16,
    pub spec: ChartSpec,
    pub dark: bool,
}

pub struct ChartJob {
    pub key: ChartRenderKey,
    pub data: ResolvedChartData,
    pub palette: Palette,
    pub picker: Picker,
}

pub struct ChartReady {
    pub key: ChartRenderKey,
    pub protocol: Protocol,
}

pub struct ChartWorker {
    tx: Sender<ChartJob>,
    rx: Receiver<ChartReady>,
}

impl Default for ChartWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ChartWorker {
    pub fn new() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<ChartJob>();
        let (done_tx, done_rx) = mpsc::channel::<ChartReady>();
        thread::spawn(move || {
            while let Ok(mut job) = job_rx.recv() {
                // Coalesce queued jobs so rapid resizing only rasterizes the newest frame.
                while let Ok(newer) = job_rx.try_recv() {
                    job = newer;
                }
                let fs = job.picker.font_size();
                let pw = (job.key.cols as u32) * (fs.width.max(8) as u32);
                let ph = (job.key.rows as u32) * (fs.height.max(14) as u32);
                if let Ok(img) = render_chart_bitmap(&job.key.spec, &job.data, &job.palette, pw, ph)
                {
                    let sz = Size::new(job.key.cols, job.key.rows);
                    if let Ok(protocol) = job.picker.new_protocol(img, sz, Resize::Fit(None)) {
                        let _ = done_tx.send(ChartReady {
                            key: job.key,
                            protocol,
                        });
                    }
                }
            }
        });
        Self {
            tx: job_tx,
            rx: done_rx,
        }
    }

    pub fn submit(&self, job: ChartJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll_ready(&self) -> Option<ChartReady> {
        let mut latest = None;
        while let Ok(ready) = self.rx.try_recv() {
            latest = Some(ready);
        }
        latest
    }
}

pub(super) fn format_compact(v: f64) -> String {
    let abs = v.abs();
    if abs >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if abs >= 10_000.0 {
        format!("{:.1}k", v / 1_000.0)
    } else if (v - v.round()).abs() < 1e-6 {
        format!("{:.0}", v)
    } else {
        format!("{:.2}", v)
    }
}

pub(super) fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
