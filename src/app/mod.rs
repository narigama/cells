pub mod commands;
pub mod state;

pub use commands::{current_date_iso, current_time_hm};
pub use state::*;

use crate::address::{MAX_COLS, MAX_ROWS};
use crate::chart::{
    ChartHoverHit, ChartReady, ChartRenderKey, ChartWorker, ResolvedChartData, resolve_chart_data,
};
use crate::config::{Config, Palette};
use crate::cursor::{CursorShape, SoftwareCursorRuntime, TerminalCursorCaps};
use crate::formula::{
    FormulaAssist, FormulaPrecedent, compute_formula_assist_with_options, extract_precedents,
};
use crate::workbook::{
    ChartSpec, EmbeddedMeta, SelectionStats, WorkbookState, check_orphaned_recovery,
};
use ratatui_image::picker::{Picker, ProtocolType};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

type RangeCacheKey = (u64, u32, [i32; 4]);
type MinMaxMap = HashMap<RangeCacheKey, Option<(f64, f64)>>;

pub struct App {
    pub wb: WorkbookState,
    pub cfg: Config,
    pub palette: Palette,
    pub picker: Picker,
    pub cursor_rt: SoftwareCursorRuntime,
    pub mouse_cell: Option<(u16, u16)>,
    pub mouse_px: Option<(u16, u16)>,
    pub cursor_shape: CursorShape,

    pub force_braille: bool,
    pub show_formulas: bool,
    pub chart_worker: ChartWorker,
    pub cached_chart: Option<ChartReady>,
    pub pending_chart_key: Option<ChartRenderKey>,
    pub chart_hover: Option<ChartHoverHit>,
    pub data_rev: u64,

    pub dock_visible: bool,
    pub chart_fullscreen: bool,
    pub active_chart_idx: usize,

    pub scroll: HashMap<u32, (i32, i32)>,

    pub edit: Option<EditState>,
    pub formula_assist: FormulaAssist,
    pub formula_precedents: Vec<FormulaPrecedent>,
    pub sparkline_target: (i32, i32),

    pub top_menu: Option<ActiveTopMenu>,
    pub context_menu: Option<ContextMenuState>,
    pub modal: Modal,

    pub search_highlights: HashSet<(u32, i32, i32)>,
    pub last_search: Option<(String, bool, bool)>,

    pub undo_log: Vec<UndoEntry>,
    pub redo_log: Vec<UndoEntry>,
    pub last_recovery_write: Instant,
    pub last_recovery_rev: u64,
    pub recovery_in_flight: Arc<AtomicBool>,
    pub last_input_at: Instant,

    stats_cache: Option<(RangeCacheKey, SelectionStats)>,
    minmax_cache: RefCell<MinMaxMap>,
    resolved_chart_cache: Option<((u64, ChartSpec), Option<ResolvedChartData>)>,

    pub drag: DragState,
    pub last_click: Option<(Instant, u16, u16)>,
    pub layout: LayoutCache,
    pub status: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(wb: WorkbookState, meta: Option<EmbeddedMeta>, cfg: Config, picker: Picker) -> Self {
        Self::new_with_cursor_caps(wb, meta, cfg, picker, TerminalCursorCaps::default())
    }

    pub fn new_with_sgr(
        wb: WorkbookState,
        meta: Option<EmbeddedMeta>,
        cfg: Config,
        picker: Picker,
        sgr_pixels_active: bool,
    ) -> Self {
        Self::new_with_cursor_caps(
            wb,
            meta,
            cfg,
            picker,
            TerminalCursorCaps {
                is_tty: false,
                sgr_pixels: sgr_pixels_active,
                osc22_none: false,
            },
        )
    }

    pub fn new_with_cursor_caps(
        wb: WorkbookState,
        meta: Option<EmbeddedMeta>,
        cfg: Config,
        picker: Picker,
        cursor_caps: TerminalCursorCaps,
    ) -> Self {
        let mut cfg = cfg;
        let mut dock_visible = !wb.charts.is_empty();
        if let Some(m) = meta {
            dock_visible = m.dock_visible || !wb.charts.is_empty();
            cfg.dock_position = m.dock_position;
            if (20..=80).contains(&m.dock_percent) {
                cfg.dock_percent = m.dock_percent;
            }
        }
        let palette = Palette::for_theme(cfg.theme);
        let cursor_rt = SoftwareCursorRuntime::with_caps(&picker, cursor_caps);
        let gfx_mode = if picker.protocol_type() != ProtocolType::Halfblocks {
            "Pixel"
        } else {
            "Braille"
        };
        let status = if check_orphaned_recovery(wb.file_path.as_deref()).is_some() {
            "Recovery snapshot available (File → Restore Recovery Snapshot)".to_string()
        } else {
            format!("{gfx_mode} mode — F1 Help · Ctrl+P Commands")
        };
        Self {
            wb,
            palette,
            cfg,
            picker,
            cursor_rt,
            mouse_cell: None,
            mouse_px: None,
            cursor_shape: CursorShape::CellCross,
            force_braille: false,
            show_formulas: false,
            chart_worker: ChartWorker::new(),
            cached_chart: None,
            pending_chart_key: None,
            chart_hover: None,
            data_rev: 1,
            dock_visible,
            chart_fullscreen: false,
            active_chart_idx: 0,
            scroll: HashMap::new(),
            edit: None,
            formula_assist: FormulaAssist::default(),
            formula_precedents: Vec::new(),
            sparkline_target: (1, 1),
            top_menu: None,
            context_menu: None,
            modal: Modal::None,
            search_highlights: HashSet::new(),
            last_search: None,
            undo_log: Vec::new(),
            redo_log: Vec::new(),
            last_recovery_write: Instant::now(),
            last_recovery_rev: 1,
            recovery_in_flight: Arc::new(AtomicBool::new(false)),
            last_input_at: Instant::now(),
            stats_cache: None,
            minmax_cache: RefCell::new(HashMap::new()),
            resolved_chart_cache: None,
            drag: DragState::None,
            last_click: None,
            layout: LayoutCache::default(),
            status,
            should_quit: false,
        }
    }

