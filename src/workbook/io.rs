use super::WorkbookState;
use super::types::{EmbeddedMeta, META_CHUNK_CHARS, META_SHEET_NAME};
use crate::address::{MAX_COLS, MAX_ROWS};
use crate::config::Config;
use crate::formats;
use crate::ooxml_chart::inject_ooxml_charts;
use anyhow::{Context, Result, anyhow};
use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::load_from_xlsx;
use ironcalc_base::formatter::dates::date_to_serial_number;
use ironcalc_base::types::Cell as IcCell;
use ironcalc_base::{Model, UserModel};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

impl WorkbookState {
    pub fn open_path(path: &Path) -> Result<(Self, Option<EmbeddedMeta>)> {
        if !path.exists() {
            let mut wb = Self::new_empty()?;
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                wb.user.set_name(stem);
            }
            wb.file_path = Some(path.to_path_buf());
            return Ok((wb, None));
        }

        match ext_lower(path).as_deref() {
            Some(ext @ ("csv" | "tsv" | "tab")) => {
                let delim = if ext == "csv" { b',' } else { b'\t' };
                let file = File::open(path)
                    .with_context(|| format!("failed to open {}", path.display()))?;
                let mut wb = Self::from_delimited_reader(
                    BufReader::new(file),
                    delim,
                    file_stem_or_default(path),
                )?;
                wb.file_path = Some(path.to_path_buf());
                Ok((wb, None))
            }
            Some("json" | "jsonl" | "ndjson") => {
                let raw = fs::read_to_string(path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                let model = formats::load_json_or_jsonl_str(&raw, file_stem_or_default(path))?;
                Ok((
                    Self::from_model_clean(model, Some(path.to_path_buf()))?,
                    None,
                ))
            }
            Some("parquet" | "pq") => {
                let model = formats::load_parquet_path(path, file_stem_or_default(path))?;
                Ok((
                    Self::from_model_clean(model, Some(path.to_path_buf()))?,
                    None,
                ))
            }
            Some("sqlite" | "sqlite3" | "db") => {
                let model = formats::load_sqlite_path(path, file_stem_or_default(path))?;
                Ok((
                    Self::from_model_clean(model, Some(path.to_path_buf()))?,
                    None,
                ))
            }
            _ => {
                let path_str = path
                    .to_str()
                    .ok_or_else(|| anyhow!("non-UTF-8 path: {}", path.display()))?;
                let model = load_from_xlsx(path_str, "en", "UTC", "en")
                    .map_err(|e| anyhow!("failed to load {}: {e:?}", path.display()))?;
                let mut tmp = UserModel::from_model(model);
                let meta = extract_and_strip_meta(&mut tmp);
                let mut user = if meta.is_some() {
                    // Rebuild from bytes so stripping `_cells_meta` leaves a clean undo history.
                    let clean_bytes = tmp.to_bytes();
                    UserModel::from_bytes(&clean_bytes, "en")
                        .map_err(|e| anyhow!("failed to hydrate workbook: {e}"))?
                } else {
                    tmp
                };
                user.evaluate();

                let mut wb = Self::with_user(user, Some(path.to_path_buf()));
                if let Some(ref m) = meta {
                    wb.charts = m.charts.clone();
                    wb.next_chart_id = wb.charts.iter().map(|c| c.id).max().unwrap_or(0) + 1;
                    wb.visual_rules = m.visual_rules.clone();
                    wb.autofilters = m.autofilters.clone();
                }
                if !wb.autofilters.is_empty() {
                    for (idx, _) in wb.visible_sheets() {
                        let _ = wb.reapply_autofilter(idx);
                    }
                    wb.dirty = false;
                }
                Ok((wb, meta))
            }
        }
    }

    pub fn from_stdin_str(raw: &str) -> Result<Self> {
        if formats::looks_like_json(raw) {
            let model = formats::load_json_or_jsonl_str(raw, "stdin")?;
            return Self::from_model_clean(model, None);
        }
        let first_line = raw.lines().next().unwrap_or("");
        let delim = if first_line.contains('\t') && !first_line.contains(',') {
            b'\t'
        } else {
            b','
        };
        Self::from_delimited_reader(raw.as_bytes(), delim, "stdin")
    }

