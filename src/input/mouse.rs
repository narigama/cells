use super::byte_index_at_char;
use super::modals::{
    apply_confirm_close_discard, apply_confirm_close_save, finish_file_picker_outcome,
};
use crate::actions::{
    Action, TOP_MENUS, cell_context_menu, chart_context_menu, col_header_context_menu,
    row_header_context_menu, sheet_tab_context_menu,
};
use crate::address::{MAX_COLS, MAX_ROWS, apply_formula_ref, format_range_ref};
use crate::app::{ActiveTopMenu, App, ColHit, ContextMenuState, DragState, Modal, RowHit};
use crate::chart::hover_chart_point;
use crate::config::{DockPosition, SWATCH_COLORS};
use crate::cursor::CursorShape;
use crate::file_picker::{FilePickerPurpose, FilePickerState, PostSaveAction};
use crate::formula::accept_completion;
use crate::ui::{filter_palette_commands, rect_contains};
use crate::workbook::ChartKind;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

const DOUBLE_CLICK_MS: u64 = 360;

pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let fs = app.picker.font_size();
    let (mx, my, px_x, px_y) =
        app.cursor_rt
            .normalize_mouse_coords(mouse.column, mouse.row, fs.width, fs.height);
    app.mouse_cell = Some((mx, my));
    app.mouse_px = Some((px_x, px_y));

    update_chart_hover(app, mx, my);
    app.cursor_shape = compute_cursor_shape(app, mx, my);

    if let Some(mrect) = app.layout.modal_rect {
        handle_modal_mouse(app, mouse, mrect, mx, my);
        return;
    }
    if app.context_menu.is_some() && handle_context_menu_mouse(app, mouse, mx, my) {
        return;
    }
    if app.top_menu.is_some() {
        handle_top_menu_mouse(app, mouse, mx, my);
        return;
    }
    if handle_formula_popup_mouse(app, mouse, mx, my) {
        return;
    }

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                app.scroll_by(0, -2);
            } else {
                app.scroll_by(-3, 0);
            }
        }
        MouseEventKind::ScrollDown => {
            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                app.scroll_by(0, 2);
            } else {
                app.scroll_by(3, 0);
            }
        }
        MouseEventKind::ScrollLeft => app.scroll_by(0, -2),
        MouseEventKind::ScrollRight => app.scroll_by(0, 2),
        MouseEventKind::Down(MouseButton::Right) => open_right_click_menu(app, mx, my),
        MouseEventKind::Down(MouseButton::Left) => handle_left_down(app, mouse, mx, my),
        MouseEventKind::Drag(MouseButton::Left) => handle_left_drag(app, mx, my),
        MouseEventKind::Up(MouseButton::Left) => handle_left_up(app),
        _ => {}
    }

    app.cursor_shape = compute_cursor_shape(app, mx, my);
}

fn record_click_and_check_double(app: &mut App, mx: u16, my: u16) -> bool {
    let now = Instant::now();
    let is_double = app.last_click.is_some_and(|(t, lx, ly)| {
        lx == mx && ly == my && now.duration_since(t) <= Duration::from_millis(DOUBLE_CLICK_MS)
    });
    app.last_click = Some((now, mx, my));
    is_double
}

