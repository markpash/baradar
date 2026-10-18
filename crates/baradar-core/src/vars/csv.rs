//! Pasted text / CSV → batch table (port of `web/src/doc/csv.ts`).
//!
//! RFC 4180 with the leniency spreadsheets need: quoted fields (`""` escapes a quote, newlines
//! inside quotes), CRLF / LF / CR line ends, a UTF-8 BOM, and `,` `;` or tab as the delimiter
//! (whichever occurs most often outside quotes in the header line). A header line with none of
//! them is a single column: the rows are then never split ("Smith, John" stays one value). The
//! header row gives the variable names, sanitised to `VARIABLE_NAME_RE`. Rows are padded /
//! truncated to the header and every limit (rows, columns, cell length, total size) applies, each
//! with a notice.

use std::collections::HashSet;

use unicode_normalization::UnicodeNormalization;

use super::variables::{
    BATCH_CELL_CHARS, BATCH_COLUMNS, BATCH_ROWS, BATCH_TOTAL_CHARS, is_js_space, is_variable_name,
    js_trim, utf16_len, utf16_prefix,
};

/// A parsed batch table. `ParsedTable`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedTable {
    /// Variable names (unique, valid `{{name}}`s).
    pub columns: Vec<String>,
    /// Cells by column index, every row exactly `columns.len()` long.
    pub rows: Vec<Vec<String>>,
    /// User-facing sentences about anything that was changed or dropped.
    pub notices: Vec<String>,
}

/// `,`, `;` or tab; `None` is one column (no delimiter; quotes and line breaks still apply).
/// `Delimiter`.
pub type Delimiter = Option<char>;

/// Tie-break order.
const DELIMITERS: [char; 3] = ['\t', ';', ','];

/// Records of `text` split on `delim` (quotes per RFC 4180). Empty lines are skipped.
/// `parseRecords`.
pub fn parse_records(text: &str, delim: Delimiter) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false; // inside a quoted section
    let mut touched = false; // the current record has content (or a delimiter)
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        if c == '"' && field.is_empty() {
            // A quote opens a quoted field only at its start; elsewhere it is a literal character.
            quoted = true;
            touched = true;
        } else if delim == Some(c) {
            record.push(std::mem::take(&mut field));
            touched = true;
        } else if c == '\r' || c == '\n' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            end_record(&mut records, &mut record, &mut field, &mut touched);
        } else {
            field.push(c);
            touched = true;
        }
    }
    end_record(&mut records, &mut record, &mut field, &mut touched);
    records
}

fn end_record(
    records: &mut Vec<Vec<String>>,
    record: &mut Vec<String>,
    field: &mut String,
    touched: &mut bool,
) {
    if *touched {
        record.push(std::mem::take(field));
        records.push(std::mem::take(record));
    }
    record.clear();
    field.clear();
    *touched = false;
}

/// The delimiter used most outside quotes in the first line (`None` when there is none: a single
/// column, e.g. one copied spreadsheet column whose values contain commas). `detectDelimiter`.
pub fn detect_delimiter(text: &str) -> Delimiter {
    let mut counts = [0usize; 3];
    let mut quoted = false;
    for c in text.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && (c == '\n' || c == '\r') {
            break;
        } else if !quoted && let Some(k) = DELIMITERS.iter().position(|d| *d == c) {
            counts[k] += 1;
        }
    }
    let mut best = None;
    let mut max = 0;
    for (k, d) in DELIMITERS.iter().enumerate() {
        if counts[k] > max {
            best = Some(*d);
            max = counts[k];
        }
    }
    best
}

