pub mod analysis;
pub mod io;
pub mod types;

pub use io::{check_orphaned_recovery, clear_recovery_snapshot, recovery_dir, recovery_path_for};
pub use types::*;

use crate::address::{MAX_COLS, MAX_ROWS, col_to_letters};
use crate::config::parse_hex_color;
use anyhow::{Context, Result, anyhow};
use base64::Engine;
use ironcalc_base::COLUMN_WIDTH_FACTOR;
use ironcalc_base::cell::CellValue;
use ironcalc_base::expressions::types::Area;
use ironcalc_base::types::{Cell as IcCell, CellType, Color as IcColor, HorizontalAlignment};
use ironcalc_base::worksheet::NavigationDirection;
use ironcalc_base::{Model, UserModel};
use ratatui::layout::Alignment as UiAlign;
use ratatui::style::Color;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write, stdout};
use std::path::PathBuf;

pub struct WorkbookState {
    pub user: UserModel<'static>,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    pub charts: Vec<ChartSpec>,
    pub(crate) next_chart_id: u64,
    pub visual_rules: Vec<VisualRule>,
    pub autofilters: Vec<AutoFilterState>,
    pub(crate) autofilter_hidden: HashMap<u32, HashSet<i32>>,
    pub(crate) bounds_cache: RefCell<HashMap<u32, (i32, i32)>>,
    pub(crate) hidden_cache: RefCell<HashMap<u32, HashSet<i32>>>,
    internal_clip: Option<InternalClip>,
    os_clipboard: Option<arboard::Clipboard>,
}

impl WorkbookState {
    pub(crate) fn with_user(user: UserModel<'static>, file_path: Option<PathBuf>) -> Self {
        Self {
            user,
            file_path,
            dirty: false,
            charts: Vec::new(),
            next_chart_id: 1,
            visual_rules: Vec::new(),
            autofilters: Vec::new(),
            autofilter_hidden: HashMap::new(),
            bounds_cache: RefCell::new(HashMap::new()),
            hidden_cache: RefCell::new(HashMap::new()),
            internal_clip: None,
            os_clipboard: arboard::Clipboard::new().ok(),
        }
    }

    pub fn invalidate_caches(&self) {
        self.bounds_cache.borrow_mut().clear();
        self.hidden_cache.borrow_mut().clear();
    }

    pub fn new_empty() -> Result<Self> {
        let user = UserModel::new_empty("Untitled", "en", "UTC", "en")
            .map_err(|e| anyhow!("failed to initialize workbook: {e}"))?;
        Ok(Self::with_user(user, None))
    }

    pub fn from_model_clean(model: Model<'static>, file_path: Option<PathBuf>) -> Result<Self> {
        Ok(Self::with_user(UserModel::from_model(model), file_path))
    }

    pub fn visible_sheets(&self) -> Vec<(u32, String)> {
        self.user
            .get_worksheets_properties()
            .into_iter()
            .enumerate()
            .filter(|(_, p)| p.state == "visible" && p.name != META_SHEET_NAME)
            .map(|(idx, p)| (idx as u32, p.name))
            .collect()
    }

    pub fn sheet_index_by_name(&self, name: &str) -> Option<u32> {
        self.user
            .get_worksheets_properties()
            .into_iter()
            .enumerate()
            .find(|(_, p)| p.name.eq_ignore_ascii_case(name) && p.name != META_SHEET_NAME)
            .map(|(idx, _)| idx as u32)
    }

    pub fn active_sheet(&self) -> u32 {
        self.user.get_selected_sheet()
    }

    pub fn active_sheet_name(&self) -> String {
        let s = self.active_sheet() as usize;
        self.user
            .get_worksheets_properties()
            .get(s)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Sheet1".to_string())
    }

    pub fn select_sheet(&mut self, sheet: u32) -> Result<()> {
        self.user
            .set_selected_sheet(sheet)
            .map_err(|e| anyhow!("{e}"))
    }

    pub fn step_sheet(&mut self, delta: isize) -> Result<String> {
        let sheets = self.visible_sheets();
        if sheets.is_empty() {
            return Ok(self.active_sheet_name());
        }
        let cur = self.active_sheet();
        let pos = sheets.iter().position(|(id, _)| *id == cur).unwrap_or(0);
        let next_pos = pos.saturating_add_signed(delta).min(sheets.len() - 1);
        let (target_id, ref name) = sheets[next_pos];
        self.select_sheet(target_id)?;
        Ok(name.clone())
    }

