use super::{ResolvedChartData, format_compact, truncate_str};
use crate::config::Palette;
use crate::workbook::{ChartKind, ChartSpec};
use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, RgbImage};
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::FontStyle;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Bar, BarChart, BarGroup, Block, Chart, Dataset, GraphType, Paragraph,
};
use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static FONT_READY: OnceLock<bool> = OnceLock::new();

pub fn ensure_fonts_registered() -> bool {
    *FONT_READY.get_or_init(|| {
        let Some(font_path) = find_system_ttf() else {
            return false;
        };
        let Ok(bytes) = fs::read(&font_path) else {
            return false;
        };
        let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let ok_sans =
            plotters::style::register_font("sans-serif", FontStyle::Normal, leaked).is_ok();
        let _ = plotters::style::register_font("sans-serif", FontStyle::Bold, leaked);
        let _ = plotters::style::register_font("monospace", FontStyle::Normal, leaked);
        ok_sans
    })
}

fn find_system_ttf() -> Option<PathBuf> {
    let preferred = [
        "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/Library/Fonts/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
    ];
    for p in preferred {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }

    let mut roots = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Some(d) = dirs::font_dir() {
        roots.push(d);
    }
    for root in roots {
        if let Some(found) = walk_for_ttf(&root, 0) {
            return Some(found);
        }
    }
    None
}

fn walk_for_ttf(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 4 || !dir.is_dir() {
        return None;
    }
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            if let Some(ext) = p.extension().and_then(|s| s.to_str())
                && matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf")
            {
                return Some(p);
            }
        } else if p.is_dir()
            && let Some(found) = walk_for_ttf(&p, depth + 1)
        {
            return Some(found);
        }
    }
    None
}

pub fn render_chart_bitmap(
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
    px_width: u32,
    px_height: u32,
) -> Result<DynamicImage> {
    let w = px_width.clamp(120, 3840);
    let h = px_height.clamp(80, 2160);
    let mut buf = vec![0u8; (w * h * 3) as usize];
    {
        let root = BitMapBackend::with_buffer(&mut buf, (w, h)).into_drawing_area();
        draw_chart_on_area(&root, spec, data, palette)
            .map_err(|e| anyhow!("chart draw error: {e}"))?;
        root.present()
            .map_err(|e| anyhow!("chart present error: {e}"))?;
    }
    let rgb = RgbImage::from_raw(w, h, buf).ok_or_else(|| anyhow!("invalid RGB buffer size"))?;
    Ok(DynamicImage::ImageRgb8(rgb))
}

pub fn export_chart_png(
    path: &Path,
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
) -> Result<()> {
    let img = render_chart_bitmap(spec, data, palette, 1280, 800)?;
    img.save(path)
        .with_context(|| format!("failed to save PNG to {}", path.display()))
}

pub fn export_chart_svg(
    path: &Path,
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
) -> Result<()> {
    let mut svg = String::new();
    {
        let root = SVGBackend::with_string(&mut svg, (1280, 800)).into_drawing_area();
        draw_chart_on_area(&root, spec, data, palette)
            .map_err(|e| anyhow!("SVG chart draw error: {e}"))?;
        root.present()
            .map_err(|e| anyhow!("SVG present error: {e}"))?;
    }
    fs::write(path, svg).with_context(|| format!("failed to write SVG to {}", path.display()))
}