fn handle_modal_mouse(app: &mut App, mouse: MouseEvent, mrect: Rect, mx: u16, my: u16) {
    match mouse.kind {
        MouseEventKind::ScrollUp => match &mut app.modal {
            Modal::FilePicker(state) => state.move_selection(-3),
            Modal::Help { scroll } => *scroll = scroll.saturating_sub(2),
            _ => {}
        },
        MouseEventKind::ScrollDown => match &mut app.modal {
            Modal::FilePicker(state) => state.move_selection(3),
            Modal::Help { scroll } => *scroll = scroll.saturating_add(2).min(28),
            _ => {}
        },
        MouseEventKind::Down(MouseButton::Left) => {
            let is_double = record_click_and_check_double(app, mx, my);

            if !rect_contains(mrect, mx, my)
                && !matches!(app.modal, Modal::ConfirmQuit { .. })
                && !matches!(
                    app.modal,
                    Modal::FilePicker(FilePickerState {
                        purpose: FilePickerPurpose::SaveWorkbook {
                            after: PostSaveAction::QuitApp | PostSaveAction::CloseWorkbook
                        },
                        ..
                    })
                )
            {
                app.modal = Modal::None;
                app.cursor_shape = compute_cursor_shape(app, mx, my);
                return;
            }

            if let Modal::FilePicker(state) = app.modal.clone() {
                handle_file_picker_click(app, state, mx, my, is_double);
                return;
            }

            if let Modal::ConfirmQuit { quit_app } = app.modal {
                if app
                    .layout
                    .confirm_btn_save
                    .is_some_and(|r| rect_contains(r, mx, my))
                {
                    app.modal = Modal::None;
                    apply_confirm_close_save(app, quit_app);
                } else if app
                    .layout
                    .confirm_btn_discard
                    .is_some_and(|r| rect_contains(r, mx, my))
                {
                    app.modal = Modal::None;
                    apply_confirm_close_discard(app, quit_app);
                } else if app
                    .layout
                    .confirm_btn_cancel
                    .is_some_and(|r| rect_contains(r, mx, my))
                {
                    app.modal = Modal::None;
                }
                return;
            }

            if let Modal::ColorPicker {
                is_fill,
                ref custom_hex,
                ..
            } = app.modal.clone()
            {
                let rel_y = my.saturating_sub(mrect.y + 1) as usize;
                if rel_y < SWATCH_COLORS.len() {
                    app.modal = Modal::None;
                    app.apply_color_choice(is_fill, rel_y, custom_hex);
                }
            } else if let Modal::CommandPalette { ref query, .. } = app.modal.clone()
                && my >= mrect.y + 3
                && my < mrect.bottom().saturating_sub(1)
            {
                let idx = (my - (mrect.y + 3)) as usize;
                let cmds = filter_palette_commands(query);
                if let Some(&(_, _, act)) = cmds.get(idx) {
                    app.modal = Modal::None;
                    app.execute_action(act);
                }
            }
        }
        _ => {}
    }
}

fn handle_file_picker_click(
    app: &mut App,
    mut state: FilePickerState,
    mx: u16,
    my: u16,
    is_double: bool,
) {
    let Some(fp) = app.layout.file_picker.clone() else {
        return;
    };
    if rect_contains(fp.btn_cancel, mx, my) {
        app.modal = Modal::None;
    } else if rect_contains(fp.btn_confirm, mx, my) {
        app.modal = Modal::None;
        let outcome = state.confirm_from_button();
        finish_file_picker_outcome(app, outcome, state);
    } else if rect_contains(fp.btn_up, mx, my) {
        state.up_dir();
        app.modal = Modal::FilePicker(state);
    } else if rect_contains(fp.btn_home, mx, my) {
        state.go_home();
        app.modal = Modal::FilePicker(state);
    } else if rect_contains(fp.btn_hidden, mx, my) {
        state.toggle_hidden();
        app.modal = Modal::FilePicker(state);
    } else if rect_contains(fp.btn_all, mx, my) {
        state.toggle_all_files();
        app.modal = Modal::FilePicker(state);
    } else if rect_contains(fp.input_rect, mx, my) {
        state.focus_input = true;
        let char_idx = mx.saturating_sub(fp.input_text_x) as usize;
        state.cursor = byte_index_at_char(&state.input, char_idx);
        app.modal = Modal::FilePicker(state);
    } else if let Some(&(entry_idx, _)) =
        fp.row_rects.iter().find(|(_, r)| rect_contains(*r, mx, my))
    {
        if is_double {
            app.modal = Modal::None;
            let outcome = state.activate_entry(entry_idx);
            finish_file_picker_outcome(app, outcome, state);
        } else {
            state.select_index_from_click(entry_idx);
            app.modal = Modal::FilePicker(state);
        }
    }
}

/// Returns `true` if the mouse event was consumed by the open context menu;
/// returns `false` on right-click so a new context menu can open at the new position.
fn handle_context_menu_mouse(app: &mut App, mouse: MouseEvent, mx: u16, my: u16) -> bool {
    match mouse.kind {
        MouseEventKind::Moved => {
            if let Some(&(idx, _)) = app
                .layout
                .context_items
                .iter()
                .find(|(_, r)| rect_contains(*r, mx, my))
                && let Some(ref mut cm) = app.context_menu
            {
                cm.selected = idx;
            }
            true
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(&(idx, _)) = app
                .layout
                .context_items
                .iter()
                .find(|(_, r)| rect_contains(*r, mx, my))
            {
                let act = app.context_menu.as_ref().unwrap().entries[idx].action;
                app.execute_action(act);
            } else {
                app.context_menu = None;
            }
            true
        }
        MouseEventKind::Down(MouseButton::Right) => {
            app.context_menu = None;
            false
        }
        _ => true,
    }
}

