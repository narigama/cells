mod file_picker;
mod grid;
mod modals;
mod overlays;

use crate::actions::{Action, MenuEntry, TOP_MENUS, all_palette_commands};
use crate::address::format_range_ref;
use crate::app::{App, DragState, LayoutCache};
use crate::config::{DockPosition, Palette};
use crate::workbook::ChartKind;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    let screen = f.area();
    app.layout = LayoutCache {
        screen,
        ..Default::default()
    };
    if screen.width < 16 || screen.height < 6 {
        return;
    }

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Top menu bar
            Constraint::Length(1), // Formula / address bar
            Constraint::Min(2),    // Workspace (grid + chart dock)
            Constraint::Length(1), // Sheet tabs + stats
            Constraint::Length(1), // Status bar
        ])
        .split(screen);

    app.layout.menubar = root[0];
    app.layout.workspace_rect = root[2];

    draw_menubar(f, app, root[0]);
    draw_formula_bar(f, app, root[1]);

    // Precompute overlay bounds so Kitty/Sixel pixel charts yield to overlapping popups.
    // `formula_popup_rect` is finalized right after `draw_grid` populates cell coordinates.
    let dropdown_rect = compute_dropdown_rect(app, root[0]);
    let context_rect = compute_context_rect(app, screen);
    let modal_rect = modals::compute_modal_rect(&app.modal, screen);
    app.layout.dropdown_rect = dropdown_rect;
    app.layout.context_rect = context_rect;
    app.layout.modal_rect = modal_rect;

    draw_workspace(
        f,
        app,
        root[2],
        [dropdown_rect, context_rect, modal_rect, None],
    );
    draw_sheet_tabs(f, app, root[3]);
    draw_status_bar(f, app, root[4]);

    if let Some(r) = app.layout.formula_popup_rect {
        overlays::draw_formula_assist_popup(f, app, r);
    }
    if let Some(r) = dropdown_rect {
        draw_top_dropdown(f, app, r);
    }
    if let Some(r) = context_rect {
        draw_context_menu(f, app, r);
    }
    if let Some(r) = modal_rect {
        modals::draw_modal(f, app, r);
        modals::set_modal_cursor(f, app, r);
    } else if app.top_menu.is_none()
        && app.context_menu.is_none()
        && let Some(ref ed) = app.edit
    {
        let prefix_chars = ed.buf[..ed.cursor.min(ed.buf.len())].chars().count() as u16;
        let max_x = app.layout.formula_rect.right().saturating_sub(1);
        let cx = (app.layout.formula_rect.x + 4 + prefix_chars).min(max_x);
        f.set_cursor_position((cx, app.layout.formula_rect.y));
    }

    if let Some((mx, my)) = app.mouse_cell {
        app.cursor_shape = crate::input::compute_cursor_shape(app, mx, my);
    }

    // Software-rendered cell fallback cursor when pixel RGBA sprites are unavailable.
    app.cursor_rt
        .draw_cell_fallback(f.buffer_mut(), screen, app.mouse_cell, app.cursor_shape);
}

fn draw_menubar(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let p = app.palette;
    fill_rect(
        f.buffer_mut(),
        area,
        Style::default().bg(p.menu_bg).fg(p.menu_fg),
    );

    let mut x = area.x + 1;
    for (idx, menu) in TOP_MENUS.iter().enumerate() {
        let label = format!(" {} ", menu.title);
        let w = label.chars().count() as u16;
        if x + w > area.right() {
            break;
        }
        let item_rect = Rect::new(x, area.y, w, 1);
        app.layout.menu_titles.push((idx, item_rect));
        let is_open = app.top_menu.is_some_and(|m| m.menu_idx == idx);
        let style = if is_open {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.menu_bg).fg(p.menu_fg)
        };
        f.buffer_mut().set_string(x, area.y, &label, style);
        x += w + 1;
    }
}

