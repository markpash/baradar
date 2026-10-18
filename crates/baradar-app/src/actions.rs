//! Actions, key bindings and the menu bar (port of the studio's keyboard shortcuts).

use gpui_kit::*;

actions!(
    baradar,
    [
        Quit,
        ToggleTheme,
        NewLabel,
        NewFromTemplate,
        OpenLibrary,
        ImportFile,
        ExportFile,
        ExportImage,
        CopyShareLink,
        PrintHistory,
        ManageFonts,
        Undo,
        Redo,
        Print,
        CancelPrint,
        DeleteBlock,
        DuplicateBlock,
        SelectPrevious,
        SelectNext,
        MoveBlockUp,
        MoveBlockDown,
        ZoomIn,
        ZoomOut,
        ZoomFit,
        ZoomReal,
        ConnectPrinter,
        DisconnectPrinter,
        PrinterSettings,
        ShowDiagnostics,
        ShowShortcuts,
    ]
);

/// Key context of the studio view; view-level bindings only fire while it has focus.
pub const CONTEXT: &str = "Studio";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-n", NewLabel, c),
        KeyBinding::new("cmd-shift-n", NewFromTemplate, c),
        KeyBinding::new("cmd-o", OpenLibrary, c),
        KeyBinding::new("cmd-shift-o", ImportFile, c),
        KeyBinding::new("cmd-s", ExportFile, c),
        KeyBinding::new("cmd-e", ExportImage, c),
        KeyBinding::new("cmd-z", Undo, c),
        KeyBinding::new("cmd-shift-z", Redo, c),
        KeyBinding::new("cmd-p", Print, c),
        KeyBinding::new("cmd-d", DuplicateBlock, c),
        KeyBinding::new("cmd-backspace", DeleteBlock, c),
        KeyBinding::new("cmd-up", SelectPrevious, c),
        KeyBinding::new("cmd-down", SelectNext, c),
        KeyBinding::new("cmd-alt-up", MoveBlockUp, c),
        KeyBinding::new("cmd-alt-down", MoveBlockDown, c),
        KeyBinding::new("cmd-=", ZoomIn, c),
        KeyBinding::new("cmd--", ZoomOut, c),
        KeyBinding::new("cmd-0", ZoomFit, c),
        KeyBinding::new("cmd-1", ZoomReal, c),
        KeyBinding::new("cmd-shift-d", ToggleTheme, None),
        KeyBinding::new("cmd-/", ShowShortcuts, c),
    ]);
}

pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Baradar").items([
            MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
            MenuItem::action("Diagnostics", ShowDiagnostics),
            MenuItem::separator(),
            MenuItem::action("Quit Baradar", Quit),
        ]),
        Menu::new("Labels").items([
            MenuItem::action("New Label", NewLabel),
            MenuItem::action("New from Template…", NewFromTemplate),
            MenuItem::action("Open…", OpenLibrary),
            MenuItem::action("Print History…", PrintHistory),
            MenuItem::separator(),
            MenuItem::action("Import File…", ImportFile),
            MenuItem::action("Export File…", ExportFile),
            MenuItem::action("Export Image (PNG, PDF)…", ExportImage),
            MenuItem::action("Copy Share Link", CopyShareLink),
            MenuItem::separator(),
            MenuItem::action("Fonts…", ManageFonts),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", Undo),
            MenuItem::action("Redo", Redo),
            MenuItem::separator(),
            MenuItem::action("Duplicate Block", DuplicateBlock),
            MenuItem::action("Delete Block", DeleteBlock),
            MenuItem::action("Move Block Up", MoveBlockUp),
            MenuItem::action("Move Block Down", MoveBlockDown),
        ]),
        Menu::new("View").items([
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Fit", ZoomFit),
            MenuItem::action("Actual Size", ZoomReal),
            MenuItem::separator(),
            MenuItem::action("Toggle Dark Mode", ToggleTheme),
        ]),
        Menu::new("Printer").items([
            MenuItem::action("Print", Print),
            MenuItem::action("Cancel Printing", CancelPrint),
            MenuItem::separator(),
            MenuItem::action("Connect", ConnectPrinter),
            MenuItem::action("Disconnect", DisconnectPrinter),
            MenuItem::action("Printer Settings…", PrinterSettings),
        ]),
    ]
}
