pub const MAX_COLS: i32 = 16_384;
pub const MAX_ROWS: i32 = 1_048_576;

pub fn col_to_letters(mut col: i32) -> String {
    if col <= 0 {
        return "A".to_string();
    }
    let mut chars = Vec::new();
    while col > 0 {
        let rem = ((col - 1) % 26) as u8;
        chars.push((b'A' + rem) as char);
        col = (col - 1) / 26;
    }
    chars.iter().rev().collect()
}

pub fn letters_to_col(s: &str) -> Option<i32> {
    if s.is_empty() {
        return None;
    }
    let mut col: i32 = 0;
    for b in s.bytes() {
        let upper = b.to_ascii_uppercase();
        if !upper.is_ascii_uppercase() {
            return None;
        }
        col = col
            .checked_mul(26)?
            .checked_add((upper - b'A' + 1) as i32)?;
    }
    (1..=MAX_COLS).contains(&col).then_some(col)
}

pub fn format_cell_ref(row: i32, col: i32) -> String {
    format!("{}{}", col_to_letters(col), row.max(1))
}

pub fn parse_cell_ref(s: &str) -> Option<(i32, i32)> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cleaned: String = trimmed.chars().filter(|&c| c != '$').collect();
    let split_idx = cleaned.find(|c: char| c.is_ascii_digit())?;
    if split_idx == 0 {
        return None;
    }
    let (col_part, row_part) = cleaned.split_at(split_idx);
    let col = letters_to_col(col_part)?;
    let row: i32 = row_part.parse().ok()?;
    (1..=MAX_ROWS).contains(&row).then_some((row, col))
}

pub fn format_sheet_prefix(sheet_name: &str) -> String {
    let needs_quotes = sheet_name.is_empty()
        || sheet_name
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && c != '_');
    if needs_quotes {
        format!("'{}'!", sheet_name.replace('\'', "''"))
    } else {
        format!("{sheet_name}!")
    }
}

pub fn format_range_ref(sheet_prefix: Option<&str>, r1: i32, c1: i32, r2: i32, c2: i32) -> String {
    let (sr, er) = (r1.min(r2), r1.max(r2));
    let (sc, ec) = (c1.min(c2), c1.max(c2));
    let prefix = sheet_prefix.map(format_sheet_prefix).unwrap_or_default();
    if sr == er && sc == ec {
        format!("{prefix}{}", format_cell_ref(sr, sc))
    } else {
        format!(
            "{prefix}{}:{}",
            format_cell_ref(sr, sc),
            format_cell_ref(er, ec)
        )
    }
}

pub fn parse_range_ref(default_sheet: &str, input: &str) -> Option<(String, [i32; 4])> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (sheet_name, range_part) = if let Some((raw_sheet, rest)) = split_sheet_bang(trimmed) {
        (unquote_sheet_name(raw_sheet), rest)
    } else {
        (default_sheet.to_string(), trimmed)
    };

    let (r1, c1, r2, c2) = if let Some((a, b)) = range_part.split_once(':') {
        let (r1, c1) = parse_cell_ref(a)?;
        let (r2, c2) = parse_cell_ref(b)?;
        (r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2))
    } else {
        let (r, c) = parse_cell_ref(range_part)?;
        (r, c, r, c)
    };
    Some((sheet_name, [r1, c1, r2, c2]))
}

fn split_sheet_bang(s: &str) -> Option<(&str, &str)> {
    let mut in_quotes = false;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                if in_quotes && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_quotes = !in_quotes;
            }
            b'!' if !in_quotes => {
                return Some((&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn unquote_sheet_name(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        t[1..t.len() - 1].replace("''", "'")
    } else {
        t.to_string()
    }
}

/// Returns true when clicking/dragging grid cells should insert a cell/range reference
/// at `cursor_byte` inside an active formula edit buffer.
pub fn can_insert_formula_ref(buf: &str, cursor_byte: usize) -> bool {
    if !buf.starts_with('=') || cursor_byte == 0 || cursor_byte > buf.len() {
        return false;
    }
    let before = buf[..cursor_byte].trim_end_matches(' ');
    matches!(
        before.as_bytes().last().copied(),
        Some(
            b'=' | b'('
                | b','
                | b';'
                | b'+'
                | b'-'
                | b'*'
                | b'/'
                | b'^'
                | b'&'
                | b':'
                | b'<'
                | b'>'
        )
    )
}

pub fn apply_formula_ref(
    buf: &mut String,
    cursor_byte: &mut usize,
    active_span: &mut Option<(usize, usize)>,
    ref_text: &str,
) -> bool {
    if let Some((start, end)) = *active_span
        && start <= end
        && end <= buf.len()
    {
        buf.replace_range(start..end, ref_text);
        let new_end = start + ref_text.len();
        *cursor_byte = new_end;
        *active_span = Some((start, new_end));
        return true;
    }

    if !can_insert_formula_ref(buf, *cursor_byte) {
        return false;
    }
    let start = *cursor_byte;
    buf.insert_str(start, ref_text);
    let end = start + ref_text.len();
    *cursor_byte = end;
    *active_span = Some((start, end));
    true
}
