mod editing;
mod modals;
mod mouse;

pub use mouse::compute_cursor_shape;

use crate::actions::{Action, TOP_MENUS, cell_context_menu};
use crate::address::{MAX_COLS, MAX_ROWS};
use crate::app::{ActiveTopMenu, App, ContextMenuState, Modal};
use crate::workbook::ChartKind;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ironcalc_base::worksheet::NavigationDirection;
use std::time::Instant;

pub fn handle_event(app: &mut App, ev: Event) {
    match ev {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            app.last_input_at = Instant::now();
            handle_key(app, key);
        }
        Event::Mouse(mouse) => {
            if !matches!(mouse.kind, MouseEventKind::Moved) {
                app.last_input_at = Instant::now();
            }
            mouse::handle_mouse(app, mouse);
        }
        Event::Paste(text) => {
            app.last_input_at = Instant::now();
            handle_bracketed_paste(app, &text);
        }
        Event::Resize(_, _) => {}
        _ => {}
    }
}

fn handle_bracketed_paste(app: &mut App, text: &str) {
    let single_line = text.replace(['\r', '\n'], " ");
    match &mut app.modal {
        Modal::FilePicker(state) => {
            if state.purpose.is_save() {
                state.focus_input = true;
            }
            state.input.insert_str(state.cursor, &single_line);
            state.cursor += single_line.len();
            state.error = None;
            if !state.purpose.is_save() {
                state.refresh_entries(None);
            }
            return;
        }
        Modal::Prompt { value, cursor, .. } => {
            value.insert_str(*cursor, &single_line);
            *cursor += single_line.len();
            return;
        }
        Modal::CommandPalette {
            query, selected, ..
        } => {
            query.push_str(&single_line);
            *selected = 0;
            return;
        }
        _ => {}
    }
    if let Some(ref mut ed) = app.edit {
        ed.buf.insert_str(ed.cursor, &single_line);
        ed.cursor += single_line.len();
        ed.formula_ref_span = None;
        ed.force_completion = false;
    }
    app.refresh_formula_state();
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if !app.modal.is_none() {
        modals::handle_modal_key(app, key);
        return;
    }
    if app.context_menu.is_some() {
        handle_context_menu_key(app, key);
        return;
    }
    if app.top_menu.is_some() {
        handle_top_menu_key(app, key);
        return;
    }

    // Alt+<mnemonic> opens top menus unless editing text (leave Alt+=, Alt+F1, Alt+Arrows/PgUp/PgDn to shortcuts).
    if app.edit.is_none()
        && key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && let KeyCode::Char(ch) = key.code
    {
        if ch == '=' || ch == '+' {
            app.execute_action(Action::AutoSum);
            return;
        }
        let lower = ch.to_ascii_lowercase();
        if let Some(idx) = TOP_MENUS.iter().position(|m| m.alt_key == lower) {
            app.top_menu = Some(ActiveTopMenu {
                menu_idx: idx,
                selected: 0,
            });
            return;
        }
    }

    if app.edit.is_some() {
        editing::handle_edit_key(app, key);
        return;
    }

    if handle_global_shortcut(app, key) {
        return;
    }

    handle_desktop_key(app, key);
}

fn open_keyboard_context_menu(app: &mut App) {
    let (r, c) = app.wb.selected_cell();
    let x = app
        .layout
        .cols
        .iter()
        .find(|cl| cl.col == c)
        .map(|cl| cl.x_start)
        .unwrap_or(app.layout.grid_rect.x + 6);
    let y = app
        .layout
        .rows
        .iter()
        .find(|rl| rl.row == r)
        .map(|rl| rl.y)
        .unwrap_or(app.layout.grid_rect.y + 2);
    app.context_menu = Some(ContextMenuState {
        x,
        y,
        entries: cell_context_menu(),
        selected: 0,
    });
}

fn select_all_excel_cycle(app: &mut App) {
    let (mr, mc) = app.wb.sheet_bounds(app.wb.active_sheet());
    let cur = app.wb.selected_range();
    let data_rng = [1, 1, mr.max(1), mc.max(1)];
    if (mr > 1 || mc > 1) && cur != data_rng && cur != [1, 1, MAX_ROWS, MAX_COLS] {
        app.wb.select_range(1, 1, mr, mc);
    } else {
        app.wb.select_range(1, 1, MAX_ROWS, MAX_COLS);
    }
}

