use crate::address::{col_to_letters, format_sheet_prefix, parse_range_ref};
use crate::workbook::{ChartKind, ChartSpec, META_SHEET_NAME};
use anyhow::{Context, Result};
use ironcalc_base::Model;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

/// Injects native OOXML DrawingML charts (`xl/charts/chartN.xml`, `xl/drawings/drawingM.xml`,
/// worksheet `<drawing r:id="..."/>`, `.rels`, and `[Content_Types].xml` overrides) into an
/// `.xlsx` archive so Excel and LibreOffice Calc render editable native charts on open.
pub fn inject_ooxml_charts(
    xlsx_bytes: Vec<u8>,
    model: &Model,
    charts: &[ChartSpec],
) -> Result<Vec<u8>> {
    if charts.is_empty() {
        return Ok(xlsx_bytes);
    }

    let mut sheet_by_name: HashMap<String, usize> = HashMap::new();
    for (idx, ws) in model.workbook.worksheets.iter().enumerate() {
        if ws.name != META_SHEET_NAME {
            sheet_by_name.insert(ws.name.to_ascii_lowercase(), idx);
        }
    }

    type SheetChartEntry<'a> = (&'a ChartSpec, [i32; 4], String);
    let mut per_sheet: HashMap<usize, Vec<SheetChartEntry<'_>>> = HashMap::new();
    for spec in charts {
        let Some((resolved_sheet, bounds)) = parse_range_ref(&spec.sheet_name, &spec.range) else {
            continue;
        };
        let Some(&s_idx) = sheet_by_name.get(&resolved_sheet.to_ascii_lowercase()) else {
            continue;
        };
        let canonical_sheet = model.workbook.worksheets[s_idx].name.clone();
        per_sheet
            .entry(s_idx)
            .or_default()
            .push((spec, bounds, canonical_sheet));
    }

    if per_sheet.is_empty() {
        return Ok(xlsx_bytes);
    }

    let mut sheet_indices: Vec<usize> = per_sheet.keys().copied().collect();
    sheet_indices.sort_unstable();

    let mut extra_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut sheet_drawing_num: HashMap<usize, usize> = HashMap::new();
    let mut ct_overrides = String::new();
    let mut next_chart_num = 1usize;

    for (d_idx, &s_idx) in sheet_indices.iter().enumerate() {
        let drawing_num = d_idx + 1;
        sheet_drawing_num.insert(s_idx, drawing_num);
        ct_overrides.push_str(&format!(
            r#"<Override PartName="/xl/drawings/drawing{drawing_num}.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/>"#
        ));

        let entries = &per_sheet[&s_idx];
        let mut drawing_anchors = String::new();
        let mut drawing_rels = String::new();

        for (local_i, (spec, bounds, src_sheet_name)) in entries.iter().enumerate() {
            let chart_num = next_chart_num;
            next_chart_num += 1;
            let rid = format!("rIdChart{}", local_i + 1);

            ct_overrides.push_str(&format!(
                r#"<Override PartName="/xl/charts/chart{chart_num}.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/>"#
            ));
            drawing_rels.push_str(&format!(
                r#"<Relationship Id="{rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart{chart_num}.xml"/>"#
            ));

            let from_col = bounds[3].max(1); // 0-based col right after table (`c2`)
            let from_row = (local_i * 16) as i32;
            let to_col = from_col + 9;
            let to_row = from_row + 15;
            let title_esc = xml_escape(&spec.title);
            drawing_anchors.push_str(&format!(
                r#"<xdr:twoCellAnchor><xdr:from><xdr:col>{from_col}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{from_row}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>{to_col}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{to_row}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{}" name="{title_esc}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="{rid}"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor>"#,
                local_i + 2,
            ));

            let chart_xml = build_chart_space_xml(spec, *bounds, src_sheet_name);
            extra_files.push((
                format!("xl/charts/chart{chart_num}.xml"),
                chart_xml.into_bytes(),
            ));
        }

        let drawing_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{drawing_anchors}</xdr:wsDr>"#
        );
        let drawing_rels_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{drawing_rels}</Relationships>"#
        );
        extra_files.push((
            format!("xl/drawings/drawing{drawing_num}.xml"),
            drawing_xml.into_bytes(),
        ));
        extra_files.push((
            format!("xl/drawings/_rels/drawing{drawing_num}.xml.rels"),
            drawing_rels_xml.into_bytes(),
        ));
    }

    let mut archive =
        ZipArchive::new(Cursor::new(xlsx_bytes)).context("failed reading xlsx zip")?;
    let mut out_cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut out_cursor);
    let opts = SimpleFileOptions::default();

    let mut existing_sheet_rels: HashMap<usize, bool> = HashMap::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name()?.into_owned();
        if entry.is_dir() {
            writer.add_directory(&name, opts)?;
            continue;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;

        if name == "[Content_Types].xml" {
            let s = String::from_utf8_lossy(&bytes);
            let patched = insert_before_tag(&s, "</Types>", &ct_overrides);
            writer.start_file(&name, opts)?;
            writer.write_all(patched.as_bytes())?;
            continue;
        }

        if let Some(s_idx) = parse_sheet_xml_index(&name)
            && sheet_drawing_num.contains_key(&s_idx)
        {
            let s = String::from_utf8_lossy(&bytes);
            let drawing_tag = r#"<drawing r:id="rIdCellsDrawing"/>"#;
            let patched = if s.contains("<extLst") {
                insert_before_tag(&s, "<extLst", drawing_tag)
            } else {
                insert_before_tag(&s, "</worksheet>", drawing_tag)
            };
            writer.start_file(&name, opts)?;
            writer.write_all(patched.as_bytes())?;
            continue;
        }

        if let Some(s_idx) = parse_sheet_rels_index(&name)
            && let Some(&d_num) = sheet_drawing_num.get(&s_idx)
        {
            existing_sheet_rels.insert(s_idx, true);
            let s = String::from_utf8_lossy(&bytes);
            let rel_tag = format!(
                r#"<Relationship Id="rIdCellsDrawing" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing{d_num}.xml"/>"#
            );
            let patched = insert_before_tag(&s, "</Relationships>", &rel_tag);
            writer.start_file(&name, opts)?;
            writer.write_all(patched.as_bytes())?;
            continue;
        }

        writer.start_file(&name, opts)?;
        writer.write_all(&bytes)?;
    }

    for (&s_idx, &d_num) in &sheet_drawing_num {
        if !existing_sheet_rels.contains_key(&s_idx) {
            let rels_name = format!("xl/worksheets/_rels/sheet{}.xml.rels", s_idx + 1);
            let rels_xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdCellsDrawing" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing{d_num}.xml"/></Relationships>"#
            );
            writer.start_file(&rels_name, opts)?;
            writer.write_all(rels_xml.as_bytes())?;
        }
    }

    for (path, content) in extra_files {
        writer.start_file(&path, opts)?;
        writer.write_all(&content)?;
    }

    writer.finish()?;
    Ok(out_cursor.into_inner())
}