    pub fn use_pixel_charts(&self) -> bool {
        !self.force_braille && self.picker.protocol_type() != ProtocolType::Halfblocks
    }

    pub fn bump_data_rev(&mut self) {
        self.wb.invalidate_caches();
        self.bump_data_rev_preserving_sheet_caches();
    }

    pub fn bump_data_rev_preserving_sheet_caches(&mut self) {
        self.data_rev = self.data_rev.wrapping_add(1);
        self.stats_cache = None;
        self.minmax_cache.borrow_mut().clear();
        self.resolved_chart_cache = None;
    }

    pub fn cached_selection_stats(&mut self) -> SelectionStats {
        let key = (
            self.data_rev,
            self.wb.active_sheet(),
            self.wb.selected_range(),
        );
        if let Some((ref k, stats)) = self.stats_cache
            && *k == key
        {
            return stats;
        }
        let stats = self.wb.selection_stats();
        self.stats_cache = Some((key, stats));
        stats
    }

    pub fn cached_range_min_max(&self, sheet: u32, range: [i32; 4]) -> Option<(f64, f64)> {
        let key = (self.data_rev, sheet, range);
        if let Some(&cached) = self.minmax_cache.borrow().get(&key) {
            return cached;
        }
        let computed = self.wb.range_min_max(sheet, range);
        self.minmax_cache.borrow_mut().insert(key, computed);
        computed
    }

    pub fn cached_chart_data(&mut self, spec: &ChartSpec) -> Option<ResolvedChartData> {
        if let Some((ref k, ref cached)) = self.resolved_chart_cache
            && k.0 == self.data_rev
            && k.1 == *spec
        {
            return cached.clone();
        }
        let computed = resolve_chart_data(&self.wb, spec);
        self.resolved_chart_cache = Some(((self.data_rev, spec.clone()), computed.clone()));
        computed
    }

    pub fn record_model_undo(&mut self) {
        self.undo_log.push(UndoEntry::ModelMarker);
        self.redo_log.clear();
    }

    pub fn record_aux_undo(&mut self) {
        self.undo_log
            .push(UndoEntry::AuxOnly(self.wb.aux_snapshot()));
        self.redo_log.clear();
    }

    pub fn refresh_formula_state(&mut self) {
        let Some((sheet, row, col, buf, cursor, force)) = self.edit.as_ref().map(|ed| {
            (
                ed.sheet,
                ed.row,
                ed.col,
                ed.buf.clone(),
                ed.cursor,
                ed.force_completion,
            )
        }) else {
            self.formula_assist = FormulaAssist::default();
            self.formula_precedents.clear();
            return;
        };
        self.formula_assist =
            compute_formula_assist_with_options(&mut self.wb, sheet, row, col, &buf, cursor, force);
        self.formula_precedents = extract_precedents(&buf);
        if let Some(ref mut ed) = self.edit {
            if self.formula_assist.candidates.is_empty() {
                ed.completion_idx = 0;
                ed.completion_scroll = 0;
            } else {
                let max_idx = self.formula_assist.candidates.len() - 1;
                ed.completion_idx = ed.completion_idx.min(max_idx);
                ed.completion_scroll = ed.completion_scroll.min(max_idx);
            }
        }
    }

    pub fn tick_housekeeping(&mut self) {
        if !self.wb.dirty
            || self.last_recovery_rev == self.data_rev
            || self.edit.is_some()
            || !self.modal.is_none()
            || self.last_input_at.elapsed() < Duration::from_secs(2)
        {
            return;
        }
        // shortcut: cap auto-recovery bitcode snapshots at <= 250k rows (and only after 2s idle)
        // because `UserModel::to_bytes()` runs on `&UserModel` before handing off to the worker thread.
        let max_rows = self.wb.sheet_bounds(self.wb.active_sheet()).0;
        if max_rows > 250_000 {
            return;
        }
        let interval = if max_rows > 50_000 {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(15)
        };
        if self.last_recovery_write.elapsed() >= interval {
            if self.wb.spawn_recovery_snapshot(
                &self.cfg,
                self.dock_visible,
                &self.recovery_in_flight,
            ) {
                self.last_recovery_rev = self.data_rev;
            }
            self.last_recovery_write = Instant::now();
        }
    }

