use super::file_picker::{draw_file_picker_modal, visible_input_tail};
use super::{fill_rect, filter_palette_commands, modal_block};
use crate::address::col_to_letters;
use crate::app::{App, Modal};
use crate::config::{Palette, SWATCH_COLORS, parse_hex_color};
use crate::workbook::{ChartSpec, ColumnFilter, NumericFilterOp, PivotAgg, SortSpec};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

pub(super) fn compute_modal_rect(modal: &Modal, screen: Rect) -> Option<Rect> {
    let (w, h) = match modal {
        Modal::None => return None,
        Modal::CommandPalette { .. } => (screen.width.min(68), screen.height.min(18)),
        Modal::Prompt { .. } => (screen.width.min(64), 6),
        Modal::FindReplace { .. } => (screen.width.min(64), 10),
        Modal::ColorPicker { .. } => (
            screen.width.min(52),
            (SWATCH_COLORS.len() as u16 + 6).min(screen.height),
        ),
        Modal::ChartEditor { .. } => (screen.width.min(62), 12),
        Modal::NamedRanges { .. } => (screen.width.min(64), screen.height.min(16)),
        Modal::ColumnFilter { .. } => (screen.width.min(62), screen.height.min(18)),
        Modal::MultiSort { .. } => (screen.width.min(58), 12),
        Modal::Pivot { .. } => (screen.width.min(56), 10),
        Modal::Help { .. } => (screen.width.min(78), screen.height.min(26)),
        Modal::FilePicker(_) => (
            screen
                .width
                .saturating_sub(6)
                .clamp(44, 82)
                .min(screen.width),
            screen
                .height
                .saturating_sub(4)
                .clamp(12, 26)
                .min(screen.height),
        ),
        Modal::ConfirmQuit { .. } => (screen.width.min(54), 7),
    };
    let x = screen.x + screen.width.saturating_sub(w) / 2;
    let y = screen.y + screen.height.saturating_sub(h) / 2;
    Some(Rect::new(x, y, w, h))
}

pub(super) fn draw_modal(f: &mut Frame<'_>, app: &mut App, rect: Rect) {
    let p = app.palette;
    f.render_widget(Clear, rect);

    let names_snapshot = if matches!(app.modal, Modal::NamedRanges { .. }) {
        app.wb.defined_names()
    } else {
        Vec::new()
    };

    match &mut app.modal {
        Modal::None => {}
        Modal::CommandPalette { query, selected } => {
            draw_palette_modal(f, &p, rect, query, *selected)
        }
        Modal::Prompt { title, value, .. } => draw_prompt_modal(f, &p, rect, title, value),
        Modal::FindReplace {
            find,
            replace,
            field,
            case_sensitive,
            all_sheets,
            matches,
            match_idx,
        } => draw_find_replace_modal(
            f,
            &p,
            rect,
            find,
            replace,
            *field,
            *case_sensitive,
            *all_sheets,
            matches.len(),
            *match_idx,
        ),
        Modal::ColorPicker {
            is_fill,
            selected,
            custom_hex,
            editing_hex,
        } => draw_color_picker_modal(f, &p, rect, *is_fill, *selected, custom_hex, *editing_hex),
        Modal::ChartEditor { draft, field, .. } => {
            draw_chart_editor_modal(f, &p, rect, draft, *field)
        }
        Modal::NamedRanges {
            selected,
            name_input,
            ref_input,
            field,
        } => draw_named_ranges_modal(
            f,
            &p,
            rect,
            &names_snapshot,
            *selected,
            name_input,
            ref_input,
            *field,
        ),
        Modal::ColumnFilter {
            col,
            draft,
            num_buf,
            values,
            selected_val,
            field,
            ..
        } => draw_column_filter_modal(
            f,
            &p,
            rect,
            *col,
            draft,
            num_buf,
            values,
            *selected_val,
            *field,
        ),
        Modal::MultiSort { rules, selected } => {
            draw_multi_sort_modal(f, &p, rect, rules, *selected)
        }
        Modal::Pivot {
            group_col,
            val_col,
            agg,
            field,
        } => draw_pivot_modal(f, &p, rect, *group_col, *val_col, *agg, *field),
        Modal::Help { scroll } => draw_help_modal(f, &p, rect, *scroll),
        Modal::FilePicker(state) => {
            app.layout.file_picker = Some(draw_file_picker_modal(f, &p, state, rect));
        }
        Modal::ConfirmQuit { quit_app } => {
            let q = *quit_app;
            draw_confirm_quit_modal(f, app, &p, rect, q);
        }
    }
}

