use super::WorkbookState;
use super::types::{
    AutoFilterState, ChartKind, ChartSpec, ColumnFilter, NumericFilterOp, PivotAgg, SelectionStats,
    SortSpec, VisualRule,
};
use crate::address::{
    MAX_COLS, MAX_ROWS, col_to_letters, format_range_ref, format_sheet_prefix, parse_range_ref,
};
use anyhow::{Result, anyhow};
use ironcalc_base::expressions::types::Area;
use ironcalc_base::types::{Cell as IcCell, FormulaValue, Style};
use std::collections::{BTreeMap, BTreeSet, HashSet};

impl WorkbookState {
    pub fn sort_selection(&mut self, ascending: bool) -> Result<()> {
        let (_, active_col) = self.selected_cell();
        self.sort_selection_multi(
            &[SortSpec {
                col: active_col,
                ascending,
            }],
            true,
        )
    }

    pub fn sort_selection_multi(
        &mut self,
        specs: &[SortSpec],
        auto_preserve_header: bool,
    ) -> Result<()> {
        if specs.is_empty() {
            return Ok(());
        }
        let sheet = self.active_sheet();
        let (_, active_col) = self.selected_cell();
        let [mut r1, mut c1, mut r2, mut c2] = self.selected_range();
        if r1 == r2 && c1 == c2 {
            let (max_r, max_c) = self.sheet_bounds(sheet);
            if max_r <= 1 {
                return Ok(());
            }
            r1 = 1;
            c1 = 1;
            r2 = max_r;
            c2 = max_c.max(active_col);
        }
        let first_col = specs[0].col.clamp(c1, c2);

        if auto_preserve_header
            && r2 > r1
            && self.cell_numeric(sheet, r1, first_col).is_none()
            && !self.cell_display(sheet, r1, first_col).is_empty()
            && (self.cell_numeric(sheet, r1 + 1, first_col).is_some()
                || (c1..=c2).any(|c| {
                    self.cell_numeric(sheet, r1, c).is_none()
                        && self.cell_numeric(sheet, r1 + 1, c).is_some()
                }))
        {
            r1 += 1;
        }
        if r2 <= r1 {
            return Ok(());
        }

        #[derive(Clone)]
        struct RowSnap {
            keys: Vec<(Option<f64>, String)>,
            inputs: Vec<String>,
            styles: Vec<Style>,
        }

        let mut rows = Vec::with_capacity((r2 - r1 + 1) as usize);
        for r in r1..=r2 {
            let mut inputs = Vec::with_capacity((c2 - c1 + 1) as usize);
            let mut styles = Vec::with_capacity((c2 - c1 + 1) as usize);
            for c in c1..=c2 {
                inputs.push(self.cell_input_content(sheet, r, c));
                styles.push(self.user.get_cell_style(sheet, r, c).unwrap_or_default());
            }
            let keys = specs
                .iter()
                .map(|sp| {
                    let kc = sp.col.clamp(c1, c2);
                    (
                        self.cell_numeric(sheet, r, kc),
                        self.cell_display(sheet, r, kc).to_lowercase(),
                    )
                })
                .collect();
            rows.push(RowSnap {
                keys,
                inputs,
                styles,
            });
        }

        rows.sort_by(|a, b| {
            let empty_a = a.inputs.iter().all(|s| s.is_empty());
            let empty_b = b.inputs.iter().all(|s| s.is_empty());
            if empty_a != empty_b {
                return empty_a.cmp(&empty_b);
            }
            for (i, sp) in specs.iter().enumerate() {
                let (na, ref sa) = a.keys[i];
                let (nb, ref sb) = b.keys[i];
                let ea = na.is_none() && sa.is_empty();
                let eb = nb.is_none() && sb.is_empty();
                if ea != eb {
                    return ea.cmp(&eb);
                }
                let ord = match (na, nb) {
                    (Some(va), Some(vb)) => va.total_cmp(&vb),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => sa.cmp(sb),
                };
                if ord != std::cmp::Ordering::Equal {
                    return if sp.ascending { ord } else { ord.reverse() };
                }
            }
            std::cmp::Ordering::Equal
        });

        self.user.pause_evaluation();
        let mut style_matrix = Vec::with_capacity(rows.len());
        for (idx, snap) in rows.into_iter().enumerate() {
            let r = r1 + idx as i32;
            for (dc, input) in snap.inputs.into_iter().enumerate() {
                let c = c1 + dc as i32;
                let _ = self.user.set_user_input(sheet, r, c, &input);
            }
            style_matrix.push(snap.styles);
        }
        self.user.resume_evaluation();
        self.user.evaluate();
        self.select_range(r1, c1, r2, c2);
        let _ = self.user.on_paste_styles(&style_matrix);
        self.invalidate_caches();
        self.dirty = true;
        Ok(())
    }

