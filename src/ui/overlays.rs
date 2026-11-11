use super::{fill_rect, rects_intersect, truncate_with_ellipsis};
use crate::app::App;
use crate::chart::{ChartJob, ChartRenderKey, render_braille_fallback};
use crate::config::ThemeMode;
use crate::formula::CompletionKind;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui_image::Image as RatatuiImage;

pub(super) fn compute_formula_popup_rect(app: &App, screen: Rect) -> Option<Rect> {
    let ed = app.edit.as_ref()?;
    if !app.modal.is_none() || app.top_menu.is_some() || app.context_menu.is_some() {
        return None;
    }
    let has_sig =
        app.formula_assist.active_fn.is_some() || app.formula_assist.fallback_fn.is_some();
    let max_cand_rows = (usize::from(screen.height) * 45 / 100).clamp(7, 28);
    let cand_rows = app.formula_assist.candidates.len().min(max_cand_rows) as u16;
    if !has_sig && cand_rows == 0 {
        return None;
    }
    let sig_rows = if has_sig {
        if cand_rows > 0 { 3 } else { 2 }
    } else {
        0
    };
    let detail_rows = if cand_rows > 0 { 2 } else { 0 };
    let chrome_rows = sig_rows + detail_rows + 2;
    let desired_h = (chrome_rows + cand_rows).min(screen.height.saturating_sub(3));
    if desired_h < 3 {
        return None;
    }
    let w = if cand_rows > 0 {
        ((u32::from(screen.width) * 58 / 100) as u16).clamp(68, 116)
    } else {
        ((u32::from(screen.width) * 48 / 100) as u16).clamp(58, 96)
    }
    .min(screen.width.saturating_sub(2));

    // Anchor right at the edited cell when visible on the active sheet; otherwise under `fx`.
    if ed.sheet == app.wb.active_sheet()
        && let Some(ch) = app.layout.cols.iter().find(|c| c.col == ed.col)
        && let Some(rh) = app.layout.rows.iter().find(|r| r.row == ed.row)
    {
        if cand_rows > 0 {
            let below_y = rh.y + 1;
            let avail_below = screen.bottom().saturating_sub(1).saturating_sub(below_y);
            let avail_above = rh.y.saturating_sub(2);
            let min_usable_h = chrome_rows + 4;
            let (y, h) = if avail_below >= desired_h
                || (avail_below >= min_usable_h && avail_below >= avail_above)
            {
                (below_y, desired_h.min(avail_below))
            } else if avail_above >= min_usable_h {
                let fitted_h = desired_h.min(avail_above);
                (rh.y - fitted_h, fitted_h)
            } else {
                (
                    below_y.min(screen.bottom().saturating_sub(desired_h)),
                    desired_h,
                )
            };
            let x = ch.x_start.min(screen.right().saturating_sub(w));
            return Some(Rect::new(x, y, w, h));
        }
        // Pure signature callout (`=SUM(`): keep rows immediately below the cell uncovered
        // so mouse range-picking below the formula cell is unobstructed.
        let h = desired_h;
        let body_top = app.layout.grid_rect.y + 1;
        if rh.y >= body_top + h {
            let x = ch.x_start.min(screen.right().saturating_sub(w));
            return Some(Rect::new(x, rh.y - h, w, h));
        }
        if ch.x_end + 2 + w <= screen.right() {
            let y = rh.y.min(screen.bottom().saturating_sub(h));
            return Some(Rect::new(ch.x_end + 2, y, w, h));
        }
        let x = ch.x_start.min(screen.right().saturating_sub(w));
        let y = (rh.y + 1).min(screen.bottom().saturating_sub(h));
        return Some(Rect::new(x, y, w, h));
    }

    let x = (app.layout.formula_rect.x + 2).min(screen.right().saturating_sub(w));
    let y = (app.layout.formula_rect.y + 1).min(screen.bottom().saturating_sub(desired_h));
    Some(Rect::new(x, y, w, desired_h))
}

