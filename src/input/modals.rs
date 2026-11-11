use super::{edit_single_line, next_char_boundary, prev_char_boundary};
use crate::actions::Action;
use crate::address::{col_to_letters, format_range_ref, parse_range_ref};
use crate::app::{App, Modal, PromptKind};
use crate::config::SWATCH_COLORS;
use crate::file_picker::{FilePickerOutcome, FilePickerPurpose, FilePickerState, PostSaveAction};
use crate::ui::filter_palette_commands;
use crate::workbook::{
    ChartSpec, ColumnFilter, NumericFilterOp, PivotAgg, SortSpec, clear_recovery_snapshot,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle_modal_key(app: &mut App, key: KeyEvent) {
    let modal = std::mem::take(&mut app.modal);
    match modal {
        Modal::None => {}
        Modal::CommandPalette { query, selected } => handle_palette_key(app, key, query, selected),
        Modal::Prompt {
            kind,
            title,
            value,
            cursor,
        } => handle_prompt_key(app, key, kind, title, value, cursor),
        Modal::FindReplace {
            find,
            replace,
            field,
            case_sensitive,
            all_sheets,
            matches,
            match_idx,
        } => handle_find_replace_key(
            app,
            key,
            find,
            replace,
            field,
            case_sensitive,
            all_sheets,
            matches,
            match_idx,
        ),
        Modal::ColorPicker {
            is_fill,
            selected,
            custom_hex,
            editing_hex,
        } => handle_color_picker_key(app, key, is_fill, selected, custom_hex, editing_hex),
        Modal::ChartEditor { idx, draft, field } => {
            handle_chart_editor_key(app, key, idx, draft, field)
        }
        Modal::NamedRanges {
            selected,
            name_input,
            ref_input,
            field,
        } => handle_named_ranges_key(app, key, selected, name_input, ref_input, field),
        Modal::ColumnFilter {
            sheet,
            col,
            draft,
            num_buf,
            values,
            selected_val,
            field,
        } => handle_column_filter_key(
            app,
            key,
            sheet,
            col,
            draft,
            num_buf,
            values,
            selected_val,
            field,
        ),
        Modal::MultiSort { rules, selected } => handle_multi_sort_key(app, key, rules, selected),
        Modal::Pivot {
            group_col,
            val_col,
            agg,
            field,
        } => handle_pivot_key(app, key, group_col, val_col, agg, field),
        Modal::Help { scroll } => handle_help_key(app, key, scroll),
        Modal::FilePicker(state) => handle_file_picker_key(app, key, state),
        Modal::ConfirmQuit { quit_app } => handle_confirm_quit_key(app, key, quit_app),
    }
}

fn handle_palette_key(app: &mut App, key: KeyEvent, mut query: String, mut selected: usize) {
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Up => {
            let n = filter_palette_commands(&query).len().max(1);
            selected = (selected + n - 1) % n;
        }
        KeyCode::Down | KeyCode::Tab => {
            let n = filter_palette_commands(&query).len().max(1);
            selected = (selected + 1) % n;
        }
        KeyCode::Backspace => {
            query.pop();
            selected = 0;
        }
        KeyCode::Enter => {
            let cmds = filter_palette_commands(&query);
            if let Some(&(_, _, act)) = cmds.get(selected).or_else(|| cmds.first()) {
                app.execute_action(act);
            } else if !query.trim().is_empty() {
                let _ = app.jump_to_target(query.trim());
            }
            return;
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            query.push(ch);
            selected = 0;
        }
        _ => {}
    }
    app.modal = Modal::CommandPalette { query, selected };
}