fn draw_formula_bar(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let p = app.palette;
    fill_rect(
        f.buffer_mut(),
        area,
        Style::default().bg(p.surface).fg(p.text),
    );

    let [r1, c1, r2, c2] = app.wb.selected_range();
    let addr_str = format!(" {} ", format_range_ref(None, r1, c1, r2, c2));
    let addr_w = (addr_str.chars().count() as u16)
        .max(10)
        .min(area.width / 3);
    let addr_rect = Rect::new(area.x, area.y, addr_w, 1);
    let fx_rect = Rect::new(
        area.x + addr_w,
        area.y,
        area.width.saturating_sub(addr_w),
        1,
    );
    app.layout.address_rect = addr_rect;
    app.layout.formula_rect = fx_rect;

    f.render_widget(
        Paragraph::new(addr_str).alignment(Alignment::Center).style(
            Style::default()
                .bg(p.header_bg)
                .fg(p.accent)
                .add_modifier(Modifier::BOLD),
        ),
        addr_rect,
    );

    let (sr, sc) = app.wb.selected_cell();
    let content = if let Some(ref ed) = app.edit {
        ed.buf.clone()
    } else {
        app.wb.cell_input_content(app.wb.active_sheet(), sr, sc)
    };

    let mut spans = vec![Span::styled(
        " fx ",
        Style::default()
            .fg(p.muted)
            .add_modifier(Modifier::ITALIC | Modifier::BOLD),
    )];

    if app.edit.is_some() && content.starts_with('=') && !app.formula_precedents.is_empty() {
        let mut last = 0usize;
        for prec in &app.formula_precedents {
            let bs = prec.byte_start.min(content.len());
            let be = prec.byte_end.min(content.len());
            if bs > last {
                spans.push(Span::styled(
                    content[last..bs].to_string(),
                    Style::default().fg(p.formula_fg),
                ));
            }
            if be > bs {
                let [r, g, b] = p.series[prec.color_idx % p.series.len()];
                spans.push(Span::styled(
                    content[bs..be].to_string(),
                    Style::default()
                        .fg(Color::Rgb(r, g, b))
                        .add_modifier(Modifier::BOLD),
                ));
                last = be;
            }
        }
        if last < content.len() {
            spans.push(Span::styled(
                content[last..].to_string(),
                Style::default().fg(p.formula_fg),
            ));
        }
    } else {
        let val_color = if content.starts_with('=') {
            p.formula_fg
        } else {
            p.text
        };
        spans.push(Span::styled(content, Style::default().fg(val_color)));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(p.surface)),
        fx_rect,
    );
}

fn draw_workspace(f: &mut Frame<'_>, app: &mut App, area: Rect, mut overlays: [Option<Rect>; 4]) {
    let screen = app.layout.screen;
    if app.chart_fullscreen && app.dock_visible && !app.wb.charts.is_empty() {
        app.layout.formula_popup_rect = overlays::compute_formula_popup_rect(app, screen);
        overlays[3] = app.layout.formula_popup_rect;
        app.layout.chart_dock_rect = Some(area);
        overlays::draw_chart_dock(f, app, area, overlays);
        return;
    }

    if app.dock_visible && area.width >= 36 && area.height >= 8 {
        let pct = app.cfg.dock_percent.clamp(20, 80);
        let (grid_r, div_r, dock_r, vertical) = match app.cfg.dock_position {
            DockPosition::Right => {
                let dock_w = ((area.width as u32 * pct as u32) / 100)
                    .clamp(18, area.width.saturating_sub(16) as u32)
                    as u16;
                let grid_w = area.width.saturating_sub(dock_w + 1);
                (
                    Rect::new(area.x, area.y, grid_w, area.height),
                    Rect::new(area.x + grid_w, area.y, 1, area.height),
                    Rect::new(area.x + grid_w + 1, area.y, dock_w, area.height),
                    true,
                )
            }
            DockPosition::Bottom => {
                let dock_h = ((area.height as u32 * pct as u32) / 100)
                    .clamp(6, area.height.saturating_sub(5) as u32)
                    as u16;
                let grid_h = area.height.saturating_sub(dock_h + 1);
                (
                    Rect::new(area.x, area.y, area.width, grid_h),
                    Rect::new(area.x, area.y + grid_h, area.width, 1),
                    Rect::new(area.x, area.y + grid_h + 1, area.width, dock_h),
                    false,
                )
            }
        };

        app.layout.grid_rect = grid_r;
        app.layout.divider_rect = Some(div_r);
        app.layout.chart_dock_rect = Some(dock_r);

        grid::draw_grid(f, app, grid_r);
        app.layout.formula_popup_rect = overlays::compute_formula_popup_rect(app, screen);
        overlays[3] = app.layout.formula_popup_rect;
        draw_divider(f, app, div_r, vertical);
        overlays::draw_chart_dock(f, app, dock_r, overlays);
        return;
    }

    app.layout.grid_rect = area;
    grid::draw_grid(f, app, area);
    app.layout.formula_popup_rect = overlays::compute_formula_popup_rect(app, screen);
}