fn build_chart_space_xml(spec: &ChartSpec, [r1, c1, r2, c2]: [i32; 4], sheet_name: &str) -> String {
    let pfx = format_sheet_prefix(sheet_name);
    let has_hdr = spec.has_header && r2 > r1;
    let data_r1 = if has_hdr { r1 + 1 } else { r1 };
    let multi_col = c2 > c1;
    let cat_formula = multi_col.then(|| abs_range(&pfx, data_r1, c1, r2, c1));
    let series_cols: Vec<i32> = if multi_col {
        ((c1 + 1)..=c2).collect()
    } else {
        vec![c1]
    };

    let build_ser = |idx: usize, col: i32, is_scatter: bool| -> String {
        let tx = if has_hdr {
            format!(
                "<c:tx><c:strRef><c:f>{}</c:f></c:strRef></c:tx>",
                abs_cell(&pfx, r1, col)
            )
        } else {
            format!("<c:tx><c:v>Series {}</c:v></c:tx>", idx + 1)
        };
        let val_f = abs_range(&pfx, data_r1, col, r2, col);
        if is_scatter {
            let x_f = cat_formula.clone().unwrap_or_else(|| val_f.clone());
            format!(
                "<c:ser><c:idx val=\"{idx}\"/><c:order val=\"{idx}\"/>{tx}<c:xVal><c:numRef><c:f>{x_f}</c:f></c:numRef></c:xVal><c:yVal><c:numRef><c:f>{val_f}</c:f></c:numRef></c:yVal></c:ser>"
            )
        } else {
            let cat_xml = cat_formula
                .as_ref()
                .map(|cf| format!("<c:cat><c:strRef><c:f>{cf}</c:f></c:strRef></c:cat>"))
                .unwrap_or_default();
            format!(
                "<c:ser><c:idx val=\"{idx}\"/><c:order val=\"{idx}\"/>{tx}{cat_xml}<c:val><c:numRef><c:f>{val_f}</c:f></c:numRef></c:val></c:ser>"
            )
        }
    };

    let plot_inner = match spec.kind {
        ChartKind::Pie => {
            let ser = build_ser(0, series_cols[0], false);
            format!("<c:pieChart><c:varyColors val=\"1\"/>{ser}</c:pieChart>")
        }
        ChartKind::Scatter => {
            let sers: String = series_cols
                .iter()
                .enumerate()
                .map(|(i, &c)| build_ser(i, c, true))
                .collect();
            format!(
                "<c:scatterChart><c:scatterStyle val=\"lineMarker\"/>{sers}<c:axId val=\"10\"/><c:axId val=\"20\"/></c:scatterChart>\
                 <c:valAx><c:axId val=\"10\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"20\"/></c:valAx>\
                 <c:valAx><c:axId val=\"20\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:crossAx val=\"10\"/></c:valAx>"
            )
        }
        ChartKind::DualAxis if series_cols.len() >= 2 => {
            let primary_ser = build_ser(0, series_cols[0], false);
            let secondary_sers: String = series_cols[1..]
                .iter()
                .enumerate()
                .map(|(i, &c)| build_ser(i + 1, c, false))
                .collect();
            format!(
                "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>{primary_ser}<c:axId val=\"10\"/><c:axId val=\"20\"/></c:barChart>\
                 <c:lineChart><c:grouping val=\"standard\"/>{secondary_sers}<c:axId val=\"30\"/><c:axId val=\"40\"/></c:lineChart>\
                 <c:catAx><c:axId val=\"10\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"20\"/></c:catAx>\
                 <c:valAx><c:axId val=\"20\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:crossAx val=\"10\"/></c:valAx>\
                 <c:catAx><c:axId val=\"30\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"1\"/><c:axPos val=\"b\"/><c:crossAx val=\"40\"/></c:catAx>\
                 <c:valAx><c:axId val=\"40\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"r\"/><c:crossAx val=\"30\"/><c:crosses val=\"max\"/></c:valAx>"
            )
        }
        other => {
            let sers: String = series_cols
                .iter()
                .enumerate()
                .map(|(i, &c)| build_ser(i, c, false))
                .collect();
            let chart_elem = match other {
                ChartKind::Line => format!(
                    "<c:lineChart><c:grouping val=\"standard\"/>{sers}<c:axId val=\"10\"/><c:axId val=\"20\"/></c:lineChart>"
                ),
                ChartKind::Area => format!(
                    "<c:areaChart><c:grouping val=\"standard\"/>{sers}<c:axId val=\"10\"/><c:axId val=\"20\"/></c:areaChart>"
                ),
                ChartKind::StackedBar => format!(
                    "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"stacked\"/><c:overlap val=\"100\"/>{sers}<c:axId val=\"10\"/><c:axId val=\"20\"/></c:barChart>"
                ),
                _ => format!(
                    "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>{sers}<c:axId val=\"10\"/><c:axId val=\"20\"/></c:barChart>"
                ),
            };
            format!(
                "{chart_elem}\
                 <c:catAx><c:axId val=\"10\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"20\"/></c:catAx>\
                 <c:valAx><c:axId val=\"20\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:crossAx val=\"10\"/></c:valAx>"
            )
        }
    };

    let title_xml = if spec.title.is_empty() {
        "<c:autoTitleDeleted val=\"1\"/>".to_string()
    } else {
        format!(
            "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title>",
            xml_escape(&spec.title)
        )
    };

    let legend_xml = if spec.show_legend {
        "<c:legend><c:legendPos val=\"r\"/><c:overlay val=\"0\"/></c:legend>"
    } else {
        ""
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart>{title_xml}<c:plotArea><c:layout/>{plot_inner}</c:plotArea>{legend_xml}<c:plotVisOnly val="1"/></c:chart></c:chartSpace>"#
    )
}

