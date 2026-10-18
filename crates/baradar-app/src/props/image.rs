//! Properties of an image block (port of `editor/props/ImageProps.svelte`): the source image
//! (choose / replace), dithering, tone adjustments and the image size.

use std::sync::Arc;

use baradar_core::doc::{DitherKind, ImageAdjust, ImageItem, Item};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::Button,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::PropertiesView;
use super::widgets::{Fields, NumSet, add_item_size, field, item_num_get, item_size_view, select, slider_view, switch_view};
use crate::studio::{Studio, StudioEvent, Tone};

/// Dithering choices (`DITHER`).
pub const DITHER: [(DitherKind, &str); 5] = [
    (DitherKind::Threshold, "Threshold (line art, logos)"),
    (DitherKind::FloydSteinberg, "Floyd–Steinberg (photos)"),
    (DitherKind::Atkinson, "Atkinson (high contrast)"),
    (DitherKind::Bayer4, "Ordered 4×4 (pattern)"),
    (DitherKind::Bayer8, "Ordered 8×8 (fine pattern)"),
];

/// "+n" for positive values (`format` of the Brightness / Contrast sliders).
pub fn signed(v: f64) -> String {
    let v = v.round() as i64;
    if v > 0 { format!("+{v}") } else { v.to_string() }
}

/// The MIME type of an image file by extension (as `StudioView::pick_image`).
pub fn image_mime(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    }
}

fn image_format(mime: &str) -> Option<ImageFormat> {
    Some(match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/bmp" => ImageFormat::Bmp,
        "image/svg+xml" => ImageFormat::Svg,
        _ => return None,
    })
}

/// Decodes standard base64 (padding optional); `None` on bad input.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Bytes and MIME type of a `data:image/…;base64,…` URL.
fn data_url_bytes(url: &str) -> Option<(Vec<u8>, String)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.strip_suffix(";base64")?;
    Some((base64_decode(data)?, mime.to_owned()))
}

pub struct ImageEditor {
    pub fields: Fields,
    studio: Entity<Studio>,
    /// Thumbnail of the current source, by blob ref (or data URL).
    thumb: Option<(String, Option<Arc<Image>>)>,
}