    pub fn has_any_hidden_rows(&self, sheet: u32) -> bool {
        if self
            .autofilter_hidden
            .get(&sheet)
            .is_some_and(|s| !s.is_empty())
        {
            return true;
        }
        let mut cache = self.hidden_cache.borrow_mut();
        let set = cache.entry(sheet).or_insert_with(|| {
            self.user
                .get_model()
                .workbook
                .worksheet(sheet)
                .map(|ws| ws.rows.iter().filter(|r| r.hidden).map(|r| r.r).collect())
                .unwrap_or_default()
        });
        !set.is_empty()
    }

    pub fn is_row_hidden(&self, sheet: u32, row: i32) -> bool {
        if self
            .autofilter_hidden
            .get(&sheet)
            .is_some_and(|s| s.contains(&row))
        {
            return true;
        }
        let mut cache = self.hidden_cache.borrow_mut();
        let set = cache.entry(sheet).or_insert_with(|| {
            self.user
                .get_model()
                .workbook
                .worksheet(sheet)
                .map(|ws| ws.rows.iter().filter(|r| r.hidden).map(|r| r.r).collect())
                .unwrap_or_default()
        });
        set.contains(&row)
    }

    pub fn active_autofilter(&self, sheet_name: &str) -> Option<&AutoFilterState> {
        self.autofilters
            .iter()
            .find(|af| af.sheet_name.eq_ignore_ascii_case(sheet_name))
    }

    pub fn active_autofilter_mut(&mut self, sheet_name: &str) -> Option<&mut AutoFilterState> {
        self.autofilters
            .iter_mut()
            .find(|af| af.sheet_name.eq_ignore_ascii_case(sheet_name))
    }

    pub fn toggle_autofilter_on_selection(&mut self) -> Result<bool> {
        let sheet = self.active_sheet();
        let sheet_name = self.active_sheet_name();
        if let Some(pos) = self
            .autofilters
            .iter()
            .position(|af| af.sheet_name.eq_ignore_ascii_case(&sheet_name))
        {
            let removed = self.autofilters.remove(pos);
            let had_fast_hidden = self.autofilter_hidden.remove(&sheet).is_some();
            let [r1, _, r2, _] = removed.range;
            if r2 > r1 && (!had_fast_hidden || (r2 - r1) <= 2_000) {
                let sel = self.selected_range();
                let _ = self.user.set_rows_hidden(sheet, r1 + 1, r2, false);
                self.select_range(sel[0], sel[1], sel[2], sel[3]);
            }
            self.invalidate_caches();
            self.dirty = true;
            return Ok(false);
        }

        let [mut r1, mut c1, mut r2, mut c2] = self.selected_range();
        if r1 == r2 && c1 == c2 {
            let (mr, mc) = self.sheet_bounds(sheet);
            r1 = 1;
            c1 = 1;
            r2 = mr.max(2);
            c2 = mc.max(1);
        }
        self.autofilters.push(AutoFilterState {
            sheet_name,
            range: [r1, c1, r2, c2],
            col_filters: BTreeMap::new(),
        });
        self.dirty = true;
        Ok(true)
    }

    pub fn distinct_column_values(
        &self,
        sheet: u32,
        r_start: i32,
        r_end: i32,
        col: i32,
    ) -> Vec<String> {
        const MAX_DISTINCT: usize = 1_000;
        let max_r = self.sheet_bounds(sheet).0;
        let end = r_end.min(max_r.max(r_start));
        let mut set = BTreeSet::new();
        for r in r_start..=end {
            let v = self.cell_display(sheet, r, col);
            let key = if v.trim().is_empty() {
                "(Blank)".to_string()
            } else {
                v
            };
            set.insert(key);
            if set.len() >= MAX_DISTINCT {
                break;
            }
        }
        set.into_iter().collect()
    }

