//! Headless checks of the properties column against a real `Studio` on a throwaway store (no
//! printer is contacted: a fresh store has no remembered printer).

// NOT `use super::*`: with test-support, `gpui_kit::*` exports GPUI's `test` macro, which shadows
// Rust's #[test] and makes #[gpui_kit::test] recurse forever.
use super::PropertiesView;
use crate::studio::Studio;
use baradar_core::doc::{CodeContent, Item, ItemKind, TextSize, WifiSettings};
use baradar_core::store::Store;
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, point, px, size};

fn setup(cx: &mut TestAppContext, name: &str) -> (std::path::PathBuf, AnyWindowHandle, Entity<Studio>, Entity<PropertiesView>) {
    let root = std::env::temp_dir().join(format!("baradar-props-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = Store::open(&root).expect("temp store");
    cx.update(gpui_kit::init);
    // The printer client runs on its own threads and wakes UI tasks from there.
    cx.executor().allow_parking();
    let studio = cx.update(|cx| cx.new(|cx| Studio::new(store, cx)));
    let s2 = studio.clone();
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(420.), px(3000.))))),
                ..Default::default()
            },
            cx,
            move |window, cx| cx.new(|cx| PropertiesView::new(s2, window, cx)),
        )
        .expect("window")
    });
    cx.run_until_parked();
    (root, window, studio, view)
}

fn frame(cx: &mut TestAppContext, w: AnyWindowHandle) {
    cx.update_window(w, |_, window, cx| window.render_frame(cx)).expect("frame");
    cx.run_until_parked();
}

fn selected_text(studio: &Entity<Studio>, cx: &mut TestAppContext) -> baradar_core::doc::TextItem {
    cx.update(|cx| match studio.read(cx).selected_item() {
        Some(Item::Text(t)) => t.clone(),
        other => panic!("expected a selected text block, got {other:?}"),
    })
}

