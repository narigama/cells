use super::{align_cell_text, blend_rgb, center_pad, fill_rect};
use crate::address::{MAX_COLS, MAX_ROWS, col_to_letters};
use crate::app::{App, ColHit, DragState, RowHit};
use crate::config::{Palette, parse_hex_color};
use crate::workbook::VisualRule;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::collections::BTreeSet;

struct GridOverlays {
    autofilter_info: Option<(i32, i32, i32, BTreeSet<i32>)>,
    data_bars: Vec<([i32; 4], Color, f64, f64)>,
    color_scales: Vec<([i32; 4], Color, Color, f64, f64)>,
    sparklines: Vec<(i32, i32, [i32; 4])>,
    precedents_on_sheet: Vec<([i32; 4], Color)>,
    hover_row: Option<i32>,
    fill_preview: Option<[i32; 4]>,
    move_preview: Option<[i32; 4]>,
}

impl GridOverlays {
    fn collect(app: &App, sheet: u32, sheet_name: &str, p: &Palette) -> Self {
        let autofilter_info = app.wb.active_autofilter(sheet_name).map(|af| {
            let active_cols = af
                .col_filters
                .iter()
                .filter(|(_, cf)| cf.is_active())
                .map(|(&col, _)| col)
                .collect();
            (af.range[0], af.range[1], af.range[3], active_cols)
        });

        let mut data_bars = Vec::new();
        let mut color_scales = Vec::new();
        let mut sparklines = Vec::new();
        for rule in &app.wb.visual_rules {
            match rule {
                VisualRule::DataBar {
                    sheet_name: sn,
                    range,
                    color_hex,
                } if sn.eq_ignore_ascii_case(sheet_name) => {
                    if let (Some(col), Some((lo, hi))) = (
                        parse_hex_color(color_hex),
                        app.cached_range_min_max(sheet, *range),
                    ) {
                        data_bars.push((*range, col, lo, hi));
                    }
                }
                VisualRule::ColorScale {
                    sheet_name: sn,
                    range,
                    low_hex,
                    high_hex,
                } if sn.eq_ignore_ascii_case(sheet_name) => {
                    if let (Some(lc), Some(hc), Some((lo, hi))) = (
                        parse_hex_color(low_hex),
                        parse_hex_color(high_hex),
                        app.cached_range_min_max(sheet, *range),
                    ) {
                        color_scales.push((*range, lc, hc, lo, hi));
                    }
                }
                VisualRule::Sparkline {
                    sheet_name: sn,
                    target_row,
                    target_col,
                    source_range,
                } if sn.eq_ignore_ascii_case(sheet_name) => {
                    sparklines.push((*target_row, *target_col, *source_range));
                }
                _ => {}
            }
        }

        let precedents_on_sheet = app
            .formula_precedents
            .iter()
            .filter(|pr| {
                pr.sheet_name
                    .as_deref()
                    .map(|sn| sn.eq_ignore_ascii_case(sheet_name))
                    .unwrap_or(true)
            })
            .map(|pr| {
                let [r, g, b] = p.series[pr.color_idx % p.series.len()];
                ([pr.r1, pr.c1, pr.r2, pr.c2], Color::Rgb(r, g, b))
            })
            .collect();

        let hover_row = app
            .chart_hover
            .as_ref()
            .and_then(|h| (h.sheet == sheet).then_some(h.source_row));

        let fill_preview = match app.drag {
            DragState::FillHandle {
                source: [sr1, sc1, sr2, sc2],
                target_row,
                target_col,
            } => Some([
                sr1.min(target_row),
                sc1.min(target_col),
                sr2.max(target_row),
                sc2.max(target_col),
            ]),
            _ => None,
        };

        let move_preview = match app.drag {
            DragState::MovingSelection {
                source: [sr1, sc1, sr2, sc2],
                target_r1,
                target_c1,
                ..
            } => Some([
                target_r1,
                target_c1,
                target_r1 + (sr2 - sr1),
                target_c1 + (sc2 - sc1),
            ]),
            _ => None,
        };

        Self {
            autofilter_info,
            data_bars,
            color_scales,
            sparklines,
            precedents_on_sheet,
            hover_row,
            fill_preview,
            move_preview,
        }
    }
}