pub(super) fn draw_formula_assist_popup(f: &mut Frame<'_>, app: &mut App, rect: Rect) {
    app.layout.formula_assist_items.clear();
    let p = app.palette;
    let has_cands = !app.formula_assist.candidates.is_empty();
    f.render_widget(Clear, rect);
    let title = if has_cands {
        " ƒ IntelliSense  (↑↓ select · Tab insert · F4 $) "
    } else {
        " ƒ Formula Signature  (F4 lock $) "
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.menu_bg).fg(p.menu_fg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height == 0 || inner.width < 8 {
        return;
    }

    let mut y = inner.y;
    if let Some((sig, arg_idx)) = app.formula_assist.active_fn {
        let mut spans = vec![Span::styled(
            format!(" {}(", sig.name),
            Style::default()
                .fg(p.formula_fg)
                .add_modifier(Modifier::BOLD),
        )];
        for (i, &arg) in sig.args.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(", ", Style::default().fg(p.muted)));
            }
            let active = i == arg_idx || (i + 1 == sig.args.len() && arg_idx >= sig.args.len());
            let st = if active {
                Style::default()
                    .fg(p.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(p.text)
            };
            spans.push(Span::styled(arg, st));
        }
        spans.push(Span::styled(
            ")",
            Style::default()
                .fg(p.formula_fg)
                .add_modifier(Modifier::BOLD),
        ));
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(p.menu_bg)),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
        if y < inner.bottom() {
            let active_param = sig
                .args
                .get(arg_idx)
                .or_else(|| sig.args.last())
                .copied()
                .unwrap_or("");
            let desc = if active_param.is_empty() {
                format!(" {}", sig.summary)
            } else {
                format!(" {}  ·  arg {}: {active_param}", sig.summary, arg_idx + 1)
            };
            let clipped: String = desc.chars().take(inner.width as usize).collect();
            f.buffer_mut().set_string(
                inner.x,
                y,
                &clipped,
                Style::default().bg(p.menu_bg).fg(p.muted),
            );
            y += 1;
        }
        if has_cands && y < inner.bottom() {
            let sep = "─".repeat(inner.width as usize);
            f.buffer_mut().set_string(
                inner.x,
                y,
                &sep,
                Style::default().bg(p.menu_bg).fg(p.border),
            );
            y += 1;
        }
    } else if let Some((ref name, arg_idx)) = app.formula_assist.fallback_fn
        && y < inner.bottom()
    {
        f.buffer_mut().set_string(
            inner.x + 1,
            y,
            format!("{name}(… arg #{})", arg_idx + 1),
            Style::default().bg(p.menu_bg).fg(p.formula_fg),
        );
        y += 1;
        if y < inner.bottom() {
            y += 1;
        }
        if has_cands && y < inner.bottom() {
            let sep = "─".repeat(inner.width as usize);
            f.buffer_mut().set_string(
                inner.x,
                y,
                &sep,
                Style::default().bg(p.menu_bg).fg(p.border),
            );
            y += 1;
        }
    }

    if !has_cands || y >= inner.bottom() {
        return;
    }

    let total = app.formula_assist.candidates.len();
    let sel = app
        .edit
        .as_ref()
        .map(|e| e.completion_idx)
        .unwrap_or(0)
        .min(total.saturating_sub(1));

    let rem_h = inner.bottom().saturating_sub(y);
    let show_footer = rem_h >= 3;
    let cap = (if show_footer { rem_h - 2 } else { rem_h }) as usize;
    let mut offset = app.edit.as_ref().map(|e| e.completion_scroll).unwrap_or(0);
    if cap > 0 {
        if sel < offset {
            offset = sel;
        } else if sel >= offset + cap {
            offset = sel + 1 - cap;
        }
        offset = offset.min(total.saturating_sub(cap));
    } else {
        offset = 0;
    }
    if let Some(ref mut ed) = app.edit {
        ed.completion_scroll = offset;
    }
    let has_scroll = total > cap;
    let row_text_w = inner.width.saturating_sub(u16::from(has_scroll)) as usize;

    for (row_i, cand) in app
        .formula_assist
        .candidates
        .iter()
        .skip(offset)
        .take(cap)
        .enumerate()
    {
        let idx = offset + row_i;
        let cy = y + row_i as u16;
        let is_sel = idx == sel;
        let row_r = Rect::new(inner.x, cy, inner.width, 1);
        app.layout.formula_assist_items.push((idx, row_r));

        let base_st = if is_sel {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.menu_bg).fg(p.menu_fg)
        };
        fill_rect(f.buffer_mut(), row_r, base_st);

        let icon_st = if is_sel {
            base_st
        } else {
            let fg = match cand.kind {
                CompletionKind::Function => p.formula_fg,
                CompletionKind::Constant | CompletionKind::DefinedName => p.accent,
                CompletionKind::SheetRef => p.frozen_line,
            };
            Style::default()
                .bg(p.menu_bg)
                .fg(fg)
                .add_modifier(Modifier::BOLD)
        };
        f.buffer_mut()
            .set_string(inner.x + 1, cy, cand.kind.icon(), icon_st);

        let left_max = if row_text_w >= 64 {
            (row_text_w * 65 / 100).max(38)
        } else {
            row_text_w.saturating_sub(4)
        };
        let shown_lbl = truncate_with_ellipsis(&cand.label, left_max);
        f.buffer_mut()
            .set_string(inner.x + 3, cy, &shown_lbl, base_st);

        let used_left = 3 + shown_lbl.chars().count();
        if row_text_w > used_left + 8 {
            let avail_detail = row_text_w - used_left - 2;
            let shown_det = truncate_with_ellipsis(&cand.detail, avail_detail);
            let dx = inner.x + (row_text_w - shown_det.chars().count() - 1) as u16;
            let det_st = if is_sel {
                Style::default().bg(p.menu_hi_bg).fg(p.menu_hi_fg)
            } else {
                Style::default().bg(p.menu_bg).fg(p.muted)
            };
            f.buffer_mut().set_string(dx, cy, &shown_det, det_st);
        }

        if has_scroll {
            let thumb_pos = (offset * cap.saturating_sub(1)) / total.saturating_sub(cap).max(1);
            let is_thumb = row_i == thumb_pos;
            f.buffer_mut().set_string(
                inner.right() - 1,
                cy,
                if is_thumb { "▐" } else { "│" },
                Style::default()
                    .bg(p.menu_bg)
                    .fg(if is_thumb { p.accent } else { p.border }),
            );
        }
    }

    if show_footer && let Some(sel_cand) = app.formula_assist.candidates.get(sel) {
        let sep_y = inner.bottom() - 2;
        let det_y = inner.bottom() - 1;
        let count_tag = format!(" {}/{} ", sel + 1, total);
        let sep_line = "─".repeat(inner.width as usize);
        f.buffer_mut().set_string(
            inner.x,
            sep_y,
            &sep_line,
            Style::default().bg(p.menu_bg).fg(p.border),
        );
        if inner.width > count_tag.len() as u16 + 2 {
            f.buffer_mut().set_string(
                inner.right() - count_tag.len() as u16 - 1,
                sep_y,
                &count_tag,
                Style::default().bg(p.menu_bg).fg(p.muted),
            );
        }
        let footer = format!(" ▸ {} — {}", sel_cand.label, sel_cand.detail);
        let clipped: String = footer.chars().take(inner.width as usize).collect();
        f.buffer_mut().set_string(
            inner.x,
            det_y,
            &clipped,
            Style::default().bg(p.menu_bg).fg(p.accent),
        );
    }
}