    pub fn reapply_autofilter(&mut self, sheet: u32) -> Result<usize> {
        let Some((_, sheet_name)) = self.visible_sheets().into_iter().find(|(i, _)| *i == sheet)
        else {
            return Ok(0);
        };
        let Some(af) = self.active_autofilter(&sheet_name).cloned() else {
            self.autofilter_hidden.remove(&sheet);
            return Ok(0);
        };
        let [r1, _c1, r2, _c2] = af.range;
        if r2 <= r1 {
            self.autofilter_hidden.remove(&sheet);
            return Ok(0);
        }

        let mut top10_cutoff: BTreeMap<i32, f64> = BTreeMap::new();
        let mut active_filters: Vec<(i32, &ColumnFilter, String)> = Vec::new();
        for (&col, cf) in &af.col_filters {
            if !cf.is_active() {
                continue;
            }
            if cf.numeric_op == NumericFilterOp::Top10 {
                let mut nums: Vec<f64> = ((r1 + 1)..=r2)
                    .filter_map(|r| self.cell_numeric(sheet, r, col))
                    .collect();
                nums.sort_by(|a, b| b.total_cmp(a));
                if let Some(&cut) = nums.get(9).or_else(|| nums.last()) {
                    top10_cutoff.insert(col, cut);
                }
            }
            active_filters.push((col, cf, cf.substring.trim().to_lowercase()));
        }

        let saved_sel = self.selected_range();
        let mut hidden_set: HashSet<i32> = HashSet::new();
        if !active_filters.is_empty() {
            for r in (r1 + 1)..=r2 {
                let mut keep = true;
                for &(col, cf, ref sub_lc) in &active_filters {
                    let need_disp = !cf.excluded_values.is_empty() || !sub_lc.is_empty();
                    if need_disp {
                        let raw_disp = self.cell_display(sheet, r, col);
                        let key_val = if raw_disp.trim().is_empty() {
                            "(Blank)"
                        } else {
                            raw_disp.as_str()
                        };
                        if cf.excluded_values.contains(key_val) {
                            keep = false;
                            break;
                        }
                        if !sub_lc.is_empty() && !raw_disp.to_lowercase().contains(sub_lc) {
                            keep = false;
                            break;
                        }
                    }
                    if cf.numeric_op != NumericFilterOp::None {
                        let Some(nv) = self.cell_numeric(sheet, r, col) else {
                            keep = false;
                            break;
                        };
                        let pass_num = match cf.numeric_op {
                            NumericFilterOp::None => true,
                            NumericFilterOp::Gt => nv > cf.numeric_val,
                            NumericFilterOp::Lt => nv < cf.numeric_val,
                            NumericFilterOp::Eq => (nv - cf.numeric_val).abs() <= 1e-9,
                            NumericFilterOp::Top10 => top10_cutoff
                                .get(&col)
                                .map(|&cut| nv >= cut)
                                .unwrap_or(false),
                        };
                        if !pass_num {
                            keep = false;
                            break;
                        }
                    }
                }
                if !keep {
                    hidden_set.insert(r);
                }
            }
        }

        let hidden_count = hidden_set.len();
        // shortcut: sheets with > 2,000 filtered rows track visibility in `autofilter_hidden`
        // rather than IronCalc's unsorted `ws.rows` Vec (which is O(N²) to mutate); upgrade if
        // IronCalc indexes `ws.rows` by row number.
        if (r2 - r1) <= 2_000 {
            self.autofilter_hidden.remove(&sheet);
            for r in (r1 + 1)..=r2 {
                let want_hidden = hidden_set.contains(&r);
                if self.is_row_hidden(sheet, r) != want_hidden {
                    let _ = self.user.set_rows_hidden(sheet, r, r, want_hidden);
                }
            }
            self.invalidate_caches();
        } else {
            self.autofilter_hidden.insert(sheet, hidden_set);
        }

        let mut target_r = saved_sel[0];
        if self.is_row_hidden(sheet, target_r) {
            target_r = r1;
        }
        self.select_cell(target_r, saved_sel[1]);
        self.dirty = true;
        Ok(hidden_count)
    }

    /// Incremental AutoFilter refresh after editing `(edited_row, edited_col)` on `sheet`.
    /// Avoids rescanning 200k rows when no filter is active or when a plain value edit
    /// cannot affect other rows.
    pub fn reapply_autofilter_for_cell(
        &mut self,
        sheet: u32,
        edited_row: i32,
        edited_col: i32,
    ) -> Result<usize> {
        let Some((_, sheet_name)) = self.visible_sheets().into_iter().find(|(i, _)| *i == sheet)
        else {
            return Ok(0);
        };
        let Some(af) = self.active_autofilter(&sheet_name).cloned() else {
            self.autofilter_hidden.remove(&sheet);
            return Ok(0);
        };
        let [r1, _c1, r2, _c2] = af.range;
        if r2 <= r1 {
            self.autofilter_hidden.remove(&sheet);
            return Ok(0);
        }
        let any_active = af.col_filters.values().any(|cf| cf.is_active());
        if !any_active {
            if let Some(prev) = self.autofilter_hidden.get(&sheet)
                && prev.is_empty()
            {
                return Ok(0);
            }
            if (r2 - r1) > 2_000 && !self.autofilter_hidden.contains_key(&sheet) {
                return Ok(0);
            }
            return self.reapply_autofilter(sheet);
        }

        let has_top10 = af
            .col_filters
            .values()
            .any(|cf| cf.is_active() && cf.numeric_op == NumericFilterOp::Top10);
        let sheet_has_formulas = self
            .user
            .get_model()
            .workbook
            .worksheet(sheet)
            .is_ok_and(|ws| !ws.shared_formulas.is_empty());

        if (r2 - r1) > 2_000 && !has_top10 && !sheet_has_formulas {
            if edited_row <= r1
                || edited_row > r2
                || !af
                    .col_filters
                    .get(&edited_col)
                    .is_some_and(|cf| cf.is_active())
            {
                return Ok(self.autofilter_hidden.get(&sheet).map_or(0, HashSet::len));
            }
            let mut keep = true;
            for (&col, cf) in &af.col_filters {
                if !cf.is_active() {
                    continue;
                }
                let sub_lc = cf.substring.trim().to_lowercase();
                if !cf.excluded_values.is_empty() || !sub_lc.is_empty() {
                    let raw_disp = self.cell_display(sheet, edited_row, col);
                    let key_val = if raw_disp.trim().is_empty() {
                        "(Blank)"
                    } else {
                        raw_disp.as_str()
                    };
                    if cf.excluded_values.contains(key_val)
                        || (!sub_lc.is_empty() && !raw_disp.to_lowercase().contains(&sub_lc))
                    {
                        keep = false;
                        break;
                    }
                }
                if cf.numeric_op != NumericFilterOp::None {
                    let Some(nv) = self.cell_numeric(sheet, edited_row, col) else {
                        keep = false;
                        break;
                    };
                    let pass = match cf.numeric_op {
                        NumericFilterOp::None | NumericFilterOp::Top10 => true,
                        NumericFilterOp::Gt => nv > cf.numeric_val,
                        NumericFilterOp::Lt => nv < cf.numeric_val,
                        NumericFilterOp::Eq => (nv - cf.numeric_val).abs() <= 1e-9,
                    };
                    if !pass {
                        keep = false;
                        break;
                    }
                }
            }
            let set = self.autofilter_hidden.entry(sheet).or_default();
            if keep {
                set.remove(&edited_row);
            } else {
                set.insert(edited_row);
            }
            return Ok(set.len());
        }

        self.reapply_autofilter(sheet)
    }

