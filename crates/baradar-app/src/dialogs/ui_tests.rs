//! Headless checks that every dialog opens and draws against a real `Studio` on a throwaway
//! store (no printer is contacted: a fresh store has no remembered printer).

use baradar_core::store::Store;
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, Point, Render, TestAppContext, Window,
    WindowBounds, WindowOptions, div, px, size,
};

use crate::studio::Studio;

struct Host {
    studio: Entity<Studio>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

type Open = fn(Entity<Studio>, &mut Window, &mut App);

/// A studio in a window on a throwaway store under `name`.
fn studio_window(cx: &mut TestAppContext, name: &str) -> (std::path::PathBuf, gpui_kit::WindowHandle<gpui_kit::base::Root>, Entity<Host>) {
    let root = std::env::temp_dir().join(format!("baradar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = Store::open(&root).expect("temp store");
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::actions::bind_keys(cx);
    });
    // The printer client runs on its own threads and wakes UI tasks from there.
    cx.executor().allow_parking();
    let (window, host) = cx.update(|cx| {
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(1200.), px(900.)) })),
            ..Default::default()
        };
        gpui_kit::open_window(opts, cx, |_, cx| {
            let studio = cx.new(|cx| Studio::new(store, cx));
            cx.new(|_| Host { studio })
        })
        .expect("window")
    });
    let window = window.downcast::<gpui_kit::base::Root>().expect("root");
    (root, window, host)
}

#[gpui_kit::test]
fn choosing_a_template_opens_it_as_a_new_label(cx: &mut TestAppContext) {
    let (root, window, host) = studio_window(cx, "templates");
    let before = cx.update(|cx| host.read(cx).studio.read(cx).doc.id.clone());
    cx.update_window(window.into(), |_, window, cx| {
        let studio = host.read(cx).studio.clone();
        let cards = super::templates::test_open_and_choose(studio, "wifi-12", window, cx);
        assert_eq!(cards, baradar_core::templates::templates().len());
    })
    .expect("choose");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(!window.has_active_dialog(cx), "the gallery closes");
        let studio = host.read(cx).studio.read(cx);
        assert_ne!(studio.doc.id, before);
        assert_eq!(studio.doc.tape.width_mm, 12.0);
        assert_eq!(studio.doc.tape.model.as_deref(), Some(studio.model().name));
        let primary = baradar_core::templates::primary_item_id(&studio.doc).map(str::to_owned);
        assert_eq!(studio.selected, primary);
    })
    .expect("check");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
fn every_dialog_opens_and_renders(cx: &mut TestAppContext) {
    let (root, window, host) = studio_window(cx, "dialogs");
    let dialogs: [(&str, Open); 6] = [
        ("library", super::library::open_library),
        ("templates", super::templates::open_templates),
        ("history", super::history::open_history),
        ("export", super::export::open_export_image),
        ("share", super::share::copy_share_link),
        ("shortcuts", super::shortcuts::open_shortcuts),
    ];
    for (name, open) in dialogs {
        cx.update_window(window.into(), |_, window, cx| {
            let studio = host.read(cx).studio.clone();
            open(studio, window, cx);
            window.render_frame(cx);
        })
        .expect(name);
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            // Copy Share Link copies straight away on a label without Wi-Fi passwords.
            if name != "share" {
                assert!(window.has_active_dialog(cx), "{name} did not open");
            }
            window.close_all_dialogs(cx);
        })
        .expect(name);
    }
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
fn print_records_reopen_as_themselves_or_as_copies(cx: &mut TestAppContext) {
    use super::history::Reopened;
    use baradar_core::doc::LabelDoc;
    use baradar_core::store::HistoryRecord;

    let (root, _window, host) = studio_window(cx, "history");
    let studio = cx.update(|cx| host.read(cx).studio.clone());
    let mut printed = LabelDoc::new();
    printed.name = "Spices".into();
    let record = HistoryRecord {
        id: "r1".into(),
        printed_at: "2026-10-18T10:00:00.000Z".into(),
        doc: serde_json::to_value(&printed).expect("doc"),
        name: printed.name.clone(),
        tape_width_mm: printed.tape.width_mm,
        media_id: None,
        model: None,
        labels: 1,
        copies: 1,
        batch_rows: None,
        tape_mm: 50.0,
        seq: 0,
    };
    cx.update(|cx| {
        studio.update(cx, |s, cx| {
            // Not in the library: opens as itself.
            assert_eq!(s.open_print_record(&record, cx), Ok(Reopened::AsItself));
            assert_eq!(s.doc.id, printed.id);
            // The library copy changed after the print: opens as a copy.
            let mut edited = printed.clone();
            edited.name = "Spices, edited".into();
            s.store.save_label(&edited, None).expect("save");
            s.new_label(cx);
            assert_eq!(s.open_print_record(&record, cx), Ok(Reopened::AsCopy));
            assert_ne!(s.doc.id, printed.id);
            assert!(s.doc.name.starts_with("Spices (printed "), "{}", s.doc.name);
            // The library holds the printed version: opens it.
            s.store.save_label(&printed, None).expect("save");
            assert_eq!(s.open_print_record(&record, cx), Ok(Reopened::AsItself));
            assert_eq!(s.doc.id, printed.id);
        });
    });
    let _ = std::fs::remove_dir_all(root);
}
