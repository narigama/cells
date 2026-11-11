use crate::actions::MenuEntry;
use crate::file_picker::FilePickerState;
use crate::workbook::{AuxSnapshot, ChartSpec, ColumnFilter, PivotAgg, SortSpec};
use ratatui::layout::Rect;

#[derive(Debug, Clone)]
pub struct EditState {
    pub buf: String,
    pub cursor: usize,
    pub sheet: u32,
    pub row: i32,
    pub col: i32,
    pub target_range: [i32; 4],
    pub formula_ref_span: Option<(usize, usize)>,
    pub completion_idx: usize,
    pub completion_scroll: usize,
    pub force_completion: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ActiveTopMenu {
    pub menu_idx: usize,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct ContextMenuState {
    pub x: u16,
    pub y: u16,
    pub entries: Vec<MenuEntry>,
    pub selected: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    RenameSheet,
    GotoCell,
    SetColWidth,
    InsertSparkline,
}

#[derive(Debug, Clone, Default)]
pub enum Modal {
    #[default]
    None,
    CommandPalette {
        query: String,
        selected: usize,
    },
    Prompt {
        kind: PromptKind,
        title: &'static str,
        value: String,
        cursor: usize,
    },
    FindReplace {
        find: String,
        replace: String,
        field: usize, // 0 = find, 1 = replace
        case_sensitive: bool,
        all_sheets: bool,
        matches: Vec<(u32, i32, i32)>,
        match_idx: usize,
    },
    ColorPicker {
        is_fill: bool,
        selected: usize,
        custom_hex: String,
        editing_hex: bool,
    },
    ChartEditor {
        idx: usize,
        draft: ChartSpec,
        field: usize, // 0 = title, 1 = kind, 2 = range, 3 = header, 4 = legend, 5 = grid
    },
    NamedRanges {
        selected: usize,
        name_input: String,
        ref_input: String,
        field: usize, // 0 = list, 1 = name, 2 = range
    },
    ColumnFilter {
        sheet: u32,
        col: i32,
        draft: ColumnFilter,
        num_buf: String,
        values: Vec<String>,
        selected_val: usize,
        field: usize, // 0 = substring, 1 = numeric rule, 2 = checklist
    },
    MultiSort {
        rules: Vec<SortSpec>,
        selected: usize,
    },
    Pivot {
        group_col: i32,
        val_col: i32,
        agg: PivotAgg,
        field: usize, // 0 = group_col, 1 = val_col, 2 = agg
    },
    Help {
        scroll: u16,
    },
    FilePicker(FilePickerState),
    ConfirmQuit {
        quit_app: bool,
    },
}

impl Modal {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum DragState {
    #[default]
    None,
    SelectingCells {
        anchor_sheet: u32,
        anchor_row: i32,
        anchor_col: i32,
        for_formula: bool,
    },
    SelectingCols {
        anchor_col: i32,
    },
    SelectingRows {
        anchor_row: i32,
    },
    ResizingCol {
        col: i32,
        start_x: u16,
        start_chars: u16,
    },
    FillHandle {
        source: [i32; 4],
        target_row: i32,
        target_col: i32,
    },
    MovingSelection {
        source: [i32; 4],
        row_offset: i32,
        col_offset: i32,
        target_r1: i32,
        target_c1: i32,
    },
    ScrollingVBar,
    ScrollingHBar,
    ResizingDock,
}

#[derive(Debug, Clone, Copy)]
pub struct ColHit {
    pub col: i32,
    pub x_start: u16,
    pub x_end: u16, // exclusive
    pub divider_x: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct RowHit {
    pub row: i32,
    pub y: u16,
}

#[derive(Debug, Clone)]
pub enum UndoEntry {
    ModelMarker,
    AuxOnly(AuxSnapshot),
}

#[derive(Debug, Clone, Default)]
pub struct FilePickerLayout {
    pub list_rect: Rect,
    pub row_rects: Vec<(usize, Rect)>,
    pub input_rect: Rect,
    pub input_text_x: u16,
    pub btn_up: Rect,
    pub btn_home: Rect,
    pub btn_hidden: Rect,
    pub btn_all: Rect,
    pub btn_confirm: Rect,
    pub btn_cancel: Rect,
}

#[derive(Debug, Clone, Default)]
pub struct LayoutCache {
    pub screen: Rect,
    pub menubar: Rect,
    pub menu_titles: Vec<(usize, Rect)>,
    pub dropdown_rect: Option<Rect>,
    pub dropdown_items: Vec<(usize, Rect)>,
    pub context_rect: Option<Rect>,
    pub context_items: Vec<(usize, Rect)>,
    pub modal_rect: Option<Rect>,
    pub formula_popup_rect: Option<Rect>,
    pub formula_assist_items: Vec<(usize, Rect)>,
    pub file_picker: Option<FilePickerLayout>,
    pub confirm_btn_save: Option<Rect>,
    pub confirm_btn_discard: Option<Rect>,
    pub confirm_btn_cancel: Option<Rect>,

    pub address_rect: Rect,
    pub formula_rect: Rect,

    pub workspace_rect: Rect,
    pub grid_rect: Rect,
    pub corner_rect: Rect,
    pub col_header_y: u16,
    pub row_header_x_end: u16,
    pub cols: Vec<ColHit>,
    pub rows: Vec<RowHit>,
    pub visible_scroll_cols: usize,
    pub visible_scroll_rows: usize,
    pub fill_handle: Option<(u16, u16)>,
    pub filter_buttons: Vec<(i32, Rect)>,
    pub v_scrollbar: Option<Rect>,
    pub h_scrollbar: Option<Rect>,
    pub selection_border_cells: Vec<(u16, u16)>,

    pub divider_rect: Option<Rect>,
    pub chart_dock_rect: Option<Rect>,
    pub chart_body_rect: Option<Rect>,
    pub chart_tab_rects: Vec<(usize, Rect)>,
    pub chart_btn_new: Option<Rect>,
    pub chart_btn_type: Option<Rect>,
    pub chart_btn_edit: Option<Rect>,
    pub chart_btn_png: Option<Rect>,
    pub chart_btn_svg: Option<Rect>,
    pub chart_btn_zoom: Option<Rect>,
    pub chart_btn_close: Option<Rect>,

    pub sheet_tabs: Vec<(u32, Rect)>,
    pub add_sheet_btn: Option<Rect>,
}
