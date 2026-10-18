//! Port of web/tests/unit/doc/variables.test.ts.

use std::borrow::Cow;

use baradar_core::doc::schema::{
    BatchCounter, BatchData, CodeContent, CodeItem, DateFormat, Item, ItemKind, LabelDoc,
    Symbology, WifiSettings,
};
use baradar_core::vars::variables::*;
use time::macros::date;

fn text(t: &str) -> Item {
    let mut i = Item::new(ItemKind::Text);
    if let Item::Text(x) = &mut i {
        x.text = t.into();
    }
    i
}

fn code(f: impl FnOnce(&mut CodeItem)) -> Item {
    let mut i = Item::new(ItemKind::Code);
    if let Item::Code(x) = &mut i {
        f(x);
    }
    i
}

fn wifi_code(symbology: Symbology, ssid: &str, password: &str) -> Item {
    code(|c| {
        c.symbology = symbology;
        c.content = CodeContent::Wifi;
        c.wifi = Some(WifiSettings {
            ssid: ssid.into(),
            password: password.into(),
            ..WifiSettings::default()
        });
    })
}

fn doc(items: Vec<Item>, batch: Option<BatchData>) -> LabelDoc {
    let mut d = LabelDoc::with_items(items);
    d.batch = batch;
    d
}

fn batch(f: impl FnOnce(&mut BatchData)) -> BatchData {
    let mut b = BatchData::default();
    f(&mut b);
    b
}

fn ctx() -> VariableContext {
    VariableContext::with_locale(date!(2026 - 10 - 08), "en-GB")
}