fn handle_prompt_key(
    app: &mut App,
    key: KeyEvent,
    kind: PromptKind,
    title: &'static str,
    mut value: String,
    mut cursor: usize,
) {
    match key.code {
        KeyCode::Esc => {}
        KeyCode::Enter => app.submit_prompt(kind, value),
        _ => {
            let _ = edit_single_line(&mut value, &mut cursor, key);
            app.modal = Modal::Prompt {
                kind,
                title,
                value,
                cursor,
            };
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_find_replace_key(
    app: &mut App,
    key: KeyEvent,
    mut find: String,
    mut replace: String,
    mut field: usize,
    mut case_sensitive: bool,
    mut all_sheets: bool,
    mut matches: Vec<(u32, i32, i32)>,
    mut match_idx: usize,
) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Tab | KeyCode::Up | KeyCode::Down => field = 1 - field,
        KeyCode::Char('c' | 'C') if alt => {
            case_sensitive = !case_sensitive;
            matches = app.wb.find_matches(&find, all_sheets, case_sensitive);
            match_idx = 0;
            sync_find_state(app, &find, case_sensitive, all_sheets, &matches, match_idx);
        }
        KeyCode::Char('a' | 'A') if alt => {
            all_sheets = !all_sheets;
            matches = app.wb.find_matches(&find, all_sheets, case_sensitive);
            match_idx = 0;
            sync_find_state(app, &find, case_sensitive, all_sheets, &matches, match_idx);
        }
        KeyCode::Char('r' | 'R') if ctrl => {
            match app
                .wb
                .replace_matches(&find, &replace, all_sheets, case_sensitive)
            {
                Ok(n) => {
                    if n > 0 {
                        app.record_model_undo();
                    }
                    app.bump_data_rev();
                    app.status = format!("Replaced {n} cell(s)");
                    matches = app.wb.find_matches(&find, all_sheets, case_sensitive);
                    match_idx = 0;
                    sync_find_state(app, &find, case_sensitive, all_sheets, &matches, match_idx);
                }
                Err(e) => app.status = format!("Error: {e}"),
            }
        }
        KeyCode::Enter => {
            if !matches.is_empty() {
                match_idx = (match_idx + 1) % matches.len();
                let (s, r, c) = matches[match_idx];
                let _ = app.wb.select_sheet(s);
                app.wb.select_cell(r, c);
                app.ensure_cursor_visible();
            }
        }
        KeyCode::Backspace => {
            if field == 0 {
                find.pop();
                matches = app.wb.find_matches(&find, all_sheets, case_sensitive);
                match_idx = 0;
                sync_find_state(app, &find, case_sensitive, all_sheets, &matches, 0);
            } else {
                replace.pop();
            }
        }
        KeyCode::Char(ch) if !ctrl && !alt => {
            if field == 0 {
                find.push(ch);
                matches = app.wb.find_matches(&find, all_sheets, case_sensitive);
                match_idx = 0;
                sync_find_state(app, &find, case_sensitive, all_sheets, &matches, 0);
            } else {
                replace.push(ch);
            }
        }
        _ => {}
    }
    app.modal = Modal::FindReplace {
        find,
        replace,
        field,
        case_sensitive,
        all_sheets,
        matches,
        match_idx,
    };
}

fn handle_color_picker_key(
    app: &mut App,
    key: KeyEvent,
    is_fill: bool,
    mut selected: usize,
    mut custom_hex: String,
    mut editing_hex: bool,
) {
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Tab => editing_hex = !editing_hex,
        KeyCode::Up if !editing_hex => {
            selected = (selected + SWATCH_COLORS.len() - 1) % SWATCH_COLORS.len();
        }
        KeyCode::Down if !editing_hex => {
            selected = (selected + 1) % SWATCH_COLORS.len();
        }
        KeyCode::Backspace if editing_hex => {
            custom_hex.pop();
        }
        KeyCode::Char(ch) if editing_hex => {
            if custom_hex.len() < 9 {
                custom_hex.push(ch);
            }
        }
        KeyCode::Enter => {
            let pick = if editing_hex {
                SWATCH_COLORS.len()
            } else {
                selected
            };
            app.apply_color_choice(is_fill, pick, &custom_hex);
            return;
        }
        _ => {}
    }
    app.modal = Modal::ColorPicker {
        is_fill,
        selected,
        custom_hex,
        editing_hex,
    };
}

