use cells::actions::Action;
use cells::address::{
    apply_formula_ref, col_to_letters, format_range_ref, letters_to_col, parse_cell_ref,
    parse_range_ref,
};
use cells::app::{App, Modal};
use cells::chart::{
    export_chart_png, export_chart_svg, hover_chart_point, render_chart_bitmap, resolve_chart_data,
};
use cells::config::{Config, DockPosition, Palette, ThemeMode};
use cells::cursor::{
    CursorShape, SoftwareCursorRuntime, TerminalCursorCaps, build_cursor_sprite,
    has_da1_terminator, parse_osc22_none_reply,
};
use cells::file_picker::{FilePickerPurpose, FilePickerState, PostSaveAction};
use cells::formula::{
    FUNCTION_CATALOG, cycle_formula_ref_at_cursor, extract_precedents, lookup_function_sig,
};
use cells::input::handle_event;
use cells::ui;
use cells::workbook::{
    ChartKind, ColumnFilter, NumericFilterOp, PivotAgg, SortSpec, WorkbookState,
    check_orphaned_recovery, clear_recovery_snapshot,
};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use parquet::basic::Compression;
use parquet::data_type::{ByteArray, ByteArrayType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui_image::picker::Picker;
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use zip::ZipArchive;

fn temp_file(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("cells_test_{}_{}", std::process::id(), name));
    p
}

#[test]
fn address_and_live_formula_range_picking() {
    assert_eq!(col_to_letters(1), "A");
    assert_eq!(col_to_letters(26), "Z");
    assert_eq!(col_to_letters(27), "AA");
    assert_eq!(col_to_letters(703), "AAA");
    assert_eq!(letters_to_col("aa"), Some(27));
    assert_eq!(parse_cell_ref("$C$42"), Some((42, 3)));

    let r = format_range_ref(Some("Q1 Sales"), 10, 4, 2, 2);
    assert_eq!(r, "'Q1 Sales'!B2:D10");
    let (sheet, bounds) = parse_range_ref("Sheet1", &r).unwrap();
    assert_eq!(sheet, "Q1 Sales");
    assert_eq!(bounds, [2, 2, 10, 4]);

    let mut buf = "=SUM(".to_string();
    let mut cursor = buf.len();
    let mut span = None;
    assert!(apply_formula_ref(&mut buf, &mut cursor, &mut span, "B2"));
    assert_eq!(buf, "=SUM(B2");
    assert!(apply_formula_ref(
        &mut buf,
        &mut cursor,
        &mut span,
        "B2:B10"
    ));
    assert_eq!(buf, "=SUM(B2:B10");
}

#[test]
fn formulas_autofill_clipboard_and_undo_redo() {
    let mut wb = WorkbookState::new_empty().unwrap();
    wb.set_cell_input(0, 1, 1, "10".into()).unwrap();
    wb.set_cell_input(0, 2, 1, "20".into()).unwrap();
    wb.set_cell_input(0, 1, 2, "=A1*3".into()).unwrap();

    assert_eq!(wb.cell_numeric(0, 1, 2), Some(30.0));

    wb.fill_drag([1, 1, 2, 1], 4, 1).unwrap();
    assert_eq!(wb.cell_numeric(0, 3, 1), Some(30.0));
    assert_eq!(wb.cell_numeric(0, 4, 1), Some(40.0));

    wb.fill_drag([1, 2, 1, 2], 4, 2).unwrap();
    assert_eq!(wb.cell_numeric(0, 4, 2), Some(120.0));

    wb.select_range(1, 2, 2, 2);
    wb.copy_or_cut(false).unwrap();
    wb.select_cell(1, 3);
    wb.paste().unwrap();
    assert_eq!(wb.cell_numeric(0, 1, 3), Some(90.0));

    assert!(wb.undo().unwrap());
    assert_eq!(wb.cell_numeric(0, 1, 3), None);
    assert!(wb.redo().unwrap());
    assert_eq!(wb.cell_numeric(0, 1, 3), Some(90.0));
}

#[test]
fn xlsx_roundtrip_native_ooxml_charts_and_repeated_save_overwrite() {
    let path = temp_file("roundtrip.xlsx");
    let _ = fs::remove_file(&path);

    let mut cfg = Config {
        theme: ThemeMode::Dark,
        dock_position: DockPosition::Bottom,
        dock_percent: 38,
    };

    {
        let mut wb = WorkbookState::new_empty().unwrap();
        wb.set_cell_input(0, 1, 1, "Month".into()).unwrap();
        wb.set_cell_input(0, 1, 2, "Revenue".into()).unwrap();
        wb.set_cell_input(0, 1, 3, "Margin".into()).unwrap();
        wb.set_cell_input(0, 2, 1, "Jan".into()).unwrap();
        wb.set_cell_input(0, 2, 2, "1250.5".into()).unwrap();
        wb.set_cell_input(0, 2, 3, "0.32".into()).unwrap();
        wb.set_cell_input(0, 3, 1, "Feb".into()).unwrap();
        wb.set_cell_input(0, 3, 2, "=B2*2".into()).unwrap();
        wb.set_cell_input(0, 3, 3, "0.41".into()).unwrap();

        wb.select_cell(2, 2);
        wb.toggle_style_flag("font.b").unwrap();
        wb.apply_style("fill.color", "#1E3A8A").unwrap();
        wb.apply_style("num_fmt", "$#,##0.00").unwrap();
        wb.set_col_width_chars(0, 2, 18).unwrap();
        wb.set_frozen(1, 1).unwrap();

        wb.select_range(2, 2, 3, 2);
        wb.add_data_bar_on_selection("#3B82F6");
        wb.add_sparkline(2, 4, [2, 2, 3, 2]);

        wb.select_range(1, 1, 3, 3);
        wb.create_chart_from_selection(ChartKind::Line);
        wb.create_chart_from_selection(ChartKind::DualAxis);
        wb.create_chart_from_selection(ChartKind::Pie);

        wb.save_as(&path, &cfg, true).unwrap();
        // Repeated Ctrl+S save to existing file must overwrite cleanly.
        wb.save(&cfg, true).unwrap();
    }

    // Verify real OOXML DrawingML chart parts inside the saved .xlsx ZIP!
    {
        let f = File::open(&path).unwrap();
        let mut zip = ZipArchive::new(f).unwrap();
        let mut chart1 = String::new();
        zip.by_name("xl/charts/chart1.xml")
            .expect("expected native OOXML chart1.xml")
            .read_to_string(&mut chart1)
            .unwrap();
        assert!(chart1.contains("<c:chartSpace"));
        assert!(chart1.contains("<c:lineChart>"));

        let mut chart2 = String::new();
        zip.by_name("xl/charts/chart2.xml")
            .expect("expected DualAxis chart2.xml")
            .read_to_string(&mut chart2)
            .unwrap();
        assert!(chart2.contains("<c:barChart>"));
        assert!(chart2.contains("<c:lineChart>"));

        let mut drawing1 = String::new();
        zip.by_name("xl/drawings/drawing1.xml")
            .expect("expected drawing1.xml")
            .read_to_string(&mut drawing1)
            .unwrap();
        assert!(drawing1.contains("<xdr:wsDr"));

        let mut sheet1 = String::new();
        zip.by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet1)
            .unwrap();
        assert!(sheet1.contains("rIdCellsDrawing"));
    }

    let (reloaded, meta) = WorkbookState::open_path(&path).unwrap();
    let _ = fs::remove_file(&path);

    let sheets = reloaded.visible_sheets();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].1, "Sheet1");
    assert!(!reloaded.user.can_undo());

    let meta = meta.expect("expected embedded metadata");
    assert!(meta.dock_visible);
    assert_eq!(meta.dock_position, DockPosition::Bottom);
    assert_eq!(meta.dock_percent, 38);
    assert_eq!(reloaded.charts.len(), 3);
    assert_eq!(reloaded.charts[0].kind, ChartKind::Line);
    assert_eq!(reloaded.charts[1].kind, ChartKind::DualAxis);
    assert_eq!(reloaded.charts[2].kind, ChartKind::Pie);
    assert_eq!(reloaded.visual_rules.len(), 2);

    assert_eq!(reloaded.cell_numeric(0, 3, 2), Some(2501.0));
    assert_eq!(reloaded.cell_display(0, 2, 2), "$1,250.50");
    let rs = reloaded.cell_render_style(0, 2, 2);
    assert!(rs.bold);
    assert_eq!(rs.bg, Some(Color::Rgb(0x1E, 0x3A, 0x8A)));
    assert_eq!(reloaded.frozen_counts(0), (1, 1));
    assert_eq!(reloaded.col_width_chars(0, 2), 18);

    cfg.dock_percent = meta.dock_percent;
    assert_eq!(cfg.dock_percent, 38);
}