fn draw_palette_modal(f: &mut Frame<'_>, p: &Palette, rect: Rect, query: &str, selected: usize) {
    let block = modal_block(" Command Palette ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height == 0 {
        return;
    }
    f.buffer_mut().set_string(
        inner.x + 1,
        inner.y,
        format!("> {query}"),
        Style::default().bg(p.surface).fg(p.text),
    );

    let filtered = filter_palette_commands(query);
    let list_y = inner.y + 2;
    let cap = inner.height.saturating_sub(2) as usize;
    let sel = selected.min(filtered.len().saturating_sub(1));
    let offset = sel.saturating_sub(cap.saturating_sub(1));

    for (row_i, &(lbl, sc, _)) in filtered.iter().skip(offset).take(cap).enumerate() {
        let idx = offset + row_i;
        let y = list_y + row_i as u16;
        let row_r = Rect::new(inner.x, y, inner.width, 1);
        let st = if idx == sel {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.surface).fg(p.text)
        };
        fill_rect(f.buffer_mut(), row_r, st);
        f.buffer_mut().set_string(inner.x + 1, y, lbl, st);
        if !sc.is_empty() && inner.width > sc.len() as u16 + 3 {
            f.buffer_mut().set_string(
                inner.right() - sc.len() as u16 - 1,
                y,
                sc,
                if idx == sel {
                    st
                } else {
                    Style::default().bg(p.surface).fg(p.muted)
                },
            );
        }
    }
}

fn draw_prompt_modal(f: &mut Frame<'_>, p: &Palette, rect: Rect, title: &str, value: &str) {
    let title_str = format!(" {title} ");
    let block = modal_block(&title_str, p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height >= 3 {
        let input_r = Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1);
        fill_rect(
            f.buffer_mut(),
            input_r,
            Style::default().bg(p.grid_bg).fg(p.text),
        );
        f.buffer_mut().set_string(
            input_r.x,
            input_r.y,
            value,
            Style::default().bg(p.grid_bg).fg(p.text),
        );
        f.buffer_mut().set_string(
            inner.x + 1,
            inner.bottom().saturating_sub(1),
            "Enter: Confirm   Esc: Cancel",
            Style::default().bg(p.surface).fg(p.muted),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_find_replace_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    find: &str,
    replace: &str,
    field: usize,
    case_sensitive: bool,
    all_sheets: bool,
    match_count: usize,
    match_idx: usize,
) {
    let block = modal_block(" Find & Replace ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let field_style = |idx: usize| {
        if field == idx {
            Style::default()
                .bg(p.grid_bg)
                .fg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.grid_bg).fg(p.text)
        }
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(" Find:    ", Style::default().fg(p.muted)),
            Span::styled(format!("{find:<38}"), field_style(0)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Replace: ", Style::default().fg(p.muted)),
            Span::styled(format!("{replace:<38}"), field_style(1)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                " [Alt+C] Case: {}   [Alt+A] All Sheets: {}   Matches: {}",
                if case_sensitive { "ON" } else { "off" },
                if all_sheets { "ON" } else { "off" },
                if match_count == 0 {
                    "0".to_string()
                } else {
                    format!("{}/{match_count}", match_idx + 1)
                }
            ),
            Style::default().fg(p.text),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " Tab: Switch Field · Enter: Next · Ctrl+R: Replace All · Esc: Close",
            Style::default().fg(p.muted),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(p.surface)),
        inner,
    );
}

fn draw_color_picker_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    is_fill: bool,
    selected: usize,
    custom_hex: &str,
    editing_hex: bool,
) {
    let title = if is_fill {
        " Cell Fill Color "
    } else {
        " Cell Text Color "
    };
    let block = modal_block(title, p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines = Vec::new();
    for (i, (name, hex)) in SWATCH_COLORS.iter().enumerate() {
        let is_sel = !editing_hex && selected == i;
        let swatch = parse_hex_color(hex).unwrap_or(p.grid_bg);
        let row_st = if is_sel {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.surface).fg(p.text)
        };
        lines.push(Line::from(vec![
            Span::styled(" ██ ", Style::default().fg(swatch).bg(p.surface)),
            Span::styled(format!(" {name:<18} {hex:<8}"), row_st),
        ]));
    }
    lines.push(Line::from(""));
    let hex_st = if editing_hex {
        Style::default()
            .bg(p.grid_bg)
            .fg(p.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(p.grid_bg).fg(p.text)
    };
    lines.push(Line::from(vec![
        Span::styled(" Custom (#RRGGBB): ", Style::default().fg(p.muted)),
        Span::styled(format!(" {custom_hex:<10} "), hex_st),
    ]));
    lines.push(Line::from(Span::styled(
        " ↑/↓ Select · Tab Custom Hex · Enter Apply · Esc Cancel",
        Style::default().fg(p.muted),
    )));
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(p.surface)),
        inner,
    );
}