    pub fn add_data_bar_on_selection(&mut self, color_hex: &str) {
        let sheet_name = self.active_sheet_name();
        let range = self.selected_range();
        self.visual_rules.retain(|vr| {
            !matches!(
                vr,
                VisualRule::DataBar {
                    sheet_name: sn,
                    range: r,
                    ..
                } if sn.eq_ignore_ascii_case(&sheet_name) && *r == range
            )
        });
        self.visual_rules.push(VisualRule::DataBar {
            sheet_name,
            range,
            color_hex: color_hex.to_string(),
        });
        self.dirty = true;
    }

    pub fn add_color_scale_on_selection(&mut self, low_hex: &str, high_hex: &str) {
        let sheet_name = self.active_sheet_name();
        let range = self.selected_range();
        self.visual_rules.retain(|vr| {
            !matches!(
                vr,
                VisualRule::ColorScale {
                    sheet_name: sn,
                    range: r,
                    ..
                } if sn.eq_ignore_ascii_case(&sheet_name) && *r == range
            )
        });
        self.visual_rules.push(VisualRule::ColorScale {
            sheet_name,
            range,
            low_hex: low_hex.to_string(),
            high_hex: high_hex.to_string(),
        });
        self.dirty = true;
    }

    pub fn add_sparkline(&mut self, target_row: i32, target_col: i32, source_range: [i32; 4]) {
        let sheet_name = self.active_sheet_name();
        self.visual_rules.retain(|vr| {
            !matches!(
                vr,
                VisualRule::Sparkline {
                    sheet_name: sn,
                    target_row: tr,
                    target_col: tc,
                    ..
                } if sn.eq_ignore_ascii_case(&sheet_name) && *tr == target_row && *tc == target_col
            )
        });
        self.visual_rules.push(VisualRule::Sparkline {
            sheet_name,
            target_row,
            target_col,
            source_range,
        });
        self.dirty = true;
    }

    pub fn clear_visual_rules_on_selection(&mut self) -> usize {
        let sheet_name = self.active_sheet_name();
        let [sr1, sc1, sr2, sc2] = self.selected_range();
        let before = self.visual_rules.len();
        self.visual_rules.retain(|vr| match vr {
            VisualRule::DataBar {
                sheet_name: sn,
                range: [r1, c1, r2, c2],
                ..
            }
            | VisualRule::ColorScale {
                sheet_name: sn,
                range: [r1, c1, r2, c2],
                ..
            } => {
                !(sn.eq_ignore_ascii_case(&sheet_name)
                    && *r1 <= sr2
                    && *r2 >= sr1
                    && *c1 <= sc2
                    && *c2 >= sc1)
            }
            VisualRule::Sparkline {
                sheet_name: sn,
                target_row,
                target_col,
                ..
            } => {
                !(sn.eq_ignore_ascii_case(&sheet_name)
                    && (sr1..=sr2).contains(target_row)
                    && (sc1..=sc2).contains(target_col))
            }
        });
        let removed = before.saturating_sub(self.visual_rules.len());
        if removed > 0 {
            self.dirty = true;
        }
        removed
    }

