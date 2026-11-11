pub use crate::formula_catalog::FUNCTION_CATALOG;
use crate::workbook::WorkbookState;
use ironcalc_base::expressions::lexer::util::get_tokens;
use ironcalc_base::expressions::parser::ExpectedTokens;
use ironcalc_base::expressions::token::TokenType;

#[derive(Debug, Clone, Copy)]
pub struct FunctionSig {
    pub name: &'static str,
    pub args: &'static [&'static str],
    pub summary: &'static str,
    pub is_constant: bool,
}

impl FunctionSig {
    pub(crate) const fn func(
        name: &'static str,
        args: &'static [&'static str],
        summary: &'static str,
    ) -> Self {
        Self {
            name,
            args,
            summary,
            is_constant: false,
        }
    }

    pub(crate) const fn constant(name: &'static str, summary: &'static str) -> Self {
        Self {
            name,
            args: &[],
            summary,
            is_constant: true,
        }
    }
}

const STARTER_FUNCTIONS: &[&str] = &[
    "SUM", "AVERAGE", "IF", "XLOOKUP", "VLOOKUP", "COUNT", "COUNTA", "COUNTIF", "SUMIFS", "INDEX",
    "MATCH", "IFERROR", "MIN", "MAX", "MEDIAN", "ROUND", "CONCAT", "TODAY",
];

pub fn lookup_function_sig(name: &str) -> Option<&'static FunctionSig> {
    let upper = name.to_ascii_uppercase();
    FUNCTION_CATALOG
        .binary_search_by(|f| f.name.cmp(upper.as_str()))
        .ok()
        .map(|i| &FUNCTION_CATALOG[i])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Function,
    Constant,
    DefinedName,
    SheetRef,
}

impl CompletionKind {
    pub fn icon(self) -> &'static str {
        match self {
            Self::Function => "ƒ ",
            Self::Constant => "◈ ",
            Self::DefinedName => "⌗ ",
            Self::SheetRef => "⊞ ",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub kind: CompletionKind,
    pub label: String,
    pub insert_text: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct FormulaAssist {
    pub replace_char_start: usize,
    pub replace_char_end: usize,
    pub candidates: Vec<CompletionCandidate>,
    /// `(signature, 0-based active argument index)`
    pub active_fn: Option<(&'static FunctionSig, usize)>,
    /// `(function_name, 0-based active argument index)` when outside catalog
    pub fallback_fn: Option<(String, usize)>,
}

pub fn compute_formula_assist(
    wb: &mut WorkbookState,
    sheet: u32,
    row: i32,
    col: i32,
    formula: &str,
    cursor_byte: usize,
) -> FormulaAssist {
    compute_formula_assist_with_options(wb, sheet, row, col, formula, cursor_byte, false)
}

pub fn compute_formula_assist_with_options(
    wb: &mut WorkbookState,
    sheet: u32,
    row: i32,
    col: i32,
    formula: &str,
    cursor_byte: usize,
    force_open: bool,
) -> FormulaAssist {
    if !formula.starts_with('=') {
        return FormulaAssist::default();
    }
    let clamped_byte = cursor_byte.min(formula.len());
    let char_cursor = formula[..clamped_byte].chars().count();
    let head_body = &formula[1..clamped_byte.max(1)];
    if in_open_string_literal(head_body) {
        return FormulaAssist::default();
    }

    let Ok(ctx) = wb
        .user
        .formula_completion(sheet, row, col, formula, char_cursor)
    else {
        return FormulaAssist::default();
    };

    let mut active_fn = None;
    let mut fallback_fn = None;
    let mut fn_prefix: Option<String> = None;
    let mut replace_char_start = ctx.replace_from + 1; // +1 for leading '='
    let mut expects_range = false;

    for exp in &ctx.expecting {
        match exp {
            ExpectedTokens::Argument(name, arg_idx_1based) => {
                let zero_idx = (*arg_idx_1based as usize).saturating_sub(1);
                if let Some(sig) = lookup_function_sig(name) {
                    active_fn = Some((sig, zero_idx));
                } else {
                    fallback_fn = Some((name.to_ascii_uppercase(), zero_idx));
                }
            }
            ExpectedTokens::FunctionName(pfx) if !pfx.is_empty() => {
                fn_prefix = Some(pfx.to_ascii_uppercase());
            }
            ExpectedTokens::Range => {
                expects_range = true;
            }
            _ => {}
        }
    }

    // IronCalc short-circuits `trailing_name` and omits `Argument` when typing a nested
    // function name (`=IF(A1>0, VLOOK`); recover the enclosing call via lexer stack.
    if active_fn.is_none()
        && fallback_fn.is_none()
        && let Some((enc_name, zero_idx)) = enclosing_call_before_cursor(head_body)
    {
        if let Some(sig) = lookup_function_sig(&enc_name) {
            active_fn = Some((sig, zero_idx));
        } else {
            fallback_fn = Some((enc_name, zero_idx));
        }
    }

    // Fallback for dotted function prefixes like `=STDEV.` where IronCalc's parser stops at `.`.
    if fn_prefix.is_none()
        && let Some((dot_pfx, start_body_chars)) = trailing_dotted_identifier(head_body)
    {
        fn_prefix = Some(dot_pfx.to_ascii_uppercase());
        replace_char_start = 1 + start_body_chars;
    }

    let mut candidates = Vec::new();

    if let Some(ref pfx) = fn_prefix {
        let mut ranked: Vec<(u8, usize, CompletionCandidate)> = Vec::new();

        for sig in FUNCTION_CATALOG {
            if let Some(tier) = match_tier(sig.name, pfx) {
                ranked.push((tier, sig.name.len(), candidate_from_sig(sig)));
            }
        }

        for (name, _scope, expr) in wb.user.get_defined_name_list() {
            let up = name.to_ascii_uppercase();
            if let Some(tier) = match_tier(&up, pfx) {
                ranked.push((
                    tier,
                    name.len(),
                    CompletionCandidate {
                        kind: CompletionKind::DefinedName,
                        label: name.clone(),
                        insert_text: name,
                        detail: format!("Named range → {expr}"),
                    },
                ));
            }
        }

        let sheets = wb.visible_sheets();
        if sheets.len() > 1 {
            for (s_idx, s_name) in sheets {
                if s_idx == sheet {
                    continue;
                }
                let up = s_name.to_ascii_uppercase();
                if let Some(tier) = match_tier(&up, pfx) {
                    let formatted = format_sheet_prefix(&s_name);
                    ranked.push((
                        tier,
                        s_name.len(),
                        CompletionCandidate {
                            kind: CompletionKind::SheetRef,
                            label: formatted.clone(),
                            insert_text: formatted,
                            detail: format!("Cross-sheet reference ({s_name})"),
                        },
                    ));
                }
            }
        }

        ranked.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.label.cmp(&b.2.label))
        });
        candidates = ranked.into_iter().map(|(_, _, c)| c).collect();
    } else if head_body.trim().is_empty() || (force_open && expects_range) {
        replace_char_start = char_cursor;
        for (name, _scope, expr) in wb.user.get_defined_name_list() {
            candidates.push(CompletionCandidate {
                kind: CompletionKind::DefinedName,
                label: name.clone(),
                insert_text: name,
                detail: format!("Named range → {expr}"),
            });
        }
        let sheets = wb.visible_sheets();
        if sheets.len() > 1 {
            for (s_idx, s_name) in sheets {
                if s_idx == sheet {
                    continue;
                }
                let formatted = format_sheet_prefix(&s_name);
                candidates.push(CompletionCandidate {
                    kind: CompletionKind::SheetRef,
                    label: formatted.clone(),
                    insert_text: formatted,
                    detail: format!("Cross-sheet reference ({s_name})"),
                });
            }
        }
        for &fn_name in STARTER_FUNCTIONS {
            if let Some(sig) = lookup_function_sig(fn_name) {
                candidates.push(candidate_from_sig(sig));
            }
        }
        for sig in FUNCTION_CATALOG {
            if !STARTER_FUNCTIONS.contains(&sig.name) {
                candidates.push(candidate_from_sig(sig));
            }
        }
    }