fn draw_chart_editor_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    draft: &ChartSpec,
    field: usize,
) {
    let block = modal_block(" Edit Chart ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let hi = |idx: usize| {
        if field == idx {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.surface).fg(p.text)
        }
    };
    let check = |on: bool| format!(" [{}] ", if on { "x" } else { " " });
    let rows = vec![
        Line::from(vec![
            Span::styled(" Title:       ", Style::default().fg(p.muted)),
            Span::styled(format!(" {:<36} ", draft.title), hi(0)),
        ]),
        Line::from(vec![
            Span::styled(" Chart Type:  ", Style::default().fg(p.muted)),
            Span::styled(format!(" ◀ {:^14} ▶ ", draft.kind.label()), hi(1)),
        ]),
        Line::from(vec![
            Span::styled(" Data Range:  ", Style::default().fg(p.muted)),
            Span::styled(format!(" {:<36} ", draft.range), hi(2)),
        ]),
        Line::from(vec![
            Span::styled(" Header Row:  ", Style::default().fg(p.muted)),
            Span::styled(check(draft.has_header), hi(3)),
        ]),
        Line::from(vec![
            Span::styled(" Show Legend: ", Style::default().fg(p.muted)),
            Span::styled(check(draft.show_legend), hi(4)),
        ]),
        Line::from(vec![
            Span::styled(" Show Grid:   ", Style::default().fg(p.muted)),
            Span::styled(check(draft.show_grid), hi(5)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " ↑/↓/Tab Field · Space/←/→ Toggle · Enter Save · Esc Cancel",
            Style::default().fg(p.muted),
        )),
    ];
    f.render_widget(
        Paragraph::new(rows).style(Style::default().bg(p.surface)),
        inner,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_named_ranges_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    names: &[(String, Option<u32>, String)],
    selected: usize,
    name_input: &str,
    ref_input: &str,
    field: usize,
) {
    let block = modal_block(" Named Ranges (Ctrl+F3) ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let hi = |f_idx: usize| {
        if field == f_idx {
            Style::default()
                .bg(p.grid_bg)
                .fg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.grid_bg).fg(p.text)
        }
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Name:  ", Style::default().fg(p.muted)),
            Span::styled(format!("{name_input:<44}"), hi(1)),
        ]),
        Line::from(vec![
            Span::styled(" Range: ", Style::default().fg(p.muted)),
            Span::styled(format!("{ref_input:<44}"), hi(2)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Defined Names:",
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        )),
    ];
    if names.is_empty() {
        lines.push(Line::from(Span::styled(
            "   (none yet — type a name above and press Enter)",
            Style::default().fg(p.muted),
        )));
    } else {
        let cap = inner.height.saturating_sub(6) as usize;
        let sel = selected.min(names.len().saturating_sub(1));
        for (i, (n, _, expr)) in names.iter().take(cap.max(1)).enumerate() {
            let is_sel = field == 0 && i == sel;
            let st = if is_sel {
                Style::default()
                    .bg(p.menu_hi_bg)
                    .fg(p.menu_hi_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(p.surface).fg(p.text)
            };
            lines.push(Line::from(Span::styled(format!("   {n:<18} → {expr}"), st)));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Tab: Field/List · Enter: Save / Jump · Del: Remove · Esc: Close",
        Style::default().fg(p.muted),
    )));
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(p.surface)),
        inner,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_column_filter_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    col: i32,
    draft: &ColumnFilter,
    num_buf: &str,
    values: &[String],
    selected_val: usize,
    field: usize,
) {
    let title = format!(" AutoFilter — Column {} ", col_to_letters(col));
    let block = modal_block(&title, p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let hi = |f_idx: usize| {
        if field == f_idx {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.grid_bg).fg(p.text)
        }
    };
    let num_disp = match draft.numeric_op {
        NumericFilterOp::None | NumericFilterOp::Top10 => draft.numeric_op.label().to_string(),
        _ => format!("{}  [{num_buf}]", draft.numeric_op.label()),
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Text Contains: ", Style::default().fg(p.muted)),
            Span::styled(format!(" {:<36} ", draft.substring), hi(0)),
        ]),
        Line::from(vec![
            Span::styled(" Numeric Rule:  ", Style::default().fg(p.muted)),
            Span::styled(format!(" ◀ {num_disp:<32} ▶ "), hi(1)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Distinct Values (Space toggle · Ctrl+A all):",
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        )),
    ];
    let cap = inner.height.saturating_sub(6) as usize;
    let sel = selected_val.min(values.len().saturating_sub(1));
    let offset = sel.saturating_sub(cap.saturating_sub(1));
    for (row_i, v) in values.iter().skip(offset).take(cap.max(1)).enumerate() {
        let idx = offset + row_i;
        let checked = !draft.excluded_values.contains(v);
        let is_sel = field == 2 && idx == sel;
        let st = if is_sel {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.surface).fg(p.text)
        };
        lines.push(Line::from(Span::styled(
            format!("   [{}] {v}", if checked { "x" } else { " " }),
            st,
        )));
    }
    lines.push(Line::from(Span::styled(
        " Tab: Section · ←/→: Rule · Ctrl+X: Clear · Enter: Apply · Esc",
        Style::default().fg(p.muted),
    )));
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(p.surface)),
        inner,
    );
}