fn texts(d: &LabelDoc) -> Vec<String> {
    d.items
        .iter()
        .map(|i| match i {
            Item::Text(t) => t.text.clone(),
            Item::Code(c) if c.content == CodeContent::Wifi => {
                let w = c.wifi.clone().unwrap_or_default();
                format!("{}|{}", w.ssid, w.password)
            }
            Item::Code(c) => c.data.clone(),
            _ => String::new(),
        })
        .collect()
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn table() -> BatchData {
    batch(|b| {
        b.enabled = true;
        b.columns = strs(&["name", "room"]);
        b.rows = vec![
            strs(&["Ada", "Lab 1"]),
            strs(&["Grace", "Lab 2"]),
            strs(&["Linus", ""]),
        ];
    })
}

#[test]
fn batch_size_zero_without_enabled_batch_rows_or_count_otherwise_capped() {
    assert_eq!(batch_size(&LabelDoc::new()), 0);
    assert_eq!(batch_size(&doc(vec![], Some(batch(|b| b.count = 7)))), 0);
    let on = |f: fn(&mut BatchData)| {
        doc(
            vec![],
            Some(batch(|b| {
                b.enabled = true;
                f(b)
            })),
        )
    };
    assert_eq!(batch_size(&on(|b| b.count = 7)), 7);
    assert_eq!(
        batch_size(&on(|b| {
            b.columns = vec!["a".into()];
            b.rows = vec![vec!["1".into()], vec!["2".into()]];
        })),
        2
    );
    assert_eq!(batch_size(&on(|b| b.count = 10_000)), BATCH_ROWS);
}

#[test]
fn resolve_doc_returns_the_same_doc_when_there_is_nothing_to_replace() {
    let d = LabelDoc::new();
    let r = resolve_doc(&d, 0, &ctx());
    assert!(matches!(r.doc, Cow::Borrowed(x) if std::ptr::eq(x, &d)));
    assert!(r.missing.is_empty());
    let plain = doc(
        vec![
            text("Hello"),
            code(|c| c.data = "https://example.com".into()),
        ],
        Some(batch(|b| {
            b.enabled = true;
            b.columns = vec!["a".into()];
            b.rows = vec![vec!["1".into()]];
        })),
    );
    assert!(matches!(
        resolve_doc(&plain, 0, &ctx()).doc,
        Cow::Borrowed(_)
    ));
}

#[test]
fn placeholder_names_in_order_deduplicated_trimmed() {
    assert_eq!(
        placeholder_names("{{a}} {{ b }} {{a}} {{}} {{x y}} {single} {{{c}}}"),
        strs(&["a", "b", "", "x y", "c"])
    );
    assert!(placeholder_names("no placeholders {{ unclosed").is_empty());
}

#[test]
fn find_variables_scans_text_code_data_and_printed_wifi_fields() {
    let d = doc(
        vec![
            text("{{name}} in {{room}}"),
            code(|c| c.data = "{{id}}".into()),
            {
                let mut i = wifi_code(Symbology::Qr, "{{net}}", "{{pw}}");
                if let Item::Code(c) = &mut i {
                    c.data = "{{stale}}".into();
                }
                i
            },
            {
                let mut i = wifi_code(Symbology::Code128, "{{ignored}}", "");
                if let Item::Code(c) = &mut i {
                    c.data = "{{sku}}".into();
                }
                i
            },
        ],
        None,
    );
    assert_eq!(
        find_variables(&d),
        strs(&["name", "room", "id", "net", "pw", "sku"])
    );
}

#[test]
fn counter_values() {
    let c = |start, step, pad| BatchCounter {
        name: "n".into(),
        start,
        step,
        pad,
    };
    let cases = [
        (c(1, 1, 0), 0, "1"),
        (c(1, 1, 0), 9, "10"),
        (c(1, 1, 3), 0, "001"),
        (c(1, 1, 3), 999, "1000"),
        (c(100, 10, 0), 3, "130"),
        (c(5, -2, 0), 4, "-3"),
        (c(1, -1, 2), 2, "-01"),
        (c(-5, 0, 3), 7, "-005"),
        (c(0, 1, 12), 42, "000000000042"),
    ];
    for (counter, i, out) in cases {
        assert_eq!(counter_value(&counter, i), out, "{counter:?} at {i}");
    }
}

#[test]
fn counter_pads_each_label() {
    let d = doc(
        vec![text("A-{{n}}")],
        Some(batch(|b| {
            b.enabled = true;
            b.count = 3;
            b.counters[0].pad = 3;
        })),
    );
    let out: Vec<String> = (0..3)
        .map(|i| texts(&resolve_doc(&d, i, &ctx()).doc)[0].clone())
        .collect();
    assert_eq!(out, strs(&["A-001", "A-002", "A-003"]));
}

#[test]
fn date_offsets() {
    assert_eq!(date_offset("today"), Some(0));
    assert_eq!(date_offset("today+30d"), Some(30));
    assert_eq!(date_offset("today-7d"), Some(-7));
    assert_eq!(date_offset("today+3650d"), Some(3650));
    assert_eq!(date_offset("today+0d"), Some(0));
    assert_eq!(date_offset("today+3651d"), None);
    assert_eq!(date_offset("today+30"), None);
    assert_eq!(date_offset("today+12345d"), None);
    assert_eq!(date_offset("today +3d"), None);
    assert_eq!(date_offset("Today"), None);
}

#[test]
fn adds_calendar_days_across_month_and_year_ends() {
    let iso = |d| format_date(d, DateFormat::Iso, None);
    assert_eq!(iso(add_days(date!(2026 - 01 - 31), 1)), "2026-02-01");
    assert_eq!(iso(add_days(date!(2026 - 12 - 31), 1)), "2027-01-01");
    assert_eq!(iso(add_days(date!(2028 - 02 - 28), 1)), "2028-02-29");
    assert_eq!(iso(add_days(date!(2026 - 03 - 01), -1)), "2026-02-28");
    assert_eq!(iso(add_days(date!(2026 - 10 - 08), 30)), "2026-11-07");
    assert_eq!(iso(add_days(date!(2026 - 10 - 25), 0)), "2026-10-25");
}

#[test]
fn formats_iso_dmy_mdy_and_long() {
    let now = date!(2026 - 10 - 08);
    assert_eq!(format_date(now, DateFormat::Iso, None), "2026-10-08");
    assert_eq!(format_date(now, DateFormat::Dmy, None), "08/10/2026");
    assert_eq!(format_date(now, DateFormat::Mdy, None), "10/08/2026");
    assert_eq!(
        format_date(now, DateFormat::Long, Some("en-US")),
        "October 8, 2026"
    );
    assert_eq!(
        format_date(now, DateFormat::Long, Some("en-GB")),
        "8 October 2026"
    );
    assert_eq!(format_date(now, DateFormat::Long, None), "October 8, 2026");
    assert_eq!(
        format_date(date!(0999 - 01 - 02), DateFormat::Iso, None),
        "0999-01-02"
    );
}

#[test]
fn today_uses_the_batch_date_format_iso_without_batch_data() {
    let items = || {
        vec![text(
            "Packed {{today}}, best before {{today+30d}}, made {{today-7d}}",
        )]
    };
    assert_eq!(
        texts(&resolve_doc(&doc(items(), None), 0, &ctx()).doc)[0],
        "Packed 2026-10-08, best before 2026-11-07, made 2026-10-01"
    );
    let dmy = doc(items(), Some(batch(|b| b.date_format = DateFormat::Dmy)));
    assert_eq!(
        texts(&resolve_doc(&dmy, 0, &ctx()).doc)[0],
        "Packed 08/10/2026, best before 07/11/2026, made 01/10/2026"
    );
    let long = doc(
        vec![text("{{today}}")],
        Some(batch(|b| b.date_format = DateFormat::Long)),
    );
    assert_eq!(
        texts(&resolve_doc(&long, 0, &ctx()).doc)[0],
        "8 October 2026"
    );
}

#[test]
fn replaces_columns_from_the_row_in_text_and_code_data() {
    let d = doc(
        vec![
            text("{{name}} – {{room}}"),
            code(|c| c.data = "https://example.com/{{name}}".into()),
        ],
        Some(table()),
    );
    let r = resolve_doc(&d, 1, &ctx());
    assert!(r.missing.is_empty());
    assert_eq!(
        texts(&r.doc),
        strs(&["Grace – Lab 2", "https://example.com/Grace"])
    );
    assert_eq!(texts(&resolve_doc(&d, 2, &ctx()).doc)[0], "Linus – ");
    assert_eq!(texts(&resolve_doc(&d, 99, &ctx()).doc)[0], "Linus – ");
}

#[test]
fn never_mutates_the_input_and_keeps_unchanged_items() {
    let d = doc(vec![text("static"), text("{{name}}")], Some(table()));
    let before = d.clone();
    let r = resolve_doc(&d, 0, &ctx());
    assert_eq!(d, before);
    assert!(matches!(r.doc, Cow::Owned(_)));
    assert_eq!(r.doc.items[0], d.items[0]);
    assert_eq!(texts(&r.doc), strs(&["static", "Ada"]));
}

#[test]
fn columns_win_over_counters_and_built_ins() {
    let b = batch(|b| {
        b.enabled = true;
        b.columns = strs(&["n", "today"]);
        b.rows = vec![strs(&["col-n", "col-today"])];
    });
    let d = doc(vec![text("{{n}} {{today}}")], Some(b));
    assert_eq!(texts(&resolve_doc(&d, 0, &ctx()).doc)[0], "col-n col-today");
}

#[test]
fn a_disabled_batch_still_resolves_the_chosen_row() {
    let mut b = table();
    b.enabled = false;
    let d = doc(vec![text("{{name}} {{n}}")], Some(b));
    assert_eq!(texts(&resolve_doc(&d, 1, &ctx()).doc)[0], "Grace 2");
}

#[test]
fn unknown_names_stay_visible_and_are_listed_once_in_order() {
    let d = doc(
        vec![
            text("{{nmae}} {{name}} {{x y}} {{nmae}}"),
            code(|c| c.data = "{{id}}".into()),
        ],
        Some(table()),
    );
    let r = resolve_doc(&d, 0, &ctx());
    assert_eq!(
        texts(&r.doc),
        strs(&["{{nmae}} Ada {{x y}} {{nmae}}", "{{id}}"])
    );
    assert_eq!(r.missing, strs(&["nmae", "x y", "id"]));
    assert_eq!(missing_variables(&d), strs(&["nmae", "x y", "id"]));
}

#[test]
fn resolves_only_once() {
    let b = batch(|b| {
        b.enabled = true;
        b.columns = strs(&["a", "b"]);
        b.rows = vec![strs(&["{{b}}", "B"])];
    });
    let d = doc(vec![text("{{a}}")], Some(b));
    let r = resolve_doc(&d, 0, &ctx());
    assert_eq!(texts(&r.doc)[0], "{{b}}");
    assert!(r.missing.is_empty());
}

#[test]
fn ssid_is_the_first_wifi_codes_network_name_itself_resolved() {
    let d = doc(
        vec![
            text("Wi-Fi\n{{ssid}}"),
            wifi_code(Symbology::Qr, "Home-{{n}}", "{{n}}"),
        ],
        Some(batch(|b| {
            b.enabled = true;
            b.count = 2;
        })),
    );
    let r = resolve_doc(&d, 1, &ctx());
    assert!(r.missing.is_empty());
    assert_eq!(texts(&r.doc), strs(&["Wi-Fi\nHome-2", "Home-2|2"]));
    assert_eq!(variable_kind(&d, "ssid"), VariableKind::Ssid);
}

#[test]
fn wifi_password_is_a_placeholder_only_when_the_whole_field_is_one() {
    let b = batch(|b| {
        b.enabled = true;
        b.columns = strs(&["y", "pw"]);
        b.rows = vec![strs(&["Y", "row-secret"])];
    });
    let literal = doc(
        vec![wifi_code(Symbology::Qr, "Home", "x{{y}}z{{today}}")],
        Some(b.clone()),
    );
    assert!(find_variables(&literal).is_empty());
    assert!(missing_variables(&literal).is_empty());
    let r = resolve_doc(&literal, 0, &ctx());
    assert!(matches!(r.doc, Cow::Borrowed(_)));
    assert_eq!(texts(&r.doc), strs(&["Home|x{{y}}z{{today}}"]));
    assert!(
        missing_variables(&doc(vec![wifi_code(Symbology::Qr, "Home", "a{{b}}")], None)).is_empty()
    );

    let whole = doc(vec![wifi_code(Symbology::Qr, "Home", "{{ pw }}")], Some(b));
    assert_eq!(find_variables(&whole), strs(&["pw"]));
    assert_eq!(
        texts(&resolve_doc(&whole, 0, &ctx()).doc),
        strs(&["Home|row-secret"])
    );
    assert_eq!(password_variable("{{pw}}"), Some("pw"));
    assert_eq!(password_variable(" {{pw}}"), None);
    assert_eq!(password_variable("pw"), None);
}

#[test]
fn ssid_stays_visible_but_not_missing_while_the_network_name_is_empty() {
    let d = doc(
        vec![text("{{ssid}}"), wifi_code(Symbology::Qr, "", "")],
        None,
    );
    let r = resolve_doc(&d, 0, &ctx());
    assert!(matches!(r.doc, Cow::Borrowed(_)));
    assert!(r.missing.is_empty());
}

#[test]
fn ssid_without_a_wifi_code_or_inside_the_network_name_is_missing() {
    assert_eq!(
        resolve_doc(&doc(vec![text("{{ssid}}")], None), 0, &ctx()).missing,
        strs(&["ssid"])
    );
    let selfref = doc(vec![wifi_code(Symbology::Qr, "x{{ssid}}", "p")], None);
    assert_eq!(resolve_doc(&selfref, 0, &ctx()).missing, strs(&["ssid"]));
    let switched = doc(
        vec![
            text("{{ssid}}"),
            code(|c| {
                c.content = CodeContent::Text;
                c.wifi = Some(WifiSettings {
                    ssid: "Net".into(),
                    ..WifiSettings::default()
                });
            }),
        ],
        None,
    );
    assert_eq!(variable_kind(&switched, "ssid"), VariableKind::Missing);
}

#[test]
fn reports_kinds_and_the_columns_the_label_uses() {
    let d = doc(
        vec![text("{{name}} {{n}} {{today+1d}} {{nope}}")],
        Some(table()),
    );
    let kinds: Vec<VariableKind> = ["name", "room", "n", "today+1d", "nope"]
        .iter()
        .map(|n| variable_kind(&d, n))
        .collect();
    use VariableKind::*;
    assert_eq!(kinds, [Column, Column, Counter, Date, Missing]);
    assert_eq!(used_columns(&d), strs(&["name"]));
}

#[test]
fn columns_without_data_blocks_empty_tables() {
    let header = batch(|b| {
        b.enabled = true;
        b.columns = strs(&["name", "room"]);
    });
    assert_eq!(
        columns_without_data(&doc(vec![text("Hi {{name}}")], Some(header.clone()))),
        strs(&["name"])
    );
    assert!(columns_without_data(&doc(vec![text("Hi {{n}}")], Some(header.clone()))).is_empty());
    let mut with_rows = header.clone();
    with_rows.rows = vec![strs(&["Ada", "1"])];
    assert!(columns_without_data(&doc(vec![text("Hi {{name}}")], Some(with_rows))).is_empty());
    let mut off = header;
    off.enabled = false;
    assert!(columns_without_data(&doc(vec![text("Hi {{name}}")], Some(off))).is_empty());
}

#[test]
fn dated_labels_and_local_day() {
    assert!(uses_dates(&doc(
        vec![text("Best before {{today+30d}}")],
        None
    )));
    assert!(!uses_dates(&doc(vec![text("{{n}} {{name}}")], None)));
    assert_eq!(local_day(date!(2026 - 10 - 08)), "2026-10-08");
    assert_eq!(local_day(date!(2026 - 10 - 09)), "2026-10-09");
}