impl ImageEditor {
    pub fn new(item: &ImageItem, studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        let mut fields = Fields::default();
        let id = item.id.as_str();
        let adjust_get = |read: fn(&ImageAdjust) -> f64| {
            item_num_get(id, move |i| match i {
                Item::Image(img) => Some(read(&img.adjust)),
                _ => None,
            })
        };
        let adjust_set = |key: &'static str, write: fn(&mut ImageAdjust, f64)| -> NumSet {
            super::widgets::item_num_set(id, key, move |i, v| {
                if let Item::Image(img) = i {
                    write(&mut img.adjust, v);
                }
            })
        };
        fields.add_slider(
            "level",
            (0.0, 255.0, 1.0),
            studio,
            adjust_get(|a| f64::from(a.level)),
            adjust_set("level", |a, v| a.level = v.round().clamp(0.0, 255.0) as u8),
            window,
            cx,
        );
        fields.add_slider(
            "brightness",
            (-100.0, 100.0, 1.0),
            studio,
            adjust_get(|a| f64::from(a.brightness)),
            adjust_set("brightness", |a, v| a.brightness = v.round().clamp(-100.0, 100.0) as i32),
            window,
            cx,
        );
        fields.add_slider(
            "contrast",
            (-100.0, 100.0, 1.0),
            studio,
            adjust_get(|a| f64::from(a.contrast)),
            adjust_set("contrast", |a, v| a.contrast = v.round().clamp(-100.0, 100.0) as i32),
            window,
            cx,
        );
        fields.add_slider(
            "gamma",
            (30.0, 300.0, 5.0),
            studio,
            adjust_get(|a| f64::from(a.gamma_x100)),
            adjust_set("gamma", |a, v| a.gamma_x100 = v.round().clamp(30.0, 300.0) as u16),
            window,
            cx,
        );
        add_item_size(&mut fields, id, "Height", studio, window, cx);
        Self {
            fields,
            studio: studio.clone(),
            thumb: None,
        }
    }

    /// The thumbnail of the item's image, loaded once per source.
    fn thumbnail(&mut self, item: &ImageItem, cx: &App) -> Option<Arc<Image>> {
        let key = item.data_url.clone().unwrap_or_else(|| item.blob_ref.clone());
        if key.is_empty() {
            return None;
        }
        if self.thumb.as_ref().is_none_or(|(k, _)| *k != key) {
            let loaded = match &item.data_url {
                Some(url) => data_url_bytes(url),
                None => self.studio.read(cx).store.get_blob(&item.blob_ref),
            };
            let img = loaded.and_then(|(bytes, mime)| image_format(&mime).map(|f| Arc::new(Image::from_bytes(f, bytes))));
            self.thumb = Some((key, img));
        }
        self.thumb.as_ref().and_then(|(_, i)| i.clone())
    }

    /// Asks for an image file and makes it this block's image (mirrors `StudioView::pick_image`;
    /// "Could not use this image" when it cannot be stored).
    fn choose(studio: Entity<Studio>, item_id: String, cx: &mut App) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose image".into()),
        });
        cx.spawn(async move |cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let read = std::fs::read(&path);
            studio
                .update(cx, |s, cx| {
                    let stored = read
                        .map_err(anyhow::Error::from)
                        .and_then(|bytes| s.store.put_blob(&bytes, image_mime(&path)));
                    match stored {
                        Ok(r) => s.update_item(
                            &item_id,
                            None,
                            move |i| {
                                if let Item::Image(img) = i {
                                    img.blob_ref = r;
                                    img.data_url = None;
                                }
                            },
                            cx,
                        ),
                        Err(e) => cx.emit(StudioEvent::Toast {
                            tone: Tone::Error,
                            title: "Could not use this image".into(),
                            detail: Some(e.to_string()),
                        }),
                    }
                });
        })
        .detach();
    }

    pub fn render(&mut self, item: &ImageItem, _window: &mut Window, cx: &mut Context<PropertiesView>) -> AnyElement {
        let theme = cx.theme().clone();
        let thumb = self.thumbnail(item, cx);
        let max_mm = self.studio.read(cx).doc.tape.width_mm;
        let has_image = !item.blob_ref.is_empty() || item.data_url.is_some();
        let a = item.adjust;
        let set = |studio: Entity<Studio>, id: String, edit: fn(&mut ImageItem, bool)| {
            move |v: bool, _: &mut Window, cx: &mut App| {
                let id = id.clone();
                studio.update(cx, |s, cx| {
                    s.update_item(
                        &id.clone(),
                        Some(&format!("{id}:invert")),
                        |i| {
                            if let Item::Image(img) = i {
                                edit(img, v);
                            }
                        },
                        cx,
                    )
                })
            }
        };
        let (studio_c, id_c) = (self.studio.clone(), item.id.clone());
        let (studio_d, id_d) = (self.studio.clone(), item.id.clone());

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .size(px(56.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .overflow_hidden()
                            .map(|d| match thumb {
                                Some(img) => d.child(img_el(img)),
                                None => d.child(div().text_color(theme.muted_foreground).child("🖼")),
                            }),
                    )
                    .child(
                        Button::new("image-choose")
                            .icon(crate::icons::icon("upload")).label(if has_image { "Replace image…" } else { "Choose image…" })
                            .small()
                            .outline()
                            .on_click(move |_, _, cx| Self::choose(studio_c.clone(), id_c.clone(), cx)),
                    ),
            )
            .child(field(
                "Dithering",
                select(
                    "image-dither",
                    DITHER.iter().map(|(k, l)| (*k, SharedString::from(*l))).collect(),
                    Some(item.dither),
                    move |dither, _, cx| {
                        studio_d.update(cx, |s, cx| {
                            s.update_item(
                                &id_d,
                                None,
                                |i| {
                                    if let Item::Image(img) = i {
                                        img.dither = dither;
                                    }
                                },
                                cx,
                            )
                        })
                    },
                ),
                None,
                cx,
            ))
            .when(item.dither == DitherKind::Threshold, |d| {
                d.child(slider_view("Threshold", a.level.to_string(), self.fields.slider("level"), None, cx))
            })
            .child(slider_view("Brightness", signed(f64::from(a.brightness)), self.fields.slider("brightness"), None, cx))
            .child(slider_view("Contrast", signed(f64::from(a.contrast)), self.fields.slider("contrast"), None, cx))
            .child(slider_view(
                "Gamma",
                format!("{:.2}", f64::from(a.gamma_x100) / 100.0),
                self.fields.slider("gamma"),
                None,
                cx,
            ))
            .child(switch_view(
                "image-invert",
                "Invert",
                a.invert,
                None,
                set(self.studio.clone(), item.id.clone(), |img, v| img.adjust.invert = v),
                cx,
            ))
            .child(item_size_view(&item.id, "Image size", item.size, max_mm, &self.fields, &self.studio, cx))
            .into_any_element()
    }
}

fn img_el(image: Arc<Image>) -> AnyElement {
    img(image).size_full().object_fit(ObjectFit::Contain).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{base64_decode, data_url_bytes, signed};

    #[test]
    fn helpers() {
        assert_eq!(signed(5.0), "+5");
        assert_eq!(signed(0.0), "0");
        assert_eq!(signed(-7.0), "-7");
        assert_eq!(base64_decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        let (b, m) = data_url_bytes("data:image/png;base64,aGk=").unwrap_or_default();
        assert_eq!((b.as_slice(), m.as_str()), (&b"hi"[..], "image/png"));
        assert!(data_url_bytes("nope").is_none());
    }
}
