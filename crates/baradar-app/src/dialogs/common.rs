//! Pieces the Labels dialogs share: dates as the studio shows them (`Intl.DateTimeFormat` with
//! `dateStyle: 'medium'`), PNG thumbnails, confirmations (the studio's `window.confirm()`) and
//! toasts.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    notification::{Notification, NotificationType},
};
use gpui_kit::*;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::studio::Tone;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Oct 18, 2026" or, with `time`, "Oct 18, 2026, 3:04 PM" (en-US medium date, short time).
pub fn format_date_time(dt: OffsetDateTime, with_time: bool) -> String {
    let month = MONTHS[usize::from(u8::from(dt.month())) - 1];
    let day = format!("{month} {}, {}", dt.day(), dt.year());
    if !with_time {
        return day;
    }
    let (h, m) = (dt.hour(), dt.minute());
    let h12 = match h % 12 {
        0 => 12,
        n => n,
    };
    format!("{day}, {h12}:{m:02} {}", if h < 12 { "AM" } else { "PM" })
}

/// An ISO 8601 timestamp in local time; `None` when it does not parse.
fn local(iso: &str) -> Option<OffsetDateTime> {
    let dt = OffsetDateTime::parse(iso, &Rfc3339).ok()?;
    let offset = UtcOffset::local_offset_at(dt).unwrap_or(UtcOffset::UTC);
    Some(dt.to_offset(offset))
}

/// `when(iso)`: date and time, or "" for an unreadable timestamp.
pub fn when(iso: &str) -> String {
    local(iso).map(|d| format_date_time(d, true)).unwrap_or_default()
}

/// `when(iso, dayFmt)`: the date alone.
pub fn day(iso: &str) -> String {
    local(iso).map(|d| format_date_time(d, false)).unwrap_or_default()
}

/// A PNG (a stored thumbnail or a fresh export) as an image GPUI can draw.
pub fn png_image(bytes: Vec<u8>) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Png, bytes))
}

/// A thumbnail on a piece of white tape, or a tag icon when there is none.
pub fn thumb(image: Option<Arc<Image>>, w: Pixels, h: Pixels, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(w)
        .h(h)
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(gpui_kit::white())
        .overflow_hidden()
        .child(match image {
            Some(i) => img(i)
                .max_w(w - px(8.))
                .max_h(h - px(8.))
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => Icon::new(gpui_kit::assets::IconName::Tag)
                .large()
                .text_color(theme.muted_foreground)
                .into_any_element(),
        })
        .into_any_element()
}

/// A muted hint line.
pub fn hint(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// A section heading inside a dialog.
pub fn heading(text: impl Into<SharedString>) -> AnyElement {
    div().text_base().font_weight(FontWeight::SEMIBOLD).child(text.into()).into_any_element()
}

/// The studio's `confirm(question)`: an alert dialog on top of the current one; `on_ok` runs
/// when the user confirms.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    question: impl Into<SharedString>,
    ok_text: &'static str,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let question: SharedString = question.into();
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        alert
            .title(question.clone())
            .show_cancel(true)
            .ok_text(ok_text)
            .on_ok(move |_, window, cx| {
                on_ok(window, cx);
                true
            })
    });
}

/// A toast, like the studio's `toast(tone, title, detail)`.
pub fn toast(window: &mut Window, cx: &mut App, tone: Tone, title: impl Into<SharedString>, detail: Option<String>) {
    let kind = match tone {
        Tone::Info => NotificationType::Info,
        Tone::Ok => NotificationType::Success,
        Tone::Warn => NotificationType::Warning,
        Tone::Error => NotificationType::Error,
    };
    let mut n = Notification::new().title(title.into()).with_type(kind);
    if let Some(d) = detail.filter(|d| !d.is_empty()) {
        n = n.message(d);
    }
    window.push_notification(n, cx);
}

/// Where save dialogs start: `~/Documents` when it exists, else the home folder.
pub fn save_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let docs = home.join("Documents");
    if docs.is_dir() { docs } else { home }
}

#[cfg(test)]
mod tests {
    use super::format_date_time;
    use time::macros::datetime;

    #[test]
    fn formats_like_intl_medium() {
        assert_eq!(format_date_time(datetime!(2026-10-18 15:04 UTC), true), "Oct 18, 2026, 3:04 PM");
        assert_eq!(format_date_time(datetime!(2026-01-02 00:30 UTC), true), "Jan 2, 2026, 12:30 AM");
        assert_eq!(format_date_time(datetime!(2026-05-09 12:00 UTC), false), "May 9, 2026");
    }

    #[test]
    fn unreadable_dates_are_blank() {
        assert_eq!(super::when("yesterday"), "");
        assert!(!super::day("2026-10-18T10:00:00.000Z").is_empty());
    }
}