#[test]
fn stdin_delimited_sort_and_find_replace() {
    let raw = "Item\tScore\nBeta\t10\nAlpha\t50\nGamma\t30\n";
    let mut wb = WorkbookState::from_stdin_str(raw).unwrap();
    assert_eq!(wb.cell_display(0, 1, 1), "Item");
    assert_eq!(wb.cell_numeric(0, 3, 2), Some(50.0));

    wb.set_cell_input(0, 5, 1, "=CONCAT(\"Alpha\", \"!\")".into())
        .unwrap();

    wb.select_range(1, 1, 4, 2);
    wb.user.set_selected_cell(1, 2).unwrap();
    wb.user.on_area_selecting(4, 1).unwrap();
    wb.sort_selection(false).unwrap();
    assert_eq!(wb.cell_display(0, 1, 1), "Item");
    assert_eq!(wb.cell_display(0, 2, 1), "Alpha");
    assert_eq!(wb.cell_numeric(0, 2, 2), Some(50.0));
    assert_eq!(wb.cell_display(0, 4, 1), "Beta");

    let hits = wb.find_matches("alpha", false, false);
    assert_eq!(hits.len(), 2);
    let replaced = wb.replace_matches("alpha", "Omega", false, false).unwrap();
    assert_eq!(replaced, 1);
    assert_eq!(wb.cell_display(0, 2, 1), "Omega");
    assert!(wb.cell_input_content(0, 5, 1).starts_with('='));
}

#[test]
fn all_eight_chart_types_hover_bitmap_png_and_svg() {
    let mut wb = WorkbookState::from_stdin_str(
        "Quarter,Sales,Margin\nQ1,120,0.18\nQ2,180,0.24\nQ3,150,0.21\nQ4,240,0.31\n",
    )
    .unwrap();
    wb.select_range(1, 1, 5, 3);
    let idx = wb.create_chart_from_selection(ChartKind::Line);
    let palette = Palette::for_theme(ThemeMode::Dark);

    assert_eq!(ChartKind::ALL.len(), 8);
    for kind in ChartKind::ALL {
        wb.charts[idx].kind = kind;
        let spec = &wb.charts[idx];
        let data = resolve_chart_data(&wb, spec).expect("expected resolved chart data");
        let img = render_chart_bitmap(spec, &data, &palette, 480, 300).unwrap();
        assert_eq!(img.width(), 480);
        assert_eq!(img.height(), 300);

        let hit = hover_chart_point(&data, Rect::new(0, 0, 60, 20), 30, 10)
            .expect("expected chart hover readout");
        assert!((2..=5).contains(&hit.source_row));
    }

    let spec = &wb.charts[idx];
    let data = resolve_chart_data(&wb, spec).unwrap();
    let png_path = temp_file("chart.png");
    let svg_path = temp_file("chart.svg");
    export_chart_png(&png_path, spec, &data, &palette).unwrap();
    export_chart_svg(&svg_path, spec, &data, &palette).unwrap();

    let png_bytes = fs::read(&png_path).unwrap();
    assert!(png_bytes.starts_with(b"\x89PNG"));
    let svg_text = fs::read_to_string(&svg_path).unwrap();
    assert!(svg_text.contains("<svg"));

    let _ = fs::remove_file(&png_path);
    let _ = fs::remove_file(&svg_path);
}

#[test]
fn tui_draw_mouse_menus_formula_drag_and_command_palette() {
    let wb = WorkbookState::from_stdin_str("Name,Val\nA,10\nB,20\nC,30\n").unwrap();
    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());
    app.execute_action(Action::FreezeTopRow);
    app.wb.select_range(1, 1, 4, 2);
    app.execute_action(Action::NewChart(ChartKind::Bar));

    let backend = TestBackend::new(120, 34);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::draw(f, &mut app)).unwrap();

    assert!(!app.layout.cols.is_empty());
    assert!(!app.layout.rows.is_empty());
    assert!(app.layout.chart_dock_rect.is_some());

    let col_b_x = app.layout.cols[1].x_start + 1;
    let row_2_y = app.layout.rows.iter().find(|r| r.row == 2).unwrap().y;
    let row_4_y = app.layout.rows.iter().find(|r| r.row == 4).unwrap().y;
    let row_5_y = app.layout.rows.iter().find(|r| r.row == 5).unwrap().y;

    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col_b_x,
            row: row_5_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col_b_x,
            row: row_5_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(app.wb.selected_cell(), (5, 2));

    for ch in "=SUM(".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col_b_x,
            row: row_2_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: col_b_x,
            row: row_4_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col_b_x,
            row: row_4_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char(')'), KeyModifiers::NONE)),
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(app.wb.cell_numeric(0, 5, 2), Some(60.0));

    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: col_b_x,
            row: row_2_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert!(app.context_menu.is_some());
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert!(!app.layout.context_items.is_empty());
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    assert!(app.context_menu.is_none());

    // Open Command Palette via Ctrl+P, jump to A1 via F5 / Go To, and extend range via Shift+Arrows.
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
    );
    assert!(matches!(app.modal, Modal::CommandPalette { .. }));
    for ch in "Go to Cell".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(matches!(app.modal, Modal::Prompt { .. }));
    for ch in "A1".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(app.wb.selected_cell(), (1, 1));

    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)),
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
    );
    assert_eq!(app.wb.selected_range(), [1, 1, 2, 2]);
}

