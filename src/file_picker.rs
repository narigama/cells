use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostSaveAction {
    None,
    CloseWorkbook,
    QuitApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePickerPurpose {
    Open,
    SaveWorkbook { after: PostSaveAction },
    ExportCsv,
    ExportTsv,
    ExportChartPng,
    ExportChartSvg,
}

impl FilePickerPurpose {
    pub fn is_save(self) -> bool {
        !matches!(self, Self::Open)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Open => " Open Workbook / Dataset ",
            Self::SaveWorkbook {
                after: PostSaveAction::None,
            } => " Save Workbook As ",
            Self::SaveWorkbook {
                after: PostSaveAction::CloseWorkbook,
            } => " Save Workbook Before Closing ",
            Self::SaveWorkbook {
                after: PostSaveAction::QuitApp,
            } => " Save Workbook Before Quitting ",
            Self::ExportCsv => " Export Active Sheet as CSV ",
            Self::ExportTsv => " Export Active Sheet as TSV ",
            Self::ExportChartPng => " Export Active Chart as PNG ",
            Self::ExportChartSvg => " Export Active Chart as SVG ",
        }
    }

    pub fn action_label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::SaveWorkbook {
                after: PostSaveAction::None,
            } => "Save",
            Self::SaveWorkbook {
                after: PostSaveAction::CloseWorkbook,
            } => "Save & Close",
            Self::SaveWorkbook {
                after: PostSaveAction::QuitApp,
            } => "Save & Quit",
            Self::ExportCsv | Self::ExportTsv | Self::ExportChartPng | Self::ExportChartSvg => {
                "Export"
            }
        }
    }

    pub fn filter_badge_label(self, show_all: bool) -> &'static str {
        if show_all {
            "All Files"
        } else {
            match self {
                Self::Open => "Spreadsheets & Data",
                Self::SaveWorkbook { .. } => ".xlsx / .csv / .tsv",
                Self::ExportCsv => ".csv",
                Self::ExportTsv => ".tsv",
                Self::ExportChartPng => ".png",
                Self::ExportChartSvg => ".svg",
            }
        }
    }

    pub fn default_extension(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::SaveWorkbook { .. } => Some("xlsx"),
            Self::ExportCsv => Some("csv"),
            Self::ExportTsv => Some("tsv"),
            Self::ExportChartPng => Some("png"),
            Self::ExportChartSvg => Some("svg"),
        }
    }

    pub fn matches_extension(self, ext_lower: &str) -> bool {
        match self {
            Self::Open => matches!(
                ext_lower,
                "xlsx"
                    | "csv"
                    | "tsv"
                    | "tab"
                    | "json"
                    | "jsonl"
                    | "ndjson"
                    | "parquet"
                    | "pq"
                    | "sqlite"
                    | "sqlite3"
                    | "db"
            ),
            Self::SaveWorkbook { .. } => matches!(ext_lower, "xlsx" | "csv" | "tsv"),
            Self::ExportCsv => ext_lower == "csv",
            Self::ExportTsv => matches!(ext_lower, "tsv" | "tab"),
            Self::ExportChartPng => ext_lower == "png",
            Self::ExportChartSvg => ext_lower == "svg",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_parent: bool,
    pub size_bytes: u64,
    pub badge: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePickerOutcome {
    Pending,
    Confirmed(PathBuf),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct FilePickerState {
    pub purpose: FilePickerPurpose,
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
    pub scroll: usize,
    pub input: String,
    pub cursor: usize,
    pub focus_input: bool,
    pub show_hidden: bool,
    pub show_all_files: bool,
    pub error: Option<String>,
}

impl FilePickerState {
    pub fn new(
        purpose: FilePickerPurpose,
        start_dir: Option<PathBuf>,
        default_name: String,
    ) -> Self {
        let cwd = resolve_start_dir(start_dir);
        let focus_input = purpose.is_save();
        let input = if purpose.is_save() {
            default_name
        } else {
            String::new()
        };
        let cursor = input.len();
        let mut st = Self {
            purpose,
            cwd,
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            input: input.clone(),
            cursor,
            focus_input,
            show_hidden: false,
            show_all_files: false,
            error: None,
        };
        let highlight = if purpose.is_save() && !input.is_empty() {
            Some(input.as_str())
        } else {
            None
        };
        st.refresh_entries(highlight);
        st
    }

    pub fn refresh_entries(&mut self, select_name: Option<&str>) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        let filter_query = if !self.purpose.is_save()
            && !self.input.trim().is_empty()
            && !self.input.contains('/')
            && !self.input.starts_with('~')
        {
            Some(self.input.trim().to_ascii_lowercase())
        } else {
            None
        };

        match fs::read_dir(&self.cwd) {
            Ok(rd) => {
                for item in rd.flatten() {
                    let fname = item.file_name().to_string_lossy().into_owned();
                    if !self.show_hidden && fname.starts_with('.') {
                        continue;
                    }
                    let path = item.path();
                    let meta = fs::metadata(&path).or_else(|_| item.metadata());
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size_bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);

                    if !is_dir && !self.show_all_files {
                        let ext = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.to_ascii_lowercase())
                            .unwrap_or_default();
                        if !self.purpose.matches_extension(&ext) {
                            continue;
                        }
                    }

                    if let Some(ref q) = filter_query
                        && !fuzzy_match_lower(&fname.to_ascii_lowercase(), q)
                    {
                        continue;
                    }

                    let badge = if is_dir { "DIR" } else { badge_for_path(&path) };
                    let entry = FileEntry {
                        name: fname,
                        path,
                        is_dir,
                        is_parent: false,
                        size_bytes,
                        badge,
                    };
                    if is_dir {
                        dirs.push(entry);
                    } else {
                        files.push(entry);
                    }
                }
            }
            Err(e) => {
                self.error = Some(format!("Cannot read {}: {e}", self.cwd.display()));
            }
        }

        let cmp_entry = |a: &FileEntry, b: &FileEntry| -> Ordering {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        };
        dirs.sort_by(cmp_entry);
        files.sort_by(cmp_entry);

        let mut entries = Vec::with_capacity(1 + dirs.len() + files.len());
        if let Some(parent) = self.cwd.parent() {
            entries.push(FileEntry {
                name: "..".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                is_parent: true,
                size_bytes: 0,
                badge: "UP",
            });
        }
        entries.extend(dirs);
        entries.extend(files);
        self.entries = entries;

        if let Some(target) = select_name
            && let Some(idx) = self.entries.iter().position(|e| e.name == target)
        {
            self.selected = idx;
            return;
        }

        if self.entries.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else if filter_query.is_some() && self.entries.len() > 1 && self.entries[0].is_parent {
            self.selected = 1;
            self.scroll = 0;
        } else if self.selected == 0
            && !self.purpose.is_save()
            && self.entries.len() > 1
            && self.entries[0].is_parent
        {
            self.selected = 1;
        } else {
            self.selected = self.selected.min(self.entries.len() - 1);
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let max_idx = self.entries.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max_idx) as usize;
        self.error = None;
        if self.purpose.is_save() {
            self.focus_input = false;
        }
    }

    pub fn select_index_from_click(&mut self, idx: usize) {
        if idx >= self.entries.len() {
            return;
        }
        self.selected = idx;
        self.error = None;
        if self.purpose.is_save() {
            if !self.entries[idx].is_dir {
                self.input = self.entries[idx].name.clone();
                self.cursor = self.input.len();
            }
            self.focus_input = false;
        }
    }

    pub fn enter_dir(&mut self, dir: PathBuf, highlight_child: Option<&str>) {
        let canonical = fs::canonicalize(&dir).unwrap_or(dir);
        if let Err(e) = fs::read_dir(&canonical) {
            self.error = Some(format!("Cannot open folder {}: {e}", canonical.display()));
            return;
        }
        self.cwd = canonical;
        self.error = None;
        self.scroll = 0;
        self.selected = 0;
        if !self.purpose.is_save() {
            self.input.clear();
            self.cursor = 0;
        }
        self.refresh_entries(highlight_child);
    }

    pub fn up_dir(&mut self) {
        let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
            return;
        };
        let prev_name = self
            .cwd
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned);
        self.enter_dir(parent, prev_name.as_deref());
    }

    pub fn go_home(&mut self) {
        if let Some(home) = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
        {
            self.enter_dir(home, None);
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let cur_name = self.entries.get(self.selected).map(|e| e.name.clone());
        self.refresh_entries(cur_name.as_deref());
    }

    pub fn toggle_all_files(&mut self) {
        self.show_all_files = !self.show_all_files;
        let cur_name = self.entries.get(self.selected).map(|e| e.name.clone());
        self.refresh_entries(cur_name.as_deref());
    }

    pub fn activate_entry(&mut self, idx: usize) -> FilePickerOutcome {
        let Some(entry) = self.entries.get(idx).cloned() else {
            return FilePickerOutcome::Pending;
        };
        if entry.is_parent {
            self.up_dir();
            FilePickerOutcome::Pending
        } else if entry.is_dir {
            self.enter_dir(entry.path, None);
            FilePickerOutcome::Pending
        } else {
            FilePickerOutcome::Confirmed(entry.path)
        }
    }

    pub fn confirm(&mut self) -> FilePickerOutcome {
        self.error = None;
        let trimmed = self.input.trim().to_string();

        if !self.purpose.is_save() {
            if !trimmed.is_empty() && (trimmed.contains('/') || trimmed.starts_with('~')) {
                let resolved = resolve_against_cwd(&self.cwd, &trimmed);
                if resolved.is_dir() {
                    self.enter_dir(resolved, None);
                    return FilePickerOutcome::Pending;
                }
                if resolved.is_file() {
                    return FilePickerOutcome::Confirmed(resolved);
                }
                self.error = Some(format!("File or folder not found: {}", resolved.display()));
                return FilePickerOutcome::Pending;
            }
            if let Some(outcome) = self
                .entries
                .get(self.selected)
                .map(|_| self.selected)
                .map(|idx| self.activate_entry(idx))
            {
                return outcome;
            }
            if !trimmed.is_empty() {
                let cand = self.cwd.join(&trimmed);
                if cand.is_file() {
                    return FilePickerOutcome::Confirmed(cand);
                }
                self.error = Some(format!("No matching file: {trimmed}"));
            }
            return FilePickerOutcome::Pending;
        }

        // Save / Export mode:
        if !self.focus_input
            && let Some(entry) = self.entries.get(self.selected).cloned()
        {
            if entry.is_parent {
                self.up_dir();
                return FilePickerOutcome::Pending;
            }
            if entry.is_dir {
                self.enter_dir(entry.path, None);
                return FilePickerOutcome::Pending;
            }
            if self.input != entry.name {
                self.input = entry.name;
                self.cursor = self.input.len();
                self.focus_input = true;
                return FilePickerOutcome::Pending;
            }
        }

        if trimmed.is_empty() {
            self.error = Some("Enter a file name first".into());
            self.focus_input = true;
            return FilePickerOutcome::Pending;
        }

        let mut resolved = resolve_against_cwd(&self.cwd, &trimmed);
        if resolved.is_dir() || trimmed.ends_with('/') {
            if resolved.is_dir() {
                self.input.clear();
                self.cursor = 0;
                self.enter_dir(resolved, None);
            } else {
                self.error = Some(format!("Folder does not exist: {}", resolved.display()));
            }
            return FilePickerOutcome::Pending;
        }

        if resolved.extension().is_none()
            && let Some(ext) = self.purpose.default_extension()
        {
            resolved.set_extension(ext);
        }
        FilePickerOutcome::Confirmed(resolved)
    }

    pub fn confirm_from_button(&mut self) -> FilePickerOutcome {
        if self.purpose.is_save() {
            self.focus_input = true;
        }
        self.confirm()
    }

    pub fn display_cwd(&self, max_chars: usize) -> String {
        let raw = if let Some(home) = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            && let Ok(rel) = self.cwd.strip_prefix(&home)
        {
            if rel.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rel.display())
            }
        } else {
            self.cwd.display().to_string()
        };
        let chars: Vec<char> = raw.chars().collect();
        if chars.len() <= max_chars || max_chars < 4 {
            return raw;
        }
        let tail: String = chars[chars.len() - (max_chars - 1)..].iter().collect();
        format!("…{tail}")
    }
}

