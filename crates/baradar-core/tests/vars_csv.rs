//! Port of web/tests/unit/doc/csv.test.ts.

use baradar_core::vars::csv::*;
use baradar_core::vars::variables::{
    BATCH_CELL_CHARS, BATCH_COLUMNS, BATCH_ROWS, BATCH_TOTAL_CHARS, is_variable_name,
};

fn rec(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect()
}

#[test]
fn parse_records_rfc_4180() {
    let cases: &[(&str, &str, &[&[&str]])] = &[
        ("plain", "a,b\n1,2", &[&["a", "b"], &["1", "2"]]),
        (
            "quoted fields with delimiters",
            "\"a,b\",c\n\"1\",2",
            &[&["a,b", "c"], &["1", "2"]],
        ),
        (
            "\"\" escapes a quote",
            "\"say \"\"hi\"\"\",x",
            &[&["say \"hi\"", "x"]],
        ),
        (
            "newline inside quotes",
            "\"line 1\nline 2\",x\ny,z",
            &[&["line 1\nline 2", "x"], &["y", "z"]],
        ),
        ("CRLF", "a,b\r\n1,2\r\n", &[&["a", "b"], &["1", "2"]]),
        ("bare CR", "a,b\r1,2\r", &[&["a", "b"], &["1", "2"]]),
        (
            "empty fields",
            "a,,c\n,,",
            &[&["a", "", "c"], &["", "", ""]],
        ),
        ("empty lines are skipped", "a\n\n\nb\n\n", &[&["a"], &["b"]]),
        (
            "a quote inside a field is literal",
            "O\"Neil,5\"",
            &[&["O\"Neil", "5\""]],
        ),
        (
            "text after a closing quote is kept",
            "\"ab\"c,d",
            &[&["abc", "d"]],
        ),
        (
            "unterminated quote runs to the end",
            "\"abc\ndef",
            &[&["abc\ndef"]],
        ),
    ];
    for (name, text, out) in cases {
        assert_eq!(parse_records(text, Some(',')), rec(out), "{name}");
    }
}

#[test]
fn no_delimiter_is_one_column() {
    assert_eq!(
        parse_records("Name\nSmith, John\n\"Doe; \"\"Jane\"\"\nJr\"", None),
        rec(&[&["Name"], &["Smith, John"], &["Doe; \"Jane\"\nJr"]])
    );
}

#[test]
fn splits_on_semicolon_and_tab() {
    assert_eq!(
        parse_records("a;b\n1;2", Some(';')),
        rec(&[&["a", "b"], &["1", "2"]])
    );
    assert_eq!(
        parse_records("a\tb\n1\t\"x\ty\"", Some('\t')),
        rec(&[&["a", "b"], &["1", "x\ty"]])
    );
}

#[test]
fn detects_the_delimiter() {
    let cases: &[(&str, Option<char>)] = &[
        ("name,room\nAda,1", Some(',')),
        ("name;room;floor\nAda;1;2", Some(';')),
        ("name\troom\nAda\t1", Some('\t')),
        ("\"a;b\",c\n1;2;3", Some(',')),
        ("single", None),
        ("\"a,b\"\nx,y", None),
        ("a;b,c;d", Some(';')),
        ("a,b;c\td", Some('\t')),
    ];
    for (text, d) in cases {
        assert_eq!(detect_delimiter(text), *d, "{text:?}");
    }
}

#[test]
fn header_names() {
    let x40 = "x".repeat(40);
    let x32 = "x".repeat(32);
    let cases: &[(&str, &str)] = &[
        ("name", "name"),
        ("  first name ", "first_name"),
        ("Serial No.", "Serial_No"),
        ("Café", "Cafe"),
        ("2nd", "_2nd"),
        ("_id", "_id"),
        ("a-b/c", "a_b_c"),
        ("!!!", ""),
        (&x40, &x32),
    ];
    for (raw, name) in cases {
        assert_eq!(sanitize_name(raw), *name, "{raw:?}");
    }
}

#[test]
fn makes_names_valid_and_unique() {
    let x40 = "x".repeat(40);
    let x32 = "x".repeat(32);
    let names = unique_names(&["name", "Name", "name", "", "!!", &x40, &x32]);
    let expected = vec![
        "name".to_string(),
        "Name".into(),
        "name_2".into(),
        "col4".into(),
        "col5".into(),
        x32.clone(),
        format!("{}_2", "x".repeat(30)),
    ];
    assert_eq!(names, expected);
    for n in &names {
        assert!(is_variable_name(n), "{n}");
    }
}