#[test]
fn software_cross_cursor_sprites_fallback_and_contextual_shapes() {
    for shape in CursorShape::ALL {
        let sp = build_cursor_sprite(shape, 25);
        assert_eq!(sp.width, 25);
        assert_eq!(sp.height, 25);
        assert_eq!(sp.rgba.len(), 25 * 25 * 4);
        assert!(sp.rgba.as_chunks::<4>().0.iter().any(|px| px[3] > 0));
    }

    // Verify classic CellCross fat white-filled interior + crisp black border + drop shadow.
    let cross = build_cursor_sprite(CursorShape::CellCross, 25);
    let center_idx = ((cross.hotspot_y as usize) * 25 + (cross.hotspot_x as usize)) * 4;
    assert_eq!(
        &cross.rgba[center_idx..center_idx + 4],
        &[255, 255, 255, 255]
    );
    assert!(cross.rgba.as_chunks::<4>().0.contains(&[14, 18, 26, 255]));
    assert!(cross.rgba.as_chunks::<4>().0.contains(&[0, 0, 0, 88]));

    // Verify SGR-Pixels (?1016h) sub-cell normalization vs cell normalization.
    let picker = Picker::halfblocks();
    let rt_px = SoftwareCursorRuntime::new(&picker, true);
    assert_eq!(rt_px.normalize_mouse_coords(95, 44, 10, 20), (9, 2, 95, 44));
    let rt_cell = SoftwareCursorRuntime::new(&picker, false);
    assert_eq!(rt_cell.normalize_mouse_coords(9, 2, 10, 20), (9, 2, 95, 50));

    // Verify OSC 22 `?none` probe parsing & single-cursor arbitration (no double cursor!).
    assert!(!has_da1_terminator(b"\x1b]22;0\x1b\\"));
    assert!(has_da1_terminator(b"\x1b]22;0\x1b\\\x1b[?62;c"));
    assert!(!parse_osc22_none_reply(b"\x1b]22;0\x1b\\\x1b[?62;c")); // Kitty 0.50 reply
    assert!(parse_osc22_none_reply(b"\x1b]22;1\x1b\\\x1b[?62;c"));

    let screen = ratatui::layout::Rect::new(0, 0, 40, 10);
    // Live TTY where `OSC 22;none` is unsupported (e.g. Kitty 0.50): must NOT draw cell `✚` or
    // Kitty sprite, and MUST switch the single OS cursor via `OSC 22;cell` / `ew-resize`.
    let mut rt_tty_no_hide = SoftwareCursorRuntime::with_caps(
        &picker,
        TerminalCursorCaps {
            is_tty: true,
            sgr_pixels: false,
            osc22_none: false,
        },
    );
    let mut buf_no_hide = ratatui::buffer::Buffer::empty(screen);
    rt_tty_no_hide.draw_cell_fallback(
        &mut buf_no_hide,
        screen,
        Some((5, 5)),
        CursorShape::CellCross,
    );
    assert_ne!(buf_no_hide[(5, 5)].symbol(), "✚");
    let mut esc_out = Vec::new();
    rt_tty_no_hide
        .sync_terminal_cursor(
            &mut esc_out,
            &picker,
            screen,
            Some((50, 50)),
            CursorShape::CellCross,
        )
        .unwrap();
    assert_eq!(esc_out, b"\x1b]22;cell\x1b\\");
    esc_out.clear();
    rt_tty_no_hide
        .sync_terminal_cursor(
            &mut esc_out,
            &picker,
            screen,
            Some((50, 50)),
            CursorShape::ResizeEW,
        )
        .unwrap();
    assert_eq!(esc_out, b"\x1b]22;ew-resize\x1b\\");
    esc_out.clear();
    rt_tty_no_hide.shutdown(&mut esc_out);
    assert_eq!(esc_out, b"\x1b]22;\x1b\\");

    // Live TTY where `OSC 22;none` IS supported on Halfblocks: hides OS cursor and draws `✚`.
    let mut rt_tty_can_hide = SoftwareCursorRuntime::with_caps(
        &picker,
        TerminalCursorCaps {
            is_tty: true,
            sgr_pixels: false,
            osc22_none: true,
        },
    );
    let mut buf_can_hide = ratatui::buffer::Buffer::empty(screen);
    rt_tty_can_hide.draw_cell_fallback(
        &mut buf_can_hide,
        screen,
        Some((5, 5)),
        CursorShape::CellCross,
    );
    assert_eq!(buf_can_hide[(5, 5)].symbol(), "✚");
    esc_out.clear();
    rt_tty_can_hide
        .sync_terminal_cursor(
            &mut esc_out,
            &picker,
            screen,
            Some((50, 50)),
            CursorShape::CellCross,
        )
        .unwrap();
    assert_eq!(esc_out, b"\x1b]22;none\x1b\\");

    // Verify contextual cursor transitions and cell-buffer `✚` rendering.
    let wb = WorkbookState::from_stdin_str("Cat,Val\nA,10\nB,20\n").unwrap();
    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());
    app.wb.select_range(1, 1, 3, 2);
    app.execute_action(Action::NewChart(ChartKind::Line));
    app.wb.select_cell(2, 2);

    let backend = TestBackend::new(120, 30);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::draw(f, &mut app)).unwrap();

    let cell_x = app.layout.cols[1].x_start + 2;
    let cell_y = app.layout.rows.iter().find(|r| r.row == 2).unwrap().y;
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: cell_x,
            row: cell_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(app.cursor_shape, CursorShape::CellCross);
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert_eq!(term.backend().buffer()[(cell_x, cell_y)].symbol(), "✚");

    let (fh_x, fh_y) = app.layout.fill_handle.unwrap();
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: fh_x,
            row: fh_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(app.cursor_shape, CursorShape::FillCrosshair);

    let (bd_x, bd_y) = app.layout.selection_border_cells[0];
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: bd_x,
            row: bd_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(app.cursor_shape, CursorShape::Move);

    let fx_x = app.layout.formula_rect.x + 3;
    let fx_y = app.layout.formula_rect.y;
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: fx_x,
            row: fx_y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(app.cursor_shape, CursorShape::IBeam);

    let cb = app.layout.chart_body_rect.unwrap();
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: cb.x + cb.width / 2,
            row: cb.y + cb.height / 2,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(app.cursor_shape, CursorShape::FillCrosshair);
    assert!(app.chart_hover.is_some());
}