    pub fn add_sheet(&mut self) -> Result<()> {
        self.user.new_sheet().map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn duplicate_active_sheet(&mut self) -> Result<()> {
        let s = self.active_sheet();
        self.user.duplicate_sheet(s).map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn rename_active_sheet(&mut self, new_name: &str) -> Result<()> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() || trimmed == META_SHEET_NAME {
            return Err(anyhow!("invalid sheet name"));
        }
        let old_name = self.active_sheet_name();
        let s = self.active_sheet();
        self.user
            .rename_sheet(s, trimmed)
            .map_err(|e| anyhow!("{e}"))?;
        for ch in &mut self.charts {
            if ch.sheet_name == old_name {
                ch.sheet_name = trimmed.to_string();
            }
        }
        for vr in &mut self.visual_rules {
            if vr.sheet_name_mut() == &old_name {
                *vr.sheet_name_mut() = trimmed.to_string();
            }
        }
        for af in &mut self.autofilters {
            if af.sheet_name == old_name {
                af.sheet_name = trimmed.to_string();
            }
        }
        self.dirty = true;
        Ok(())
    }

    pub fn delete_active_sheet(&mut self) -> Result<()> {
        if self.visible_sheets().len() <= 1 {
            return Err(anyhow!("cannot delete the only visible sheet"));
        }
        let s = self.active_sheet();
        self.user.delete_sheet(s).map_err(|e| anyhow!("{e}"))?;
        self.autofilter_hidden.clear();
        self.invalidate_caches();
        for (idx, _) in self.visible_sheets() {
            let _ = self.reapply_autofilter(idx);
        }
        self.dirty = true;
        Ok(())
    }

    pub fn selected_cell(&self) -> (i32, i32) {
        let (_, r, c) = self.user.get_selected_cell();
        (r.max(1), c.max(1))
    }

    pub fn selected_range(&self) -> [i32; 4] {
        let v = self.user.get_selected_view();
        let [r1, c1, r2, c2] = v.range;
        [
            r1.min(r2).max(1),
            c1.min(c2).max(1),
            r1.max(r2).max(1),
            c1.max(c2).max(1),
        ]
    }

    pub fn selected_area(&self) -> Area {
        let [r1, c1, r2, c2] = self.selected_range();
        Area {
            sheet: self.active_sheet(),
            row: r1,
            column: c1,
            width: c2 - c1 + 1,
            height: r2 - r1 + 1,
        }
    }

    pub fn select_cell(&mut self, row: i32, col: i32) {
        let r = row.clamp(1, MAX_ROWS);
        let c = col.clamp(1, MAX_COLS);
        let _ = self.user.set_selected_cell(r, c);
    }

    pub fn select_range(&mut self, r1: i32, c1: i32, r2: i32, c2: i32) {
        let anchor_r = r1.clamp(1, MAX_ROWS);
        let anchor_c = c1.clamp(1, MAX_COLS);
        let target_r = r2.clamp(1, MAX_ROWS);
        let target_c = c2.clamp(1, MAX_COLS);
        let _ = self.user.set_selected_cell(anchor_r, anchor_c);
        if (anchor_r, anchor_c) != (target_r, target_c) {
            let _ = self
                .user
                .set_selected_range(anchor_r, anchor_c, target_r, target_c);
        }
    }

    pub fn selected_focus(&self) -> (i32, i32) {
        let (ar, ac) = self.selected_cell();
        let [r1, c1, r2, c2] = self.selected_range();
        let fr = if r1 == ar { r2 } else { r1 };
        let fc = if c1 == ac { c2 } else { c1 };
        (fr.clamp(1, MAX_ROWS), fc.clamp(1, MAX_COLS))
    }

    pub fn extend_selection_to(&mut self, target_row: i32, target_col: i32) {
        let (ar, ac) = self.selected_cell();
        let tr = target_row.clamp(1, MAX_ROWS);
        let tc = target_col.clamp(1, MAX_COLS);
        let _ = self.user.set_selected_range(ar, ac, tr, tc);
    }

    pub fn extend_selection_to_edge(&mut self, dir: NavigationDirection) -> (i32, i32) {
        let sheet = self.active_sheet();
        let (fr, fc) = self.selected_focus();
        let (nr, nc) = self
            .user
            .get_model()
            .workbook
            .worksheet(sheet)
            .and_then(|ws| ws.navigate_to_edge_in_direction(fr, fc, dir))
            .unwrap_or((fr, fc));
        let tr = nr.clamp(1, MAX_ROWS);
        let tc = nc.clamp(1, MAX_COLS);
        self.extend_selection_to(tr, tc);
        (tr, tc)
    }

    pub fn cell_display(&self, sheet: u32, row: i32, col: i32) -> String {
        self.user
            .get_formatted_cell_value(sheet, row, col)
            .unwrap_or_default()
    }

    pub fn cell_input_content(&self, sheet: u32, row: i32, col: i32) -> String {
        self.user
            .get_cell_content(sheet, row, col)
            .unwrap_or_default()
    }

    pub fn cell_numeric(&self, sheet: u32, row: i32, col: i32) -> Option<f64> {
        match self
            .user
            .get_model()
            .get_cell_value_by_index(sheet, row, col)
        {
            Ok(CellValue::Number(v)) if v.is_finite() => Some(v),
            _ => None,
        }
    }

    pub fn cell_render_style(&self, sheet: u32, row: i32, col: i32) -> CellRenderStyle {
        let style = self
            .user
            .get_cell_style(sheet, row, col)
            .unwrap_or_default();
        let cell_type = self.user.get_cell_type(sheet, row, col).ok();
        let is_formula = self
            .user
            .get_model()
            .get_cell_formula(sheet, row, col)
            .ok()
            .flatten()
            .is_some();

        let fg = self.resolve_ic_color(&style.font.color);
        let bg = self.resolve_ic_color(&style.fill.color);

        let align = match style.alignment.as_ref().map(|a| &a.horizontal) {
            Some(HorizontalAlignment::Left) => UiAlign::Left,
            Some(HorizontalAlignment::Center | HorizontalAlignment::CenterContinuous) => {
                UiAlign::Center
            }
            Some(HorizontalAlignment::Right) => UiAlign::Right,
            _ => match cell_type {
                Some(CellType::Number) => UiAlign::Right,
                Some(CellType::LogicalValue | CellType::ErrorValue) => UiAlign::Center,
                _ => UiAlign::Left,
            },
        };

        CellRenderStyle {
            bold: style.font.b,
            italic: style.font.i,
            underline: style.font.u,
            strikethrough: style.font.strike,
            fg,
            bg,
            align,
            is_error: matches!(cell_type, Some(CellType::ErrorValue)),
            is_formula,
        }
    }

    fn resolve_ic_color(&self, color: &IcColor) -> Option<Color> {
        if matches!(color, IcColor::None) {
            return None;
        }
        parse_hex_color(&self.user.resolve_color(color))
    }

    pub fn set_cell_input(&mut self, sheet: u32, row: i32, col: i32, value: String) -> Result<()> {
        let was_array = self
            .user
            .get_model()
            .workbook
            .worksheet(sheet)
            .ok()
            .and_then(|ws| ws.cell(row, col))
            .is_some_and(|c| matches!(*c, IcCell::ArrayFormula { .. }));

        self.user
            .set_user_input(sheet, row, col, &value)
            .map_err(|e| anyhow!("{e}"))?;

        let (is_new_array, is_now_empty) = self
            .user
            .get_model()
            .workbook
            .worksheet(sheet)
            .ok()
            .and_then(|ws| ws.cell(row, col))
            .map(|c| {
                (
                    matches!(*c, IcCell::ArrayFormula { .. }),
                    matches!(*c, IcCell::EmptyCell { .. }),
                )
            })
            .unwrap_or((false, true));

        // Maintain `bounds_cache` in O(1) for single-cell edits so committing a cell on a
        // 200k-row sheet doesn't trigger a 2M-cell rescan on the next scrollbar draw.
        let cached_bounds = self.bounds_cache.borrow().get(&sheet).copied();
        if was_array || is_new_array {
            self.bounds_cache.borrow_mut().remove(&sheet);
        } else if let Some((mr, mc)) = cached_bounds {
            if !is_now_empty {
                self.bounds_cache
                    .borrow_mut()
                    .insert(sheet, (mr.max(row), mc.max(col)));
            } else if row >= mr || col >= mc {
                self.bounds_cache.borrow_mut().remove(&sheet);
            }
        }
        self.dirty = true;
        Ok(())
    }

    pub fn clear_selection(&mut self, include_formatting: bool) -> Result<()> {
        let area = self.selected_area();
        if include_formatting {
            self.user
                .range_clear_all(&area)
                .map_err(|e| anyhow!("{e}"))?;
        } else {
            self.user
                .range_clear_contents(&area)
                .map_err(|e| anyhow!("{e}"))?;
        }
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool> {
        if !self.user.can_undo() {
            return Ok(false);
        }
        self.user.undo().map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool> {
        if !self.user.can_redo() {
            return Ok(false);
        }
        self.user.redo().map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(true)
    }

    pub fn copy_or_cut(&mut self, is_cut: bool) -> Result<usize> {
        let clip = self.user.copy_to_clipboard().map_err(|e| anyhow!("{e}"))?;
        let val = serde_json::to_value(clip).context("clipboard serialize failed")?;
        let mirror: ClipboardMirror =
            serde_json::from_value(val).context("clipboard mirror failed")?;
        let cells_count = ((mirror.range.2 - mirror.range.0 + 1).max(1)
            * (mirror.range.3 - mirror.range.1 + 1).max(1)) as usize;
        let synced_to_os = write_system_clipboard(&mut self.os_clipboard, &mirror.csv);
        self.internal_clip = Some(InternalClip {
            tsv: mirror.csv,
            data: mirror.data,
            sheet: mirror.sheet,
            range: mirror.range,
            is_cut,
            synced_to_os,
        });
        Ok(cells_count)
    }

    pub fn paste(&mut self) -> Result<()> {
        let os_text = read_system_clipboard(&mut self.os_clipboard);
        let use_internal = match (&self.internal_clip, &os_text) {
            (Some(ic), _) if !ic.synced_to_os => true,
            (Some(ic), Some(ext)) => ext.trim() == ic.tsv.trim(),
            (Some(_), None) => true,
            (None, _) => false,
        };

        if use_internal {
            let Self {
                user,
                internal_clip,
                dirty,
                ..
            } = self;
            let ic = internal_clip.as_ref().unwrap();
            user.paste_from_clipboard(ic.sheet, ic.range, &ic.data, ic.is_cut)
                .map_err(|e| anyhow!("{e}"))?;
            if ic.is_cut {
                *internal_clip = None;
            }
            *dirty = true;
            self.invalidate_caches();
            return Ok(());
        }

        if let Some(ext) = os_text
            && !ext.is_empty()
        {
            self.paste_delimited_text(&ext)?;
            return Ok(());
        }
        Err(anyhow!("clipboard is empty"))
    }

    fn paste_delimited_text(&mut self, text: &str) -> Result<()> {
        let (start_r, start_c) = self.selected_cell();
        let sheet = self.active_sheet();
        let delim = if text.contains('\t') { b'\t' } else { b',' };
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(delim)
            .from_reader(text.as_bytes());
        self.user.pause_evaluation();
        for (dr, rec) in rdr.records().enumerate() {
            if let Ok(rec) = rec {
                let r = start_r + dr as i32;
                if r > MAX_ROWS {
                    break;
                }
                for (dc, val) in rec.iter().enumerate() {
                    let c = start_c + dc as i32;
                    if c > MAX_COLS {
                        break;
                    }
                    let _ = self.user.set_user_input(sheet, r, c, val);
                }
            }
        }
        self.user.resume_evaluation();
        self.user.evaluate();
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn toggle_style_flag(&mut self, path: &str) -> Result<()> {
        let (r, c) = self.selected_cell();
        let style = self
            .user
            .get_cell_style(self.active_sheet(), r, c)
            .unwrap_or_default();
        let cur = match path {
            "font.b" => style.font.b,
            "font.i" => style.font.i,
            "font.u" => style.font.u,
            "font.strike" => style.font.strike,
            _ => false,
        };
        let area = self.selected_area();
        self.user
            .update_range_style(&area, path, if cur { "false" } else { "true" })
            .map_err(|e| anyhow!("{e}"))?;
        self.dirty = true;
        Ok(())
    }

    pub fn apply_style(&mut self, path: &str, value: &str) -> Result<()> {
        let area = self.selected_area();
        self.user
            .update_range_style(&area, path, value)
            .map_err(|e| anyhow!("{e}"))?;
        self.dirty = true;
        Ok(())
    }

    pub fn col_width_chars(&self, sheet: u32, col: i32) -> u16 {
        let px = self.user.get_column_width(sheet, col).unwrap_or(90.0);
        ((px / COLUMN_WIDTH_FACTOR).round() as i32).clamp(3, 80) as u16
    }

    pub fn set_col_width_chars(&mut self, sheet: u32, col: i32, chars: u16) -> Result<()> {
        let clamped = chars.clamp(3, 80) as f64 * COLUMN_WIDTH_FACTOR;
        self.user
            .set_columns_width(sheet, col, col, clamped)
            .map_err(|e| anyhow!("{e}"))?;
        self.dirty = true;
        Ok(())
    }

    pub fn autofit_column(&mut self, sheet: u32, col: i32) -> Result<u16> {
        let header_len = col_to_letters(col).len() + 2;
        let mut max_len = header_len.max(6);
        let max_r = self.sheet_bounds(sheet).0;
        if max_r >= 1 {
            let stride = ((max_r as usize) / 4_000).max(1) as i32;
            let mut r = 1;
            while r <= max_r {
                let text = self.cell_display(sheet, r, col);
                if !text.is_empty() {
                    max_len = max_len.max(text.chars().count() + 2);
                }
                r += stride;
            }
            if max_r > 1 && (max_r - 1) % stride != 0 {
                let text = self.cell_display(sheet, max_r, col);
                if !text.is_empty() {
                    max_len = max_len.max(text.chars().count() + 2);
                }
            }
        }
        let width = max_len.clamp(4, 60) as u16;
        self.set_col_width_chars(sheet, col, width)?;
        Ok(width)
    }

    pub fn insert_rows_at_selection(&mut self, count: i32, below: bool) -> Result<()> {
        let (r, _) = self.selected_cell();
        let target = if below { (r + 1).min(MAX_ROWS) } else { r };
        self.user
            .insert_rows(self.active_sheet(), target, count)
            .map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn delete_selected_rows(&mut self) -> Result<()> {
        let [r1, _, r2, _] = self.selected_range();
        self.user
            .delete_rows(self.active_sheet(), r1, r2 - r1 + 1)
            .map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn insert_cols_at_selection(&mut self, count: i32, right: bool) -> Result<()> {
        let (_, c) = self.selected_cell();
        let target = if right { (c + 1).min(MAX_COLS) } else { c };
        self.user
            .insert_columns(self.active_sheet(), target, count)
            .map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn delete_selected_cols(&mut self) -> Result<()> {
        let [_, c1, _, c2] = self.selected_range();
        self.user
            .delete_columns(self.active_sheet(), c1, c2 - c1 + 1)
            .map_err(|e| anyhow!("{e}"))?;
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn frozen_counts(&self, sheet: u32) -> (i32, i32) {
        let r = self.user.get_frozen_rows_count(sheet).unwrap_or(0).max(0);
        let c = self
            .user
            .get_frozen_columns_count(sheet)
            .unwrap_or(0)
            .max(0);
        (r, c)
    }

    pub fn set_frozen(&mut self, rows: i32, cols: i32) -> Result<()> {
        let s = self.active_sheet();
        self.user
            .set_frozen_rows_count(s, rows.max(0))
            .map_err(|e| anyhow!("{e}"))?;
        self.user
            .set_frozen_columns_count(s, cols.max(0))
            .map_err(|e| anyhow!("{e}"))?;
        self.dirty = true;
        Ok(())
    }

    pub fn fill_drag(
        &mut self,
        source: [i32; 4],
        target_row: i32,
        target_col: i32,
    ) -> Result<[i32; 4]> {
        let [r1, c1, r2, c2] = source;
        let area = Area {
            sheet: self.active_sheet(),
            row: r1,
            column: c1,
            width: c2 - c1 + 1,
            height: r2 - r1 + 1,
        };
        let v_delta = if target_row > r2 {
            target_row - r2
        } else if target_row < r1 {
            r1 - target_row
        } else {
            0
        };
        let h_delta = if target_col > c2 {
            target_col - c2
        } else if target_col < c1 {
            c1 - target_col
        } else {
            0
        };

        if v_delta == 0 && h_delta == 0 {
            return Ok(source);
        }

        if v_delta >= h_delta {
            self.user
                .auto_fill_rows(&area, target_row)
                .map_err(|e| anyhow!("{e}"))?;
            self.invalidate_caches();
            self.dirty = true;
            Ok([r1.min(target_row), c1, r2.max(target_row), c2])
        } else {
            self.user
                .auto_fill_columns(&area, target_col)
                .map_err(|e| anyhow!("{e}"))?;
            self.invalidate_caches();
            self.dirty = true;
            Ok([r1, c1.min(target_col), r2, c2.max(target_col)])
        }
    }

    /// Excel `Ctrl+D`: fills top row of selection down (or copies row `r1 - 1` into `r1` if 1 row selected).
    pub fn fill_down(&mut self) -> Result<[i32; 4]> {
        let [r1, c1, r2, c2] = self.selected_range();
        let sheet = self.active_sheet();
        let (src_r, dst_r) = if r2 > r1 {
            (r1, r2)
        } else if r1 > 1 {
            (r1 - 1, r1)
        } else {
            return Ok([r1, c1, r2, c2]);
        };
        let src = Area {
            sheet,
            row: src_r,
            column: c1,
            width: c2 - c1 + 1,
            height: 1,
        };
        self.user
            .auto_fill_rows(&src, dst_r)
            .map_err(|e| anyhow!("{e}"))?;
        self.select_range(r1, c1, r2, c2);
        self.invalidate_caches();
        self.dirty = true;
        Ok([r1, c1, r2, c2])
    }

    /// Excel `Ctrl+R`: fills left column of selection right (or copies col `c1 - 1` into `c1` if 1 col selected).
    pub fn fill_right(&mut self) -> Result<[i32; 4]> {
        let [r1, c1, r2, c2] = self.selected_range();
        let sheet = self.active_sheet();
        let (src_c, dst_c) = if c2 > c1 {
            (c1, c2)
        } else if c1 > 1 {
            (c1 - 1, c1)
        } else {
            return Ok([r1, c1, r2, c2]);
        };
        let src = Area {
            sheet,
            row: r1,
            column: src_c,
            width: 1,
            height: r2 - r1 + 1,
        };
        self.user
            .auto_fill_columns(&src, dst_c)
            .map_err(|e| anyhow!("{e}"))?;
        self.select_range(r1, c1, r2, c2);
        self.invalidate_caches();
        self.dirty = true;
        Ok([r1, c1, r2, c2])
    }

    /// Excel `Ctrl+Enter`: enters `input` across `range` (with relative formula adjustment) and keeps `range` selected.
    pub fn fill_range_with_input(
        &mut self,
        sheet: u32,
        range: [i32; 4],
        input: String,
    ) -> Result<()> {
        let [r1, c1, r2, c2] = range;
        let _ = self.select_sheet(sheet);
        self.set_cell_input(sheet, r1, c1, input)?;
        if c2 > c1 {
            let src_col = Area {
                sheet,
                row: r1,
                column: c1,
                width: 1,
                height: 1,
            };
            self.user
                .auto_fill_columns(&src_col, c2)
                .map_err(|e| anyhow!("{e}"))?;
        }
        if r2 > r1 {
            let src_row = Area {
                sheet,
                row: r1,
                column: c1,
                width: c2 - c1 + 1,
                height: 1,
            };
            self.user
                .auto_fill_rows(&src_row, r2)
                .map_err(|e| anyhow!("{e}"))?;
        }
        self.select_range(r1, c1, r2, c2);
        self.invalidate_caches();
        Ok(())
    }

    /// Excel `Alt+=` AutoSum target cell `(row, col)` and `=SUM(...)` formula.
    pub fn autosum_target_and_formula(&self) -> (i32, i32, String) {
        let sheet = self.active_sheet();
        let [r1, c1, r2, c2] = self.selected_range();
        if r2 > r1 && c1 == c2 {
            let col_s = col_to_letters(c1);
            if self.cell_input_content(sheet, r2, c1).trim().is_empty() {
                return (r2, c1, format!("=SUM({col_s}{r1}:{col_s}{})", r2 - 1));
            } else if r2 < MAX_ROWS {
                return (r2 + 1, c1, format!("=SUM({col_s}{r1}:{col_s}{r2})"));
            }
        } else if c2 > c1 && r1 == r2 {
            if self.cell_input_content(sheet, r1, c2).trim().is_empty() {
                return (
                    r1,
                    c2,
                    format!(
                        "=SUM({}{r1}:{}{r1})",
                        col_to_letters(c1),
                        col_to_letters(c2 - 1)
                    ),
                );
            } else if c2 < MAX_COLS {
                return (
                    r1,
                    c2 + 1,
                    format!(
                        "=SUM({}{r1}:{}{r1})",
                        col_to_letters(c1),
                        col_to_letters(c2)
                    ),
                );
            }
        }

        let (r, c) = self.selected_cell();
        let mut top_r = r;
        while top_r > 1 && self.cell_numeric(sheet, top_r - 1, c).is_some() {
            top_r -= 1;
        }
        if top_r < r {
            let col_s = col_to_letters(c);
            return (r, c, format!("=SUM({col_s}{top_r}:{col_s}{})", r - 1));
        }

        let mut left_c = c;
        while left_c > 1 && self.cell_numeric(sheet, r, left_c - 1).is_some() {
            left_c -= 1;
        }
        if left_c < c {
            return (
                r,
                c,
                format!(
                    "=SUM({}{r}:{}{r})",
                    col_to_letters(left_c),
                    col_to_letters(c - 1)
                ),
            );
        }

        if r > 1 {
            let col_s = col_to_letters(c);
            (r, c, format!("=SUM({col_s}1:{col_s}{})", r - 1))
        } else {
            (r, c, "=SUM()".to_string())
        }
    }

    pub fn move_range(&mut self, src: [i32; 4], dst_r1: i32, dst_c1: i32) -> Result<[i32; 4]> {
        let [r1, c1, r2, c2] = src;
        let h = r2 - r1;
        let w = c2 - c1;
        let tr1 = dst_r1.clamp(1, MAX_ROWS - h);
        let tc1 = dst_c1.clamp(1, MAX_COLS - w);
        if tr1 == r1 && tc1 == c1 {
            return Ok(src);
        }
        let sheet = self.active_sheet();
        self.select_range(r1, c1, r2, c2);
        let clip = self.user.copy_to_clipboard().map_err(|e| anyhow!("{e}"))?;
        let val = serde_json::to_value(clip).context("clipboard serialize failed")?;
        let mirror: ClipboardMirror =
            serde_json::from_value(val).context("clipboard mirror failed")?;
        self.select_cell(tr1, tc1);
        self.user
            .paste_from_clipboard(sheet, mirror.range, &mirror.data, true)
            .map_err(|e| anyhow!("{e}"))?;
        let final_r = [tr1, tc1, tr1 + h, tc1 + w];
        self.select_range(final_r[0], final_r[1], final_r[2], final_r[3]);
        self.invalidate_caches();
        self.dirty = true;
        Ok(final_r)
    }

    pub fn aux_snapshot(&self) -> AuxSnapshot {
        AuxSnapshot {
            charts: self.charts.clone(),
            next_chart_id: self.next_chart_id,
            visual_rules: self.visual_rules.clone(),
            autofilters: self.autofilters.clone(),
        }
    }

    pub fn restore_aux(&mut self, snap: AuxSnapshot) {
        self.charts = snap.charts;
        self.next_chart_id = snap.next_chart_id;
        self.visual_rules = snap.visual_rules;
        self.autofilters = snap.autofilters;
        self.autofilter_hidden.clear();
        let _ = self.reapply_autofilter(self.active_sheet());
        self.invalidate_caches();
        self.dirty = true;
    }
}

fn write_system_clipboard(cb: &mut Option<arboard::Clipboard>, text: &str) -> bool {
    let os_ok = cb
        .as_mut()
        .and_then(|c| c.set_text(text.to_string()).ok())
        .is_some();
    // Emit OSC 52 only when stdout is a real terminal so test output stays clean.
    if stdout().is_terminal() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        if encoded.len() <= 100_000 {
            let mut out = stdout();
            let _ = write!(out, "\x1b]52;c;{encoded}\x07");
            let _ = out.flush();
        }
    }
    os_ok
}

fn read_system_clipboard(cb: &mut Option<arboard::Clipboard>) -> Option<String> {
    cb.as_mut()?.get_text().ok()
}
