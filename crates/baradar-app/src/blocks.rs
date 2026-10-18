//! The left column: insert tiles and the block list (port of `editor/InsertPanel.svelte` and
//! `editor/BlockList.svelte`), and the selected block's card header.

use baradar_core::doc::{CodeContent, Item, ItemKind, Symbology, WifiSettings};
use baradar_core::render::icons::icon_by_id;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

use crate::app::StudioView;
use crate::icons::icon;
use crate::view_model::item_summary;

/// The eight insert tiles: label and what they add.
#[derive(Debug, Clone, Copy)]
enum Tile {
    Text,
    Icon,
    Qr,
    WifiQr,
    Barcode,
    Image,
    Shape,
    Spacer,
}

/// The insert tiles: what they add, their label and icon (`InsertPanel.svelte`).
const TILES: [(Tile, &str, &str); 8] = [
    (Tile::Text, "Text", "type"),
    (Tile::Icon, "Icon", "star"),
    (Tile::Qr, "QR code", "qr"),
    (Tile::WifiQr, "Wi-Fi QR", "wifi"),
    (Tile::Barcode, "Barcode", "barcode"),
    (Tile::Image, "Image", "image"),
    (Tile::Shape, "Shape", "square"),
    (Tile::Spacer, "Spacer", "space"),
];

/// A block being dragged in the block list.
#[derive(Debug, Clone)]
pub struct DraggedBlock {
    id: String,
    from: usize,
    title: String,
    glyph: &'static str,
}

/// The position a block dragged from `from` ends up at when dropped at insertion index `at`
/// (0 = before the first block, n = after the last). Removing the block first shifts the later
/// positions up by one.
fn drop_position(from: usize, at: usize) -> usize {
    if from < at { at - 1 } else { at }
}

/// What follows the pointer while a block is dragged.
struct DragPreview {
    title: String,
    glyph: &'static str,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(theme.primary)
            .bg(theme.background)
            .text_color(theme.foreground)
            .shadow_md()
            .text_sm()
            .child(icon(self.glyph).small())
            .child(self.title.clone())
    }
}

/// The block list icon of an item (`KIND_META`; a code that is not QR shows a barcode).
fn kind_icon(item: &Item) -> &'static str {
    match item {
        Item::Text(_) => "type",
        Item::Icon(_) => "star",
        Item::Code(c) if c.symbology != Symbology::Qr => "barcode",
        Item::Code(_) => "qr",
        Item::Image(_) => "image",
        Item::Shape(_) => "square",
        Item::Spacer(_) => "space",
    }
}

impl StudioView {
    fn insert_tile(&mut self, tile: Tile, cx: &mut Context<Self>) {
        match tile {
            Tile::Text => self.studio.update(cx, |s, cx| s.insert(ItemKind::Text, |_| {}, cx)),
            Tile::Icon => self.studio.update(cx, |s, cx| s.insert(ItemKind::Icon, |_| {}, cx)),
            Tile::Qr => self.studio.update(cx, |s, cx| s.insert(ItemKind::Code, |_| {}, cx)),
            Tile::WifiQr => self.studio.update(cx, |s, cx| {
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
            }),
            Tile::Barcode => self.studio.update(cx, |s, cx| {
                s.insert(
                    ItemKind::Code,
                    |i| {
                        if let Item::Code(c) = i {
                            c.symbology = Symbology::Code128;
                            c.data = "ABC-123".into();
                            c.show_text = true;
                        }
                    },
                    cx,
                )
            }),
            Tile::Shape => self.studio.update(cx, |s, cx| s.insert(ItemKind::Shape, |_| {}, cx)),
            Tile::Spacer => self.studio.update(cx, |s, cx| s.insert(ItemKind::Spacer, |_| {}, cx)),
            Tile::Image => self.pick_image(None, cx),
        }
    }