#[test]
fn formula_autocomplete_f4_cycling_precedents_and_named_ranges() {
    let mut wb = WorkbookState::new_empty().unwrap();
    wb.set_cell_input(0, 1, 1, "40".into()).unwrap();
    wb.set_cell_input(0, 2, 1, "60".into()).unwrap();
    wb.set_cell_input(0, 1, 2, "0.15".into()).unwrap();
    wb.add_or_update_defined_name("TaxRate", "$B$1").unwrap();

    // Verify F4 reference cycling: A1 -> $A$1 -> A$1 -> $A1 -> A1
    let mut fbuf = "=A1+1".to_string();
    let mut fcur = 2usize;
    assert!(cycle_formula_ref_at_cursor(&wb, &mut fbuf, &mut fcur));
    assert_eq!(fbuf, "=$A$1+1");
    assert!(cycle_formula_ref_at_cursor(&wb, &mut fbuf, &mut fcur));
    assert_eq!(fbuf, "=A$1+1");
    assert!(cycle_formula_ref_at_cursor(&wb, &mut fbuf, &mut fcur));
    assert_eq!(fbuf, "=$A1+1");
    assert!(cycle_formula_ref_at_cursor(&wb, &mut fbuf, &mut fcur));
    assert_eq!(fbuf, "=A1+1");

    let precs = extract_precedents("=SUM(A1:A2, B1)");
    assert_eq!(precs.len(), 2);
    assert_eq!(
        (precs[0].r1, precs[0].c1, precs[0].r2, precs[0].c2),
        (1, 1, 2, 1)
    );
    assert_eq!((precs[1].r1, precs[1].c1), (1, 2));

    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());
    app.wb.select_cell(3, 1);

    // Type `=SU` + Tab -> autocompletes `=SUM(`, then type `A1:A2` + F4 + `)*Tax` + Tab + Enter!
    for ch in "=SU".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    assert!(!app.formula_assist.candidates.is_empty());
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
    );
    assert_eq!(app.edit.as_ref().unwrap().buf, "=SUM(");
    assert!(app.formula_assist.active_fn.is_some());

    for ch in "A1:A2".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
    );
    assert!(app.edit.as_ref().unwrap().buf.contains("$A$1:$A$2"));

    for ch in ")*Tax".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    // Defined name `TaxRate` appears in autocomplete candidates!
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "TaxRate")
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(app.wb.cell_numeric(0, 3, 1), Some(15.0));
}

#[test]
fn autofilter_visual_rules_multisort_pivot_and_drag_move() {
    let raw = "Region,Rep,Sales\nWest,Alice,300\nEast,Bob,100\nWest,Cara,500\nEast,Dan,200\n";
    let wb = WorkbookState::from_stdin_str(raw).unwrap();
    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());

    // Multi-column sort: Region ASC (col 1), Sales DESC (col 3)
    app.wb.select_range(1, 1, 5, 3);
    app.wb
        .sort_selection_multi(
            &[
                SortSpec {
                    col: 1,
                    ascending: true,
                },
                SortSpec {
                    col: 3,
                    ascending: false,
                },
            ],
            true,
        )
        .unwrap();
    assert_eq!(app.wb.cell_display(0, 2, 2), "Dan"); // East, 200 before East, 100
    assert_eq!(app.wb.cell_display(0, 3, 2), "Bob");
    assert_eq!(app.wb.cell_display(0, 4, 2), "Cara"); // West, 500 before West, 300

    // Sparkline & conditional visual rules
    let spark = app.wb.compute_sparkline_text(0, [2, 3, 5, 3], 8);
    assert_eq!(spark.chars().count(), 4);
    assert!(spark.contains('▁') && spark.contains('█'));

    // AutoFilter: hide rows where Sales < 250 -> hides East rows (200, 100), keeps West (500, 300)
    app.wb.select_range(1, 1, 5, 3);
    assert!(app.wb.toggle_autofilter_on_selection().unwrap());
    {
        let af = app.wb.active_autofilter_mut("Sheet1").unwrap();
        af.col_filters.insert(
            3,
            ColumnFilter {
                substring: String::new(),
                numeric_op: NumericFilterOp::Gt,
                numeric_val: 250.0,
                excluded_values: Default::default(),
            },
        );
    }
    let hidden = app.wb.reapply_autofilter(0).unwrap();
    assert_eq!(hidden, 2);
    assert!(app.wb.is_row_hidden(0, 2));
    assert!(app.wb.is_row_hidden(0, 3));
    assert!(!app.wb.is_row_hidden(0, 4));

    let backend = TestBackend::new(100, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let visible_rows: Vec<i32> = app.layout.rows.iter().map(|r| r.row).collect();
    assert!(!visible_rows.contains(&2));
    assert!(!visible_rows.contains(&3));
    assert!(visible_rows.contains(&4));

    // Disable AutoFilter so all rows participate in Pivot Summary
    assert!(!app.wb.toggle_autofilter_on_selection().unwrap());
    app.wb.select_range(1, 1, 5, 3);
    let pivot_name = app.wb.create_pivot_sheet(1, 3, PivotAgg::Sum).unwrap();
    assert_eq!(pivot_name, "Pivot 1");
    let p_idx = app.wb.sheet_index_by_name("Pivot 1").unwrap();
    assert_eq!(app.wb.cell_display(p_idx, 2, 1), "East");
    assert_eq!(app.wb.cell_numeric(p_idx, 2, 2), Some(300.0));
    assert_eq!(app.wb.cell_display(p_idx, 3, 1), "West");
    assert_eq!(app.wb.cell_numeric(p_idx, 3, 2), Some(800.0));
    assert_eq!(app.wb.cell_display(p_idx, 4, 1), "Grand Total");
    assert_eq!(app.wb.cell_numeric(p_idx, 4, 2), Some(1100.0));

    // Move range on Pivot sheet
    let moved = app.wb.move_range([2, 1, 3, 2], 6, 4).unwrap();
    assert_eq!(moved, [6, 4, 7, 5]);
    assert_eq!(app.wb.cell_display(p_idx, 6, 4), "East");
    assert_eq!(app.wb.cell_numeric(p_idx, 7, 5), Some(800.0));
}