    FormulaAssist {
        replace_char_start,
        replace_char_end: char_cursor,
        candidates,
        active_fn,
        fallback_fn,
    }
}

fn candidate_from_sig(sig: &'static FunctionSig) -> CompletionCandidate {
    if sig.is_constant {
        CompletionCandidate {
            kind: CompletionKind::Constant,
            label: sig.name.to_string(),
            insert_text: sig.name.to_string(),
            detail: sig.summary.to_string(),
        }
    } else if sig.args.is_empty() {
        CompletionCandidate {
            kind: CompletionKind::Function,
            label: format!("{}()", sig.name),
            insert_text: format!("{}()", sig.name),
            detail: sig.summary.to_string(),
        }
    } else {
        CompletionCandidate {
            kind: CompletionKind::Function,
            label: format!("{}({})", sig.name, sig.args.join(", ")),
            insert_text: format!("{}(", sig.name),
            detail: sig.summary.to_string(),
        }
    }
}

fn format_sheet_prefix(name: &str) -> String {
    let needs_quotes = name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if needs_quotes {
        format!("'{}'!", name.replace('\'', "''"))
    } else {
        format!("{name}!")
    }
}

fn match_tier(target_upper: &str, query_upper: &str) -> Option<u8> {
    if target_upper.starts_with(query_upper) {
        return Some(0);
    }
    if target_upper.contains(query_upper) {
        return Some(1);
    }
    if query_upper.len() >= 2 {
        let mut chars = target_upper.chars();
        if query_upper.chars().all(|qc| chars.any(|tc| tc == qc)) {
            return Some(2);
        }
    }
    None
}

fn in_open_string_literal(body: &str) -> bool {
    let mut open = false;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if open && chars.peek() == Some(&'"') {
                chars.next();
            } else {
                open = !open;
            }
        }
    }
    open
}