pub(super) fn draw_chart_dock(
    f: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    overlays: [Option<Rect>; 4],
) {
    let p = app.palette;
    fill_rect(
        f.buffer_mut(),
        area,
        Style::default().bg(p.grid_bg).fg(p.text),
    );
    if area.height < 3 || area.width < 12 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(2)])
        .split(area);
    let hdr = chunks[0];
    let body = chunks[1];
    app.layout.chart_body_rect = Some(body);

    fill_rect(
        f.buffer_mut(),
        hdr,
        Style::default().bg(p.header_bg).fg(p.header_fg),
    );
    let mut x = hdr.x + 1;
    for (idx, ch) in app.wb.charts.iter().enumerate() {
        let lbl = format!(" {}:{} ", idx + 1, ch.kind.label());
        let w = lbl.chars().count() as u16;
        if x + w + 26 > hdr.right() {
            break;
        }
        let r = Rect::new(x, hdr.y, w, 1);
        app.layout.chart_tab_rects.push((idx, r));
        let st = if idx == app.active_chart_idx {
            Style::default()
                .bg(p.header_active_bg)
                .fg(p.header_active_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.header_bg).fg(p.header_fg)
        };
        f.buffer_mut().set_string(x, hdr.y, &lbl, st);
        x += w + 1;
    }

    let buttons: [(&str, u8); 7] = [
        (" × ", 6),
        (" ⤢ ", 5),
        (" SVG ", 4),
        (" PNG ", 3),
        (" Edit ", 2),
        (" Type ", 1),
        (" +Chart ", 0),
    ];
    let mut rx = hdr.right();
    for (label, id) in buttons {
        let w = label.chars().count() as u16;
        if rx < x + w + 1 {
            break;
        }
        rx -= w;
        let rect = Rect::new(rx, hdr.y, w, 1);
        f.buffer_mut().set_string(
            rx,
            hdr.y,
            label,
            Style::default().bg(p.surface).fg(p.accent),
        );
        match id {
            0 => app.layout.chart_btn_new = Some(rect),
            1 => app.layout.chart_btn_type = Some(rect),
            2 => app.layout.chart_btn_edit = Some(rect),
            3 => app.layout.chart_btn_png = Some(rect),
            4 => app.layout.chart_btn_svg = Some(rect),
            5 => app.layout.chart_btn_zoom = Some(rect),
            6 => app.layout.chart_btn_close = Some(rect),
            _ => {}
        }
        rx = rx.saturating_sub(1);
    }

    if let Some(ref hit) = app.chart_hover
        && rx > x + 6
    {
        let tip = format!(
            "⌖ {} · {}: {:.2} (R{})",
            hit.category, hit.series_name, hit.value, hit.source_row
        );
        let avail = rx.saturating_sub(x + 1) as usize;
        let clipped: String = tip.chars().take(avail).collect();
        f.buffer_mut().set_string(
            x,
            hdr.y,
            &clipped,
            Style::default()
                .bg(p.header_bg)
                .fg(p.accent)
                .add_modifier(Modifier::BOLD),
        );
    }

    if app.wb.charts.is_empty() {
        let msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No charts yet",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "  Select a cell range (e.g. A1:C10) and click [+Chart] or Chart → New Chart.",
                Style::default().fg(p.muted),
            )),
        ];
        f.render_widget(
            Paragraph::new(msg).style(Style::default().bg(p.grid_bg)),
            body,
        );
        return;
    }

    let idx = app.active_chart_idx.min(app.wb.charts.len() - 1);
    app.active_chart_idx = idx;
    let spec = app.wb.charts[idx].clone();
    let Some(data) = app.cached_chart_data(&spec) else {
        let msg = Paragraph::new(format!(
            "\n  Range {}!{} has no numeric values to plot.\n  Click [Edit] to change the source range.",
            spec.sheet_name, spec.range
        ))
        .style(Style::default().bg(p.grid_bg).fg(p.muted));
        f.render_widget(msg, body);
        return;
    };

    let overlapped_by_popup = overlays
        .iter()
        .flatten()
        .any(|ov| rects_intersect(*ov, body));

    if app.use_pixel_charts() && !overlapped_by_popup {
        let key = ChartRenderKey {
            chart_id: spec.id,
            data_rev: app.data_rev,
            cols: body.width,
            rows: body.height,
            spec: spec.clone(),
            dark: app.cfg.theme == ThemeMode::Dark,
        };
        let has_ready = app.cached_chart.as_ref().is_some_and(|r| r.key == key);

        if !has_ready && app.pending_chart_key.as_ref() != Some(&key) {
            app.pending_chart_key = Some(key.clone());
            app.chart_worker.submit(ChartJob {
                key,
                data: data.clone(),
                palette: p,
                picker: app.picker.clone(),
            });
        }

        if let Some(ref ready) = app.cached_chart
            && ready.key.chart_id == spec.id
            && ready.key.cols == body.width
            && ready.key.rows == body.height
        {
            f.render_widget(RatatuiImage::new(&ready.protocol), body);
            return;
        }
    }

    render_braille_fallback(f, body, &spec, &data, &p);
}