#[test]
fn json_jsonl_sqlite_parquet_unified_undo_and_recovery() {
    // 1. JSONL stdin import + formula injection protection
    let jsonl = "{\"host\":\"srv-1\",\"cpu\":42.5,\"note\":\"=cmd|'/c calc'!A0\"}\n{\"host\":\"srv-2\",\"cpu\":88.0,\"note\":\"ok\"}\n";
    let wb_json = WorkbookState::from_stdin_str(jsonl).unwrap();
    assert_eq!(wb_json.cell_display(0, 1, 1), "host");
    assert_eq!(wb_json.cell_numeric(0, 3, 2), Some(88.0));
    assert!(wb_json.cell_input_content(0, 2, 3).starts_with('\''));

    // 2. SQLite multi-table import -> one worksheet per table
    let db_path = temp_file("sample.sqlite");
    let _ = fs::remove_file(&db_path);
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE metrics (day TEXT, reqs INTEGER);
             INSERT INTO metrics VALUES ('Mon', 1200), ('Tue', 1850);
             CREATE TABLE users (id INTEGER, handle TEXT);
             INSERT INTO users VALUES (1, 'ada'), (2, 'grace');",
        )
        .unwrap();
    }
    let (wb_sql, _) = WorkbookState::open_path(&db_path).unwrap();
    let _ = fs::remove_file(&db_path);
    let sheet_names: Vec<String> = wb_sql
        .visible_sheets()
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    assert_eq!(sheet_names, vec!["metrics", "users"]);
    assert_eq!(wb_sql.cell_numeric(0, 3, 2), Some(1850.0));
    assert_eq!(wb_sql.cell_display(1, 2, 2), "ada");

    // 3. Parquet import
    let pq_path = temp_file("sample.parquet");
    let _ = fs::remove_file(&pq_path);
    {
        let schema = Arc::new(
            parse_message_type(
                "message schema { REQUIRED INT64 qty; REQUIRED BYTE_ARRAY sku (UTF8); }",
            )
            .unwrap(),
        );
        let props = Arc::new(
            WriterProperties::builder()
                .set_compression(Compression::SNAPPY)
                .build(),
        );
        let file = File::create(&pq_path).unwrap();
        let mut writer = SerializedFileWriter::new(file, schema, props).unwrap();
        let mut rg = writer.next_row_group().unwrap();
        {
            let mut col = rg.next_column().unwrap().unwrap();
            col.typed::<Int64Type>()
                .write_batch(&[15, 30], None, None)
                .unwrap();
            col.close().unwrap();
        }
        {
            let mut col = rg.next_column().unwrap().unwrap();
            col.typed::<ByteArrayType>()
                .write_batch(
                    &[ByteArray::from("SKU-A"), ByteArray::from("SKU-B")],
                    None,
                    None,
                )
                .unwrap();
            col.close().unwrap();
        }
        rg.close().unwrap();
        writer.close().unwrap();
    }
    let (wb_pq, _) = WorkbookState::open_path(&pq_path).unwrap();
    let _ = fs::remove_file(&pq_path);
    assert_eq!(wb_pq.cell_display(0, 1, 1), "qty");
    assert_eq!(wb_pq.cell_numeric(0, 3, 1), Some(30.0));
    assert_eq!(wb_pq.cell_display(0, 2, 2), "SKU-A");

    // 4. Unified Undo/Redo across cell edits + chart creation + conditional rules + recovery
    let target_xlsx = temp_file("rec_target.xlsx");
    let _ = fs::remove_file(&target_xlsx);
    clear_recovery_snapshot(Some(&target_xlsx));

    let mut wb = WorkbookState::new_empty().unwrap();
    wb.file_path = Some(target_xlsx.clone());
    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());

    app.start_edit_replace("42".into());
    app.commit_edit();
    assert_eq!(app.wb.cell_numeric(0, 1, 1), Some(42.0));

    app.execute_action(Action::NewChart(ChartKind::Histogram));
    assert_eq!(app.wb.charts.len(), 1);
    app.execute_action(Action::AddDataBars);
    assert_eq!(app.wb.visual_rules.len(), 1);

    // Write periodic recovery snapshot and verify detection + `Action::RecoverSnapshot`
    let rec_file = app
        .wb
        .write_recovery_snapshot(&app.cfg, app.dock_visible)
        .unwrap();
    assert!(rec_file.is_file());
    assert!(check_orphaned_recovery(Some(&target_xlsx)).is_some());

    // Undo DataBars -> Undo Chart -> Undo Cell Edit -> Redo all 3!
    app.execute_action(Action::Undo);
    assert_eq!(app.wb.visual_rules.len(), 0);
    assert_eq!(app.wb.charts.len(), 1);

    app.execute_action(Action::Undo);
    assert_eq!(app.wb.charts.len(), 0);
    assert_eq!(app.wb.cell_numeric(0, 1, 1), Some(42.0));

    app.execute_action(Action::Undo);
    assert_eq!(app.wb.cell_numeric(0, 1, 1), None);

    app.execute_action(Action::Redo);
    assert_eq!(app.wb.cell_numeric(0, 1, 1), Some(42.0));
    app.execute_action(Action::Redo);
    assert_eq!(app.wb.charts.len(), 1);
    app.execute_action(Action::Redo);
    assert_eq!(app.wb.visual_rules.len(), 1);

    clear_recovery_snapshot(Some(&target_xlsx));
    assert!(check_orphaned_recovery(Some(&target_xlsx)).is_none());
}