fn handle_chart_editor_key(
    app: &mut App,
    key: KeyEvent,
    idx: usize,
    mut draft: ChartSpec,
    mut field: usize,
) {
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Up | KeyCode::BackTab => field = (field + 5) % 6,
        KeyCode::Down | KeyCode::Tab => field = (field + 1) % 6,
        KeyCode::Left if field == 1 => draft.kind = draft.kind.prev(),
        KeyCode::Right if field == 1 => draft.kind = draft.kind.next(),
        KeyCode::Char(' ') if matches!(field, 1 | 3 | 4 | 5) => match field {
            1 => draft.kind = draft.kind.next(),
            3 => draft.has_header = !draft.has_header,
            4 => draft.show_legend = !draft.show_legend,
            5 => draft.show_grid = !draft.show_grid,
            _ => {}
        },
        KeyCode::Backspace if matches!(field, 0 | 2) => {
            if field == 0 {
                draft.title.pop();
            } else {
                draft.range.pop();
            }
        }
        KeyCode::Char(ch)
            if matches!(field, 0 | 2) && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            if field == 0 {
                draft.title.push(ch);
            } else {
                draft.range.push(ch);
            }
        }
        KeyCode::Enter => {
            if let Some((sn, bounds)) = parse_range_ref(&draft.sheet_name, &draft.range) {
                draft.sheet_name = sn;
                draft.range = format_range_ref(None, bounds[0], bounds[1], bounds[2], bounds[3]);
            }
            if idx < app.wb.charts.len() {
                app.record_aux_undo();
                app.wb.charts[idx] = draft;
                app.wb.dirty = true;
                app.bump_data_rev();
                app.status = "Updated chart configuration".into();
            }
            return;
        }
        _ => {}
    }
    app.modal = Modal::ChartEditor { idx, draft, field };
}

fn handle_named_ranges_key(
    app: &mut App,
    key: KeyEvent,
    mut selected: usize,
    mut name_input: String,
    mut ref_input: String,
    mut field: usize,
) {
    let names = app.wb.defined_names();
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Tab => {
            field = (field + 1) % 3;
            if field == 0 && names.is_empty() {
                field = 1;
            }
        }
        KeyCode::BackTab => {
            field = (field + 2) % 3;
            if field == 0 && names.is_empty() {
                field = 2;
            }
        }
        KeyCode::Up if field == 0 && !names.is_empty() => {
            selected = (selected + names.len() - 1) % names.len();
        }
        KeyCode::Down if field == 0 && !names.is_empty() => {
            selected = (selected + 1) % names.len();
        }
        KeyCode::Delete if field == 0 && !names.is_empty() => {
            let sel = selected.min(names.len() - 1);
            let (ref n, scope, _) = names[sel];
            match app.wb.remove_defined_name(n, scope) {
                Ok(()) => {
                    app.record_model_undo();
                    app.bump_data_rev();
                    app.status = format!("Deleted named range '{n}'");
                    selected = selected.saturating_sub(1);
                }
                Err(e) => app.status = format!("Error: {e}"),
            }
        }
        KeyCode::Backspace if field == 1 => {
            name_input.pop();
        }
        KeyCode::Backspace if field == 2 => {
            ref_input.pop();
        }
        KeyCode::Char(ch)
            if (field == 1 || field == 2) && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            if field == 1 {
                name_input.push(ch);
            } else {
                ref_input.push(ch);
            }
        }
        KeyCode::Enter => {
            if field == 0 && name_input.trim().is_empty() {
                if let Some((n, _, _)) = names.get(selected) {
                    let _ = app.jump_to_target(n);
                }
                return;
            }
            match app.wb.add_or_update_defined_name(&name_input, &ref_input) {
                Ok(()) => {
                    app.record_model_undo();
                    app.bump_data_rev();
                    app.status = format!("Saved named range '{}'", name_input.trim());
                    return;
                }
                Err(e) => app.status = format!("Error: {e}"),
            }
        }
        _ => {}
    }
    app.modal = Modal::NamedRanges {
        selected,
        name_input,
        ref_input,
        field,
    };
}

