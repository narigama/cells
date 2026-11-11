use super::{fill_rect, modal_block, truncate_with_ellipsis};
use crate::app::FilePickerLayout;
use crate::config::Palette;
use crate::file_picker::{FilePickerState, format_file_size};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

pub(super) fn draw_file_picker_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    state: &mut FilePickerState,
    rect: Rect,
) -> FilePickerLayout {
    let mut fp = FilePickerLayout::default();
    let block = modal_block(state.purpose.title(), p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    fill_rect(
        f.buffer_mut(),
        inner,
        Style::default().bg(p.surface).fg(p.text),
    );
    if inner.height < 6 || inner.width < 24 {
        return fp;
    }

    // 1. Top toolbar: [↑ Up] [~ Home]  ~/path/to/dir              [.* Hidden] [Filter]
    let top_y = inner.y;
    let up_lbl = "[↑ Up]";
    let home_lbl = "[~ Home]";
    let mut lx = inner.x + 1;
    let up_w = up_lbl.chars().count() as u16;
    let home_w = home_lbl.chars().count() as u16;
    fp.btn_up = Rect::new(lx, top_y, up_w, 1);
    f.buffer_mut().set_string(
        lx,
        top_y,
        up_lbl,
        Style::default().bg(p.header_bg).fg(p.accent),
    );
    lx += up_w + 1;
    fp.btn_home = Rect::new(lx, top_y, home_w, 1);
    f.buffer_mut().set_string(
        lx,
        top_y,
        home_lbl,
        Style::default().bg(p.header_bg).fg(p.text),
    );
    lx += home_w + 2;

    let hid_lbl = if state.show_hidden { "[.* ON]" } else { "[.*]" };
    let all_lbl = format!(
        "[{}]",
        state.purpose.filter_badge_label(state.show_all_files)
    );
    let hid_w = hid_lbl.chars().count() as u16;
    let all_w = all_lbl.chars().count() as u16;
    let right_need = hid_w + 1 + all_w + 1;
    if inner.width > right_need + (lx - inner.x) {
        let all_x = inner.right() - all_w - 1;
        let hid_x = all_x - 1 - hid_w;
        fp.btn_hidden = Rect::new(hid_x, top_y, hid_w, 1);
        fp.btn_all = Rect::new(all_x, top_y, all_w, 1);
        f.buffer_mut().set_string(
            hid_x,
            top_y,
            hid_lbl,
            Style::default()
                .bg(p.header_bg)
                .fg(if state.show_hidden { p.accent } else { p.muted }),
        );
        f.buffer_mut().set_string(
            all_x,
            top_y,
            &all_lbl,
            Style::default()
                .bg(p.header_bg)
                .fg(if state.show_all_files {
                    p.accent
                } else {
                    p.formula_fg
                }),
        );
        let max_cwd = hid_x.saturating_sub(lx + 1) as usize;
        let cwd_txt = state.display_cwd(max_cwd);
        f.buffer_mut().set_string(
            lx,
            top_y,
            &cwd_txt,
            Style::default()
                .bg(p.surface)
                .fg(p.text)
                .add_modifier(Modifier::BOLD),
        );
    }

    // 2. Column header row.
    let hdr_y = inner.y + 1;
    fill_rect(
        f.buffer_mut(),
        Rect::new(inner.x, hdr_y, inner.width, 1),
        Style::default().bg(p.header_bg).fg(p.header_fg),
    );
    f.buffer_mut().set_string(
        inner.x + 2,
        hdr_y,
        "Name",
        Style::default()
            .bg(p.header_bg)
            .fg(p.header_fg)
            .add_modifier(Modifier::BOLD),
    );
    let col_hdr_right = "Type        Size ";
    if inner.width > col_hdr_right.len() as u16 + 10 {
        f.buffer_mut().set_string(
            inner.right() - col_hdr_right.len() as u16 - 1,
            hdr_y,
            col_hdr_right,
            Style::default().bg(p.header_bg).fg(p.muted),
        );
    }

    // 3. Directory & file list.
    let list_y = inner.y + 2;
    let list_bottom = inner.bottom().saturating_sub(3);
    let list_h = list_bottom.saturating_sub(list_y);
    fp.list_rect = Rect::new(inner.x, list_y, inner.width, list_h);

    let vis_cap = list_h as usize;
    if vis_cap > 0 {
        if state.selected < state.scroll {
            state.scroll = state.selected;
        } else if state.selected >= state.scroll + vis_cap {
            state.scroll = state.selected + 1 - vis_cap;
        }
    }

    if state.entries.is_empty() && list_h > 0 {
        f.buffer_mut().set_string(
            inner.x + 2,
            list_y + list_h / 2,
            "No matching items — press Ctrl+A for All Files or Backspace",
            Style::default().bg(p.surface).fg(p.muted),
        );
    } else {
        let total = state.entries.len();
        let has_scroll = total > vis_cap && vis_cap > 0;
        let row_w = inner.width.saturating_sub(u16::from(has_scroll));

        for (row_i, entry) in state
            .entries
            .iter()
            .skip(state.scroll)
            .take(vis_cap)
            .enumerate()
        {
            let idx = state.scroll + row_i;
            let ry = list_y + row_i as u16;
            let row_r = Rect::new(inner.x, ry, row_w, 1);
            fp.row_rects.push((idx, row_r));

            let is_sel = idx == state.selected;
            let row_st = if is_sel && !state.focus_input {
                Style::default()
                    .bg(p.menu_hi_bg)
                    .fg(p.menu_hi_fg)
                    .add_modifier(Modifier::BOLD)
            } else if is_sel {
                Style::default()
                    .bg(p.header_active_bg)
                    .fg(p.header_active_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(p.surface).fg(p.text)
            };
            fill_rect(f.buffer_mut(), row_r, row_st);

            let (icon, disp_name) = if entry.is_parent {
                ("↩ ", ".. (Parent Directory)".to_string())
            } else if entry.is_dir {
                ("▸ ", format!("{}/", entry.name))
            } else {
                ("  ", entry.name.clone())
            };

            let name_st = if is_sel {
                row_st
            } else if entry.is_dir {
                Style::default()
                    .bg(p.surface)
                    .fg(p.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                row_st
            };

            let right_meta_w = 18usize;
            let name_max = (row_w as usize).saturating_sub(right_meta_w + 4).max(6);
            let clipped_name = truncate_with_ellipsis(&disp_name, name_max);

            f.buffer_mut().set_string(inner.x + 1, ry, icon, name_st);
            f.buffer_mut()
                .set_string(inner.x + 3, ry, &clipped_name, name_st);

            if (row_w as usize) > right_meta_w + 10 {
                let size_str = if entry.is_dir {
                    "—".to_string()
                } else {
                    format_file_size(entry.size_bytes)
                };
                let meta_txt = format!("{:<5} {:>10} ", entry.badge, size_str);
                let mx = row_r
                    .right()
                    .saturating_sub(meta_txt.chars().count() as u16);
                let meta_st = if is_sel {
                    row_st
                } else if entry.is_dir {
                    Style::default().bg(p.surface).fg(p.muted)
                } else {
                    Style::default().bg(p.surface).fg(p.formula_fg)
                };
                f.buffer_mut().set_string(mx, ry, &meta_txt, meta_st);
            }

            if has_scroll {
                let thumb_pos = (state.scroll * vis_cap.saturating_sub(1))
                    / total.saturating_sub(vis_cap).max(1);
                let is_thumb = row_i == thumb_pos;
                f.buffer_mut().set_string(
                    inner.right() - 1,
                    ry,
                    if is_thumb { "▐" } else { "│" },
                    Style::default()
                        .bg(p.surface)
                        .fg(if is_thumb { p.accent } else { p.border }),
                );
            }
        }
    }

    // 4. Divider / Error status line.
    let sep_y = inner.bottom().saturating_sub(3);
    if let Some(ref err) = state.error {
        let msg: String = format!(" ⚠ {err}")
            .chars()
            .take(inner.width as usize)
            .collect();
        f.buffer_mut().set_string(
            inner.x,
            sep_y,
            &msg,
            Style::default()
                .bg(p.surface)
                .fg(p.error_fg)
                .add_modifier(Modifier::BOLD),
        );
    } else {
        let sep = "─".repeat(inner.width as usize);
        f.buffer_mut().set_string(
            inner.x,
            sep_y,
            &sep,
            Style::default().bg(p.surface).fg(p.border),
        );
        let item_count = state.entries.iter().filter(|e| !e.is_parent).count();
        let tag = format!(" {item_count} item(s) ");
        if inner.width > tag.len() as u16 + 4 {
            f.buffer_mut().set_string(
                inner.right() - tag.len() as u16 - 2,
                sep_y,
                &tag,
                Style::default().bg(p.surface).fg(p.muted),
            );
        }
    }

    // 5. Filter / Filename input bar.
    let inp_y = inner.bottom().saturating_sub(2);
    let label = if state.purpose.is_save() {
        " File name:   "
    } else {
        " Filter/Path: "
    };
    let lbl_w = label.chars().count() as u16;
    f.buffer_mut().set_string(
        inner.x,
        inp_y,
        label,
        Style::default()
            .bg(p.surface)
            .fg(if state.focus_input || !state.purpose.is_save() {
                p.accent
            } else {
                p.muted
            }),
    );
    let inp_w = inner.width.saturating_sub(lbl_w + 1);
    let inp_r = Rect::new(inner.x + lbl_w, inp_y, inp_w, 1);
    fp.input_rect = inp_r;
    fp.input_text_x = inp_r.x + 1;
    fill_rect(
        f.buffer_mut(),
        inp_r,
        Style::default().bg(p.grid_bg).fg(p.text),
    );
    if state.input.is_empty() && !state.purpose.is_save() {
        let ph: String = "type to filter list or enter ~/path…"
            .chars()
            .take(inp_w.saturating_sub(2) as usize)
            .collect();
        f.buffer_mut().set_string(
            inp_r.x + 1,
            inp_y,
            &ph,
            Style::default().bg(p.grid_bg).fg(p.muted),
        );
    } else {
        let vis_txt =
            visible_input_tail(&state.input, state.cursor, inp_w.saturating_sub(2) as usize).0;
        let inp_st = if state.focus_input || !state.purpose.is_save() {
            Style::default()
                .bg(p.grid_bg)
                .fg(p.text)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.grid_bg).fg(p.text)
        };
        f.buffer_mut()
            .set_string(inp_r.x + 1, inp_y, &vis_txt, inp_st);
    }

    // 6. Footer hints + Confirm / Cancel buttons.
    let foot_y = inner.bottom().saturating_sub(1);
    let confirm_txt = format!(" [ {} ] ", state.purpose.action_label());
    let cancel_txt = " [ Cancel ] ";
    let cw = confirm_txt.chars().count() as u16;
    let xw = cancel_txt.chars().count() as u16;
    let buttons_w = cw + 1 + xw + 1;
    if inner.width > buttons_w + 6 {
        let cancel_x = inner.right() - xw - 1;
        let confirm_x = cancel_x - 1 - cw;
        fp.btn_confirm = Rect::new(confirm_x, foot_y, cw, 1);
        fp.btn_cancel = Rect::new(cancel_x, foot_y, xw, 1);
        f.buffer_mut().set_string(
            confirm_x,
            foot_y,
            &confirm_txt,
            Style::default()
                .bg(p.accent)
                .fg(Color::Rgb(10, 14, 20))
                .add_modifier(Modifier::BOLD),
        );
        f.buffer_mut().set_string(
            cancel_x,
            foot_y,
            cancel_txt,
            Style::default().bg(p.header_bg).fg(p.text),
        );

        let hint_max = confirm_x.saturating_sub(inner.x + 1) as usize;
        let hint: String = " ↑↓ Nav · Enter Confirm · Bksp Up · Tab Focus · Ctrl+H .* · Ctrl+A All"
            .chars()
            .take(hint_max)
            .collect();
        f.buffer_mut().set_string(
            inner.x,
            foot_y,
            &hint,
            Style::default().bg(p.surface).fg(p.muted),
        );
    }

    fp
}

pub(super) fn visible_input_tail(
    input: &str,
    cursor_byte: usize,
    max_chars: usize,
) -> (String, u16) {
    if max_chars == 0 {
        return (String::new(), 0);
    }
    let clamped = cursor_byte.min(input.len());
    let cur_chars = input[..clamped].chars().count();
    let chars: Vec<char> = input.chars().collect();
    let start = cur_chars.saturating_sub(max_chars.saturating_sub(1));
    let shown: String = chars.into_iter().skip(start).take(max_chars).collect();
    let cursor_col = (cur_chars - start) as u16;
    (shown, cursor_col)
}