fn handle_top_menu_mouse(app: &mut App, mouse: MouseEvent, mx: u16, my: u16) {
    match mouse.kind {
        MouseEventKind::Moved => {
            if let Some(&(m_idx, _)) = app
                .layout
                .menu_titles
                .iter()
                .find(|(_, r)| rect_contains(*r, mx, my))
            {
                app.top_menu = Some(ActiveTopMenu {
                    menu_idx: m_idx,
                    selected: 0,
                });
                return;
            }
            if let Some(&(e_idx, _)) = app
                .layout
                .dropdown_items
                .iter()
                .find(|(_, r)| rect_contains(*r, mx, my))
                && let Some(ref mut tm) = app.top_menu
            {
                tm.selected = e_idx;
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(&(e_idx, _)) = app
                .layout
                .dropdown_items
                .iter()
                .find(|(_, r)| rect_contains(*r, mx, my))
            {
                let tm = app.top_menu.unwrap();
                let act = TOP_MENUS[tm.menu_idx].entries[e_idx].action;
                app.execute_action(act);
            } else if let Some(&(m_idx, _)) = app
                .layout
                .menu_titles
                .iter()
                .find(|(_, r)| rect_contains(*r, mx, my))
            {
                if app.top_menu.map(|t| t.menu_idx) == Some(m_idx) {
                    app.top_menu = None;
                } else {
                    app.top_menu = Some(ActiveTopMenu {
                        menu_idx: m_idx,
                        selected: 0,
                    });
                }
            } else {
                app.top_menu = None;
            }
        }
        _ => {}
    }
}

fn handle_formula_popup_mouse(app: &mut App, mouse: MouseEvent, mx: u16, my: u16) -> bool {
    let Some(prect) = app.layout.formula_popup_rect else {
        return false;
    };
    if app.formula_assist.candidates.is_empty() || !rect_contains(prect, mx, my) {
        return false;
    }
    let cand_len = app.formula_assist.candidates.len();
    match mouse.kind {
        MouseEventKind::Moved => {
            if let Some(&(idx, _)) = app
                .layout
                .formula_assist_items
                .iter()
                .find(|(_, r)| rect_contains(*r, mx, my))
                && let Some(ref mut ed) = app.edit
            {
                ed.completion_idx = idx.min(cand_len - 1);
            }
            true
        }
        MouseEventKind::ScrollUp => {
            if let Some(ref mut ed) = app.edit {
                ed.completion_idx = ed.completion_idx.saturating_sub(3);
                ed.completion_scroll = ed.completion_scroll.saturating_sub(3);
            }
            true
        }
        MouseEventKind::ScrollDown => {
            if let Some(ref mut ed) = app.edit {
                ed.completion_idx = (ed.completion_idx + 3).min(cand_len - 1);
                ed.completion_scroll = (ed.completion_scroll + 3).min(cand_len - 1);
            }
            true
        }
        _ => false,
    }
}

fn update_chart_hover(app: &mut App, mx: u16, my: u16) {
    if app.modal.is_none()
        && app.top_menu.is_none()
        && app.context_menu.is_none()
        && let Some(body) = app.layout.chart_body_rect
        && rect_contains(body, mx, my)
        && let Some(spec) = app.wb.charts.get(app.active_chart_idx).cloned()
        && let Some(data) = app.cached_chart_data(&spec)
    {
        app.chart_hover = hover_chart_point(&data, body, mx, my);
    } else {
        app.chart_hover = None;
    }
}

pub fn compute_cursor_shape(app: &App, mx: u16, my: u16) -> CursorShape {
    match app.drag {
        DragState::ResizingCol { .. } => return CursorShape::ResizeEW,
        DragState::ResizingDock => {
            return match app.cfg.dock_position {
                DockPosition::Right => CursorShape::ResizeEW,
                DockPosition::Bottom => CursorShape::ResizeNS,
            };
        }
        DragState::FillHandle { .. } => return CursorShape::FillCrosshair,
        DragState::MovingSelection { .. } => return CursorShape::Move,
        DragState::ScrollingVBar | DragState::ScrollingHBar => return CursorShape::Pointer,
        DragState::SelectingCells { .. } => return CursorShape::CellCross,
        DragState::SelectingCols { .. } | DragState::SelectingRows { .. } => {
            return CursorShape::Pointer;
        }
        DragState::None => {}
    }

    if !app.modal.is_none() || app.context_menu.is_some() || app.top_menu.is_some() {
        if matches!(app.modal, Modal::FilePicker(_))
            && app
                .layout
                .file_picker
                .as_ref()
                .is_some_and(|fp| rect_contains(fp.input_rect, mx, my))
        {
            return CursorShape::IBeam;
        }
        return CursorShape::Pointer;
    }

    if app
        .layout
        .formula_assist_items
        .iter()
        .any(|(_, r)| rect_contains(*r, mx, my))
    {
        return CursorShape::Pointer;
    }

    if rect_contains(app.layout.formula_rect, mx, my) {
        return CursorShape::IBeam;
    }

    if is_on_dock_divider(app, mx, my) {
        return match app.cfg.dock_position {
            DockPosition::Right => CursorShape::ResizeEW,
            DockPosition::Bottom => CursorShape::ResizeNS,
        };
    }

    if app
        .layout
        .chart_body_rect
        .is_some_and(|body| rect_contains(body, mx, my))
    {
        return CursorShape::FillCrosshair;
    }

    if rect_contains(app.layout.grid_rect, mx, my) {
        if app
            .layout
            .v_scrollbar
            .is_some_and(|r| rect_contains(r, mx, my))
            || app
                .layout
                .h_scrollbar
                .is_some_and(|r| rect_contains(r, mx, my))
        {
            return CursorShape::Pointer;
        }
        if app.layout.fill_handle == Some((mx, my)) && app.edit.is_none() {
            return CursorShape::FillCrosshair;
        }
        if app.edit.is_none() && app.layout.selection_border_cells.contains(&(mx, my)) {
            return CursorShape::Move;
        }
        if app
            .layout
            .filter_buttons
            .iter()
            .any(|(_, r)| rect_contains(*r, mx, my))
        {
            return CursorShape::Pointer;
        }
        if my == app.layout.col_header_y {
            let on_col_div = app
                .layout
                .cols
                .iter()
                .any(|c| mx.abs_diff(c.divider_x) <= 1 && mx >= c.x_end.saturating_sub(1));
            return if on_col_div {
                CursorShape::ResizeEW
            } else {
                CursorShape::Pointer
            };
        }
        if mx < app.layout.row_header_x_end {
            return CursorShape::Pointer;
        }
        return CursorShape::CellCross;
    }

    CursorShape::Pointer
}

fn is_on_dock_divider(app: &App, mx: u16, my: u16) -> bool {
    app.layout.divider_rect.is_some_and(|div_r| {
        rect_contains(div_r, mx, my)
            || (div_r.width == 1
                && my >= div_r.y
                && my < div_r.bottom()
                && mx.abs_diff(div_r.x) <= 1)
    })
}

fn open_right_click_menu(app: &mut App, mx: u16, my: u16) {
    if app.edit.is_some() {
        app.commit_edit();
    }

    if let Some((sheet_idx, _)) = app
        .layout
        .sheet_tabs
        .iter()
        .copied()
        .find(|(_, r)| rect_contains(*r, mx, my))
    {
        let _ = app.wb.select_sheet(sheet_idx);
        app.context_menu = Some(ContextMenuState {
            x: mx,
            y: my,
            entries: sheet_tab_context_menu(),
            selected: 0,
        });
        return;
    }

    if app
        .layout
        .chart_dock_rect
        .is_some_and(|dock_r| rect_contains(dock_r, mx, my))
    {
        app.context_menu = Some(ContextMenuState {
            x: mx,
            y: my,
            entries: chart_context_menu(),
            selected: 0,
        });
        return;
    }

    if !rect_contains(app.layout.grid_rect, mx, my) {
        return;
    }

    if my == app.layout.col_header_y
        && let Some(ch) = hit_col(app, mx)
    {
        let [_, c1, _, c2] = app.wb.selected_range();
        if !(c1..=c2).contains(&ch.col) {
            app.wb.select_range(1, ch.col, MAX_ROWS, ch.col);
        }
        app.context_menu = Some(ContextMenuState {
            x: mx,
            y: my,
            entries: col_header_context_menu(),
            selected: 0,
        });
    } else if mx < app.layout.row_header_x_end
        && let Some(rh) = hit_row(app, my)
    {
        let [r1, _, r2, _] = app.wb.selected_range();
        if !(r1..=r2).contains(&rh.row) {
            app.wb.select_range(rh.row, 1, rh.row, MAX_COLS);
        }
        app.context_menu = Some(ContextMenuState {
            x: mx,
            y: my,
            entries: row_header_context_menu(),
            selected: 0,
        });
    } else if let (Some(rh), Some(ch)) = (hit_row(app, my), hit_col(app, mx)) {
        let [r1, c1, r2, c2] = app.wb.selected_range();
        if !(r1..=r2).contains(&rh.row) || !(c1..=c2).contains(&ch.col) {
            app.wb.select_cell(rh.row, ch.col);
        }
        app.context_menu = Some(ContextMenuState {
            x: mx,
            y: my,
            entries: cell_context_menu(),
            selected: 0,
        });
    }
}

fn handle_left_down(app: &mut App, mouse: MouseEvent, mx: u16, my: u16) {
    let is_double = record_click_and_check_double(app, mx, my);

    // Formula IntelliSense candidate click accepts completion directly.
    if let Some(&(idx, _)) = app
        .layout
        .formula_assist_items
        .iter()
        .find(|(_, r)| rect_contains(*r, mx, my))
    {
        let assist = app.formula_assist.clone();
        if let Some(ref mut ed) = app.edit
            && accept_completion(&mut ed.buf, &mut ed.cursor, &assist, idx)
        {
            ed.completion_idx = 0;
            ed.completion_scroll = 0;
            ed.force_completion = false;
            ed.formula_ref_span = None;
        }
        app.refresh_formula_state();
        return;
    }

    if let Some(&(m_idx, _)) = app
        .layout
        .menu_titles
        .iter()
        .find(|(_, r)| rect_contains(*r, mx, my))
    {
        if app.edit.is_some() {
            app.commit_edit();
        }
        app.top_menu = Some(ActiveTopMenu {
            menu_idx: m_idx,
            selected: 0,
        });
        return;
    }

    if rect_contains(app.layout.address_rect, mx, my) {
        app.execute_action(Action::GotoCellPrompt);
        return;
    }

    if rect_contains(app.layout.formula_rect, mx, my) {
        if app.edit.is_none() {
            app.start_edit_existing();
        }
        if let Some(ref mut ed) = app.edit {
            let char_idx = mx.saturating_sub(app.layout.formula_rect.x + 4) as usize;
            ed.cursor = byte_index_at_char(&ed.buf, char_idx);
            ed.formula_ref_span = None;
        }
        app.refresh_formula_state();
        return;
    }

    // Sheet tab click (allowed during formula editing so cross-sheet refs can be clicked).
    if let Some((sheet_idx, _)) = app
        .layout
        .sheet_tabs
        .iter()
        .copied()
        .find(|(_, r)| rect_contains(*r, mx, my))
    {
        if app.edit.as_ref().is_some_and(|e| !e.buf.starts_with('=')) {
            app.commit_edit();
        }
        let _ = app.wb.select_sheet(sheet_idx);
        return;
    }
    if app
        .layout
        .add_sheet_btn
        .is_some_and(|add_r| rect_contains(add_r, mx, my))
    {
        app.execute_action(Action::NewSheet);
        return;
    }

    if is_on_dock_divider(app, mx, my) {
        app.drag = DragState::ResizingDock;
        return;
    }

    if app
        .layout
        .chart_dock_rect
        .is_some_and(|dock_r| rect_contains(dock_r, mx, my))
    {
        handle_chart_dock_click(app, mx, my);
        return;
    }

    if !rect_contains(app.layout.grid_rect, mx, my) {
        return;
    }

    if let Some(vr) = app.layout.v_scrollbar
        && rect_contains(vr, mx, my)
    {
        app.drag = DragState::ScrollingVBar;
        scroll_vbar_to(app, vr, my);
        return;
    }
    if let Some(hr) = app.layout.h_scrollbar
        && rect_contains(hr, mx, my)
    {
        app.drag = DragState::ScrollingHBar;
        scroll_hbar_to(app, hr, mx);
        return;
    }

    if let Some(&(col, _)) = app
        .layout
        .filter_buttons
        .iter()
        .find(|(_, r)| rect_contains(*r, mx, my))
    {
        if app.edit.is_some() {
            app.commit_edit();
        }
        let _ = app.open_column_filter_modal(col);
        return;
    }

    if app.layout.fill_handle == Some((mx, my)) && app.edit.is_none() {
        let src = app.wb.selected_range();
        app.drag = DragState::FillHandle {
            source: src,
            target_row: src[2],
            target_col: src[3],
        };
        return;
    }

    if app.edit.is_none() && app.layout.selection_border_cells.contains(&(mx, my)) {
        let src = app.wb.selected_range();
        let r_click = hit_row_clamped(app, my).map(|r| r.row).unwrap_or(src[0]);
        let c_click = hit_col_clamped(app, mx).map(|c| c.col).unwrap_or(src[1]);
        app.drag = DragState::MovingSelection {
            source: src,
            row_offset: (r_click - src[0]).clamp(0, src[2] - src[0]),
            col_offset: (c_click - src[1]).clamp(0, src[3] - src[1]),
            target_r1: src[0],
            target_c1: src[1],
        };
        return;
    }

    if rect_contains(app.layout.corner_rect, mx, my) {
        if app.edit.is_some() {
            app.commit_edit();
        }
        let (mr, mc) = app.wb.sheet_bounds(app.wb.active_sheet());
        app.wb.select_range(1, 1, mr.max(20), mc.max(10));
        return;
    }

    if my == app.layout.col_header_y {
        if app.edit.is_some() {
            app.commit_edit();
        }
        if let Some(div_col) = app
            .layout
            .cols
            .iter()
            .find(|c| mx.abs_diff(c.divider_x) <= 1 && mx >= c.x_end.saturating_sub(1))
            .map(|c| c.col)
        {
            let sheet = app.wb.active_sheet();
            if is_double {
                let _ = app.wb.autofit_column(sheet, div_col);
                app.record_model_undo();
                app.status = "Auto-fit column width".into();
            } else {
                let start_chars = app.wb.col_width_chars(sheet, div_col);
                app.drag = DragState::ResizingCol {
                    col: div_col,
                    start_x: mx,
                    start_chars,
                };
            }
            return;
        }
        if let Some(ch) = hit_col(app, mx) {
            app.wb.select_range(1, ch.col, MAX_ROWS, ch.col);
            app.drag = DragState::SelectingCols { anchor_col: ch.col };
        }
        return;
    }

    if mx < app.layout.row_header_x_end {
        if app.edit.is_some() {
            app.commit_edit();
        }
        if let Some(rh) = hit_row(app, my) {
            app.wb.select_range(rh.row, 1, rh.row, MAX_COLS);
            app.drag = DragState::SelectingRows { anchor_row: rh.row };
        }
        return;
    }

    let (Some(rh), Some(ch)) = (hit_row(app, my), hit_col(app, mx)) else {
        return;
    };

    // Live formula range picking when editing a formula (`=SUM(` -> click/drag `B2:B10`).
    if let Some(ref mut ed) = app.edit {
        let view_sheet = app.wb.active_sheet();
        let sheet_prefix = (view_sheet != ed.sheet).then(|| app.wb.active_sheet_name());
        let ref_txt = format_range_ref(sheet_prefix.as_deref(), rh.row, ch.col, rh.row, ch.col);
        if apply_formula_ref(
            &mut ed.buf,
            &mut ed.cursor,
            &mut ed.formula_ref_span,
            &ref_txt,
        ) {
            app.drag = DragState::SelectingCells {
                anchor_sheet: view_sheet,
                anchor_row: rh.row,
                anchor_col: ch.col,
                for_formula: true,
            };
            app.refresh_formula_state();
            return;
        }
        app.commit_edit();
    }

    if is_double {
        app.wb.select_cell(rh.row, ch.col);
        app.start_edit_existing();
        return;
    }

    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        let (ar, ac) = app.wb.selected_cell();
        app.wb.select_range(ar, ac, rh.row, ch.col);
    } else {
        app.wb.select_cell(rh.row, ch.col);
    }
    app.drag = DragState::SelectingCells {
        anchor_sheet: app.wb.active_sheet(),
        anchor_row: rh.row,
        anchor_col: ch.col,
        for_formula: false,
    };
}

