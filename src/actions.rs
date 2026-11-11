use crate::workbook::ChartKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // File
    NewWorkbook,
    OpenFilePrompt,
    Save,
    SaveAsPrompt,
    ExportCsvPrompt,
    ExportTsvPrompt,
    RecoverSnapshot,
    CloseWorkbook,
    Quit,

    // Edit
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    FillDown,
    FillRight,
    AutoSum,
    ClearContents,
    ClearAll,
    FindReplace,
    GotoCellPrompt,
    EditActiveCell,

    // View
    ToggleTheme,
    ToggleShowFormulas,
    ToggleDock,
    ToggleDockPosition,
    ToggleChartFullscreen,
    ToggleChartRenderer,
    FreezeAtCursor,
    FreezeTopRow,
    FreezeFirstCol,
    UnfreezePanes,

    // Insert / Structure
    InsertRowAbove,
    InsertRowBelow,
    DeleteSelectedRows,
    InsertColLeft,
    InsertColRight,
    DeleteSelectedCols,
    InsertCurrentDate,
    InsertCurrentTime,
    NamedRangesModal,
    InsertSparklinePrompt,
    NewSheet,
    DuplicateSheet,
    RenameSheetPrompt,
    DeleteSheet,
    NextSheet,
    PrevSheet,

    // Format
    ToggleBold,
    ToggleItalic,
    ToggleUnderline,
    ToggleStrikethrough,
    AlignLeft,
    AlignCenter,
    AlignRight,
    AlignAuto,
    NumFmtGeneral,
    NumFmtInteger,
    NumFmtDecimal,
    NumFmtCurrency,
    NumFmtPercent,
    NumFmtDate,
    PickTextColor,
    PickFillColor,
    AddDataBars,
    AddColorScale,
    ClearVisualRules,
    AutoFitCol,
    SetColWidthPrompt,

    // Data
    SortAsc,
    SortDesc,
    MultiSortModal,
    ToggleAutoFilter,
    FilterActiveColumn,
    PivotModal,

    // Chart
    NewChart(ChartKind),
    EditActiveChart,
    CycleChartKind,
    NextChart,
    PrevChart,
    ExportChartPngPrompt,
    ExportChartSvgPrompt,
    DeleteActiveChart,

    // Help
    ShowHelp,
}

#[derive(Debug, Clone, Copy)]
pub struct MenuEntry {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub action: Action,
    pub separator_after: bool,
}

impl MenuEntry {
    pub const fn item(label: &'static str, shortcut: &'static str, action: Action) -> Self {
        Self {
            label,
            shortcut,
            action,
            separator_after: false,
        }
    }

    pub const fn sep(label: &'static str, shortcut: &'static str, action: Action) -> Self {
        Self {
            label,
            shortcut,
            action,
            separator_after: true,
        }
    }
}

pub struct TopMenu {
    pub title: &'static str,
    pub alt_key: char,
    pub entries: &'static [MenuEntry],
}