pub(super) fn draw_grid(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.width < 8 || area.height < 3 {
        return;
    }
    let p = app.palette;
    let sheet = app.wb.active_sheet();
    let sheet_name = app.wb.active_sheet_name();
    let (frozen_rows, frozen_cols) = app.wb.frozen_counts(sheet);
    let (scroll_row, scroll_col) = app.sheet_scroll(sheet);
    let (cur_r, cur_c) = app.wb.selected_cell();
    let [sel_r1, sel_c1, sel_r2, sel_c2] = app.wb.selected_range();

    let show_scrollbars = area.width >= 20 && area.height >= 7;
    let grid_right = if show_scrollbars {
        area.right().saturating_sub(1)
    } else {
        area.right()
    };
    let grid_bottom = if show_scrollbars {
        area.bottom().saturating_sub(1)
    } else {
        area.bottom()
    };

    let max_vis_row = scroll_row.saturating_add(area.height as i32);
    let row_hdr_w = (max_vis_row.to_string().len() as u16 + 2)
        .max(5)
        .min(area.width / 3);
    let header_y = area.y;
    let body_x = area.x + row_hdr_w;
    let body_y = area.y + 1;

    app.layout.corner_rect = Rect::new(area.x, header_y, row_hdr_w, 1);
    app.layout.col_header_y = header_y;
    app.layout.row_header_x_end = body_x;

    let buf = f.buffer_mut();
    fill_rect(buf, area, Style::default().bg(p.grid_bg).fg(p.text));
    fill_rect(
        buf,
        app.layout.corner_rect,
        Style::default().bg(p.header_bg).fg(p.header_fg),
    );
    buf.set_string(
        area.x + row_hdr_w.saturating_sub(1),
        header_y,
        "┼",
        Style::default().bg(p.header_bg).fg(p.grid_line),
    );

    let col_plan = build_visible_cols(app, sheet, frozen_cols, scroll_col, body_x, grid_right);
    draw_col_headers(buf, &col_plan, header_y, grid_right, sel_c1, sel_c2, &p);
    let row_plan = build_visible_rows(app, sheet, frozen_rows, scroll_row, body_y, grid_bottom);
    let ov = GridOverlays::collect(app, sheet, &sheet_name, &p);

    let mut ry = body_y;
    for item in &row_plan {
        let Some(r) = *item else {
            let line = "═".repeat((grid_right - area.x) as usize);
            buf.set_string(
                area.x,
                ry,
                &line,
                Style::default().bg(p.grid_bg).fg(p.frozen_line),
            );
            ry += 1;
            continue;
        };

        let row_in_sel = (sel_r1..=sel_r2).contains(&r);
        let row_is_hover = ov.hover_row == Some(r);
        let rh_style = if row_is_hover {
            Style::default()
                .bg(p.accent)
                .fg(Color::Rgb(10, 14, 20))
                .add_modifier(Modifier::BOLD)
        } else if row_in_sel {
            Style::default()
                .bg(p.header_active_bg)
                .fg(p.header_active_fg)
                .add_modifier(Modifier::BOLD)
        } else if ov.autofilter_info.is_some() {
            Style::default().bg(p.header_bg).fg(p.accent)
        } else {
            Style::default().bg(p.header_bg).fg(p.header_fg)
        };
        let r_txt = format!(
            "{:>width$}│",
            r,
            width = row_hdr_w.saturating_sub(1) as usize
        );
        buf.set_string(area.x, ry, &r_txt, rh_style);

        for &(c, cx, cw, is_frozen_edge) in &col_plan {
            let is_cursor = r == cur_r && c == cur_c;
            let in_sel = row_in_sel && (sel_c1..=sel_c2).contains(&c);
            let in_fill = ov.fill_preview.is_some_and(|[fr1, fc1, fr2, fc2]| {
                (fr1..=fr2).contains(&r) && (fc1..=fc2).contains(&c)
            });
            let in_move = ov.move_preview.is_some_and(|[mr1, mc1, mr2, mc2]| {
                (mr1..=mr2).contains(&r) && (mc1..=mc2).contains(&c)
            });
            let is_match = app.search_highlights.contains(&(sheet, r, c));

            let cs = app.wb.cell_render_style(sheet, r, c);
            let editing_here = app
                .edit
                .as_ref()
                .is_some_and(|ed| ed.sheet == sheet && ed.row == r && ed.col == c);

            let mut text = if editing_here {
                app.edit.as_ref().unwrap().buf.clone()
            } else if app.show_formulas && cs.is_formula {
                app.wb.cell_input_content(sheet, r, c)
            } else {
                app.wb.cell_display(sheet, r, c)
            };

            let mut bg = cs.bg.unwrap_or(p.grid_bg);
            let mut fg = if cs.is_error {
                p.error_fg
            } else {
                cs.fg.unwrap_or(p.text)
            };

            if text.is_empty()
                && !editing_here
                && let Some(&(_, _, src_rng)) = ov
                    .sparklines
                    .iter()
                    .find(|&&(tr, tc, _)| tr == r && tc == c)
            {
                text = app.wb.compute_sparkline_text(sheet, src_rng, cw as usize);
                if cs.fg.is_none() {
                    fg = p.accent;
                }
            }

            let mut has_scale_bg = false;
            if let Some(&(_, lc, hc, lo, hi)) = ov
                .color_scales
                .iter()
                .find(|&&([r1, c1, r2, c2], ..)| (r1..=r2).contains(&r) && (c1..=c2).contains(&c))
                && let Some(nv) = app.wb.cell_numeric(sheet, r, c)
            {
                let t = if (hi - lo).abs() < 1e-12 {
                    0.5
                } else {
                    ((nv - lo) / (hi - lo)).clamp(0.0, 1.0)
                };
                bg = blend_rgb(lc, hc, t);
                has_scale_bg = true;
            }

            if let Some(&(_, pcol)) = ov
                .precedents_on_sheet
                .iter()
                .find(|&&([r1, c1, r2, c2], _)| (r1..=r2).contains(&r) && (c1..=c2).contains(&c))
            {
                bg = blend_rgb(bg, pcol, 0.30);
            }

            if row_is_hover && !is_cursor {
                bg = blend_rgb(bg, p.accent, 0.22);
            }
            if is_match {
                bg = p.match_bg;
            }
            if (in_fill || in_move) && !in_sel {
                fg = p.fill_preview;
            }
            if in_sel && cs.bg.is_none() && !has_scale_bg {
                bg = p.selection_bg;
            }
            if is_cursor && !editing_here {
                bg = p.cursor_bg;
                fg = p.cursor_fg;
            }

            let mut mod_flags = Modifier::empty();
            if cs.bold || is_cursor {
                mod_flags |= Modifier::BOLD;
            }
            if cs.italic {
                mod_flags |= Modifier::ITALIC;
            }
            if cs.underline {
                mod_flags |= Modifier::UNDERLINED;
            }
            if cs.strikethrough {
                mod_flags |= Modifier::CROSSED_OUT;
            }

            let formatted = align_cell_text(
                &text,
                cw as usize,
                if editing_here || (app.show_formulas && cs.is_formula) {
                    Alignment::Left
                } else {
                    cs.align
                },
            );
            buf.set_string(
                cx,
                ry,
                &formatted,
                Style::default().bg(bg).fg(fg).add_modifier(mod_flags),
            );

            if !is_cursor
                && let Some(&(_, bar_col, lo, hi)) =
                    ov.data_bars.iter().find(|&&([r1, c1, r2, c2], ..)| {
                        (r1..=r2).contains(&r) && (c1..=c2).contains(&c)
                    })
                && let Some(nv) = app.wb.cell_numeric(sheet, r, c)
            {
                let base = lo.min(0.0);
                let span = (hi - base).max(1e-9);
                let frac = ((nv - base) / span).clamp(0.0, 1.0);
                let bar_w = (frac * cw as f64).round() as u16;
                let bar_bg = blend_rgb(bg, bar_col, 0.42);
                for dx in 0..bar_w.min(cw) {
                    if let Some(cell) = buf.cell_mut((cx + dx, ry)) {
                        cell.set_bg(bar_bg);
                    }
                }
            }

            if let Some((af_row, af_c1, af_c2, ref active_cols)) = ov.autofilter_info
                && r == af_row
                && (af_c1..=af_c2).contains(&c)
                && cw >= 2
                && !editing_here
            {
                let is_filtered = active_cols.contains(&c);
                let icon = if is_filtered { "▼" } else { "▾" };
                let icon_fg = if is_filtered { p.accent } else { p.muted };
                buf.set_string(
                    cx + cw - 1,
                    ry,
                    icon,
                    Style::default()
                        .bg(bg)
                        .fg(icon_fg)
                        .add_modifier(Modifier::BOLD),
                );
                app.layout
                    .filter_buttons
                    .push((c, Rect::new(cx + cw.saturating_sub(2), ry, 2, 1)));
            }

            if row_in_sel && c == sel_c1 && app.edit.is_none() && cx > area.x {
                let lx = cx - 1;
                app.layout.selection_border_cells.push((lx, ry));
                buf.set_string(
                    lx,
                    ry,
                    "┃",
                    Style::default()
                        .bg(if c == 1 { p.header_bg } else { p.grid_bg })
                        .fg(p.accent),
                );
            }

            if cx + cw < grid_right {
                let is_sel_bottom_right = r == sel_r2 && c == sel_c2 && app.edit.is_none();
                if is_sel_bottom_right {
                    app.layout.fill_handle = Some((cx + cw, ry));
                    buf.set_string(
                        cx + cw,
                        ry,
                        "▪",
                        Style::default()
                            .bg(p.grid_bg)
                            .fg(p.fill_preview)
                            .add_modifier(Modifier::BOLD),
                    );
                } else if row_in_sel && c == sel_c2 && app.edit.is_none() {
                    app.layout.selection_border_cells.push((cx + cw, ry));
                    buf.set_string(
                        cx + cw,
                        ry,
                        "┃",
                        Style::default().bg(p.grid_bg).fg(p.accent),
                    );
                } else {
                    let (sep_ch, sep_fg) = if is_frozen_edge {
                        ("║", p.frozen_line)
                    } else if in_fill || in_move {
                        ("┆", p.fill_preview)
                    } else {
                        ("│", p.grid_line)
                    };
                    buf.set_string(
                        cx + cw,
                        ry,
                        sep_ch,
                        Style::default().bg(p.grid_bg).fg(sep_fg),
                    );
                }
            }
        }
        ry += 1;
    }

    if show_scrollbars {
        draw_scrollbars(
            buf,
            app,
            sheet,
            scroll_row,
            scroll_col,
            body_x,
            body_y,
            grid_right,
            grid_bottom,
            &p,
        );
    }
}