#[allow(clippy::too_many_arguments)]
fn handle_column_filter_key(
    app: &mut App,
    key: KeyEvent,
    sheet: u32,
    col: i32,
    mut draft: ColumnFilter,
    mut num_buf: String,
    values: Vec<String>,
    mut selected_val: usize,
    mut field: usize,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Tab => field = (field + 1) % 3,
        KeyCode::BackTab => field = (field + 2) % 3,
        KeyCode::Char('x' | 'X') if ctrl => {
            draft = ColumnFilter::default();
            num_buf.clear();
        }
        KeyCode::Backspace if field == 0 => {
            draft.substring.pop();
        }
        KeyCode::Char(ch) if field == 0 && !ctrl => {
            draft.substring.push(ch);
        }
        KeyCode::Left if field == 1 => {
            draft.numeric_op = match draft.numeric_op {
                NumericFilterOp::None => NumericFilterOp::Top10,
                NumericFilterOp::Gt => NumericFilterOp::None,
                NumericFilterOp::Lt => NumericFilterOp::Gt,
                NumericFilterOp::Eq => NumericFilterOp::Lt,
                NumericFilterOp::Top10 => NumericFilterOp::Eq,
            };
        }
        KeyCode::Right | KeyCode::Char(' ') if field == 1 => {
            draft.numeric_op = draft.numeric_op.next();
        }
        KeyCode::Backspace if field == 1 => {
            num_buf.pop();
        }
        KeyCode::Char(ch)
            if field == 1 && !ctrl && (ch.is_ascii_digit() || ch == '.' || ch == '-') =>
        {
            num_buf.push(ch);
        }
        KeyCode::Up if field == 2 && !values.is_empty() => {
            selected_val = (selected_val + values.len() - 1) % values.len();
        }
        KeyCode::Down if field == 2 && !values.is_empty() => {
            selected_val = (selected_val + 1) % values.len();
        }
        KeyCode::Char(' ') if field == 2 && !values.is_empty() => {
            let sel = selected_val.min(values.len() - 1);
            let v = &values[sel];
            if !draft.excluded_values.remove(v) {
                draft.excluded_values.insert(v.clone());
            }
        }
        KeyCode::Char('a' | 'A') if field == 2 && ctrl => {
            if draft.excluded_values.is_empty() {
                draft.excluded_values = values.iter().cloned().collect();
            } else {
                draft.excluded_values.clear();
            }
        }
        KeyCode::Enter => {
            draft.numeric_val = num_buf.trim().parse::<f64>().unwrap_or(0.0);
            let sheet_name = app.wb.active_sheet_name();
            if let Some(af) = app.wb.active_autofilter_mut(&sheet_name) {
                if draft.is_active() {
                    af.col_filters.insert(col, draft);
                } else {
                    af.col_filters.remove(&col);
                }
            }
            match app.wb.reapply_autofilter(sheet) {
                Ok(hidden) => {
                    app.bump_data_rev();
                    app.status = format!(
                        "Filtered column {} ({hidden} row(s) hidden)",
                        col_to_letters(col)
                    );
                }
                Err(e) => app.status = format!("Filter error: {e}"),
            }
            return;
        }
        _ => {}
    }
    app.modal = Modal::ColumnFilter {
        sheet,
        col,
        draft,
        num_buf,
        values,
        selected_val,
        field,
    };
}

fn handle_multi_sort_key(
    app: &mut App,
    key: KeyEvent,
    mut rules: Vec<SortSpec>,
    mut selected: usize,
) {
    let (_, max_c) = app.wb.sheet_bounds(app.wb.active_sheet());
    let max_col = max_c.max(app.wb.selected_range()[3]).max(1);
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Up if !rules.is_empty() => {
            selected = (selected + rules.len() - 1) % rules.len();
        }
        KeyCode::Down if !rules.is_empty() => {
            selected = (selected + 1) % rules.len();
        }
        KeyCode::Left if !rules.is_empty() => {
            let sel = selected.min(rules.len() - 1);
            rules[sel].col = if rules[sel].col <= 1 {
                max_col
            } else {
                rules[sel].col - 1
            };
        }
        KeyCode::Right if !rules.is_empty() => {
            let sel = selected.min(rules.len() - 1);
            rules[sel].col = if rules[sel].col >= max_col {
                1
            } else {
                rules[sel].col + 1
            };
        }
        KeyCode::Char(' ' | 'o' | 'O') if !rules.is_empty() => {
            let sel = selected.min(rules.len() - 1);
            rules[sel].ascending = !rules[sel].ascending;
        }
        KeyCode::Char('+' | 'a' | 'A') if rules.len() < 4 => {
            let next_col = (rules.last().map(|r| r.col).unwrap_or(1) % max_col) + 1;
            rules.push(SortSpec {
                col: next_col,
                ascending: true,
            });
            selected = rules.len() - 1;
        }
        KeyCode::Char('-' | 'd' | 'D') | KeyCode::Delete | KeyCode::Backspace
            if rules.len() > 1 =>
        {
            let sel = selected.min(rules.len() - 1);
            rules.remove(sel);
            selected = selected.min(rules.len() - 1);
        }
        KeyCode::Enter => {
            match app.wb.sort_selection_multi(&rules, true) {
                Ok(()) => {
                    app.record_model_undo();
                    app.bump_data_rev();
                    app.status = format!("Sorted by {} column level(s)", rules.len());
                }
                Err(e) => app.status = format!("Sort error: {e}"),
            }
            return;
        }
        _ => {}
    }
    app.modal = Modal::MultiSort { rules, selected };
}