    pub fn from_delimited_reader<R: Read>(reader: R, delimiter: u8, name: &str) -> Result<Self> {
        let mut seed_model = Model::new_empty("Untitled", "en", "UTC", "en")
            .map_err(|e| anyhow!("failed to create model: {e}"))?;
        let _ = seed_model.set_user_input(0, 1, 1, "2026-01-01".to_string());
        let iso_date_style = match seed_model.workbook.worksheets[0]
            .sheet_data
            .cell(1, 1)
            .as_deref()
        {
            Some(&IcCell::NumberCell { s, .. }) => s,
            _ => 0,
        };
        seed_model.workbook.worksheets[0].sheet_data.remove_row(1);
        seed_model.workbook.name = name.to_string();

        let mut workbook = seed_model.workbook;
        let mut shared_map: HashMap<String, i32> = workbook
            .shared_strings
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i as i32))
            .collect();

        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .buffer_capacity(128 * 1024)
            .delimiter(delimiter)
            .from_reader(reader);

        let mut record = csv::StringRecord::new();
        let mut row_cells: Vec<(i32, IcCell)> = Vec::new();
        let mut deferred: Vec<(i32, i32, String)> = Vec::new();
        let mut has_formula = false;
        let mut max_r = 0i32;
        let mut max_c = 0i32;
        let mut row = 0i32;

        while rdr
            .read_record(&mut record)
            .context("failed to parse delimited row")?
        {
            row += 1;
            if row > MAX_ROWS {
                break;
            }
            row_cells.clear();
            for (c_idx, field) in record.iter().enumerate() {
                let col = (c_idx as i32) + 1;
                if col > MAX_COLS {
                    break;
                }
                if field.is_empty() {
                    continue;
                }
                max_r = row;
                max_c = max_c.max(col);

                if let Some(v) = fast_plain_number(field) {
                    row_cells.push((col, IcCell::NumberCell { v, s: 0 }));
                } else if let Some(serial) = fast_iso_date_serial(field) {
                    row_cells.push((
                        col,
                        IcCell::NumberCell {
                            v: f64::from(serial),
                            s: iso_date_style,
                        },
                    ));
                } else if field.eq_ignore_ascii_case("true") {
                    row_cells.push((col, IcCell::BooleanCell { v: true, s: 0 }));
                } else if field.eq_ignore_ascii_case("false") {
                    row_cells.push((col, IcCell::BooleanCell { v: false, s: 0 }));
                } else if is_definitely_plain_text(field) {
                    let si = if let Some(&idx) = shared_map.get(field) {
                        idx
                    } else {
                        let idx = workbook.shared_strings.len() as i32;
                        let owned = field.to_string();
                        workbook.shared_strings.push(owned.clone());
                        shared_map.insert(owned, idx);
                        idx
                    };
                    row_cells.push((col, IcCell::SharedString { si, s: 0 }));
                } else {
                    if field.starts_with('=') {
                        has_formula = true;
                    }
                    deferred.push((row, col, field.to_string()));
                }
            }
            if !row_cells.is_empty() {
                workbook.worksheets[0]
                    .sheet_data
                    .set_row(row, row_cells.drain(..));
            }
        }

        let mut model = Model::from_workbook(workbook, "en")
            .map_err(|e| anyhow!("failed to finalize workbook: {e}"))?;
        for (r, c, val) in deferred {
            let _ = model.set_user_input(0, r, c, val);
        }
        if has_formula {
            model.evaluate();
        }
        let wb = Self::from_model_clean(model, None)?;
        wb.bounds_cache.borrow_mut().insert(0, (max_r, max_c));
        Ok(wb)
    }

    pub fn encode_xlsx_bytes(&self, cfg: &Config, dock_visible: bool) -> Result<Vec<u8>> {
        let meta = EmbeddedMeta {
            charts: self.charts.clone(),
            dock_visible,
            dock_position: cfg.dock_position,
            dock_percent: cfg.dock_percent,
            visual_rules: self.visual_rules.clone(),
            autofilters: self.autofilters.clone(),
        };
        let bytes = self.user.to_bytes();
        let mut tmp = UserModel::from_bytes(&bytes, "en")
            .map_err(|e| anyhow!("failed to snapshot workbook: {e}"))?;
        inject_meta(&mut tmp, &meta)?;
        let raw_zip = save_xlsx_to_writer(tmp.get_model(), Cursor::new(Vec::new()))
            .map_err(|e| anyhow!("failed to serialize xlsx: {e:?}"))?
            .into_inner();
        inject_ooxml_charts(raw_zip, tmp.get_model(), &self.charts)
    }

    pub fn save(&mut self, cfg: &Config, dock_visible: bool) -> Result<PathBuf> {
        let path = self
            .file_path
            .clone()
            .ok_or_else(|| anyhow!("no file path set — use Save As"))?;
        self.save_as(&path, cfg, dock_visible)?;
        Ok(path)
    }

    pub fn save_as(&mut self, path: &Path, cfg: &Config, dock_visible: bool) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let prev_path = self.file_path.clone();
        match ext_lower(path).as_deref() {
            Some("csv") => self.export_delimited(path, b',')?,
            Some("tsv" | "tab") => self.export_delimited(path, b'\t')?,
            _ => {
                let final_bytes = self.encode_xlsx_bytes(cfg, dock_visible)?;
                fs::write(path, final_bytes)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
        }
        clear_recovery_snapshot(prev_path.as_deref());
        clear_recovery_snapshot(Some(path));
        self.file_path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    pub fn export_delimited(&self, path: &Path, delimiter: u8) -> Result<()> {
        let sheet = self.active_sheet();
        let (max_row, max_col) = self.sheet_bounds(sheet);
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_path(path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        if max_row > 0 && max_col > 0 {
            for r in 1..=max_row {
                let mut rec = Vec::with_capacity(max_col as usize);
                for c in 1..=max_col {
                    rec.push(self.cell_display(sheet, r, c));
                }
                wtr.write_record(&rec)?;
            }
        }
        wtr.flush()?;
        Ok(())
    }

    pub fn write_recovery_snapshot(&self, cfg: &Config, dock_visible: bool) -> Result<PathBuf> {
        let rec_path = recovery_path_for(self.file_path.as_deref())
            .ok_or_else(|| anyhow!("no state directory available"))?;
        if let Some(parent) = rec_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = self.encode_xlsx_bytes(cfg, dock_visible)?;
        fs::write(&rec_path, bytes)?;
        Ok(rec_path)
    }

    /// Snapshots workbook bitcode on the caller thread, then runs XLSX ZIP serialization
    /// and disk write on a background thread so large workbooks never freeze the TUI.
    pub fn spawn_recovery_snapshot(
        &self,
        cfg: &Config,
        dock_visible: bool,
        in_flight: &Arc<AtomicBool>,
    ) -> bool {
        let Some(rec_path) = recovery_path_for(self.file_path.as_deref()) else {
            return false;
        };
        if in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let meta = EmbeddedMeta {
            charts: self.charts.clone(),
            dock_visible,
            dock_position: cfg.dock_position,
            dock_percent: cfg.dock_percent,
            visual_rules: self.visual_rules.clone(),
            autofilters: self.autofilters.clone(),
        };
        let charts = self.charts.clone();
        let bytes = self.user.to_bytes();
        let flag = Arc::clone(in_flight);
        std::thread::spawn(move || {
            struct ClearOnDrop(Arc<AtomicBool>);
            impl Drop for ClearOnDrop {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _guard = ClearOnDrop(flag);
            let Ok(mut tmp) = UserModel::from_bytes(&bytes, "en") else {
                return;
            };
            if inject_meta(&mut tmp, &meta).is_err() {
                return;
            }
            let Ok(writer) = save_xlsx_to_writer(tmp.get_model(), Cursor::new(Vec::new())) else {
                return;
            };
            let Ok(final_bytes) =
                inject_ooxml_charts(writer.into_inner(), tmp.get_model(), &charts)
            else {
                return;
            };
            if let Some(parent) = rec_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let tmp_path = rec_path.with_extension("xlsx.tmp");
            if fs::write(&tmp_path, final_bytes).is_ok() {
                let _ = fs::rename(&tmp_path, &rec_path);
            }
        });
        true
    }
}

fn extract_and_strip_meta(user: &mut UserModel<'_>) -> Option<EmbeddedMeta> {
    let idx = user
        .get_worksheets_properties()
        .iter()
        .position(|p| p.name == META_SHEET_NAME)? as u32;

    let mut json = String::new();
    for row in 1..=64 {
        let part = user.get_cell_content(idx, row, 1).unwrap_or_default();
        if part.is_empty() {
            break;
        }
        let clean = part
            .strip_prefix("CELLS:")
            .or_else(|| part.strip_prefix('\''))
            .unwrap_or(&part);
        json.push_str(clean);
    }

    if user.get_worksheets_properties().len() > 1 {
        let _ = user.unhide_sheet(idx);
        let _ = user.delete_sheet(idx);
    }
    serde_json::from_str(&json).ok()
}

fn inject_meta(user: &mut UserModel<'_>, meta: &EmbeddedMeta) -> Result<()> {
    if meta.charts.is_empty()
        && meta.visual_rules.is_empty()
        && meta.autofilters.is_empty()
        && !meta.dock_visible
    {
        return Ok(());
    }
    let json = serde_json::to_string(meta).context("failed to encode chart metadata")?;
    let existing = user
        .get_worksheets_properties()
        .iter()
        .position(|p| p.name == META_SHEET_NAME)
        .map(|i| i as u32);

    let idx = if let Some(i) = existing {
        i
    } else {
        let prev = user.get_selected_sheet();
        user.new_sheet().map_err(|e| anyhow!("{e}"))?;
        let new_idx = (user.get_worksheets_properties().len() - 1) as u32;
        user.rename_sheet(new_idx, META_SHEET_NAME)
            .map_err(|e| anyhow!("{e}"))?;
        let _ = user.set_selected_sheet(prev);
        new_idx
    };

    let chars: Vec<char> = json.chars().collect();
    for (i, chunk) in chars.chunks(META_CHUNK_CHARS).enumerate() {
        let mut s = String::with_capacity(chunk.len() + 6);
        s.push_str("CELLS:");
        s.extend(chunk);
        user.set_user_input(idx, (i as i32) + 1, 1, &s)
            .map_err(|e| anyhow!("{e}"))?;
    }
    user.hide_sheet(idx).map_err(|e| anyhow!("{e}"))?;
    Ok(())
}

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn file_stem_or_default(path: &Path) -> &str {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Sheet1")
}

pub fn recovery_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::cache_dir)
        .map(|d| d.join("cells").join("recovery"))
}