#[gpui_kit::test]
fn text_editor_edits_and_reloads(cx: &mut TestAppContext) {
    let (root, w, studio, view) = setup(cx, "text");
    frame(cx, w);
    let before = selected_text(&studio, cx);

    // Quick size M, then switches.
    cx.update_window(w, |_, window, cx| {
        window.render_frame(cx);
        window.click("quick-M", cx);
        window.click("text-italic", cx);
        window.click("label-frame", cx);
    })
    .expect("click");
    cx.run_until_parked();
    let t = selected_text(&studio, cx);
    assert!(matches!(t.size, TextSize::Mm { .. }), "quick M stores a mm size: {:?}", t.size);
    assert!(t.italic);
    assert!(cx.update(|cx| studio.read(cx).doc.frame.is_some()), "frame switch");

    // Typing into the text field edits the block; one undo step for the whole word.
    let area = cx.update(|cx| match &view.read(cx).editor {
        Some(super::ItemEditor::Text(e)) => match e.fields.str("text").map(|f| f.state.clone()) {
            Some(super::widgets::StrState::Area(a)) => a,
            _ => panic!("text area"),
        },
        _ => panic!("text editor"),
    });
    cx.update_window(w, |_, window, cx| {
        area.update(cx, |s, cx| s.focus(window, cx));
        window.render_frame(cx);
        window.input("XY", cx);
        window.input("Z", cx);
    })
    .expect("type");
    cx.run_until_parked();
    let typed = selected_text(&studio, cx).text;
    assert!(typed.contains("XYZ"), "typed text reaches the doc: {typed:?}");
    studio.update(cx, |s, cx| s.undo(cx));
    cx.run_until_parked();
    frame(cx, w);
    let undone = selected_text(&studio, cx).text;
    assert_eq!(undone, before.text, "typing coalesces into one undo step");
    // The undo bumped the epoch, so the (focused) field reloaded.
    let shown = cx.update(|cx| area.read(cx).value().to_string());
    assert_eq!(shown, before.text);
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
fn every_block_kind_renders(cx: &mut TestAppContext) {
    let (root, w, studio, _view) = setup(cx, "kinds");
    for kind in [ItemKind::Icon, ItemKind::Code, ItemKind::Image, ItemKind::Shape, ItemKind::Spacer] {
        studio.update(cx, |s, cx| s.insert(kind, |_| {}, cx));
        cx.run_until_parked();
        frame(cx, w);
    }
    // Wi-Fi QR with its fields.
    studio.update(cx, |s, cx| {
        s.insert(
            ItemKind::Code,
            |i| {
                if let Item::Code(c) = i {
                    c.content = CodeContent::Wifi;
                    c.wifi = Some(WifiSettings::default());
                }
            },
            cx,
        )
    });
    cx.run_until_parked();
    frame(cx, w);
    cx.update_window(w, |_, window, cx| {
        window.render_frame(cx);
        window.click("wifi-add-ssid", cx);
    })
    .expect("click");
    cx.run_until_parked();
    let t = selected_text(&studio, cx);
    assert_eq!(t.text, "{{ssid}}");

    // Icon grid: pick another icon.
    studio.update(cx, |s, cx| s.insert(ItemKind::Icon, |_| {}, cx));
    cx.run_until_parked();
    cx.update_window(w, |_, window, cx| {
        window.render_frame(cx);
        window.click("icon-plug", cx);
    })
    .expect("click");
    cx.run_until_parked();
    frame(cx, w);
    let icon = cx.update(|cx| match studio.read(cx).selected_item() {
        Some(Item::Icon(i)) => i.icon_id.clone(),
        _ => String::new(),
    });
    assert_eq!(icon, "plug");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
fn number_field_commits_and_clamps(cx: &mut TestAppContext) {
    let (root, w, studio, view) = setup(cx, "numbers");
    studio.update(cx, |s, cx| s.insert(ItemKind::Spacer, |_| {}, cx));
    cx.run_until_parked();
    frame(cx, w);
    let width = |cx: &mut TestAppContext| {
        cx.update(|cx| match studio.read(cx).selected_item() {
            Some(Item::Spacer(s)) => s.width_mm,
            _ => f64::NAN,
        })
    };
    let input = cx.update(|cx| match &view.read(cx).editor {
        Some(super::ItemEditor::Spacer(e)) => e.fields.num("width").map(|f| f.state.clone()).expect("width field"),
        _ => panic!("spacer editor"),
    });
    cx.update_window(w, |_, window, cx| {
        input.update(cx, |s, cx| s.focus(window, cx));
        window.render_frame(cx);
        window.press("cmd-a", cx);
        window.input("12.5", cx);
    })
    .expect("type");
    cx.run_until_parked();
    assert_eq!(width(cx), 12.5, "valid values commit while typing");
    cx.update_window(w, |_, window, cx| {
        window.press("cmd-a", cx);
        window.input("900", cx);
        window.press("enter", cx);
    })
    .expect("type");
    cx.run_until_parked();
    assert_eq!(width(cx), 500.0, "Enter clamps to the maximum");
    let shown = cx.update(|cx| input.read(cx).value().to_string());
    assert_eq!(shown, "500");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
fn font_picker_opens_and_picks(cx: &mut TestAppContext) {
    let (root, w, studio, _view) = setup(cx, "fonts");
    frame(cx, w);
    cx.update_window(w, |_, window, cx| {
        window.render_frame(cx);
        window.click("text-font", cx);
    })
    .expect("open");
    cx.run_until_parked();
    cx.update_window(w, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx), "the picker opens");
        window.input("jetbrains", cx);
    })
    .expect("search");
    cx.run_until_parked();
    cx.update_window(w, |_, window, cx| {
        window.render_frame(cx);
        window.click(("font-row", 0usize), cx);
    })
    .expect("pick");
    cx.run_until_parked();
    let t = selected_text(&studio, cx);
    assert_eq!(t.font_family, "jetbrains-mono");
    assert!(cx.update(|cx| studio.read(cx).prefs.recent_fonts.first().cloned()) == Some("b:jetbrains-mono".into()));
    cx.update_window(w, |_, window, cx| assert!(!window.has_active_dialog(cx), "picking closes")).expect("w");
    let _ = std::fs::remove_dir_all(root);
}