fn build_visible_cols(
    app: &mut App,
    sheet: u32,
    frozen_cols: i32,
    scroll_col: i32,
    body_x: u16,
    grid_right: u16,
) -> Vec<(i32, u16, u16, bool)> {
    let mut x = body_x;
    let mut scroll_col_count = 0usize;
    let mut col_plan = Vec::new();

    for phase in 0..2 {
        let range = if phase == 0 {
            1..=frozen_cols
        } else {
            scroll_col.max(frozen_cols + 1)..=MAX_COLS
        };
        for c in range {
            if x >= grid_right {
                break;
            }
            let w = app.wb.col_width_chars(sheet, c);
            let cell_w = w.min(grid_right - x);
            if cell_w == 0 {
                break;
            }
            let is_frozen_edge = phase == 0 && c == frozen_cols;
            let sep_w = u16::from(x + cell_w < grid_right);
            let div_x = (x + cell_w).min(grid_right.saturating_sub(1));
            app.layout.cols.push(ColHit {
                col: c,
                x_start: x,
                x_end: x + cell_w,
                divider_x: div_x,
            });
            col_plan.push((c, x, cell_w, is_frozen_edge));
            if phase == 1 {
                scroll_col_count += 1;
            }
            x += cell_w + sep_w;
        }
    }
    app.layout.visible_scroll_cols = scroll_col_count.max(1);
    col_plan
}