    /// Asks for an image file; inserts an image block, or replaces `item_id`'s image.
    pub fn pick_image(&mut self, item_id: Option<String>, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose image".into()),
        });
        let studio = self.studio.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let Ok(bytes) = std::fs::read(&path) else { return };
            let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
                Some("png") => "image/png",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                Some("bmp") => "image/bmp",
                _ => "application/octet-stream",
            };
            studio
                .update(cx, |s, cx| {
                    let Ok(r) = s.store.put_blob(&bytes, mime) else { return };
                    let set = move |i: &mut Item| {
                        if let Item::Image(img) = i {
                            img.blob_ref = r;
                            img.data_url = None;
                        }
                    };
                    match item_id {
                        Some(id) => s.update_item(&id, None, set, cx),
                        None => s.insert(ItemKind::Image, set, cx),
                    }
                });
        })
        .detach();
    }

    pub fn left_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let warned: Vec<String> = s
            .render
            .as_ref()
            .map(|r| r.warnings.iter().filter_map(|w| w.item_id.clone()).collect())
            .unwrap_or_default();
        let selected = s.selected.clone();
        let n = s.doc.items.len();
        let rows: Vec<(String, String, String, bool, &'static str)> = s
            .doc
            .items
            .iter()
            .map(|item| {
                let (title, summary) = item_summary(item, |id| icon_by_id(id).map(|i| i.label.to_owned()));
                (item.id().to_owned(), title, summary, warned.iter().any(|w| w == item.id()), kind_icon(item))
            })
            .collect();

        let tiles = div().grid().grid_cols(4).gap_1().children(TILES.iter().map(|&(tile, label, glyph)| {
            v_flex()
                .id(SharedString::from(format!("insert-{label}")))
                .items_center()
                .justify_center()
                .gap_1()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .cursor_pointer()
                .hover(|d| d.bg(theme.accent))
                .on_click(cx.listener(move |this, _, _, cx| this.insert_tile(tile, cx)))
                .child(icon(glyph))
                .child(div().text_xs().child(label))
        }));

        // While a block is dragged, a line marks where it will land.
        if !cx.has_active_drag() {
            self.block_drop = None;
        }
        let drop_at = self.block_drop;
        let line = |at: usize| {
            div()
                .h(px(2.))
                .mx_1()
                .rounded_full()
                .when(drop_at == Some(at), |d| d.bg(theme.primary))
        };
        let mut list = v_flex().id("block-list").gap_0p5();
        for (ix, (id, title, summary, warn, glyph)) in rows.into_iter().enumerate() {
            let is_sel = selected.as_deref() == Some(id.as_str());
            let (sel_id, dup_id, del_id) = (id.clone(), id.clone(), id.clone());
            let dragged = DraggedBlock { id: id.clone(), from: ix, title: title.clone(), glyph };
            list = list.child(line(ix)).child(
                h_flex()
                    .id(SharedString::from(format!("block-{id}")))
                    .p_2()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if is_sel { theme.primary } else { theme.border })
                    .when(is_sel, |d| d.bg(theme.primary.opacity(0.08)))
                    .cursor_grab()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.studio.update(cx, |s, cx| s.select(Some(sel_id.clone()), cx))
                    }))
                    .on_drag(dragged, |d, _, _, cx| {
                        let (title, glyph) = (d.title.clone(), d.glyph);
                        cx.new(|_| DragPreview { title, glyph })
                    })
                    .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<DraggedBlock>, _, cx| {
                        let (pos, b) = (ev.event.position, ev.bounds);
                        if b.contains(&pos) {
                            let upper = pos.y < b.origin.y + b.size.height / 2.0;
                            let at = if upper { ix } else { ix + 1 };
                            if this.block_drop != Some(at) {
                                this.block_drop = Some(at);
                                cx.notify();
                            }
                        }
                    }))
                    .child(div().text_color(theme.muted_foreground).child(icon("grip").small()))
                    .child(div().text_color(theme.muted_foreground).child(icon(glyph).small()))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(h_flex().gap_1().child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title)).when(warn, |d| {
                                d.child(div().text_color(theme.warning).child(icon("alert").xsmall()))
                            }))
                            .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(summary)),
                    )
                    .child(
                        h_flex()
                            .child(
                                Button::new(SharedString::from(format!("dup-{id}")))
                                    .icon(icon("copy"))
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Duplicate")
                                    .on_click(cx.listener(move |this, _, _, cx| this.studio.update(cx, |s, cx| s.duplicate(&dup_id, cx)))),
                            )
                            .child(
                                Button::new(SharedString::from(format!("del-{id}")))
                                    .icon(icon("trash"))
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Delete")
                                    .on_click(cx.listener(move |this, _, _, cx| this.studio.update(cx, |s, cx| s.remove_item(&del_id, cx)))),
                            ),
                    ),
            );
        }
        let list = list.child(line(n)).on_drop(cx.listener(|this, d: &DraggedBlock, _, cx| {
            if let Some(at) = this.block_drop.take() {
                let to = drop_position(d.from, at);
                this.studio.update(cx, |s, cx| s.move_to(&d.id, to, cx));
            }
            cx.notify();
        }));

        v_flex()
            .id("left")
            .size_full()
            .p_3()
            .gap_3()
            .bg(theme.sidebar)
            .overflow_y_scroll()
            .child(div().text_sm().text_color(theme.muted_foreground).child("INSERT"))
            .child(tiles)
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_sm().text_color(theme.muted_foreground).child(format!("BLOCKS ({n})")))
                    .when(n > 1, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child("Drag to reorder"))),
            )
            .when(n == 0, |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("The label is empty. Insert text, an icon or a QR code to start."),
                )
            })
            .child(list).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::drop_position;
    use crate::batch::ui_tests::new_studio;
    use gpui_kit::TestAppContext;

    #[test]
    fn drop_positions() {
        // Three blocks A B C; insertion indexes 0..=3.
        assert_eq!(drop_position(0, 0), 0); // A before A: stays
        assert_eq!(drop_position(0, 1), 0); // A after A: stays
        assert_eq!(drop_position(0, 3), 2); // A to the end
        assert_eq!(drop_position(2, 0), 0); // C to the front
        assert_eq!(drop_position(2, 2), 2); // C before C: stays
        assert_eq!(drop_position(1, 3), 2); // B to the end
    }

    #[gpui_kit::test]
    fn move_to_reorders_blocks(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "reorder");
        let ids = cx.update(|cx| {
            studio.update(cx, |s, cx| {
                s.insert(baradar_core::doc::ItemKind::Icon, |_| {}, cx);
                s.insert(baradar_core::doc::ItemKind::Shape, |_| {}, cx);
                s.doc.items.iter().map(|i| i.id().to_owned()).collect::<Vec<_>>()
            })
        });
        assert_eq!(ids.len(), 3);
        cx.update(|cx| studio.update(cx, |s, cx| s.move_to(&ids[0], 2, cx)));
        let now = cx.update(|cx| studio.read(cx).doc.items.iter().map(|i| i.id().to_owned()).collect::<Vec<_>>());
        assert_eq!(now, [ids[1].clone(), ids[2].clone(), ids[0].clone()]);
        assert!(cx.update(|cx| studio.read(cx).can_undo()));
    }
}