    pub fn range_min_max(
        &self,
        sheet: u32,
        [r1, c1, mut r2, mut c2]: [i32; 4],
    ) -> Option<(f64, f64)> {
        let (mr, mc) = self.sheet_bounds(sheet);
        r2 = r2.min(mr);
        c2 = c2.min(mc);
        if r2 < r1 || c2 < c1 {
            return None;
        }
        let Ok(ws) = self.user.get_model().workbook.worksheet(sheet) else {
            return None;
        };
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        let mut any = false;
        for r in r1..=r2 {
            if let Some(nums) = ws.sheet_data.numbers_in_row_between(r, c1, c2) {
                for &v in nums {
                    if v.is_finite() {
                        lo = lo.min(v);
                        hi = hi.max(v);
                        any = true;
                    }
                }
                continue;
            }
            for (c, cell) in ws.sheet_data.cells_in_row_between(r, c1, c2) {
                let num = match cell.as_ref() {
                    IcCell::NumberCell { v, .. }
                    | IcCell::CellFormula {
                        v: FormulaValue::Number(v),
                        ..
                    }
                    | IcCell::ArrayFormula {
                        v: FormulaValue::Number(v),
                        ..
                    } => v.is_finite().then_some(*v),
                    IcCell::SpillCell { .. } | IcCell::ArrayFormula { .. } => {
                        self.cell_numeric(sheet, r, c)
                    }
                    _ => None,
                };
                if let Some(v) = num {
                    lo = lo.min(v);
                    hi = hi.max(v);
                    any = true;
                }
            }
        }
        any.then_some((lo, hi))
    }

    pub fn compute_sparkline_text(
        &self,
        sheet: u32,
        [r1, c1, mut r2, mut c2]: [i32; 4],
        max_width: usize,
    ) -> String {
        const TICKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let (mr, mc) = self.sheet_bounds(sheet);
        r2 = r2.min(mr);
        c2 = c2.min(mc);
        if r2 < r1 || c2 < c1 || max_width == 0 {
            return String::new();
        }
        let mut vals = Vec::new();
        let row_span = (r2 - r1 + 1) as usize;
        let r_stride = (row_span / 2_048).max(1) as i32;
        let mut r = r1;
        while r <= r2 {
            for c in c1..=c2 {
                if let Some(v) = self.cell_numeric(sheet, r, c) {
                    vals.push(v);
                }
            }
            r += r_stride;
        }
        if vals.is_empty() {
            return String::new();
        }
        let sampled: Vec<f64> = if vals.len() <= max_width {
            vals
        } else {
            (0..max_width)
                .map(|i| {
                    let idx = (i * (vals.len() - 1)) / (max_width - 1).max(1);
                    vals[idx]
                })
                .collect()
        };
        let lo = sampled.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = sampled.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = (hi - lo).abs();
        sampled
            .into_iter()
            .map(|v| {
                let idx = if span < 1e-12 {
                    3
                } else {
                    (((v - lo) / span) * 7.0).round().clamp(0.0, 7.0) as usize
                };
                TICKS[idx]
            })
            .collect()
    }