fn handle_pivot_key(
    app: &mut App,
    key: KeyEvent,
    mut group_col: i32,
    mut val_col: i32,
    mut agg: PivotAgg,
    mut field: usize,
) {
    let (_, max_c) = app.wb.sheet_bounds(app.wb.active_sheet());
    let max_col = max_c.max(app.wb.selected_range()[3]).max(2);
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Tab | KeyCode::Down => field = (field + 1) % 3,
        KeyCode::BackTab | KeyCode::Up => field = (field + 2) % 3,
        KeyCode::Left => match field {
            0 => {
                group_col = if group_col <= 1 {
                    max_col
                } else {
                    group_col - 1
                }
            }
            1 => val_col = if val_col <= 1 { max_col } else { val_col - 1 },
            _ => {
                agg = match agg {
                    PivotAgg::Sum => PivotAgg::Max,
                    PivotAgg::Avg => PivotAgg::Sum,
                    PivotAgg::Count => PivotAgg::Avg,
                    PivotAgg::Min => PivotAgg::Count,
                    PivotAgg::Max => PivotAgg::Min,
                };
            }
        },
        KeyCode::Right | KeyCode::Char(' ') => match field {
            0 => {
                group_col = if group_col >= max_col {
                    1
                } else {
                    group_col + 1
                }
            }
            1 => val_col = if val_col >= max_col { 1 } else { val_col + 1 },
            _ => agg = agg.next(),
        },
        KeyCode::Enter => {
            match app.wb.create_pivot_sheet(group_col, val_col, agg) {
                Ok(name) => {
                    app.record_model_undo();
                    app.bump_data_rev();
                    app.status = format!("Created summary sheet '{name}'");
                }
                Err(e) => app.status = format!("Pivot error: {e}"),
            }
            return;
        }
        _ => {}
    }
    app.modal = Modal::Pivot {
        group_col,
        val_col,
        agg,
        field,
    };
}

fn handle_help_key(app: &mut App, key: KeyEvent, mut scroll: u16) {
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::F(1) => return,
        KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1).min(28),
        KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
        _ => {}
    }
    app.modal = Modal::Help { scroll };
}

fn handle_file_picker_key(app: &mut App, key: KeyEvent, mut state: FilePickerState) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Esc => return,
        KeyCode::Char('h' | 'H') if ctrl => state.toggle_hidden(),
        KeyCode::Char('.') if alt => state.toggle_hidden(),
        KeyCode::Char('a' | 'A') if ctrl || alt => state.toggle_all_files(),
        KeyCode::Char('~') if alt => state.go_home(),
        KeyCode::Char('u' | 'U') if ctrl => {
            state.input.clear();
            state.cursor = 0;
            state.error = None;
            if !state.purpose.is_save() {
                state.refresh_entries(None);
            }
        }
        KeyCode::Char('s' | 'S') if ctrl && state.purpose.is_save() => {
            let outcome = state.confirm_from_button();
            finish_file_picker_outcome(app, outcome, state);
            return;
        }
        KeyCode::Up if alt => state.up_dir(),
        KeyCode::Left if alt => state.up_dir(),
        KeyCode::Up => state.move_selection(-1),
        KeyCode::Down => state.move_selection(1),
        KeyCode::PageUp => state.move_selection(-10),
        KeyCode::PageDown => state.move_selection(10),
        KeyCode::Tab | KeyCode::BackTab => {
            if state.purpose.is_save() {
                state.focus_input = !state.focus_input;
            } else if matches!(key.code, KeyCode::BackTab) || shift {
                state.move_selection(-1);
            } else {
                state.move_selection(1);
            }
        }
        KeyCode::Home => {
            if state.focus_input {
                state.cursor = 0;
            } else {
                state.move_selection(-(state.entries.len() as isize));
            }
        }
        KeyCode::End => {
            if state.focus_input {
                state.cursor = state.input.len();
            } else {
                state.move_selection(state.entries.len() as isize);
            }
        }
        KeyCode::Left => {
            if state.focus_input || (!state.purpose.is_save() && !state.input.is_empty()) {
                state.cursor = prev_char_boundary(&state.input, state.cursor);
            } else {
                state.up_dir();
            }
        }
        KeyCode::Right => {
            if state.focus_input || (!state.purpose.is_save() && !state.input.is_empty()) {
                state.cursor = next_char_boundary(&state.input, state.cursor);
            } else if let Some(entry) = state.entries.get(state.selected).cloned()
                && entry.is_dir
                && !entry.is_parent
            {
                state.enter_dir(entry.path, None);
            }
        }
        KeyCode::Backspace => {
            if state.cursor > 0 && (state.focus_input || !state.purpose.is_save()) {
                let prev = prev_char_boundary(&state.input, state.cursor);
                state.input.drain(prev..state.cursor);
                state.cursor = prev;
                state.error = None;
                if !state.purpose.is_save() {
                    state.refresh_entries(None);
                }
            } else {
                state.up_dir();
            }
        }
        KeyCode::Delete => {
            if state.cursor < state.input.len() && (state.focus_input || !state.purpose.is_save()) {
                let next = next_char_boundary(&state.input, state.cursor);
                state.input.drain(state.cursor..next);
                state.error = None;
                if !state.purpose.is_save() {
                    state.refresh_entries(None);
                }
            }
        }
        KeyCode::Enter => {
            let outcome = state.confirm();
            finish_file_picker_outcome(app, outcome, state);
            return;
        }
        KeyCode::Char(ch) if !ctrl && !alt => {
            if state.purpose.is_save() {
                state.focus_input = true;
            }
            state.input.insert(state.cursor, ch);
            state.cursor += ch.len_utf8();
            state.error = None;
            if !state.purpose.is_save() {
                state.refresh_entries(None);
            }
        }
        _ => {}
    }
    app.modal = Modal::FilePicker(state);
}