fn handle_chart_dock_click(app: &mut App, mx: u16, my: u16) {
    if let Some((idx, _)) = app
        .layout
        .chart_tab_rects
        .iter()
        .copied()
        .find(|(_, r)| rect_contains(*r, mx, my))
    {
        app.active_chart_idx = idx;
        return;
    }
    let btn_actions = [
        (app.layout.chart_btn_new, Action::NewChart(ChartKind::Line)),
        (app.layout.chart_btn_type, Action::CycleChartKind),
        (app.layout.chart_btn_edit, Action::EditActiveChart),
        (app.layout.chart_btn_png, Action::ExportChartPngPrompt),
        (app.layout.chart_btn_svg, Action::ExportChartSvgPrompt),
        (app.layout.chart_btn_zoom, Action::ToggleChartFullscreen),
    ];
    for (btn, act) in btn_actions {
        if btn.is_some_and(|r| rect_contains(r, mx, my)) {
            app.execute_action(act);
            return;
        }
    }
    if app
        .layout
        .chart_btn_close
        .is_some_and(|r| rect_contains(r, mx, my))
    {
        app.dock_visible = false;
        app.chart_fullscreen = false;
        return;
    }
    if let Some(hit) = app.chart_hover.clone() {
        let _ = app.wb.select_sheet(hit.sheet);
        app.wb.select_cell(hit.source_row, 1);
        app.ensure_cursor_visible();
    }
}