    pub fn sheet_scroll(&self, sheet: u32) -> (i32, i32) {
        let (fr, fc) = self.wb.frozen_counts(sheet);
        let (sr, sc) = self.scroll.get(&sheet).copied().unwrap_or((1, 1));
        (sr.max(fr + 1), sc.max(fc + 1))
    }

    pub fn set_sheet_scroll(&mut self, sheet: u32, row: i32, col: i32) {
        let (fr, fc) = self.wb.frozen_counts(sheet);
        self.scroll.insert(
            sheet,
            (row.clamp(fr + 1, MAX_ROWS), col.clamp(fc + 1, MAX_COLS)),
        );
    }

    pub fn scroll_by(&mut self, d_rows: i32, d_cols: i32) {
        let s = self.wb.active_sheet();
        let (sr, sc) = self.sheet_scroll(s);
        self.set_sheet_scroll(s, sr.saturating_add(d_rows), sc.saturating_add(d_cols));
    }

    pub fn scroll_into_view_target(&mut self, r: i32, c: i32) {
        let s = self.wb.active_sheet();
        let (fr, fc) = self.wb.frozen_counts(s);
        let (mut sr, mut sc) = self.sheet_scroll(s);
        let vis_rows = self.layout.visible_scroll_rows.max(1) as i32;
        let vis_cols = self.layout.visible_scroll_cols.max(1) as i32;

        if r > fr {
            if r < sr {
                sr = r;
            } else if r >= sr + vis_rows {
                sr = r - vis_rows + 1;
            }
        }
        if c > fc {
            if c < sc {
                sc = c;
            } else if c >= sc + vis_cols {
                sc = c - vis_cols + 1;
            }
        }
        self.set_sheet_scroll(s, sr, sc);
    }

    pub fn ensure_cursor_visible(&mut self) {
        let (r, c) = self.wb.selected_cell();
        self.scroll_into_view_target(r, c);
    }

    pub fn start_edit_existing(&mut self) {
        let s = self.wb.active_sheet();
        let (r, c) = self.wb.selected_cell();
        let buf = self.wb.cell_input_content(s, r, c);
        self.start_edit_replace(buf);
    }

    pub fn start_edit_replace(&mut self, initial: String) {
        let s = self.wb.active_sheet();
        let (r, c) = self.wb.selected_cell();
        let target_range = self.wb.selected_range();
        let cursor = initial.len();
        self.edit = Some(EditState {
            buf: initial,
            cursor,
            sheet: s,
            row: r,
            col: c,
            target_range,
            formula_ref_span: None,
            completion_idx: 0,
            completion_scroll: 0,
            force_completion: false,
        });
        self.refresh_formula_state();
    }

    pub fn commit_edit(&mut self) {
        let Some(ed) = self.edit.take() else {
            return;
        };
        self.formula_assist = FormulaAssist::default();
        self.formula_precedents.clear();
        let _ = self.wb.select_sheet(ed.sheet);
        self.wb.select_cell(ed.row, ed.col);
        match self.wb.set_cell_input(ed.sheet, ed.row, ed.col, ed.buf) {
            Ok(()) => {
                self.record_model_undo();
                let _ = self
                    .wb
                    .reapply_autofilter_for_cell(ed.sheet, ed.row, ed.col);
                self.bump_data_rev_preserving_sheet_caches();
            }
            Err(e) => self.status = format!("Error: {e}"),
        }
    }

    /// Excel `Ctrl+Enter`: commits active edit across every cell in `target_range` and keeps selection.
    pub fn commit_edit_fill_selection(&mut self) {
        let Some(ed) = self.edit.take() else {
            return;
        };
        self.formula_assist = FormulaAssist::default();
        self.formula_precedents.clear();
        let [r1, c1, r2, c2] = ed.target_range;
        let res = if (r1, c1) == (r2, c2) {
            let _ = self.wb.select_sheet(ed.sheet);
            self.wb.select_cell(ed.row, ed.col);
            self.wb.set_cell_input(ed.sheet, ed.row, ed.col, ed.buf)
        } else {
            self.wb
                .fill_range_with_input(ed.sheet, ed.target_range, ed.buf)
        };
        match res {
            Ok(()) => {
                self.record_model_undo();
                let _ = self.wb.reapply_autofilter(ed.sheet);
                self.bump_data_rev();
            }
            Err(e) => self.status = format!("Error: {e}"),
        }
    }

    pub fn cancel_edit(&mut self) {
        if let Some(ed) = self.edit.take() {
            self.formula_assist = FormulaAssist::default();
            self.formula_precedents.clear();
            let _ = self.wb.select_sheet(ed.sheet);
            self.wb.select_cell(ed.row, ed.col);
        }
    }
}