fn draw_chart_on_area<DB: DrawingBackend>(
    root: &DrawingArea<DB, Shift>,
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
) -> std::result::Result<(), String> {
    let has_font = ensure_fonts_registered();
    let bg = RGBColor(
        palette.chart_bg[0],
        palette.chart_bg[1],
        palette.chart_bg[2],
    );
    let plot_bg = RGBColor(
        palette.chart_plot_bg[0],
        palette.chart_plot_bg[1],
        palette.chart_plot_bg[2],
    );
    let fg = RGBColor(
        palette.chart_fg[0],
        palette.chart_fg[1],
        palette.chart_fg[2],
    );
    let grid_c = RGBColor(
        palette.chart_grid[0],
        palette.chart_grid[1],
        palette.chart_grid[2],
    );

    root.fill(&bg).map_err(|e| format!("{e:?}"))?;

    if spec.kind == ChartKind::Pie {
        return draw_pie_chart(root, spec, data, palette, has_font, fg);
    }

    let (w, h) = root.dim_in_pixel();
    let title_sz = (h as f64 * 0.055).clamp(13.0, 26.0) as u32;
    let label_sz = (h as f64 * 0.036).clamp(10.0, 16.0) as u32;
    let margin = (w.min(h) as f64 * 0.035).clamp(8.0, 24.0) as u32;

    let mut builder = ChartBuilder::on(root);
    builder.margin(margin);
    if has_font && !spec.title.is_empty() {
        builder.caption(&spec.title, ("sans-serif", title_sz).into_font().color(&fg));
    }
    if has_font {
        builder
            .x_label_area_size((label_sz * 2 + 8).min(44))
            .y_label_area_size((label_sz * 4 + 8).min(68));
        if spec.kind == ChartKind::DualAxis && data.series.len() >= 2 {
            builder.right_y_label_area_size((label_sz * 4 + 8).min(68));
        }
    }

    let is_bar_like = matches!(
        spec.kind,
        ChartKind::Bar | ChartKind::StackedBar | ChartKind::DualAxis | ChartKind::Histogram
    );
    let x_start = if is_bar_like { -0.6 } else { data.x_min };
    let x_end = if is_bar_like {
        (data.categories.len().saturating_sub(1) as f64) + 0.6
    } else {
        data.x_max
    };

    let base_chart = builder
        .build_cartesian_2d(x_start..x_end, data.y_min..data.y_max)
        .map_err(|e| format!("{e:?}"))?;

    if spec.kind == ChartKind::DualAxis && data.series.len() >= 2 {
        let mut chart = base_chart.set_secondary_coord(x_start..x_end, data.y2_min..data.y2_max);
        chart
            .plotting_area()
            .fill(&plot_bg)
            .map_err(|e| format!("{e:?}"))?;
        let cats = &data.categories;
        let x_fmt = |v: &f64| -> String {
            let idx = v.round() as isize;
            if (v - idx as f64).abs() < 0.25 && idx >= 0 && (idx as usize) < cats.len() {
                truncate_str(&cats[idx as usize], 12)
            } else {
                String::new()
            }
        };
        let y_fmt = |v: &f64| format_compact(*v);
        let mut mesh = chart.configure_mesh();
        mesh.axis_style(grid_c.stroke_width(1));
        if !spec.show_grid {
            mesh.disable_mesh();
        } else {
            mesh.bold_line_style(grid_c.stroke_width(1))
                .light_line_style(grid_c.mix(0.35));
        }
        if has_font {
            mesh.label_style(("sans-serif", label_sz).into_font().color(&fg))
                .x_labels(cats.len().clamp(2, 10))
                .y_labels(6)
                .x_label_formatter(&x_fmt)
                .y_label_formatter(&y_fmt);
        } else {
            mesh.x_labels(0).y_labels(0);
        }
        mesh.draw().map_err(|e| format!("{e:?}"))?;

        if has_font {
            chart
                .configure_secondary_axes()
                .axis_style(grid_c.stroke_width(1))
                .label_style(("sans-serif", label_sz).into_font().color(&fg))
                .y_labels(6)
                .y_label_formatter(&y_fmt)
                .draw()
                .map_err(|e| format!("{e:?}"))?;
        }

        let s0 = &data.series[0];
        let [r0, g0, b0] = palette.series[0];
        let c0 = RGBColor(r0, g0, b0);
        let bars = s0.points.iter().map(|&(x, y)| {
            let (y0, y1) = if y >= 0.0 { (0.0, y) } else { (y, 0.0) };
            Rectangle::new([(x - 0.32, y0), (x + 0.32, y1)], c0.mix(0.85).filled())
        });
        let anno0 = chart.draw_series(bars).map_err(|e| format!("{e:?}"))?;
        if has_font && spec.show_legend {
            anno0
                .label(format!("{} (L)", s0.name))
                .legend(move |(x, y)| Rectangle::new([(x, y - 5), (x + 14, y + 5)], c0.filled()));
        }

        for (s_idx, s) in data.series[1..].iter().enumerate() {
            let [r, g, b] = palette.series[(s_idx + 1) % palette.series.len()];
            let color = RGBColor(r, g, b);
            let anno = chart
                .draw_secondary_series(LineSeries::new(
                    s.points.iter().copied(),
                    color.stroke_width(2),
                ))
                .map_err(|e| format!("{e:?}"))?;
            if has_font && spec.show_legend {
                anno.label(format!("{} (R)", s.name)).legend(move |(x, y)| {
                    PathElement::new(vec![(x, y), (x + 16, y)], color.stroke_width(3))
                });
            }
            chart
                .draw_secondary_series(
                    s.points
                        .iter()
                        .map(|&(x, y)| Circle::new((x, y), 3, color.filled())),
                )
                .map_err(|e| format!("{e:?}"))?;
        }

        if has_font && spec.show_legend {
            chart
                .configure_series_labels()
                .background_style(bg.mix(0.82))
                .border_style(grid_c)
                .label_font(("sans-serif", label_sz).into_font().color(&fg))
                .draw()
                .map_err(|e| format!("{e:?}"))?;
        }
        return Ok(());
    }

    let mut chart = base_chart;
    chart
        .plotting_area()
        .fill(&plot_bg)
        .map_err(|e| format!("{e:?}"))?;

    let cats = &data.categories;
    let x_fmt = |v: &f64| -> String {
        if data.use_category_x {
            let idx = v.round() as isize;
            if (v - idx as f64).abs() < 0.25 && idx >= 0 && (idx as usize) < cats.len() {
                truncate_str(&cats[idx as usize], 12)
            } else {
                String::new()
            }
        } else {
            format_compact(*v)
        }
    };
    let y_fmt = |v: &f64| format_compact(*v);

    let mut mesh = chart.configure_mesh();
    mesh.axis_style(grid_c.stroke_width(1));
    if !spec.show_grid {
        mesh.disable_mesh();
    } else {
        mesh.bold_line_style(grid_c.stroke_width(1))
            .light_line_style(grid_c.mix(0.35));
    }
    if has_font {
        mesh.label_style(("sans-serif", label_sz).into_font().color(&fg))
            .x_labels(cats.len().clamp(2, 10))
            .y_labels(7)
            .x_label_formatter(&x_fmt)
            .y_label_formatter(&y_fmt);
    } else {
        mesh.x_labels(0).y_labels(0);
    }
    mesh.draw().map_err(|e| format!("{e:?}"))?;

    let n_series = data.series.len().max(1);
    let mut stack_pos = vec![0.0_f64; data.categories.len().max(1)];
    let mut stack_neg = vec![0.0_f64; data.categories.len().max(1)];

    for (s_idx, s) in data.series.iter().enumerate() {
        let [r, g, b] = palette.series[s_idx % palette.series.len()];
        let color = RGBColor(r, g, b);
        match spec.kind {
            ChartKind::Line => {
                let anno = chart
                    .draw_series(LineSeries::new(
                        s.points.iter().copied(),
                        color.stroke_width(2),
                    ))
                    .map_err(|e| format!("{e:?}"))?;
                if has_font && spec.show_legend {
                    anno.label(s.name.clone()).legend(move |(x, y)| {
                        PathElement::new(vec![(x, y), (x + 16, y)], color.stroke_width(3))
                    });
                }
                if s.points.len() <= 40 {
                    chart
                        .draw_series(
                            s.points
                                .iter()
                                .map(|&(x, y)| Circle::new((x, y), 3, color.filled())),
                        )
                        .map_err(|e| format!("{e:?}"))?;
                }
            }
            ChartKind::Area => {
                let anno = chart
                    .draw_series(
                        AreaSeries::new(s.points.iter().copied(), 0.0, color.mix(0.28))
                            .border_style(color.stroke_width(2)),
                    )
                    .map_err(|e| format!("{e:?}"))?;
                if has_font && spec.show_legend {
                    anno.label(s.name.clone()).legend(move |(x, y)| {
                        Rectangle::new([(x, y - 5), (x + 14, y + 5)], color.filled())
                    });
                }
            }
            ChartKind::Scatter => {
                let anno = chart
                    .draw_series(
                        s.points
                            .iter()
                            .map(|&(x, y)| Circle::new((x, y), 4, color.mix(0.85).filled())),
                    )
                    .map_err(|e| format!("{e:?}"))?;
                if has_font && spec.show_legend {
                    anno.label(s.name.clone())
                        .legend(move |(x, y)| Circle::new((x + 7, y), 4, color.filled()));
                }
            }
            ChartKind::StackedBar => {
                let mut rects = Vec::with_capacity(s.points.len());
                for &(x, y) in &s.points {
                    let idx = (x.round().max(0.0) as usize).min(stack_pos.len() - 1);
                    let (y0, y1) = if y >= 0.0 {
                        let base = stack_pos[idx];
                        stack_pos[idx] += y;
                        (base, base + y)
                    } else {
                        let base = stack_neg[idx];
                        stack_neg[idx] += y;
                        (base + y, base)
                    };
                    rects.push(Rectangle::new(
                        [(x - 0.34, y0), (x + 0.34, y1)],
                        color.filled(),
                    ));
                }
                let anno = chart.draw_series(rects).map_err(|e| format!("{e:?}"))?;
                if has_font && spec.show_legend {
                    anno.label(s.name.clone()).legend(move |(x, y)| {
                        Rectangle::new([(x, y - 5), (x + 14, y + 5)], color.filled())
                    });
                }
            }
            ChartKind::Bar | ChartKind::DualAxis | ChartKind::Histogram => {
                let group_width = if spec.kind == ChartKind::Histogram {
                    0.92
                } else {
                    0.74
                };
                let bar_w = group_width / n_series as f64;
                let offset = -group_width / 2.0 + s_idx as f64 * bar_w;
                let bars = s.points.iter().map(|&(x, y)| {
                    let x0 = x + offset + bar_w * 0.04;
                    let x1 = x + offset + bar_w * 0.96;
                    let (y0, y1) = if y >= 0.0 { (0.0, y) } else { (y, 0.0) };
                    Rectangle::new([(x0, y0), (x1, y1)], color.filled())
                });
                let anno = chart.draw_series(bars).map_err(|e| format!("{e:?}"))?;
                if has_font && spec.show_legend {
                    anno.label(s.name.clone()).legend(move |(x, y)| {
                        Rectangle::new([(x, y - 5), (x + 14, y + 5)], color.filled())
                    });
                }
            }
            ChartKind::Pie => {}
        }
    }

    if has_font && spec.show_legend && (data.series.len() > 1 || !spec.title.is_empty()) {
        chart
            .configure_series_labels()
            .background_style(bg.mix(0.82))
            .border_style(grid_c)
            .label_font(("sans-serif", label_sz).into_font().color(&fg))
            .draw()
            .map_err(|e| format!("{e:?}"))?;
    }
    Ok(())
}