fn draw_multi_sort_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    rules: &[SortSpec],
    selected: usize,
) {
    let block = modal_block(" Multi-Column Sort ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines = vec![Line::from(Span::styled(
        " Sort Priority Levels (header row preserved automatically):",
        Style::default().fg(p.muted),
    ))];
    for (i, sp) in rules.iter().enumerate() {
        let st = if i == selected {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.surface).fg(p.text)
        };
        let ord = if sp.ascending {
            "Ascending (A → Z)"
        } else {
            "Descending (Z → A)"
        };
        lines.push(Line::from(Span::styled(
            format!(
                "  {}. Column {:<4}  │  {ord}",
                i + 1,
                col_to_letters(sp.col)
            ),
            st,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑/↓ Select · ←/→ Col · Space Order · + Add · - Del · Enter Sort",
        Style::default().fg(p.muted),
    )));
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(p.surface)),
        inner,
    );
}

fn draw_pivot_modal(
    f: &mut Frame<'_>,
    p: &Palette,
    rect: Rect,
    group_col: i32,
    val_col: i32,
    agg: PivotAgg,
    field: usize,
) {
    let block = modal_block(" Pivot / Group Summary (New Sheet) ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let hi = |idx: usize| {
        if field == idx {
            Style::default()
                .bg(p.menu_hi_bg)
                .fg(p.menu_hi_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(p.surface).fg(p.text)
        }
    };
    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(" Group By Column:  ", Style::default().fg(p.muted)),
            Span::styled(format!(" ◀ {:^8} ▶ ", col_to_letters(group_col)), hi(0)),
        ]),
        Line::from(vec![
            Span::styled(" Value Column:     ", Style::default().fg(p.muted)),
            Span::styled(format!(" ◀ {:^8} ▶ ", col_to_letters(val_col)), hi(1)),
        ]),
        Line::from(vec![
            Span::styled(" Aggregation:      ", Style::default().fg(p.muted)),
            Span::styled(format!(" ◀ {:^8} ▶ ", agg.label()), hi(2)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " ↑/↓/Tab Field · ←/→/Space Cycle · Enter Build Sheet · Esc Cancel",
            Style::default().fg(p.muted),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(p.surface)),
        inner,
    );
}