pub const TOP_MENUS: &[TopMenu] = &[
    TopMenu {
        title: "File",
        alt_key: 'f',
        entries: &[
            MenuEntry::item("New Workbook", "Ctrl+N", Action::NewWorkbook),
            MenuEntry::sep(
                "Open File… (.xlsx/.csv/.json/.parquet/.sqlite)",
                "Ctrl+O",
                Action::OpenFilePrompt,
            ),
            MenuEntry::item("Save", "Ctrl+S", Action::Save),
            MenuEntry::sep("Save As…", "F12", Action::SaveAsPrompt),
            MenuEntry::item("Export Sheet as CSV…", "", Action::ExportCsvPrompt),
            MenuEntry::sep("Export Sheet as TSV…", "", Action::ExportTsvPrompt),
            MenuEntry::sep("Restore Recovery Snapshot", "", Action::RecoverSnapshot),
            MenuEntry::item("Close Workbook", "Ctrl+W", Action::CloseWorkbook),
            MenuEntry::item("Quit", "Ctrl+Q", Action::Quit),
        ],
    },
    TopMenu {
        title: "Edit",
        alt_key: 'e',
        entries: &[
            MenuEntry::item("Undo", "Ctrl+Z", Action::Undo),
            MenuEntry::sep("Redo", "Ctrl+Y", Action::Redo),
            MenuEntry::item("Cut", "Ctrl+X", Action::Cut),
            MenuEntry::item("Copy", "Ctrl+C", Action::Copy),
            MenuEntry::sep("Paste", "Ctrl+V", Action::Paste),
            MenuEntry::item("Fill Down", "Ctrl+D", Action::FillDown),
            MenuEntry::item("Fill Right", "Ctrl+R", Action::FillRight),
            MenuEntry::sep("AutoSum", "Alt+=", Action::AutoSum),
            MenuEntry::item("Clear Contents", "Del", Action::ClearContents),
            MenuEntry::sep("Clear All (+Formatting)", "", Action::ClearAll),
            MenuEntry::item("Find & Replace…", "Ctrl+F / Ctrl+H", Action::FindReplace),
            MenuEntry::item("Go to Cell…", "Ctrl+G / F5", Action::GotoCellPrompt),
        ],
    },
    TopMenu {
        title: "View",
        alt_key: 'v',
        entries: &[
            MenuEntry::item("Toggle Theme (Dark / Light)", "", Action::ToggleTheme),
            MenuEntry::sep(
                "Show Formulas in Grid",
                "Ctrl+`",
                Action::ToggleShowFormulas,
            ),
            MenuEntry::item("Toggle Chart Dock", "F9", Action::ToggleDock),
            MenuEntry::item(
                "Dock Position (Right / Bottom)",
                "",
                Action::ToggleDockPosition,
            ),
            MenuEntry::item(
                "Chart Fullscreen Toggle",
                "F11",
                Action::ToggleChartFullscreen,
            ),
            MenuEntry::sep(
                "Chart Renderer (Pixel / Braille)",
                "",
                Action::ToggleChartRenderer,
            ),
            MenuEntry::item("Freeze Up to Active Cell", "", Action::FreezeAtCursor),
            MenuEntry::item("Freeze Top Row", "", Action::FreezeTopRow),
            MenuEntry::item("Freeze First Column", "", Action::FreezeFirstCol),
            MenuEntry::item("Unfreeze Panes", "", Action::UnfreezePanes),
        ],
    },
    TopMenu {
        title: "Insert",
        alt_key: 'i',
        entries: &[
            MenuEntry::item("Insert Row Above", "Ctrl++", Action::InsertRowAbove),
            MenuEntry::item("Insert Row Below", "", Action::InsertRowBelow),
            MenuEntry::sep(
                "Delete Selected Row(s)",
                "Ctrl+-",
                Action::DeleteSelectedRows,
            ),
            MenuEntry::item("Insert Column Left", "", Action::InsertColLeft),
            MenuEntry::item("Insert Column Right", "", Action::InsertColRight),
            MenuEntry::sep("Delete Selected Column(s)", "", Action::DeleteSelectedCols),
            MenuEntry::item("Current Date (ISO)", "Ctrl+;", Action::InsertCurrentDate),
            MenuEntry::sep("Current Time", "Ctrl+Shift+;", Action::InsertCurrentTime),
            MenuEntry::item("Named Ranges…", "Ctrl+F3", Action::NamedRangesModal),
            MenuEntry::sep("In-Cell Sparkline…", "", Action::InsertSparklinePrompt),
            MenuEntry::item("New Sheet", "Shift+F11", Action::NewSheet),
            MenuEntry::item("Next Sheet", "Ctrl+PgDn", Action::NextSheet),
            MenuEntry::item("Previous Sheet", "Ctrl+PgUp", Action::PrevSheet),
            MenuEntry::item("Duplicate Sheet", "", Action::DuplicateSheet),
            MenuEntry::item("Rename Sheet…", "", Action::RenameSheetPrompt),
            MenuEntry::item("Delete Sheet", "", Action::DeleteSheet),
        ],
    },
    TopMenu {
        title: "Format",
        alt_key: 'o',
        entries: &[
            MenuEntry::item("Bold", "Ctrl+B / Ctrl+2", Action::ToggleBold),
            MenuEntry::item("Italic", "Ctrl+I / Ctrl+3", Action::ToggleItalic),
            MenuEntry::item("Underline", "Ctrl+U / Ctrl+4", Action::ToggleUnderline),
            MenuEntry::sep("Strikethrough", "Ctrl+5", Action::ToggleStrikethrough),
            MenuEntry::item("Align Left", "", Action::AlignLeft),
            MenuEntry::item("Align Center", "", Action::AlignCenter),
            MenuEntry::item("Align Right", "", Action::AlignRight),
            MenuEntry::sep("Align Auto", "", Action::AlignAuto),
            MenuEntry::item("Number: General", "Ctrl+Shift+~", Action::NumFmtGeneral),
            MenuEntry::item("Number: Integer (1,234)", "", Action::NumFmtInteger),
            MenuEntry::item(
                "Number: Decimal (1,234.56)",
                "Ctrl+Shift+!",
                Action::NumFmtDecimal,
            ),
            MenuEntry::item(
                "Number: Currency ($1,234.56)",
                "Ctrl+Shift+$",
                Action::NumFmtCurrency,
            ),
            MenuEntry::item(
                "Number: Percent (12.3%)",
                "Ctrl+Shift+%",
                Action::NumFmtPercent,
            ),
            MenuEntry::sep(
                "Number: ISO Date (YYYY-MM-DD)",
                "Ctrl+Shift+#",
                Action::NumFmtDate,
            ),
            MenuEntry::item("Text Color…", "", Action::PickTextColor),
            MenuEntry::sep("Fill Color…", "", Action::PickFillColor),
            MenuEntry::item("Conditional: Data Bars", "", Action::AddDataBars),
            MenuEntry::item(
                "Conditional: Color Scale (Heatmap)",
                "",
                Action::AddColorScale,
            ),
            MenuEntry::sep(
                "Clear Conditional Rules / Sparklines",
                "",
                Action::ClearVisualRules,
            ),
            MenuEntry::item("Auto-Fit Column Width", "", Action::AutoFitCol),
            MenuEntry::item("Set Column Width…", "", Action::SetColWidthPrompt),
        ],
    },
    TopMenu {
        title: "Data",
        alt_key: 'd',
        entries: &[
            MenuEntry::item("Sort Ascending (A → Z)", "", Action::SortAsc),
            MenuEntry::item("Sort Descending (Z → A)", "", Action::SortDesc),
            MenuEntry::sep("Multi-Column Sort…", "", Action::MultiSortModal),
            MenuEntry::item(
                "Toggle AutoFilter",
                "Ctrl+Shift+L",
                Action::ToggleAutoFilter,
            ),
            MenuEntry::sep("Filter Active Column…", "Alt+↓", Action::FilterActiveColumn),
            MenuEntry::item("Pivot / Group Summary…", "", Action::PivotModal),
        ],
    },
    TopMenu {
        title: "Chart",
        alt_key: 'c',
        entries: &[
            MenuEntry::item("New Line Chart", "", Action::NewChart(ChartKind::Line)),
            MenuEntry::item("New Bar Chart", "Alt+F1", Action::NewChart(ChartKind::Bar)),
            MenuEntry::item(
                "New Stacked Bar Chart",
                "",
                Action::NewChart(ChartKind::StackedBar),
            ),
            MenuEntry::item(
                "New Dual-Axis Chart",
                "",
                Action::NewChart(ChartKind::DualAxis),
            ),
            MenuEntry::item("New Scatter Plot", "", Action::NewChart(ChartKind::Scatter)),
            MenuEntry::item("New Area Chart", "", Action::NewChart(ChartKind::Area)),
            MenuEntry::item("New Histogram", "", Action::NewChart(ChartKind::Histogram)),
            MenuEntry::sep("New Pie Chart", "", Action::NewChart(ChartKind::Pie)),
            MenuEntry::item("Edit Active Chart…", "", Action::EditActiveChart),
            MenuEntry::sep("Cycle Chart Type", "", Action::CycleChartKind),
            MenuEntry::item("Export Chart as PNG…", "", Action::ExportChartPngPrompt),
            MenuEntry::sep("Export Chart as SVG…", "", Action::ExportChartSvgPrompt),
            MenuEntry::item("Delete Active Chart", "", Action::DeleteActiveChart),
        ],
    },
    TopMenu {
        title: "Help",
        alt_key: 'h',
        entries: &[MenuEntry::item(
            "Keyboard & Mouse Reference",
            "F1",
            Action::ShowHelp,
        )],
    },
];