fn draw_divider(f: &mut Frame<'_>, app: &App, area: Rect, vertical: bool) {
    let fg = if matches!(app.drag, DragState::ResizingDock) {
        app.palette.accent
    } else {
        app.palette.border
    };
    let style = Style::default().bg(app.palette.surface).fg(fg);
    let buf = f.buffer_mut();
    if vertical {
        for y in area.y..area.bottom() {
            buf.set_string(area.x, y, "┃", style);
        }
    } else {
        let line = "━".repeat(area.width as usize);
        buf.set_string(area.x, area.y, &line, style);
    }
}

fn draw_sheet_tabs(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let p = app.palette;
    fill_rect(
        f.buffer_mut(),
        area,
        Style::default().bg(p.header_bg).fg(p.header_fg),
    );

    let active = app.wb.active_sheet();
    let mut x = area.x + 1;
    for (idx, name) in app.wb.visible_sheets() {
        let label = format!(" {name} ");
        let w = label.chars().count() as u16;
        if x + w + 6 > area.right() {
            break;
        }
        let rect = Rect::new(x, area.y, w, 1);
        app.layout.sheet_tabs.push((idx, rect));
        let st = if idx == active {
            Style::default()
                .bg(p.grid_bg)
                .fg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.header_bg).fg(p.muted)
        };
        f.buffer_mut().set_string(x, area.y, &label, st);
        x += w + 1;
    }

    if x + 4 <= area.right() {
        let add_r = Rect::new(x, area.y, 3, 1);
        app.layout.add_sheet_btn = Some(add_r);
        f.buffer_mut().set_string(
            x,
            area.y,
            " + ",
            Style::default()
                .bg(p.surface)
                .fg(p.accent)
                .add_modifier(Modifier::BOLD),
        );
        x += 4;
    }

    let [sr1, sc1, sr2, sc2] = app.wb.selected_range();
    if sr1 != sr2 || sc1 != sc2 {
        let stats = app.cached_selection_stats();
        if stats.non_empty > 1 {
            let stat_str = if let Some(avg) = stats.avg() {
                format!(
                    " CNT: {}  SUM: {:.2}  AVG: {:.2}  MIN: {:.2}  MAX: {:.2} ",
                    stats.non_empty, stats.sum, avg, stats.min, stats.max
                )
            } else {
                format!(" CNT: {} ", stats.non_empty)
            };
            let sw = stat_str.chars().count() as u16;
            if area.right().saturating_sub(x) >= sw {
                f.buffer_mut().set_string(
                    area.right() - sw,
                    area.y,
                    &stat_str,
                    Style::default().bg(p.header_bg).fg(p.text),
                );
            }
        }
    }
}