    pub fn create_pivot_sheet(
        &mut self,
        group_col: i32,
        val_col: i32,
        agg: PivotAgg,
    ) -> Result<String> {
        let src_sheet = self.active_sheet();
        let [mut r1, mut c1, mut r2, mut c2] = self.selected_range();
        if r1 == r2 && c1 == c2 {
            let (mr, mc) = self.sheet_bounds(src_sheet);
            if mr <= 1 {
                return Err(anyhow!("not enough data rows to summarize"));
            }
            r1 = 1;
            c1 = 1;
            r2 = mr;
            c2 = mc;
        }
        let g_col = group_col.clamp(c1, c2);
        let v_col = val_col.clamp(c1, c2);

        let has_hdr = r2 > r1
            && (self.cell_numeric(src_sheet, r1, v_col).is_none()
                && !self.cell_display(src_sheet, r1, g_col).is_empty());
        let g_title = if has_hdr {
            let s = self.cell_display(src_sheet, r1, g_col);
            if s.is_empty() { "Group".into() } else { s }
        } else {
            format!("Col {}", col_to_letters(g_col))
        };
        let v_title = if has_hdr {
            let s = self.cell_display(src_sheet, r1, v_col);
            if s.is_empty() { "Value".into() } else { s }
        } else {
            format!("Col {}", col_to_letters(v_col))
        };
        let data_start = if has_hdr { r1 + 1 } else { r1 };

        #[derive(Default)]
        struct Acc {
            count: usize,
            num_count: usize,
            sum: f64,
            min: f64,
            max: f64,
        }
        let mut groups: BTreeMap<String, Acc> = BTreeMap::new();
        for r in data_start..=r2 {
            if self.is_row_hidden(src_sheet, r) {
                continue;
            }
            let raw_g = self.cell_display(src_sheet, r, g_col);
            let num_v = self.cell_numeric(src_sheet, r, v_col);
            let raw_v = self.cell_display(src_sheet, r, v_col);
            if raw_g.trim().is_empty() && raw_v.trim().is_empty() {
                continue;
            }
            let key = if raw_g.trim().is_empty() {
                "(Blank)".to_string()
            } else {
                raw_g
            };
            let entry = groups.entry(key).or_insert_with(|| Acc {
                count: 0,
                num_count: 0,
                sum: 0.0,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
            });
            entry.count += 1;
            if let Some(v) = num_v {
                entry.num_count += 1;
                entry.sum += v;
                entry.min = entry.min.min(v);
                entry.max = entry.max.max(v);
            }
        }
        if groups.is_empty() {
            return Err(anyhow!("no non-empty rows found for pivot summary"));
        }

        let mut pivot_idx = 1usize;
        let existing_names: Vec<String> = self
            .visible_sheets()
            .into_iter()
            .map(|(_, n)| n.to_lowercase())
            .collect();
        let pivot_name = loop {
            let cand = format!("Pivot {pivot_idx}");
            if !existing_names.contains(&cand.to_lowercase()) {
                break cand;
            }
            pivot_idx += 1;
        };

        self.user.new_sheet().map_err(|e| anyhow!("{e}"))?;
        let new_sheet = self.active_sheet();
        self.user
            .rename_sheet(new_sheet, &pivot_name)
            .map_err(|e| anyhow!("{e}"))?;

        self.user.pause_evaluation();
        let _ = self.user.set_user_input(new_sheet, 1, 1, &g_title);
        let _ =
            self.user
                .set_user_input(new_sheet, 1, 2, &format!("{} of {}", agg.label(), v_title));

        let mut out_r = 2i32;
        for (k, acc) in &groups {
            let val = match agg {
                PivotAgg::Count => Some(acc.count as f64),
                PivotAgg::Sum => Some(acc.sum),
                PivotAgg::Avg => (acc.num_count > 0).then(|| acc.sum / acc.num_count as f64),
                PivotAgg::Min => (acc.num_count > 0).then_some(acc.min),
                PivotAgg::Max => (acc.num_count > 0).then_some(acc.max),
            };
            let _ = self.user.set_user_input(new_sheet, out_r, 1, k);
            if let Some(v) = val {
                let _ = self
                    .user
                    .set_user_input(new_sheet, out_r, 2, &v.to_string());
            }
            out_r += 1;
        }
        let last_data_r = out_r - 1;
        let total_fn = match agg {
            PivotAgg::Sum | PivotAgg::Count => "SUM",
            PivotAgg::Avg => "AVERAGE",
            PivotAgg::Min => "MIN",
            PivotAgg::Max => "MAX",
        };
        let _ = self.user.set_user_input(new_sheet, out_r, 1, "Grand Total");
        let _ = self.user.set_user_input(
            new_sheet,
            out_r,
            2,
            &format!("={total_fn}(B2:B{last_data_r})"),
        );
        self.user.resume_evaluation();
        self.user.evaluate();

        let hdr_area = Area {
            sheet: new_sheet,
            row: 1,
            column: 1,
            width: 2,
            height: 1,
        };
        let _ = self.user.update_range_style(&hdr_area, "font.b", "true");
        let tot_area = Area {
            sheet: new_sheet,
            row: out_r,
            column: 1,
            width: 2,
            height: 1,
        };
        let _ = self.user.update_range_style(&tot_area, "font.b", "true");
        let _ = self.autofit_column(new_sheet, 1);
        let _ = self.autofit_column(new_sheet, 2);
        self.select_cell(1, 1);
        self.invalidate_caches();
        self.dirty = true;
        Ok(pivot_name)
    }

    pub fn defined_names(&self) -> Vec<(String, Option<u32>, String)> {
        self.user.get_defined_name_list()
    }

    pub fn add_or_update_defined_name(&mut self, name: &str, expr: &str) -> Result<()> {
        let clean_name = name.trim();
        if clean_name.is_empty() {
            return Err(anyhow!("name cannot be empty"));
        }
        let mut formula = expr.trim().trim_start_matches('=').to_string();
        if !formula.contains('!') {
            let pfx = format_sheet_prefix(&self.active_sheet_name());
            formula = format!("{pfx}{formula}");
        }
        if let Some((_, scope, _)) = self
            .user
            .get_defined_name_list()
            .into_iter()
            .find(|(n, _, _)| n.eq_ignore_ascii_case(clean_name))
        {
            self.user
                .update_defined_name(clean_name, scope, clean_name, None, &formula)
                .map_err(|e| anyhow!("{e}"))?;
        } else {
            self.user
                .new_defined_name(clean_name, None, &formula)
                .map_err(|e| anyhow!("{e}"))?;
        }
        self.dirty = true;
        Ok(())
    }