fn resolve_start_dir(start_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = start_dir {
        let candidate = if dir.is_dir() {
            Some(dir)
        } else {
            dir.parent().filter(|p| p.is_dir()).map(Path::to_path_buf)
        };
        if let Some(d) = candidate {
            return fs::canonicalize(&d).unwrap_or(d);
        }
    }
    std::env::current_dir()
        .map(|d| fs::canonicalize(&d).unwrap_or(d))
        .unwrap_or_else(|_| PathBuf::from("/"))
}

pub fn resolve_against_cwd(cwd: &Path, raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(rest);
    }
    if raw == "~"
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home);
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

fn badge_for_path(path: &Path) -> &'static str {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return "FILE";
    };
    match ext.to_ascii_lowercase().as_str() {
        "xlsx" | "xlsm" | "xls" => "XLSX",
        "csv" => "CSV",
        "tsv" | "tab" => "TSV",
        "json" => "JSON",
        "jsonl" | "ndjson" => "JSONL",
        "parquet" | "pq" => "PARQ",
        "sqlite" | "sqlite3" | "db" => "SQL",
        "png" => "PNG",
        "svg" => "SVG",
        "md" | "txt" => "TXT",
        _ => "FILE",
    }
}

pub fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if (bytes as f64) < MB {
        format!("{:.1} KB", bytes as f64 / KB)
    } else if (bytes as f64) < GB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{:.2} GB", bytes as f64 / GB)
    }
}

fn fuzzy_match_lower(hay_lower: &str, needle_lower: &str) -> bool {
    if hay_lower.contains(needle_lower) {
        return true;
    }
    let mut chars = hay_lower.chars();
    for qc in needle_lower.chars() {
        if !chars.any(|hc| hc == qc) {
            return false;
        }
    }
    true
}
