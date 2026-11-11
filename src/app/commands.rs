use super::{App, Modal, PromptKind, UndoEntry};
use crate::actions::Action;
use crate::address::{MAX_COLS, col_to_letters, format_range_ref, parse_cell_ref, parse_range_ref};
use crate::chart::{ResolvedChartData, export_chart_png, export_chart_svg, resolve_chart_data};
use crate::config::{Palette, SWATCH_COLORS, ThemeMode, parse_hex_color};
use crate::file_picker::{FilePickerPurpose, FilePickerState, PostSaveAction};
use crate::workbook::{
    ChartSpec, PivotAgg, SortSpec, WorkbookState, check_orphaned_recovery, clear_recovery_snapshot,
    recovery_path_for,
};
use anyhow::Result;
use std::path::PathBuf;

impl App {
    pub fn open_find_replace(&mut self, focus_replace: bool) {
        let (q, cs, all) = self.last_search.clone().unwrap_or_default();
        let matches = self.wb.find_matches(&q, all, cs);
        self.modal = Modal::FindReplace {
            find: q,
            replace: String::new(),
            field: usize::from(focus_replace),
            case_sensitive: cs,
            all_sheets: all,
            matches,
            match_idx: 0,
        };
    }

    pub fn open_prompt(&mut self, kind: PromptKind, title: &'static str, initial: String) {
        let cursor = initial.len();
        self.modal = Modal::Prompt {
            kind,
            title,
            value: initial,
            cursor,
        };
    }