    pub fn remove_defined_name(&mut self, name: &str, scope: Option<u32>) -> Result<()> {
        self.user
            .delete_defined_name(name, scope)
            .map_err(|e| anyhow!("{e}"))?;
        self.dirty = true;
        Ok(())
    }

    pub fn sheet_bounds(&self, sheet: u32) -> (i32, i32) {
        if let Some(&cached) = self.bounds_cache.borrow().get(&sheet) {
            return cached;
        }
        let mut max_r = 0;
        let mut max_c = 0;
        if let Ok(ws) = self.user.get_model().workbook.worksheet(sheet) {
            let rows = ws.sheet_data.rows();
            for &r in rows.iter().rev() {
                if ws
                    .sheet_data
                    .cells_in_row_between(r, 1, MAX_COLS)
                    .any(|(_, cell)| !matches!(cell.as_ref(), IcCell::EmptyCell { .. }))
                {
                    max_r = r;
                    break;
                }
            }
            if max_r > 0 {
                // Only inspect columns strictly right of `max_c` (`max_c + 1..=MAX_COLS`),
                // letting `Row::iter_from` skip narrower rows in O(log C) without building `Cow<Cell>`.
                for &r in &rows {
                    if r > max_r {
                        break;
                    }
                    for (c, cell) in ws.sheet_data.cells_in_row_between(r, max_c + 1, MAX_COLS) {
                        if !matches!(cell.as_ref(), IcCell::EmptyCell { .. }) {
                            max_c = max_c.max(c);
                        }
                    }
                }
            }
        }
        let bounds = (max_r, max_c);
        self.bounds_cache.borrow_mut().insert(sheet, bounds);
        bounds
    }

    pub fn find_matches(
        &self,
        query: &str,
        all_sheets: bool,
        case_sensitive: bool,
    ) -> Vec<(u32, i32, i32)> {
        if query.is_empty() {
            return Vec::new();
        }
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        let active = self.active_sheet();
        let target_sheets: Vec<u32> = self
            .visible_sheets()
            .into_iter()
            .map(|(i, _)| i)
            .filter(|&i| all_sheets || i == active)
            .collect();
        let mut hits = Vec::new();
        let wb = &self.user.get_model().workbook;
        for sheet in target_sheets {
            let Ok(ws) = wb.worksheet(sheet) else {
                continue;
            };
            for (row, col, cell) in ws.sheet_data.cells() {
                if matches!(cell.as_ref(), IcCell::EmptyCell { .. }) {
                    continue;
                }
                let disp = self.cell_display(sheet, row, col);
                let disp_match = if case_sensitive {
                    disp.contains(&needle)
                } else {
                    disp.to_lowercase().contains(&needle)
                };
                if disp_match {
                    hits.push((sheet, row, col));
                    continue;
                }
                if matches!(
                    cell.as_ref(),
                    IcCell::CellFormula { .. } | IcCell::ArrayFormula { .. }
                ) {
                    let raw = self.cell_input_content(sheet, row, col);
                    let raw_match = if case_sensitive {
                        raw.contains(&needle)
                    } else {
                        raw.to_lowercase().contains(&needle)
                    };
                    if raw_match {
                        hits.push((sheet, row, col));
                    }
                }
            }
        }
        hits.sort_unstable();
        hits
    }

    pub fn replace_matches(
        &mut self,
        query: &str,
        replacement: &str,
        all_sheets: bool,
        case_sensitive: bool,
    ) -> Result<usize> {
        let matches = self.find_matches(query, all_sheets, case_sensitive);
        if matches.is_empty() {
            return Ok(0);
        }
        let mut replaced = 0usize;
        self.user.pause_evaluation();
        for (sheet, r, c) in matches {
            let raw = self.cell_input_content(sheet, r, c);
            if raw.starts_with('=') {
                continue; // avoid corrupting formula syntax on literal text replace
            }
            let updated = if case_sensitive {
                raw.replace(query, replacement)
            } else {
                replace_case_insensitive(&raw, query, replacement)
            };
            if updated != raw {
                self.user
                    .set_user_input(sheet, r, c, &updated)
                    .map_err(|e| anyhow!("{e}"))?;
                replaced += 1;
            }
        }
        self.user.resume_evaluation();
        self.user.evaluate();
        if replaced > 0 {
            self.invalidate_caches();
            self.dirty = true;
        }
        Ok(replaced)
    }