fn copy_cell_above(app: &mut App, evaluated_value: bool) {
    let s = app.wb.active_sheet();
    let (r, c) = app.wb.selected_cell();
    if r <= 1 {
        return;
    }
    let val = if evaluated_value {
        app.wb.cell_display(s, r - 1, c)
    } else {
        app.wb.cell_input_content(s, r - 1, c)
    };
    if app.wb.set_cell_input(s, r, c, val).is_ok() {
        app.record_model_undo();
        let _ = app.wb.reapply_autofilter(s);
        app.bump_data_rev();
    }
}

fn insert_or_delete_selection_axis(app: &mut App, insert: bool) {
    let [r1, _, r2, _] = app.wb.selected_range();
    let full_col = r1 == 1 && r2 == MAX_ROWS;
    let action = match (insert, full_col) {
        (true, true) => Action::InsertColLeft,
        (true, false) => Action::InsertRowAbove,
        (false, true) => Action::DeleteSelectedCols,
        (false, false) => Action::DeleteSelectedRows,
    };
    app.execute_action(action);
}

fn handle_global_shortcut(app: &mut App, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::F(1) => {
            app.execute_action(if alt {
                Action::NewChart(ChartKind::Bar)
            } else {
                Action::ShowHelp
            });
            return true;
        }
        KeyCode::F(2) => {
            app.start_edit_existing();
            return true;
        }
        KeyCode::F(3) => {
            if ctrl {
                app.execute_action(Action::NamedRangesModal);
            } else {
                app.step_search_match(!shift);
            }
            return true;
        }
        KeyCode::F(4) if ctrl => {
            app.execute_action(Action::CloseWorkbook);
            return true;
        }
        KeyCode::F(5) => {
            app.execute_action(Action::GotoCellPrompt);
            return true;
        }
        KeyCode::F(9) => {
            app.execute_action(Action::ToggleDock);
            return true;
        }
        KeyCode::F(10) => {
            if shift {
                open_keyboard_context_menu(app);
            } else {
                app.top_menu = Some(ActiveTopMenu {
                    menu_idx: 0,
                    selected: 0,
                });
            }
            return true;
        }
        KeyCode::Menu => {
            open_keyboard_context_menu(app);
            return true;
        }
        KeyCode::F(11) => {
            app.execute_action(if shift {
                Action::NewSheet
            } else {
                Action::ToggleChartFullscreen
            });
            return true;
        }
        KeyCode::F(12) => {
            app.execute_action(Action::SaveAsPrompt);
            return true;
        }
        KeyCode::PageDown if ctrl => {
            app.execute_action(Action::NextSheet);
            return true;
        }
        KeyCode::PageUp if ctrl => {
            app.execute_action(Action::PrevSheet);
            return true;
        }
        KeyCode::Down if alt && !ctrl => {
            app.execute_action(Action::FilterActiveColumn);
            return true;
        }
        KeyCode::Backspace if ctrl => {
            app.ensure_cursor_visible();
            return true;
        }
        KeyCode::Null => {
            let [_, c1, _, c2] = app.wb.selected_range();
            app.wb.select_range(1, c1, MAX_ROWS, c2);
            return true;
        }
        KeyCode::Char(' ') if ctrl => {
            if shift {
                select_all_excel_cycle(app);
            } else {
                let [_, c1, _, c2] = app.wb.selected_range();
                app.wb.select_range(1, c1, MAX_ROWS, c2);
            }
            return true;
        }
        KeyCode::Char(ch) if ctrl => {
            match ch {
                'p' | 'P' => {
                    app.modal = Modal::CommandPalette {
                        query: String::new(),
                        selected: 0,
                    };
                }
                's' | 'S' => app.execute_action(Action::Save),
                'q' | 'Q' => app.execute_action(Action::Quit),
                'w' | 'W' => app.execute_action(Action::CloseWorkbook),
                'o' | 'O' => app.execute_action(Action::OpenFilePrompt),
                'n' | 'N' => app.execute_action(Action::NewWorkbook),
                'f' | 'F' => app.open_find_replace(false),
                'h' | 'H' => app.open_find_replace(true),
                'g' | 'G' => app.execute_action(Action::GotoCellPrompt),
                'l' | 'L' => app.execute_action(Action::ToggleAutoFilter),
                'b' | 'B' => app.execute_action(Action::ToggleBold),
                'i' | 'I' => app.execute_action(Action::ToggleItalic),
                'u' | 'U' => app.execute_action(Action::ToggleUnderline),
                '1' => {
                    if shift {
                        app.execute_action(Action::NumFmtDecimal);
                    } else if let Some(idx) = TOP_MENUS.iter().position(|m| m.title == "Format") {
                        app.top_menu = Some(ActiveTopMenu {
                            menu_idx: idx,
                            selected: 0,
                        });
                    }
                }
                '2' if !shift => app.execute_action(Action::ToggleBold),
                '3' => app.execute_action(if shift {
                    Action::NumFmtDate
                } else {
                    Action::ToggleItalic
                }),
                '4' => app.execute_action(if shift {
                    Action::NumFmtCurrency
                } else {
                    Action::ToggleUnderline
                }),
                '5' => app.execute_action(if shift {
                    Action::NumFmtPercent
                } else {
                    Action::ToggleStrikethrough
                }),
                '$' => app.execute_action(Action::NumFmtCurrency),
                '%' => app.execute_action(Action::NumFmtPercent),
                '#' => app.execute_action(Action::NumFmtDate),
                '!' => app.execute_action(Action::NumFmtDecimal),
                '~' => app.execute_action(Action::NumFmtGeneral),
                '`' => app.execute_action(if shift {
                    Action::NumFmtGeneral
                } else {
                    Action::ToggleShowFormulas
                }),
                ';' => app.execute_action(if shift {
                    Action::InsertCurrentTime
                } else {
                    Action::InsertCurrentDate
                }),
                ':' => app.execute_action(Action::InsertCurrentTime),
                '\'' => copy_cell_above(app, shift),
                '"' => copy_cell_above(app, true),
                '+' => insert_or_delete_selection_axis(app, true),
                '=' if shift => insert_or_delete_selection_axis(app, true),
                '-' => insert_or_delete_selection_axis(app, false),
                _ => return false,
            }
            return true;
        }
        _ => {}
    }
    false
}