#[test]
fn excel_keyboard_shortcuts_end_to_end() {
    let wb = WorkbookState::from_stdin_str("Item,Q1,Q2\nAlpha,10,20\nBeta,30,40\n").unwrap();
    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());

    let press = |app: &mut App, code: KeyCode, mods: KeyModifiers| {
        handle_event(app, Event::Key(KeyEvent::new(code, mods)));
    };

    // 1. Ctrl+Shift+Down then Ctrl+Shift+Right selects full data block A1:C3.
    app.wb.select_cell(1, 1);
    press(
        &mut app,
        KeyCode::Down,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.wb.selected_range(), [1, 1, 3, 1]);
    press(
        &mut app,
        KeyCode::Right,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.wb.selected_range(), [1, 1, 3, 3]);

    // 2. Ctrl+End jumps to C3; Ctrl+Shift+Home extends back to A1:C3; Ctrl+A cycles region -> full sheet.
    app.wb.select_cell(1, 1);
    press(&mut app, KeyCode::End, KeyModifiers::CONTROL);
    assert_eq!(app.wb.selected_cell(), (3, 3));
    press(
        &mut app,
        KeyCode::Home,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.wb.selected_range(), [1, 1, 3, 3]);
    app.wb.select_cell(1, 1);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(app.wb.selected_range(), [1, 1, 3, 3]);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(app.wb.selected_range()[2], cells::address::MAX_ROWS);

    // 3. Ctrl+Space selects full column; Shift+Space selects full row.
    app.wb.select_cell(2, 2);
    press(&mut app, KeyCode::Char(' '), KeyModifiers::CONTROL);
    assert_eq!(app.wb.selected_range(), [1, 2, cells::address::MAX_ROWS, 2]);
    app.wb.select_cell(2, 2);
    press(&mut app, KeyCode::Char(' '), KeyModifiers::SHIFT);
    assert_eq!(app.wb.selected_range(), [2, 1, 2, cells::address::MAX_COLS]);

    // 4. Alt+= AutoSum on B4 sums B2:B3 (= 40), and Ctrl+R on B4:C4 fills right (=SUM(C2:C3) = 60).
    app.wb.select_cell(4, 2);
    press(&mut app, KeyCode::Char('='), KeyModifiers::ALT);
    assert_eq!(app.wb.cell_input_content(0, 4, 2), "=SUM(B2:B3)");
    assert_eq!(app.wb.cell_numeric(0, 4, 2), Some(40.0));
    app.wb.select_range(4, 2, 4, 3);
    press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(app.wb.cell_input_content(0, 4, 3), "=SUM(C2:C3)");
    assert_eq!(app.wb.cell_numeric(0, 4, 3), Some(60.0));

    // 5. Ctrl+D Fill Down on D2:D3 with relative formula `=B2+C2`.
    app.wb.set_cell_input(0, 2, 4, "=B2+C2".into()).unwrap();
    app.wb.select_range(2, 4, 3, 4);
    press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(app.wb.cell_input_content(0, 3, 4), "=B3+C3");
    assert_eq!(app.wb.cell_numeric(0, 3, 4), Some(70.0));

    // 6. Ctrl+Enter commits formula/value across multi-cell selection E2:E3 and keeps selection.
    app.wb.select_range(2, 5, 3, 5);
    press(&mut app, KeyCode::Char('='), KeyModifiers::NONE);
    for ch in "D2*2".chars() {
        press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
    assert_eq!(app.wb.selected_range(), [2, 5, 3, 5]);
    assert_eq!(app.wb.cell_numeric(0, 2, 5), Some(60.0));
    assert_eq!(app.wb.cell_numeric(0, 3, 5), Some(140.0));

    // 7. Ctrl+` toggles Show Formulas; Ctrl+5 toggles Strikethrough; Ctrl+Shift+$ formats Currency.
    press(&mut app, KeyCode::Char('`'), KeyModifiers::CONTROL);
    assert!(app.show_formulas);
    app.wb.select_cell(2, 5);
    press(&mut app, KeyCode::Char('5'), KeyModifiers::CONTROL);
    assert!(app.wb.cell_render_style(0, 2, 5).strikethrough);
    press(
        &mut app,
        KeyCode::Char('$'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(app.wb.cell_display(0, 2, 5).contains('$'));

    // 8. Ctrl+; inserts ISO date, Ctrl+' copies formula from above, Ctrl+" copies value from above.
    app.wb.select_cell(6, 1);
    press(&mut app, KeyCode::Char(';'), KeyModifiers::CONTROL);
    assert_eq!(app.wb.cell_display(0, 6, 1).len(), 10);
    app.wb.select_cell(4, 5);
    press(&mut app, KeyCode::Char('\''), KeyModifiers::CONTROL);
    assert_eq!(app.wb.cell_input_content(0, 4, 5), "=D3*2");
    press(&mut app, KeyCode::Char('"'), KeyModifiers::CONTROL);
    assert_eq!(app.wb.cell_input_content(0, 4, 5), "140");

    // 9. Shift+F11 adds a worksheet; Ctrl+PageUp / Ctrl+PageDown switches sheets; Alt+F1 adds chart.
    press(&mut app, KeyCode::F(11), KeyModifiers::SHIFT);
    assert_eq!(app.wb.visible_sheets().len(), 2);
    let second_sheet = app.wb.active_sheet();
    press(&mut app, KeyCode::PageUp, KeyModifiers::CONTROL);
    assert_eq!(app.wb.active_sheet(), 0);
    press(&mut app, KeyCode::PageDown, KeyModifiers::CONTROL);
    assert_eq!(app.wb.active_sheet(), second_sheet);
    press(&mut app, KeyCode::PageUp, KeyModifiers::CONTROL);

    app.wb.select_range(1, 1, 3, 3);
    press(&mut app, KeyCode::F(1), KeyModifiers::ALT);
    assert_eq!(app.wb.charts.len(), 1);

    // 10. Ctrl+H opens Find & Replace focused on Replace (field == 1); F5 opens Go To prompt.
    press(&mut app, KeyCode::Char('h'), KeyModifiers::CONTROL);
    assert!(matches!(app.modal, Modal::FindReplace { field: 1, .. }));
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, KeyCode::F(5), KeyModifiers::NONE);
    assert!(matches!(app.modal, Modal::Prompt { .. }));
}

#[test]
fn virtual_file_picker_open_save_close_and_mouse_navigation() {
    let dir = temp_file("picker_root");
    let _ = fs::remove_dir_all(&dir);
    let sub = dir.join("reports");
    fs::create_dir_all(&sub).unwrap();
    fs::write(dir.join(".hidden.csv"), "k,v\nhid,1\n").unwrap();
    fs::write(dir.join("readme.txt"), "not a spreadsheet\n").unwrap();
    fs::write(
        sub.join("metrics.csv"),
        "Metric,Score\nLatency,12\nThroughput,950\n",
    )
    .unwrap();

    let wb = WorkbookState::new_empty().unwrap();
    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());
    let backend = TestBackend::new(110, 28);
    let mut term = Terminal::new(backend).unwrap();

    // 1. Ctrl+O opens Virtual File Picker; verify extension & dotfile filters + toggles.
    app.modal = Modal::FilePicker(FilePickerState::new(
        FilePickerPurpose::Open,
        Some(dir.clone()),
        String::new(),
    ));
    if let Modal::FilePicker(ref st) = app.modal {
        let names: Vec<&str> = st.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"reports"));
        assert!(!names.contains(&".hidden.csv"));
        assert!(!names.contains(&"readme.txt"));
    } else {
        panic!("expected FilePicker modal");
    }

    // Ctrl+H reveals dotfiles; Ctrl+A reveals all extensions.
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
    );
    if let Modal::FilePicker(ref st) = app.modal {
        assert!(st.entries.iter().any(|e| e.name == ".hidden.csv"));
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
    );
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
    );
    if let Modal::FilePicker(ref st) = app.modal {
        assert!(st.entries.iter().any(|e| e.name == "readme.txt"));
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
    );

    // 2. Navigate into `reports/` via Enter, then open `metrics.csv` via mouse click on `[ Open ]`.
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let fp_layout = app
        .layout
        .file_picker
        .clone()
        .expect("expected file picker layout");
    assert!(!fp_layout.row_rects.is_empty());
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: fp_layout.btn_confirm.x + 2,
            row: fp_layout.btn_confirm.y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert!(matches!(app.modal, Modal::None));
    assert_eq!(app.wb.cell_numeric(0, 3, 2), Some(950.0));

    // 3. Create dirty untitled workbook -> Ctrl+W (`CloseWorkbook`) -> `[S] Save & Close`
    //    opens Virtual File Picker, appends `.xlsx`, saves, and closes back to fresh workbook!
    app.execute_action(Action::NewWorkbook);
    app.wb.set_cell_input(0, 1, 1, "777".into()).unwrap();
    assert!(app.wb.dirty);

    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
    );
    assert!(matches!(app.modal, Modal::ConfirmQuit { quit_app: false }));
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
    );
    if let Modal::FilePicker(ref mut st) = app.modal {
        assert_eq!(
            st.purpose,
            FilePickerPurpose::SaveWorkbook {
                after: PostSaveAction::CloseWorkbook
            }
        );
        st.enter_dir(dir.clone(), None);
        st.input = "closed_snapshot".into(); // omit .xlsx to verify auto-extension
        st.cursor = st.input.len();
        st.focus_input = true;
    } else {
        panic!("expected SaveWorkbook FilePicker on untitled Save & Close");
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(matches!(app.modal, Modal::None));
    let saved_xlsx = dir.join("closed_snapshot.xlsx");
    assert!(saved_xlsx.is_file());
    // Current workbook was closed back to an empty clean sheet, while saved file holds 777!
    assert_eq!(app.wb.cell_numeric(0, 1, 1), None);
    assert!(!app.wb.dirty);
    let (reopened, _) = WorkbookState::open_path(&saved_xlsx).unwrap();
    assert_eq!(reopened.cell_numeric(0, 1, 1), Some(777.0));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn formula_intellisense_catalog_fuzzy_anchoring_and_mouse_accept() {
    // 1. Verify FUNCTION_CATALOG covers all 497 IronCalc functions, strictly sorted.
    assert_eq!(FUNCTION_CATALOG.len(), 497);
    for pair in FUNCTION_CATALOG.windows(2) {
        assert!(
            pair[0].name < pair[1].name,
            "FUNCTION_CATALOG out of order: {} >= {}",
            pair[0].name,
            pair[1].name
        );
    }
    let mut wb = WorkbookState::new_empty().unwrap();
    for sig in FUNCTION_CATALOG {
        assert!(
            lookup_function_sig(sig.name).is_some(),
            "missing {}",
            sig.name
        );
        if sig.is_constant {
            wb.set_cell_input(0, 1, 1, format!("={}", sig.name))
                .unwrap();
            assert_ne!(
                wb.cell_display(0, 1, 1),
                "#NAME?",
                "unrecognized {}",
                sig.name
            );
        } else {
            let probe = format!("={}(", sig.name);
            let assist =
                cells::formula::compute_formula_assist(&mut wb, 0, 1, 1, &probe, probe.len());
            assert!(
                assist.active_fn.is_some_and(|(s, _)| s.name == sig.name),
                "IronCalc did not recognize function signature for {}",
                sig.name
            );
        }
    }
    wb.set_cell_input(0, 1, 1, String::new()).unwrap();
    wb.set_cell_input(0, 1, 2, "25".into()).unwrap();
    wb.set_cell_input(0, 2, 2, "75".into()).unwrap();
    wb.add_or_update_defined_name("DiscountRate", "$B$1")
        .unwrap();
    wb.add_sheet().unwrap();
    wb.rename_active_sheet("Q2 Revenue").unwrap();
    wb.select_sheet(0).unwrap();

    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());
    let backend = TestBackend::new(120, 30);
    let mut term = Terminal::new(backend).unwrap();

    // 2. Typing `=` in D6 immediately exposes ALL 497 functions (+ Defined Names + Sheet refs),
    //    anchored right below cell D6 `(col 4, row 6)`.
    app.wb.select_cell(6, 4);
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE)),
    );
    assert_eq!(app.formula_assist.candidates.len(), 499); // 497 functions + 1 name + 1 sheet
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "SUM(")
    );
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "ZTEST(")
    );
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "DiscountRate")
    );
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "'Q2 Revenue'!")
    );

    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let col_d_x = app.layout.cols.iter().find(|c| c.col == 4).unwrap().x_start;
    let row_6_y = app.layout.rows.iter().find(|r| r.row == 6).unwrap().y;
    let popup = app
        .layout
        .formula_popup_rect
        .expect("expected IntelliSense popup");
    assert_eq!(popup.x, col_d_x);
    assert_eq!(popup.y, row_6_y + 1);
    let compact_items = app.layout.formula_assist_items.len();

    // Larger terminal (`200x55`) expands IntelliSense both horizontally and vertically,
    // and mouse hover after scrolling down keeps `completion_scroll` stable.
    let mut big_term = Terminal::new(TestBackend::new(200, 55)).unwrap();
    big_term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let big_popup = app.layout.formula_popup_rect.unwrap();
    assert!(big_popup.width > popup.width + 25);
    assert!(app.layout.formula_assist_items.len() >= compact_items + 10);
    for _ in 0..40 {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        );
    }
    big_term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let scrolled_top_idx = app.layout.formula_assist_items[0].0;
    assert!(scrolled_top_idx > 10);
    let second_row_rect = app.layout.formula_assist_items[1].1;
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: second_row_rect.x + 3,
            row: second_row_rect.y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    big_term.draw(|f| ui::draw(f, &mut app)).unwrap();
    assert_eq!(app.layout.formula_assist_items[0].0, scrolled_top_idx);

    // 3. Near bottom of viewport (row 24), IntelliSense flips cleanly ABOVE the edited cell.
    app.cancel_edit();
    app.wb.select_cell(24, 4);
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE)),
    );
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let row_24_y = app.layout.rows.iter().find(|r| r.row == 24).unwrap().y;
    let flipped = app.layout.formula_popup_rect.unwrap();
    assert!(flipped.bottom() <= row_24_y);

    // 4. Nested formula `=IF(B1>0, XLK`: keeps enclosing `IF` at 0-based arg 1 (`value_if_true`)
    //    AND matches `XLOOKUP(` via subsequence fuzzy search; `=STDEV.` matches dotted functions.
    app.cancel_edit();
    app.wb.select_cell(5, 3);
    for ch in "=IF(B1>0, XLK".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    let (active_sig, active_arg) = app.formula_assist.active_fn.expect("expected enclosing IF");
    assert_eq!(active_sig.name, "IF");
    assert_eq!(active_arg, 1); // 0-based index 1 == 2nd argument (`value_if_true`)
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "XLOOKUP(")
    );

    app.cancel_edit();
    for ch in "=STDEV.".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "STDEV.S(")
    );
    assert!(
        app.formula_assist
            .candidates
            .iter()
            .any(|c| c.insert_text == "STDEV.P(")
    );

    // 5. Mouse click on an IntelliSense candidate row inserts `AVERAGE(`, shifts signature
    //    above C5 so cells below stay uncovered, and Ctrl+Space reopens completions on demand.
    app.cancel_edit();
    app.wb.select_cell(5, 3);
    for ch in "=AVER".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let avg_idx = app
        .formula_assist
        .candidates
        .iter()
        .position(|c| c.insert_text == "AVERAGE(")
        .unwrap();
    let avg_rect = app
        .layout
        .formula_assist_items
        .iter()
        .find(|(i, _)| *i == avg_idx)
        .unwrap()
        .1;
    handle_event(
        &mut app,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: avg_rect.x + 4,
            row: avg_rect.y,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(app.edit.as_ref().unwrap().buf, "=AVERAGE(");
    assert_eq!(app.formula_assist.active_fn.unwrap().1, 0); // 1st argument (`number1`)
    assert!(app.formula_assist.candidates.is_empty());

    // Ctrl+Space inside `=AVERAGE(` re-opens completions on demand!
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
    );
    assert!(!app.formula_assist.candidates.is_empty());
}