fn draw_pie_chart<DB: DrawingBackend>(
    root: &DrawingArea<DB, Shift>,
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
    has_font: bool,
    fg: RGBColor,
) -> std::result::Result<(), String> {
    let (w, h) = root.dim_in_pixel();
    let title_sz = (h as f64 * 0.055).clamp(13.0, 26.0) as u32;
    let label_sz = (h as f64 * 0.036).clamp(10.0, 16.0) as u32;

    if has_font && !spec.title.is_empty() {
        root.draw(&Text::new(
            spec.title.clone(),
            (16, 12),
            ("sans-serif", title_sz).into_font().color(&fg),
        ))
        .map_err(|e| format!("{e:?}"))?;
    }

    let first = &data.series[0];
    let slices: Vec<(String, f64)> = first
        .points
        .iter()
        .filter(|&&(_, v)| v > 0.0)
        .map(|&(x, v)| {
            let idx = x.round() as usize;
            let name = data
                .categories
                .get(idx)
                .cloned()
                .unwrap_or_else(|| format!("#{}", idx + 1));
            (name, v)
        })
        .collect();

    let total: f64 = slices.iter().map(|(_, v)| *v).sum();
    if total <= 0.0 {
        return Ok(());
    }

    let cx = if spec.show_legend {
        (w as f64 * 0.36) as i32
    } else {
        (w as f64 * 0.5) as i32
    };
    let cy = (h as f64 * 0.54) as i32;
    let radius = (w.min(h) as f64 * 0.34).max(20.0);

    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (i, (label, val)) in slices.iter().enumerate() {
        let frac = *val / total;
        let sweep = frac * TAU;
        let steps = ((sweep * 24.0).ceil() as usize).clamp(4, 96);
        let mut pts = Vec::with_capacity(steps + 2);
        pts.push((cx, cy));
        for step in 0..=steps {
            let a = angle + sweep * (step as f64 / steps as f64);
            let px = cx + (radius * a.cos()).round() as i32;
            let py = cy + (radius * a.sin()).round() as i32;
            pts.push((px, py));
        }
        let [r, g, b] = palette.series[i % palette.series.len()];
        let c = RGBColor(r, g, b);
        root.draw(&Polygon::new(pts.clone(), c.filled()))
            .map_err(|e| format!("{e:?}"))?;
        let bg = RGBColor(
            palette.chart_bg[0],
            palette.chart_bg[1],
            palette.chart_bg[2],
        );
        root.draw(&PathElement::new(pts, bg.stroke_width(2)))
            .map_err(|e| format!("{e:?}"))?;

        if has_font && spec.show_legend {
            let lx = (w as f64 * 0.66) as i32;
            let ly = 44 + (i as i32) * (label_sz as i32 + 10);
            if ly + (label_sz as i32) < h as i32 - 8 {
                root.draw(&Rectangle::new([(lx, ly), (lx + 12, ly + 12)], c.filled()))
                    .map_err(|e| format!("{e:?}"))?;
                let txt = format!("{} ({:.1}%)", truncate_str(label, 16), frac * 100.0);
                root.draw(&Text::new(
                    txt,
                    (lx + 18, ly - 1),
                    ("sans-serif", label_sz).into_font().color(&fg),
                ))
                .map_err(|e| format!("{e:?}"))?;
            }
        }
        angle += sweep;
    }
    Ok(())
}