    pub fn open_file_picker(&mut self, purpose: FilePickerPurpose) {
        let start_dir = self
            .wb
            .file_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let cur_stem = self
            .wb
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let default_name = match purpose {
            FilePickerPurpose::Open => String::new(),
            FilePickerPurpose::SaveWorkbook { .. } => self
                .wb
                .file_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| "workbook.xlsx".into()),
            FilePickerPurpose::ExportCsv if cur_stem.is_empty() => "export.csv".into(),
            FilePickerPurpose::ExportCsv => format!("{cur_stem}.csv"),
            FilePickerPurpose::ExportTsv if cur_stem.is_empty() => "export.tsv".into(),
            FilePickerPurpose::ExportTsv => format!("{cur_stem}.tsv"),
            FilePickerPurpose::ExportChartPng => "chart.png".into(),
            FilePickerPurpose::ExportChartSvg => "chart.svg".into(),
        };
        self.modal = Modal::FilePicker(FilePickerState::new(purpose, start_dir, default_name));
    }

    pub fn reset_to_empty_workbook(&mut self) -> Result<()> {
        clear_recovery_snapshot(self.wb.file_path.as_deref());
        self.wb = WorkbookState::new_empty()?;
        self.dock_visible = false;
        self.chart_fullscreen = false;
        self.active_chart_idx = 0;
        self.scroll.clear();
        self.undo_log.clear();
        self.redo_log.clear();
        self.bump_data_rev();
        Ok(())
    }

    fn active_chart_with_data(&self) -> Result<(&ChartSpec, ResolvedChartData)> {
        let spec = self
            .wb
            .charts
            .get(self.active_chart_idx)
            .ok_or_else(|| anyhow::anyhow!("no active chart"))?;
        let data = resolve_chart_data(&self.wb, spec)
            .ok_or_else(|| anyhow::anyhow!("chart range contains no numeric data"))?;
        Ok((spec, data))
    }

    pub fn apply_file_picker_path(
        &mut self,
        purpose: FilePickerPurpose,
        path: PathBuf,
    ) -> Result<()> {
        match purpose {
            FilePickerPurpose::Open => {
                let (wb, meta) = WorkbookState::open_path(&path)?;
                self.wb = wb;
                if let Some(m) = meta {
                    self.dock_visible = m.dock_visible || !self.wb.charts.is_empty();
                    self.cfg.dock_position = m.dock_position;
                    self.cfg.dock_percent = m.dock_percent.clamp(20, 80);
                } else {
                    self.dock_visible = !self.wb.charts.is_empty();
                }
                self.chart_fullscreen = false;
                self.active_chart_idx = 0;
                self.scroll.clear();
                self.undo_log.clear();
                self.redo_log.clear();
                self.bump_data_rev();
                self.status = format!("Opened {}", path.display());
            }
            FilePickerPurpose::SaveWorkbook { after } => {
                self.wb.save_as(&path, &self.cfg, self.dock_visible)?;
                self.last_recovery_rev = self.data_rev;
                match after {
                    PostSaveAction::None => {
                        self.status = format!("Saved {}", path.display());
                    }
                    PostSaveAction::CloseWorkbook => {
                        self.reset_to_empty_workbook()?;
                        self.status = format!("Saved {} and closed workbook", path.display());
                    }
                    PostSaveAction::QuitApp => {
                        clear_recovery_snapshot(self.wb.file_path.as_deref());
                        self.should_quit = true;
                    }
                }
            }
            FilePickerPurpose::ExportCsv => {
                self.wb.export_delimited(&path, b',')?;
                self.status = format!("Exported CSV to {}", path.display());
            }
            FilePickerPurpose::ExportTsv => {
                self.wb.export_delimited(&path, b'\t')?;
                self.status = format!("Exported TSV to {}", path.display());
            }
            FilePickerPurpose::ExportChartPng => {
                let (spec, data) = self.active_chart_with_data()?;
                export_chart_png(&path, spec, &data, &self.palette)?;
                self.status = format!("Exported PNG chart to {}", path.display());
            }
            FilePickerPurpose::ExportChartSvg => {
                let (spec, data) = self.active_chart_with_data()?;
                export_chart_svg(&path, spec, &data, &self.palette)?;
                self.status = format!("Exported SVG chart to {}", path.display());
            }
        }
        Ok(())
    }

    pub fn open_column_filter_modal(&mut self, col: i32) -> Result<()> {
        let sheet = self.wb.active_sheet();
        let sheet_name = self.wb.active_sheet_name();
        if self.wb.active_autofilter(&sheet_name).is_none() {
            self.wb.toggle_autofilter_on_selection()?;
        }
        let af = self
            .wb
            .active_autofilter(&sheet_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("failed to initialize AutoFilter"))?;
        let [r1, c1, r2, c2] = af.range;
        let target_col = col.clamp(c1, c2);
        let draft = af.col_filters.get(&target_col).cloned().unwrap_or_default();
        let num_buf = if draft.numeric_val == 0.0 {
            String::new()
        } else {
            draft.numeric_val.to_string()
        };
        let values = self
            .wb
            .distinct_column_values(sheet, (r1 + 1).min(r2), r2, target_col);
        self.modal = Modal::ColumnFilter {
            sheet,
            col: target_col,
            draft,
            num_buf,
            values,
            selected_val: 0,
            field: 0,
        };
        Ok(())
    }

    fn sync_dock_after_aux_restore(&mut self) {
        if self.wb.charts.is_empty() {
            self.dock_visible = false;
            self.chart_fullscreen = false;
            self.active_chart_idx = 0;
        } else {
            self.dock_visible = true;
            self.active_chart_idx = self.active_chart_idx.min(self.wb.charts.len() - 1);
        }
    }

    pub fn execute_action(&mut self, action: Action) {
        self.top_menu = None;
        self.context_menu = None;
        if self.edit.is_some() && action != Action::EditActiveCell {
            self.commit_edit();
        }

        if let Err(e) = self.try_execute_action(action) {
            self.status = format!("Error: {e}");
        }
    }

    fn try_execute_action(&mut self, action: Action) -> Result<()> {
        if let Some(flag_path) = style_toggle_path(action) {
            self.wb.toggle_style_flag(flag_path)?;
            self.record_model_undo();
            return Ok(());
        }
        if let Some((style_path, style_val)) = style_kv_for_action(action) {
            self.wb.apply_style(style_path, style_val)?;
            self.record_model_undo();
            return Ok(());
        }

        match action {
            Action::NewWorkbook => {
                self.reset_to_empty_workbook()?;
                self.status = "Created new workbook".into();
            }
            Action::OpenFilePrompt => self.open_file_picker(FilePickerPurpose::Open),
            Action::Save => {
                if self.wb.file_path.is_some() {
                    let p = self.wb.save(&self.cfg, self.dock_visible)?;
                    self.last_recovery_rev = self.data_rev;
                    self.status = format!("Saved {}", p.display());
                } else {
                    self.open_file_picker(FilePickerPurpose::SaveWorkbook {
                        after: PostSaveAction::None,
                    });
                }
            }
            Action::SaveAsPrompt => {
                self.open_file_picker(FilePickerPurpose::SaveWorkbook {
                    after: PostSaveAction::None,
                });
            }
            Action::ExportCsvPrompt => self.open_file_picker(FilePickerPurpose::ExportCsv),
            Action::ExportTsvPrompt => self.open_file_picker(FilePickerPurpose::ExportTsv),
            Action::RecoverSnapshot => {
                let orig_path = self.wb.file_path.clone();
                if let Some(rec) = check_orphaned_recovery(orig_path.as_deref())
                    .or_else(|| recovery_path_for(orig_path.as_deref()).filter(|p| p.is_file()))
                {
                    let (mut wb, meta) = WorkbookState::open_path(&rec)?;
                    wb.file_path = orig_path;
                    wb.dirty = true;
                    self.wb = wb;
                    if let Some(m) = meta {
                        self.dock_visible = m.dock_visible || !self.wb.charts.is_empty();
                    }
                    self.bump_data_rev();
                    self.status = "Restored recovery snapshot".into();
                } else {
                    self.status = "No recovery snapshot found".into();
                }
            }
            Action::CloseWorkbook => {
                if self.wb.dirty {
                    self.modal = Modal::ConfirmQuit { quit_app: false };
                } else if self.wb.file_path.is_some() {
                    self.reset_to_empty_workbook()?;
                    self.status = "Closed workbook".into();
                } else {
                    let s = self.wb.active_sheet();
                    let (mr, mc) = self.wb.sheet_bounds(s);
                    if mr > 1 || mc > 1 || !self.wb.cell_display(s, 1, 1).is_empty() {
                        self.reset_to_empty_workbook()?;
                        self.status = "Closed workbook".into();
                    } else {
                        clear_recovery_snapshot(self.wb.file_path.as_deref());
                        self.should_quit = true;
                    }
                }
            }
            Action::Quit => {
                if self.wb.dirty {
                    self.modal = Modal::ConfirmQuit { quit_app: true };
                } else {
                    clear_recovery_snapshot(self.wb.file_path.as_deref());
                    self.should_quit = true;
                }
            }
            Action::Undo => {
                if let Some(UndoEntry::AuxOnly(prev_aux)) = self.undo_log.last().cloned() {
                    self.undo_log.pop();
                    self.redo_log
                        .push(UndoEntry::AuxOnly(self.wb.aux_snapshot()));
                    self.wb.restore_aux(prev_aux);
                    self.sync_dock_after_aux_restore();
                    self.bump_data_rev();
                    self.status = "Undone".into();
                } else {
                    if matches!(self.undo_log.last(), Some(UndoEntry::ModelMarker)) {
                        self.undo_log.pop();
                    }
                    if self.wb.undo()? {
                        self.redo_log.push(UndoEntry::ModelMarker);
                        self.bump_data_rev();
                        self.status = "Undone".into();
                    } else {
                        self.status = "Nothing to undo".into();
                    }
                }
            }
            Action::Redo => {
                if let Some(UndoEntry::AuxOnly(next_aux)) = self.redo_log.last().cloned() {
                    self.redo_log.pop();
                    self.undo_log
                        .push(UndoEntry::AuxOnly(self.wb.aux_snapshot()));
                    self.wb.restore_aux(next_aux);
                    self.sync_dock_after_aux_restore();
                    self.bump_data_rev();
                    self.status = "Redone".into();
                } else {
                    if matches!(self.redo_log.last(), Some(UndoEntry::ModelMarker)) {
                        self.redo_log.pop();
                    }
                    if self.wb.redo()? {
                        self.undo_log.push(UndoEntry::ModelMarker);
                        self.bump_data_rev();
                        self.status = "Redone".into();
                    } else {
                        self.status = "Nothing to redo".into();
                    }
                }
            }
            Action::Cut => {
                let n = self.wb.copy_or_cut(true)?;
                self.status = format!("Cut {n} cell(s)");
            }
            Action::Copy => {
                let n = self.wb.copy_or_cut(false)?;
                self.status = format!("Copied {n} cell(s)");
            }
            Action::Paste => {
                self.wb.paste()?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = "Pasted".into();
            }
            Action::FillDown => {
                let [r1, c1, r2, c2] = self.wb.fill_down()?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = format!("Filled down {}", format_range_ref(None, r1, c1, r2, c2));
            }
            Action::FillRight => {
                let [r1, c1, r2, c2] = self.wb.fill_right()?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = format!("Filled right {}", format_range_ref(None, r1, c1, r2, c2));
            }
            Action::AutoSum => {
                let sheet = self.wb.active_sheet();
                let (tr, tc, formula) = self.wb.autosum_target_and_formula();
                self.wb.select_cell(tr, tc);
                self.ensure_cursor_visible();
                if formula == "=SUM()" {
                    self.start_edit_replace(formula);
                    if let Some(ref mut ed) = self.edit {
                        ed.cursor = 5;
                    }
                } else {
                    self.wb.set_cell_input(sheet, tr, tc, formula.clone())?;
                    self.record_model_undo();
                    self.bump_data_rev();
                    self.status = format!("AutoSum {formula}");
                }
            }
            Action::ClearContents => {
                self.wb.clear_selection(false)?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = "Cleared contents".into();
            }
            Action::ClearAll => {
                self.wb.clear_selection(true)?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = "Cleared contents and formatting".into();
            }
            Action::FindReplace => self.open_find_replace(false),
            Action::GotoCellPrompt => {
                self.open_prompt(
                    PromptKind::GotoCell,
                    "Go to Cell / Range (e.g. B42, Sheet2!A1:D10)",
                    String::new(),
                );
            }
            Action::EditActiveCell => self.start_edit_existing(),
            Action::ToggleTheme => {
                self.cfg.theme = self.cfg.theme.toggle();
                self.palette = Palette::for_theme(self.cfg.theme);
                let _ = self.cfg.save();
                self.bump_data_rev();
                self.status = match self.cfg.theme {
                    ThemeMode::Dark => "Switched to Dark theme".into(),
                    ThemeMode::Light => "Switched to Light theme".into(),
                };
            }
            Action::ToggleShowFormulas => {
                self.show_formulas = !self.show_formulas;
                self.status = if self.show_formulas {
                    "Show Formulas: ON (Ctrl+` to show values)".into()
                } else {
                    "Show Formulas: OFF".into()
                };
            }
            Action::ToggleDock => {
                self.dock_visible = !self.dock_visible;
                if !self.dock_visible {
                    self.chart_fullscreen = false;
                }
            }
            Action::ToggleDockPosition => {
                self.cfg.dock_position = self.cfg.dock_position.toggle();
                let _ = self.cfg.save();
                self.dock_visible = true;
            }
            Action::ToggleChartFullscreen => {
                if !self.wb.charts.is_empty() {
                    self.dock_visible = true;
                    self.chart_fullscreen = !self.chart_fullscreen;
                }
            }
            Action::ToggleChartRenderer => {
                self.force_braille = !self.force_braille;
                self.status = if self.use_pixel_charts() {
                    "Chart renderer: Pixel mode".into()
                } else {
                    "Chart renderer: Braille mode".into()
                };
            }
            Action::FreezeAtCursor => {
                let (r, c) = self.wb.selected_cell();
                self.wb.set_frozen((r - 1).max(0), (c - 1).max(0))?;
                self.record_model_undo();
                self.status = format!("Frozen {} row(s), {} col(s)", r - 1, c - 1);
            }
            Action::FreezeTopRow => {
                let (_, fc) = self.wb.frozen_counts(self.wb.active_sheet());
                self.wb.set_frozen(1, fc)?;
                self.record_model_undo();
                self.status = "Frozen top row".into();
            }
            Action::FreezeFirstCol => {
                let (fr, _) = self.wb.frozen_counts(self.wb.active_sheet());
                self.wb.set_frozen(fr, 1)?;
                self.record_model_undo();
                self.status = "Frozen first column".into();
            }
            Action::UnfreezePanes => {
                self.wb.set_frozen(0, 0)?;
                self.record_model_undo();
                self.status = "Unfroze panes".into();
            }
            Action::InsertRowAbove | Action::InsertRowBelow => {
                self.wb
                    .insert_rows_at_selection(1, action == Action::InsertRowBelow)?;
                self.record_model_undo();
                self.bump_data_rev();
            }
            Action::DeleteSelectedRows => {
                self.wb.delete_selected_rows()?;
                self.record_model_undo();
                self.bump_data_rev();
            }
            Action::InsertColLeft | Action::InsertColRight => {
                self.wb
                    .insert_cols_at_selection(1, action == Action::InsertColRight)?;
                self.record_model_undo();
                self.bump_data_rev();
            }
            Action::DeleteSelectedCols => {
                self.wb.delete_selected_cols()?;
                self.record_model_undo();
                self.bump_data_rev();
            }
            Action::InsertCurrentDate | Action::InsertCurrentTime => {
                let s = self.wb.active_sheet();
                let (r, c) = self.wb.selected_cell();
                let is_date = action == Action::InsertCurrentDate;
                let stamp = if is_date {
                    current_date_iso()
                } else {
                    current_time_hm()
                };
                self.wb.set_cell_input(s, r, c, stamp.clone())?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = format!("Inserted {} {stamp}", if is_date { "date" } else { "time" });
            }
            Action::NamedRangesModal => {
                let [r1, c1, r2, c2] = self.wb.selected_range();
                let sheet_name = self.wb.active_sheet_name();
                let ref_input = format_range_ref(Some(&sheet_name), r1, c1, r2, c2);
                self.modal = Modal::NamedRanges {
                    selected: 0,
                    name_input: String::new(),
                    ref_input,
                    field: 1,
                };
            }
            Action::InsertSparklinePrompt => {
                let [r1, c1, r2, c2] = self.wb.selected_range();
                let (init, target) = if (r1, c1) != (r2, c2) {
                    (
                        format_range_ref(None, r1, c1, r2, c2),
                        (r1, (c2 + 1).min(MAX_COLS)),
                    )
                } else {
                    (String::new(), (r1, c1))
                };
                self.sparkline_target = target;
                self.open_prompt(
                    PromptKind::InsertSparkline,
                    "Sparkline Source Range (e.g. B2:G2 or B2:G2 -> H2)",
                    init,
                );
            }
            Action::NewSheet => {
                self.wb.add_sheet()?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = format!("Added {}", self.wb.active_sheet_name());
            }
            Action::DuplicateSheet => {
                self.wb.duplicate_active_sheet()?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = "Duplicated sheet".into();
            }
            Action::RenameSheetPrompt => {
                let cur = self.wb.active_sheet_name();
                self.open_prompt(PromptKind::RenameSheet, "Rename Active Sheet", cur);
            }
            Action::DeleteSheet => {
                self.wb.delete_active_sheet()?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = "Deleted sheet".into();
            }
            Action::NextSheet | Action::PrevSheet => {
                let delta = if action == Action::NextSheet { 1 } else { -1 };
                let name = self.wb.step_sheet(delta)?;
                self.bump_data_rev();
                self.status = format!("Sheet: {name}");
            }
            Action::PickTextColor | Action::PickFillColor => {
                let is_fill = action == Action::PickFillColor;
                self.modal = Modal::ColorPicker {
                    is_fill,
                    selected: 1,
                    custom_hex: if is_fill { "#1E3A8A" } else { "#3B82F6" }.into(),
                    editing_hex: false,
                };
            }
            Action::AddDataBars => {
                self.record_aux_undo();
                self.wb.add_data_bar_on_selection("#3B82F6");
                self.bump_data_rev();
                self.status = "Added conditional Data Bars".into();
            }
            Action::AddColorScale => {
                self.record_aux_undo();
                self.wb.add_color_scale_on_selection("#1E3A5F", "#10B981");
                self.bump_data_rev();
                self.status = "Added conditional Color Scale heatmap".into();
            }
            Action::ClearVisualRules => {
                self.record_aux_undo();
                let n = self.wb.clear_visual_rules_on_selection();
                self.bump_data_rev();
                self.status = format!("Cleared {n} visual rule(s)");
            }
            Action::AutoFitCol => {
                let s = self.wb.active_sheet();
                let [_, c1, _, c2] = self.wb.selected_range();
                for c in c1..=c2 {
                    self.wb.autofit_column(s, c)?;
                }
                self.record_model_undo();
                self.status = "Auto-fit column width".into();
            }
            Action::SetColWidthPrompt => {
                let s = self.wb.active_sheet();
                let (_, c) = self.wb.selected_cell();
                let w = self.wb.col_width_chars(s, c);
                self.open_prompt(
                    PromptKind::SetColWidth,
                    "Column Width (characters, 3–80)",
                    w.to_string(),
                );
            }
            Action::SortAsc | Action::SortDesc => {
                let asc = action == Action::SortAsc;
                self.wb.sort_selection(asc)?;
                self.record_model_undo();
                self.bump_data_rev();
                self.status = if asc {
                    "Sorted ascending".into()
                } else {
                    "Sorted descending".into()
                };
            }
            Action::MultiSortModal => {
                let [_, c1, _, c2] = self.wb.selected_range();
                let mut rules = vec![SortSpec {
                    col: c1,
                    ascending: true,
                }];
                if c2 > c1 {
                    rules.push(SortSpec {
                        col: c1 + 1,
                        ascending: true,
                    });
                }
                self.modal = Modal::MultiSort { rules, selected: 0 };
            }
            Action::ToggleAutoFilter => {
                let on = self.wb.toggle_autofilter_on_selection()?;
                self.bump_data_rev();
                self.status = if on {
                    "Enabled AutoFilter — click ▾ on column headers or Data → Filter".into()
                } else {
                    "Disabled AutoFilter and unhid rows".into()
                };
            }
            Action::FilterActiveColumn => {
                let (_, c) = self.wb.selected_cell();
                self.open_column_filter_modal(c)?;
            }
            Action::PivotModal => {
                let [_, c1, _, c2] = self.wb.selected_range();
                let (_, max_c) = self.wb.sheet_bounds(self.wb.active_sheet());
                let v_col = if c2 > c1 {
                    c1 + 1
                } else {
                    (c1 + 1).min(max_c.max(c1))
                };
                self.modal = Modal::Pivot {
                    group_col: c1,
                    val_col: v_col,
                    agg: PivotAgg::Sum,
                    field: 0,
                };
            }
            Action::NewChart(kind) => {
                self.record_aux_undo();
                let idx = self.wb.create_chart_from_selection(kind);
                self.active_chart_idx = idx;
                self.dock_visible = true;
                self.bump_data_rev();
                self.status = format!("Created {} chart", kind.label());
            }
            Action::EditActiveChart => {
                if let Some(mut spec) = self.wb.charts.get(self.active_chart_idx).cloned() {
                    spec.range = format!("{}!{}", spec.sheet_name, spec.range);
                    self.modal = Modal::ChartEditor {
                        idx: self.active_chart_idx,
                        draft: spec,
                        field: 0,
                    };
                } else {
                    self.status = "No chart to edit — create a chart first".into();
                }
            }
            Action::CycleChartKind => {
                if self.wb.charts.get(self.active_chart_idx).is_some() {
                    self.record_aux_undo();
                    let spec = &mut self.wb.charts[self.active_chart_idx];
                    spec.kind = spec.kind.next();
                    self.wb.dirty = true;
                    let lbl = spec.kind.label();
                    self.bump_data_rev();
                    self.status = format!("Switched chart to {lbl}");
                }
            }
            Action::NextChart => {
                if !self.wb.charts.is_empty() {
                    self.active_chart_idx = (self.active_chart_idx + 1) % self.wb.charts.len();
                }
            }
            Action::PrevChart => {
                if !self.wb.charts.is_empty() {
                    self.active_chart_idx =
                        (self.active_chart_idx + self.wb.charts.len() - 1) % self.wb.charts.len();
                }
            }
            Action::ExportChartPngPrompt | Action::ExportChartSvgPrompt => {
                if self.wb.charts.get(self.active_chart_idx).is_some() {
                    let purpose = if action == Action::ExportChartPngPrompt {
                        FilePickerPurpose::ExportChartPng
                    } else {
                        FilePickerPurpose::ExportChartSvg
                    };
                    self.open_file_picker(purpose);
                } else {
                    self.status = "No active chart to export".into();
                }
            }
            Action::DeleteActiveChart => {
                if self.active_chart_idx < self.wb.charts.len() {
                    self.record_aux_undo();
                    self.wb.charts.remove(self.active_chart_idx);
                    if self.active_chart_idx >= self.wb.charts.len() && !self.wb.charts.is_empty() {
                        self.active_chart_idx = self.wb.charts.len() - 1;
                    }
                    if self.wb.charts.is_empty() {
                        self.dock_visible = false;
                        self.chart_fullscreen = false;
                    }
                    self.wb.dirty = true;
                    self.bump_data_rev();
                    self.status = "Deleted chart".into();
                }
            }
            Action::ShowHelp => {
                self.modal = Modal::Help { scroll: 0 };
            }
            // Handled above via `style_toggle_path` and `style_kv_for_action`.
            Action::ToggleBold
            | Action::ToggleItalic
            | Action::ToggleUnderline
            | Action::ToggleStrikethrough
            | Action::AlignLeft
            | Action::AlignCenter
            | Action::AlignRight
            | Action::AlignAuto
            | Action::NumFmtGeneral
            | Action::NumFmtInteger
            | Action::NumFmtDecimal
            | Action::NumFmtCurrency
            | Action::NumFmtPercent
            | Action::NumFmtDate => {}
        }
        Ok(())
    }

    pub fn submit_prompt(&mut self, kind: PromptKind, val: String) {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            return;
        }
        let res: Result<()> = (|| {
            match kind {
                PromptKind::RenameSheet => {
                    self.wb.rename_active_sheet(trimmed)?;
                    self.record_model_undo();
                    self.bump_data_rev();
                    self.status = format!("Renamed sheet to {trimmed}");
                }
                PromptKind::GotoCell => self.jump_to_target(trimmed)?,
                PromptKind::SetColWidth => {
                    let chars: u16 = trimmed.parse()?;
                    let s = self.wb.active_sheet();
                    let [_, c1, _, c2] = self.wb.selected_range();
                    for c in c1..=c2 {
                        self.wb.set_col_width_chars(s, c, chars)?;
                    }
                    self.record_model_undo();
                    self.status = format!("Set column width to {chars}");
                }
                PromptKind::InsertSparkline => {
                    let sheet_name = self.wb.active_sheet_name();
                    let (src_str, target) = if let Some((l, r)) = trimmed.split_once("->") {
                        let tgt = parse_cell_ref(r.trim())
                            .ok_or_else(|| anyhow::anyhow!("invalid target cell: {}", r.trim()))?;
                        (l.trim(), tgt)
                    } else {
                        (trimmed, self.sparkline_target)
                    };
                    let (_, bounds) = parse_range_ref(&sheet_name, src_str)
                        .ok_or_else(|| anyhow::anyhow!("invalid source range: {src_str}"))?;
                    self.record_aux_undo();
                    self.wb.add_sparkline(target.0, target.1, bounds);
                    self.wb.select_cell(target.0, target.1);
                    self.bump_data_rev();
                    self.status = format!(
                        "Added sparkline in {}{}",
                        col_to_letters(target.1),
                        target.0
                    );
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            self.status = format!("Error: {e}");
        }
    }

    pub fn jump_to_target(&mut self, target: &str) -> Result<()> {
        let cur_sheet = self.wb.active_sheet_name();
        if let Ok(row_only) = target.parse::<i32>() {
            let (_, c) = self.wb.selected_cell();
            self.wb.select_cell(row_only, c);
            self.ensure_cursor_visible();
            return Ok(());
        }
        if let Some((_, _, expr)) = self
            .wb
            .defined_names()
            .into_iter()
            .find(|(n, _, _)| n.eq_ignore_ascii_case(target.trim()))
        {
            let clean = expr.trim_start_matches('=');
            if let Some((sheet_name, [r1, c1, r2, c2])) = parse_range_ref(&cur_sheet, clean) {
                if let Some(idx) = self.wb.sheet_index_by_name(&sheet_name) {
                    self.wb.select_sheet(idx)?;
                }
                self.wb.select_range(r1, c1, r2, c2);
                self.ensure_cursor_visible();
                return Ok(());
            }
        }
        if let Some((sheet_name, [r1, c1, r2, c2])) = parse_range_ref(&cur_sheet, target) {
            if let Some(idx) = self.wb.sheet_index_by_name(&sheet_name) {
                self.wb.select_sheet(idx)?;
            }
            self.wb.select_range(r1, c1, r2, c2);
            self.ensure_cursor_visible();
            return Ok(());
        }
        Err(anyhow::anyhow!("invalid cell or range reference: {target}"))
    }

    pub fn apply_color_choice(&mut self, is_fill: bool, selected: usize, custom_hex: &str) {
        let hex = if selected < SWATCH_COLORS.len() {
            SWATCH_COLORS[selected].1.to_string()
        } else {
            let clean = custom_hex.trim();
            if parse_hex_color(clean).is_none() {
                self.status = format!("Invalid #RRGGBB hex color: {clean}");
                return;
            }
            if clean.starts_with('#') {
                clean.to_uppercase()
            } else {
                format!("#{}", clean.to_uppercase())
            }
        };
        let path = if is_fill { "fill.color" } else { "font.color" };
        match self.wb.apply_style(path, &hex) {
            Ok(()) => {
                self.record_model_undo();
                self.status = "Applied color".into();
            }
            Err(e) => self.status = format!("Error: {e}"),
        }
    }

    pub fn step_search_match(&mut self, forward: bool) {
        let Some((ref q, cs, all)) = self.last_search else {
            return;
        };
        let matches = self.wb.find_matches(q, all, cs);
        if matches.is_empty() {
            self.status = "Pattern not found".into();
            return;
        }
        let (cr, cc) = self.wb.selected_cell();
        let cur = (self.wb.active_sheet(), cr, cc);
        let idx = if forward {
            matches.iter().position(|&m| m > cur).unwrap_or(0)
        } else {
            matches
                .iter()
                .rposition(|&m| m < cur)
                .unwrap_or(matches.len() - 1)
        };
        let (s, r, c) = matches[idx];
        let _ = self.wb.select_sheet(s);
        self.wb.select_cell(r, c);
        self.ensure_cursor_visible();
        self.status = format!("Match {}/{}", idx + 1, matches.len());
    }
}

fn style_toggle_path(action: Action) -> Option<&'static str> {
    match action {
        Action::ToggleBold => Some("font.b"),
        Action::ToggleItalic => Some("font.i"),
        Action::ToggleUnderline => Some("font.u"),
        Action::ToggleStrikethrough => Some("font.strike"),
        _ => None,
    }
}