#[test]
fn large_csv_fast_load_cached_render_and_autofilter() {
    use std::fmt::Write as _;
    use std::time::Instant;

    // Build a 5,000-row mixed CSV exercising every fast-path & fallback branch (> 2,000-row threshold).
    let mut csv_buf = String::with_capacity(350_000);
    csv_buf.push_str("id,date,sku,active,qty,price,note,extra\n");
    for i in 1..=5_000 {
        let active = if i % 2 == 0 { "true" } else { "false" };
        let extra = match i {
            1 => "\"$1,250.50\"",
            2 => "12.5%",
            3 => "=E4*F4",
            _ => "plain",
        };
        let _ = writeln!(
            csv_buf,
            "{i},2026-02-02,SKU-{i},{active},{},{:.2},Order batch #{i} verified,{extra}",
            i % 10,
            10.5 + (i % 5) as f64
        );
    }

    let wb =
        WorkbookState::from_delimited_reader(csv_buf.as_bytes(), b',', "large_sample").unwrap();
    assert_eq!(wb.sheet_bounds(0), (5_001, 8));
    assert_eq!(wb.cell_numeric(0, 2, 1), Some(1.0));
    assert_eq!(wb.cell_display(0, 2, 2), "2026-02-02");
    assert_eq!(wb.cell_display(0, 2, 3), "SKU-1");
    assert_eq!(wb.cell_display(0, 2, 4), "FALSE");
    assert_eq!(wb.cell_display(0, 3, 4), "TRUE");
    assert_eq!(wb.cell_display(0, 2, 7), "Order batch #1 verified");
    assert_eq!(wb.cell_numeric(0, 2, 8), Some(1250.5));
    assert!((wb.cell_numeric(0, 3, 8).unwrap() - 0.125).abs() < 1e-9);
    // Row 4 (`i == 3`): E4 = 3, F4 = 13.50 -> `=E4*F4` = 40.5
    assert!((wb.cell_numeric(0, 4, 8).unwrap() - 40.5).abs() < 1e-9);

    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());
    let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
    term.draw(|f| ui::draw(f, &mut app)).unwrap();

    // Ctrl+A selects all 5,001 × 8 cells; first frame caches stats, subsequent frames skip scanning.
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
    );
    assert_eq!(app.wb.selected_range(), [1, 1, 5_001, 8]);
    term.draw(|f| ui::draw(f, &mut app)).unwrap();
    let stats = app.cached_selection_stats();
    assert_eq!(stats.non_empty, 5_001 * 8);

    let t_cached = Instant::now();
    for _ in 0..5 {
        term.draw(|f| ui::draw(f, &mut app)).unwrap();
    }
    assert!(
        t_cached.elapsed().as_millis() < 250,
        "cached draw frames after Ctrl+A took too long: {:?}",
        t_cached.elapsed()
    );

    // Distinct values on unique SKU column caps cleanly at 1,000 entries.
    let distinct_skus = app.wb.distinct_column_values(0, 2, 5_001, 3);
    assert_eq!(distinct_skus.len(), 1_000);

    // Large-sheet AutoFilter (> 2,000 rows) uses O(1) `autofilter_hidden` without O(N²) `ws.rows` bloat.
    app.wb.select_cell(1, 1);
    assert!(app.wb.toggle_autofilter_on_selection().unwrap());
    let sheet_name = app.wb.active_sheet_name();
    {
        let af = app.wb.active_autofilter_mut(&sheet_name).unwrap();
        let mut cf = cells::workbook::ColumnFilter::default();
        cf.excluded_values.insert("FALSE".to_string());
        af.col_filters.insert(4, cf);
    }
    let hidden = app.wb.reapply_autofilter(0).unwrap();
    assert_eq!(hidden, 2_500);
    assert!(app.wb.is_row_hidden(0, 2));
    assert!(!app.wb.is_row_hidden(0, 3));
    assert!(!app.wb.toggle_autofilter_on_selection().unwrap());
    assert!(!app.wb.is_row_hidden(0, 2));
}