#[test]
fn header_row_names_the_columns_rows_padded_or_cut() {
    let t = parse_table("name,room\nAda,Lab 1\nGrace\nLinus,Lab 3,extra");
    assert_eq!(t.columns, ["name", "room"]);
    assert_eq!(
        t.rows,
        rec(&[&["Ada", "Lab 1"], &["Grace", ""], &["Linus", "Lab 3"]])
    );
    assert!(t.notices.join(" ").contains("different number of cells"));
}

#[test]
fn a_single_pasted_column_keeps_values_with_commas_whole() {
    assert_eq!(
        parse_table("Name\nSmith, John\nDoe, Jane"),
        ParsedTable {
            columns: vec!["Name".into()],
            rows: rec(&[&["Smith, John"], &["Doe, Jane"]]),
            notices: vec![],
        }
    );
    assert_eq!(
        parse_table("Address\n12 Main St, Springfield\n").rows,
        rec(&[&["12 Main St, Springfield"]])
    );
}

#[test]
fn strips_bom_and_trailing_empty_rows_semicolons_from_excel() {
    let t = parse_table("\u{feff}Name;Qty\r\nBolts;10\r\n;\r\n;\r\n");
    assert_eq!(t.columns, ["Name", "Qty"]);
    assert_eq!(t.rows, rec(&[&["Bolts", "10"]]));
    assert!(t.notices.is_empty());
}

#[test]
fn tab_separated_cells_from_a_spreadsheet() {
    let t = parse_table("id\tlocation\nA-1\tShelf \"B\"\n");
    assert_eq!(t.columns, ["id", "location"]);
    assert_eq!(t.rows, rec(&[&["A-1", "Shelf \"B\""]]));
}

#[test]
fn sanitises_and_deduplicates_the_header_with_a_notice() {
    let t = parse_table("first name,first name,\n1,2,3");
    assert_eq!(t.columns, ["first_name", "first_name_2", "col3"]);
    assert!(t.notices.join(" ").contains("3 column names were adjusted"));
    let one = parse_table("first name\nx");
    assert_eq!(
        one.notices,
        ["A column name was adjusted so it can be used as {{name}}."]
    );
}

#[test]
fn caps_rows() {
    let mut lines = vec!["n".to_string()];
    lines.extend((0..600).map(|i| i.to_string()));
    let t = parse_table(&lines.join("\n"));
    assert_eq!(t.rows.len(), BATCH_ROWS);
    assert_eq!(t.rows.last(), Some(&vec!["499".to_string()]));
    assert!(
        t.notices
            .contains(&"Only the first 500 rows were kept.".to_string())
    );
}

#[test]
fn caps_columns_cell_length_and_total_size() {
    let header = (0..25)
        .map(|j| format!("c{j}"))
        .collect::<Vec<_>>()
        .join(",");
    let cols = parse_table(&format!("{header}\n{}x", "x,".repeat(24)));
    assert_eq!(cols.columns.len(), BATCH_COLUMNS);
    assert_eq!(cols.rows[0].len(), BATCH_COLUMNS);
    assert!(cols.notices.contains(&format!(
        "Only the first {BATCH_COLUMNS} columns were kept."
    )));

    let long = parse_table(&format!("a\n{}", "y".repeat(BATCH_CELL_CHARS + 10)));
    assert_eq!(long.rows[0][0].len(), BATCH_CELL_CHARS);
    assert!(long.notices.join(" ").contains("shortened"));

    let row = vec!["z".repeat(BATCH_CELL_CHARS); 20].join(",");
    let head20 = header.split(',').take(20).collect::<Vec<_>>().join(",");
    let mut lines = vec![head20];
    lines.extend(std::iter::repeat_n(row, 30));
    let big = parse_table(&lines.join("\n"));
    assert_eq!(big.rows.len(), BATCH_TOTAL_CHARS / (20 * BATCH_CELL_CHARS));
    assert!(big.notices.join(" ").contains("too large"));
}

#[test]
fn cell_length_counts_utf16_units() {
    // 😀 is two UTF-16 units: 250 of them fill a 500-unit cell.
    let t = parse_table(&format!("a\n{}", "😀".repeat(260)));
    assert_eq!(t.rows[0][0].chars().count(), 250);
}

#[test]
fn header_only_gives_columns_without_rows_nothing_gives_a_notice() {
    assert_eq!(
        parse_table("a,b\n"),
        ParsedTable {
            columns: vec!["a".into(), "b".into()],
            rows: vec![],
            notices: vec![],
        }
    );
    let none = parse_table("\n\n");
    assert!(none.columns.is_empty());
    assert!(none.notices[0].contains("No data"));
}