fn draw_col_headers(
    buf: &mut Buffer,
    col_plan: &[(i32, u16, u16, bool)],
    header_y: u16,
    grid_right: u16,
    sel_c1: i32,
    sel_c2: i32,
    p: &Palette,
) {
    for &(c, cx, cw, is_frozen_edge) in col_plan {
        let in_sel = (sel_c1..=sel_c2).contains(&c);
        let h_style = if in_sel {
            Style::default()
                .bg(p.header_active_bg)
                .fg(p.header_active_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.header_bg).fg(p.header_fg)
        };
        let label = center_pad(&col_to_letters(c), cw as usize);
        buf.set_string(cx, header_y, &label, h_style);
        if cx + cw < grid_right {
            let (sep_ch, sep_fg) = if is_frozen_edge {
                ("║", p.frozen_line)
            } else {
                ("│", p.grid_line)
            };
            buf.set_string(
                cx + cw,
                header_y,
                sep_ch,
                Style::default().bg(p.header_bg).fg(sep_fg),
            );
        }
    }
}

fn build_visible_rows(
    app: &mut App,
    sheet: u32,
    frozen_rows: i32,
    scroll_row: i32,
    body_y: u16,
    grid_bottom: u16,
) -> Vec<Option<i32>> {
    let mut y = body_y;
    let mut scroll_row_count = 0usize;
    let mut row_plan = Vec::new();

    for r in 1..=frozen_rows {
        if y >= grid_bottom {
            break;
        }
        if app.wb.is_row_hidden(sheet, r) {
            continue;
        }
        app.layout.rows.push(RowHit { row: r, y });
        row_plan.push(Some(r));
        y += 1;
    }
    if frozen_rows > 0 && y < grid_bottom {
        row_plan.push(None);
        y += 1;
    }
    let mut r = scroll_row.max(frozen_rows + 1);
    while y < grid_bottom && r <= MAX_ROWS {
        if app.wb.is_row_hidden(sheet, r) {
            r += 1;
            continue;
        }
        app.layout.rows.push(RowHit { row: r, y });
        row_plan.push(Some(r));
        scroll_row_count += 1;
        r += 1;
        y += 1;
    }
    app.layout.visible_scroll_rows = scroll_row_count.max(1);
    row_plan
}