/// Crisp native Braille/block fallback chart renderer for text-only terminals.
pub fn render_braille_fallback(
    f: &mut Frame<'_>,
    area: Rect,
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
) {
    if area.width < 8 || area.height < 4 {
        return;
    }
    match spec.kind {
        ChartKind::Bar | ChartKind::StackedBar | ChartKind::Histogram => {
            render_native_bar(f, area, spec, data, palette)
        }
        ChartKind::Pie => render_native_pie(f, area, spec, data, palette),
        ChartKind::Line | ChartKind::Area | ChartKind::Scatter | ChartKind::DualAxis => {
            let marker = if spec.kind == ChartKind::Scatter {
                Marker::Dot
            } else {
                Marker::Braille
            };
            let graph_type = if spec.kind == ChartKind::Scatter {
                GraphType::Scatter
            } else {
                GraphType::Line
            };
            let scaled_points: Vec<Vec<(f64, f64)>> = data
                .series
                .iter()
                .enumerate()
                .map(|(idx, s)| {
                    if spec.kind == ChartKind::DualAxis
                        && idx > 0
                        && (data.y2_max - data.y2_min).abs() > 1e-12
                    {
                        s.points
                            .iter()
                            .map(|&(x, y)| {
                                let t = (y - data.y2_min) / (data.y2_max - data.y2_min);
                                (x, data.y_min + t * (data.y_max - data.y_min))
                            })
                            .collect()
                    } else {
                        s.points.clone()
                    }
                })
                .collect();
            let datasets: Vec<Dataset<'_>> = data
                .series
                .iter()
                .enumerate()
                .map(|(idx, s)| {
                    Dataset::default()
                        .name(s.name.as_str())
                        .marker(marker)
                        .graph_type(graph_type)
                        .style(Style::default().fg(palette.series_color(idx)))
                        .data(&scaled_points[idx])
                })
                .collect();

            let x_labels = if data.use_category_x && !data.categories.is_empty() {
                let first = data.categories.first().cloned().unwrap_or_default();
                let mid = data
                    .categories
                    .get(data.categories.len() / 2)
                    .cloned()
                    .unwrap_or_default();
                let last = data.categories.last().cloned().unwrap_or_default();
                vec![Span::raw(first), Span::raw(mid), Span::raw(last)]
            } else {
                vec![
                    Span::raw(format_compact(data.x_min)),
                    Span::raw(format_compact((data.x_min + data.x_max) * 0.5)),
                    Span::raw(format_compact(data.x_max)),
                ]
            };

            let y_labels = vec![
                Span::raw(format_compact(data.y_min)),
                Span::raw(format_compact((data.y_min + data.y_max) * 0.5)),
                Span::raw(format_compact(data.y_max)),
            ];

            let mut chart = Chart::new(datasets)
                .style(Style::default().bg(palette.grid_bg).fg(palette.text))
                .x_axis(
                    Axis::default()
                        .style(Style::default().fg(palette.muted))
                        .bounds([data.x_min, data.x_max])
                        .labels(x_labels),
                )
                .y_axis(
                    Axis::default()
                        .style(Style::default().fg(palette.muted))
                        .bounds([data.y_min, data.y_max])
                        .labels(y_labels),
                );
            if !spec.title.is_empty() {
                chart = chart.block(
                    Block::new()
                        .title(Span::styled(
                            format!(" {} ", spec.title),
                            Style::default()
                                .fg(palette.text)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .style(Style::default().bg(palette.grid_bg)),
                );
            }
            f.render_widget(chart, area);
        }
    }
}

fn render_native_bar(
    f: &mut Frame<'_>,
    area: Rect,
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
) {
    let pts: Vec<(f64, f64)> = if spec.kind == ChartKind::StackedBar {
        (0..data.categories.len())
            .map(|idx| {
                let sum: f64 = data
                    .series
                    .iter()
                    .filter_map(|s| s.points.iter().find(|&&(x, _)| x.round() as usize == idx))
                    .map(|&(_, y)| y.max(0.0))
                    .sum();
                (idx as f64, sum)
            })
            .collect()
    } else {
        data.series[0].points.clone()
    };
    let scale = pts.iter().map(|(_, y)| y.abs()).fold(0.0_f64, f64::max);
    let factor = if scale > 0.0 && scale < 10.0 {
        100.0
    } else {
        1.0
    };
    let bars: Vec<Bar<'_>> = pts
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let lbl = data
                .categories
                .get(x.round() as usize)
                .map(|s| truncate_str(s, 8))
                .unwrap_or_else(|| format!("{}", i + 1));
            Bar::default()
                .label(Line::from(lbl))
                .value((y.max(0.0) * factor).round() as u64)
                .text_value(format_compact(y))
                .style(Style::default().fg(palette.series_color(i)))
        })
        .collect();

    let bar_w = ((area.width as usize / bars.len().max(1)).saturating_sub(1)).clamp(3, 10) as u16;
    let bc = BarChart::default()
        .block(
            Block::new()
                .title(format!(" {} ", spec.title))
                .style(Style::default().bg(palette.grid_bg).fg(palette.text)),
        )
        .bar_width(bar_w)
        .bar_gap(1)
        .data(BarGroup::default().bars(&bars));
    f.render_widget(bc, area);
}