    pub fn selection_stats(&self) -> SelectionStats {
        let sheet = self.active_sheet();
        let [r1, c1, mut r2, mut c2] = self.selected_range();
        let (mr, mc) = self.sheet_bounds(sheet);
        r2 = r2.min(mr);
        c2 = c2.min(mc);
        if r2 < r1 || c2 < c1 {
            return SelectionStats::default();
        }
        let Ok(ws) = self.user.get_model().workbook.worksheet(sheet) else {
            return SelectionStats::default();
        };

        let mut stats = SelectionStats {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            ..Default::default()
        };

        let record_cell = |r: i32, c: i32, cell: &IcCell, stats: &mut SelectionStats| match cell {
            IcCell::EmptyCell { .. } => {}
            IcCell::NumberCell { v, .. }
            | IcCell::CellFormula {
                v: FormulaValue::Number(v),
                ..
            } => {
                stats.non_empty += 1;
                if v.is_finite() {
                    stats.numeric_count += 1;
                    stats.sum += *v;
                    stats.min = stats.min.min(*v);
                    stats.max = stats.max.max(*v);
                }
            }
            IcCell::SharedString { .. }
            | IcCell::BooleanCell { .. }
            | IcCell::ErrorCell { .. }
            | IcCell::CellFormula { .. } => {
                stats.non_empty += 1;
            }
            IcCell::ArrayFormula { .. } | IcCell::SpillCell { .. } => {
                self.accum_stat(sheet, r, c, stats);
            }
        };

        if (r2 - r1) > 2_000 && (c2 - c1 + 1) * 2 >= mc.max(1) {
            for (r, c, cell) in ws.sheet_data.cells() {
                if r < r1 {
                    continue;
                }
                if r > r2 {
                    break;
                }
                if c >= c1 && c <= c2 {
                    record_cell(r, c, cell.as_ref(), &mut stats);
                }
            }
        } else {
            for r in r1..=r2 {
                if let Some(nums) = ws.sheet_data.numbers_in_row_between(r, c1, c2) {
                    for &v in nums {
                        if v.is_finite() {
                            stats.non_empty += 1;
                            stats.numeric_count += 1;
                            stats.sum += v;
                            stats.min = stats.min.min(v);
                            stats.max = stats.max.max(v);
                        }
                    }
                    continue;
                }
                for (c, cell) in ws.sheet_data.cells_in_row_between(r, c1, c2) {
                    record_cell(r, c, cell.as_ref(), &mut stats);
                }
            }
        }
        if stats.numeric_count == 0 {
            stats.min = 0.0;
            stats.max = 0.0;
        }
        stats
    }

    fn accum_stat(&self, sheet: u32, r: i32, c: i32, stats: &mut SelectionStats) {
        if self
            .user
            .get_model()
            .is_empty_cell(sheet, r, c)
            .unwrap_or(true)
        {
            return;
        }
        stats.non_empty += 1;
        if let Some(v) = self.cell_numeric(sheet, r, c) {
            stats.numeric_count += 1;
            stats.sum += v;
            stats.min = stats.min.min(v);
            stats.max = stats.max.max(v);
        }
    }

    pub fn create_chart_from_selection(&mut self, kind: ChartKind) -> usize {
        let sheet = self.active_sheet();
        let sheet_name = self.active_sheet_name();
        let [mut r1, mut c1, mut r2, mut c2] = self.selected_range();
        if r1 == r2 && c1 == c2 {
            let (mr, mc) = self.sheet_bounds(sheet);
            if mr >= 1 && mc >= 1 {
                r1 = 1;
                c1 = 1;
                r2 = mr;
                c2 = mc;
            } else {
                r2 = (r1 + 5).min(MAX_ROWS);
                c2 = (c1 + 1).min(MAX_COLS);
            }
        }
        let range = format_range_ref(None, r1, c1, r2, c2);
        let has_header = r2 > r1
            && (c1..=c2).any(|c| {
                self.cell_numeric(sheet, r1, c).is_none()
                    && !self.cell_display(sheet, r1, c).is_empty()
            });
        let id = self.next_chart_id;
        self.next_chart_id += 1;
        let title = format!("Chart {id} ({range})");
        self.charts.push(ChartSpec {
            id,
            title,
            kind,
            sheet_name,
            range,
            has_header,
            show_legend: true,
            show_grid: true,
        });
        self.dirty = true;
        self.charts.len() - 1
    }

    pub fn parse_spec_bounds(&self, spec: &ChartSpec) -> Option<(u32, [i32; 4])> {
        let (sheet_name, bounds) = parse_range_ref(&spec.sheet_name, &spec.range)?;
        let sheet = self.sheet_index_by_name(&sheet_name)?;
        Some((sheet, bounds))
    }
}

fn replace_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower_h = haystack.to_lowercase();
    let lower_n = needle.to_lowercase();
    if lower_h.len() != haystack.len() || lower_n.len() != needle.len() {
        return if lower_h == lower_n {
            replacement.to_string()
        } else {
            haystack.to_string()
        };
    }
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    for (idx, _) in lower_h.match_indices(&lower_n) {
        out.push_str(&haystack[last..idx]);
        out.push_str(replacement);
        last = idx + needle.len();
    }
    out.push_str(&haystack[last..]);
    out
}