fn trailing_dotted_identifier(body: &str) -> Option<(&str, usize)> {
    let mut split_byte = body.len();
    for (idx, ch) in body.char_indices().rev() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            split_byte = idx;
        } else {
            break;
        }
    }
    let ident = &body[split_byte..];
    if !ident.contains('.')
        || !ident
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    let prev_ok = body[..split_byte]
        .chars()
        .next_back()
        .map(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '(' | ',' | ';' | '+' | '-' | '*' | '/' | '^' | '&' | '=' | '<' | '>'
                )
        })
        .unwrap_or(true);
    if !prev_ok {
        return None;
    }
    let start_chars = body[..split_byte].chars().count();
    Some((ident, start_chars))
}

fn enclosing_call_before_cursor(body: &str) -> Option<(String, usize)> {
    let tokens = get_tokens(body);
    let mut stack: Vec<(Option<String>, usize)> = Vec::new();
    let mut idx = 0;
    while idx < tokens.len() {
        match &tokens[idx].token {
            TokenType::Ident(name)
                if idx + 1 < tokens.len()
                    && matches!(tokens[idx + 1].token, TokenType::LeftParenthesis) =>
            {
                let clean = name
                    .trim_start_matches("_xlfn._xlws.")
                    .trim_start_matches("_xlfn.")
                    .trim_start_matches("_xlpm.")
                    .to_ascii_uppercase();
                stack.push((Some(clean), 0));
                idx += 2;
                continue;
            }
            TokenType::LeftParenthesis => {
                stack.push((None, 0));
            }
            TokenType::RightParenthesis => {
                stack.pop();
            }
            TokenType::Comma => {
                if let Some((_, arg_i)) = stack.last_mut() {
                    *arg_i += 1;
                }
            }
            _ => {}
        }
        idx += 1;
    }
    stack
        .into_iter()
        .rev()
        .find_map(|(fn_name, arg_i)| fn_name.map(|n| (n, arg_i)))
}

/// Applies the selected completion candidate into `buf` and updates `cursor_byte`.
pub fn accept_completion(
    buf: &mut String,
    cursor_byte: &mut usize,
    assist: &FormulaAssist,
    idx: usize,
) -> bool {
    let Some(cand) = assist.candidates.get(idx) else {
        return false;
    };
    let start_b = char_to_byte(buf, assist.replace_char_start);
    let end_b = char_to_byte(buf, assist.replace_char_end).max(start_b);
    buf.replace_range(start_b..end_b, &cand.insert_text);
    *cursor_byte = start_b + cand.insert_text.len();
    true
}

/// Cycles `A1 -> $A$1 -> A$1 -> $A1 -> A1` at `cursor_byte` using IronCalc's lexer.
pub fn cycle_formula_ref_at_cursor(
    wb: &WorkbookState,
    buf: &mut String,
    cursor_byte: &mut usize,
) -> bool {
    if !buf.starts_with('=') {
        return false;
    }
    let clamped = (*cursor_byte).min(buf.len());
    let ch_pos = buf[..clamped].chars().count();
    let Ok((updated, _new_start, new_end)) = wb.user.cycle_reference(buf, ch_pos, ch_pos) else {
        return false;
    };
    if updated == *buf {
        return false;
    }
    *buf = updated;
    *cursor_byte = char_to_byte(buf, new_end.max(0) as usize);
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaPrecedent {
    pub sheet_name: Option<String>,
    pub r1: i32,
    pub c1: i32,
    pub r2: i32,
    pub c2: i32,
    pub byte_start: usize,
    pub byte_end: usize,
    pub color_idx: usize,
}

/// Extracts cell/range precedents from an in-progress formula (`=SUM(A1:B4, C2)`) with
/// exact byte spans in `formula` and matching color indices for grid & formula-bar tinting.
pub fn extract_precedents(formula: &str) -> Vec<FormulaPrecedent> {
    let Some(body) = formula.strip_prefix('=') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, mt) in get_tokens(body).into_iter().enumerate() {
        let ch_s = (mt.start.max(0) as usize) + 1;
        let ch_e = (mt.end.max(0) as usize) + 1;
        let byte_start = char_to_byte(formula, ch_s);
        let byte_end = char_to_byte(formula, ch_e);
        match mt.token {
            TokenType::Reference {
                sheet, row, column, ..
            } => {
                out.push(FormulaPrecedent {
                    sheet_name: sheet,
                    r1: row,
                    c1: column,
                    r2: row,
                    c2: column,
                    byte_start,
                    byte_end,
                    color_idx: out.len(),
                });
            }
            TokenType::Range { sheet, left, right } => {
                out.push(FormulaPrecedent {
                    sheet_name: sheet,
                    r1: left.row.min(right.row),
                    c1: left.column.min(right.column),
                    r2: left.row.max(right.row),
                    c2: left.column.max(right.column),
                    byte_start,
                    byte_end,
                    color_idx: out.len(),
                });
            }
            _ => {
                let _ = idx;
            }
        }
    }
    out
}

pub fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}