#[allow(clippy::too_many_arguments)]
fn draw_scrollbars(
    buf: &mut Buffer,
    app: &mut App,
    sheet: u32,
    scroll_row: i32,
    scroll_col: i32,
    body_x: u16,
    body_y: u16,
    grid_right: u16,
    grid_bottom: u16,
    p: &Palette,
) {
    let (used_r, used_c) = app.wb.sheet_bounds(sheet);
    let vis_r = app.layout.visible_scroll_rows.max(1) as i32;
    let vis_c = app.layout.visible_scroll_cols.max(1) as i32;

    let v_h = grid_bottom.saturating_sub(body_y);
    if v_h >= 2 {
        let v_rect = Rect::new(grid_right, body_y, 1, v_h);
        app.layout.v_scrollbar = Some(v_rect);
        let total_r = used_r.max(scroll_row + vis_r + 20).max(30) as f64;
        let frac = ((scroll_row - 1) as f64 / (total_r - vis_r as f64).max(1.0)).clamp(0.0, 1.0);
        let thumb_h = ((vis_r as f64 / total_r) * v_h as f64)
            .round()
            .clamp(1.0, v_h as f64) as u16;
        let thumb_y = v_rect.y + ((v_h.saturating_sub(thumb_h)) as f64 * frac).round() as u16;
        for sy in v_rect.y..v_rect.bottom() {
            let in_thumb = sy >= thumb_y && sy < thumb_y + thumb_h;
            buf.set_string(
                v_rect.x,
                sy,
                if in_thumb { "█" } else { "░" },
                Style::default()
                    .bg(p.header_bg)
                    .fg(if in_thumb { p.accent } else { p.border }),
            );
        }
    }

    let h_w = grid_right.saturating_sub(body_x);
    if h_w >= 4 {
        let h_rect = Rect::new(body_x, grid_bottom, h_w, 1);
        app.layout.h_scrollbar = Some(h_rect);
        let total_c = used_c.max(scroll_col + vis_c + 8).max(16) as f64;
        let frac = ((scroll_col - 1) as f64 / (total_c - vis_c as f64).max(1.0)).clamp(0.0, 1.0);
        let thumb_w = ((vis_c as f64 / total_c) * h_w as f64)
            .round()
            .clamp(2.0, h_w as f64) as u16;
        let thumb_x = h_rect.x + ((h_w.saturating_sub(thumb_w)) as f64 * frac).round() as u16;
        for sx in h_rect.x..h_rect.right() {
            let in_thumb = sx >= thumb_x && sx < thumb_x + thumb_w;
            buf.set_string(
                sx,
                h_rect.y,
                if in_thumb { "▃" } else { "─" },
                Style::default()
                    .bg(p.header_bg)
                    .fg(if in_thumb { p.accent } else { p.border }),
            );
        }
    }
}