fn handle_confirm_quit_key(app: &mut App, key: KeyEvent, quit_app: bool) {
    match key.code {
        KeyCode::Char('d' | 'D' | 'y' | 'Y') => apply_confirm_close_discard(app, quit_app),
        KeyCode::Char('s' | 'S') => apply_confirm_close_save(app, quit_app),
        KeyCode::Esc | KeyCode::Char('n' | 'N') => {}
        _ => app.modal = Modal::ConfirmQuit { quit_app },
    }
}

pub(super) fn finish_file_picker_outcome(
    app: &mut App,
    outcome: FilePickerOutcome,
    mut state: FilePickerState,
) {
    match outcome {
        FilePickerOutcome::Pending => app.modal = Modal::FilePicker(state),
        FilePickerOutcome::Cancelled => app.modal = Modal::None,
        FilePickerOutcome::Confirmed(path) => {
            let purpose = state.purpose;
            if let Err(e) = app.apply_file_picker_path(purpose, path) {
                app.status = format!("Error: {e}");
                state.error = Some(e.to_string());
                app.modal = Modal::FilePicker(state);
            }
        }
    }
}

pub(super) fn apply_confirm_close_discard(app: &mut App, quit_app: bool) {
    if quit_app {
        clear_recovery_snapshot(app.wb.file_path.as_deref());
        app.should_quit = true;
    } else if let Err(e) = app.reset_to_empty_workbook() {
        app.status = format!("Error: {e}");
    } else {
        app.status = "Discarded changes and closed workbook".into();
    }
}

pub(super) fn apply_confirm_close_save(app: &mut App, quit_app: bool) {
    if app.wb.file_path.is_some() {
        app.execute_action(Action::Save);
        if !app.wb.dirty {
            if quit_app {
                clear_recovery_snapshot(app.wb.file_path.as_deref());
                app.should_quit = true;
            } else if let Err(e) = app.reset_to_empty_workbook() {
                app.status = format!("Error: {e}");
            } else {
                app.status = "Saved and closed workbook".into();
            }
        }
    } else {
        app.open_file_picker(FilePickerPurpose::SaveWorkbook {
            after: if quit_app {
                PostSaveAction::QuitApp
            } else {
                PostSaveAction::CloseWorkbook
            },
        });
    }
}

fn sync_find_state(
    app: &mut App,
    find: &str,
    case_sensitive: bool,
    all_sheets: bool,
    matches: &[(u32, i32, i32)],
    idx: usize,
) {
    app.search_highlights = matches.iter().copied().collect();
    if !find.is_empty() {
        app.last_search = Some((find.to_string(), case_sensitive, all_sheets));
    }
    if let Some(&(s, r, c)) = matches.get(idx) {
        let _ = app.wb.select_sheet(s);
        app.wb.select_cell(r, c);
        app.ensure_cursor_visible();
    }
}