fn draw_status_bar(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let p = app.palette;
    fill_rect(
        f.buffer_mut(),
        area,
        Style::default().bg(p.status_bg).fg(p.status_fg),
    );

    let (mode_txt, badge_bg) = if app.edit.is_some() {
        (" EDIT ", p.fill_preview)
    } else {
        (" READY ", p.accent)
    };
    let bw = mode_txt.chars().count() as u16;
    f.buffer_mut().set_string(
        area.x,
        area.y,
        mode_txt,
        Style::default()
            .bg(badge_bg)
            .fg(Color::Rgb(10, 14, 20))
            .add_modifier(Modifier::BOLD),
    );

    let sheet_name = app.wb.active_sheet_name();
    let (fr, fc) = app.wb.frozen_counts(app.wb.active_sheet());
    let freeze_tag = if fr > 0 || fc > 0 {
        format!(" [❄ {fr}R×{fc}C]")
    } else {
        String::new()
    };
    let filter_tag = if app.wb.active_autofilter(&sheet_name).is_some() {
        " [▾ Filter]"
    } else {
        ""
    };
    let file_label = app
        .wb
        .file_path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("Untitled.xlsx");
    let dirty_mark = if app.wb.dirty { " ●" } else { "" };
    let file_tag = format!(" {file_label}{dirty_mark} ");
    let fw = file_tag.chars().count() as u16;
    let right_reserved = if area.width > bw + fw + 12 {
        f.buffer_mut().set_string(
            area.right() - fw,
            area.y,
            &file_tag,
            Style::default().bg(p.status_bg).fg(p.muted),
        );
        fw
    } else {
        0
    };

    let msg = format!("{freeze_tag}{filter_tag} {}", app.status);
    let max_chars = area.width.saturating_sub(bw + right_reserved + 1) as usize;
    let clipped: String = msg.chars().take(max_chars).collect();
    f.buffer_mut().set_string(
        area.x + bw,
        area.y,
        &clipped,
        Style::default().bg(p.status_bg).fg(p.status_fg),
    );
}

fn compute_dropdown_rect(app: &App, menubar: Rect) -> Option<Rect> {
    let tm = app.top_menu?;
    let menu = TOP_MENUS.get(tm.menu_idx)?;
    let mut x = menubar.x + 1;
    for item in TOP_MENUS.iter().take(tm.menu_idx) {
        x += item.title.chars().count() as u16 + 3;
    }
    let sep_count = menu.entries.iter().filter(|e| e.separator_after).count() as u16;
    let h =
        (menu.entries.len() as u16 + sep_count + 2).min(app.layout.screen.height.saturating_sub(1));
    let w = menu
        .entries
        .iter()
        .map(|e| e.label.chars().count() + e.shortcut.chars().count() + 6)
        .max()
        .unwrap_or(20) as u16;
    let clamped_x = x.min(app.layout.screen.right().saturating_sub(w));
    Some(Rect::new(clamped_x, menubar.y + 1, w, h))
}

fn draw_top_dropdown(f: &mut Frame<'_>, app: &mut App, rect: Rect) {
    let Some(tm) = app.top_menu else {
        return;
    };
    let menu = &TOP_MENUS[tm.menu_idx];
    draw_menu_popup(
        f,
        &app.palette,
        rect,
        menu.entries,
        tm.selected,
        &mut app.layout.dropdown_items,
    );
}

fn compute_context_rect(app: &App, screen: Rect) -> Option<Rect> {
    let cm = app.context_menu.as_ref()?;
    let sep_count = cm.entries.iter().filter(|e| e.separator_after).count() as u16;
    let h = (cm.entries.len() as u16 + sep_count + 2).min(screen.height);
    let w = cm
        .entries
        .iter()
        .map(|e| e.label.chars().count() + e.shortcut.chars().count() + 6)
        .max()
        .unwrap_or(22) as u16;
    let x = cm.x.min(screen.right().saturating_sub(w));
    let y = cm.y.min(screen.bottom().saturating_sub(h));
    Some(Rect::new(x, y, w, h))
}