fn render_native_pie(
    f: &mut Frame<'_>,
    area: Rect,
    spec: &ChartSpec,
    data: &ResolvedChartData,
    palette: &Palette,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", spec.title),
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(palette.grid_bg)),
        chunks[0],
    );

    let first = &data.series[0];
    let total: f64 = first.points.iter().map(|(_, y)| y.max(0.0)).sum();
    if total <= 0.0 {
        return;
    }
    let bar_max = chunks[1].width.saturating_sub(28).max(6) as f64;
    let mut lines = Vec::new();
    for (i, &(x, y)) in first.points.iter().filter(|&&(_, y)| y > 0.0).enumerate() {
        let pct = y / total;
        let filled = (pct * bar_max).round() as usize;
        let lbl = data
            .categories
            .get(x.round() as usize)
            .map(|s| truncate_str(s, 12))
            .unwrap_or_else(|| format!("#{}", i + 1));
        lines.push(Line::from(vec![
            Span::styled(format!(" {lbl:<12} "), Style::default().fg(palette.text)),
            Span::styled(
                "█".repeat(filled.max(1)),
                Style::default().fg(palette.series_color(i)),
            ),
            Span::styled(
                format!(" {:>5.1}% ({})", pct * 100.0, format_compact(y)),
                Style::default().fg(palette.muted),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(palette.grid_bg)),
        chunks[1],
    );
}