pub fn all_palette_commands() -> Vec<(&'static str, &'static str, Action)> {
    let mut out = Vec::new();
    for menu in TOP_MENUS {
        for entry in menu.entries {
            out.push((entry.label, entry.shortcut, entry.action));
        }
    }
    out
}

pub fn cell_context_menu() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Edit Cell", "F2", Action::EditActiveCell),
        MenuEntry::item("Cut", "Ctrl+X", Action::Cut),
        MenuEntry::item("Copy", "Ctrl+C", Action::Copy),
        MenuEntry::sep("Paste", "Ctrl+V", Action::Paste),
        MenuEntry::item("Bold", "Ctrl+B", Action::ToggleBold),
        MenuEntry::item("Fill Color…", "", Action::PickFillColor),
        MenuEntry::item("Data Bars", "", Action::AddDataBars),
        MenuEntry::item("Color Scale (Heatmap)", "", Action::AddColorScale),
        MenuEntry::sep("In-Cell Sparkline…", "", Action::InsertSparklinePrompt),
        MenuEntry::item("Sort Ascending", "", Action::SortAsc),
        MenuEntry::item("Sort Descending", "", Action::SortDesc),
        MenuEntry::item("Toggle AutoFilter", "Ctrl+L", Action::ToggleAutoFilter),
        MenuEntry::sep("Pivot Summary…", "", Action::PivotModal),
        MenuEntry::item(
            "Chart Selection (Line)",
            "",
            Action::NewChart(ChartKind::Line),
        ),
        MenuEntry::sep(
            "Chart Selection (Bar)",
            "",
            Action::NewChart(ChartKind::Bar),
        ),
        MenuEntry::item("Insert Row Below", "", Action::InsertRowBelow),
        MenuEntry::item("Insert Column Right", "", Action::InsertColRight),
        MenuEntry::item("Clear Contents", "Del", Action::ClearContents),
    ]
}