pub fn recovery_path_for(file_path: Option<&Path>) -> Option<PathBuf> {
    let dir = recovery_dir()?;
    let slug = match file_path {
        Some(p) => {
            let abs = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for &b in abs.to_string_lossy().as_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("workbook");
            format!("{stem}_{h:016x}.xlsx")
        }
        None => "untitled_recovery.xlsx".to_string(),
    };
    Some(dir.join(slug))
}

pub fn clear_recovery_snapshot(file_path: Option<&Path>) {
    if let Some(p) = recovery_path_for(file_path) {
        let _ = fs::remove_file(p);
    }
}

pub fn check_orphaned_recovery(file_path: Option<&Path>) -> Option<PathBuf> {
    let rec = recovery_path_for(file_path)?;
    if !rec.is_file() {
        return None;
    }
    if let Some(orig) = file_path
        && orig.is_file()
        && let (Ok(rm), Ok(om)) = (
            fs::metadata(&rec).and_then(|m| m.modified()),
            fs::metadata(orig).and_then(|m| m.modified()),
        )
        && rm <= om
    {
        return None;
    }
    Some(rec)
}

#[inline]
fn fast_plain_number(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 20 {
        return None;
    }
    let digits = if b[0] == b'-' { &b[1..] } else { b };
    if digits.is_empty() || digits[0] == b'.' || digits[digits.len() - 1] == b'.' {
        return None;
    }
    // Let zero-padded strings like "007" fall back to IronCalc's parser just in case.
    if digits.len() > 1 && digits[0] == b'0' && digits[1] != b'.' {
        return None;
    }
    let mut saw_dot = false;
    for &ch in digits {
        if ch == b'.' {
            if saw_dot {
                return None;
            }
            saw_dot = true;
        } else if !ch.is_ascii_digit() {
            return None;
        }
    }
    let v: f64 = s.parse().ok()?;
    v.is_finite().then_some(if v == 0.0 { 0.0 } else { v })
}

