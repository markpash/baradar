//! Keyboard shortcuts dialog (port of ShortcutsDialog.svelte and `shortcutList` in
//! ui/state/shortcuts.ts). The keys are read from the live key bindings (actions.rs), so the list
//! never disagrees with them. The web studio's single-key shortcuts and their switch have no
//! desktop counterpart: every desktop shortcut uses ⌘.

use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, kbd::Kbd, v_flex};
use gpui_kit::*;

use super::common::heading;
use crate::actions::*;
use crate::studio::Studio;

pub fn open_shortcuts(_studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, window, cx| {
        dialog.title("Keyboard shortcuts").w(px(520.)).child(shortcut_list(window, cx))
    });
}

/// One shortcut row: the action and its label.
type Row = (Box<dyn Action>, &'static str);

/// The rows, grouped like the menus: (section, rows).
fn sections() -> Vec<(&'static str, Vec<Row>)> {
    vec![
        (
            "Labels",
            vec![
                (Box::new(NewLabel), "New label"),
                (Box::new(NewFromTemplate), "New from template"),
                (Box::new(OpenLibrary), "Your labels"),
                (Box::new(ImportFile), "Import file"),
                (Box::new(ExportFile), "Export file"),
                (Box::new(ExportImage), "Export image"),
            ],
        ),
        (
            "Editing",
            vec![
                (Box::new(Undo), "Undo"),
                (Box::new(Redo), "Redo"),
                (Box::new(DuplicateBlock), "Duplicate block"),
                (Box::new(DeleteBlock), "Delete block"),
                (Box::new(SelectPrevious), "Select previous block"),
                (Box::new(SelectNext), "Select next block"),
                (Box::new(MoveBlockUp), "Move block earlier"),
                (Box::new(MoveBlockDown), "Move block later"),
            ],
        ),
        (
            "View",
            vec![
                (Box::new(ZoomIn), "Zoom in"),
                (Box::new(ZoomOut), "Zoom out"),
                (Box::new(ZoomFit), "Zoom to fit"),
                (Box::new(ZoomReal), "Real size"),
                (Box::new(ToggleTheme), "Dark mode"),
            ],
        ),
        (
            "Printer",
            vec![(Box::new(Print), "Print"), (Box::new(CancelPrint), "Cancel printing")],
        ),
        (
            "App",
            vec![(Box::new(ShowShortcuts), "Show shortcuts"), (Box::new(Quit), "Quit")],
        ),
    ]
}

fn shortcut_list(window: &mut Window, cx: &mut App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let mut list = v_flex().gap_4();
    for (title, rows) in sections() {
        let rows: Vec<(Kbd, &'static str)> = rows
            .iter()
            .filter_map(|(action, label)| Kbd::binding_for_action(action.as_ref(), Some(CONTEXT), window).map(|k| (k, *label)))
            .collect();
        if rows.is_empty() {
            continue;
        }
        list = list.child(
            v_flex().gap_1().child(heading(title)).children(rows.into_iter().map(|(kbd, label)| {
                h_flex()
                    .gap_3()
                    .text_sm()
                    .child(h_flex().w(px(120.)).child(kbd))
                    .child(div().text_color(muted).child(label))
            })),
        );
    }
    list.into_any_element()
}