fn handle_left_drag(app: &mut App, mx: u16, my: u16) {
    match app.drag {
        DragState::None => {}
        DragState::ScrollingVBar => {
            if let Some(vr) = app.layout.v_scrollbar {
                scroll_vbar_to(app, vr, my);
            }
        }
        DragState::ScrollingHBar => {
            if let Some(hr) = app.layout.h_scrollbar {
                scroll_hbar_to(app, hr, mx);
            }
        }
        DragState::ResizingDock => {
            let ws = app.layout.workspace_rect;
            match app.cfg.dock_position {
                DockPosition::Right if ws.width > 0 => {
                    let right_w = ws.right().saturating_sub(mx) as u32;
                    app.cfg.dock_percent = ((right_w * 100) / ws.width as u32).clamp(20, 80) as u16;
                }
                DockPosition::Bottom if ws.height > 0 => {
                    let bot_h = ws.bottom().saturating_sub(my) as u32;
                    app.cfg.dock_percent = ((bot_h * 100) / ws.height as u32).clamp(20, 80) as u16;
                }
                _ => {}
            }
        }
        DragState::ResizingCol {
            col,
            start_x,
            start_chars,
        } => {
            let delta = mx as i32 - start_x as i32;
            let new_w = (start_chars as i32 + delta).clamp(3, 80) as u16;
            let sheet = app.wb.active_sheet();
            let _ = app.wb.set_col_width_chars(sheet, col, new_w);
        }
        DragState::SelectingCols { anchor_col } => {
            if let Some(ch) = hit_col_clamped(app, mx) {
                app.wb.select_range(1, anchor_col, MAX_ROWS, ch.col);
            }
        }
        DragState::SelectingRows { anchor_row } => {
            if let Some(rh) = hit_row_clamped(app, my) {
                app.wb.select_range(anchor_row, 1, rh.row, MAX_COLS);
            }
        }
        DragState::FillHandle { source, .. } => {
            let tr = hit_row_clamped(app, my).map(|r| r.row).unwrap_or(source[2]);
            let tc = hit_col_clamped(app, mx).map(|c| c.col).unwrap_or(source[3]);
            app.drag = DragState::FillHandle {
                source,
                target_row: tr,
                target_col: tc,
            };
        }
        DragState::MovingSelection {
            source,
            row_offset,
            col_offset,
            ..
        } => {
            let r_cur = hit_row_clamped(app, my).map(|r| r.row).unwrap_or(source[0]);
            let c_cur = hit_col_clamped(app, mx).map(|c| c.col).unwrap_or(source[1]);
            let h = source[2] - source[0];
            let w = source[3] - source[1];
            let tr1 = (r_cur - row_offset).clamp(1, MAX_ROWS - h);
            let tc1 = (c_cur - col_offset).clamp(1, MAX_COLS - w);
            app.drag = DragState::MovingSelection {
                source,
                row_offset,
                col_offset,
                target_r1: tr1,
                target_c1: tc1,
            };
        }
        DragState::SelectingCells {
            anchor_sheet,
            anchor_row,
            anchor_col,
            for_formula,
        } => {
            let gr = app.layout.grid_rect;
            if my >= gr.bottom().saturating_sub(1) {
                app.scroll_by(1, 0);
            } else if my <= app.layout.col_header_y {
                app.scroll_by(-1, 0);
            }
            if mx >= gr.right().saturating_sub(1) {
                app.scroll_by(0, 1);
            } else if mx <= app.layout.row_header_x_end {
                app.scroll_by(0, -1);
            }

            let (Some(rh), Some(ch)) = (hit_row_clamped(app, my), hit_col_clamped(app, mx)) else {
                return;
            };

            if for_formula {
                if let Some(ref mut ed) = app.edit {
                    let prefix = (anchor_sheet != ed.sheet).then(|| app.wb.active_sheet_name());
                    let ref_txt =
                        format_range_ref(prefix.as_deref(), anchor_row, anchor_col, rh.row, ch.col);
                    let _ = apply_formula_ref(
                        &mut ed.buf,
                        &mut ed.cursor,
                        &mut ed.formula_ref_span,
                        &ref_txt,
                    );
                }
                app.refresh_formula_state();
            } else {
                app.wb.select_range(anchor_row, anchor_col, rh.row, ch.col);
            }
        }
    }
}