fn draw_help_modal(f: &mut Frame<'_>, p: &Palette, rect: Rect, scroll: u16) {
    let block = modal_block(" cells — Keyboard & Mouse Reference ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let help_lines = vec![
        Line::from(Span::styled(
            "Microsoft Excel Shortcuts",
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from("  • Ctrl+Arrows / Ctrl+Shift+Arrows  Jump / extend selection to data edge"),
        Line::from("  • Ctrl+Home / Ctrl+End (+Shift)    Jump / extend to A1 or last used cell"),
        Line::from("  • Ctrl+Space / Shift+Space         Select full column(s) / full row(s)"),
        Line::from("  • Ctrl+A (twice for whole sheet)   Select data region → entire sheet"),
        Line::from(
            "  • Ctrl+PgDn / Ctrl+PgUp            Next / Previous worksheet · Shift+F11 New",
        ),
        Line::from("  • Alt+PgDn / Alt+PgUp              Scroll page right / left"),
        Line::from("  • F2 Edit · F4 ($A$1 cycle)        Ctrl+Enter Fill across selection"),
        Line::from("  • Ctrl+D / Ctrl+R                  Fill Down / Fill Right"),
        Line::from("  • Alt+= AutoSum · Ctrl+`           Toggle Show Formulas in grid"),
        Line::from("  • Ctrl+; / Ctrl+Shift+;            Insert Date (YYYY-MM-DD) / Time (HH:MM)"),
        Line::from("  • Ctrl+' / Ctrl+\"                  Copy formula / value from cell above"),
        Line::from(
            "  • Ctrl+1 Format · Ctrl+2/3/4/5     Bold / Italic / Underline / Strikethrough",
        ),
        Line::from("  • Ctrl+Shift+ ~ ! $ % #            General / Decimal / Currency / % / Date"),
        Line::from("  • Ctrl++ / Ctrl+-                  Insert / Delete row(s) or column(s)"),
        Line::from("  • Ctrl+Shift+L / Alt+↓             Toggle AutoFilter / Column Filter modal"),
        Line::from("  • Ctrl+F / Ctrl+H · F5 / Ctrl+G    Find / Replace · Go To · Ctrl+F3 Names"),
        Line::from("  • Alt+F1 / F11 · F12               Insert Bar Chart / Fullscreen · Save As"),
        Line::from("  • Ctrl+O / F12 / Ctrl+W            Virtual File Picker · Close Workbook"),
        Line::from("  • = / Tab / Ctrl+Space             Cell-anchored Formula IntelliSense"),
        Line::from(""),
        Line::from(Span::styled(
            "Mouse & Cross Cursor",
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from("  • Classic spreadsheet cross cursor (`cell` / Pixel RGBA sprite) over grid"),
        Line::from("  • Click / Drag cells: Select range (or pick live range while typing `=`)"),
        Line::from("  • Drag `┃` selection border: Move range · Drag `▪` handle: Auto-fill"),
        Line::from("  • Click `▾` header icon: Column AutoFilter · Hover chart: Point readout"),
        Line::from("  • Right-click / Shift+F10: Context menu · Ctrl+P: Command Palette"),
    ];
    f.render_widget(
        Paragraph::new(help_lines)
            .scroll((scroll, 0))
            .style(Style::default().bg(p.surface).fg(p.text)),
        inner,
    );
}

fn draw_confirm_quit_modal(
    f: &mut Frame<'_>,
    app: &mut App,
    p: &Palette,
    rect: Rect,
    quit_app: bool,
) {
    let block = modal_block(" Unsaved Changes ", p);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    fill_rect(
        f.buffer_mut(),
        inner,
        Style::default().bg(p.surface).fg(p.text),
    );
    if inner.height < 4 {
        return;
    }
    f.buffer_mut().set_string(
        inner.x + 2,
        inner.y + 1,
        "Current workbook has unsaved changes.",
        Style::default().bg(p.surface).fg(p.text),
    );
    let by = inner.y + 3;
    let save_lbl = if quit_app {
        " [S] Save & Quit "
    } else {
        " [S] Save & Close "
    };
    let disc_lbl = " [D] Discard ";
    let canc_lbl = " [Esc] Cancel ";
    let mut bx = inner.x + 2;
    let sw = save_lbl.chars().count() as u16;
    let dw = disc_lbl.chars().count() as u16;
    let cw = canc_lbl.chars().count() as u16;
    if bx + sw <= inner.right() {
        app.layout.confirm_btn_save = Some(Rect::new(bx, by, sw, 1));
        f.buffer_mut().set_string(
            bx,
            by,
            save_lbl,
            Style::default()
                .bg(p.accent)
                .fg(Color::Rgb(10, 14, 20))
                .add_modifier(Modifier::BOLD),
        );
        bx += sw + 2;
    }
    if bx + dw <= inner.right() {
        app.layout.confirm_btn_discard = Some(Rect::new(bx, by, dw, 1));
        f.buffer_mut().set_string(
            bx,
            by,
            disc_lbl,
            Style::default()
                .bg(p.header_bg)
                .fg(p.error_fg)
                .add_modifier(Modifier::BOLD),
        );
        bx += dw + 2;
    }
    if bx + cw <= inner.right() {
        app.layout.confirm_btn_cancel = Some(Rect::new(bx, by, cw, 1));
        f.buffer_mut().set_string(
            bx,
            by,
            canc_lbl,
            Style::default().bg(p.header_bg).fg(p.text),
        );
    }
}

pub(super) fn set_modal_cursor(f: &mut Frame<'_>, app: &App, rect: Rect) {
    match &app.modal {
        Modal::CommandPalette { query, .. } => {
            let cx =
                (rect.x + 4 + query.chars().count() as u16).min(rect.right().saturating_sub(2));
            f.set_cursor_position((cx, rect.y + 1));
        }
        Modal::Prompt { value, cursor, .. } => {
            let chars_before = value[..(*cursor).min(value.len())].chars().count() as u16;
            let cx = (rect.x + 2 + chars_before).min(rect.right().saturating_sub(2));
            f.set_cursor_position((cx, rect.y + 2));
        }
        Modal::FindReplace {
            find,
            replace,
            field,
            ..
        } => {
            let (txt, y_off) = if *field == 0 { (find, 1) } else { (replace, 3) };
            let cx = (rect.x + 11 + txt.chars().count() as u16).min(rect.right().saturating_sub(2));
            f.set_cursor_position((cx, rect.y + y_off));
        }
        Modal::NamedRanges {
            name_input,
            ref_input,
            field,
            ..
        } if *field == 1 || *field == 2 => {
            let (txt, y_off) = if *field == 1 {
                (name_input, 1)
            } else {
                (ref_input, 2)
            };
            let cx = (rect.x + 9 + txt.chars().count() as u16).min(rect.right().saturating_sub(2));
            f.set_cursor_position((cx, rect.y + y_off));
        }
        Modal::ColumnFilter {
            draft, field: 0, ..
        } => {
            let cx = (rect.x + 17 + draft.substring.chars().count() as u16)
                .min(rect.right().saturating_sub(2));
            f.set_cursor_position((cx, rect.y + 1));
        }
        Modal::FilePicker(state)
            if (state.focus_input || !state.purpose.is_save())
                && app.layout.file_picker.is_some() =>
        {
            let fp = app.layout.file_picker.as_ref().unwrap();
            let max_chars = fp.input_rect.width.saturating_sub(2) as usize;
            let (_, cur_off) = visible_input_tail(&state.input, state.cursor, max_chars);
            let cx = (fp.input_text_x + cur_off).min(fp.input_rect.right().saturating_sub(1));
            f.set_cursor_position((cx, fp.input_rect.y));
        }
        _ => {}
    }
}
