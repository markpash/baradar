//! UI icons (port of `web/src/ui/common/Icon.svelte`): the studio's icon names, drawn from the
//! Lucide SVGs gpui-kit bundles (the studio inlines a subset of the same Lucide set).

use gpui_kit::SharedString;
use gpui_kit::component::Icon;

/// The icon the studio calls `name`.
pub fn icon(name: &str) -> Icon {
    Icon::default().path(SharedString::from(format!("icons/{}.svg", lucide_name(name))))
}

/// The bundled Lucide file for a studio icon name (most are the same; a few were renamed or are
/// the studio's own names).
fn lucide_name(name: &str) -> &str {
    match name {
        "qr" => "qr-code",
        "undo" => "undo-2",
        "redo" => "redo-2",
        "space" => "move-horizontal",
        "grip" => "grip-vertical",
        "alert" => "triangle-alert",
        "loader" => "loader-circle",
        "refresh" => "refresh-cw",
        "more" => "ellipsis",
        "history" => "clock",
        "template" => "layout-template",
        "flip" => "flip-horizontal-2",
        "share" => "share-2",
        "stop" => "square",
        "line" => "minus",
        "align-start" => "text-align-start",
        "align-center" => "text-align-center",
        "align-end" => "text-align-end",
        "valign-start" => "align-vertical-justify-start",
        "valign-center" => "align-vertical-justify-center",
        "valign-end" => "align-vertical-justify-end",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::lucide_name;

    /// Every name the app uses exists in gpui-kit's bundled icon set.
    #[test]
    fn icons_exist_in_the_bundle() {
        let names = [
            "printer", "bluetooth", "usb", "cable", "type", "qr", "barcode", "image", "star", "square",
            "circle", "line", "space", "settings", "x", "undo", "redo", "trash", "copy", "chevron-up",
            "chevron-down", "chevron-left", "chevron-right", "arrow-up", "arrow-down", "grip", "plus",
            "minus", "sun", "moon", "monitor", "battery", "battery-low", "plug", "keyboard", "eject",
            "loader", "alert", "info", "check", "table", "upload", "tag", "refresh", "search",
            "smartphone", "send", "download", "activity", "file-plus", "history", "folder", "link",
            "share", "more", "flip", "template", "wifi", "stop", "align-start", "align-center",
            "align-end", "valign-start", "valign-center", "valign-end",
        ];
        let assets = gpui_kit::assets::AllAssets;
        for n in names {
            let path = format!("icons/{}.svg", lucide_name(n));
            let found = gpui_kit::AssetSource::load(&assets, &path).ok().flatten().is_some();
            assert!(found, "{n} → {path} is not bundled");
        }
    }
}