#[inline]
fn fast_iso_date_serial(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !b[0..4].iter().all(u8::is_ascii_digit)
        || !b[5..7].iter().all(u8::is_ascii_digit)
        || !b[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    if year < 1901 {
        return None;
    }
    date_to_serial_number(day, month, year).ok()
}

#[inline]
fn is_definitely_plain_text(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || s.len() != s.trim().len() {
        return false;
    }
    if matches!(b[0], b'=' | b'+' | b'-' | b'\'' | b'#') {
        return false;
    }
    if s.eq_ignore_ascii_case("nan")
        || s.eq_ignore_ascii_case("inf")
        || s.eq_ignore_ascii_case("infinity")
    {
        return false;
    }
    if !b.iter().any(u8::is_ascii_digit) {
        return true;
    }
    // IronCalc's `parse_number` rejects any ASCII letter other than scientific `e`/`E` and any
    // internal space, and `parse_date` only matches 3-part `/`, `-`, or `.` tokens without spaces.
    let has_non_exp_alpha = b
        .iter()
        .any(|&c| c.is_ascii_alphabetic() && c != b'e' && c != b'E');
    let has_space = b.contains(&b' ');
    if !has_non_exp_alpha && !has_space {
        return false;
    }
    if has_space {
        if s.starts_with('$')
            || s.ends_with('$')
            || s.starts_with('€')
            || s.ends_with('€')
            || s.ends_with('%')
        {
            return false;
        }
        return has_non_exp_alpha;
    }
    let count_sep = |sep: u8| b.iter().filter(|&&c| c == sep).count();
    count_sep(b'/') != 2 && count_sep(b'-') != 2 && count_sep(b'.') != 2
}