fn navigate_to_edge(app: &mut App, dir: NavigationDirection, extend: bool) {
    if extend {
        let (tr, tc) = app.wb.extend_selection_to_edge(dir);
        app.scroll_into_view_target(tr, tc);
    } else {
        let _ = app.wb.user.on_navigate_to_edge_in_direction(dir);
        app.ensure_cursor_visible();
    }
}

fn jump_or_extend_to(app: &mut App, target_row: Option<i32>, target_col: i32, extend: bool) {
    if extend {
        let tr = target_row.unwrap_or_else(|| app.wb.selected_focus().0);
        app.wb.extend_selection_to(tr, target_col);
        app.scroll_into_view_target(tr, target_col);
    } else {
        let tr = target_row.unwrap_or_else(|| app.wb.selected_cell().0);
        app.wb.select_cell(tr, target_col);
        app.ensure_cursor_visible();
    }
}

fn handle_desktop_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if ctrl {
        match key.code {
            KeyCode::Char('z' | 'Z') => {
                app.execute_action(if shift { Action::Redo } else { Action::Undo });
                return;
            }
            KeyCode::Char('y' | 'Y') => {
                app.execute_action(Action::Redo);
                return;
            }
            KeyCode::Char('c' | 'C') => {
                app.execute_action(Action::Copy);
                return;
            }
            KeyCode::Char('x' | 'X') => {
                app.execute_action(Action::Cut);
                return;
            }
            KeyCode::Char('v' | 'V') => {
                app.execute_action(Action::Paste);
                return;
            }
            KeyCode::Char('d' | 'D') => {
                app.execute_action(Action::FillDown);
                return;
            }
            KeyCode::Char('r' | 'R') => {
                app.execute_action(Action::FillRight);
                return;
            }
            KeyCode::Char('a' | 'A') => {
                select_all_excel_cycle(app);
                return;
            }
            KeyCode::Left => {
                navigate_to_edge(app, NavigationDirection::Left, shift);
                return;
            }
            KeyCode::Right => {
                navigate_to_edge(app, NavigationDirection::Right, shift);
                return;
            }
            KeyCode::Up => {
                navigate_to_edge(app, NavigationDirection::Up, shift);
                return;
            }
            KeyCode::Down => {
                navigate_to_edge(app, NavigationDirection::Down, shift);
                return;
            }
            KeyCode::Home => {
                jump_or_extend_to(app, Some(1), 1, shift);
                return;
            }
            KeyCode::End => {
                let (mr, mc) = app.wb.sheet_bounds(app.wb.active_sheet());
                jump_or_extend_to(app, Some(mr.max(1)), mc.max(1), shift);
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Up => step_selection(app, -1, 0, shift),
        KeyCode::Down => step_selection(app, 1, 0, shift),
        KeyCode::Left => step_selection(app, 0, -1, shift),
        KeyCode::Right => step_selection(app, 0, 1, shift),
        KeyCode::Tab => step_selection(app, 0, if shift { -1 } else { 1 }, false),
        KeyCode::BackTab => step_selection(app, 0, -1, false),
        KeyCode::Enter => step_selection(app, if shift { -1 } else { 1 }, 0, false),
        KeyCode::PageDown => {
            if alt {
                let jump = app.layout.visible_scroll_cols.max(3) as i32;
                step_selection(app, 0, jump, shift);
            } else {
                let jump = app.layout.visible_scroll_rows.max(5) as i32;
                step_selection(app, jump, 0, shift);
            }
        }
        KeyCode::PageUp => {
            if alt {
                let jump = app.layout.visible_scroll_cols.max(3) as i32;
                step_selection(app, 0, -jump, shift);
            } else {
                let jump = app.layout.visible_scroll_rows.max(5) as i32;
                step_selection(app, -jump, 0, shift);
            }
        }
        KeyCode::Home => jump_or_extend_to(app, None, 1, shift),
        KeyCode::End => {
            let (_, mc) = app.wb.sheet_bounds(app.wb.active_sheet());
            jump_or_extend_to(app, None, mc.max(1), shift);
        }
        KeyCode::Delete | KeyCode::Backspace => app.execute_action(Action::ClearContents),
        KeyCode::Esc => {
            let (r, c) = app.wb.selected_cell();
            app.wb.select_cell(r, c);
            app.search_highlights.clear();
        }
        KeyCode::Char(' ') if shift && !alt => {
            let [r1, _, r2, _] = app.wb.selected_range();
            app.wb.select_range(r1, 1, r2, MAX_COLS);
        }
        KeyCode::Char(ch) if !alt => app.start_edit_replace(ch.to_string()),
        _ => {}
    }
}

pub(super) fn step_selection(app: &mut App, dr: i32, dc: i32, extend: bool) {
    if extend {
        let key_name = match (dr.signum(), dc.signum()) {
            (-1, 0) => "ArrowUp",
            (1, 0) => "ArrowDown",
            (0, -1) => "ArrowLeft",
            (0, 1) => "ArrowRight",
            _ => "",
        };
        if !key_name.is_empty() && dr.abs() <= 1 && dc.abs() <= 1 {
            let _ = app.wb.user.on_expand_selected_range(key_name);
            app.ensure_cursor_visible();
            return;
        }
        let (fr, fc) = app.wb.selected_focus();
        let tr = fr.saturating_add(dr).clamp(1, MAX_ROWS);
        let tc = fc.saturating_add(dc).clamp(1, MAX_COLS);
        app.wb.extend_selection_to(tr, tc);
        app.scroll_into_view_target(tr, tc);
        return;
    }
    let sheet = app.wb.active_sheet();
    let (r, c) = app.wb.selected_cell();
    let mut nr = r.saturating_add(dr).clamp(1, MAX_ROWS);
    let nc = c.saturating_add(dc).clamp(1, MAX_COLS);
    if dr != 0 && dc == 0 {
        let step = dr.signum();
        let mut guard = 0;
        while (1..=MAX_ROWS).contains(&nr) && app.wb.is_row_hidden(sheet, nr) && guard < 10_000 {
            let cand = nr + step;
            if !(1..=MAX_ROWS).contains(&cand) {
                nr = r;
                break;
            }
            nr = cand;
            guard += 1;
        }
    }
    app.wb.select_cell(nr, nc);
    app.ensure_cursor_visible();
}

fn handle_top_menu_key(app: &mut App, key: KeyEvent) {
    let Some(mut tm) = app.top_menu else {
        return;
    };
    let entries = TOP_MENUS[tm.menu_idx].entries;
    match key.code {
        KeyCode::Esc | KeyCode::F(10) => app.top_menu = None,
        KeyCode::Left => {
            tm.menu_idx = (tm.menu_idx + TOP_MENUS.len() - 1) % TOP_MENUS.len();
            tm.selected = 0;
            app.top_menu = Some(tm);
        }
        KeyCode::Right => {
            tm.menu_idx = (tm.menu_idx + 1) % TOP_MENUS.len();
            tm.selected = 0;
            app.top_menu = Some(tm);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            tm.selected = (tm.selected + entries.len() - 1) % entries.len();
            app.top_menu = Some(tm);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            tm.selected = (tm.selected + 1) % entries.len();
            app.top_menu = Some(tm);
        }
        KeyCode::Enter => {
            let act = entries[tm.selected].action;
            app.execute_action(act);
        }
        _ => {}
    }
}

fn handle_context_menu_key(app: &mut App, key: KeyEvent) {
    let Some(ref mut cm) = app.context_menu else {
        return;
    };
    let len = cm.entries.len();
    match key.code {
        KeyCode::Esc => app.context_menu = None,
        KeyCode::Up | KeyCode::Char('k') => cm.selected = (cm.selected + len - 1) % len,
        KeyCode::Down | KeyCode::Char('j') => cm.selected = (cm.selected + 1) % len,
        KeyCode::Enter => {
            let act = cm.entries[cm.selected].action;
            app.execute_action(act);
        }
        _ => {}
    }
}

/// Applies standard single-line UTF-8 cursor movement / editing to `(buf, cursor)`.
/// Returns `Some(true)` if `buf` changed, `Some(false)` if only `cursor` moved (or hit boundary),
/// or `None` if `key` is not a standard text-input key.
pub(super) fn edit_single_line(
    buf: &mut String,
    cursor: &mut usize,
    key: KeyEvent,
) -> Option<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Left => {
            *cursor = if ctrl {
                prev_word_boundary(buf, *cursor)
            } else {
                prev_char_boundary(buf, *cursor)
            };
            Some(false)
        }
        KeyCode::Right => {
            *cursor = if ctrl {
                next_word_boundary(buf, *cursor)
            } else {
                next_char_boundary(buf, *cursor)
            };
            Some(false)
        }
        KeyCode::Home => {
            *cursor = 0;
            Some(false)
        }
        KeyCode::End => {
            *cursor = buf.len();
            Some(false)
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                let prev = prev_char_boundary(buf, *cursor);
                buf.drain(prev..*cursor);
                *cursor = prev;
                Some(true)
            } else {
                Some(false)
            }
        }
        KeyCode::Delete => {
            if *cursor < buf.len() {
                let next = next_char_boundary(buf, *cursor);
                buf.drain(*cursor..next);
                Some(true)
            } else {
                Some(false)
            }
        }
        KeyCode::Char(ch) if !ctrl => {
            buf.insert(*cursor, ch);
            *cursor += ch.len_utf8();
            Some(true)
        }
        _ => None,
    }
}

pub(super) fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let clamped = idx.min(s.len());
    s[..clamped]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub(super) fn next_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        s.len()
    } else {
        idx + s[idx..].chars().next().map(char::len_utf8).unwrap_or(0)
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '.')
}

fn prev_word_boundary(s: &str, idx: usize) -> usize {
    let chars: Vec<(usize, char)> = s[..idx.min(s.len())].char_indices().collect();
    let mut i = chars.len();
    while i > 0 && !is_word_char(chars[i - 1].1) {
        i -= 1;
    }
    while i > 0 && is_word_char(chars[i - 1].1) {
        i -= 1;
    }
    chars.get(i).map(|&(b, _)| b).unwrap_or(0)
}

fn next_word_boundary(s: &str, idx: usize) -> usize {
    let start = idx.min(s.len());
    let chars: Vec<(usize, char)> = s[start..].char_indices().collect();
    let mut i = 0;
    while i < chars.len() && is_word_char(chars[i].1) {
        i += 1;
    }
    while i < chars.len() && !is_word_char(chars[i].1) {
        i += 1;
    }
    chars.get(i).map(|&(b, _)| start + b).unwrap_or(s.len())
}

pub(super) fn byte_index_at_char(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}