fn draw_context_menu(f: &mut Frame<'_>, app: &mut App, rect: Rect) {
    let Some(cm) = app.context_menu.clone() else {
        return;
    };
    draw_menu_popup(
        f,
        &app.palette,
        rect,
        &cm.entries,
        cm.selected,
        &mut app.layout.context_items,
    );
}

fn draw_menu_popup(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    entries: &[MenuEntry],
    selected: usize,
    hit_out: &mut Vec<(usize, Rect)>,
) {
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.menu_bg).fg(p.menu_fg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let buf = f.buffer_mut();
    let mut y = inner.y;
    for (idx, entry) in entries.iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        let row_r = Rect::new(inner.x, y, inner.width, 1);
        hit_out.push((idx, row_r));
        let is_sel = idx == selected;
        let st = if is_sel {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.menu_bg).fg(p.menu_fg)
        };
        fill_rect(buf, row_r, st);
        buf.set_string(inner.x + 1, y, entry.label, st);
        if !entry.shortcut.is_empty() {
            let sw = entry.shortcut.chars().count() as u16;
            if inner.width > sw + 2 {
                let sc_st = if is_sel {
                    st
                } else {
                    Style::default().bg(p.menu_bg).fg(p.muted)
                };
                buf.set_string(inner.right() - sw - 1, y, entry.shortcut, sc_st);
            }
        }
        y += 1;
        if entry.separator_after && y < inner.bottom() {
            let sep = "─".repeat(inner.width as usize);
            buf.set_string(
                inner.x,
                y,
                &sep,
                Style::default().bg(p.menu_bg).fg(p.border),
            );
            y += 1;
        }
    }
}

pub fn filter_palette_commands(query: &str) -> Vec<(&'static str, &'static str, Action)> {
    let all = all_palette_commands();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return all;
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    all.into_iter()
        .filter(|(lbl, sc, _)| {
            let hay = format!("{} {}", lbl.to_lowercase(), sc.to_lowercase());
            tokens.iter().all(|t| hay.contains(t))
        })
        .collect()
}

pub(super) fn modal_block<'a>(title: &'a str, p: &Palette) -> Block<'a> {
    Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.surface).fg(p.text))
}

pub(super) fn blend_rgb(a: Color, b: Color, t: f64) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return b;
    };
    let f = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 { (x as f64 + (y as f64 - x as f64) * f).round() as u8 };
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

pub(super) fn fill_rect(buf: &mut Buffer, rect: Rect, style: Style) {
    let blank = " ".repeat(rect.width as usize);
    for y in rect.y..rect.bottom() {
        buf.set_string(rect.x, y, &blank, style);
    }
}

pub(super) fn center_pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        return s.chars().take(width).collect();
    }
    let left = (width - len) / 2;
    let right = width - len - left;
    format!("{}{}{}", " ".repeat(left), s, " ".repeat(right))
}

pub(super) fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_chars {
        let mut out: String = chars
            .into_iter()
            .take(max_chars.saturating_sub(1))
            .collect();
        out.push('…');
        out
    } else {
        s.to_string()
    }
}

pub(super) fn align_cell_text(s: &str, width: usize, align: Alignment) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > width {
        return truncate_with_ellipsis(s, width);
    }
    let pad = width - chars.len();
    match align {
        Alignment::Left => format!("{s}{}", " ".repeat(pad)),
        Alignment::Right => format!("{}{s}", " ".repeat(pad)),
        Alignment::Center => {
            let l = pad / 2;
            let r = pad - l;
            format!("{}{s}{}", " ".repeat(l), " ".repeat(r))
        }
    }
}

pub fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x < b.right() && a.right() > b.x && a.y < b.bottom() && a.bottom() > b.y
}

pub fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

pub fn chart_kind_prev(kind: ChartKind) -> ChartKind {
    kind.prev()
}