fn style_kv_for_action(action: Action) -> Option<(&'static str, &'static str)> {
    match action {
        Action::AlignLeft => Some(("alignment.horizontal", "left")),
        Action::AlignCenter => Some(("alignment.horizontal", "center")),
        Action::AlignRight => Some(("alignment.horizontal", "right")),
        Action::AlignAuto => Some(("alignment", "")),
        Action::NumFmtGeneral => Some(("num_fmt", "general")),
        Action::NumFmtInteger => Some(("num_fmt", "#,##0")),
        Action::NumFmtDecimal => Some(("num_fmt", "#,##0.00")),
        Action::NumFmtCurrency => Some(("num_fmt", "$#,##0.00")),
        Action::NumFmtPercent => Some(("num_fmt", "0.0%")),
        Action::NumFmtDate => Some(("num_fmt", "yyyy-mm-dd")),
        _ => None,
    }
}

#[cfg(unix)]
fn local_ymdhms() -> (i32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (
        tm.tm_year + 1900,
        (tm.tm_mon + 1).clamp(1, 12) as u32,
        tm.tm_mday.clamp(1, 31) as u32,
        tm.tm_hour.clamp(0, 23) as u32,
        tm.tm_min.clamp(0, 59) as u32,
    )
}

#[cfg(not(unix))]
fn local_ymdhms() -> (i32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400) + 719_468;
    let rem = secs.rem_euclid(86_400);
    let era = days.div_euclid(146_097);
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (
        year as i32,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
    )
}

pub fn current_date_iso() -> String {
    let (y, m, d, _, _) = local_ymdhms();
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn current_time_hm() -> String {
    let (_, _, _, hh, mm) = local_ymdhms();
    format!("{hh:02}:{mm:02}")
}