fn handle_left_up(app: &mut App) {
    match app.drag {
        DragState::FillHandle {
            source,
            target_row,
            target_col,
        } => match app.wb.fill_drag(source, target_row, target_col) {
            Ok([r1, c1, r2, c2]) => {
                app.wb.select_range(r1, c1, r2, c2);
                if [r1, c1, r2, c2] != source {
                    app.record_model_undo();
                }
                app.bump_data_rev();
                app.status = "Auto-filled range".into();
            }
            Err(e) => app.status = format!("Auto-fill error: {e}"),
        },
        DragState::MovingSelection {
            source,
            target_r1,
            target_c1,
            ..
        } => {
            if (target_r1, target_c1) != (source[0], source[1]) {
                match app.wb.move_range(source, target_r1, target_c1) {
                    Ok(_) => {
                        app.record_model_undo();
                        app.bump_data_rev();
                        app.status = "Moved selection".into();
                    }
                    Err(e) => app.status = format!("Move error: {e}"),
                }
            }
        }
        DragState::ResizingCol { .. } => app.record_model_undo(),
        DragState::ResizingDock => {
            let _ = app.cfg.save();
        }
        _ => {}
    }
    app.drag = DragState::None;
}

fn scroll_vbar_to(app: &mut App, vr: Rect, my: u16) {
    let sheet = app.wb.active_sheet();
    let (used_r, _) = app.wb.sheet_bounds(sheet);
    let vis_r = app.layout.visible_scroll_rows.max(1) as i32;
    let (_, sc) = app.sheet_scroll(sheet);
    let total_r = used_r.max(vis_r + 30);
    let frac = if vr.height <= 1 {
        0.0
    } else {
        (my.saturating_sub(vr.y) as f64 / (vr.height - 1) as f64).clamp(0.0, 1.0)
    };
    let target_r = 1 + (frac * (total_r - vis_r).max(1) as f64).round() as i32;
    app.set_sheet_scroll(sheet, target_r, sc);
}

