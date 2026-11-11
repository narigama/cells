use super::{edit_single_line, step_selection};
use crate::app::{App, current_date_iso, current_time_hm};
use crate::formula::{accept_completion, cycle_formula_ref_at_cursor};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle_edit_key(app: &mut App, key: KeyEvent) {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let cand_len = app.formula_assist.candidates.len();

    match key.code {
        KeyCode::Esc => {
            app.cancel_edit();
            return;
        }
        KeyCode::F(4) => {
            if let Some((mut buf, mut cursor)) =
                app.edit.as_ref().map(|e| (e.buf.clone(), e.cursor))
                && cycle_formula_ref_at_cursor(&app.wb, &mut buf, &mut cursor)
                && let Some(ref mut ed) = app.edit
            {
                ed.buf = buf;
                ed.cursor = cursor;
                ed.formula_ref_span = None;
            }
            app.refresh_formula_state();
            return;
        }
        KeyCode::Null => {
            accept_or_force_completion(app, cand_len, true);
            return;
        }
        KeyCode::Char(' ') if ctrl => {
            accept_or_force_completion(app, cand_len, true);
            return;
        }
        KeyCode::Up if cand_len > 0 => {
            if let Some(ref mut ed) = app.edit {
                ed.completion_idx = (ed.completion_idx + cand_len - 1) % cand_len;
            }
            return;
        }
        KeyCode::Down if cand_len > 0 => {
            if let Some(ref mut ed) = app.edit {
                ed.completion_idx = (ed.completion_idx + 1) % cand_len;
            }
            return;
        }
        KeyCode::PageUp if cand_len > 0 => {
            let page = completion_page_step(app, cand_len);
            if let Some(ref mut ed) = app.edit {
                ed.completion_idx = (ed.completion_idx + cand_len - page) % cand_len;
            }
            return;
        }
        KeyCode::PageDown if cand_len > 0 => {
            let page = completion_page_step(app, cand_len);
            if let Some(ref mut ed) = app.edit {
                ed.completion_idx = (ed.completion_idx + page) % cand_len;
            }
            return;
        }
        KeyCode::Tab if cand_len > 0 && !shift => {
            accept_or_force_completion(app, cand_len, false);
            return;
        }
        KeyCode::Enter => {
            if ctrl {
                app.commit_edit_fill_selection();
            } else {
                app.commit_edit();
                step_selection(app, if shift { -1 } else { 1 }, 0, false);
            }
            return;
        }
        KeyCode::Tab => {
            app.commit_edit();
            step_selection(app, 0, if shift { -1 } else { 1 }, false);
            return;
        }
        KeyCode::BackTab => {
            app.commit_edit();
            step_selection(app, 0, -1, false);
            return;
        }
        KeyCode::Char(';' | ':') if ctrl => {
            let stamp = if shift || key.code == KeyCode::Char(':') {
                current_time_hm()
            } else {
                current_date_iso()
            };
            if let Some(ref mut ed) = app.edit {
                ed.buf.insert_str(ed.cursor, &stamp);
                ed.cursor += stamp.len();
                ed.formula_ref_span = None;
                ed.force_completion = false;
            }
        }
        _ => {
            if let Some(ref mut ed) = app.edit
                && let Some(modified) = edit_single_line(&mut ed.buf, &mut ed.cursor, key)
            {
                ed.formula_ref_span = None;
                ed.force_completion = false;
                if modified {
                    ed.completion_idx = 0;
                    ed.completion_scroll = 0;
                }
            }
        }
    }
    app.refresh_formula_state();
}

fn accept_or_force_completion(app: &mut App, cand_len: usize, force_when_empty: bool) {
    if cand_len > 0 {
        let assist = app.formula_assist.clone();
        if let Some(ref mut ed) = app.edit {
            let idx = ed.completion_idx.min(cand_len - 1);
            if accept_completion(&mut ed.buf, &mut ed.cursor, &assist, idx) {
                ed.completion_idx = 0;
                ed.completion_scroll = 0;
                ed.force_completion = false;
                ed.formula_ref_span = None;
            }
        }
    } else if force_when_empty
        && let Some(ref mut ed) = app.edit
        && ed.buf.starts_with('=')
    {
        ed.force_completion = true;
    }
    app.refresh_formula_state();
}

fn completion_page_step(app: &App, cand_len: usize) -> usize {
    app.layout
        .formula_assist_items
        .len()
        .saturating_sub(1)
        .max(6)
        .min(cand_len)
}