/// A header cell as a variable name: accents dropped, other characters and runs of spaces → `_`
/// (none at the ends), at most 32 characters, never starting with a digit; '' when nothing is
/// left. `sanitizeName`.
pub fn sanitize_name(raw: &str) -> String {
    // "Serial No." → "Serial_No", not "Serial_No_": other characters become spaces first.
    let spaced: String = raw
        .nfkd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .flat_map(|c| {
            let keep = c.is_ascii_alphanumeric() || c == '_';
            // JavaScript replaces per UTF-16 unit; extra spaces collapse below anyway.
            std::iter::repeat_n(
                if keep { c } else { ' ' },
                if keep { 1 } else { c.len_utf16() },
            )
        })
        .collect();
    let mut s = js_trim(&spaced)
        .split(is_js_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s.truncate(32); // ASCII only by now
    s
}

/// `names` made valid and unique (`name`, `name_2`, …; empty → `col<N>`). `uniqueNames`.
pub fn unique_names<S: AsRef<str>>(names: &[S]) -> Vec<String> {
    let mut out = Vec::with_capacity(names.len());
    let mut taken: HashSet<String> = HashSet::new();
    for (j, raw) in names.iter().enumerate() {
        let mut base = sanitize_name(raw.as_ref());
        if base.is_empty() {
            base = format!("col{}", j + 1);
        }
        let mut name = base.clone();
        let mut k = 2;
        while taken.contains(&name) || !is_variable_name(&name) {
            let suffix = format!("_{k}");
            name = format!("{}{suffix}", &base[..base.len().min(32 - suffix.len())]);
            k += 1;
        }
        taken.insert(name.clone());
        out.push(name);
    }
    out
}

/// Pasted text or a CSV file → a batch table (header row = variable names). `parseTable`.
pub fn parse_table(text: &str) -> ParsedTable {
    let mut notices = Vec::new();
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut records = parse_records(body, detect_delimiter(body)).into_iter();
    let Some(mut header) = records.next() else {
        return ParsedTable {
            columns: Vec::new(),
            rows: Vec::new(),
            notices: vec![
                "No data was found. Paste a table whose first row names the columns.".into(),
            ],
        };
    };

    if header.len() > BATCH_COLUMNS {
        notices.push(format!("Only the first {BATCH_COLUMNS} columns were kept."));
        header.truncate(BATCH_COLUMNS);
    }
    let columns = unique_names(&header);
    let renamed = header
        .iter()
        .zip(&columns)
        .filter(|(h, c)| js_trim(h) != c.as_str())
        .count();
    if renamed > 0 {
        let what = if renamed == 1 {
            "A column name was".to_owned()
        } else {
            format!("{renamed} column names were")
        };
        notices.push(format!(
            "{what} adjusted so it can be used as {{{{name}}}}."
        ));
    }

    let mut data: Vec<Vec<String>> = records.collect();
    // Spreadsheets often export trailing rows of empty cells (",,,"): not labels.
    while data
        .last()
        .is_some_and(|r| r.iter().all(|c| js_trim(c).is_empty()))
    {
        data.pop();
    }
    if data.len() > BATCH_ROWS {
        notices.push(format!("Only the first {BATCH_ROWS} rows were kept."));
        data.truncate(BATCH_ROWS);
    }
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(data.len());
    let mut ragged = false;
    let mut long_cells = false;
    let mut chars = 0usize;
    for rec in data {
        let extra_empty =
            rec.len() > columns.len() && rec[columns.len()..].iter().all(|c| c.is_empty());
        if rec.len() != columns.len() && !extra_empty {
            ragged = true;
        }
        let row: Vec<String> = (0..columns.len())
            .map(|j| {
                let v = rec.get(j).map(String::as_str).unwrap_or("");
                if utf16_len(v) <= BATCH_CELL_CHARS {
                    return v.to_owned();
                }
                long_cells = true;
                utf16_prefix(v, BATCH_CELL_CHARS).to_owned()
            })
            .collect();
        chars += row.iter().map(|c| utf16_len(c)).sum::<usize>();
        if chars > BATCH_TOTAL_CHARS {
            notices.push(format!(
                "The data is too large; only the first {} rows were kept.",
                rows.len()
            ));
            break;
        }
        rows.push(row);
    }
    if ragged {
        notices.push(
            "Some rows had a different number of cells than the header; they were padded or cut to fit."
                .into(),
        );
    }
    if long_cells {
        notices.push(format!(
            "Cells longer than {BATCH_CELL_CHARS} characters were shortened."
        ));
    }
    ParsedTable {
        columns,
        rows,
        notices,
    }
}