fn scroll_hbar_to(app: &mut App, hr: Rect, mx: u16) {
    let sheet = app.wb.active_sheet();
    let (_, used_c) = app.wb.sheet_bounds(sheet);
    let vis_c = app.layout.visible_scroll_cols.max(1) as i32;
    let (sr, _) = app.sheet_scroll(sheet);
    let total_c = used_c.max(vis_c + 16);
    let frac = if hr.width <= 1 {
        0.0
    } else {
        (mx.saturating_sub(hr.x) as f64 / (hr.width - 1) as f64).clamp(0.0, 1.0)
    };
    let target_c = 1 + (frac * (total_c - vis_c).max(1) as f64).round() as i32;
    app.set_sheet_scroll(sheet, sr, target_c);
}

fn hit_col(app: &App, mx: u16) -> Option<ColHit> {
    app.layout
        .cols
        .iter()
        .copied()
        .find(|c| mx >= c.x_start && mx <= c.x_end)
}

fn hit_col_clamped(app: &App, mx: u16) -> Option<ColHit> {
    hit_col(app, mx).or_else(|| {
        if mx < app.layout.row_header_x_end {
            app.layout.cols.first().copied()
        } else {
            app.layout.cols.last().copied()
        }
    })
}

fn hit_row(app: &App, my: u16) -> Option<RowHit> {
    app.layout.rows.iter().copied().find(|r| r.y == my)
}

fn hit_row_clamped(app: &App, my: u16) -> Option<RowHit> {
    hit_row(app, my).or_else(|| {
        if my <= app.layout.col_header_y {
            app.layout.rows.first().copied()
        } else {
            app.layout.rows.last().copied()
        }
    })
}