pub fn col_header_context_menu() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Auto-Fit Column Width", "DblClick", Action::AutoFitCol),
        MenuEntry::sep("Set Column Width…", "", Action::SetColWidthPrompt),
        MenuEntry::item("Filter Column…", "", Action::FilterActiveColumn),
        MenuEntry::item("Toggle AutoFilter", "Ctrl+L", Action::ToggleAutoFilter),
        MenuEntry::item("Sort Ascending (A → Z)", "", Action::SortAsc),
        MenuEntry::item("Sort Descending (Z → A)", "", Action::SortDesc),
        MenuEntry::sep("Multi-Column Sort…", "", Action::MultiSortModal),
        MenuEntry::item("Insert Column Left", "", Action::InsertColLeft),
        MenuEntry::item("Insert Column Right", "", Action::InsertColRight),
        MenuEntry::sep("Delete Column(s)", "", Action::DeleteSelectedCols),
        MenuEntry::item("Freeze First Column", "", Action::FreezeFirstCol),
        MenuEntry::item("Unfreeze Panes", "", Action::UnfreezePanes),
    ]
}

pub fn row_header_context_menu() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Insert Row Above", "", Action::InsertRowAbove),
        MenuEntry::item("Insert Row Below", "", Action::InsertRowBelow),
        MenuEntry::sep("Delete Row(s)", "", Action::DeleteSelectedRows),
        MenuEntry::item("Freeze Top Row", "", Action::FreezeTopRow),
        MenuEntry::item("Unfreeze Panes", "", Action::UnfreezePanes),
    ]
}

pub fn sheet_tab_context_menu() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Rename Sheet…", "", Action::RenameSheetPrompt),
        MenuEntry::item("Duplicate Sheet", "", Action::DuplicateSheet),
        MenuEntry::sep("New Sheet", "", Action::NewSheet),
        MenuEntry::item("Delete Sheet", "", Action::DeleteSheet),
    ]
}

pub fn chart_context_menu() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Edit Chart…", "", Action::EditActiveChart),
        MenuEntry::item("Cycle Chart Type", "", Action::CycleChartKind),
        MenuEntry::sep("Toggle Fullscreen", "F11", Action::ToggleChartFullscreen),
        MenuEntry::item("Export as PNG…", "", Action::ExportChartPngPrompt),
        MenuEntry::sep("Export as SVG…", "", Action::ExportChartSvgPrompt),
        MenuEntry::item("Delete Chart", "", Action::DeleteActiveChart),
    ]
}