#[test]
fn large_sheet_formula_edit_preserves_bounds_defers_recovery_and_updates_autofilter() {
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    let mut csv_buf = String::with_capacity(220_000);
    csv_buf.push_str("id,active,qty,price,note\n");
    for i in 1..=5_000 {
        let active = if i % 2 == 0 { "true" } else { "false" };
        let _ = writeln!(csv_buf, "{i},{active},{},{:.2},row-{i}", i % 10, 5.0);
    }

    let wb = WorkbookState::from_delimited_reader(csv_buf.as_bytes(), b',', "fml_lag").unwrap();
    assert_eq!(wb.sheet_bounds(0), (5_001, 5));

    // Incremental single-cell AutoFilter update on a data-only >2,000-row sheet:
    let mut app = App::new(wb, None, Config::default(), Picker::halfblocks());
    app.wb.select_cell(1, 1);
    assert!(app.wb.toggle_autofilter_on_selection().unwrap());
    {
        let sheet_name = app.wb.active_sheet_name();
        let af = app.wb.active_autofilter_mut(&sheet_name).unwrap();
        let mut cf = cells::workbook::ColumnFilter::default();
        cf.excluded_values.insert("FALSE".to_string());
        af.col_filters.insert(2, cf);
    }
    assert_eq!(app.wb.reapply_autofilter(0).unwrap(), 2_500);
    assert!(app.wb.is_row_hidden(0, 2));

    // Edit B2 (`FALSE` -> `TRUE`) via `commit_edit` and verify O(1) row visibility update:
    app.wb.select_cell(2, 2);
    for ch in "TRUE".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(!app.wb.is_row_hidden(0, 2));
    assert!(!app.wb.toggle_autofilter_on_selection().unwrap());

    // Type and commit `=SUM(C2:C5001)` into F2 (expanding `max_col` 5 -> 6):
    app.last_recovery_write = Instant::now() - Duration::from_secs(90);
    let rev_before = app.last_recovery_rev;
    app.wb.select_cell(2, 6);
    for ch in "=SUM(C2:C5001)".chars() {
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
        );
    }
    // Recovery housekeeping must NOT fire mid-edit while `app.edit.is_some()`.
    app.tick_housekeeping();
    assert_eq!(app.last_recovery_rev, rev_before);

    handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    let expected_sum: f64 = (1..=5_000).map(|i| (i % 10) as f64).sum();
    assert_eq!(app.wb.cell_numeric(0, 2, 6), Some(expected_sum));
    assert_eq!(app.wb.sheet_bounds(0), (5_001, 6));

    // Immediately after `Enter`, `last_input_at` is fresh (< 2s), so `tick_housekeeping` defers
    // `UserModel::to_bytes()` rather than stalling the commit frame.
    app.tick_housekeeping();
    assert_eq!(app.last_recovery_rev, rev_before);

    // Clearing the edge cell F2 shrinks `sheet_bounds` back to `(5_001, 5)`.
    app.wb.set_cell_input(0, 2, 6, String::new()).unwrap();
    assert_eq!(app.wb.sheet_bounds(0), (5_001, 5));
}