fn abs_cell(pfx: &str, r: i32, c: i32) -> String {
    format!("{pfx}${}${r}", col_to_letters(c))
}

fn abs_range(pfx: &str, r1: i32, c1: i32, r2: i32, c2: i32) -> String {
    format!(
        "{pfx}${}${r1}:${}${r2}",
        col_to_letters(c1),
        col_to_letters(c2)
    )
}

fn parse_sheet_xml_index(name: &str) -> Option<usize> {
    let num = name
        .strip_prefix("xl/worksheets/sheet")?
        .strip_suffix(".xml")?;
    num.parse::<usize>().ok()?.checked_sub(1)
}

fn parse_sheet_rels_index(name: &str) -> Option<usize> {
    let num = name
        .strip_prefix("xl/worksheets/_rels/sheet")?
        .strip_suffix(".xml.rels")?;
    num.parse::<usize>().ok()?.checked_sub(1)
}

fn insert_before_tag(haystack: &str, tag: &str, snippet: &str) -> String {
    if let Some(pos) = haystack.rfind(tag) {
        let mut out = String::with_capacity(haystack.len() + snippet.len());
        out.push_str(&haystack[..pos]);
        out.push_str(snippet);
        out.push_str(&haystack[pos..]);
        out
    } else {
        format!("{haystack}{snippet}")
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}
